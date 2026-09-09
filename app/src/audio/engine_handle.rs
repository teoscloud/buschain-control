//! Process-wide [`buschain_engine::Engine`] for the audio worker thread.
//!
//! Graph/filter shims call into this instead of shelling out to pactl/pw-link.
//!
//! **UI thread must never take the engine mutex.** Wetness for meters is a
//! lock-free cache updated only by the worker after FX ensure/teardown/probe.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use buschain_engine::{
    BusLevel, ChainEnsureMode, ChainSpec, ChainState, Engine, GraphClock, InsertSlot, Intent,
    NodeName, NodeRole, NodeSpec, PerformanceProfile,
};

use crate::session::Session;

static ENGINE: OnceLock<Mutex<Engine>> = OnceLock::new();
/// Bus → wet. Read from UI; written from worker after FX ops.
static WET_CACHE: OnceLock<RwLock<HashMap<String, bool>>> = OnceLock::new();
/// Dry-meter want list — UI writes, worker applies (never PW RPC on the UI thread).
static DRY_WANT: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static DRY_DIRTY: AtomicBool = AtomicBool::new(false);
/// Last GraphClock seen under ENGINE — dry meters must not lock for this.
static CACHED_SR: AtomicU32 = AtomicU32::new(48_000);
static CACHED_Q: AtomicU32 = AtomicU32::new(256);
static CACHED_SOFT: AtomicBool = AtomicBool::new(true);
/// Monotonic ms since process start of last Interactive Class A engine touch.
static LAST_CLASS_A_MS: AtomicU64 = AtomicU64::new(0);
static PROCESS_START: OnceLock<Instant> = OnceLock::new();

fn process_start() -> Instant {
    *PROCESS_START.get_or_init(Instant::now)
}

fn engine_mutex() -> &'static Mutex<Engine> {
    ENGINE.get_or_init(|| Mutex::new(Engine::new()))
}

fn wet_cache() -> &'static RwLock<HashMap<String, bool>> {
    WET_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn dry_want() -> &'static Mutex<Vec<String>> {
    DRY_WANT.get_or_init(|| Mutex::new(Vec::new()))
}

fn cache_clock_from(clock: &GraphClock) {
    CACHED_SR.store(clock.sample_rate, Ordering::Release);
    CACHED_Q.store(clock.quantum, Ordering::Release);
    CACHED_SOFT.store(clock.soft_quantum, Ordering::Release);
}

fn cached_graph_clock() -> GraphClock {
    GraphClock {
        sample_rate: CACHED_SR.load(Ordering::Acquire).max(8_000),
        quantum: CACHED_Q.load(Ordering::Acquire).max(64),
        soft_quantum: CACHED_SOFT.load(Ordering::Acquire),
        force_suspend_timeout_zero: false,
        bound_device: String::new(),
    }
}

/// Mark Interactive Class A activity (mute/fader/Props) for Supervisor backoff.
pub fn note_class_a() {
    let ms = process_start().elapsed().as_millis() as u64;
    LAST_CLASS_A_MS.store(ms, Ordering::Release);
}

fn class_a_recent(within: Duration) -> bool {
    let last = LAST_CLASS_A_MS.load(Ordering::Acquire);
    if last == 0 {
        return false;
    }
    let now = process_start().elapsed().as_millis() as u64;
    now.saturating_sub(last) < within.as_millis() as u64
}

/// Supervisor idle: true when Interactive touched ENGINE in the last 100ms.
pub fn class_a_is_recent() -> bool {
    class_a_recent(Duration::from_millis(100))
}

fn set_wet_cached(bus: &str, wet: bool) {
    if let Ok(mut g) = wet_cache().write() {
        g.insert(bus.to_string(), wet);
    }
}

fn clear_wet_cache() {
    if let Ok(mut g) = wet_cache().write() {
        g.clear();
    }
}

/// UI-safe: never takes the engine lock or shells out.
pub fn chain_is_wet_cached(bus: &str) -> bool {
    wet_cache()
        .read()
        .ok()
        .and_then(|g| g.get(bus).copied())
        .unwrap_or(false)
}

/// Worker/UI: force wet-cache bit (e.g. after failed ensure / missing FX sink).
pub fn set_chain_wet_cached(bus: &str, wet: bool) {
    set_wet_cached(bus, wet);
}

pub fn with_engine<R>(f: impl FnOnce(&mut Engine) -> R) -> R {
    let mut g = match engine_mutex().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let r = f(&mut g);
    cache_clock_from(g.clock());
    r
}

