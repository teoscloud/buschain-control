//! End-to-end track arming — disarm egress until spine is stable, then arm dests.
//!
//! Apps stay on the bus (never cork). Keepalive `{bus}.monitor → buschain_hold` always stays.

use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::backend::{link_is_live, sink_exists, sink_has_input, AudioBackend};
use crate::domain::{fx_name_for_bus, post_name_for_bus};

/// Continuous ready window before arming wet egress (flap resets).
/// Keep short — add/remove ForceRespawn holds the track silent until this passes.
pub const WET_DWELL: Duration = Duration::from_millis(60);
/// Light existence check for dry (no-FX) tracks.
pub const DRY_DWELL: Duration = Duration::from_millis(40);

/// Wet spine ready *without* requiring post→dest yet (pre-arm probe).
pub fn spine_instant_ready(bus: &str) -> bool {
    let from = format!("{bus}.monitor");
    let fx = fx_name_for_bus(bus);
    let post = post_name_for_bus(bus);
    let fx_out = format!("{fx}_out");
    if !sink_exists(&fx) || !sink_exists(&post) {
        return false;
    }
    if !link_is_live(&from, &fx) {
        return false;
    }
    // `pactl sink-inputs` flakes when the filter-chain helper is IDLE/SUSPENDED
    // even though `pw-link` still has `{fx}_out → post`. That false-negative used
    // to block Master post→HW repair (meters via hold/bus, audible silence).
    sink_has_input(&post) || link_is_live(&fx_out, &post)
}

/// Dry spine: bus exists (egress source is bus.monitor).
pub fn dry_spine_instant_ready(bus: &str) -> bool {
    sink_exists(bus)
}

/// Wait until `probe` stays true continuously for `dwell`. Any flap resets the timer.
pub fn wait_stable(mut probe: impl FnMut() -> bool, dwell: Duration, max_wait: Duration) -> bool {
    let deadline = Instant::now() + max_wait;
    let mut stable_since: Option<Instant> = None;
    while Instant::now() < deadline {
        if probe() {
            let since = *stable_since.get_or_insert_with(Instant::now);
            if Instant::now().duration_since(since) >= dwell {
                return true;
            }
        } else {
            stable_since = None;
        }
        thread::sleep(Duration::from_millis(5));
    }
    false
}

pub fn wait_spine_stable(bus: &str, dwell: Duration) -> bool {
    // Short max — long waits here HOL the worker and make knobs feel dead.
    wait_stable(|| spine_instant_ready(bus), dwell, Duration::from_millis(500))
}

pub fn wait_dry_spine_stable(bus: &str, dwell: Duration) -> bool {
    wait_stable(|| dry_spine_instant_ready(bus), dwell, Duration::from_secs(2))
}

/// Spine ready + every configured destination sink exists.
pub fn track_path_ready(bus: &str, wet: bool, dests: &[String]) -> bool {
    if wet {
        if !spine_instant_ready(bus) {
            return false;
        }
    } else if !dry_spine_instant_ready(bus) {
        return false;
    }
    for d in dests {
        if d.is_empty() || !sink_exists(d) {
            return false;
        }
    }
    true
}

/// Drop wet egress (post→*). Optionally keep a dry `bus→dest` bridge and/or FX feed.
///
/// Pass `keep_dry_dest` during ForceRespawn rebuilds so apps (e.g. Chromium) keep
/// hearing audio instead of a silent gap that stalls A/V.
pub fn disarm_track_egress(backend: &mut dyn AudioBackend, bus: &str, keep_fx_feed: bool) {
    disarm_track_egress_ex(backend, bus, keep_fx_feed, None);
}

pub fn disarm_track_egress_ex(
    backend: &mut dyn AudioBackend,
    bus: &str,
    keep_fx_feed: bool,
    keep_dry_dest: Option<&str>,
) {
    let from = format!("{bus}.monitor");
    let post = post_name_for_bus(bus);
    let post_mon = format!("{post}.monitor");
    let fx = fx_name_for_bus(bus);

    let mut keep: Vec<&str> = vec!["buschain_hold"];
    if keep_fx_feed && sink_exists(&fx) {
        keep.push(fx.as_str());
    }
    if let Some(d) = keep_dry_dest {
        if !d.is_empty() {
            keep.push(d);
        }
    }
    let _ = backend.unlink_from_source_except(&from, &keep);
    let _ = backend.unlink_from_source_except(&post_mon, &[]);
    let _ = backend.ensure_link_raw(&from, "buschain_hold");
    if let Some(d) = keep_dry_dest {
        if !d.is_empty() {
            let _ = backend.ensure_link_raw(&from, d);
        }
    }
}

/// Exclusive arm: egress_source → each dest (plus hold on bus if source is bus).
pub fn arm_track_egress(
    backend: &mut dyn AudioBackend,
    bus: &str,
    wet: bool,
    dests: &[String],
) -> Result<()> {
    let from = format!("{bus}.monitor");
    let post = post_name_for_bus(bus);
    let post_mon = format!("{post}.monitor");
    let fx = fx_name_for_bus(bus);

    if wet {
        let _ = backend.unlink_from_source_except(&from, &[fx.as_str(), "buschain_hold"]);
        let _ = backend.ensure_link_raw(&from, &fx);
        let _ = backend.ensure_link_raw(&from, "buschain_hold");
        let allow: Vec<&str> = dests
            .iter()
            .map(|s| s.as_str())
            .chain(std::iter::once("buschain_hold"))
            .collect();
        let _ = backend.unlink_from_source_except(&post_mon, &allow);
        for d in dests {
            if d.is_empty() || !sink_exists(d) {
                continue;
            }
            backend.ensure_link_raw(&post_mon, d)?;
        }
    } else {
        let _ = backend.unlink_from_source_except(&post_mon, &[]);
        let allow: Vec<&str> = dests
            .iter()
            .map(|s| s.as_str())
            .chain(std::iter::once("buschain_hold"))
            .collect();
        let _ = backend.unlink_from_source_except(&from, &allow);
        let _ = backend.ensure_link_raw(&from, "buschain_hold");
        for d in dests {
            if d.is_empty() || !sink_exists(d) {
                continue;
            }
            backend.ensure_link_raw(&from, d)?;
        }
    }
    Ok(())
}

pub fn disarm_master_hw(backend: &mut dyn AudioBackend, hw: &str) {
    let from = "buschain_master.monitor";
    let post = post_name_for_bus("buschain_master");
    let post_mon = format!("{post}.monitor");
    let fx = fx_name_for_bus("buschain_master");
    if sink_exists(&fx) {
        let _ = backend.unlink_from_source_except(from, &[fx.as_str(), "buschain_hold"]);
    } else {
        let _ = backend.unlink_from_source_except(from, &["buschain_hold"]);
    }
    let _ = backend.unlink_raw(from, hw);
    let _ = backend.unlink_from_source_except(&post_mon, &[]);
    let _ = backend.unlink_raw(&post_mon, hw);
    let _ = backend.ensure_link_raw(from, "buschain_hold");
}

pub fn arm_master_hw(backend: &mut dyn AudioBackend, hw: &str, wet: bool) -> Result<()> {
    if hw.is_empty() || !sink_exists(hw) {
        return Ok(());
    }
    let dests = vec![hw.to_string()];
    arm_track_egress(backend, "buschain_master", wet, &dests)
}
