//! Desired graph state — intents update this; backend applies the diff.

use std::collections::{HashMap, HashSet};

use crate::clock::GraphClock;
use crate::domain::{ChainSpec, LinkSpec, NodeName, NodeSpec};

/// Mixer fader intent for one bus (app sink stays open; mute gates outbound).
#[derive(Debug, Clone, Copy)]
pub struct BusLevel {
    pub gain_db: f32,
    /// Mixer mute — never mute the app-facing sink (corks Chromium).
    pub mixer_mute: bool,
}

impl Default for BusLevel {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            mixer_mute: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DesiredState {
    pub clock: GraphClock,
    pub buses: HashMap<String, NodeSpec>,
    pub routes: HashSet<(String, String)>,
    /// Rate-bridge node name → (from_rate, to_rate).
    pub bridges: HashMap<String, (u32, u32)>,
    /// Bus name → sealed insert chain.
    pub fx_chains: HashMap<String, ChainSpec>,
    /// Bus → configured egress destinations (Master and/or other track buses).
    pub bus_egress: HashMap<String, Vec<String>>,
    /// Resolved Master HW sink (speakers / headphones).
    pub master_hw: Option<String>,
    /// Preferred Pulse/PipeWire default sink (apps open here).
    pub preferred_default: Option<String>,
    /// Per-bus mixer levels (keyed by bus name).
    pub bus_levels: HashMap<String, BusLevel>,
    /// Session barrier latch — Master→HW only after cold bring-up clears.
    /// Idle reconcile must not clear this (would click speakers).
    pub speakers_armed: bool,
    /// Buses whose FX ensure Failed (dry restored) — counts as resolved for barrier.
    pub fx_failed: HashSet<String>,
    /// Track buses that should expose `buschain_vin_*` (description for remap).
    pub virtual_inputs: HashMap<String, String>,
}

impl DesiredState {
    pub fn set_clock(&mut self, clock: GraphClock) {
        self.clock = clock;
    }

    pub fn ensure_bus(&mut self, spec: NodeSpec) {
        self.buses.insert(spec.name.0.clone(), spec);
    }

    pub fn ensure_route(&mut self, link: &LinkSpec) {
        self.routes
            .insert((link.source.clone(), link.sink.clone()));
    }

    pub fn clear_routes(&mut self) {
        self.routes.clear();
    }

    pub fn ensure_fx_chain(&mut self, spec: ChainSpec) {
        self.fx_chains.insert(spec.bus.0.clone(), spec);
    }

    pub fn remove_fx_chain(&mut self, bus: &str) {
        self.fx_chains.remove(bus);
    }

    pub fn set_master_hw(&mut self, hw: Option<String>) {
        self.master_hw = hw.filter(|s| !s.is_empty());
    }

    pub fn set_preferred_default(&mut self, name: Option<String>) {
        self.preferred_default = name.filter(|s| !s.is_empty());
    }

    pub fn set_bus_level(&mut self, bus: &str, level: BusLevel) {
        self.bus_levels.insert(bus.to_string(), level);
    }

    pub fn set_bus_egress(&mut self, bus: &str, dests: Vec<String>) {
        self.bus_egress.insert(bus.to_string(), dests);
    }

    pub fn set_virtual_input(&mut self, bus: &str, description: Option<String>) {
        match description {
            Some(d) => {
                self.virtual_inputs.insert(bus.to_string(), d);
            }
            None => {
                self.virtual_inputs.remove(bus);
            }
        }
    }

    pub fn egress_dests(&self, bus: &str) -> Vec<String> {
        if let Some(d) = self.bus_egress.get(bus) {
            if !d.is_empty() {
                return d.clone();
            }
        }
        if let Some(spec) = self.fx_chains.get(bus) {
            if !spec.dest.is_empty() {
                return vec![spec.dest.clone()];
            }
        }
        if bus == "buschain_master" {
            if let Some(hw) = &self.master_hw {
                return vec![hw.clone()];
            }
        } else {
            return vec!["buschain_master".into()];
        }
        Vec::new()
    }

    pub fn bridge_name(src_rate: u32, dst_rate: u32, source: &str, sink: &str) -> NodeName {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        source.hash(&mut h);
        sink.hash(&mut h);
        src_rate.hash(&mut h);
        dst_rate.hash(&mut h);
        NodeName::new(format!("buschain_rs_{:x}", h.finish()))
    }
}

/// Operations the backend should execute for a reconcile.
#[derive(Debug, Clone, Default)]
pub struct Diff {
    pub ensure_nodes: Vec<NodeSpec>,
    pub ensure_links: Vec<LinkSpec>,
    pub destroy_bridges: Vec<String>,
}
