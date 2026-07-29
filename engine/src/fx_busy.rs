//! Per-FX rebuild gate — Props must not CLI-probe during ForceRespawn stop→spawn.
//!
//! Keyed by `buschain_fx_*` name (the Props hot path never goes through
//! `pipeline::insert::push_fx_controls`).

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

struct BusyState {
    names: HashSet<String>,
    /// Keep the gate up briefly after ForceRespawn returns so the first
    /// post-spawn Props ticks don't race an empty pactl cache.
    settle_until: Option<(String, Instant)>,
}

fn busy() -> &'static Mutex<BusyState> {
    static B: OnceLock<Mutex<BusyState>> = OnceLock::new();
    B.get_or_init(|| {
        Mutex::new(BusyState {
            names: HashSet::new(),
            settle_until: None,
        })
    })
}

const SETTLE: Duration = Duration::from_millis(250);

pub fn mark_rebuilding(fx_name: &str, on: bool) {
    let Ok(mut g) = busy().lock() else {
        return;
    };
    if on {
        g.names.insert(fx_name.to_string());
        g.settle_until = None;
    } else {
        g.names.remove(fx_name);
        g.settle_until = Some((fx_name.to_string(), Instant::now() + SETTLE));
    }
}

pub fn is_rebuilding(fx_name: &str) -> bool {
    let Ok(mut g) = busy().lock() else {
        return false;
    };
    if g.names.contains(fx_name) {
        return true;
    }
    if let Some((name, until)) = g.settle_until.as_ref() {
        if name == fx_name {
            if Instant::now() < *until {
                return true;
            }
            g.settle_until = None;
        }
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
