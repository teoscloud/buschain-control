//! Typed graph identities — no stringly routing in contracts.

mod insert;

pub use insert::{
    bus_suffix, fx_name_for_bus, inserts_signature, normalize_ladspa_label, post_name_for_bus,
    ChainEnsureMode, ChainSpec, ChainState, InsertSlot, WirePlan,
};

use serde::{Deserialize, Serialize};

/// Stable logical bus / helper name (Pulse sink name).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeName(pub String);

impl NodeName {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_shadow(&self) -> bool {
        self.0.starts_with("buschain_")
    }

    pub fn is_buschain_helper(&self) -> bool {
        self.0.starts_with("buschain_fx_")
            || self.0.starts_with("buschain_post_")
            || self.0.starts_with("buschain_mid_")
            || self.0.starts_with("buschain_rs_")
            || self.0 == "buschain_hold"
    }

    pub fn is_rate_bridge(&self) -> bool {
        self.0.starts_with("buschain_rs_")
    }

    /// Pulse monitor source name for a sink.
    pub fn monitor_source(&self) -> String {
        format!("{}.monitor", self.0)
    }
}

impl std::fmt::Display for NodeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for NodeName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Role of a BusChain-owned node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeRole {
    MasterBus,
    TrackBus,
    PostBus,
    Hold,
    FxSink,
    RateBridge,
    External,
}

/// Desired null-sink / helper specification.
#[derive(Debug, Clone)]
pub struct NodeSpec {
    pub name: NodeName,
    pub description: String,
    pub role: NodeRole,
    /// When true, start muted at 0% (app-facing buses).
    pub start_muted: bool,
}

/// Stereo route hop (Pulse-style source → sink names).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LinkSpec {
    /// Pulse source (often `bus.monitor` or capture name).
    pub source: String,
    /// Pulse sink name.
    pub sink: String,
    pub exclusive: bool,
}

/// Opaque backend handle (CLI backend uses names; native may use object ids).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LinkId(pub String);

/// Key/value props pushed to a live node.
#[derive(Debug, Clone, Default)]
pub struct Props {
    pub entries: Vec<(String, String)>,
}

impl Props {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.entries.push((key.into(), value.into()));
        self
    }
}

/// Lightweight enumeration of sinks/sources for probing.
#[derive(Debug, Clone, Default)]
pub struct DeviceNode {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default)]
pub struct GraphSnapshot {
    pub sinks: Vec<DeviceNode>,
    pub sources: Vec<DeviceNode>,
}
