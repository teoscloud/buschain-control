//! Track / Master egress arming after wet (or dry) spine is ready.

use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::backend::{
    egress_allow_target, egress_hop_live, ensure_egress_clocked_route, is_physical_hw_dest,
    link_is_live, sink_exists, AudioBackend,
};
use crate::clock::GraphClock;
use crate::domain::mtr_name_for_bus;
use crate::fx_gen::{live_fx_name, live_post_name};
use crate::host::registry;
use crate::pipeline::glc;
use crate::plan::DesiredState;

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
    let glc_name = glc::glc_name_for_bus(bus);
    let _ = backend.unlink_from_source_except(&post_mon, &[]);
    if sweep_pulse {
        crate::backend::unload_legacy_from_source_except(&post_mon, &[]);
    }
    // Master-edge GLC is owned by pipeline::glc; drop post/bus → glc on disarm.
    let _ = backend.unlink_raw(&post_mon, &glc_name);
    let _ = backend.unlink_raw(&from, &glc_name);
    let mtr = mtr_name_for_bus(bus);
    if keep_fx_feed && registry::host_running(bus) {
        let allow = [fx.as_str(), "buschain_hold", mtr.as_str()];
        let _ = backend.unlink_from_source_except(&from, &allow);
        if sweep_pulse {
            crate::backend::unload_legacy_from_source_except(&from, &allow);
        }
        let _ = backend.ensure_link_raw(&from, &fx);
    } else {
        let allow = ["buschain_hold", mtr.as_str()];
        let _ = backend.unlink_from_source_except(&from, &allow);
        if sweep_pulse {
            crate::backend::unload_legacy_from_source_except(&from, &allow);
        }
    }
    let _ = backend.ensure_link_raw(&from, "buschain_hold");
}

/// True when every existing dest is linked from `src` (direct or egress bridge).
fn dests_linked(src: &str, dests: &[String], desired: &DesiredState) -> bool {
    let mut any = false;
    for d in dests {
        if d.is_empty() || !sink_exists(d) {
            continue;
        }
        any = true;
        if !egress_hop_live(src, d, desired) {
            return false;
        }
    }
    any
}

/// Link `source` → dest (BusChain direct, or Master→HW via `buschain_rs_out_*`).
fn link_to_dest(
    backend: &mut dyn AudioBackend,
    desired: &mut DesiredState,
    clock: &GraphClock,
    source: &str,
    dest: &str,
) -> Result<()> {
    if dest.is_empty() || !sink_exists(dest) {
        return Ok(());
    }
    if is_physical_hw_dest(dest) {
        ensure_egress_clocked_route(backend, source, dest, clock, desired)
    } else {
        backend.ensure_link_raw(source, dest)?;
        Ok(())
    }
}

fn allow_for_dests(source: &str, dests: &[String], desired: &DesiredState) -> Vec<String> {
    dests
        .iter()
        .filter(|d| !d.is_empty())
        .map(|d| {
            if is_physical_hw_dest(d) {
                egress_allow_target(source, d, desired)
            } else {
                d.clone()
            }
        })
        .collect()
}

/// Desired egress → physical link targets (Master-edge GLC when δ>0).
fn physical_dests(bus: &str, dests: &[String]) -> Vec<String> {
    glc::physical_egress_dests(bus, dests)
}

/// True dry: no FX host.
///
/// When a post fader stage exists: `bus.monitor → post → dests` (volume last).
/// Otherwise legacy `bus.monitor → dests`.
fn arm_dry_to_dests(
    backend: &mut dyn AudioBackend,
    desired: &mut DesiredState,
    clock: &GraphClock,
    bus: &str,
    from: &str,
    post_mon: &str,
    dests: &[String],
) -> Result<()> {
    let mtr = mtr_name_for_bus(bus);
    let post = live_post_name(bus);
    if sink_exists(&post) {
        // Already correctly armed through the fader stage?
        if dests_linked(post_mon, dests, desired)
            && link_is_live(from, &post)
            && !dests
                .iter()
                .any(|d| !d.is_empty() && link_is_live(from, d))
        {
            let _ = backend.ensure_link_raw(from, "buschain_hold");
            return Ok(());
        }
        let allow_bus = [post.as_str(), "buschain_hold", mtr.as_str()];
        let _ = backend.unlink_from_source_except(from, &allow_bus);
        let _ = backend.ensure_link_raw(from, "buschain_hold");
        let _ = backend.ensure_link_raw(from, &post);
        for d in dests {
            link_to_dest(backend, desired, clock, post_mon, d)?;
        }
        let allow_owned = allow_for_dests(post_mon, dests, desired);
        let allow_post: Vec<&str> = allow_owned
            .iter()
            .map(|s| s.as_str())
            .chain(["buschain_hold"])
            .collect();
        let _ = backend.unlink_from_source_except(post_mon, &allow_post);
        return Ok(());
    }
    if dests_linked(from, dests, desired)
        && !dests
            .iter()
            .any(|d| !d.is_empty() && egress_hop_live(post_mon, d, desired))
    {
        let _ = backend.ensure_link_raw(from, "buschain_hold");
        return Ok(());
    }
    let _ = backend.unlink_from_source_except(post_mon, &[]);
    for d in dests {
        link_to_dest(backend, desired, clock, from, d)?;
    }
    let allow_owned = allow_for_dests(from, dests, desired);
    let allow: Vec<&str> = allow_owned
        .iter()
        .map(|s| s.as_str())
        .chain(["buschain_hold", mtr.as_str()])
        .collect();
    let _ = backend.unlink_from_source_except(from, &allow);
    let _ = backend.ensure_link_raw(from, "buschain_hold");
    Ok(())
}

