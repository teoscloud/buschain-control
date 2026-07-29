//! Sealed insert-chain wet switch — spawn FX hold-only, dwell, then arm egress.

use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Result};

use crate::backend::{
    filter_chain_clock_for_sink, push_insert_controls, read_signature, sink_exists, sink_has_input,
    spawn_sidechain, FilterChainRuntime,
};
use crate::backend::link_is_live;
use crate::backend::AudioBackend;
use crate::clock::{probe_sink_running_rate, GraphClock};
use crate::domain::{
    bus_suffix, ChainEnsureMode, ChainSpec, ChainState, InsertSlot, NodeRole, NodeSpec, WirePlan,
};
use crate::pipeline::arm::{
    arm_track_egress, disarm_track_egress_ex, spine_instant_ready, wait_spine_stable, WET_DWELL,
};

/// Wet spine + (optionally) post→dest.
fn path_audible(plan: &WirePlan, require_dest: bool) -> bool {
    let from = plan.bus_monitor();
    let fx = plan.fx_sink.as_str();
    let post_mon = plan.post_monitor();
    let dest = plan.dest.as_str();
    let spine = link_is_live(&from, fx)
        && sink_exists(plan.post_sink.as_str())
        && sink_has_input(plan.post_sink.as_str());
    if !spine {
        return false;
    }
    if !require_dest || dest.is_empty() {
        return true;
    }
    link_is_live(&post_mon, dest)
}

