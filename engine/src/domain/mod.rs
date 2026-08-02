//! Typed graph identities — no stringly routing in contracts.

mod insert;

pub use insert::{
    bus_suffix, fx_name_for_bus, glc_name_for_bus, inserts_signature, mtr_name_for_bus,
    normalize_ladspa_label, post_name_for_bus, ChainEnsureMode, ChainSpec, ChainState,
    InsertFormat, InsertSlot, WirePlan,
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
            || self.0.starts_with("buschain_mtr_")
            || self.0.starts_with("buschain_glc_")
            || self.0.starts_with("buschain_rs_")
            || self.0.starts_with("buschain_vinf_")
            || self.0.starts_with("buschain_vin_")
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

/// Pulse-visible null-sink class (Master / virtual outputs).
pub const MEDIA_CLASS_PUBLIC_SINK: &str = "Audio/Sink";

/// Sealed helper class — standard PipeWire (same as ALSA `*/Internal` nodes).
/// Creates ports on `support.null-audio-sink` and stays out of `pactl`/pavucontrol.
/// Never use a custom class like `BusChain/Internal` (that yields portless nodes).
pub const MEDIA_CLASS_INTERNAL_SINK: &str = "Audio/Sink/Internal";

/// Role of a BusChain-owned node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeRole {
    MasterBus,
    TrackBus,
    PostBus,
    Hold,
    FxSink,
    RateBridge,
    /// Internal null-sink that receives post/bus egress for a virtual mic remap.
    VirtualInputFeed,
    External,
}

impl NodeRole {
    /// Default Pulse export when the caller does not pass an explicit flag.
    /// Master is public; tracks need explicit `virtual_output` (see session sync).
    /// Helpers stay sealed (`Audio/Sink/Internal`).
    pub fn default_pulse_export(self) -> bool {
        matches!(self, Self::MasterBus)
    }
}

/// Desired null-sink / helper specification.
#[derive(Debug, Clone)]
pub struct NodeSpec {
    pub name: NodeName,
    pub description: String,
    pub role: NodeRole,
    /// When true, start muted at 0% (app-facing buses).
    pub start_muted: bool,
    /// Expose in desktop Output lists (Master / VO). When false, create as
    /// [`MEDIA_CLASS_INTERNAL_SINK`] so Pulse/pavucontrol omit the node while
    /// ports remain linkable. Also stamped as `buschain.pulse.export`.
    pub pulse_export: bool,
}

impl NodeSpec {
    pub fn media_class(&self) -> &'static str {
        if self.pulse_export {
            MEDIA_CLASS_PUBLIC_SINK
        } else {
            MEDIA_CLASS_INTERNAL_SINK
        }
    }

    pub fn with_pulse_export(mut self, export: bool) -> Self {
        self.pulse_export = export;
        self
    }
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
