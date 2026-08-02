//! Sealed wet path via in-process DSP host (`PwFxNode`).

use std::thread;
use std::time::Duration;

use anyhow::Result;

use crate::backend::{sink_exists, AudioBackend};
use crate::backend::link_is_live;
use crate::clock::GraphClock;
use crate::domain::{
    bus_suffix, fx_name_for_bus, post_name_for_bus, ChainEnsureMode, ChainSpec, ChainState,
    InsertSlot, NodeName, NodeRole, NodeSpec, WirePlan,
};
use crate::fx_gen::{clear_live_gen, set_live_gen};
use crate::host::registry;
use crate::pipeline::arm::arm_track_egress;
use crate::plan::DesiredState;

fn host_wire_plan(spec: &ChainSpec) -> WirePlan {
    let bus = spec.bus.as_str();
    WirePlan {
        bus: spec.bus.clone(),
        fx_sink: NodeName::new(fx_name_for_bus(bus)),
        post_sink: NodeName::new(post_name_for_bus(bus)),
        dest: spec.dest.clone(),
    }
}

fn ensure_post_bus(backend: &mut dyn AudioBackend, clock: &GraphClock, plan: &WirePlan) -> Result<()> {
    let suffix = bus_suffix(plan.bus.as_str());
    let post = plan.post_sink.as_str();
    // Skip rate reprobes on every ensure — ClockBind tears posts when the graph
    // clock actually changes. Cold create path only needs ensure_node.
    // Drop retired A/B staging sibling if present (steals HW links).
    let stg = format!("{post}__stg");
    if sink_exists(&stg) {
        let _ = backend.destroy_node(&stg);
    }
    let already = sink_exists(post);
    backend.ensure_node(
        &NodeSpec {
            name: plan.post_sink.clone(),
            description: format!("BusChainControl_Post_{suffix}"),
            role: NodeRole::PostBus,
            start_muted: false,
            pulse_export: false,
        },
        clock,
    )?;
    if !already {
        // Open the fresh post at unity, unmuted — native SPA Props first;
        // pactl only when the registry is down (cold create is off the RT path,
        // but two extra forks per bus still hurt ArmSession bring-up).
        if crate::backend::native_ready()
            && crate::backend::native_set_levels(post, 0.0, false).is_ok()
        {
            // done natively
        } else {
            let _ = std::process::Command::new("pactl")
                .args(["set-sink-mute", post, "0"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            let _ = std::process::Command::new("pactl")
                .args(["set-sink-volume", post, "100%"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
    Ok(())
}

/// Host spine ready for probes — prefer registry atomics over CLI.
fn host_spine_ready(plan: &WirePlan) -> bool {
    let from = plan.bus_monitor();
    let fx = plan.fx_sink.as_str();
    let post = plan.post_sink.as_str();
    if !registry::host_running(plan.bus.as_str()) {
        return false;
    }
    if !sink_exists(post) {
        return false;
    }
    link_is_live(&from, fx) && link_is_live(fx, post)
}

fn link_spine(backend: &mut dyn AudioBackend, plan: &WirePlan) -> Result<()> {
    let from = plan.bus_monitor();
    let fx = plan.fx_sink.as_str();
    let post = plan.post_sink.as_str();
    let _ = backend.ensure_link_raw(&from, "buschain_hold");
    backend.ensure_link_raw(&from, fx)?;
    backend.ensure_link_raw(fx, post)?;
    Ok(())
}

pub fn ensure_fx_chain(
    backend: &mut dyn AudioBackend,
    desired: &mut DesiredState,
    clock: &GraphClock,
    spec: &ChainSpec,
    mode: ChainEnsureMode,
    arm_egress: bool,
) -> Result<ChainState> {
    let bus = spec.bus.as_str();
    if spec.inserts.is_empty() {
        teardown_fx_chain(backend, bus)?;
        return Ok(ChainState::Dry);
    }

    let plan = host_wire_plan(spec);
    let want_sig = spec.signature();
    let fp_ok = registry::host_fingerprint(bus).as_deref() == Some(want_sig.as_str());
    let running = registry::host_running(bus);

    // Fast paths before post/CLI work — host already matches.
    if mode == ChainEnsureMode::ProbeOnly {
        if fp_ok && running {
            return Ok(ChainState::Wet(plan));
        }
        return Ok(ChainState::Failed(
            "FX host path not ready — idle will not ForceRespawn".into(),
        ));
    }

    if mode == ChainEnsureMode::Idempotent && fp_ok && running {
        // Soft-link repair if needed (cheap when already live).
        if !host_spine_ready(&plan) {
            let _ = ensure_post_bus(backend, clock, &plan);
            let _ = link_spine(backend, &plan);
        }
        return Ok(ChainState::Wet(plan));
    }

    let force = mode == ChainEnsureMode::ForceRespawn || !fp_ok || !running;
    let span = crate::fx_trace::span("EnsureFxHost");

    ensure_post_bus(backend, clock, &plan)?;

    if let Err(e) = registry::ensure_host(bus, &spec.inserts, clock, force) {
        span.end(format!("fail {e:#}"));
        return Ok(ChainState::Failed(format!("host ensure: {e:#}")));
    }

    set_live_gen(bus, plan.fx_sink.as_str(), plan.post_sink.as_str());

    // Single invalidate + up to 3 quick link attempts (ports appear with the node).
    crate::backend::invalidate_probe_caches();
    let mut link_err = None;
    for attempt in 0..3 {
        match link_spine(backend, &plan) {
            Ok(()) => {
                link_err = None;
                break;
            }
            Err(e) => {
                link_err = Some(e);
                if attempt + 1 < 3 {
                    thread::sleep(Duration::from_millis(10));
                    crate::backend::invalidate_probe_caches();
                }
            }
        }
    }
    if let Some(e) = link_err {
        span.end(format!("link fail {e:#}"));
        return Ok(ChainState::Failed(format!("host link: {e:#}")));
    }

    if arm_egress && !plan.dest.is_empty() {
        let dests = [plan.dest.clone()];
        let _ = arm_track_egress(backend, desired, clock, bus, true, &dests);
    }

    if !registry::host_running(bus) {
        span.end("not running");
        return Ok(ChainState::Failed("FX host node not running".into()));
    }

    span.end(format!(
        "wet {} xruns={} lat={}",
        plan.fx_sink.as_str(),
        registry::host_xruns(bus),
        registry::host_latency(bus)
    ));
    Ok(ChainState::Wet(plan))
}

pub fn push_fx_controls(bus: &str, inserts: &[InsertSlot]) -> Result<()> {
    registry::push_host_controls(bus, inserts)
}

pub fn teardown_fx_chain(backend: &mut dyn AudioBackend, bus: &str) -> Result<()> {
    let from = format!("{bus}.monitor");
    let fx = fx_name_for_bus(bus);
    let post = post_name_for_bus(bus);
    let post_mon = format!("{post}.monitor");
    let _ = backend.unlink_from_source_except(&from, &["buschain_hold"]);
    let _ = backend.unlink_from_source_except(&fx, &[]);
    let _ = backend.unlink_from_source_except(&post_mon, &[]);
    registry::teardown_host(bus);
    let _ = backend.destroy_node(&post);
    clear_live_gen(bus);
    Ok(())
}

pub fn probe_chain_state(bus: &str, inserts_len: usize, dest: &str, require_dest: bool) -> ChainState {
    if inserts_len == 0 {
        return ChainState::Dry;
    }
    let plan = WirePlan {
        bus: NodeName::new(bus),
        fx_sink: NodeName::new(fx_name_for_bus(bus)),
        post_sink: NodeName::new(post_name_for_bus(bus)),
        dest: dest.to_string(),
    };
    if !registry::host_running(bus) {
        return ChainState::Failed("FX host not running".into());
    }
    // Host running + fingerprint is enough for meters/route; full spine CLI is optional.
    // Probe has no Desired — accept direct post→dest (egress bridge checked at arm time).
    if require_dest {
        if link_is_live(&plan.post_monitor(), plan.dest.as_str()) {
            ChainState::Wet(plan)
        } else {
            ChainState::Failed("wet path not audible".into())
        }
    } else {
        ChainState::Wet(plan)
    }
}
