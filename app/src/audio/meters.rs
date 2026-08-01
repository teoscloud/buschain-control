//! Live track peak meters — Pulse monitor taps (opt-in via `BUSCHAIN_PULSE_METERS`).
//!
//! Default path: wet strips use in-process FX host peaks; dry strips use native
//! `buschain_mtr_*` taps. Never meter `buschain_fx_*`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use libpulse_binding::context::{Context, FlagSet as CtxFlags, State as CtxState};
use libpulse_binding::def::BufferAttr;
use libpulse_binding::mainloop::threaded::Mainloop;
use libpulse_binding::proplist::{self, Proplist};
use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::{FlagSet as StreamFlags, PeekResult, State as StreamState, Stream};

/// Where to listen, and which key the UI uses for [`MeterHub::peak_db`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeterTap {
    /// Stable lookup key (track bus name, e.g. `buschain_track_…`).
    pub key: String,
    /// Sink whose `.monitor` we record (post when wet, else the bus).
    pub tap: String,
    /// Capture rate — must follow GraphClock after bus migrate (48k streams die on 96k posts).
    pub sample_rate: u32,
}

enum MeterCmd {
    SetTargets(Vec<MeterTap>),
    /// Tear down and recreate all streams (same tap names after null-sink migrate).
    ForceRebuild,
    /// Pause Pulse peeks while the UI is Idle / withdrawn.
    SetPaused(bool),
    Shutdown,
}

/// Shared peak store: lookup key → displayed level in dBFS (with ballistics).
#[derive(Clone)]
pub struct MeterHub {
    peaks_db: Arc<Mutex<HashMap<String, f32>>>,
    cmd_tx: Sender<MeterCmd>,
    alive: Arc<AtomicBool>,
}

impl MeterHub {
    pub fn spawn() -> Self {
        let peaks_db = Arc::new(Mutex::new(HashMap::new()));
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let alive = Arc::new(AtomicBool::new(true));
        let peaks_thread = peaks_db.clone();
        let alive_thread = alive.clone();

        thread::Builder::new()
            .name("buschain-control-meters".into())
            .spawn(move || meter_loop(cmd_rx, peaks_thread, alive_thread))
            .expect("spawn meter thread");

        Self {
            peaks_db,
            cmd_tx,
            alive,
        }
    }

    pub fn set_targets(&self, taps: Vec<MeterTap>) {
        let _ = self.cmd_tx.send(MeterCmd::SetTargets(taps));
    }

    /// Force stream recreate — call after clock bind / bus migrate (names unchanged).
    pub fn force_rebuild(&self) {
        let _ = self.cmd_tx.send(MeterCmd::ForceRebuild);
    }

    /// Drop monitor streams and stop peeking while the window is Idle.
    pub fn set_paused(&self, paused: bool) {
        let _ = self.cmd_tx.send(MeterCmd::SetPaused(paused));
    }

    /// Current meter reading in dBFS (−inf shown as −90).
    /// `key` is the track bus name (`sink_name`), not necessarily the tap sink.
    pub fn peak_db(&self, key: &str) -> f32 {
        self.peaks_db
            .lock()
            .ok()
            .and_then(|m| m.get(key).copied())
            .unwrap_or(-90.0)
    }

    pub fn shutdown(&self) {
        self.alive.store(false, Ordering::SeqCst);
        let _ = self.cmd_tx.send(MeterCmd::Shutdown);
    }
}

struct TrackMeter {
    key: String,
    tap: String,
    sample_rate: u32,
    stream: Stream,
    raw: f32,
    display: f32,
    last_tick: Instant,
    /// When the stream left Ready — used to heal stuck captures.
    not_ready_since: Option<Instant>,
}

static STREAM_GEN: AtomicU64 = AtomicU64::new(1);

