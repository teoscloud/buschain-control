//! Sealed insert-chain wet switch — spawn FX hold-only, dwell, then arm egress.
//!
//! Warm ForceRespawn uses A/B dual helpers (build staging behind live, flip).
//! Cold ForceRespawn (no live wet) keeps monitor-gated silence until sealed.

use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Result};

use crate::backend::{
    filter_chain_clock_for_sink, gate_bus_monitor, prepare_fx_conf, push_insert_controls,
    read_signature, sink_exists, sink_has_input, spawn_pipewire_conf_from_prepared,
    spawn_sidechain_named, write_signature, FilterChainRuntime,
};
use crate::backend::link_is_live;
use crate::backend::AudioBackend;
use crate::clock::{probe_sink_running_rate, GraphClock};
use crate::domain::{
    bus_suffix, ChainEnsureMode, ChainSpec, ChainState, InsertSlot, NodeName, NodeRole, NodeSpec,
    WirePlan,
};
use crate::fx_gen::{
    canonical_fx, canonical_post, clear_live_gen, live_fx_name, live_post_name, set_live_gen,
    staging_fx, staging_pair, staging_post,
};
use crate::pipeline::arm::{
    arm_track_egress, arm_track_egress_soft_cutover, arm_wet_ab_cutover, disarm_track_egress_ex,
    spine_instant_ready, wait_spine_stable, WET_DWELL,
};

/// Silence `{bus}.monitor` for the cold ForceRespawn window only.
struct RebuildSilence {
    bus: String,
}

impl RebuildSilence {
    fn enter(bus: &str) -> Self {
        let _ = gate_bus_monitor(bus, true);
        Self {
            bus: bus.to_string(),
        }
    }
}

impl Drop for RebuildSilence {
    fn drop(&mut self) {
        let _ = gate_bus_monitor(&self.bus, false);
    }
}

fn live_wire_plan(spec: &ChainSpec) -> WirePlan {
    let bus = spec.bus.as_str();
    WirePlan {
        bus: spec.bus.clone(),
        fx_sink: NodeName::new(live_fx_name(bus)),
        post_sink: NodeName::new(live_post_name(bus)),
        dest: spec.dest.clone(),
    }
}

fn named_wire_plan(spec: &ChainSpec, fx: &str, post: &str) -> WirePlan {
    WirePlan {
        bus: spec.bus.clone(),
        fx_sink: NodeName::new(fx),
        post_sink: NodeName::new(post),
        dest: spec.dest.clone(),
    }
}

/// Wet spine + (optionally) post→dest.
fn path_audible(plan: &WirePlan, require_dest: bool) -> bool {
    let from = plan.bus_monitor();
    let fx = plan.fx_sink.as_str();
    let post_mon = plan.post_monitor();
    let dest = plan.dest.as_str();
    let spine = link_is_live(&from, fx)
        && sink_exists(plan.post_sink.as_str())
        && (sink_has_input(plan.post_sink.as_str())
            || link_is_live(&format!("{fx}_out"), plan.post_sink.as_str()));
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
    if clock.sample_rate > 0 {
        if let Some(have) = probe_sink_running_rate(post) {
            if have != clock.sample_rate {
                let _ = backend.destroy_node(post);
                thread::sleep(Duration::from_millis(40));
            }
        }
    }
    let desc = if post.ends_with("__stg") {
        format!("BusChainControl_Post_{suffix}_stg")
    } else {
        format!("BusChainControl_Post_{suffix}")
    };
    backend.ensure_node(
        &NodeSpec {
            name: plan.post_sink.clone(),
            description: desc,
            role: NodeRole::PostBus,
            start_muted: false,
        },
        clock,
    )?;
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
    runtime.stop_one(plan.fx_sink.as_str());
    Ok(())
}

fn spine_named(runtime: &mut FilterChainRuntime, from: &str, fx: &str, post: &str) -> bool {
    // Owned `pipewire -c` child wins. Parallel ForceRespawn at launch stamps pactl
    // until `sink_exists` / `link_is_live` false-negative for ~400ms each — that
    // used to burn ~10s per track and mark every FX Failed while helpers were fine.
    if runtime.owns_live(fx) {
        return true;
    }
    let fx_out = format!("{fx}_out");
    if !sink_exists(fx) || !sink_exists(post) {
        return false;
    }
    if !link_is_live(from, fx) {
        return false;
    }
    link_is_live(&fx_out, post) || sink_has_input(post)
}

