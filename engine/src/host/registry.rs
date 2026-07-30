//! Per-bus insert host registry — node + control queue + published rack.

use std::collections::HashMap;
use std::sync::Mutex;
use std::thread;

use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use uuid::Uuid;

use crate::backend::ladspa_search_path;
use crate::clock::GraphClock;
use crate::domain::{normalize_ladspa_label, ChainSpec, InsertSlot};

use super::control::ControlMsg;
use super::node::PwFxNode;
use super::rack::Rack;

static REGISTRY: Lazy<Mutex<HashMap<String, HostTrack>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub struct HostTrack {
    pub node: PwFxNode,
    pub fingerprint: String,
}

/// Ensure filter node + publish rack from inserts. Returns fx node name.
pub fn ensure_host(
    bus: &str,
    inserts: &[InsertSlot],
    clock: &GraphClock,
    force_rebuild: bool,
) -> Result<String> {
    if inserts.is_empty() {
        teardown_host(bus);
        return Err(anyhow!("empty rack"));
    }
    let fx_name = crate::domain::fx_name_for_bus(bus);
    let want_fp = crate::domain::inserts_signature(inserts);
    let sr = clock_sr(clock);
    let quantum = clock_q(clock);
    let path = ladspa_search_path();

    // Fast path under lock — never block the registry on PW wait / dlopen.
    {
        let mut reg = REGISTRY.lock().unwrap();
        if let Some(track) = reg.get_mut(bus) {
            if !force_rebuild && track.fingerprint == want_fp && track.node.is_running() {
                push_insert_controls_to_queue(&track.node, inserts);
                return Ok(fx_name);
            }
            // Rebuild rack off-lock below when we only need a gen-swap.
            if track.node.is_running() {
                let bus_owned = bus.to_string();
                // Capture live CLAP/VST3 state before the old rack is dropped.
                let snaps = harvest_from_node(&track.node);
                drop(reg);
                let mut inserts_owned = inserts.to_vec();
                apply_harvest_to_inserts(&mut inserts_owned, &snaps);
                let rack = Rack::build(&inserts_owned, sr, quantum, &path)?;
                let mut reg = REGISTRY.lock().unwrap();
                if let Some(track) = reg.get_mut(&bus_owned) {
                    track.fingerprint = want_fp;
                    track.node.publish_rack(rack);
                }
                drop(reg);
                crate::host::node_latency::publish_bus_latency(&bus_owned);
                return Ok(fx_name);
            }
        }
    }

    crate::host::orphan::sweep_orphan_helpers_once();

    // Start filter + build rack without holding the registry (other buses can proceed).
    let node = PwFxNode::start(bus, sr, quantum)?;
    let rack = Rack::build(inserts, sr, quantum, &path)?;
    node.publish_rack(rack);
    let bus_owned = bus.to_string();

    let mut reg = REGISTRY.lock().unwrap();
    // Another thread may have won — prefer the survivor.
    if let Some(existing) = reg.get(bus) {
        if existing.node.is_running() {
            drop(node); // tear down duplicate
            if let Some(track) = reg.get_mut(bus) {
                if track.fingerprint != want_fp {
                    let snaps = harvest_from_node(&track.node);
                    let mut inserts_owned = inserts.to_vec();
                    apply_harvest_to_inserts(&mut inserts_owned, &snaps);
                    let rack = Rack::build(&inserts_owned, sr, quantum, &path)?;
                    track.fingerprint = want_fp;
                    track.node.publish_rack(rack);
                }
            }
            drop(reg);
            crate::host::node_latency::publish_bus_latency(&bus_owned);
            return Ok(fx_name);
        }
    }
    reg.insert(
        bus_owned.clone(),
        HostTrack {
            node,
            fingerprint: want_fp,
        },
    );
    drop(reg);
    crate::host::node_latency::publish_bus_latency(&bus_owned);
    Ok(fx_name)
}