fn meter_loop(
    cmd_rx: Receiver<MeterCmd>,
    peaks_db: Arc<Mutex<HashMap<String, f32>>>,
    alive: Arc<AtomicBool>,
) {
    let mut proplist = Proplist::new().unwrap();
    let _ = proplist.set_str(
        proplist::properties::APPLICATION_NAME,
        "BusChain Control Meters",
    );

    let Some(mut ml) = Mainloop::new() else {
        return;
    };
    let Some(mut ctx) = Context::new_with_proplist(&ml, "buschain-control-meters", &proplist) else {
        return;
    };
    if ctx.connect(None, CtxFlags::NOFLAGS, None).is_err() {
        return;
    }
    if ml.start().is_err() {
        return;
    }

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        ml.lock();
        let st = ctx.get_state();
        ml.unlock();
        if st == CtxState::Ready {
            break;
        }
        if st == CtxState::Failed || st == CtxState::Terminated || Instant::now() > deadline {
            let _ = ml.stop();
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }

    let mut meters: Vec<TrackMeter> = Vec::new();
    let mut pending_targets: Option<Vec<MeterTap>> = None;
    let mut last_targets: Vec<MeterTap> = Vec::new();
    let mut paused = false;

    while alive.load(Ordering::SeqCst) {
        let mut force = false;
        loop {
            match cmd_rx.try_recv() {
                Ok(MeterCmd::SetTargets(t)) => pending_targets = Some(t),
                Ok(MeterCmd::ForceRebuild) => force = true,
                Ok(MeterCmd::SetPaused(p)) => {
                    if p && !paused {
                        ml.lock();
                        for mut m in meters.drain(..) {
                            let _ = m.stream.disconnect();
                        }
                        ml.unlock();
                        if let Ok(mut g) = peaks_db.lock() {
                            g.clear();
                        }
                    }
                    paused = p;
                }
                Ok(MeterCmd::Shutdown) => {
                    alive.store(false, Ordering::SeqCst);
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    alive.store(false, Ordering::SeqCst);
                    break;
                }
            }
        }
        if !alive.load(Ordering::SeqCst) {
            break;
        }

        if paused {
            // Keep target list fresh while Idle, but do not open streams.
            if let Some(targets) = pending_targets.take() {
                last_targets = targets;
            }
            thread::sleep(Duration::from_millis(250));
            continue;
        }

        if force {
            // Drop kept streams so rebuild recreates against new sink indices.
            ml.lock();
            for mut m in meters.drain(..) {
                let _ = m.stream.disconnect();
            }
            ml.unlock();
            thread::sleep(Duration::from_millis(40));
            let targets = pending_targets
                .take()
                .unwrap_or_else(|| last_targets.clone());
            last_targets = targets.clone();
            rebuild_meters(&mut ml, &mut ctx, &mut meters, &targets);
        } else if let Some(targets) = pending_targets.take() {
            // Ignore no-op retargets (prevents heal thrash / duplicate PW streams).
            if targets != last_targets {
                last_targets = targets.clone();
                rebuild_meters(&mut ml, &mut ctx, &mut meters, &targets);
            }
        } else if meters.is_empty() && !last_targets.is_empty() {
            // Resume from Idle — recreate streams for the last known taps.
            let targets = last_targets.clone();
            rebuild_meters(&mut ml, &mut ctx, &mut meters, &targets);
        }

        // Enforce one stream per strip key (zombies from failed disconnects).
        dedupe_meters_by_key(&mut ml, &mut meters);

        // Heal streams stuck non-Ready, or Ready but unbound (Source=invalid).
        let mut need_heal: Vec<(String, String, u32)> = Vec::new();
        {
            ml.lock();
            let now = Instant::now();
            for m in &mut meters {
                let ready = m.stream.get_state() == StreamState::Ready;
                let unbound = ready
                    && m.stream
                        .get_device_index()
                        .map(|i| i == u32::MAX || i == 0)
                        .unwrap_or(true);
                if ready && !unbound {
                    m.not_ready_since = None;
                } else {
                    let since = m.not_ready_since.get_or_insert(now);
                    if now.duration_since(*since) > Duration::from_millis(1500) {
                        need_heal.push((m.key.clone(), m.tap.clone(), m.sample_rate));
                        *since = now;
                    }
                }
            }
            ml.unlock();
        }
        if !need_heal.is_empty() {
            for (key, tap, rate) in need_heal {
                recreate_one(&mut ml, &mut ctx, &mut meters, &key, &tap, rate);
            }
        }

        ml.lock();
        for m in &mut meters {
            if m.stream.get_state() != StreamState::Ready {
                continue;
            }
            loop {
                match m.stream.peek() {
                    Ok(PeekResult::Data(bytes)) => {
                        let mut peak = 0.0f32;
                        for chunk in bytes.chunks_exact(4) {
                            let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                            let a = v.abs();
                            if a > peak {
                                peak = a;
                            }
                        }
                        if peak > m.raw {
                            m.raw = peak;
                        }
                        let _ = m.stream.discard();
                    }
                    Ok(PeekResult::Hole(_)) => {
                        let _ = m.stream.discard();
                    }
                    Ok(PeekResult::Empty) => break,
                    Err(_) => break,
                }
            }
        }
        ml.unlock();

        let now = Instant::now();
        if let Ok(mut g) = peaks_db.lock() {
            // Fresh frame map — never max-merge with a stale zombie stream.
            let mut frame: HashMap<String, f32> = HashMap::new();
            for m in &mut meters {
                let dt = now.saturating_duration_since(m.last_tick).as_secs_f32();
                m.last_tick = now;
                if m.raw > m.display {
                    m.display = m.raw;
                } else {
                    let release = (1.0 - (-dt * 4.0).exp()).clamp(0.0, 1.0);
                    m.display *= 1.0 - release;
                }
                let db = if m.display < 1e-6 {
                    -90.0
                } else {
                    (20.0 * m.display.log10()).clamp(-90.0, 12.0)
                };
                frame
                    .entry(m.key.clone())
                    .and_modify(|p| *p = (*p).max(db))
                    .or_insert(db);
                m.raw *= 0.35;
            }
            *g = frame;
        }

        thread::sleep(Duration::from_millis(16));
    }

    ml.lock();
    for m in meters.drain(..) {
        let mut s = m.stream;
        let _ = s.disconnect();
    }
    ml.unlock();
    let _ = ml.stop();
}

