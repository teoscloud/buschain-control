//! Native dry-bus peak taps — `{bus}.monitor` → `buschain_mtr_*` → hold.
//!
//! Used when no FX host is live so strip meters move without Pulse record streams.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use once_cell::sync::Lazy;

use crate::clock::GraphClock;
use crate::domain::mtr_name_for_bus;

use super::node::{HostRtState, PwFxNode};
use super::registry;

static DRY: Lazy<Mutex<HashMap<String, DryMeter>>> = Lazy::new(|| Mutex::new(HashMap::new()));
static DRY_STATE: Lazy<Mutex<HashMap<String, Arc<HostRtState>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

struct DryMeter {
    node: PwFxNode,
}

fn clock_sr(clock: &GraphClock) -> u32 {
    clock.sample_rate.max(8_000)
}

fn clock_q(clock: &GraphClock) -> u32 {
    clock.quantum.max(64)
}

fn publish_state(bus: &str, state: Arc<HostRtState>) {
    if let Ok(mut g) = DRY_STATE.lock() {
        g.insert(bus.to_string(), state);
    }
}

fn unpublish_state(bus: &str) {
    if let Ok(mut g) = DRY_STATE.lock() {
        g.remove(bus);
    }
}

fn dry_state(bus: &str) -> Option<Arc<HostRtState>> {
    DRY_STATE.lock().ok()?.get(bus).cloned()
}

/// Ensure a peak tap on `{bus}.monitor` for dry strips.
pub fn ensure_dry_meter(bus: &str, clock: &GraphClock) -> Result<()> {
    if bus.is_empty() || registry::host_is_live(bus) {
        teardown_dry_meter(bus);
        return Ok(());
    }
    let mtr = mtr_name_for_bus(bus);
    let from = format!("{bus}.monitor");
    {
        let reg = DRY.lock().unwrap();
        if let Some(m) = reg.get(bus) {
            if m.node.is_running() {
                publish_state(bus, Arc::clone(&m.node.state));
                drop(reg);
                // Re-assert the tap links — teardown_fx_chain / heal sweeps may
                // have stripped bus→mtr while the node itself stayed alive.
                let _ = crate::backend::ensure_link(&from, &mtr);
                let _ = crate::backend::ensure_link(&mtr, "buschain_hold");
                return Ok(());
            }
        }
    }

    let sr = clock_sr(clock);
    let quantum = clock_q(clock);
    let node = PwFxNode::start_named(bus, &mtr, sr, quantum)?;
    publish_state(bus, Arc::clone(&node.state));

    let _ = crate::backend::ensure_link(&from, &mtr);
    let _ = crate::backend::ensure_link(&mtr, "buschain_hold");
    // Keepalive parallel (arm paths also assert this).
    let _ = crate::backend::ensure_link(&from, "buschain_hold");

    let mut reg = DRY.lock().unwrap();
    if let Some(old) = reg.insert(bus.to_string(), DryMeter { node }) {
        drop(old);
    }
    // Host may have come up while we were spawning — never leave both taps.
    if registry::host_is_live(bus) {
        drop(reg);
        teardown_dry_meter(bus);
    }
    Ok(())
}

/// Tear down every dry meter (Quit / Intent::Teardown / UI hidden).
pub fn teardown_all_dry_meters() {
    let live: Vec<String> = DRY
        .lock()
        .ok()
        .map(|g| g.keys().cloned().collect())
        .unwrap_or_default();
    for bus in live {
        teardown_dry_meter(&bus);
    }
}

pub fn teardown_dry_meter(bus: &str) {
    let old = {
        let mut reg = DRY.lock().unwrap();
        reg.remove(bus)
    };
    if let Some(m) = old {
        let mtr = m.node.fx_name.clone();
        let from = format!("{bus}.monitor");
        let _ = crate::backend::unlink(&from, &mtr);
        let _ = crate::backend::unlink(&mtr, "buschain_hold");
        m.node.stop();
    }
    unpublish_state(bus);
}

/// Keep dry meters only for `want` buses that have no live FX host.
pub fn sync_dry_meters(want: &[String], clock: &GraphClock) {
    let want_set: HashSet<&str> = want.iter().map(|s| s.as_str()).collect();
    let live: Vec<String> = DRY
        .lock()
        .ok()
        .map(|g| g.keys().cloned().collect())
        .unwrap_or_default();
    for bus in live {
        if !want_set.contains(bus.as_str()) || registry::host_is_live(&bus) {
            teardown_dry_meter(&bus);
        }
    }
    for bus in want {
        if registry::host_is_live(bus) {
            continue;
        }
        let _ = ensure_dry_meter(bus, clock);
    }
}

pub fn dry_meter_peaks(bus: &str) -> Option<(f32, f32)> {
    dry_state(bus).map(|s| s.meter_peaks())
}

pub fn dry_meter_is_live(bus: &str) -> bool {
    dry_state(bus).is_some_and(|s| s.running.load(std::sync::atomic::Ordering::Relaxed))
}
