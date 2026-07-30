//! Per-FX rebuild gate — suppress stale CLI probes during host ForceRespawn.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

struct BusyState {
    names: HashSet<String>,
    /// Keep the gate up briefly after ForceRespawn returns so the first
    /// post-spawn reconcile ticks don't race an empty link cache.
    settle_until: HashMap<String, Instant>,
}

fn busy() -> &'static Mutex<BusyState> {
    static B: OnceLock<Mutex<BusyState>> = OnceLock::new();
    B.get_or_init(|| {
        Mutex::new(BusyState {
            names: HashSet::new(),
            settle_until: HashMap::new(),
        })
    })
}

const SETTLE: Duration = Duration::from_millis(120);

pub fn mark_rebuilding(fx_name: &str, on: bool) {
    let Ok(mut g) = busy().lock() else {
        return;
    };
    if on {
        g.names.insert(fx_name.to_string());
        g.settle_until.remove(fx_name);
    } else {
        g.names.remove(fx_name);
        g.settle_until
            .insert(fx_name.to_string(), Instant::now() + SETTLE);
    }
}

pub fn is_rebuilding(fx_name: &str) -> bool {
    let Ok(mut g) = busy().lock() else {
        return false;
    };
    if g.names.contains(fx_name) {
        return true;
    }
    if let Some(until) = g.settle_until.get(fx_name).copied() {
        if Instant::now() < until {
            return true;
        }
        g.settle_until.remove(fx_name);
    }
    false
}

/// RAII: mark FX sink rebuilding for the ForceRespawn critical section.
pub struct RebuildGuard {
    fx_name: String,
}

impl RebuildGuard {
    pub fn enter(fx_name: &str) -> Self {
        mark_rebuilding(fx_name, true);
        Self {
            fx_name: fx_name.to_string(),
        }
    }
}

impl Drop for RebuildGuard {
    fn drop(&mut self) {
        mark_rebuilding(&self.fx_name, false);
    }
}