fn dedupe_meters_by_key(ml: &mut Mainloop, meters: &mut Vec<TrackMeter>) {
    // Dedupe identical (key, tap) pairs only — bus+post dual taps share a key.
    ml.lock();
    let mut by_pair: HashMap<(String, String), TrackMeter> = HashMap::new();
    for m in meters.drain(..) {
        let pair = (m.key.clone(), m.tap.clone());
        if let Some(mut prev) = by_pair.insert(pair, m) {
            let _ = prev.stream.disconnect();
        }
    }
    *meters = by_pair.into_values().collect();
    ml.unlock();
}

fn rebuild_meters(
    ml: &mut Mainloop,
    ctx: &mut Context,
    meters: &mut Vec<TrackMeter>,
    targets: &[MeterTap],
) {
    // Allow multiple taps per strip key (bus + post) — peak_db takes max.
    let wanted: Vec<(String, String, u32)> = {
        let mut v = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for t in targets {
            if t.key.is_empty() || t.tap.is_empty() {
                continue;
            }
            if seen.insert((t.key.clone(), t.tap.clone())) {
                let rate = if t.sample_rate >= 8_000 {
                    t.sample_rate
                } else {
                    48_000
                };
                v.push((t.key.clone(), t.tap.clone(), rate));
            }
        }
        v
    };

    ml.lock();
    let mut kept = Vec::new();
    for mut m in meters.drain(..) {
        if wanted
            .iter()
            .any(|(k, tap, rate)| k == &m.key && tap == &m.tap && *rate == m.sample_rate)
        {
            kept.push(m);
        } else {
            let _ = m.stream.disconnect();
        }
    }
    // Dedupe kept by (key, tap).
    let mut by_pair: HashMap<(String, String), TrackMeter> = HashMap::new();
    for m in kept {
        let pair = (m.key.clone(), m.tap.clone());
        if let Some(mut prev) = by_pair.insert(pair, m) {
            let _ = prev.stream.disconnect();
        }
    }
    *meters = by_pair.into_values().collect();
    ml.unlock();

    for (key, tap, rate) in &wanted {
        let already = {
            ml.lock();
            let ok = meters
                .iter()
                .any(|m| &m.key == key && &m.tap == tap && m.sample_rate == *rate);
            ml.unlock();
            ok
        };
        if already {
            continue;
        }
        recreate_one(ml, ctx, meters, key, tap, *rate);
    }
}

