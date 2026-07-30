//! Master-bus plugin delay compensation (non-RT bookkeeping + RT-safe delay line).
//!
//! Policy: delay each track host output to `max_latency` among Master peers.
//! The RT delay line lives in [`HostRtState`] — this module only computes targets.

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

/// Non-RT: publish reported rack latency for `bus` and recompute peer delays.
pub fn set_bus_latency(bus: &str, latency: u32) {
    if let Ok(mut g) = reported().lock() {
        g.insert(bus.to_string(), latency);
        let max = g.values().copied().max().unwrap_or(0);
        let snapshot: Vec<(String, u32)> = g
            .iter()
            .map(|(b, l)| (b.clone(), max.saturating_sub(*l)))
            .collect();
        drop(g);
        if let Ok(mut t) = targets().lock() {
            for (b, d) in snapshot {
                t.entry(b)
                    .or_insert_with(|| AtomicU32::new(0))
                    .store(d, Ordering::Release);
            }
        }
    }
}

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
    // Recompute remaining peers.
    if let Ok(g) = reported().lock() {
        let max = g.values().copied().max().unwrap_or(0);
        let snapshot: Vec<(String, u32)> = g
            .iter()
            .map(|(b, l)| (b.clone(), max.saturating_sub(*l)))
            .collect();
        drop(g);
        if let Ok(mut t) = targets().lock() {
            for (b, d) in snapshot {
                t.entry(b)
                    .or_insert_with(|| AtomicU32::new(0))
                    .store(d, Ordering::Release);
            }
        }
    }
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
