//! Per-bus FFT spectrum publisher for the mixer Analyzer / Equalizer UI.
//!
//! RT path: accumulate mono mid, Hann-windowed analysis frame, real FFT when the
//! UI is watching. Work is capped to **one FFT per process()** and only the
//! requested side (pre XOR post) so the PipeWire quantum never xruns.
//! Non-RT: snapshot magnitude bins (linear) + sample rate.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex;

/// Time-domain analysis window (= FFT size; no zero-pad on the RT path).
pub const ANALYSIS_N: usize = 8192;
pub const FFT_N: usize = ANALYSIS_N;
/// Hop between FFT frames — larger = less RT/UI churn (UI already ballistics-smooths).
pub const HOP: usize = 4096;
pub const MAG_N: usize = FFT_N / 2 + 1;
/// Drop FFT once this many process() ticks pass without a UI watch refresh.
/// ~750 ms at ~48 kHz / 256-frame quanta (~188 Hz).
const WATCH_TTL_TICKS: u64 = 200;

/// Published magnitude frame (linear peak-ish FFT mags).
#[derive(Clone, Debug)]
pub struct SpectrumFrame {
    pub sample_rate: u32,
    pub mags: Vec<f32>,
    pub gen: u64,
}

struct SpectrumBins {
    mags: Box<[AtomicU32]>,
    gen: AtomicU64,
    sample_rate: AtomicU32,
}

impl SpectrumBins {
    fn new() -> Self {
        let mags = (0..MAG_N)
            .map(|_| AtomicU32::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            mags,
            gen: AtomicU64::new(0),
            sample_rate: AtomicU32::new(48_000),
        }
    }

    fn publish(&self, mags: &[f32], sr: u32) {
        let n = mags.len().min(MAG_N);
        for i in 0..n {
            self.mags[i].store(mags[i].to_bits(), Ordering::Relaxed);
        }
        self.sample_rate.store(sr, Ordering::Relaxed);
        self.gen.fetch_add(1, Ordering::Release);
    }

    fn snapshot(&self) -> SpectrumFrame {
        let gen = self.gen.load(Ordering::Acquire);
        let sample_rate = self.sample_rate.load(Ordering::Relaxed);
        let mags = self
            .mags
            .iter()
            .map(|a| f32::from_bits(a.load(Ordering::Relaxed)))
            .collect();
        SpectrumFrame {
            sample_rate,
            mags,
            gen,
        }
    }
}

/// Shared pre/post spectrum for one FX bus.
pub struct SpectrumBus {
    /// RT process() counter — never uses wall clock.
    rt_tick: AtomicU64,
    /// `rt_tick` value at last UI watch.
    watch_tick: AtomicU64,
    /// Set by UI watch; cleared after TTL elapses on RT.
    watch_active: AtomicBool,
    /// `true` → publish post-FX; `false` → pre-FX.
    want_post: AtomicBool,
    pre: SpectrumBins,
    post: SpectrumBins,
}

impl SpectrumBus {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            rt_tick: AtomicU64::new(0),
            watch_tick: AtomicU64::new(0),
            watch_active: AtomicBool::new(false),
            want_post: AtomicBool::new(true),
            pre: SpectrumBins::new(),
            post: SpectrumBins::new(),
        })
    }

    /// RT: advance monotonic process tick (call once per `process()`).
    #[inline]
    pub fn tick_rt(&self) {
        self.rt_tick.fetch_add(1, Ordering::Relaxed);
    }

    /// UI: mark interest for pre (`post=false`) or post-FX spectrum.
    pub fn watch(&self, post: bool) {
        self.want_post.store(post, Ordering::Relaxed);
        self.watch_tick
            .store(self.rt_tick.load(Ordering::Relaxed), Ordering::Relaxed);
        self.watch_active.store(true, Ordering::Relaxed);
    }

    /// UI: drop interest immediately (Idle / hide) — no TTL wait.
    pub fn clear_watch(&self) {
        self.watch_active.store(false, Ordering::Relaxed);
    }

    #[inline]
    pub fn interested(&self) -> bool {
        if !self.watch_active.load(Ordering::Relaxed) {
            return false;
        }
        let now = self.rt_tick.load(Ordering::Relaxed);
        let w = self.watch_tick.load(Ordering::Relaxed);
        if now.saturating_sub(w) >= WATCH_TTL_TICKS {
            self.watch_active.store(false, Ordering::Relaxed);
            return false;
        }
        true
    }

    #[inline]
    pub fn want_post(&self) -> bool {
        self.want_post.load(Ordering::Relaxed)
    }

    pub fn snapshot(&self, post: bool) -> SpectrumFrame {
        if post {
            self.post.snapshot()
        } else {
            self.pre.snapshot()
        }
    }
}

