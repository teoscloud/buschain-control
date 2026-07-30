//! Black-box audio processor contract for the in-process insert host.

/// Normalized MIDI event for M2 plugin feeding (drained on the RT thread).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiEvent {
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    Cc {
        channel: u8,
        controller: u8,
        value: u8,
    },
}

impl MidiEvent {
    pub fn channel(&self) -> u8 {
        match self {
            Self::NoteOn { channel, .. }
            | Self::NoteOff { channel, .. }
            | Self::Cc { channel, .. } => *channel,
        }
    }
}

/// Stereo planar processor — RT-safe `process` (no alloc/lock/syscall).
pub trait AudioProcessor: Send {
    fn prepare(&mut self, sample_rate: u32, max_block: u32);
    fn latency_samples(&self) -> u32 {
        0
    }
    /// Process in-place stereo planar buffers of equal length.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]);
    /// Optional external sidechain pair (P7). Default ignores aux and calls [`process`].
    fn process_with_sidechain(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        side_l: Option<&[f32]>,
        side_r: Option<&[f32]>,
    ) {
        let _ = (side_l, side_r);
        self.process(left, right);
    }
    fn set_control(&mut self, index: usize, value: f32);
    fn control_count(&self) -> usize;
    fn control_name(&self, index: usize) -> Option<&str>;
    /// Optional MIDI event inbox (M2 — default no-op).
    fn feed_midi(&mut self, _events: &[MidiEvent]) {}
    /// Off-RT: opaque plugin state chunk for session / rack rebuild restore.
    fn capture_state_blob(&mut self) -> Option<Vec<u8>> {
        None
    }
    /// Off-RT: current named control values (normalized) for session mirror.
    fn named_control_values(&self) -> Vec<(String, f32)> {
        (0..self.control_count())
            .filter_map(|i| {
                let name = self.control_name(i)?.to_string();
                let value = self.control_value(i)?;
                Some((name, value))
            })
            .collect()
    }
    /// Current normalized value for a control index, if known.
    fn control_value(&self, _index: usize) -> Option<f32> {
        None
    }
}

/// Pass-through processor (tests / empty slot).
pub struct Passthrough;

impl AudioProcessor for Passthrough {
    fn prepare(&mut self, _sample_rate: u32, _max_block: u32) {}
    fn process(&mut self, _left: &mut [f32], _right: &mut [f32]) {}
    fn set_control(&mut self, _index: usize, _value: f32) {}
    fn control_count(&self) -> usize {
        0
    }
    fn control_name(&self, _index: usize) -> Option<&str> {
        None
    }
}