/// UI-safe: never block. Returns `None` if the worker holds ENGINE (clock bind, apply).
pub fn with_engine_try<R>(f: impl FnOnce(&mut Engine) -> R) -> Option<R> {
    match engine_mutex().try_lock() {
        Ok(mut g) => {
            let r = f(&mut g);
            cache_clock_from(g.clock());
            Some(r)
        }
        Err(std::sync::TryLockError::Poisoned(p)) => {
            let mut g = p.into_inner();
            let r = f(&mut g);
            cache_clock_from(g.clock());
            Some(r)
        }
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

/// Supervisor path: backoff while Interactive Class A is hot (&lt;100ms).
pub fn with_engine_supervisor<R>(f: impl FnOnce(&mut Engine) -> R) -> Option<R> {
    for _ in 0..32 {
        if class_a_recent(Duration::from_millis(100)) {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        match engine_mutex().try_lock() {
            Ok(mut g) => {
                let r = f(&mut g);
                cache_clock_from(g.clock());
                return Some(r);
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(std::sync::TryLockError::Poisoned(p)) => {
                let mut g = p.into_inner();
                let r = f(&mut g);
                cache_clock_from(g.clock());
                return Some(r);
            }
        }
    }
    // Never fall through to a blocking lock — that is how idle reconcile
    // stacked behind clock bind and the UI ANR'd on the same mutex.
    None
}

/// Sync session performance into the engine before graph mutations.
pub fn sync_profile(profile: &PerformanceProfile) {
    with_engine(|eng| eng.set_profile(profile));
}

/// Push mixer DesiredState (buses, levels, FX, egress, Master HW, preferred default).
pub fn sync_desired_from_session(session: &Session, hw_sink: &str) {
    let any_solo = session
        .tracks
        .iter()
        .any(|t| t.solo && !t.kind.is_master());
    let master_id = session.master_id();
    with_engine(|eng| {
        eng.set_profile(&session.performance);
        eng.remember_master_hw(hw_sink);
        eng.desired_mut().set_master_hw(Some(hw_sink.to_string()));
        eng.desired_mut()
            .set_preferred_default(session.preferred_default_sink.clone());

        // Buses that currently have Desired FX — tear down after rebuild if dropped.
        let prev_fx: Vec<String> = eng.desired().fx_chains.keys().cloned().collect();

        eng.desired_mut().buses.clear();
        eng.desired_mut().bus_levels.clear();
        eng.desired_mut().fx_chains.clear();
        eng.desired_mut().bus_egress.clear();
        eng.desired_mut().virtual_inputs.clear();
        eng.desired_mut().bus_inputs.clear();
        eng.desired_mut().bus_playback.clear();
        eng.desired_mut().glc_direct.clear();
        // Master Direct (default on) disables graph GLC; host peer PDC restored.
        eng.desired_mut().glc_disabled = session
            .tracks
            .iter()
            .find(|t| t.kind.is_master())
            .map(|t| t.direct_out)
            .unwrap_or(true);

        for track in &session.tracks {
            let bus = track.expected_sink_name();
            let desc = if track.kind.is_master() {
                "BusChainControl_Master".into()
            } else {
                format!("BusChainControl_{}", track.name.replace(' ', "_"))
            };
            let role = if track.kind.is_master() {
                NodeRole::MasterBus
            } else {
                NodeRole::TrackBus
            };
            if !track.kind.is_master() && track.direct_out {
                eng.desired_mut().glc_direct.insert(bus.clone());
            }
            // Master always public; tracks when VO is on OR Apps-rack pins exist
            // (pins require Pulse-visible Audio/Sink — same invariant as PlaceApp).
            let pulse_export = track.kind.is_master()
                || track.virtual_output
                || !track.assigned_playback.is_empty();
            eng.desired_mut().ensure_bus(NodeSpec {
                name: NodeName::new(&bus),
                description: desc.clone(),
                role,
                start_muted: true,
                pulse_export,
            });
            // Prefer sticky QS/UI fader authority over a stale ApplySession clone.
            let (gain_db, track_mute) =
                crate::daemon::track_mixer_authority_values(track.id)
                    .unwrap_or((track.gain_db, track.mute));
            let mixer_mute =
                track_mute || (any_solo && !track.solo && !track.kind.is_master());
            eng.desired_mut().set_bus_level(
                &bus,
                BusLevel {
                    gain_db,
                    mixer_mute,
                },
            );

            // Egress destination list for sealed arming.
            let mut dests = Vec::new();
            if track.kind.is_master() {
                dests.push(hw_sink.to_string());
            } else {
                let mut targets = track.output_targets.clone();
                // Listen (AFL) always adds Master. Empty Output to… stays empty —
                // hold-only / intermediate bus (do not force Master).
                if track.listen {
                    if let Some(mid) = master_id {
                        if !targets.contains(&mid) {
                            targets.push(mid);
                        }
                    }
                }
                targets.sort();
                targets.dedup();
                for tid in targets {
                    if master_id == Some(tid) {
                        dests.push("buschain_master".into());
                    } else if let Some(t) = session.tracks.iter().find(|t| t.id == tid) {
                        let dest = t.expected_sink_name();
                        if dest != bus {
                            dests.push(dest);
                        }
                    }
                }
                // System virtual input: arm post/bus into feed sink (remap masters .monitor).
                if track.virtual_input {
                    let feed = track.expected_virtual_input_feed_name();
                    let vin_desc = format!("{desc}_In");
                    eng.desired_mut().ensure_bus(NodeSpec {
                        name: NodeName::new(&feed),
                        description: format!("{desc}_VinFeed"),
                        role: NodeRole::VirtualInputFeed,
                        start_muted: false,
                        pulse_export: false,
                    });
                    // Explicit empty egress — heal must not default vinf→Master.
                    eng.desired_mut().set_bus_egress(&feed, Vec::new());
                    eng.desired_mut()
                        .set_virtual_input(&bus, Some(vin_desc));
                    if !dests.iter().any(|d| d == &feed) {
                        dests.push(feed);
                    }
                }
            }
            eng.desired_mut().set_bus_egress(&bus, dests.clone());

            // Shared capture rack (unmuted rows only). Soft-bind happens in session store.
            if !track.kind.is_master() {
                let sources: Vec<String> = track
                    .inputs
                    .iter()
                    .filter(|i| !i.mute)
                    .map(|i| i.source.clone())
                    .filter(|s| {
                        !s.is_empty()
                            && s != "(none)"
                            && !s.starts_with("buschain_")
                            && s != &track.expected_virtual_input_name()
                    })
                    .collect();
                eng.desired_mut().set_bus_inputs(&bus, sources);
            }

            // Session-pinned playback apps (Apps rack).
            if !track.assigned_playback.is_empty() {
                eng.desired_mut()
                    .set_bus_playback(&bus, track.assigned_playback.clone());
            }

            if let Some(mut spec) = crate::audio::insert_map::chain_spec_for_track(
                session,
                track.id,
                &crate::audio::insert_map::primary_fx_dest(session, track.id, hw_sink),
            ) {
                if !spec.inserts.is_empty() {
                    // Dest already set by primary_fx_dest; keep in lockstep with egress.
                    if let Some(d) = dests.first() {
                        spec.dest = d.clone();
                    }
                    eng.desired_mut().ensure_fx_chain(spec);
                }
            }
        }

        let keep: std::collections::HashSet<String> =
            eng.desired().fx_chains.keys().cloned().collect();
        for bus in prev_fx {
            if !keep.contains(&bus) {
                let _ = eng.teardown_fx_chain(&bus);
            }
        }
        // Also drop live FX sinks for buses that are no longer Desired-wet.
        let buses: Vec<String> = eng.desired().buses.keys().cloned().collect();
        for bus in buses {
            if keep.contains(&bus) {
                continue;
            }
            if buschain_engine::any_gen_live(&bus) || eng.chain_is_wet(&bus) {
                let _ = eng.teardown_fx_chain(&bus);
            }
        }
        // Session / mixer authority owns mute. Align latches to what we just wrote
        // so a prior DualMic mute cannot survive unmute and strip post→Master while
        // FX meters stay live. Then reinforce only still-latched (UI-muted) buses.
        eng.align_mute_latches_to_desired();
        eng.reinforce_mute_latches();
    });
}

/// After PipeWire daemon restart: reconnect native plane is caller's job;
/// this tears FX hosts and cold-arms Desired (`force_fx`).
pub fn reconnect_pipewire_graph(session: &mut Session) -> anyhow::Result<String> {
    let _ = session.ensure_assigned_playback_vo();
    let _ = session.ensure_buschain_preferred_default();
    // Sticky Master, else desktop / non-BusChain system default (plug-n-play).
    let hw = session
        .master_output
        .clone()
        .filter(|n| !n.is_empty() && !n.starts_with("buschain_"))
        .or_else(|| crate::audio::graph::seed_master_hw_plug_and_play(session))
        .or_else(|| crate::audio::graph::resolve_hardware_output(session).ok())
        .unwrap_or_default();
    if hw.is_empty() {
        return Err(anyhow::anyhow!(
            "PipeWire reconnect: no Master HW sink in session"
        ));
    }
    crate::audio::engine_handle::remember_master_hw(&hw);
    session.master_output = Some(hw.clone());
    let desc = session.master_output_desc.clone();
    crate::audio::graph::remember_desktop_hw(session, &hw, desc.as_deref());
    sync_desired_from_session(session, &hw);
    let (msg, buses) = with_engine(|eng| {
        let report = eng.apply(Intent::ReconnectPipeWire)?;
        let buses: Vec<String> = eng.desired().buses.keys().cloned().collect();
        Ok::<_, anyhow::Error>((report.join(), buses))
    })?;
    for bus in &buses {
        let wet = with_engine(|eng| eng.chain_is_wet(bus));
        set_wet_cached(bus, wet);
    }
    for track in &session.tracks {
        let bus = track.expected_sink_name();
        if track.inserts.is_empty() {
            continue;
        }
        if let Some(spec) =
            crate::audio::insert_map::chain_spec_for_track(session, track.id, &hw)
        {
            let _ = with_engine(|eng| eng.push_fx_controls(&bus, spec.inserts));
        }
    }
    let _ = crate::audio::graph::apply_track_levels(session);
    if let Some(pref) = session.preferred_default_sink.clone() {
        let _ = crate::audio::graph::set_default_sink_if_needed(&pref);
    }
    let _ = sync_playback(session);
    Ok(msg)
}

/// Full Apply / launch — warm-adopts a healthy live graph; otherwise sealed cold arm.
pub fn arm_session(session: &Session, hw_sink: &str, force_fx: bool) -> anyhow::Result<String> {
    sync_desired_from_session(session, hw_sink);
    let (msg, buses) = with_engine(|eng| {
        let report = if eng.session_graph_healthy() {
            eng.adopt_live_session()?
        } else {
            eng.apply(Intent::ArmSession { force_fx })?
        };
        let buses: Vec<String> = eng.desired().buses.keys().cloned().collect();
        Ok::<_, anyhow::Error>((report.join(), buses))
    })?;
    for bus in &buses {
        let wet = with_engine(|eng| eng.chain_is_wet(bus));
        set_wet_cached(bus, wet);
    }
    // Push bypass/mix/params from session into live filter-chains after arm.
    for track in &session.tracks {
        let bus = track.expected_sink_name();
        if track.inserts.is_empty() {
            continue;
        }
        if let Some(spec) =
            crate::audio::insert_map::chain_spec_for_track(session, track.id, hw_sink)
        {
            let _ = with_engine(|eng| eng.push_fx_controls(&bus, spec.inserts));
        }
    }
    Ok(msg)
}

/// Ensure or tear down system virtual input for one track (session flag is truth).
pub fn apply_virtual_input(session: &Session, track_id: uuid::Uuid) -> anyhow::Result<String> {
    let hw = crate::audio::graph::resolve_hardware_output(session)
        .unwrap_or_else(|_| session.master_output.clone().unwrap_or_default());
    sync_desired_from_session(session, &hw);
    let track = session
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .ok_or_else(|| anyhow::anyhow!("track not found"))?;
    if track.kind.is_master() {
        return Ok("virtual input: master ignored".into());
    }
    let bus = track.expected_sink_name();
    let desc = format!(
        "BusChainControl_{}_In",
        track.name.replace(' ', "_")
    );
    let report = with_engine(|eng| {
        if track.virtual_input {
            eng.apply(Intent::EnsureVirtualInput {
                bus: NodeName::new(&bus),
                description: desc,
            })
        } else {
            eng.apply(Intent::TeardownVirtualInput {
                bus: NodeName::new(&bus),
            })
        }
    })?;
    // Relink so feed is in/out of egress allow-lists.
    let _ = relink_routes(session, &hw);
    Ok(report.join())
}

/// Arm one track's configured egress hops (wet post→dests or dry bus→dests).
///
/// Wet + host alive: always attempt post→dest (Master or track→Master). A flaky
/// spine probe must not leave the bus dry/hold while the in-process host is up.
pub fn arm_track_egress(bus: &str, wet: bool) -> anyhow::Result<()> {
    with_engine(|eng| {
        // Master→HW stays behind the session barrier (ensure_fx_chain already
        // honors this; rewire_track_fx must not punch through).
        if bus == "buschain_master" && !eng.desired().speakers_armed {
            return Ok(());
        }
        let dests = eng.desired().egress_dests(bus);
        if wet {
            let helper = buschain_engine::any_gen_live(bus) || eng.chain_is_wet(bus);
            if helper && buschain_engine::spine_instant_ready(bus) {
                eng.arm_track_egress(bus, true, &dests)?;
                set_wet_cached(bus, true);
            } else {
                // FX claimed but post spine not ready — keep dry audible (never hold-only).
                eng.arm_track_egress(bus, false, &dests)?;
                set_wet_cached(bus, false);
            }
        } else if buschain_engine::spine_instant_ready(bus) {
            // Host owns the bus — prefer wet exclusive when spine is actually up.
            eng.arm_track_egress(bus, true, &dests)?;
            set_wet_cached(bus, true);
        } else {
            eng.arm_track_egress(bus, false, &dests)?;
            set_wet_cached(bus, false);
        }
        Ok(())
    })
}

/// Disarm track egress (hold-only). Used at start of route rewire when FX pending.
pub fn disarm_track_egress(bus: &str, keep_fx_feed: bool) {
    with_engine(|eng| eng.disarm_track_egress(bus, keep_fx_feed));
    set_wet_cached(bus, false);
}

/// Run engine GraphSupervisor reconcile; refresh wet cache for known buses.
pub fn reconcile(session: &Session, hw_sink: &str) -> anyhow::Result<String> {
    sync_desired_from_session(session, hw_sink);
    let (msg, buses) = with_engine(|eng| {
        let report = eng.reconcile()?;
        let buses: Vec<String> = eng.desired().buses.keys().cloned().collect();
        Ok::<_, anyhow::Error>((report.join(), buses))
    })?;
    for bus in buses {
        let wet = with_engine(|eng| eng.chain_is_wet(&bus));
        set_wet_cached(&bus, wet);
    }
    Ok(msg)
}

/// Route / Hotplug: links + egress only — never ForceRespawn FX racks.
pub fn relink_routes(session: &Session, hw_sink: &str) -> anyhow::Result<String> {
    sync_desired_from_session(session, hw_sink);
    let (msg, buses) = with_engine(|eng| {
        let report = eng.relink_routes()?;
        let buses: Vec<String> = eng.desired().buses.keys().cloned().collect();
        Ok::<_, anyhow::Error>((report.join(), buses))
    })?;
    for bus in buses {
        let wet = with_engine(|eng| eng.chain_is_wet(&bus));
        set_wet_cached(&bus, wet);
    }
    Ok(msg)
}

/// Route / Hotplug egress path: sync Desired → `relink_routes` only.
/// Capture is `sync_capture_delta` / cold `SyncCapture` — never bundled here.
pub fn rewire_session_routes(session: &Session, hw_sink: &str) -> anyhow::Result<String> {
    sync_desired_from_session(session, hw_sink);
    let (msg, buses) = with_engine(|eng| {
        let report = eng.relink_routes()?;
        let buses: Vec<String> = eng.desired().buses.keys().cloned().collect();
        Ok::<_, anyhow::Error>((report.join(), buses))
    })?;
    for bus in buses {
        let wet = with_engine(|eng| eng.chain_is_wet(&bus));
        set_wet_cached(&bus, wet);
    }
    Ok(msg)
}

/// Patch one track's Desired `bus_inputs` and apply `Intent::SyncCaptureDelta`.
pub fn sync_capture_delta(session: &Session, track_id: uuid::Uuid) -> anyhow::Result<String> {
    let track = session
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .ok_or_else(|| anyhow::anyhow!("track not found"))?;
    if track.kind.is_master() {
        return Ok("capture: master has no In rack".into());
    }
    let bus = track.expected_sink_name();
    let want: Vec<String> = track
        .inputs
        .iter()
        .filter(|i| !i.mute)
        .map(|i| i.source.clone())
        .filter(|s| {
            !s.is_empty()
                && s != "(none)"
                && !s.starts_with("buschain_")
                && s != &track.expected_virtual_input_name()
        })
        .collect();
    // Keep Desired egress for this bus (Capture must not leave hold-only silence).
    patch_bus_egress(session, track_id);
    let unmuted = !track.mute;
    let report = with_engine(|eng| {
        let prev = eng
            .last_applied_bus_inputs()
            .get(&bus)
            .cloned()
            .unwrap_or_default();
        // Force-recreate: new sources OR fingerprinted-but-dead (ghost In).
        let (remove, add) = {
            let desired = eng.desired();
            buschain_engine::runtime::plan_capture_delta(&prev, &want, |s| {
                buschain_engine::backend::capture_hop_verified(s, &bus, desired)
            })
        };
        eng.desired_mut().set_bus_inputs(&bus, want);
        let report = eng.apply(Intent::SyncCaptureDelta {
            bus: bus.clone(),
            remove,
            add,
        })?;
        // Heal dry/wet egress if capture is live but track→Master was never armed
        // (EnsureTrack used to rely on demoted Pulse loopback).
        if unmuted && !eng.mixer_muted_public(&bus) {
            let dests = eng.desired().egress_dests(&bus);
            let wet = buschain_engine::spine_instant_ready(&bus);
            if !dests.is_empty()
                && !eng.egress_to_dests_live(&bus, wet, &dests)
                && !eng.egress_to_dests_live(&bus, false, &dests)
            {
                let _ = eng.arm_track_egress(&bus, wet, &dests);
                if !eng.egress_to_dests_live(&bus, true, &dests)
                    && !eng.egress_to_dests_live(&bus, false, &dests)
                {
                    let _ = eng.arm_track_egress(&bus, false, &dests);
                }
                eng.mark_monitor_mute_applied(&bus, false);
            }
        }
        Ok::<_, anyhow::Error>(report.join())
    })?;
    Ok(report)
}

/// Patch one track's Desired `bus_egress` from session output targets.
pub fn patch_bus_egress(session: &Session, track_id: uuid::Uuid) {
    let Some(track) = session.tracks.iter().find(|t| t.id == track_id) else {
        return;
    };
    if track.kind.is_master() {
        // Master Direct toggles global GLC disable.
        // UI click — never block behind clock bind / apply.
        let _ = with_engine_try(|eng| {
            eng.desired_mut().glc_disabled = track.direct_out;
        });
        return;
    }
    let bus = track.expected_sink_name();
    let master_id = session.master_id();
    let mut dests = Vec::new();
    let mut targets = track.output_targets.clone();
    if track.listen {
        if let Some(mid) = master_id {
            if !targets.contains(&mid) {
                targets.push(mid);
            }
        }
    }
    targets.sort();
    targets.dedup();
    for tid in targets {
        if master_id == Some(tid) {
            dests.push("buschain_master".into());
        } else if let Some(t) = session.tracks.iter().find(|t| t.id == tid) {
            let dest = t.expected_sink_name();
            if dest != bus {
                dests.push(dest);
            }
        }
    }
    if track.virtual_input {
        let feed = track.expected_virtual_input_feed_name();
        if !dests.iter().any(|d| d == &feed) {
            dests.push(feed);
        }
    }
    let _ = with_engine_try(|eng| {
        eng.desired_mut().set_bus_egress(&bus, dests);
        if track.direct_out {
            eng.desired_mut().glc_direct.insert(bus.clone());
        } else {
            eng.desired_mut().glc_direct.remove(&bus);
        }
        // Keep Master Direct → glc_disabled in lockstep if session already updated.
        if let Some(m) = session.tracks.iter().find(|t| t.kind.is_master()) {
            eng.desired_mut().glc_disabled = m.direct_out;
        }
    });
}

/// PlaceApp: patch `bus_playback` only — never full Desired rebuild.
pub fn patch_bus_playback(session: &Session, track_id: uuid::Uuid) {
    let Some(track) = session.tracks.iter().find(|t| t.id == track_id) else {
        return;
    };
    let bus = track.expected_sink_name();
    with_engine(|eng| {
        eng.desired_mut()
            .set_bus_playback(&bus, track.assigned_playback.clone());
    });
}

/// One-shot Apps place: sync Desired → `Intent::SyncPlayback` (single list pass).
pub fn sync_playback(session: &mut Session) -> anyhow::Result<String> {
    // Pins require Pulse-visible VO buses — same invariant as PlaceApp.
    let _ = session.ensure_assigned_playback_vo();
    let hw = crate::audio::graph::resolve_hardware_output(session)
        .unwrap_or_else(|_| session.master_output.clone().unwrap_or_default());
    sync_desired_from_session(session, &hw);
    with_engine(|eng| {
        let report = eng.apply(Intent::SyncPlayback)?;
        Ok(report.join())
    })
}

pub fn bind_master_clock(profile: &PerformanceProfile) -> anyhow::Result<String> {
    with_engine(|eng| {
        let report = eng.apply(Intent::BindMasterClock {
            profile: profile.clone(),
        })?;
        Ok(report.join())
    })
}

/// Apply clock to one HW device. `bind_buschain` force-rates Master HW only (GraphClock unchanged).
pub fn bind_device_clock(
    device: &str,
    sample_rate: u32,
    quantum: u32,
    soft_quantum: bool,
    bind_buschain: bool,
) -> anyhow::Result<String> {
    with_engine(|eng| {
        let report = eng.apply(Intent::BindDeviceClock {
            device: device.to_string(),
            sample_rate,
            quantum,
            soft_quantum,
            bind_buschain,
        })?;
        Ok(report.join())
    })
}

pub fn ensure_bus(name: &str, description: &str) -> anyhow::Result<()> {
    let role = role_for_name(name);
    let start_muted = matches!(role, NodeRole::TrackBus | NodeRole::MasterBus);
    with_engine(|eng| {
        // Prefer Desired.pulse_export (VO flag from session sync) over role default.
        let pulse_export = eng
            .desired()
            .buses
            .get(name)
            .map(|s| s.pulse_export)
            .unwrap_or_else(|| role.default_pulse_export());
        eng.apply(Intent::EnsureBus {
            spec: NodeSpec {
                name: NodeName::new(name),
                description: description.into(),
                role,
                start_muted,
                pulse_export,
            },
        })?;
        // Immediately open — never leave create-mute@0 for the UI / apps.
        let level = eng
            .desired()
            .bus_levels
            .get(name)
            .copied()
            .unwrap_or(BusLevel {
                gain_db: 0.0,
                mixer_mute: false,
            });
        eng.desired_mut().ensure_bus(NodeSpec {
            name: NodeName::new(name),
            description: description.into(),
            role,
            start_muted,
            pulse_export,
        });
        let _ = eng.apply(Intent::SetLevels {
            sink: name.into(),
            gain_db: level.gain_db,
            muted: false, // never cork
        });
        Ok(())
    })
}

/// Shared capture by default — exclusive `unlink_from_source_except` steals the
/// HW mic from desktop apps and from other tracks. Rate-bridge dual-path is
/// avoided by sink-side cleanup in the graph shim, not source-side exclusivity.
/// Keepalive `{bus}.monitor → buschain_hold` must also stay non-exclusive.
pub fn ensure_route(source: &str, sink: &str) -> anyhow::Result<()> {
    with_engine(|eng| eng.ensure_route(source, sink, false))
}

pub fn ensure_link_raw(source: &str, sink: &str) -> anyhow::Result<()> {
    with_engine(|eng| eng.ensure_link_raw(source, sink))
}

pub fn link_is_live(source: &str, sink: &str) -> bool {
    Engine::link_is_live(source, sink)
}

pub fn unlink_from_source_except(source: &str, allow: &[&str]) -> u32 {
    with_engine(|eng| eng.unlink_from_source_except(source, allow))
}

pub fn unlink_raw(source: &str, sink: &str) -> anyhow::Result<()> {
    with_engine(|eng| eng.unlink_raw(source, sink))
}

pub fn teardown_links() {
    with_engine(|eng| eng.teardown_links());
}

/// Destroy a BusChain null-sink / helper (native linger first, Pulse fallback).
pub fn destroy_node(name: &str) -> anyhow::Result<()> {
    with_engine(|eng| eng.destroy_node(name))
}

/// Full engine teardown (clears DesiredState + `speakers_armed`).
pub fn engine_teardown() -> anyhow::Result<String> {
    with_engine(|eng| {
        let report = eng.apply(Intent::Teardown)?;
        Ok(report.join())
    })
}

/// Drop only `buschain_rs_*` helpers (inbound rate bridges). Does not touch route links.
pub fn teardown_rate_bridges() {
    with_engine(|eng| eng.destroy_rate_bridges_only());
}

pub fn filter_chain_clock_fragment() -> String {
    with_engine(|eng| eng.filter_chain_clock_fragment())
}

pub fn ensure_fx_chain(
    spec: ChainSpec,
    mode: ChainEnsureMode,
) -> anyhow::Result<ChainState> {
    let bus = spec.bus.as_str().to_string();
    let span = buschain_engine::fx_trace::span("EnsureFxChain");
    let mode_tag = format!("{mode:?}");
    let state = with_engine(|eng| eng.ensure_fx_chain(spec, mode))?;
    set_wet_cached(&bus, state.is_wet());
    span.end(format!("{bus} {mode_tag} wet={}", state.is_wet()));
    Ok(state)
}

/// Relink Master→HW without ApplySession / ForceRespawn.
pub fn set_master_hw_light(hw: &str) -> anyhow::Result<String> {
    with_engine(|eng| {
        let report = eng.apply_master_hw_relink(hw)?;
        Ok(report.messages.join(" · "))
    })
}

/// After engine ClockBind: rebuild Master→HW via clocked egress (not raw pw-link).
pub fn arm_master_hw_after_clock() -> anyhow::Result<String> {
    with_engine(|eng| {
        let report = eng.arm_master_hw_egress()?;
        Ok(report.messages.join(" · "))
    })
}

pub fn push_fx_controls(bus: &str, inserts: Vec<InsertSlot>) -> anyhow::Result<()> {
    // In-process host control queue — lock-free of Engine ForceRespawn mutex.
    buschain_engine::host::registry::push_host_controls(bus, &inserts)?;
    // Always mirror Desired — never skip on try_lock (idle reconcile must see knobs).
    let inserts_for_desired = inserts;
    with_engine(|eng| -> anyhow::Result<()> {
        if let Some(spec) = eng.desired_mut().fx_chains.get_mut(bus) {
            spec.inserts = inserts_for_desired;
        }
        Ok(())
    })?;
    Ok(())
}

pub fn teardown_fx_chain(bus: &str) -> anyhow::Result<()> {
    let r = with_engine(|eng| eng.teardown_fx_chain(bus));
    set_wet_cached(bus, false);
    r
}

/// Worker-only: probe PipeWire and refresh the UI cache.
pub fn chain_is_wet(bus: &str) -> bool {
    let wet = with_engine(|eng| eng.chain_is_wet(bus));
    set_wet_cached(bus, wet);
    wet
}

pub fn remember_master_hw(hw: &str) {
    with_engine(|eng| eng.remember_master_hw(hw));
}

/// Native-preferring sink/source levels (db + mute).
pub fn set_levels(sink: &str, gain_db: f32, muted: bool) -> anyhow::Result<()> {
    note_class_a();
    with_engine(|eng| {
        eng.apply(Intent::SetLevels {
            sink: sink.to_string(),
            gain_db,
            muted,
        })?;
        Ok(())
    })
}

/// Flip mute without clobbering the current fader gain (reads DesiredState).
pub fn set_mute(sink: &str, muted: bool) -> anyhow::Result<()> {
    with_engine(|eng| {
        let gain = eng
            .desired()
            .bus_levels
            .get(sink)
            .map(|l| l.gain_db)
            .unwrap_or(0.0);
        eng.apply(Intent::SetLevels {
            sink: sink.to_string(),
            gain_db: gain,
            muted,
        })?;
        Ok(())
    })
}

/// Instant mixer mute — disarm/arm egress + pactl monitor mute (never cork app sink).
pub fn gate_track_mute(bus: &str, muted: bool) -> anyhow::Result<()> {
    note_class_a();
    with_engine(|eng| eng.gate_track_mute(bus, muted))
}

pub fn set_default_sink(name: &str) -> anyhow::Result<bool> {
    with_engine(|eng| eng.set_default_sink(name))
}

pub fn graph_snapshot() -> anyhow::Result<buschain_engine::GraphSnapshot> {
    with_engine(|eng| eng.snapshot())
}

/// Host pre/post insert meter peaks (prefer over Pulse meter-* for FX buses).
pub fn host_meter_peaks(bus: &str) -> Option<(f32, f32)> {
    buschain_engine::host::registry::host_meter_peaks(bus)
}

/// In-process FX host is live for this bus (prefer host meters over Pulse).
pub fn host_is_live(bus: &str) -> bool {
    buschain_engine::host::registry::host_is_live(bus)
}

/// Native dry-bus monitor peaks (no Pulse meter streams).
pub fn dry_meter_peaks(bus: &str) -> Option<(f32, f32)> {
    buschain_engine::host::dry_meter::dry_meter_peaks(bus)
}

pub fn dry_meter_is_live(bus: &str) -> bool {
    buschain_engine::host::dry_meter::dry_meter_is_live(bus)
}

/// UI: request dry peak taps. Never takes ENGINE or talks to PipeWire.
pub fn sync_dry_meters(buses: &[String]) {
    if let Ok(mut g) = dry_want().lock() {
        g.clear();
        g.extend(buses.iter().cloned());
    }
    DRY_DIRTY.store(true, Ordering::Release);
}

/// Worker: create/tear dry taps. Skip while clock mutate so we don't add PW churn.
pub fn apply_pending_dry_meters() {
    if buschain_engine::clock_mutation_in_flight() {
        return;
    }
    if !DRY_DIRTY.swap(false, Ordering::AcqRel) {
        return;
    }
    let buses = dry_want()
        .lock()
        .ok()
        .map(|g| g.clone())
        .unwrap_or_default();
    buschain_engine::host::dry_meter::sync_dry_meters(&buses, &cached_graph_clock());
}

/// Keep FFT running for `bus` and return a spectrum snapshot (post-FX when `post`).
pub fn host_spectrum(bus: &str, post: bool) -> Option<buschain_engine::host::SpectrumFrame> {
    buschain_engine::host::registry::host_spectrum(bus, post)
}

pub fn host_spectrum_watch(bus: &str, post: bool) {
    buschain_engine::host::registry::host_spectrum_watch(bus, post);
}

/// Stop all host FFTs immediately (UI Idle / window withdrawn).
pub fn host_spectrum_clear_watches() {
    buschain_engine::host::registry::host_spectrum_clear_watches();
}

pub fn host_latency_ms(bus: &str, sample_rate: u32) -> f32 {
    let samples = buschain_engine::host::reported_latency(bus);
    if sample_rate == 0 {
        return 0.0;
    }
    samples as f32 * 1000.0 / sample_rate as f32
}

/// Master-edge GLC compensation pad (samples / ms) for fan-in align readout.
pub fn host_compensation_ms(bus: &str, sample_rate: u32) -> (u32, f32) {
    let samples = buschain_engine::master_pad_samples(bus);
    samples_to_ms(samples, sample_rate)
}

/// Path latency L(T) to wet out (samples / ms).
pub fn host_path_latency_ms(bus: &str, sample_rate: u32) -> (u32, f32) {
    let samples = buschain_engine::path_latency_samples(bus);
    samples_to_ms(samples, sample_rate)
}

/// Master fan-in L* (longest stem path before pads).
pub fn host_lstar_ms(sample_rate: u32) -> (u32, f32) {
    let samples = buschain_engine::l_star_samples();
    samples_to_ms(samples, sample_rate)
}

fn samples_to_ms(samples: u32, sample_rate: u32) -> (u32, f32) {
    if sample_rate == 0 {
        return (samples, 0.0);
    }
    (
        samples,
        samples as f32 * 1000.0 / sample_rate as f32,
    )
}

/// One GraphClock quantum in ms (device/graph period).
pub fn hw_quantum_ms(sample_rate: u32, quantum: u32) -> f32 {
    if sample_rate == 0 {
        return 0.0;
    }
    quantum as f32 * 1000.0 / sample_rate as f32
}

/// True when Master Direct has disabled Master-edge GLC.
pub fn glc_is_disabled() -> bool {
    buschain_engine::glc_is_disabled()
}

pub fn stop_all_fx() {
    with_engine(|eng| eng.stop_all_fx());
    clear_wet_cache();
}

fn role_for_name(name: &str) -> NodeRole {
    if name == "buschain_master" {
        NodeRole::MasterBus
    } else if name == "buschain_hold" {
        NodeRole::Hold
    } else if name.starts_with("buschain_post_") {
        NodeRole::PostBus
    } else if name.starts_with("buschain_fx_") {
        NodeRole::FxSink
    } else if name.starts_with("buschain_rs_") {
        NodeRole::RateBridge
    } else if name.starts_with("buschain_track_") {
        NodeRole::TrackBus
    } else {
        NodeRole::TrackBus
    }
}
