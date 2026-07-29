//! Clocked null-sink create / unload / migrate-recreate.

use anyhow::{anyhow, Context, Result};

use crate::clock::probe_sink_running_rate;
use crate::contract::ClockProps;
use crate::domain::{NodeRole, NodeSpec};

pub fn ensure_null_sink(spec: &NodeSpec, clock: &ClockProps) -> Result<()> {
    let name = spec.name.as_str();
    let is_app_bus = matches!(spec.role, NodeRole::TrackBus | NodeRole::MasterBus);
    if sink_exists(name) {
        let live = probe_sink_running_rate(name);
        // App buses: never auto-migrate here (cork risk / idle thrash).
        // Rate retarget is BindMasterClock → migrate_recreate_null_sink only.
        if is_app_bus {
            let _ = push_clock_props(name, clock);
            // Keep device.description in sync with session track renames.
            let _ = push_description(name, &spec.description);
            return Ok(());
        }
        if sink_is_stereo_fl_fr(name) && live == Some(clock.sample_rate) {
            let _ = push_clock_props(name, clock);
            let _ = push_description(name, &spec.description);
            return Ok(());
        }
        // Helpers (post / hold / rate-bridge): recreate bad layouts / wrong rate.
        let _ = unload_named_null_sink(name);
        std::thread::sleep(std::time::Duration::from_millis(30));
    }

    create_null_sink(spec, clock)
}

/// Unload + recreate an app bus at a new GraphClock, moving sink-inputs via `buschain_hold`.
pub fn migrate_recreate_null_sink(spec: &NodeSpec, clock: &ClockProps) -> Result<()> {
    let name = spec.name.as_str();
    if !sink_exists(name) {
        return create_null_sink(spec, clock);
    }
    if probe_sink_running_rate(name) == Some(clock.sample_rate) {
        let _ = push_clock_props(name, clock);
        return Ok(());
    }

    // Parking lot for streams during recreate (must exist).
    let hold_spec = NodeSpec {
        name: crate::domain::NodeName::new("buschain_hold"),
        description: "BusChainControl_Hold".into(),
        role: NodeRole::Hold,
        start_muted: true,
    };
    if !sink_exists("buschain_hold") {
        let _ = create_null_sink(&hold_spec, clock);
    }

    let inputs = list_sink_input_indices_on(name);
    for idx in &inputs {
        let _ = run_ok("pactl", &["move-sink-input", idx, "buschain_hold"]);
    }

    let _ = unload_named_null_sink(name);
    std::thread::sleep(std::time::Duration::from_millis(50));

    create_null_sink(spec, clock)?;

    // App buses must be audible after migrate (levels reconcile will refine).
    if matches!(spec.role, NodeRole::TrackBus | NodeRole::MasterBus) {
        let _ = run_ok("pactl", &["set-sink-mute", name, "0"]);
        let _ = run_ok("pactl", &["set-sink-volume", name, "100%"]);
        let _ = run_ok("pactl", &["suspend-sink", name, "0"]);
    }

    for idx in &inputs {
        let _ = run_ok("pactl", &["move-sink-input", idx, name]);
    }

    // Confirm rate landed.
    for _ in 0..20 {
        if probe_sink_running_rate(name) == Some(clock.sample_rate) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Ok(())
}

fn create_null_sink(spec: &NodeSpec, clock: &ClockProps) -> Result<()> {
    let name = spec.name.as_str();
    let is_app_bus = matches!(spec.role, NodeRole::TrackBus | NodeRole::MasterBus);
    let props = format!(
        "sink_properties=device.description={} media.name=buschain-control session.suspend-timeout-seconds={} node.virtual=true node.latency={} audio.rate={} node.force-quantum={} node.lock-quantum={}",
        sanitize_desc(&spec.description),
        clock.suspend_timeout,
        clock.node_latency,
        clock.sample_rate,
        clock.quantum,
        if clock.soft_quantum { "false" } else { "true" },
    );

    run_ok(
        "pactl",
        &[
            "load-module",
            "module-null-sink",
            &format!("sink_name={name}"),
            &format!("rate={}", clock.sample_rate),
            "channels=2",
            "channel_map=front-left,front-right",
            &props,
        ],
    )?;

    if is_app_bus || spec.start_muted {
        let _ = run_ok("pactl", &["set-sink-mute", name, "1"]);
        let _ = run_ok("pactl", &["set-sink-volume", name, "0%"]);
    } else {
        let _ = run_ok("pactl", &["set-sink-volume", name, "100%"]);
        let _ = run_ok("pactl", &["set-sink-mute", name, "0"]);
    }
    Ok(())
}

fn sink_index_for_name(sink_name: &str) -> Option<String> {
    let out = std::process::Command::new("pactl")
        .args(["list", "short", "sinks"])
        .output()
        .ok()?;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.split('\t');
        let idx = parts.next()?;
        let name = parts.next()?;
        if name == sink_name {
            return Some(idx.to_string());
        }
    }
    None
}

