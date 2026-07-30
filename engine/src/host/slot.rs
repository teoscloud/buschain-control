//! One insert slot in a rack generation.

use uuid::Uuid;

use super::fade::BypassFade;
use super::processor::AudioProcessor;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SlotId(pub Uuid);

impl SlotId {
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }
}

pub struct Slot {
    pub id: SlotId,
    pub plugin_key: String,
    pub processor: Box<dyn AudioProcessor>,
    pub bypassed: bool,
    pub fade: BypassFade,
    /// Sidechain source bus (P7) — aux buffers supplied by rack when present.
    pub sidechain_from: Option<String>,
    /// Scratch for dry copy during bypass fade (sized in prepare).
    dry_l: Vec<f32>,
    dry_r: Vec<f32>,
}

impl Slot {
    pub fn new(id: Uuid, plugin_key: String, processor: Box<dyn AudioProcessor>) -> Self {
        Self {
            id: SlotId(id),
            plugin_key,
            processor,
            bypassed: false,
            fade: BypassFade::default(),
            sidechain_from: None,
            dry_l: Vec::new(),
            dry_r: Vec::new(),
        }
    }

    pub fn prepare(&mut self, sample_rate: u32, max_block: u32) {
        self.processor.prepare(sample_rate, max_block);
        let n = max_block as usize;
        self.dry_l.resize(n, 0.0);
        self.dry_r.resize(n, 0.0);
    }

    pub fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed;
        if bypassed {
            // Power-off: snap dry immediately. Crossfading from pitched delay
            // tails feels like a sluggish disable; dry is click-safe.
            self.fade.snap(true);
        } else {
            self.fade.set_bypassed(false);
        }
    }

    pub fn latency_samples(&self) -> u32 {
        if self.bypassed && self.fade.is_idle() {
            0
        } else {
            self.processor.latency_samples()
        }
    }

    /// Process one block with click-free bypass crossfade.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len();
        debug_assert_eq!(n, right.len());
        if n == 0 {
            return;
        }

        // Fully bypassed and fade complete → skip DSP.
        if self.bypassed && self.fade.is_idle() {
            return;
        }

        let need_fade = !self.fade.is_idle() || self.bypassed;
        if need_fade {
            let use_n = n.min(self.dry_l.len()).min(self.dry_r.len());
            if use_n == 0 {
                self.processor.process(left, right);
                return;
            }
            self.dry_l[..use_n].copy_from_slice(&left[..use_n]);
            self.dry_r[..use_n].copy_from_slice(&right[..use_n]);
            self.processor
                .process_with_sidechain(left, right, None, None);
            for i in 0..use_n {
                let w = self.fade.next();
                left[i] = super::fade::mix_sample(self.dry_l[i], left[i], w);
                right[i] = super::fade::mix_sample(self.dry_r[i], right[i], w);
            }
        } else {
            self.processor
                .process_with_sidechain(left, right, None, None);
        }
    }
}
