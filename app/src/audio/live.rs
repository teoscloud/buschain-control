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
//! 1. **Levels** — volume/mute on existing buses (no module load)
//! 2. **FxParams** — Props on the monolithic rack (knobs / mix / insert power)
//! 3. **FxRewire** — add/remove/reorder (warm A/B dual-helper cutover when wet)
//! 4. **EnsureTrack / PruneTrack** — one bus only
//! 5. **Route** — link-only rewire (listen / HW / targets) — never respawn FX
//! 6. **Reconcile** — idle ensure; recovery / cold bring-up
//! 7. **Teardown** — ERROR RECOVERY ONLY (not a normal edit path)
//!
//! ## Hard rules
//!
//! - One sealed `buschain_fx_*` helper per bus (`n0→n1→…`); structural edits A/B cutover.
//! - Power/knobs = Props only. Route = destination links only.
//! - Exclusive `ensure_loopback` routes (never stack loads → double audio).
//! - Never move sink-inputs as a side effect of mixer edits (Chromium/Spotify pause).
//! - If the graph is cold, **auto bring-up** via Reconcile — do not prompt for Apply.
//! - Apps stay on stable `buschain_*` null-sink buses; FX is monolithic filter-chain.

use uuid::Uuid;

/// A session mutation that must hit the live graph (or bring it up).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveChange {
    /// Single-track fader.
    Level { track_id: Uuid },
    /// Mute/solo recount across tracks.
    Levels,
    /// Knobs / mix / insert power — Props on slots, no process restart.
    FxParams { track_id: Uuid },
    /// Insert add / remove / reorder — A/B FX cutover + exclusive rewire.
    FxRewire { track_id: Uuid },
    /// New track bus.
    EnsureTrack { track_id: Uuid },
    /// Master HW out, listen, app assign targets, etc. (links only).
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
            Self::FxRewire { .. } => "FX rewire",
            Self::EnsureTrack { .. } => "ensure track",
            Self::Route => "routing",
            Self::Reconcile => "reconcile",
        }
    }

    /// True if this change needs a live graph (buses/FX) to take effect.
    /// Levels + Props never trigger ApplySession bring-up — they are hot-path only.
    pub fn needs_graph(self) -> bool {
        !matches!(
            self,
            Self::Level { .. } | Self::Levels | Self::FxParams { .. }
        )
    }
}
