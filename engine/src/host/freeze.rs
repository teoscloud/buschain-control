//! Offline freeze / bounce through a host rack at GraphClock.

use anyhow::Result;

use crate::domain::InsertSlot;

use super::rack::Rack;

/// Render `frames` of silence→rack→buffers at `sample_rate` (offline, non-RT).
pub fn bounce_rack(
    inserts: &[InsertSlot],
    sample_rate: u32,
    frames: usize,
    ladspa_path: &str,
) -> Result<(Vec<f32>, Vec<f32>)> {
    let max_block = 256u32;
    let mut rack = Rack::build(inserts, sample_rate, max_block, ladspa_path)?;
    let mut left = vec![0.0f32; frames];
    let mut right = vec![0.0f32; frames];
    let mut i = 0;
    while i < frames {
        let n = (frames - i).min(max_block as usize);
        rack.render_offline(&mut left[i..i + n], &mut right[i..i + n]);
        i += n;
    }
    Ok((left, right))
}

/// Freeze payload — replace live inserts with a buffer player (session layer).
#[derive(Debug, Clone)]
pub struct FreezeBuffer {
    pub sample_rate: u32,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

impl FreezeBuffer {
    pub fn from_bounce(
        inserts: &[InsertSlot],
        sample_rate: u32,
        frames: usize,
        ladspa_path: &str,
    ) -> Result<Self> {
        let (left, right) = bounce_rack(inserts, sample_rate, frames, ladspa_path)?;
        Ok(Self {
            sample_rate,
            left,
            right,
        })
    }
}
