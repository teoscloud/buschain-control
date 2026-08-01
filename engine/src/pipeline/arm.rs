//! Track / Master egress arming after wet (or dry) spine is ready.

use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::backend::{link_is_live, sink_exists, AudioBackend};
use crate::fx_gen::{live_fx_name, live_post_name};
use crate::host::registry;

/// Keep short — structural edits should not sit in dwell.
pub const WET_DWELL: Duration = Duration::from_millis(20);
pub const DRY_DWELL: Duration = Duration::from_millis(20);

/// Host-aware wet spine: bus.monitor → fx → post (no `{fx}_out`, no Pulse FX sink).
pub fn spine_instant_ready(bus: &str) -> bool {
    let from = format!("{bus}.monitor");
    let fx = live_fx_name(bus);
    let post = live_post_name(bus);

    if !sink_exists(&post) {
        return false;
    }

    // In-process host — duplex FX is not a Pulse sink and has no `{fx}_out` stream.
    if registry::host_running(bus) {
        return link_is_live(&from, &fx) && link_is_live(&fx, &post);
    }

    // Legacy leftover helpers (should be gone after orphan sweep).
    let fx_out = format!("{fx}_out");
    if sink_exists(&fx)
        && link_is_live(&from, &fx)
        && (link_is_live(&fx_out, &post) || link_is_live(&fx, &post))
    {
        return true;
    }
    false
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
    wait_stable(|| spine_instant_ready(bus), dwell, Duration::from_millis(400))
}

pub fn wait_dry_spine_stable(bus: &str, dwell: Duration) -> bool {
    wait_stable(
        || dry_spine_instant_ready(bus),
        dwell,
        Duration::from_millis(200),
    )
}

pub fn track_path_ready(bus: &str, wet: bool, _dests: &[String]) -> bool {
    if wet {
        spine_instant_ready(bus) || registry::host_running(bus)
    } else {
        dry_spine_instant_ready(bus)
    }
}

/// Silence bus monitor during cold rebuild (RAII-friendly free fn lives in backend).
///
/// Native unlink (registry ready) skips Pulse — optionally unload legacy loopbacks
/// so a Pulse DualMic→Master hop cannot outlive mixer mute. Idle paths must pass
/// `sweep_pulse=false` (Pulse list on every tick wedged the control plane).
pub fn disarm_track_egress(backend: &mut dyn AudioBackend, bus: &str, keep_fx_feed: bool) {
    disarm_track_egress_ex(backend, bus, keep_fx_feed, true);
}

/// Like [`disarm_track_egress`] with optional Pulse module sweep.
pub fn disarm_track_egress_ex(
    backend: &mut dyn AudioBackend,
    bus: &str,
    keep_fx_feed: bool,
    sweep_pulse: bool,
) {
    let from = format!("{bus}.monitor");
    let post = live_post_name(bus);
    let post_mon = format!("{post}.monitor");
    let fx = live_fx_name(bus);
    let _ = backend.unlink_from_source_except(&post_mon, &[]);
    if sweep_pulse {
        crate::backend::unload_legacy_from_source_except(&post_mon, &[]);
    }
    if keep_fx_feed && registry::host_running(bus) {
        let allow = [fx.as_str(), "buschain_hold"];
        let _ = backend.unlink_from_source_except(&from, &allow);
        if sweep_pulse {
            crate::backend::unload_legacy_from_source_except(&from, &allow);
        }
        let _ = backend.ensure_link_raw(&from, &fx);
    } else {
        let allow = ["buschain_hold"];
        let _ = backend.unlink_from_source_except(&from, &allow);
        if sweep_pulse {
            crate::backend::unload_legacy_from_source_except(&from, &allow);
        }
    }
    let _ = backend.ensure_link_raw(&from, "buschain_hold");
}

/// True when every existing dest is linked from `src` (vin feed alone must not
/// count as healthy when Master is also in Desired egress).
fn dests_linked(src: &str, dests: &[String]) -> bool {
    let mut any = false;
    for d in dests {
        if d.is_empty() || !sink_exists(d) {
            continue;
        }
        any = true;
        if !link_is_live(src, d) {
            return false;
        }
    }
    any
}

