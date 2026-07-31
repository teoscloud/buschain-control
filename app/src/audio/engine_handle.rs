//! Process-wide [`buschain_engine::Engine`] for the audio worker thread.
//!
//! Graph/filter shims call into this instead of shelling out to pactl/pw-link.
//!
//! **UI thread must never take the engine mutex.** Wetness for meters is a
//! lock-free cache updated only by the worker after FX ensure/teardown/probe.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, RwLock};

use buschain_engine::{
    BusLevel, ChainEnsureMode, ChainSpec, ChainState,
    Engine, InsertSlot, Intent, NodeName, NodeRole, NodeSpec, PerformanceProfile,
};

use crate::session::Session;

static ENGINE: OnceLock<Mutex<Engine>> = OnceLock::new();
/// Bus → wet. Read from UI; written from worker after FX ops.
static WET_CACHE: OnceLock<RwLock<HashMap<String, bool>>> = OnceLock::new();

fn engine_mutex() -> &'static Mutex<Engine> {
    ENGINE.get_or_init(|| Mutex::new(Engine::new()))
}

fn wet_cache() -> &'static RwLock<HashMap<String, bool>> {
    WET_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
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
    let mut g = engine_mutex().lock().expect("buschain-engine lock");
    f(&mut g)
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
            eng.desired_mut().ensure_bus(NodeSpec {
                name: NodeName::new(&bus),
                description: desc.clone(),
                role,
                start_muted: true,
            });
            let mixer_mute =
                track.mute || (any_solo && !track.solo && !track.kind.is_master());
            eng.desired_mut().set_bus_level(
                &bus,
                BusLevel {
                    gain_db: track.gain_db,
                    mixer_mute,
                },
            );

            // Egress destination list for sealed arming.
            let mut dests = Vec::new();
            if track.kind.is_master() {
                dests.push(hw_sink.to_string());
            } else {
                let mut targets = track.output_targets.clone();
                if targets.is_empty() || track.listen {
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
                if dests.is_empty() {
                    dests.push("buschain_master".into());
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
                    });
                    eng.desired_mut()
                        .set_virtual_input(&bus, Some(vin_desc));
                    if !dests.iter().any(|d| d == &feed) {
                        dests.push(feed);
                    }
                }
            }
            eng.desired_mut().set_bus_egress(&bus, dests.clone());

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
    });
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
            if !helper && !buschain_engine::spine_instant_ready(bus) {
                eng.disarm_track_egress(bus, true);
                set_wet_cached(bus, false);
                return Ok(());
            }
            eng.arm_track_egress(bus, true, &dests)?;
            set_wet_cached(bus, true);
        } else {
            // Never dry-bypass while an FX helper still owns the bus.
            if buschain_engine::any_gen_live(bus) {
                eng.arm_track_egress(bus, true, &dests)?;
                set_wet_cached(bus, true);
                return Ok(());
            }
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

pub fn bind_master_clock(profile: &PerformanceProfile) -> anyhow::Result<String> {
    with_engine(|eng| {
        let report = eng.apply(Intent::BindMasterClock {
            profile: profile.clone(),
        })?;
        Ok(report.join())
    })
}

/// Apply clock to one HW device. `bind_buschain` migrates BusChain buses (Master HW out).
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
        eng.ensure_bus(name, description, role, start_muted)?;
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
        });
        // open via reconcile helpers on backend
        let _ = eng.apply(Intent::SetLevels {
            sink: name.into(),
            gain_db: level.gain_db,
            muted: false, // never cork
        });
        Ok(())
    })
}

/// Mic→bus (and similar) are exclusive so we never dual-path with a rate-bridge.
/// Keepalive `{bus}.monitor → buschain_hold` must NOT be exclusive — that would
/// strip bus→fx / bus→dest and silence the mix.
pub fn ensure_route(source: &str, sink: &str) -> anyhow::Result<()> {
    let exclusive = sink != "buschain_hold";
    with_engine(|eng| eng.ensure_route(source, sink, exclusive))
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