/// Audible dry while FX spine is settling — **must keep bus→fx** or the sealed
/// chain can never form (dry unlink used to strip fx permanently).
///
/// Dry parallel prefers `bus→post→dest` (fader last) when post exists; falls
/// back to `bus→dest` only when there is no post stage yet.
fn arm_dry_preserving_fx(
    backend: &mut dyn AudioBackend,
    desired: &mut DesiredState,
    clock: &GraphClock,
    bus: &str,
    from: &str,
    post_mon: &str,
    fx: &str,
    dests: &[String],
) -> Result<()> {
    let mtr = mtr_name_for_bus(bus);
    let post = live_post_name(bus);
    let _ = backend.ensure_link_raw(from, fx);
    let _ = backend.ensure_link_raw(from, "buschain_hold");
    if sink_exists(&post) {
        // Bypass-FX dry into the fader stage until wet fx→post lands.
        let _ = backend.ensure_link_raw(from, &post);
        let allow: Vec<&str> = [fx, post.as_str(), "buschain_hold", mtr.as_str()]
            .into_iter()
            .collect();
        let _ = backend.unlink_from_source_except(from, &allow);
        for d in dests {
            if d.is_empty() || !sink_exists(d) {
                continue;
            }
            // Drop any pre-fader bus→dest leak.
            if link_is_live(from, d) {
                let _ = backend.unlink_raw(from, d);
            }
            let _ = link_to_dest(backend, desired, clock, post_mon, d);
        }
        return Ok(());
    }
    // Don't strip post→dest if somehow live — soft path owns that.
    for d in dests {
        if d.is_empty() || !sink_exists(d) {
            continue;
        }
        // Per-dest cutover: once this dest hears the wet hop, dry parallel
        // there is double audio — keep dry only for dests still waiting.
        if egress_hop_live(post_mon, d, desired) {
            let _ = backend.unlink_raw(from, d);
            continue;
        }
        let _ = link_to_dest(backend, desired, clock, from, d);
    }
    let allow_owned = allow_for_dests(from, dests, desired);
    let allow: Vec<&str> = allow_owned
        .iter()
        .map(|s| s.as_str())
        .chain([fx, "buschain_hold", mtr.as_str()])
        .collect();
    let _ = backend.unlink_from_source_except(from, &allow);
    Ok(())
}

/// Soft cutover: dry bus→dest stays until post→dest is up, then drop dry.
fn arm_wet_soft(
    backend: &mut dyn AudioBackend,
    desired: &mut DesiredState,
    clock: &GraphClock,
    bus: &str,
    from: &str,
    fx: &str,
    post: &str,
    post_mon: &str,
    dests: &[String],
) -> Result<()> {
    let mtr = mtr_name_for_bus(bus);
    let _ = backend.ensure_link_raw(from, fx);
    let _ = backend.ensure_link_raw(from, "buschain_hold");
    let _ = backend.ensure_link_raw(fx, post);
    let mut wet_ok = false;
    for d in dests {
        if d.is_empty() || !sink_exists(d) {
            continue;
        }
        if link_to_dest(backend, desired, clock, post_mon, d).is_ok() {
            wet_ok = true;
        }
    }
    let allow_owned = allow_for_dests(post_mon, dests, desired);
    let allow_post: Vec<&str> = allow_owned
        .iter()
        .map(|s| s.as_str())
        .chain(std::iter::once("buschain_hold"))
        .collect();
    let _ = backend.unlink_from_source_except(post_mon, &allow_post);
    if wet_ok && dests_linked(post_mon, dests, desired) {
        // Exclusive wet: drop dry bus→dest (keep fx + optional dry-meter tap).
        let _ = backend.unlink_from_source_except(from, &[fx, "buschain_hold", mtr.as_str()]);
        return Ok(());
    }
    // Keep dry parallel until post→dest lands (never strip fx).
    arm_dry_preserving_fx(backend, desired, clock, bus, from, post_mon, fx, dests)
}

