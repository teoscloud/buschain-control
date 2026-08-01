//! # Live Graph Contract
//!
//! **Every audio mutation goes through the live PipeWire graph.**
//! The only intentional outsides are Tear down (recovery) and hard errors.
//!
//! UI and session code must call [`crate::app_state::AppState::commit`] with a
//! [`LiveChange`] — never `pactl` / `pw-cli` from the UI thread, and never ask
//! the user to “Apply graph” for normal edits.
//!
//! ## Severity ladder (least → most invasive)
//!
//! 1. **Level** — one track fader / one track mute (Class A, &lt;20ms)
//! 2. **Levels** — solo recount only (multi-bus mute math)
//! 3. **FxParams** — Props on the in-process host ControlQueue
//! 4. **Capture** — In rack delta (add / mute-row / remove); never Route (Class B, &lt;200ms)
//! 5. **PlaceApp** — session-pinned app → bus; never ApplyLevels
//! 6. **FxRewire** — add/remove/reorder via surgical gen-swap ForceRespawn per track
//! 7. **EnsureTrack / PruneTrack / VirtualInput** — one bus topology
//! 8. **Route** — egress + listen + Master HW **only** (no capture purge)
//! 9. **Reconcile** — idle ensure; recovery / cold bring-up
//! 10. **Teardown** — ERROR RECOVERY ONLY
//!
//! ## Latency classes
//!
//! - **A (Instant):** mute/fader/Props — no `pactl`/`pw-link`; p99 &lt;20ms
//! - **B (Surgical):** Capture / PlaceApp — one hop or one list+move; p99 &lt;200ms
//! - **C (Structural):** Route / Ensure / Reconcile / snapshot — never ahead of pending A
//!
//! ## Hard rules
//!
//! - One sealed `buschain_fx_*` host per bus; structural edits gen-swap the rack.
//! - Power/knobs = Props only. Capture = `Intent::SyncCaptureDelta` (verify-live).
//! - Route = egress `relink_routes` only (never ArmSession / ForceRespawn from In edits).
//! - Apps rack assign = PlaceApp / SyncPlayback only (never Route, never ApplyLevels HOL).
//! - PruneTrack silence-first: gate + disarm egress + unlink capture before FX teardown.
//! - Never move sink-inputs as a side effect of mixer fader/FX/Capture edits.
//! - If the graph is cold, **only Reconcile** may Full Apply / ArmSession.
//! - Capture hop is applied only after **native verified-live** (not Pulse module presence).
//! - Apps stay on stable `buschain_*` null-sink buses; FX is the in-process host filter.

use uuid::Uuid;

/// A session mutation that must hit the live graph (or bring it up).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveChange {
    /// Single-track fader or single-track mute.
    Level { track_id: Uuid },
    /// Solo recount across tracks (multi-bus mute math only).
    Levels,
    /// Knobs / mix / insert power — Props on slots, no process restart.
    FxParams { track_id: Uuid },
    /// In-rack add / mute-row / remove — capture hops only (never Route).
    Capture { track_id: Uuid },
    /// Insert add / remove / reorder — surgical gen-swap on that track's host.
    FxRewire { track_id: Uuid },
    /// New track bus.
    EnsureTrack { track_id: Uuid },
    /// Create or tear down system virtual input for one track (session flag is source of truth).
    VirtualInput { track_id: Uuid },
    /// Session-pinned Apps rack place for one track (PlaceApp hot path; never ApplyLevels).
    PlaceApp { track_id: Uuid },
    /// Master HW out, listen, output targets (egress links only — not In / Apps).
    Route,
    /// Idle reconcile / cold bring-up / manual recovery.
    Reconcile,
}

impl LiveChange {
    /// Human-readable status fragment.
    pub fn label(self) -> &'static str {
        match self {
            Self::Level { .. } | Self::Levels => "levels",
            Self::FxParams { .. } => "live params",
            Self::Capture { .. } => "capture",
            Self::FxRewire { .. } => "FX rewire",
            Self::EnsureTrack { .. } => "ensure track",
            Self::VirtualInput { .. } => "virtual input",
            Self::PlaceApp { .. } => "place app",
            Self::Route => "routing",
            Self::Reconcile => "reconcile",
        }
    }

    /// True if this change needs a live graph (buses/FX) to take effect.
    /// Levels + Props + Capture + PlaceApp never trigger ApplySession bring-up.
    pub fn needs_graph(self) -> bool {
        !matches!(
            self,
            Self::Level { .. }
                | Self::Levels
                | Self::FxParams { .. }
                | Self::Capture { .. }
                | Self::PlaceApp { .. }
        )
    }

    /// Latency class letter for lat_trace (`A` / `B` / `C`).
    pub fn lat_class(self) -> char {
        match self {
            Self::Level { .. } | Self::Levels | Self::FxParams { .. } => 'A',
            Self::Capture { .. } | Self::PlaceApp { .. } => 'B',
            _ => 'C',
        }
    }
}