/// Cold ArmSession helper: bring up many hosts concurrently (one filter thread each).
pub fn ensure_hosts_parallel(
    specs: &[ChainSpec],
    clock: &GraphClock,
    force: bool,
) -> Vec<(String, Result<()>)> {
    if specs.is_empty() {
        return Vec::new();
    }
    crate::host::orphan::sweep_orphan_helpers_once();
    let clock = clock.clone();

    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(specs.len());
        for spec in specs {
            let bus = spec.bus.as_str().to_string();
            let inserts = spec.inserts.clone();
            let clock = clock.clone();
            handles.push(scope.spawn(move || {
                let r = ensure_host(&bus, &inserts, &clock, force);
                (bus, r.map(|_| ()))
            }));
        }
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect()
    })
}

fn clock_sr(clock: &GraphClock) -> u32 {
    if clock.sample_rate > 0 {
        clock.sample_rate
    } else {
        48_000
    }
}

fn clock_q(clock: &GraphClock) -> u32 {
    if clock.quantum > 0 {
        clock.quantum
    } else {
        256
    }
}

pub fn push_host_param(bus: &str, slot_id: Uuid, control_index: u32, value: f32) {
    let reg = REGISTRY.lock().unwrap();
    if let Some(track) = reg.get(bus) {
        track
            .node
            .state
            .queue
            .push(ControlMsg::param(slot_id, control_index, value));
    }
}

/// Off-RT snapshot of live slot state (blob + named params) before Props/rewire.
///
/// Locks the published rack briefly — RT uses `try_lock` and skips a block if contended.
#[derive(Debug, Clone)]
pub struct SlotStateSnapshot {
    pub slot_id: Uuid,
    pub state_blob: Option<Vec<u8>>,
    pub controls: Vec<(String, f32)>,
}

pub fn harvest_host_slot_states(bus: &str) -> Vec<SlotStateSnapshot> {
    let reg = REGISTRY.lock().unwrap();
    let Some(track) = reg.get(bus) else {
        return Vec::new();
    };
    harvest_from_node(&track.node)
}

fn harvest_from_node(node: &PwFxNode) -> Vec<SlotStateSnapshot> {
    let Some(arc) = node.state.load_rack_arc() else {
        return Vec::new();
    };
    // Prefer a real lock off-RT so we don't silently skip harvest under load.
    let Ok(mut rack) = arc.lock() else {
        return Vec::new();
    };
    rack.slots
        .iter_mut()
        .map(|slot| {
            let controls = slot.processor.named_control_values();
            let state_blob = slot.processor.capture_state_blob();
            SlotStateSnapshot {
                slot_id: slot.id.0,
                state_blob,
                controls,
            }
        })
        .collect()
}

/// Merge harvested live state into insert slots (used before `Rack::build`).
pub fn apply_harvest_to_inserts(inserts: &mut [crate::domain::InsertSlot], snaps: &[SlotStateSnapshot]) {
    for ins in inserts.iter_mut() {
        let Some(snap) = snaps.iter().find(|s| s.slot_id == ins.slot_id) else {
            continue;
        };
        if let Some(ref blob) = snap.state_blob {
            if !blob.is_empty() {
                ins.state_blob = Some(blob.clone());
            }
        }
        for (name, val) in &snap.controls {
            let lname = name.to_ascii_lowercase();
            if lname == "bypass" || lname == "enable" {
                continue;
            }
            if let Some((_, v)) = ins
                .controls
                .iter_mut()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
            {
                *v = *val;
            } else {
                ins.controls.push((name.clone(), *val));
            }
        }
    }
}

pub fn push_host_controls(bus: &str, inserts: &[InsertSlot]) -> Result<()> {
    let reg = REGISTRY.lock().unwrap();
    let track = reg
        .get(bus)
        .ok_or_else(|| anyhow!("no host for bus `{bus}`"))?;
    if track.fingerprint != crate::domain::inserts_signature(inserts) {
        return Err(anyhow!(
            "host topology mismatch — ensure ForceRespawn before controls"
        ));
    }
    push_insert_controls_to_queue(&track.node, inserts);
    Ok(())
}

