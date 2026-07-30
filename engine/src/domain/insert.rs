//! Sealed DAW insert-chain types — plugins are black boxes; mixer only declares the rack.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::NodeName;

/// Plugin format for rack dispatch (LADSPA / LV2 / CLAP / VST3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InsertFormat {
    #[default]
    Ladspa,
    Lv2,
    Clap,
    Vst3,
}

/// One insert in a track rack (black box). Engine wires audio through it; never opens UIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsertSlot {
    pub slot_id: Uuid,
    /// Plugin id / LADSPA label / CLAP id.
    pub plugin_key: String,
    /// Absolute `.so` / `.clap` / module path when known (preferred); else stem.
    pub plugin_so: String,
    /// Effective control map (bypass/mix already folded in by app).
    pub controls: Vec<(String, f32)>,
    /// Format for host dispatch. Default LADSPA for session backward compat.
    #[serde(default)]
    pub format: InsertFormat,
    /// Opaque plugin state chunk (CLAP/VST3) when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_blob: Option<Vec<u8>>,
    /// Sidechain source bus name (monitor tap), when the plugin uses aux input (P7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidechain_from: Option<String>,
}

/// Desired insert chain for one bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainSpec {
    pub bus: NodeName,
    /// Rack order = process order (n0 → n1 → …).
    pub inserts: Vec<InsertSlot>,
    /// Exclusive wet destination (`buschain_master` or HW sink name).
    pub dest: String,
}

impl ChainSpec {
    /// Resolved physical nodes for the sealed wet path (canonical names only).
    pub fn wire_plan(&self) -> WirePlan {
        WirePlan {
            bus: self.bus.clone(),
            fx_sink: NodeName::new(fx_name_for_bus(self.bus.as_str())),
            post_sink: NodeName::new(post_name_for_bus(self.bus.as_str())),
            dest: self.dest.clone(),
        }
    }

    pub fn signature(&self) -> String {
        inserts_signature(&self.inserts)
    }
}

/// Topology fingerprint for Props gating (slot order + plugin keys + IO layout; not bypass/knobs).
pub fn inserts_signature(inserts: &[InsertSlot]) -> String {
    let body = inserts
        .iter()
        .map(|p| {
            let sc = p
                .sidechain_from
                .as_deref()
                .unwrap_or("-");
            format!(
                "{}:{}:sc={}",
                p.slot_id.simple(),
                normalize_ladspa_label(&p.plugin_key),
                sc
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    format!("stereo2|{body}")
}

/// Resolved physical nodes for the sealed wet path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WirePlan {
    pub bus: NodeName,
    pub fx_sink: NodeName,
    pub post_sink: NodeName,
    pub dest: String,
}

impl WirePlan {
    pub fn bus_monitor(&self) -> String {
        self.bus.monitor_source()
    }

    pub fn post_monitor(&self) -> String {
        self.post_sink.monitor_source()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChainEnsureMode {
    /// Idle/supervisor: adopt if signature matches; never stop/spawn.
    /// Audible-path flaps must not ForceRespawn and steal the worker for minutes.
    ProbeOnly,
    /// Explicit repair path: respawn when signature / audible path mismatches.
    Idempotent,
    /// Always respawn the filter-chain (add/remove/reorder).
    ForceRespawn,
}

/// Live state of a bus insert chain — single source of truth for meters + routing.
#[derive(Debug, Clone)]
pub enum ChainState {
    /// No inserts — dry bus→dest.
    Dry,
    /// Spawn / wet switch in progress.
    Building,
    /// Audible through plugins.
    Wet(WirePlan),
    /// Rack non-empty but wet path failed (never pretend OK).
    Failed(String),
}

impl ChainState {
    pub fn is_wet(&self) -> bool {
        matches!(self, Self::Wet(_))
    }

    pub fn wire(&self) -> Option<&WirePlan> {
        match self {
            Self::Wet(w) => Some(w),
            _ => None,
        }
    }
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

pub fn fx_name_for_bus(bus: &str) -> String {
    format!("buschain_fx_{}", bus_suffix(bus))
}

pub fn post_name_for_bus(bus: &str) -> String {
    format!("buschain_post_{}", bus_suffix(bus))
}

/// Normalize LADSPA labels the same way the app catalog does.
/// Rewrites pre-rebrand `shadow_*` labels to `buschain_*`.
pub fn normalize_ladspa_label(id: &str) -> String {
    let id = id.strip_prefix("ladspa:").unwrap_or(id).trim();
    if let Some(rest) = id.strip_prefix("shadow_") {
        format!("buschain_{rest}")
    } else {
        id.to_string()
    }
}
