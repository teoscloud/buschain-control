//! UI visualization runtime — Live vs Idle.
//!
//! Idle (window withdrawn / headless): no paint, Pulse meters paused, spectrum
//! watches cleared. Audio graph + worker keep running via [`AppState::tick`].

use crate::app_state::AppState;

/// egui cadence while the window is painting meters / charts.
pub const LIVE_REPAINT_MS: u64 = 16;
/// egui wake interval while withdrawn (tray/IPC still polled).
pub const IDLE_REPAINT_MS: u64 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VizMode {
    Live,
    Idle,
}

/// Central switch for visualization side-effects.
#[derive(Debug)]
pub struct UiRuntime {
    mode: VizMode,
}

impl Default for UiRuntime {
    fn default() -> Self {
        Self::new_live()
    }
}

impl UiRuntime {
    pub fn new_live() -> Self {
        Self {
            mode: VizMode::Live,
        }
    }

    pub fn new_idle() -> Self {
        Self {
            mode: VizMode::Idle,
        }
    }

    pub fn mode(&self) -> VizMode {
        self.mode
    }

    pub fn is_live(&self) -> bool {
        self.mode == VizMode::Live
    }

    /// Pause meters + clear FFT watches. Idempotent.
    pub fn enter_idle(&mut self, state: &mut AppState) {
        if self.mode == VizMode::Idle {
            return;
        }
        self.mode = VizMode::Idle;
        state.sleep_visualization();
    }

    /// Resume meters + retarget taps. Idempotent.
    pub fn enter_live(&mut self, state: &mut AppState) {
        if self.mode == VizMode::Live {
            return;
        }
        self.mode = VizMode::Live;
        state.wake_visualization();
    }
}
