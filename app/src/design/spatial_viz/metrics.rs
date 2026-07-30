//! Stable metric bus — DSP / ports → UI ring buffers.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub const METRIC_RT60: &str = "rt60";
pub const METRIC_ECHO_DENSITY: &str = "echo_density";
pub const METRIC_ER_TAIL: &str = "er_tail_ratio";
pub const METRIC_BAND_T60_LO: &str = "band_t60_lo";
pub const METRIC_BAND_T60_MID: &str = "band_t60_mid";
pub const METRIC_BAND_T60_HI: &str = "band_t60_hi";
pub const METRIC_WET_PEAK: &str = "wet_peak";
pub const METRIC_DUCK_GR: &str = "duck_gr";

pub const REVERB_METRIC_IDS: &[&str] = &[
    METRIC_RT60,
    METRIC_ECHO_DENSITY,
    METRIC_ER_TAIL,
    METRIC_BAND_T60_LO,
    METRIC_BAND_T60_MID,
    METRIC_BAND_T60_HI,
    METRIC_WET_PEAK,
    METRIC_DUCK_GR,
];

#[derive(Clone, Default)]
pub struct SpatialMetricBus {
    slots: HashMap<&'static str, Arc<AtomicU32>>,
    history: HashMap<&'static str, Vec<f32>>,
    history_cap: usize,
}

impl SpatialMetricBus {
    pub fn new() -> Self {
        let mut bus = Self {
            slots: HashMap::new(),
            history: HashMap::new(),
            history_cap: 96,
        };
        for id in REVERB_METRIC_IDS {
            bus.register(id);
        }
        bus
    }

    pub fn register(&mut self, id: &'static str) {
        self.slots
            .entry(id)
            .or_insert_with(|| Arc::new(AtomicU32::new(0)));
        self.history.entry(id).or_default();
    }

    pub fn slot(&self, id: &'static str) -> Option<Arc<AtomicU32>> {
        self.slots.get(id).cloned()
    }

    pub fn set(&self, id: &'static str, value: f32) {
        if let Some(a) = self.slots.get(id) {
            a.store(value.to_bits(), Ordering::Relaxed);
        }
    }

    pub fn get(&self, id: &'static str) -> f32 {
        self.slots
            .get(id)
            .map(|a| f32::from_bits(a.load(Ordering::Relaxed)))
            .unwrap_or(0.0)
    }

    /// Push current slot values into ring buffers (call from UI thread).
    pub fn sample_history(&mut self) {
        for id in REVERB_METRIC_IDS {
            let v = self.get(id);
            if let Some(h) = self.history.get_mut(id) {
                h.push(v);
                if h.len() > self.history_cap {
                    let drop = h.len() - self.history_cap;
                    h.drain(0..drop);
                }
            }
        }
    }

    pub fn history(&self, id: &'static str) -> &[f32] {
        self.history.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }
}