fn recreate_one(
    ml: &mut Mainloop,
    ctx: &mut Context,
    meters: &mut Vec<TrackMeter>,
    key: &str,
    tap: &str,
    sample_rate: u32,
) {
    // Drop existing stream for this exact (key, tap) pair — keep the other dual tap.
    ml.lock();
    let mut kept = Vec::with_capacity(meters.len());
    for mut m in meters.drain(..) {
        if m.key == key && m.tap == tap {
            let _ = m.stream.disconnect();
        } else {
            kept.push(m);
        }
    }
    *meters = kept;
    ml.unlock();

    // Let Pulse/PipeWire finish tearing down before we reuse the logical name.
    thread::sleep(Duration::from_millis(30));

    let rate = if sample_rate >= 8_000 {
        sample_rate
    } else {
        48_000
    };
    let spec = Spec {
        format: Format::F32le,
        channels: 2,
        rate,
    };
    if !spec.is_valid() {
        return;
    }
    // ~2 ms of stereo f32 at GraphClock.
    let frag = (rate as u32 / 500).saturating_mul(8).max(1024);
    let attr = BufferAttr {
        maxlength: rate.saturating_mul(16).max(192_000),
        tlength: u32::MAX,
        prebuf: u32::MAX,
        minreq: u32::MAX,
        fragsize: frag,
    };

    // Unique node name every recreate — prevents zombie meter-* doubles in PW.
    let gen = STREAM_GEN.fetch_add(1, Ordering::Relaxed);
    let monitor = format!("{tap}.monitor");
    let stream_name = format!("meter-{key}-{gen}");

    let mut spl = match Proplist::new() {
        Some(p) => p,
        None => return,
    };
    let _ = spl.set_str(proplist::properties::APPLICATION_NAME, "BusChain Control Meters");
    let _ = spl.set_str(proplist::properties::APPLICATION_ID, "org.buschain.control.meters");
    let _ = spl.set_str(proplist::properties::MEDIA_NAME, &stream_name);
    let _ = spl.set_str(proplist::properties::MEDIA_ROLE, "filter");
    let _ = spl.set_str("media.category", "Filter");
    let _ = spl.set_str("node.name", &stream_name);
    let _ = spl.set_str("node.virtual", "true");
    // Pulse record streams clutter pavucontrol Recording. Default OFF — wet path
    // uses host pre/post peaks. Set BUSCHAIN_PULSE_METERS=1 to force Pulse taps.
    let pulse_meters = matches!(
        std::env::var("BUSCHAIN_PULSE_METERS").as_deref(),
        Ok("1") | Ok("true") | Ok("yes") | Ok("on")
    );
    if !pulse_meters {
        return;
    }

    ml.lock();
    let mut stream = match Stream::new_with_proplist(ctx, &stream_name, &spec, None, &mut spl) {
        Some(s) => s,
        None => {
            ml.unlock();
            return;
        }
    };
    let flags = StreamFlags::START_UNMUTED | StreamFlags::AUTO_TIMING_UPDATE;
    let mut connected = stream
        .connect_record(Some(monitor.as_str()), Some(&attr), flags)
        .is_ok();
    if connected {
        ml.unlock();
        thread::sleep(Duration::from_millis(50));
        ml.lock();
        let bound = stream.get_state() == StreamState::Ready
            && stream
                .get_device_index()
                .map(|i| i != u32::MAX && i != 0)
                .unwrap_or(false);
        if !bound {
            let _ = stream.disconnect();
            connected = stream
                .connect_record(Some(tap), Some(&attr), flags)
                .is_ok();
        }
    }
    if !connected {
        ml.unlock();
        return;
    }
    meters.push(TrackMeter {
        key: key.to_string(),
        tap: tap.to_string(),
        sample_rate: rate,
        stream,
        raw: 0.0,
        display: 0.0,
        last_tick: Instant::now(),
        not_ready_since: Some(Instant::now()),
    });
    ml.unlock();
}
