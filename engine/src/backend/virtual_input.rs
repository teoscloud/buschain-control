//! System virtual input: feed null-sink + Pulse remap-source (Audio/Source).
//!
//! Apps capture from `buschain_vin_{suffix}`; egress arms into
//! `buschain_vinf_{suffix}` (same wet/dry tap as track→track routing).

use anyhow::{anyhow, Context, Result};

use crate::contract::ClockProps;
use crate::domain::{NodeName, NodeRole, NodeSpec};

use super::null_sink::{ensure_null_sink, push_description, unload_named_null_sink};

fn run_ok(bin: &str, args: &[&str]) -> Result<()> {
    let out = std::process::Command::new(bin)
        .args(args)
        .output()
        .with_context(|| format!("spawn {bin}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "{bin} {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// `(vin_source, vinf_feed)` for a track bus, or `None` for master / non-track.
pub fn names_for_bus(bus: &str) -> Option<(String, String)> {
    let suffix = bus.strip_prefix("buschain_track_")?;
    if suffix.is_empty() {
        return None;
    }
    Some((
        format!("buschain_vin_{suffix}"),
        format!("buschain_vinf_{suffix}"),
    ))
}

pub fn is_virtual_input_source(name: &str) -> bool {
    name.starts_with("buschain_vin_")
}

pub fn is_virtual_input_feed(name: &str) -> bool {
    name.starts_with("buschain_vinf_")
}

fn source_exists(name: &str) -> bool {
    let Ok(out) = std::process::Command::new("pactl")
        .args(["list", "short", "sources"])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|line| line.split('\t').nth(1) == Some(name))
}

fn module_args_source_name(args: &str, name: &str) -> bool {
    for tok in args.split_whitespace() {
        if let Some(v) = tok.strip_prefix("source_name=") {
            if v == name {
                return true;
            }
        }
    }
    false
}

fn unload_remap_source(name: &str) -> Result<()> {
    let out = std::process::Command::new("pactl")
        .args(["list", "short", "modules"])
        .output()
        .context("pactl list modules")?;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.splitn(3, '\t');
        let Some(idx) = parts.next() else { continue };
        let Some(mod_name) = parts.next() else { continue };
        let args = parts.next().unwrap_or("");
        if mod_name == "module-remap-source" && module_args_source_name(args, name) {
            let _ = std::process::Command::new("pactl")
                .args(["unload-module", idx])
                .output();
        }
    }
    Ok(())
}

fn sanitize_desc(d: &str) -> String {
    d.replace(' ', "_").replace('=', "_")
}

/// True when `feed.monitor` is linked into the remap capture stream.
fn remap_hears_feed(feed: &str, vin: &str) -> bool {
    let feed_mon = format!("{feed}.monitor");
    let input = format!("input.{vin}");
    if super::native::native_ready() {
        if super::native::native_link_is_live(&feed_mon, &input)
            || super::native::native_link_is_live(&feed_mon, vin)
        {
            return true;
        }
        // Native registry is authoritative when up — missing hop means bounce.
        return false;
    }
    // Pulse-only fallback: require an explicit monitor→input.vin adjacency.
    let Ok(out) = std::process::Command::new("pw-link")
        .args(["-l"])
        .output()
    else {
        return false;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mon_fl = format!("{feed}:monitor_FL");
    let mut under_mon = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with(&mon_fl) && !t.contains("|->") && !t.contains("|<-") {
            under_mon = true;
            continue;
        }
        if under_mon {
            if t.starts_with("|->") {
                if t.contains(&input) || t.contains(vin) {
                    return true;
                }
                continue;
            }
            if !t.starts_with("|") {
                under_mon = false;
            }
        }
    }
    false
}

/// Ensure feed null-sink + remap-source for `bus` (`buschain_track_*`).
pub fn ensure_virtual_input(bus: &str, description: &str, clock: &ClockProps) -> Result<()> {
    let Some((vin, feed)) = names_for_bus(bus) else {
        return Err(anyhow!("virtual input only for buschain_track_* (got {bus})"));
    };

    let feed_spec = NodeSpec {
        name: NodeName::new(&feed),
        description: format!("{description}_VinFeed"),
        role: NodeRole::VirtualInputFeed,
        start_muted: false,
        pulse_export: false,
    };
    ensure_null_sink(&feed_spec, clock)?;
    // Feed must pass audio. Internal feeds are absent from pactl — use native.
    if super::native::native_ready() {
        let _ = super::native::native_set_levels(&feed, 0.0, false);
    } else {
        let _ = run_ok("pactl", &["set-sink-mute", &feed, "0"]);
        let _ = run_ok("pactl", &["set-sink-volume", &feed, "100%"]);
    }

    let master = format!("{feed}.monitor");
    let desc = sanitize_desc(description);

    if source_exists(&vin) {
        // Remap module can stay loaded while feed.monitor→input.vin links were
        // stripped (anti-Master wipe). Bounce so WirePlumber reattaches capture.
        if remap_hears_feed(&feed, &vin) {
            let _ = push_description(&vin, &desc);
            return Ok(());
        }
        let _ = unload_remap_source(&vin);
        std::thread::sleep(std::time::Duration::from_millis(40));
    }

    let props = format!(
        "device.description={desc} media.name=buschain-control node.virtual=true"
    );
    run_ok(
        "pactl",
        &[
            "load-module",
            "module-remap-source",
            &format!("source_name={vin}"),
            &format!("master={master}"),
            "channels=2",
            "channel_map=front-left,front-right",
            &format!("source_properties={props}"),
        ],
    )
    .with_context(|| format!("load module-remap-source {vin} master={master}"))?;

    // Brief settle — Pulse may list the source a tick later.
    for _ in 0..20 {
        if source_exists(&vin) {
            let _ = push_description(&vin, &desc);
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    if source_exists(&vin) {
        let _ = push_description(&vin, &desc);
        Ok(())
    } else {
        Err(anyhow!("remap-source {vin} did not appear after load"))
    }
}

/// Unload remap-source + feed null-sink for a track bus.
pub fn teardown_virtual_input(bus: &str) -> Result<()> {
    let Some((vin, feed)) = names_for_bus(bus) else {
        return Ok(());
    };
    let _ = unload_remap_source(&vin);
    let _ = unload_named_null_sink(&feed);
    Ok(())
}

/// Push human description onto an existing vin source (track rename).
pub fn push_virtual_input_description(bus: &str, description: &str) -> Result<()> {
    let Some((vin, _)) = names_for_bus(bus) else {
        return Ok(());
    };
    if source_exists(&vin) {
        push_description(&vin, description)?;
    }
    Ok(())
}