fn push_insert_controls_to_queue(node: &PwFxNode, inserts: &[InsertSlot]) {
    let index_map: Vec<(uuid::Uuid, bool, Vec<(u32, f32)>)> = {
        let Some(arc) = node.state.load_rack_arc() else {
            return;
        };
        let Ok(rack) = arc.try_lock() else {
            for ins in inserts {
                node.state
                    .queue
                    .push(ControlMsg::bypass(ins.slot_id, insert_bypassed(ins)));
            }
            return;
        };
        inserts
            .iter()
            .map(|ins| {
                let bypassed = insert_bypassed(ins);
                let mut params = Vec::new();
                if let Some(slot_idx) = rack.slot_index(ins.slot_id) {
                    let slot = &rack.slots[slot_idx];
                    for (name, val) in &ins.controls {
                        let lname = name.to_ascii_lowercase();
                        if lname == "bypass" || lname == "enable" {
                            continue;
                        }
                        if let Some(ci) = (0..slot.processor.control_count()).find(|&i| {
                            slot.processor
                                .control_name(i)
                                .is_some_and(|n| n.eq_ignore_ascii_case(name))
                        }) {
                            params.push((ci as u32, *val));
                        }
                    }
                }
                (ins.slot_id, bypassed, params)
            })
            .collect()
    };

    for (slot_id, bypassed, params) in index_map {
        node.state.queue.push(ControlMsg::bypass(slot_id, bypassed));
        for (control_index, value) in params {
            node.state
                .queue
                .push(ControlMsg::param(slot_id, control_index, value));
        }
    }
}

fn insert_bypassed(ins: &InsertSlot) -> bool {
    for (name, val) in &ins.controls {
        let n = name.to_ascii_lowercase();
        if n == "bypass" {
            return *val >= 0.5;
        }
        if n == "enable" {
            return *val < 0.5;
        }
    }
    false
}

pub fn teardown_host(bus: &str) {
    let mut reg = REGISTRY.lock().unwrap();
    if let Some(track) = reg.remove(bus) {
        track.node.stop();
    }
    drop(reg);
    super::pdc::remove_bus(bus);
}

/// Non-RT: set Master-bus PDC delay on the live host.
pub fn set_host_pdc_delay(bus: &str, samples: u32) {
    let reg = REGISTRY.lock().unwrap();
    if let Some(track) = reg.get(bus) {
        track.node.state.set_pdc_delay(samples);
    }
}

/// Host meter peaks (pre, post) — lock-free atomics.
pub fn host_meter_peaks(bus: &str) -> Option<(f32, f32)> {
    let reg = REGISTRY.lock().ok()?;
    reg.get(bus).map(|t| t.node.state.meter_peaks())
}

pub fn host_running(bus: &str) -> bool {
    let reg = REGISTRY.lock().unwrap();
    reg.get(bus).is_some_and(|t| t.node.is_running())
}

pub fn host_fingerprint(bus: &str) -> Option<String> {
    let reg = REGISTRY.lock().unwrap();
    reg.get(bus).map(|t| t.fingerprint.clone())
}

pub fn host_xruns(bus: &str) -> u64 {
    let reg = REGISTRY.lock().unwrap();
    reg.get(bus)
        .map(|t| t.node.state.xruns.load(std::sync::atomic::Ordering::Relaxed))
        .unwrap_or(0)
}

pub fn host_latency(bus: &str) -> u32 {
    let reg = REGISTRY.lock().unwrap();
    reg.get(bus)
        .map(|t| {
            t.node
                .state
                .latency_samples
                .load(std::sync::atomic::Ordering::Acquire)
        })
        .unwrap_or(0)
}

pub fn control_index_for(bus: &str, slot_id: Uuid, port_name: &str) -> Option<u32> {
    let reg = REGISTRY.lock().unwrap();
    let track = reg.get(bus)?;
    let arc = track.node.state.load_rack_arc()?;
    let rack = arc.lock().ok()?;
    let idx = rack.slot_index(slot_id)?;
    let slot = &rack.slots[idx];
    (0..slot.processor.control_count()).find_map(|i| {
        let n = slot.processor.control_name(i)?;
        if n.eq_ignore_ascii_case(port_name)
            || normalize_ladspa_label(n) == normalize_ladspa_label(port_name)
        {
            Some(i as u32)
        } else {
            None
        }
    })
}

pub fn hosted_count() -> usize {
    REGISTRY.lock().unwrap().len()
}
