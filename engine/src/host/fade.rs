//! Click-free micro-fades for bypass / structural transitions.

/// Linear equal-power-ish crossfade length in samples (short enough for live feel).
pub const FADE_LEN: usize = 96;

/// Tracks a 0→1 or 1→0 ramp for wet/dry mix on power toggle.
#[derive(Debug, Clone)]
pub struct BypassFade {
    /// Current wet amount (1 = fully processed, 0 = fully dry).
    wet: f32,
    target: f32,
    step: f32,
}

impl Default for BypassFade {
    fn default() -> Self {
        Self {
            wet: 1.0,
            target: 1.0,
            step: 1.0 / FADE_LEN as f32,
        }
    }
}

impl BypassFade {
    pub fn set_bypassed(&mut self, bypassed: bool) {
        self.target = if bypassed { 0.0 } else { 1.0 };
    }

    pub fn is_idle(&self) -> bool {
        (self.wet - self.target).abs() < 1.0e-6
    }

    /// Advance one sample; returns wet mix in `[0, 1]`.
    #[inline]
    pub fn next(&mut self) -> f32 {
        if self.wet < self.target {
            self.wet = (self.wet + self.step).min(self.target);
        } else if self.wet > self.target {
            self.wet = (self.wet - self.step).max(self.target);
        }
        self.wet
    }

    pub fn snap(&mut self, bypassed: bool) {
        self.target = if bypassed { 0.0 } else { 1.0 };
        self.wet = self.target;
    }
}

/// Mix wet/dry: `out = dry + wet_amt * (wet - dry)`.
#[inline]
pub fn mix_sample(dry: f32, wet: f32, wet_amt: f32) -> f32 {
    dry + wet_amt * (wet - dry)
}
