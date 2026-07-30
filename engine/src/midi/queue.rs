//! Wait-free SPSC MIDI event mailbox (M2 — drained in rack / filter node).

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::host::MidiEvent;

const CAP: usize = 256;

struct SlotCell(UnsafeCell<MidiEvent>);

// SAFETY: SPSC — only producer writes index `head`, only consumer reads `tail`.
unsafe impl Sync for SlotCell {}

/// Single-producer single-consumer ring. Producer = MIDI worker; consumer = RT.
pub struct MidiEventQueue {
    buf: [SlotCell; CAP],
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl Default for MidiEventQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiEventQueue {
    pub fn new() -> Self {
        const EMPTY: MidiEvent = MidiEvent::Cc {
            channel: 0,
            controller: 0,
            value: 0,
        };
        Self {
            buf: std::array::from_fn(|_| SlotCell(UnsafeCell::new(EMPTY))),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Non-RT producer. Drops oldest on overflow (keep latest).
    pub fn push(&self, msg: MidiEvent) {
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % CAP;
        let tail = self.tail.load(Ordering::Acquire);
        if next == tail {
            let _ = self.tail.compare_exchange(
                tail,
                (tail + 1) % CAP,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
        }
        unsafe {
            *self.buf[head].0.get() = msg;
        }
        self.head.store(next, Ordering::Release);
    }

    /// RT-safe drain into a fixed stack buffer. Returns count written.
    pub fn drain_to(&self, out: &mut [MidiEvent]) -> usize {
        let mut n = 0;
        while n < out.len() {
            let tail = self.tail.load(Ordering::Relaxed);
            let head = self.head.load(Ordering::Acquire);
            if tail == head {
                break;
            }
            out[n] = unsafe { *self.buf[tail].0.get() };
            self.tail.store((tail + 1) % CAP, Ordering::Release);
            n += 1;
        }
        n
    }

    /// Non-RT helper (tests) — may allocate.
    pub fn drain_into(&self, out: &mut Vec<MidiEvent>) {
        let mut tmp = [MidiEvent::Cc {
            channel: 0,
            controller: 0,
            value: 0,
        }; CAP];
        let n = self.drain_to(&mut tmp);
        out.extend_from_slice(&tmp[..n]);
    }
}

unsafe impl Sync for MidiEventQueue {}