fn list_sink_input_indices_on(sink_name: &str) -> Vec<String> {
    // PipeWire short format: idx \t sink_idx \t client \t driver \t spec
    let Some(sink_idx) = sink_index_for_name(sink_name) else {
        return Vec::new();
    };
    let Ok(out) = std::process::Command::new("pactl")
        .args(["list", "short", "sink-inputs"])
        .output()
    else {
        return Vec::new();
    };
    let mut idxs = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.split('\t');
        let Some(idx) = parts.next() else { continue };
        let Some(si) = parts.next() else { continue };
        if si == sink_idx {
            idxs.push(idx.to_string());
        }
    }
    idxs
}

fn sanitize_desc(d: &str) -> String {
    d.replace(' ', "_").replace('=', "_")
}

fn push_clock_props(name: &str, clock: &ClockProps) -> Result<()> {
    let body = format!(
        "{{ node.latency = \"{}\" audio.rate = {} node.force-quantum = {} node.lock-quantum = {} }}",
        clock.node_latency,
        clock.sample_rate,
        clock.quantum,
        if clock.soft_quantum { "false" } else { "true" },
    );
    let _ = std::process::Command::new("pw-cli")
        .args(["s", name, "Props", &body])
        .output();
    Ok(())
}

/// Update the human label on an existing null-sink (session rename path).
pub fn push_description(name: &str, description: &str) -> Result<()> {
    let desc = sanitize_desc(description);
    let body = format!("{{ device.description = \"{}\" }}", desc.replace('\"', "\\\""));
    let _ = std::process::Command::new("pw-cli")
        .args(["s", name, "Props", &body])
        .output();
    Ok(())
}

pub fn unload_named_null_sink(name: &str) -> Result<()> {
    let out = std::process::Command::new("pactl")
        .args(["list", "short", "modules"])
        .output()
        .context("pactl list modules")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let needle = format!("sink_name={name}");
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let Some(idx) = parts.next() else { continue };
        let Some(mod_name) = parts.next() else { continue };
        let args = parts.next().unwrap_or("");
        if mod_name == "module-null-sink" && args.contains(&needle) {
            let _ = std::process::Command::new("pactl")
                .args(["unload-module", idx])
                .status();
            return Ok(());
        }
    }
    Ok(())
}

fn sink_exists(name: &str) -> bool {
    let Ok(out) = std::process::Command::new("pactl")
        .args(["list", "short", "sinks"])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|l| l.split('\t').nth(1) == Some(name))
}

fn sink_is_stereo_fl_fr(name: &str) -> bool {
    let Ok(out) = std::process::Command::new("pactl")
        .args(["list", "sinks"])
        .output()
    else {
        return true;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut in_sink = false;
    for line in text.lines() {
        if line.starts_with("Sink #") {
            in_sink = false;
        }
        if line.contains("Name:") && line.contains(name) {
            in_sink = true;
        }
        if in_sink && line.contains("channel map:") {
            let map = line.to_lowercase();
            return map.contains("front-left") && map.contains("front-right");
        }
    }
    true
}

fn run_ok(bin: &str, args: &[&str]) -> Result<()> {
    let out = std::process::Command::new(bin)
        .args(args)
        .output()
        .with_context(|| format!("spawn {bin}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "{bin} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}
