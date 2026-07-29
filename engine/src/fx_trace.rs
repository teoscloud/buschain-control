//! Env-gated FX control-plane timing (`BUSCHAIN_FX_TRACE=1`).
//!
//! Measures time-to-effect for host actuation — not DSP/buffer latency.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);
static INIT: AtomicBool = AtomicBool::new(false);

fn ensure_init() {
    if INIT.swap(true, Ordering::Relaxed) {
        return;
    }
    let on = std::env::var_os("BUSCHAIN_FX_TRACE")
        .map(|v| v != "0" && v != "false" && !v.is_empty())
        .unwrap_or(false);
    ENABLED.store(on, Ordering::Relaxed);
    if on {
        eprintln!("[buschain-fx-trace] enabled (time-to-effect / control plane)");
    }
}

pub fn enabled() -> bool {
    ensure_init();
    ENABLED.load(Ordering::Relaxed)
}

pub fn log(phase: &str, detail: &str, elapsed_ms: u128) {
    if !enabled() {
        return;
    }
    eprintln!("[buschain-fx-trace] {phase} {elapsed_ms}ms {detail}");
}

pub fn span(phase: &str) -> FxSpan {
    FxSpan {
        phase: phase.to_string(),
        start: Instant::now(),
        finished: false,
    }
}

pub struct FxSpan {
    phase: String,
    start: Instant,
    finished: bool,
}

impl FxSpan {
    pub fn end(mut self, detail: impl AsRef<str>) {
        self.finished = true;
        log(&self.phase, detail.as_ref(), self.start.elapsed().as_millis());
    }

    pub fn end_ok(self) {
        self.end("ok");
    }
}

impl Drop for FxSpan {
    fn drop(&mut self) {
        if !self.finished {
            log(
                &self.phase,
                "dropped",
                self.start.elapsed().as_millis(),
            );
        }
    }
}
