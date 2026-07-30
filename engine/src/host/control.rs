//! Wait-free SPSC control mailbox for the RT thread.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

use uuid::Uuid;

const CAP: usize = 256;

#[derive(Clone, Copy, Debug)]
pub enum ControlMsg {
    /// Set control by slot_id + control port index.
    Param {
        slot_id: Uuid,
        control_index: u32,
        value: f32,
        /// Sample offset within the current process block (automation).
        sample_offset: u32,
    },
    /// Insert power (bypassed = true → fade to dry).
    Bypass {
        slot_id: Uuid,
        bypassed: bool,
        sample_offset: u32,
    },
}

impl ControlMsg {
    pub fn param(slot_id: Uuid, control_index: u32, value: f32) -> Self {
        Self::Param {
            slot_id,
            control_index,
            value,
            sample_offset: 0,
        }
    }

    pub fn bypass(slot_id: Uuid, bypassed: bool) -> Self {
        Self::Bypass {
            slot_id,
            bypassed,
            sample_offset: 0,
        }
    }

    pub fn with_offset(mut self, sample_offset: u32) -> Self {
        match &mut self {
            Self::Param { sample_offset: o, .. } | Self::Bypass { sample_offset: o, .. } => {
                *o = sample_offset;
            }
        }
        self
    }
}

struct SlotCell(UnsafeCell<ControlMsg>);

// SAFETY: SPSC — only producer writes index `head`, only consumer reads `tail`.
unsafe impl Sync for SlotCell {}

/// Single-producer single-consumer ring. Producer = worker; consumer = RT.
pub struct ControlQueue {
    buf: [SlotCell; CAP],
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl Default for ControlQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlQueue {
    pub fn new() -> Self {
        const EMPTY: ControlMsg = ControlMsg::Bypass {
            slot_id: uuid::Uuid::nil(),
            bypassed: false,
            sample_offset: 0,
        };
        Self {
            buf: std::array::from_fn(|_| SlotCell(UnsafeCell::new(EMPTY))),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Non-RT producer. Drops oldest on overflow (keep latest).
    pub fn push(&self, msg: ControlMsg) {
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
    pub fn drain_to(&self, out: &mut [ControlMsg]) -> usize {
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
    pub fn drain_into(&self, out: &mut Vec<ControlMsg>) {
        let mut tmp = [ControlMsg::Bypass {
            slot_id: Uuid::nil(),
            bypassed: false,
            sample_offset: 0,
        }; CAP];
        let n = self.drain_to(&mut tmp);
        out.extend_from_slice(&tmp[..n]);
    }
}

unsafe impl Sync for ControlQueue {}
