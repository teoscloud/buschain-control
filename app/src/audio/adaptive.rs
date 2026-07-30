//! Situational cadence policy — coalesce / repaint caps, not settle sleeps.
//!
//! Correctness waits use host readiness / typed events. This module only
//! batches human input and UI paint while Live.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// Hard-capped adaptive knobs for Live UI → worker traffic and paint.
#[derive(Debug)]
pub struct AdaptivePolicy {
    /// Drag coalesce for FxParams / levels (ms).
    drag_coalesce_ms: AtomicU64,
    /// Structural FX rewire batch window (ms).
    fx_coalesce_ms: AtomicU64,
    /// Live egui floor when meters/spectrum are quiet (ms).
    quiet_repaint_ms: AtomicU64,
    /// Live egui when viz is active (ms).
    live_repaint_ms: AtomicU64,
    /// Worker command-queue depth hint (updated by worker).
    queue_depth: AtomicU64,
}

impl Default for AdaptivePolicy {
    fn default() -> Self {
        Self {
            drag_coalesce_ms: AtomicU64::new(4),
            fx_coalesce_ms: AtomicU64::new(12),
            quiet_repaint_ms: AtomicU64::new(33),
            live_repaint_ms: AtomicU64::new(16),
            queue_depth: AtomicU64::new(0),
        }
    }
}

impl AdaptivePolicy {
    pub fn global() -> &'static AdaptivePolicy {
        static P: OnceLock<AdaptivePolicy> = OnceLock::new();
        P.get_or_init(AdaptivePolicy::default)
    }

    pub fn set_queue_depth(&self, n: usize) {
        self.queue_depth.store(n as u64, Ordering::Relaxed);
        // Under backlog, slightly widen drag coalesce (hard cap 16ms).
        let drag = if n > 8 { 8 } else if n > 3 { 6 } else { 4 };
        self.drag_coalesce_ms.store(drag, Ordering::Relaxed);
        let fx = if n > 8 { 20 } else if n > 3 { 16 } else { 12 };
        self.fx_coalesce_ms.store(fx.min(32), Ordering::Relaxed);
    }

    pub fn drag_coalesce_ms(&self) -> u64 {
        self.drag_coalesce_ms.load(Ordering::Relaxed).clamp(0, 16)
    }

    pub fn fx_coalesce_ms(&self) -> u64 {
        self.fx_coalesce_ms.load(Ordering::Relaxed).clamp(0, 32)
    }

    pub fn live_repaint_ms(&self) -> u64 {
        self.live_repaint_ms.load(Ordering::Relaxed).clamp(8, 50)
    }

    pub fn quiet_repaint_ms(&self) -> u64 {
        self.quiet_repaint_ms.load(Ordering::Relaxed).clamp(16, 100)
    }

    /// Pick Live repaint cadence from whether viz energy changed.
    pub fn repaint_ms_for_activity(&self, active: bool) -> u64 {
        if active {
            self.live_repaint_ms()
        } else {
            self.quiet_repaint_ms()
        }
    }
}
