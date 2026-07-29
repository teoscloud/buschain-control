//! Thin naming helpers + runtime stub.
//!
//! FX spawn / Props / wet-switch live in `buschain-engine`. Do not add `pipewire -c`
//! or `pw-cli` here — extend the engine insert pipeline instead.

use uuid::Uuid;

pub use buschain_engine::{fx_name_for_bus, post_name_for_bus};

/// Legacy stub — FX children are owned by the process-wide engine.
#[derive(Default)]
pub struct FilterChainRuntime;

impl FilterChainRuntime {
    pub fn new() -> Self {
        Self
    }

    pub fn stop_all(&mut self) {
        crate::audio::engine_handle::stop_all_fx();
    }

    /// Legacy per-name stop — FX children live in buschain-engine.
    pub fn stop_one(&mut self, _name: &str) {}
}

pub fn bus_suffix(bus: &str) -> &str {
    if bus == "buschain_master" {
        "master"
    } else if let Some(rest) = bus.strip_prefix("buschain_track_") {
        rest
    } else {
        bus
    }
}

pub fn slot_fx_name(bus: &str, slot_id: Uuid) -> String {
    format!("buschain_fx_{}_{}", bus_suffix(bus), slot_id.simple())
}

pub fn slot_mid_name(bus: &str, slot_id: Uuid) -> String {
    format!("buschain_mid_{}_{}", bus_suffix(bus), slot_id.simple())
}

pub fn slot_fx_prefix(bus: &str) -> String {
    format!("buschain_fx_{}_", bus_suffix(bus))
}

pub fn slot_mid_prefix(bus: &str) -> String {
    format!("buschain_mid_{}_", bus_suffix(bus))
}

pub fn stop_slots_for_bus(_runtime: &mut FilterChainRuntime, bus: &str) {
    let _ = crate::audio::engine_handle::teardown_fx_chain(bus);
}