fn arm_dry_to_dests(
    backend: &mut dyn AudioBackend,
    from: &str,
    post_mon: &str,
    dests: &[String],
) -> Result<()> {
    if dests_linked(from, dests) {
        let _ = backend.ensure_link_raw(from, "buschain_hold");
        return Ok(());
    }
    let _ = backend.unlink_from_source_except(post_mon, &[]);
    let allow: Vec<&str> = dests
        .iter()
        .map(|s| s.as_str())
        .chain(std::iter::once("buschain_hold"))
        .collect();
    let _ = backend.unlink_from_source_except(from, &allow);
    let _ = backend.ensure_link_raw(from, "buschain_hold");
    for d in dests {
        if d.is_empty() || !sink_exists(d) {
            continue;
        }
        backend.ensure_link_raw(from, d)?;
    }
    Ok(())
}

/// Exclusive arm: egress_source → each dest (plus hold on bus if source is bus).
///
/// Wet is only taken when the post spine can actually carry audio. Otherwise we
/// keep/restore dry bus→dest — never leave hold-only after a failed wet cutover
/// (mute/unmute + FX half-up was silencing DualMic while meters still moved).
pub fn arm_track_egress(
    backend: &mut dyn AudioBackend,
    bus: &str,
    wet: bool,
    dests: &[String],
) -> Result<()> {
    let from = format!("{bus}.monitor");
    let post = live_post_name(bus);
    let post_mon = format!("{post}.monitor");
    let fx = live_fx_name(bus);

    let wet_ready = wet && sink_exists(&post) && spine_instant_ready(bus);

    if wet_ready {
        // Fast path: wet exclusive already up — skip unlink storms.
        if link_is_live(&from, &fx)
            && link_is_live(&fx, &post)
            && dests_linked(&post_mon, dests)
            && !dests.iter().any(|d| !d.is_empty() && link_is_live(&from, d))
        {
            let _ = backend.ensure_link_raw(&from, "buschain_hold");
            return Ok(());
        }
        // Exclusive wet: drop dry bus→dest beside post→dest.
        let _ = backend.unlink_from_source_except(&from, &[fx.as_str(), "buschain_hold"]);
        let _ = backend.ensure_link_raw(&from, &fx);
        let _ = backend.ensure_link_raw(&from, "buschain_hold");
        let _ = backend.ensure_link_raw(&fx, &post);
        let allow: Vec<&str> = dests
            .iter()
            .map(|s| s.as_str())
            .chain(std::iter::once("buschain_hold"))
            .collect();
        let _ = backend.unlink_from_source_except(&post_mon, &allow);
        let mut wet_ok = false;
        for d in dests {
            if d.is_empty() || !sink_exists(d) {
                continue;
            }
            match backend.ensure_link_raw(&post_mon, d) {
                Ok(_) => wet_ok = true,
                Err(_) => {}
            }
        }
        if wet_ok && dests_linked(&post_mon, dests) {
            return Ok(());
        }
        // Wet cutover failed — restore dry so the track is audible again.
    }

    arm_dry_to_dests(backend, &from, &post_mon, dests)
}

/// Soft cutover: keep dry audible until post→dest is up, then drop dry.
pub fn arm_track_egress_soft_cutover(
    backend: &mut dyn AudioBackend,
    bus: &str,
    dests: &[String],
) -> Result<()> {
    let from = format!("{bus}.monitor");
    let post = live_post_name(bus);
    let post_mon = format!("{post}.monitor");
    let fx = live_fx_name(bus);

    let mut bus_keep: Vec<&str> = vec![fx.as_str(), "buschain_hold"];
    for d in dests {
        if !d.is_empty() {
            bus_keep.push(d.as_str());
        }
    }
    let _ = backend.unlink_from_source_except(&from, &bus_keep);
    let _ = backend.ensure_link_raw(&from, &fx);
    let _ = backend.ensure_link_raw(&from, "buschain_hold");
    let _ = backend.ensure_link_raw(&fx, &post);
    for d in dests {
        if d.is_empty() || !sink_exists(d) {
            continue;
        }
        let _ = backend.ensure_link_raw(&from, d);
    }

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

    for d in dests {
        if d.is_empty() {
            continue;
        }
        let _ = link_is_live(&post_mon, d);
        let _ = backend.unlink_raw(&from, d);
    }
    let _ = backend.unlink_from_source_except(&from, &[fx.as_str(), "buschain_hold"]);
    let _ = backend.ensure_link_raw(&from, &fx);
    let _ = backend.ensure_link_raw(&from, "buschain_hold");
    Ok(())
}

pub fn disarm_master_hw(backend: &mut dyn AudioBackend, hw: &str) {
    let from = "buschain_master.monitor";
    let post = live_post_name("buschain_master");
    let post_mon = format!("{post}.monitor");
    let fx = live_fx_name("buschain_master");
    if registry::host_running("buschain_master") {
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