/// RT-local FFT worker (owned by filter userdata — no alloc in process).
pub struct SpectrumRt {
    window: Vec<f32>,
    ring_pre: Vec<f32>,
    ring_post: Vec<f32>,
    w_pre: usize,
    w_post: usize,
    n_pre: usize,
    n_post: usize,
    hop_pre: usize,
    hop_post: usize,
    time: Vec<f32>,
    freq: Vec<Complex<f32>>,
    r2c: Arc<dyn RealToComplex<f32>>,
    mag: Vec<f32>,
    sample_rate: u32,
}

impl SpectrumRt {
    pub fn new(sample_rate: u32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(FFT_N);
        let mut window = vec![0.0f32; ANALYSIS_N];
        for (i, w) in window.iter_mut().enumerate() {
            let t = std::f32::consts::PI * 2.0 * (i as f32) / (ANALYSIS_N as f32);
            *w = 0.5 - 0.5 * t.cos();
        }
        let freq = r2c.make_output_vec();
        Self {
            window,
            ring_pre: vec![0.0; ANALYSIS_N],
            ring_post: vec![0.0; ANALYSIS_N],
            w_pre: 0,
            w_post: 0,
            n_pre: 0,
            n_post: 0,
            hop_pre: 0,
            hop_post: 0,
            time: vec![0.0; FFT_N],
            freq,
            r2c,
            mag: vec![0.0; MAG_N],
            sample_rate: sample_rate.max(1),
        }
    }

    pub fn set_sample_rate(&mut self, sr: u32) {
        if sr > 0 {
            self.sample_rate = sr;
        }
    }

    #[inline]
    fn push_ring(
        ring: &mut [f32],
        w: &mut usize,
        n: &mut usize,
        hop: &mut usize,
        left: &[f32],
        right: &[f32],
    ) {
        let ns = left.len().min(right.len());
        for i in 0..ns {
            ring[*w] = 0.5 * (left[i] + right[i]);
            *w = (*w + 1) % ANALYSIS_N;
            if *n < ANALYSIS_N {
                *n += 1;
            }
            *hop += 1;
        }
    }

    pub fn push_pre(&mut self, left: &[f32], right: &[f32]) {
        Self::push_ring(
            &mut self.ring_pre,
            &mut self.w_pre,
            &mut self.n_pre,
            &mut self.hop_pre,
            left,
            right,
        );
    }

    pub fn push_post(&mut self, left: &[f32], right: &[f32]) {
        Self::push_ring(
            &mut self.ring_post,
            &mut self.w_post,
            &mut self.n_post,
            &mut self.hop_post,
            left,
            right,
        );
    }

    /// Run at most one pending FFT into `bus`. No heap alloc.
    pub fn process_pending(&mut self, bus: &SpectrumBus) {
        if !bus.interested() {
            self.hop_pre = self.hop_pre.min(HOP);
            self.hop_post = self.hop_post.min(HOP);
            return;
        }

        let want_post = bus.want_post();
        if want_post {
            self.hop_pre = self.hop_pre.min(HOP);
            if self.n_post >= ANALYSIS_N && self.hop_post >= HOP {
                self.hop_post -= HOP;
                // Drop backlog — never catch up with multiple FFTs in one quantum.
                if self.hop_post >= HOP {
                    self.hop_post %= HOP;
                }
                Self::fill_time(&self.ring_post, self.w_post, &self.window, &mut self.time);
                if self.r2c.process(&mut self.time, &mut self.freq).is_ok() {
                    Self::mags_from_freq(&self.freq, &mut self.mag);
                    bus.post.publish(&self.mag, self.sample_rate);
                }
            }
        } else {
            self.hop_post = self.hop_post.min(HOP);
            if self.n_pre >= ANALYSIS_N && self.hop_pre >= HOP {
                self.hop_pre -= HOP;
                if self.hop_pre >= HOP {
                    self.hop_pre %= HOP;
                }
                Self::fill_time(&self.ring_pre, self.w_pre, &self.window, &mut self.time);
                if self.r2c.process(&mut self.time, &mut self.freq).is_ok() {
                    Self::mags_from_freq(&self.freq, &mut self.mag);
                    bus.pre.publish(&self.mag, self.sample_rate);
                }
            }
        }
    }

    #[inline]
    fn fill_time(ring: &[f32], write: usize, window: &[f32], time: &mut [f32]) {
        for i in 0..ANALYSIS_N {
            time[i] = ring[(write + i) % ANALYSIS_N] * window[i];
        }
    }

    #[inline]
    fn mags_from_freq(freq: &[Complex<f32>], mag: &mut [f32]) {
        // Normalize vs analysis window (Hann coherent gain ~0.5).
        let norm = 2.0 / (ANALYSIS_N as f32 * 0.5);
        let n = freq.len().min(mag.len()).min(MAG_N);
        for i in 0..n {
            mag[i] = freq[i].norm() * norm;
        }
    }
}