/// Exclusive arm: egress_source → each dest (plus hold on bus if source is bus).
///
/// When `wet` is requested and an FX host/post exists, **never** fall through to
/// a dry unlink that removes `bus→fx` — that permanently skipped the sealed
/// track chain (apps/mics audible dry into Master, track FX only on Master).
pub fn arm_track_egress(
    backend: &mut dyn AudioBackend,
    desired: &mut DesiredState,
    clock: &GraphClock,
    bus: &str,
    wet: bool,
    dests: &[String],
) -> Result<()> {
    let from = format!("{bus}.monitor");
    let post = live_post_name(bus);
    let post_mon = format!("{post}.monitor");
    let fx = live_fx_name(bus);
    // Master-only pad: Track→Track / vin stay on logical dests (no GLC).
    let dests = physical_dests(bus, dests);

    let host_up = registry::host_running(bus);
    let post_up = sink_exists(&post);
    let want_wet = wet && (host_up || post_up);

    if want_wet {
        // Fast path: exclusive wet already correct.
        if spine_instant_ready(bus)
            && dests_linked(&post_mon, &dests, desired)
            && !dests
                .iter()
                .any(|d| !d.is_empty() && link_is_live(&from, d))
            // No dry∥wet Master dual-feed (direct Master while GLC owns the edge).
            && !(glc::master_pad_samples(bus) > 0
                && link_is_live(&from, "buschain_master"))
            && !(glc::master_pad_samples(bus) > 0
                && link_is_live(&post_mon, "buschain_master"))
        {
            let _ = backend.ensure_link_raw(&from, "buschain_hold");
            return Ok(());
        }
        return arm_wet_soft(
            backend, desired, clock, bus, &from, &fx, &post, &post_mon, &dests,
        );
    }

    arm_dry_to_dests(backend, desired, clock, bus, &from, &post_mon, &dests)
}

/// Soft cutover: keep dry audible until post→dest is up, then drop dry.
pub fn arm_track_egress_soft_cutover(
    backend: &mut dyn AudioBackend,
    desired: &mut DesiredState,
    clock: &GraphClock,
    bus: &str,
    dests: &[String],
) -> Result<()> {
    let from = format!("{bus}.monitor");
    let post = live_post_name(bus);
    let post_mon = format!("{post}.monitor");
    let fx = live_fx_name(bus);
    let dests = physical_dests(bus, dests);
    arm_wet_soft(
        backend, desired, clock, bus, &from, &fx, &post, &post_mon, &dests,
    )
}

pub fn disarm_master_hw(backend: &mut dyn AudioBackend, hw: &str) {
    let from = "buschain_master.monitor";
    let post = live_post_name("buschain_master");
    let post_mon = format!("{post}.monitor");
    let fx = live_fx_name("buschain_master");
    let mtr = mtr_name_for_bus("buschain_master");
    if registry::host_running("buschain_master") {
        let _ = backend.unlink_from_source_except(from, &[fx.as_str(), "buschain_hold", mtr.as_str()]);
    } else {
        let _ = backend.unlink_from_source_except(from, &["buschain_hold", mtr.as_str()]);
    }
    let _ = backend.unlink_raw(from, hw);
    let _ = backend.unlink_from_source_except(&post_mon, &[]);
    let _ = backend.unlink_raw(&post_mon, hw);
    // Drop egress converter hops into this HW (name prefix sweep).
    if let Ok(names) = backend.list_sink_names() {
        for name in names {
            if name.starts_with("buschain_rs_out_") {
                let mon = format!("{name}.monitor");
                let _ = backend.unlink_raw(&mon, hw);
                let _ = backend.unlink_raw(from, &name);
                let _ = backend.unlink_raw(&post_mon, &name);
                let _ = backend.destroy_node(&name);
            }
        }
    }
    let _ = backend.ensure_link_raw(from, "buschain_hold");
}

pub fn arm_master_hw(
    backend: &mut dyn AudioBackend,
    desired: &mut DesiredState,
    clock: &GraphClock,
    hw: &str,
    wet: bool,
) -> Result<()> {
    if hw.is_empty() || !sink_exists(hw) {
        return Ok(());
    }
    let dests = vec![hw.to_string()];
    arm_track_egress(backend, desired, clock, "buschain_master", wet, &dests)
}