fn stop_both_gens(runtime: &mut FilterChainRuntime, backend: &mut dyn AudioBackend, bus: &str) {
    let can_fx = canonical_fx(bus);
    let can_post = canonical_post(bus);
    let stg_fx = staging_fx(bus);
    let stg_post = staging_post(bus);
    runtime.stop_one(&can_fx);
    runtime.stop_one(&stg_fx);
    let _ = backend.destroy_node(&can_post);
    let _ = backend.destroy_node(&stg_post);
    clear_live_gen(bus);
}

/// Warm A/B: build staging behind live, wet/wet flip, retire old. Fail-open keeps live.
fn ensure_ab_cutover(
    runtime: &mut FilterChainRuntime,
    backend: &mut dyn AudioBackend,
    clock: &GraphClock,
    spec: &ChainSpec,
    clock_fragment: &str,
    arm_egress: bool,
) -> Result<ChainState> {
    let bus = spec.bus.as_str();
    let from = format!("{bus}.monitor");
    let old_fx = live_fx_name(bus);
    let old_post = live_post_name(bus);
    let (new_fx, new_post) = staging_pair(bus);
    let dest = spec.dest.clone();
    let old_plan = named_wire_plan(spec, &old_fx, &old_post);
    let new_plan = named_wire_plan(spec, &new_fx, &new_post);

    // Cancel any leftover staging from a prior aborted job.
    runtime.stop_one(&new_fx);
    let _ = backend.destroy_node(&new_post);

    let span = crate::fx_trace::span("AbCutover");
    let t0 = std::time::Instant::now();
    // Gate Props for the whole cutover — mid-spawn CLI storms false-"not found"
    // and kick a second ensure that tears the live path down.
    let _gate_old = crate::fx_busy::RebuildGuard::enter(&old_fx);
    let _gate_new = crate::fx_busy::RebuildGuard::enter(&new_fx);

    if let Err(e) = ensure_post_bus(backend, clock, &new_plan) {
        return ab_fail_open(
            runtime,
            backend,
            clock,
            spec,
            clock_fragment,
            arm_egress,
            &from,
            &old_fx,
            &old_post,
            &dest,
            old_plan,
            span,
            &format!("post FAIL {new_post}: {e:#}"),
        );
    }

    let t_spawn = std::time::Instant::now();
    if let Err(e) = spawn_sidechain_named(runtime, spec, &new_fx, &new_post, clock_fragment) {
        runtime.stop_one(&new_fx);
        let _ = backend.destroy_node(&new_post);
        crate::fx_trace::log("AbSpawn", &new_fx, t_spawn.elapsed().as_millis());
        return ab_fail_open(
            runtime,
            backend,
            clock,
            spec,
            clock_fragment,
            arm_egress,
            &from,
            &old_fx,
            &old_post,
            &dest,
            old_plan,
            span,
            &format!("spawn FAIL {new_fx}: {e:#}"),
        );
    }
    crate::fx_trace::log("AbSpawn", &new_fx, t_spawn.elapsed().as_millis());

    let _ = backend.ensure_link_raw(&from, &new_fx);
    let _ = backend.ensure_link_raw(&from, &old_fx);
    let _ = backend.ensure_link_raw(&from, "buschain_hold");
    let _ = push_insert_controls(&new_fx, &spec.inserts);

    let t_spine = std::time::Instant::now();
    let mut ok = false;
    for _ in 0..12 {
        if spine_named(runtime, &from, &new_fx, &new_post) {
            ok = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    crate::fx_trace::log("AbSpine", &new_fx, t_spine.elapsed().as_millis());
    if !ok {
        runtime.stop_one(&new_fx);
        let _ = backend.destroy_node(&new_post);
        return ab_fail_open(
            runtime,
            backend,
            clock,
            spec,
            clock_fragment,
            arm_egress,
            &from,
            &old_fx,
            &old_post,
            &dest,
            old_plan,
            span,
            &format!("spine FAIL {new_fx}"),
        );
    }

    let t_flip = std::time::Instant::now();
    if arm_egress && !dest.is_empty() {
        let dests = vec![dest.clone()];
        if let Err(e) = arm_wet_ab_cutover(
            backend, bus, &old_fx, &old_post, &new_fx, &new_post, &dests,
        ) {
            runtime.stop_one(&new_fx);
            let _ = backend.destroy_node(&new_post);
            return ab_fail_open(
                runtime,
                backend,
                clock,
                spec,
                clock_fragment,
                arm_egress,
                &from,
                &old_fx,
                &old_post,
                &dest,
                old_plan,
                span,
                &format!("flip FAIL: {e:#}"),
            );
        }
    } else {
        // Hold-only barrier: feed new, drop old feed, no dest arm.
        let _ = backend.unlink_from_source_except(&from, &[new_fx.as_str(), "buschain_hold"]);
        let _ = backend.ensure_link_raw(&from, &new_fx);
        let _ = backend.ensure_link_raw(&from, "buschain_hold");
    }
    crate::fx_trace::log("AbFlip", &new_fx, t_flip.elapsed().as_millis());

    // Promote pointer BEFORE killing old — Props/Done must not chase a dead gen.
    set_live_gen(bus, &new_fx, &new_post);
    runtime.stop_one(&old_fx);
    let _ = backend.destroy_node(&old_post);
    let _ = push_insert_controls(&new_fx, &spec.inserts);

    span.end(format!(
        "wet {new_fx} total={}ms",
        t0.elapsed().as_millis()
    ));
    Ok(ChainState::Wet(new_plan))
}

/// After a failed A/B staging attempt: exclusive feed + wet egress on the survivor.
fn restore_ab_live(
    backend: &mut dyn AudioBackend,
    bus: &str,
    from: &str,
    old_fx: &str,
    dest: &str,
    arm_egress: bool,
) {
    let _ = backend.unlink_from_source_except(from, &[old_fx, "buschain_hold"]);
    let _ = backend.ensure_link_raw(from, old_fx);
    let _ = backend.ensure_link_raw(from, "buschain_hold");
    // Drop any dry bus→dest that may have been fail-opened while staging ran.
    if !dest.is_empty() {
        let _ = backend.unlink_raw(from, dest);
    }
    if arm_egress && !dest.is_empty() {
        let dests = vec![dest.to_string()];
        let _ = arm_track_egress(backend, bus, true, &dests);
    }
}

/// Keep old gen only when its signature still matches Desired. Otherwise cold
/// rebuild — otherwise reorder/enable leaves UI on the new rack while audio
/// keeps the previous order (the "plugins stuck in old order" bug).
fn ab_fail_open(
    runtime: &mut FilterChainRuntime,
    backend: &mut dyn AudioBackend,
    clock: &GraphClock,
    spec: &ChainSpec,
    clock_fragment: &str,
    arm_egress: bool,
    from: &str,
    old_fx: &str,
    old_post: &str,
    dest: &str,
    old_plan: WirePlan,
    span: crate::fx_trace::FxSpan,
    reason: &str,
) -> Result<ChainState> {
    let bus = spec.bus.as_str();
    restore_ab_live(backend, bus, from, old_fx, dest, arm_egress);
    let want = spec.signature();
    let old_ok = (runtime.owns_live(old_fx) || sink_exists(old_fx))
        && read_signature(old_fx).as_deref() == Some(want.as_str());
    if old_ok {
        span.end(format!("{reason} keep {old_fx}"));
        let _ = old_post;
        return Ok(ChainState::Wet(old_plan));
    }
    span.end(format!("{reason} → cold (sig mismatch)"));
    ensure_cold_respawn(
        runtime,
        backend,
        clock,
        spec,
        clock_fragment,
        arm_egress,
    )
}

/// Cold ForceRespawn — silence + rebuild canonical generation.
fn ensure_cold_respawn(
    runtime: &mut FilterChainRuntime,
    backend: &mut dyn AudioBackend,
    clock: &GraphClock,
    spec: &ChainSpec,
    clock_fragment: &str,
    arm_egress: bool,
) -> Result<ChainState> {
    let bus = spec.bus.as_str();
    let fx_name = canonical_fx(bus);
    let post_name = canonical_post(bus);
    let plan = named_wire_plan(spec, &fx_name, &post_name);
    let from = plan.bus_monitor();
    let post_mon = plan.post_monitor();
    let dest = &plan.dest;

    stop_both_gens(runtime, backend, bus);
    set_live_gen(bus, &fx_name, &post_name);
    ensure_post_bus(backend, clock, &plan)?;

    // Prepare conf before Props gate / silence so stop→spawn is shorter.
    let conf_path = prepare_fx_conf(runtime, &fx_name, &post_name, spec, clock_fragment)?;

    let _rebuild_gate = crate::fx_busy::RebuildGuard::enter(&fx_name);
    let _rebuild_silence = RebuildSilence::enter(bus);
    let ensure_span = crate::fx_trace::span("ForceRespawn");
    let t0 = std::time::Instant::now();

    if arm_egress && !dest.is_empty() {
        let _ = backend.ensure_link_raw(&from, dest);
    }
    disarm_track_egress_ex(
        backend,
        bus,
        true,
        if arm_egress && !dest.is_empty() {
            Some(dest.as_str())
        } else {
            None
        },
    );
    crate::fx_trace::log("ForceRespawn.disarm", &fx_name, t0.elapsed().as_millis());

    let t_stop = std::time::Instant::now();
    // Already stopped in stop_both_gens — keep invalidate only.
    crate::backend::invalidate_probe_caches();
    crate::fx_trace::log("ForceRespawn.stop", &fx_name, t_stop.elapsed().as_millis());

    let t_spawn = std::time::Instant::now();
    if let Err(e) = spawn_pipewire_conf_from_prepared(runtime, &fx_name, &conf_path, &post_name) {
        runtime.mark_spawn_failed(&fx_name);
        if !runtime.owns_live(&fx_name) {
            let _ = restore_dry(backend, runtime, &plan, arm_egress);
        }
        ensure_span.end(format!("spawn FAIL {fx_name}: {e:#}"));
        return Ok(ChainState::Failed(format!("FX spawn: {e:#}")));
    }
    let _ = write_signature(runtime.conf_dir(), &fx_name, &spec.signature());
    runtime.clear_spawn_failed(&fx_name);
    crate::backend::invalidate_probe_caches();
    crate::fx_trace::log("ForceRespawn.spawn", &fx_name, t_spawn.elapsed().as_millis());

    if let Err(e) = backend.ensure_link_raw(&from, &fx_name) {
        runtime.mark_spawn_failed(&fx_name);
        if !runtime.owns_live(&fx_name) {
            let _ = restore_dry(backend, runtime, &plan, arm_egress);
        }
        ensure_span.end(format!("feed FAIL {fx_name}: {e:#}"));
        return Ok(ChainState::Failed(format!("FX feed link: {e:#}")));
    }
    let _ = backend.unlink_from_source_except(&format!("{fx_name}.monitor"), &[]);
    let _ = backend.unlink_from_source_except(&post_mon, &[]);
    let _ = backend.ensure_link_raw(&from, "buschain_hold");

    let fx_out = format!("{fx_name}_out");
    let t_spine = std::time::Instant::now();
    // Owns-live first — never lead with spine_instant_ready (pactl stampede).
    let mut wet_ok = spine_named(runtime, &from, &fx_name, &post_name);
    if !wet_ok {
        for _ in 0..12 {
            wet_ok = spine_named(runtime, &from, &fx_name, &post_name)
                || spine_instant_ready(bus);
            if wet_ok {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    if !wet_ok {
        runtime.mark_spawn_failed(&fx_name);
        if !runtime.owns_live(&fx_name) {
            let _ = restore_dry(backend, runtime, &plan, arm_egress);
        }
        ensure_span.end(format!(
            "spine FAIL {fx_name} after {}ms",
            t_spine.elapsed().as_millis()
        ));
        return Ok(ChainState::Failed(
            "wet path not live after FX spawn (fx_out never reached post)".into(),
        ));
    }

    // Skip dwell CLI when we own the helper — wait_spine_stable under load is what
    // stretched ForceRespawn into multi-second "spine FAIL" at launch.
    let dwell_ok = spine_named(runtime, &from, &fx_name, &post_name)
        || wait_spine_stable(bus, WET_DWELL);
    if !dwell_ok {
        runtime.mark_spawn_failed(&fx_name);
        if !runtime.owns_live(&fx_name) {
            let _ = restore_dry(backend, runtime, &plan, arm_egress);
        }
        ensure_span.end(format!("dwell FAIL {fx_name}"));
        return Ok(ChainState::Failed(
            "spine unstable during dwell — restored dry".into(),
        ));
    }
    crate::fx_trace::log("ForceRespawn.spine", &fx_name, t_spine.elapsed().as_millis());

    if arm_egress && !dest.is_empty() {
        let dests = vec![dest.clone()];
        if let Err(e) = arm_track_egress_soft_cutover(backend, bus, &dests) {
            runtime.mark_spawn_failed(&fx_name);
            if !runtime.owns_live(&fx_name) {
                let _ = restore_dry(backend, runtime, &plan, true);
            }
            ensure_span.end(format!("arm FAIL {fx_name}: {e:#}"));
            return Ok(ChainState::Failed(format!(
                "post→dest arm failed — restored dry: {e:#}"
            )));
        }
        let link_wet = link_is_live(&plan.post_monitor(), dest.as_str())
            && spine_named(runtime, &from, &fx_name, &post_name);
        if !path_audible(&plan, true) && !link_wet {
            runtime.mark_spawn_failed(&fx_name);
            if !runtime.owns_live(&fx_name) {
                let _ = restore_dry(backend, runtime, &plan, true);
            }
            ensure_span.end(format!("audible FAIL {fx_name}"));
            return Ok(ChainState::Failed(
                "exclusive wet arm failed — restored dry".into(),
            ));
        }
    }

    let _ = fx_out;
    ensure_span.end(format!(
        "wet {fx_name} total={}ms",
        t0.elapsed().as_millis()
    ));
    Ok(ChainState::Wet(plan))
}

/// Build or refresh the sealed wet insert chain for one bus.
pub fn ensure_fx_chain(
    runtime: &mut FilterChainRuntime,
    backend: &mut dyn AudioBackend,
    clock: &GraphClock,
    spec: &ChainSpec,
    mode: ChainEnsureMode,
    arm_egress: bool,
) -> Result<ChainState> {
    let bus = spec.bus.as_str();
    if spec.inserts.is_empty() {
        teardown_fx_chain(runtime, backend, bus)?;
        return Ok(ChainState::Dry);
    }

    // Heal before planning — Probe/Idempotent must see `__stg` when that's live.
    crate::fx_gen::heal_live_gen(bus);
    let plan = live_wire_plan(spec);
    let fx_name = plan.fx_sink.as_str().to_string();
    let want_sig = spec.signature();
    let (fx_clock, clock_fragment) = filter_chain_clock_for_sink(clock, bus);

    ensure_post_bus(backend, &fx_clock, &plan)?;

    // Signature may live on either A/B generation after a warm cutover.
    let sig_ok = [&fx_name, &canonical_fx(bus), &staging_fx(bus)]
        .into_iter()
        .any(|fx| {
            (runtime.owns_live(fx) || sink_exists(fx))
                && read_signature(fx).as_deref() == Some(want_sig.as_str())
        });
    let audible = path_audible(&plan, arm_egress);
    // Spine without post→dest still counts as wet helper for Probe/Idempotent —
    // egress re-arm is reconcile's job. Avoids false Failed → dry bypass on every track.
    let helper_wet = sig_ok && (audible || spine_instant_ready(bus));

    if mode == ChainEnsureMode::ProbeOnly {
        if helper_wet {
            runtime.clear_spawn_failed(&fx_name);
            return Ok(ChainState::Wet(plan));
        }
        if runtime.spawn_in_backoff(&fx_name) {
            return Ok(ChainState::Failed(
                "FX spawn backoff — worker staying responsive".into(),
            ));
        }
        return Ok(ChainState::Failed(
            "FX path not audible — idle will not ForceRespawn".into(),
        ));
    }

    if mode == ChainEnsureMode::Idempotent && helper_wet {
        runtime.clear_spawn_failed(&fx_name);
        return Ok(ChainState::Wet(plan));
    }

    if mode == ChainEnsureMode::Idempotent && runtime.spawn_in_backoff(&fx_name) {
        return Ok(ChainState::Failed(
            "FX spawn backoff — worker staying responsive".into(),
        ));
    }

    runtime.clear_spawn_failed(&fx_name);

    // Warm A/B whenever *either* generation helper exists. Do NOT require spine/
    // audible — a brief pactl flap used to fall through to cold ForceRespawn,
    // stop_both() the live `__stg`, and leave Props chasing a dead pointer.
    let warm = mode == ChainEnsureMode::ForceRespawn
        && (runtime.owns_live(&fx_name)
            || sink_exists(&fx_name)
            || crate::fx_gen::any_gen_live(bus));

    if warm {
        // Point live at whichever helper is actually present before A/B.
        crate::fx_gen::heal_live_gen(bus);
        return ensure_ab_cutover(
            runtime,
            backend,
            &fx_clock,
            spec,
            &clock_fragment,
            arm_egress,
        );
    }

    ensure_cold_respawn(
        runtime,
        backend,
        &fx_clock,
        spec,
        &clock_fragment,
        arm_egress,
    )
}

pub fn push_fx_controls(
    runtime: &mut FilterChainRuntime,
    fx_name: &str,
    inserts: &[InsertSlot],
) -> Result<()> {
    if crate::fx_busy::is_rebuilding(fx_name) {
        return Err(anyhow!("FX rebuilding — props deferred"));
    }
    if !runtime.owns_live(fx_name) {
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
    let from = format!("{bus}.monitor");
    let live_post = live_post_name(bus);
    let live_mon = format!("{live_post}.monitor");
    stop_both_gens(runtime, backend, bus);
    let _ = backend.unlink_from_source_except(&from, &[]);
    let _ = backend.unlink_from_source_except(&live_mon, &[]);
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
    let plan = WirePlan {
        bus: NodeName::new(bus),
        fx_sink: NodeName::new(live_fx_name(bus)),
        post_sink: NodeName::new(live_post_name(bus)),
        dest: dest.to_string(),
    };
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