fn ensure_post_bus(backend: &mut dyn AudioBackend, clock: &GraphClock, plan: &WirePlan) -> Result<()> {
    let suffix = bus_suffix(plan.bus.as_str());
    let post = plan.post_sink.as_str();
    // Post must match the bus/FX rate. Stale Custom-192k posts block wet landing.
    if clock.sample_rate > 0 {
        if let Some(have) = probe_sink_running_rate(post) {
            if have != clock.sample_rate {
                let _ = backend.destroy_node(post);
                thread::sleep(Duration::from_millis(40));
            }
        }
    }
    backend.ensure_node(
        &NodeSpec {
            name: plan.post_sink.clone(),
            description: format!("BusChainControl_Post_{suffix}"),
            role: NodeRole::PostBus,
            start_muted: false,
        },
        clock,
    )?;
    // Wake post before FX-out tries to land.
    let _ = std::process::Command::new("pactl")
        .args(["suspend-sink", post, "0"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
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
    Ok(())
}

fn restore_dry(
    backend: &mut dyn AudioBackend,
    runtime: &mut FilterChainRuntime,
    plan: &WirePlan,
    arm_dest: bool,
) -> Result<()> {
    let from = plan.bus_monitor();
    let dest = &plan.dest;
    let post_mon = plan.post_monitor();
    let _ = backend.unlink_from_source_except(&post_mon, &[]);
    if arm_dest && !dest.is_empty() {
        let _ = backend.unlink_from_source_except(&from, &[dest.as_str(), "buschain_hold"]);
        backend.ensure_link_raw(&from, dest)?;
    } else {
        let _ = backend.unlink_from_source_except(&from, &["buschain_hold"]);
    }
    let _ = backend.ensure_link_raw(&from, "buschain_hold");
    runtime.stop_one_keep_artifacts(plan.fx_sink.as_str());
    Ok(())
}

/// Build or refresh the sealed wet insert chain for one bus.
///
/// When `arm_egress` is false (Master during session barrier), builds the spine
/// hold-only and returns Wet after dwell without opening post→dest.
pub fn ensure_fx_chain(
    runtime: &mut FilterChainRuntime,
    backend: &mut dyn AudioBackend,
    clock: &GraphClock,
    spec: &ChainSpec,
    mode: ChainEnsureMode,
    arm_egress: bool,
) -> Result<ChainState> {
    if spec.inserts.is_empty() {
        teardown_fx_chain(runtime, backend, spec.bus.as_str())?;
        return Ok(ChainState::Dry);
    }

    let plan = spec.wire_plan();
    let fx_name = plan.fx_sink.as_str();
    let from = plan.bus_monitor();
    let post_mon = plan.post_monitor();
    let dest = &plan.dest;
    let want_sig = spec.signature();
    let (fx_clock, clock_fragment) = filter_chain_clock_for_sink(clock, plan.bus.as_str());

    ensure_post_bus(backend, &fx_clock, &plan)?;

    let sig_ok = (runtime.owns_live(fx_name) || sink_exists(fx_name))
        && read_signature(fx_name).as_deref() == Some(want_sig.as_str());
    let audible = path_audible(&plan, arm_egress);

    // Idle ProbeOnly: adopt when healthy; NEVER stop/spawn (path flaps stole minutes).
    if mode == ChainEnsureMode::ProbeOnly {
        if sig_ok && audible {
            runtime.clear_spawn_failed(fx_name);
            return Ok(ChainState::Wet(plan));
        }
        if runtime.spawn_in_backoff(fx_name) {
            return Ok(ChainState::Failed(
                "FX spawn backoff — worker staying responsive".into(),
            ));
        }
        return Ok(ChainState::Failed(
            "FX path not audible — idle will not ForceRespawn".into(),
        ));
    }

    // Idempotent: adopt when healthy; otherwise fall through to respawn.
    if mode == ChainEnsureMode::Idempotent && sig_ok && audible {
        runtime.clear_spawn_failed(fx_name);
        // Do NOT re-push controls here — live Props are owned by PushFxControls.
        return Ok(ChainState::Wet(plan));
    }

    if mode == ChainEnsureMode::Idempotent && runtime.spawn_in_backoff(fx_name) {
        return Ok(ChainState::Failed(
            "FX spawn backoff — worker staying responsive".into(),
        ));
    }

    // 1) Drop wet egress but keep a dry bus→dest bridge so playback (YouTube /
    // Chromium) does not hear a silent gap and stall A/V during ForceRespawn.
    // Wet is re-armed exclusively below and the dry hop is pruned.
    if arm_egress && !dest.is_empty() {
        let _ = backend.ensure_link_raw(&from, dest);
    }
    disarm_track_egress_ex(
        backend,
        plan.bus.as_str(),
        true,
        if arm_egress && !dest.is_empty() {
            Some(dest.as_str())
        } else {
            None
        },
    );

    // 2) Respawn FX helper (hard-fails on spawn errors).
    runtime.stop_one(fx_name);
    if let Err(e) = spawn_sidechain(runtime, spec, &clock_fragment) {
        runtime.mark_spawn_failed(fx_name);
        let _ = restore_dry(backend, runtime, &plan, arm_egress);
        return Ok(ChainState::Failed(format!("FX spawn: {e:#}")));
    }
    runtime.clear_spawn_failed(fx_name);

    // 3) Feed FX silently — no dest yet.
    backend.ensure_link_raw(&from, fx_name)?;
    let _ = backend.unlink_from_source_except(&format!("{fx_name}.monitor"), &[]);
    let _ = backend.unlink_from_source_except(&post_mon, &[]);
    let _ = backend.ensure_link_raw(&from, "buschain_hold");

    // 4) Poll instant spine ready, then short continuous dwell.
    let mut wet_ok = false;
    for _ in 0..25 {
        wet_ok = spine_instant_ready(plan.bus.as_str());
        if wet_ok {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }

    if !wet_ok {
        runtime.mark_spawn_failed(fx_name);
        let _ = restore_dry(backend, runtime, &plan, arm_egress);
        return Ok(ChainState::Failed(
            "wet path not live after FX spawn (fx_out never reached post)".into(),
        ));
    }

    if !wait_spine_stable(plan.bus.as_str(), WET_DWELL) {
        runtime.mark_spawn_failed(fx_name);
        let _ = restore_dry(backend, runtime, &plan, arm_egress);
        return Ok(ChainState::Failed(
            "spine unstable during dwell — restored dry".into(),
        ));
    }

    // 5) Arm egress (or leave hold-only when barrier holds Master→HW).
    if arm_egress && !dest.is_empty() {
        let dests = vec![dest.clone()];
        if let Err(e) = arm_track_egress(backend, plan.bus.as_str(), true, &dests) {
            runtime.mark_spawn_failed(fx_name);
            let _ = restore_dry(backend, runtime, &plan, true);
            return Ok(ChainState::Failed(format!(
                "post→dest arm failed — restored dry: {e:#}"
            )));
        }
        // Prune any race dry hop.
        for _ in 0..5 {
            let _ = backend.unlink_raw(&from, dest);
            if !link_is_live(&from, dest) {
                break;
            }
            thread::sleep(Duration::from_millis(30));
        }
        if !path_audible(&plan, true) {
            runtime.mark_spawn_failed(fx_name);
            let _ = restore_dry(backend, runtime, &plan, true);
            return Ok(ChainState::Failed(
                "exclusive wet arm failed — restored dry".into(),
            ));
        }
    }

    Ok(ChainState::Wet(plan))
}

pub fn push_fx_controls(
    runtime: &mut FilterChainRuntime,
    fx_name: &str,
    inserts: &[InsertSlot],
) -> Result<()> {
    // Prefer owns_live — avoid pactl list-short on every knob tick.
    if !runtime.owns_live(fx_name) {
        // Signature file is enough proof the helper was spawned; skip sink_exists
        // unless we have neither ownership nor a signature (orphan after UI restart).
        if read_signature(fx_name).is_none() && !sink_exists(fx_name) {
            return Err(anyhow!("FX chain `{fx_name}` is not wet"));
        }
    }
    push_insert_controls(fx_name, inserts)
}

pub fn teardown_fx_chain(
    runtime: &mut FilterChainRuntime,
    backend: &mut dyn AudioBackend,
    bus: &str,
) -> Result<()> {
    let spec = ChainSpec {
        bus: crate::domain::NodeName::new(bus),
        inserts: Vec::new(),
        dest: String::new(),
    };
    let plan = spec.wire_plan();
    runtime.stop_one(plan.fx_sink.as_str());
    let _ = backend.destroy_node(plan.post_sink.as_str());
    let from = plan.bus_monitor();
    let _ = backend.unlink_from_source_except(&from, &[]);
    let _ = backend.unlink_from_source_except(&plan.post_monitor(), &[]);
    Ok(())
}

pub fn probe_chain_state(
    runtime: &mut FilterChainRuntime,
    bus: &str,
    inserts_len: usize,
    dest: &str,
    require_dest: bool,
) -> ChainState {
    if inserts_len == 0 {
        return ChainState::Dry;
    }
    let plan = ChainSpec {
        bus: crate::domain::NodeName::new(bus),
        inserts: Vec::new(),
        dest: dest.to_string(),
    }
    .wire_plan();
    let fx_name = plan.fx_sink.as_str();
    if !runtime.owns_live(fx_name) && !sink_exists(fx_name) {
        return ChainState::Failed("FX helper not running".into());
    }
    if path_audible(&plan, require_dest) {
        ChainState::Wet(plan)
    } else if spine_instant_ready(bus) && !require_dest {
        ChainState::Wet(plan)
    } else {
        ChainState::Failed("wet path not audible".into())
    }
}
