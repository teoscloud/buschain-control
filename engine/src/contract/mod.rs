//! Public intents — the only audio API the app may use.

use crate::clock::{GraphClock, PerformanceProfile};
use crate::domain::{ChainEnsureMode, ChainSpec, InsertSlot, LinkSpec, NodeName, NodeSpec, Props};

/// Typed mutation the engine applies to the live graph.
#[derive(Debug, Clone)]
pub enum Intent {
    /// Create/reattach a BusChain null-sink at the current graph clock.
    EnsureBus { spec: NodeSpec },
    /// Stereo hop; engine inserts a rate-bridge when endpoint rates differ.
    SetRoute { link: LinkSpec },
    /// Volume / mute on an existing bus.
    SetLevels {
        sink: String,
        gain_db: f32,
        muted: bool,
    },
    /// Push PipeWire Props (generic).
    PushProps { node: String, props: Props },
    /// Bind BusChain GraphClock from session performance (no PW force-rate).
    BindMasterClock { profile: PerformanceProfile },
    /// Sealed insert chain: spawn/reattach filter-chain + exclusive wet wire.
    EnsureFxChain {
        spec: ChainSpec,
        mode: ChainEnsureMode,
    },
    /// Props-only controls on a wet chain (power / knobs / mix). Never rewires.
    PushFxControls {
        bus: NodeName,
        inserts: Vec<InsertSlot>,
    },
    /// Tear down FX helpers for one bus (track prune / recovery).
    TeardownFxChain { bus: NodeName },
    /// Ensure system virtual input (feed sink + remap-source) for a track bus.
    EnsureVirtualInput {
        bus: NodeName,
        description: String,
    },
    /// Unload virtual input for a track bus.
    TeardownVirtualInput { bus: NodeName },
    /// Drop all BusChain-owned links + rate bridges (recovery).
    Teardown,
    /// Re-apply desired routes after recovery.
    Recover,
    /// Cold bring-up: disarm → build spines silent → dwell → arm track egress →
    /// session barrier → Master→HW. Sets `speakers_armed`.
    ArmSession {
        /// ForceRespawn all FX chains (restart / Apply Full).
        force_fx: bool,
    },
    /// Apply rate/quantum to one HW node via graph force-clock.
    /// When `bind_buschain` is true (Master HW out), force-rate HW only — GraphClock unchanged.
    BindDeviceClock {
        device: String,
        sample_rate: u32,
        quantum: u32,
        soft_quantum: bool,
        bind_buschain: bool,
    },
    /// Apply Desired `bus_inputs` onto the live graph (shared capture hops).
    /// Caller must sync Desired first — this never ForceRespawns FX.
    /// Cold / hotplug / Reconcile only — interactive In edits use [`SyncCaptureDelta`].
    SyncCapture,
    /// Surgical capture: remove and/or add specific sources on one track bus.
    /// Force-recreates adds; updates `last_applied` only after native verified-live.
    /// Never ForceRespawns FX; never relinks egress.
    SyncCaptureDelta {
        bus: String,
        remove: Vec<String>,
        add: Vec<String>,
    },
    /// Apply Desired `bus_playback` pins (move sink-inputs onto session buses).
    /// Caller must sync Desired first — never Full Apply / ForceRespawn.
    SyncPlayback,
    /// PipeWire daemon came back empty: tear FX hosts, keep Desired topology,
    /// cold `ArmSession { force_fx: true }`. Caller must [`reconnect_plane`] first.
    ReconnectPipeWire,
}

/// Result fragment for UI status lines.
#[derive(Debug, Clone, Default)]
pub struct ApplyReport {
    pub messages: Vec<String>,
}

impl ApplyReport {
    pub fn push(&mut self, m: impl Into<String>) {
        self.messages.push(m.into());
    }

    pub fn join(&self) -> String {
        self.messages.join(" · ")
    }
}

/// Clock + latency property bundle for node creation.
#[derive(Debug, Clone)]
pub struct ClockProps {
    pub sample_rate: u32,
    pub quantum: u32,
    pub soft_quantum: bool,
    pub suspend_timeout: u32,
    pub node_latency: String,
}

impl From<&GraphClock> for ClockProps {
    fn from(c: &GraphClock) -> Self {
        Self {
            sample_rate: c.sample_rate,
            quantum: c.quantum,
            soft_quantum: c.soft_quantum,
            suspend_timeout: if c.force_suspend_timeout_zero { 0 } else { 5 },
            node_latency: c.node_latency_prop(),
        }
    }
}

impl From<&PerformanceProfile> for ClockProps {
    fn from(p: &PerformanceProfile) -> Self {
        ClockProps::from(&p.graph_clock())
    }
}
