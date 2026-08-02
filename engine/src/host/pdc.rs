//! Rack latency bookkeeping + RT-safe delay line.
//!
//! When Master fan-in GLC is active (`!glc::is_disabled()`), reported rack latency
//! feeds `L_local` and host peer pads stay 0. When Master Direct disables GLC,
//! host-side peer PDC pads each FX wet out to `max` reported latency among peers
//! (legacy behavior). DelayLine is also reused by GLC filter nodes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

fn reported() -> &'static Mutex<HashMap<String, u32>> {
    static R: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn targets() -> &'static Mutex<HashMap<String, AtomicU32>> {
    static T: OnceLock<Mutex<HashMap<String, AtomicU32>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Recompute host peer pads from the reported map (call after Master Direct toggles).
pub fn refresh_peer_pads() {
    let glc_off = crate::pipeline::glc::is_disabled();
    let snapshot: Vec<(String, u32)> = {
        let Ok(g) = reported().lock() else {
            return;
        };
        let max = if glc_off {
            g.values().copied().max().unwrap_or(0)
        } else {
            0
        };
        g.iter()
            .map(|(b, l)| {
                let d = if glc_off {
                    max.saturating_sub(*l)
                } else {
                    0
                };
                (b.clone(), d)
            })
            .collect()
    };
    if let Ok(mut t) = targets().lock() {
        for (b, d) in snapshot {
            t.entry(b)
                .or_insert_with(|| AtomicU32::new(0))
                .store(d, Ordering::Release);
        }
    }
}

/// Non-RT: publish reported rack latency for `bus`.
/// Peer pads follow Master Direct (GLC off) vs GLC-on policy.
pub fn set_bus_latency(bus: &str, latency: u32) {
    if let Ok(mut g) = reported().lock() {
        g.insert(bus.to_string(), latency);
    }
    refresh_peer_pads();
}

/// Host-side peer pad samples (non-zero only when GLC is disabled).
pub fn compensation_samples(bus: &str) -> u32 {
    targets()
        .lock()
        .ok()
        .and_then(|g| g.get(bus).map(|a| a.load(Ordering::Acquire)))
        .unwrap_or(0)
}

pub fn reported_latency(bus: &str) -> u32 {
    reported()
        .lock()
        .ok()
        .and_then(|g| g.get(bus).copied())
        .unwrap_or(0)
}

pub fn remove_bus(bus: &str) {
    if let Ok(mut g) = reported().lock() {
        g.remove(bus);
    }
    if let Ok(mut g) = targets().lock() {
        g.remove(bus);
    }
    refresh_peer_pads();
}

/// Snapshot of (bus, compensation_samples) for pushing into live hosts.
pub fn all_compensation_targets() -> Vec<(String, u32)> {
    targets()
        .lock()
        .ok()
        .map(|g| {
            g.iter()
                .map(|(b, a)| (b.clone(), a.load(Ordering::Acquire)))
                .collect()
        })
        .unwrap_or_default()
}

/// Lock-free circular delay for RT use (owned by HostRtState).
pub struct DelayLine {
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    write: usize,
    delay: AtomicU32,
}

impl DelayLine {
    pub fn new() -> Self {
        Self {
            buf_l: Vec::new(),
            buf_r: Vec::new(),
            write: 0,
            delay: AtomicU32::new(0),
        }
    }

    /// Non-RT: resize ring for `samples` delay (0 = bypass).
    pub fn set_delay(&mut self, samples: u32) {
        self.delay.store(samples, Ordering::Release);
        let n = samples as usize;
        if n == 0 {
            self.buf_l.clear();
            self.buf_r.clear();
            self.write = 0;
            return;
        }
        if self.buf_l.len() != n {
            self.buf_l = vec![0.0; n];
            self.buf_r = vec![0.0; n];
            self.write = 0;
        }
    }

    /// RT-safe process (no alloc when delay unchanged).
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let d = self.delay.load(Ordering::Acquire) as usize;
        if d == 0 || self.buf_l.len() != d {
            return;
        }
        let n = d;
        for i in 0..left.len() {
            let ri = self.write;
            let ol = self.buf_l[ri];
            let or = self.buf_r[ri];
            self.buf_l[ri] = left[i];
            self.buf_r[ri] = right[i];
            left[i] = ol;
            right[i] = or;
            self.write = (self.write + 1) % n;
        }
    }
}

impl Default for DelayLine {
    fn default() -> Self {
        Self::new()
    }
}
