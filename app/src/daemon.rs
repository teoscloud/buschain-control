//! IPC runtime for ctl / waybar / GTK mixer.
//!
//! - Headless: [`run`] owns a local worker (legacy `buschain-daemon`).
//! - Embedded: [`start_embedded`] forwards into the tray UI's in-process worker.

use std::io::Read;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use uuid::Uuid;

use crate::audio::graph::{self, PwSnapshot};
use crate::audio::worker::{AudioWorker, Command, Event, SessionAppliedKind};
use crate::ipc::{self, Request, Response, SessionListItem, Status};
use crate::session::{self, Session};

/// Track gain/mute patch for UI ↔ embedded IPC session sync.
#[derive(Debug, Clone, Copy)]
pub struct TrackMixerPatch {
    pub track_id: Uuid,
    pub gain_db: f32,
    pub mute: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrackMixerOrigin {
    /// `buschain-ctl` / Quickshell via SetTrackMixer.
    External,
    /// Tray / mixer UI fader.
    Ui,
}

/// Last intentional track vol/mute write. Sticky until replaced — TTL expiry was
/// letting SessionApplied / dual-session copies snap faders back after ~900ms.
#[derive(Debug, Clone, Copy)]
struct TrackMixerAuthority {
    gain_db: f32,
    mute: bool,
    /// Monotonic per-track generation; worker drops SetTrackLevel with older rev.
    rev: u64,
    origin: TrackMixerOrigin,
}

fn ui_track_mixer_patches() -> &'static Mutex<Vec<TrackMixerPatch>> {
    static Q: OnceLock<Mutex<Vec<TrackMixerPatch>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(Vec::new()))
}

fn daemon_track_mixer_patches() -> &'static Mutex<Vec<TrackMixerPatch>> {
    static Q: OnceLock<Mutex<Vec<TrackMixerPatch>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(Vec::new()))
}

fn track_mixer_authority() -> &'static Mutex<std::collections::HashMap<Uuid, TrackMixerAuthority>> {
    static Q: OnceLock<Mutex<std::collections::HashMap<Uuid, TrackMixerAuthority>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn mixer_vals_differ(a_gain: f32, a_mute: bool, gain_db: f32, mute: bool) -> bool {
    (a_gain - gain_db).abs() > 0.001 || a_mute != mute
}

/// Record an intentional mixer write. Returns the new generation for SetTrackLevel.
pub fn note_track_mixer_write(
    track_id: Uuid,
    gain_db: f32,
    mute: bool,
    external: bool,
) -> u64 {
    let origin = if external {
        TrackMixerOrigin::External
    } else {
        TrackMixerOrigin::Ui
    };
    let Ok(mut m) = track_mixer_authority().lock() else {
        return 0;
    };
    let rev = m.get(&track_id).map(|a| a.rev.saturating_add(1)).unwrap_or(1);
    m.insert(
        track_id,
        TrackMixerAuthority {
            gain_db,
            mute,
            rev,
            origin,
        },
    );
    rev
}

pub fn track_mixer_write_rev(track_id: Uuid) -> u64 {
    track_mixer_authority()
        .lock()
        .ok()
        .and_then(|m| m.get(&track_id).map(|a| a.rev))
        .unwrap_or(0)
}

fn clear_daemon_track_mixer_patch(track_id: Uuid) {
    if let Ok(mut q) = daemon_track_mixer_patches().lock() {
        q.retain(|p| p.track_id != track_id);
    }
}

/// Overlay last intentional track vol/mute so GetMixer / SessionApplied / egui
/// cannot snap sliders back to a stale session copy.
pub fn overlay_track_mixer_authority(session: &mut Session) {
    let Ok(m) = track_mixer_authority().lock() else {
        return;
    };
    for (id, a) in m.iter() {
        if let Some(t) = session.tracks.iter_mut().find(|t| t.id == *id) {
            t.gain_db = a.gain_db;
            t.mute = a.mute;
        }
    }
}

/// Seed authority from a loaded session (Full Apply / session switch).
pub fn seed_track_mixer_authority_from_session(session: &Session) {
    let Ok(mut m) = track_mixer_authority().lock() else {
        return;
    };
    m.clear();
    for t in &session.tracks {
        m.insert(
            t.id,
            TrackMixerAuthority {
                gain_db: t.gain_db,
                mute: t.mute,
                rev: 1,
                origin: TrackMixerOrigin::Ui,
            },
        );
    }
}

/// Active external (QS/ctl) mixer write the UI session may not have adopted yet.
pub fn external_track_mixer_authority(track_id: Uuid) -> Option<(f32, bool)> {
    let Ok(m) = track_mixer_authority().lock() else {
        return None;
    };
    m.get(&track_id).and_then(|a| {
        if a.origin == TrackMixerOrigin::External {
            Some((a.gain_db, a.mute))
        } else {
            None
        }
    })
}

/// Latest intentional gain/mute for a track (any origin), if known.
pub fn track_mixer_authority_values(track_id: Uuid) -> Option<(f32, bool)> {
    let Ok(m) = track_mixer_authority().lock() else {
        return None;
    };
    m.get(&track_id).map(|a| (a.gain_db, a.mute))
}

/// UI adopted a QS/ctl patch — local fader moves may overwrite authority.
pub fn acknowledge_track_mixer_authority(track_id: Uuid) {
    if let Ok(mut m) = track_mixer_authority().lock() {
        if let Some(a) = m.get_mut(&track_id) {
            a.origin = TrackMixerOrigin::Ui;
        }
    }
}

/// QS/ctl changed a track fader — UI should adopt on next tick.
pub fn push_track_mixer_to_ui(track_id: Uuid, gain_db: f32, mute: bool) {
    note_track_mixer_write(track_id, gain_db, mute, true);
    // Drop any in-flight UI→daemon echo that still carries the pre-drag value.
    clear_daemon_track_mixer_patch(track_id);
    if let Ok(mut q) = ui_track_mixer_patches().lock() {
        // Coalesce: keep latest patch per track.
        if let Some(p) = q.iter_mut().find(|p| p.track_id == track_id) {
            *p = TrackMixerPatch {
                track_id,
                gain_db,
                mute,
            };
        } else {
            q.push(TrackMixerPatch {
                track_id,
                gain_db,
                mute,
            });
        }
    }
}

/// Drain patches for the tray UI mixer strips.
pub fn take_track_mixer_ui_patches() -> Vec<TrackMixerPatch> {
    ui_track_mixer_patches()
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

/// UI changed a track fader — embedded daemon session / get_mixer should adopt.
/// Returns the mixer write generation to stamp on SetTrackLevel.
pub fn push_track_mixer_to_daemon(track_id: Uuid, gain_db: f32, mute: bool) -> u64 {
    // While a QS/ctl write is still authoritative and the UI session has not
    // adopted it, ignore echoes that would regress the fader.
    if let Some((ag, am)) = external_track_mixer_authority(track_id) {
        if mixer_vals_differ(ag, am, gain_db, mute) {
            return track_mixer_write_rev(track_id);
        }
    }
    let rev = note_track_mixer_write(track_id, gain_db, mute, false);
    if let Ok(mut q) = daemon_track_mixer_patches().lock() {
        if let Some(p) = q.iter_mut().find(|p| p.track_id == track_id) {
            *p = TrackMixerPatch {
                track_id,
                gain_db,
                mute,
            };
        } else {
            q.push(TrackMixerPatch {
                track_id,
                gain_db,
                mute,
            });
        }
    }
    rev
}

fn apply_track_mixer_patches(session: &mut Session, patches: &[TrackMixerPatch]) -> bool {
    let mut dirty = false;
    for p in patches {
        if let Some(t) = session.tracks.iter_mut().find(|t| t.id == p.track_id) {
            if (t.gain_db - p.gain_db).abs() > 0.001 || t.mute != p.mute {
                t.gain_db = p.gain_db;
                t.mute = p.mute;
                dirty = true;
            }
        }
    }
    dirty
}

fn drain_daemon_track_mixer_patches(session: &mut Session) {
    let patches = daemon_track_mixer_patches()
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default();
    if !patches.is_empty() && apply_track_mixer_patches(session, &patches) {
        let _ = session.save();
    }
    // Always re-assert fresh authority so a just-written QS value wins over a
    // stale UI echo that drained in the same handle() call.
    overlay_track_mixer_authority(session);
}

/// Set by embedded IPC on `Shutdown` / tray Quit coordination.
static EMBEDDED_QUIT: AtomicBool = AtomicBool::new(false);
/// Tray / ctl asked the main window to show.
static EMBEDDED_SHOW: AtomicBool = AtomicBool::new(false);

pub fn take_embedded_quit() -> bool {
    EMBEDDED_QUIT.swap(false, Ordering::SeqCst)
}

pub fn take_embedded_show() -> bool {
    EMBEDDED_SHOW.swap(false, Ordering::SeqCst)
}

pub fn request_embedded_show() {
    EMBEDDED_SHOW.store(true, Ordering::SeqCst);
}

pub fn request_embedded_quit() {
    EMBEDDED_QUIT.store(true, Ordering::SeqCst);
}

enum CmdSink {
    /// Headless daemon owns the worker.
    Worker(AudioWorker),
    /// Tray UI owns the worker; IPC only forwards commands.
    Forward {
        tx: Sender<Command>,
        embedded: bool,
    },
}

impl CmdSink {
    fn send(&self, cmd: Command) {
        match self {
            Self::Worker(w) => w.send(cmd),
            Self::Forward { tx, .. } => {
                let _ = tx.send(cmd);
            }
        }
    }

    fn poll(&self) -> Vec<Event> {
        match self {
            Self::Worker(w) => w.poll(),
            Self::Forward { .. } => Vec::new(),
        }
    }

    fn is_embedded(&self) -> bool {
        matches!(self, Self::Forward { embedded: true, .. })
    }
}

struct DaemonState {
    session: Session,
    cmds: CmdSink,
    snapshot: PwSnapshot,
    status_msg: String,
    last_idle: Instant,
    /// Last forced `pactl` snapshot refresh — avoid blocking IPC on every poll.
    last_snapshot_at: Instant,
    /// Optimistic Master HW vol/mute so rapid waybar/ctl scroll bursts
    /// accumulate. TTL must match snapshot overlay — a shorter overlay used to
    /// clear the cache on a stale Event::Snapshot while status still trusted
    /// the cache (or the reverse), desyncing waybar vs panel.
    hw_vol_cache: Option<(u32, bool, Instant)>,
    /// Last applied AdjustHwVolume — coalesce Waybar parallel forkExec floods.
    hw_vol_last_adjust: Option<Instant>,
}

/// Cache TTL for Master HW % (scroll optimistic + idle status). Avoids 2× pactl
/// on every Waybar `exec` while still refreshing within a couple seconds.
const HW_VOL_STATUS_TTL: Duration = Duration::from_millis(2000);
/// Reserved for diagnostics; AdjustHwVolume always applies (stuck scroll > flood).
#[allow(dead_code)]
const HW_VOL_ADJUST_MIN: Duration = Duration::from_millis(8);
/// Skip RTMIN+9 while this marker is fresher than cool-down.
const WAYBAR_SCROLL_COOLDOWN_MS: u128 = 1500;

/// Ordered Master HW pactl writer — IPC ACKs before `pactl` so Waybar scroll
/// forkExecs return instantly. Seq picks the newest intent (never an older %).
fn hw_vol_apply_tx() -> &'static Sender<(u64, String, u32)> {
    static TX: OnceLock<Sender<(u64, String, u32)>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<(u64, String, u32)>();
        thread::Builder::new()
            .name("hw-vol-apply".into())
            .spawn(move || {
                let mut pending: Option<(u64, String, u32)> = None;
                loop {
                    let first = pending.take().or_else(|| rx.recv().ok());
                    let Some(item) = first else {
                        break;
                    };
                    let mut best = item;
                    while let Ok(next) = rx.try_recv() {
                        if next.0 >= best.0 {
                            best = next;
                        }
                    }
                    let (seq, name, pct) = best;
                    let _ = seq;
                    if let Err(e) = graph::set_sink_volume_pct(&name, pct) {
                        eprintln!("buschain-control: hw-vol apply {pct}%: {e}");
                    }
                    // Prefer highest seq among anything that arrived during pactl.
                    let mut again: Option<(u64, String, u32)> = None;
                    while let Ok(next) = rx.try_recv() {
                        match &again {
                            Some(cur) if next.0 < cur.0 => {}
                            _ => again = Some(next),
                        }
                    }
                    pending = again;
                }
            })
            .expect("hw-vol-apply thread");
        tx
    })
}

fn enqueue_hw_volume_set(name: String, pct: u32) {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let _ = hw_vol_apply_tx().send((seq, name, pct.min(100)));
}

fn lat_trace(msg: &str) {
    if matches!(
        std::env::var("BUSCHAIN_CONTROL_LAT_TRACE").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    ) {
        eprintln!("[buschain-lat] {msg}");
    }
}

impl DaemonState {
    fn new_headless() -> Self {
        let mut session = Session::load();
        session.autostart_graph = false;
        let worker = AudioWorker::spawn_local();
        worker.send(Command::ApplySession(session.clone()));
        Self {
            session,
            cmds: CmdSink::Worker(worker),
            snapshot: PwSnapshot::default(),
            status_msg: "daemon starting".into(),
            last_idle: Instant::now(),
            last_snapshot_at: Instant::now()
                .checked_sub(Duration::from_secs(60))
                .unwrap_or_else(Instant::now),
            hw_vol_cache: None,
            hw_vol_last_adjust: None,
        }
    }

    fn new_embedded(tx: Sender<Command>) -> Self {
        let mut session = Session::load();
        session.autostart_graph = false;
        Self {
            session,
            cmds: CmdSink::Forward { tx, embedded: true },
            snapshot: PwSnapshot::default(),
            status_msg: "embedded ipc".into(),
            last_idle: Instant::now(),
            last_snapshot_at: Instant::now()
                .checked_sub(Duration::from_secs(60))
                .unwrap_or_else(Instant::now),
            hw_vol_cache: None,
            hw_vol_last_adjust: None,
        }
    }

    fn waybar_runtime_dir() -> std::path::PathBuf {
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("buschain-control")
    }

    /// Mark an active Master HW scroll gesture for pill / RTMIN cool-down.
    fn touch_waybar_scroll_marker() {
        let dir = Self::waybar_runtime_dir();
        let _ = std::fs::create_dir_all(&dir);
        let marker = dir.join("waybar-scroll-ms");
        let _ = std::fs::write(&marker, b"1");
    }

    fn waybar_scroll_marker_hot() -> bool {
        let marker = Self::waybar_runtime_dir().join("waybar-scroll-ms");
        std::fs::metadata(&marker)
            .and_then(|m| m.modified())
            .map(|t| {
                t.elapsed()
                    .map(|d| d.as_millis() < WAYBAR_SCROLL_COOLDOWN_MS)
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }


    /// Refresh PipeWire snapshot at most every `ttl` (or when empty).
    fn ensure_snapshot(&mut self, ttl: Duration) {
        self.drain_events();
        let empty = self.snapshot.sinks.is_empty() && self.snapshot.sink_inputs.is_empty();
        if empty || self.last_snapshot_at.elapsed() >= ttl {
            self.snapshot = graph::refresh_snapshot();
            self.last_snapshot_at = Instant::now();
            self.apply_hw_cache_to_snapshot();
        }
    }

    fn patch_sink_input(&mut self, index: u32, pct: Option<u32>, mute: Option<bool>) {
        if let Some(si) = self.snapshot.sink_inputs.iter_mut().find(|s| s.index == index) {
            if let Some(p) = pct {
                si.volume_pct = p.min(150);
            }
            if let Some(m) = mute {
                si.mute = m;
            }
        }
    }

    fn patch_device_vol(
        &mut self,
        sinks: bool,
        name: &str,
        pct: Option<u32>,
        mute: Option<bool>,
    ) {
        let list = if sinks {
            &mut self.snapshot.sinks
        } else {
            &mut self.snapshot.sources
        };
        if let Some(d) = list.iter_mut().find(|d| d.name == name) {
            if let Some(p) = pct {
                d.volume_pct = p.min(150);
            }
            if let Some(m) = mute {
                d.mute = m;
            }
        }
    }

    fn soft_bind(&mut self) {
        let sinks: Vec<_> = self
            .snapshot
            .sinks
            .iter()
            .map(|s| (s.name.clone(), s.description.clone()))
            .collect();
        let sources: Vec<_> = self
            .snapshot
            .sources
            .iter()
            .map(|s| (s.name.clone(), s.description.clone()))
            .collect();
        let report = session::resolve_devices(&mut self.session, &sinks, &sources);
        if !report.messages.is_empty() {
            self.status_msg = report.join();
        }
    }

    fn drain_events(&mut self) {
        for ev in self.cmds.poll() {
            match ev {
                Event::Snapshot(s) => {
                    self.snapshot = s;
                    self.apply_hw_cache_to_snapshot();
                    self.soft_bind();
                }
                Event::SinkInputs(inputs) => {
                    self.snapshot.sink_inputs = inputs;
                }
                Event::Status(s) | Event::Error(s) => self.status_msg = s,
                Event::MidiSnapshot(_) | Event::MidiLearnBound { .. } => {}
                Event::FxReady { .. } => {}
                Event::SessionApplied {
                    session,
                    message,
                    kind,
                } => {
                    // Topology-only applies often carry a stale gain/mute clone from
                    // before a QS/UI fader move — keep live mixer bits unless this is
                    // a full session load / clock replace.
                    let keep_mixer = !matches!(
                        kind,
                        SessionAppliedKind::Full | SessionAppliedKind::Clock
                    );
                    let mixer: Vec<(Uuid, f32, bool)> = if keep_mixer {
                        self.session
                            .tracks
                            .iter()
                            .map(|t| (t.id, t.gain_db, t.mute))
                            .collect()
                    } else {
                        Vec::new()
                    };
                    // Never reseed authority from ApplySession clones — those often
                    // predate a QS/UI fader move and were wiping gain back to 0 dB.
                    // Explicit SessionLoad calls seed_track_mixer_authority_from_session.
                    self.session = session;
                    for (id, gain_db, mute) in mixer {
                        if let Some(t) = self.session.tracks.iter_mut().find(|t| t.id == id)
                        {
                            t.gain_db = gain_db;
                            t.mute = mute;
                        }
                    }
                    overlay_track_mixer_authority(&mut self.session);
                    self.status_msg = message;
                }
            }
        }
    }

    fn hw_sink(&self) -> Option<&str> {
        self.session.master_output.as_deref()
    }

    fn is_master_hw_sink(&self, name: &str) -> bool {
        self.session.master_output.as_deref() == Some(name)
    }

    /// Overlay fresh optimistic / status-cached HW % onto the snapshot.
    fn apply_hw_cache_to_snapshot(&mut self) {
        let Some((pct, mute, at)) = self.hw_vol_cache else {
            return;
        };
        if at.elapsed() >= HW_VOL_STATUS_TTL {
            self.hw_vol_cache = None;
            return;
        }
        if let Some(name) = self.session.master_output.clone() {
            if let Some(sink) = self.snapshot.sinks.iter_mut().find(|x| x.name == name) {
                sink.volume_pct = pct;
                sink.mute = mute;
            }
        }
    }

    fn patch_hw_into_snapshot(&mut self, pct: u32, mute: bool) {
        if let Some(name) = self.session.master_output.clone() {
            if let Some(s) = self.snapshot.sinks.iter_mut().find(|s| s.name == name) {
                s.volume_pct = pct;
                s.mute = mute;
            }
        }
    }

    /// Master HW percent for status / AdjustHwVolume.
    /// Gesture / status cache first, else live PipeWire (then cache for status TTL).
    fn hw_volume(&mut self) -> (u32, bool) {
        if let Some((pct, mute, at)) = self.hw_vol_cache {
            // Trust optimistic cache through the scroll cool-down — mid-gesture
            // pactl probes can lag / disagree with the notch we just applied and
            // make AdjustHwVolume / waybar feel stuck or snap back.
            let hold = at.elapsed() < HW_VOL_STATUS_TTL || Self::waybar_scroll_marker_hot();
            if hold {
                let pct = pct.min(100);
                self.patch_hw_into_snapshot(pct, mute);
                return (pct, mute);
            }
            self.hw_vol_cache = None;
        }
        let Some(name) = self.hw_sink().map(|s| s.to_string()) else {
            return (0, false);
        };
        let t0 = Instant::now();
        let (pct, mute) = graph::probe_sink_volume_mute(&name)
            .or_else(|| {
                self.snapshot
                    .sinks
                    .iter()
                    .find(|s| s.name == name)
                    .map(|s| (s.volume_pct, s.mute))
            })
            .unwrap_or((0, false));
        lat_trace(&format!(
            "hw_volume probe {}ms",
            t0.elapsed().as_millis()
        ));
        let pct = pct.min(100);
        self.hw_vol_cache = Some((pct, mute, Instant::now()));
        self.patch_hw_into_snapshot(pct, mute);
        (pct, mute)
    }

    /// One scroll notch: ±5 on a 5-grid, snap through 100, hard-capped at 100%.
    fn snap_hw_notch(cur: u32, up: bool) -> u32 {
        let cur = cur.min(100);
        if up {
            if cur >= 100 {
                100
            } else {
                (((cur / 5) + 1) * 5).min(100)
            }
        } else if cur == 0 {
            0
        } else if cur == 100 {
            95
        } else if cur % 5 == 0 {
            cur.saturating_sub(5)
        } else {
            (cur / 5) * 5
        }
    }

    fn patch_hw_volume(&mut self, pct: u32, mute: Option<bool>) {
        let mute = mute.unwrap_or_else(|| {
            // Avoid nested hw_volume() while writing cache — read mute only.
            if let Some((_, m, at)) = self.hw_vol_cache {
                if at.elapsed() < HW_VOL_STATUS_TTL {
                    return m;
                }
            }
            self.hw_sink()
                .and_then(|n| graph::probe_sink_volume_mute(n))
                .map(|(_, m)| m)
                .or_else(|| {
                    let name = self.hw_sink()?;
                    self.snapshot
                        .sinks
                        .iter()
                        .find(|s| s.name == name)
                        .map(|s| s.mute)
                })
                .unwrap_or(false)
        });
        let pct = pct.min(100);
        self.hw_vol_cache = Some((pct, mute, Instant::now()));
        self.patch_hw_into_snapshot(pct, mute);
    }

    fn signal_waybar_hw(&self) {
        // Pill refresh via RTMIN+9 (module uses exec-on-event: false so scroll
        // is not blocked by a second `ctl status`). Throttle burst signals and
        // always schedule a trailing nudge so the final % lands on the pill.
        static LAST_MS: AtomicU64 = AtomicU64::new(0);
        static TRAIL_GEN: AtomicU64 = AtomicU64::new(0);

        fn pkill_waybar_signal() {
            let _ = std::process::Command::new("pkill")
                .args(["-RTMIN+9", "waybar"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let prev = LAST_MS.load(Ordering::Relaxed);
        if now.saturating_sub(prev) >= 45 {
            LAST_MS.store(now, Ordering::Relaxed);
            std::thread::spawn(pkill_waybar_signal);
        }

        let gen = TRAIL_GEN.fetch_add(1, Ordering::Relaxed) + 1;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(70));
            if TRAIL_GEN.load(Ordering::Relaxed) == gen {
                LAST_MS.store(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0),
                    Ordering::Relaxed,
                );
                pkill_waybar_signal();
            }
        });
    }

    /// Apply Master HW volume on the IPC thread (same path as `hw-vol set`).
    fn apply_master_hw_volume(&mut self, pct: u32) -> Result<(), String> {
        let pct = pct.min(100);
        let name = self
            .hw_sink()
            .map(|s| s.to_string())
            .ok_or_else(|| "no Master HW out".to_string())?;
        // Cache + enqueue + return — never block the IPC ACK on pactl (Waybar
        // drops on-scroll forkExecs while ctl waits).
        self.patch_hw_volume(pct, None);
        Self::touch_waybar_scroll_marker();
        enqueue_hw_volume_set(name, pct);
        self.signal_waybar_hw();
        Ok(())
    }

    fn apply_master_hw_mute(&mut self, mute: bool) -> Result<(), String> {
        let name = self
            .hw_sink()
            .map(|s| s.to_string())
            .ok_or_else(|| "no Master HW out".to_string())?;
        let pct = self.hw_volume().0;
        self.patch_hw_volume(pct, Some(mute));
        graph::set_sink_mute(&name, mute).map_err(|e| e.to_string())?;
        self.signal_waybar_hw();
        Ok(())
    }

    fn build_status(&mut self) -> Status {
        let (pct, mute) = self.hw_volume();
        Status {
            ok: true,
            message: self.status_msg.clone(),
            master_hw: self.session.master_output.clone(),
            master_hw_desc: self.session.master_output_desc.clone(),
            hw_volume_pct: pct,
            hw_mute: mute,
            session_slug: self.session.slug.clone(),
            session_name: self.session.name.clone(),
            sample_rate: self.session.performance.sample_rate,
            quantum: self.session.performance.quantum,
        }
    }

    fn handle(&mut self, req: Request) -> Response {
        // UI fader drags land here so get_mixer / QS see live track gains.
        drain_daemon_track_mixer_patches(&mut self.session);
        match req {
            Request::Ping => Response::Ok {
                message: "pong".into(),
                status: Some(self.build_status()),
                sessions: None,
                snapshot: None,
                session: None,
            },
            Request::GetStatus => Response::Ok {
                message: self.status_msg.clone(),
                status: Some(self.build_status()),
                sessions: None,
                snapshot: None,
                session: None,
            },
            Request::PushFxControls { bus, inserts } => {
                // Queue and return immediately — never block knobs on snapshot/session.
                self.cmds.send(Command::PushFxControls { bus, inserts });
                Response::Ok {
                    message: "ok".into(),
                    status: None,
                    sessions: None,
                    snapshot: None,
                    session: None,
                }
            }
            Request::GetSnapshot => {
                // Serve cache — idle refreshes every ~2s off the request mutex.
                // Cold start only: one inline refresh if we have never listed devices.
                if self.snapshot.sinks.is_empty() && self.snapshot.sources.is_empty() {
                    self.snapshot = graph::refresh_snapshot();
                    self.last_snapshot_at = Instant::now();
                }
                Response::Ok {
                    message: "snapshot".into(),
                    status: Some(self.build_status()),
                    sessions: None,
                    snapshot: Some(self.snapshot.clone()),
                    session: Some(self.session.clone()),
                }
            }
            Request::GetMixer => {
                if self.snapshot.sinks.is_empty() && self.snapshot.sources.is_empty() {
                    self.snapshot = graph::refresh_snapshot();
                    self.last_snapshot_at = Instant::now();
                }
                let status = self.build_status();
                let mixer = crate::mixer_api::build_mixer_json(
                    Some(&status),
                    Some(&self.snapshot),
                    Some(&self.session),
                );
                Response::Mixer {
                    mixer,
                    status: Some(status),
                }
            }
            Request::Exec { cmd } => {
                // Only structural graph ops block IPC / return a session for UI adopt.
                // PushFxParams / ApplyLevels / levels must be fire-and-forget (sub-frame).
                let structural = matches!(
                    cmd,
                    Command::ApplySession(_)
                        | Command::ReconnectPipeWire
                        | Command::BindMasterClock(_)
                        | Command::BindDeviceClock { .. }
                        | Command::RewireSessionRoutes(_)
                        | Command::RewireTrackFx { .. }
                        | Command::HotplugSession(_)
                        | Command::HotplugTrack { .. }
                        | Command::EnsureTrack { .. }
                        | Command::VirtualInput { .. }
                        | Command::PruneTrack { .. }
                );
                // Keep daemon session in sync when UI sends a full session.
                match &cmd {
                    Command::ApplySession(s)
                    | Command::BindMasterClock(s)
                    | Command::RewireSessionRoutes(s)
                    | Command::HotplugSession(s)
                    | Command::ApplyLevels(s) => {
                        self.session = s.clone();
                    }
                    Command::BindDeviceClock { session, .. }
                    | Command::RewireTrackFx { session, .. }
                    | Command::HotplugTrack { session, .. }
                    | Command::PushFxParams { session, .. }
                    | Command::EnsureTrack { session, .. }
                    | Command::VirtualInput { session, .. }
                    | Command::PruneTrack { session, .. }
                    | Command::PlaceApp { session, .. } => {
                        self.session = session.clone();
                    }
                    Command::SyncPlayback(s) => {
                        self.session = s.clone();
                    }
                    _ => {}
                }
                self.cmds.send(cmd);
                // Always ACK immediately — never sleep under the daemon mutex.
                // UI adopts SessionApplied / snapshot via events.
                self.drain_events();
                Response::Ok {
                    message: self.status_msg.clone(),
                    status: Some(self.build_status()),
                    sessions: None,
                    snapshot: if structural {
                        Some(self.snapshot.clone())
                    } else {
                        None
                    },
                    session: if structural {
                        Some(self.session.clone())
                    } else {
                        None
                    },
                }
            }
            Request::SetHwVolume { pct } => match self.apply_master_hw_volume(pct) {
                Ok(()) => {
                    crate::mixer_api::touch_mixer_tick();
                    Response::Ok {
                        message: format!("hw vol {}%", pct.min(100)),
                        status: Some(self.build_status()),
                        sessions: None,
                        snapshot: None,
                        session: None,
                    }
                }
                Err(error) => Response::Err { error },
            },
            Request::AdjustHwVolume { delta } => {
                // Always apply. Coalescing made hover-scroll feel stuck (dropped
                // notches). Waybar smooth-scrolling-threshold already thins floods.
                Self::touch_waybar_scroll_marker();
                self.hw_vol_last_adjust = Some(Instant::now());
                let (cur, _) = self.hw_volume();
                // Treat |delta|/5 as snap-aware notches (waybar sends ±5 per wheel tick).
                let notches = match delta {
                    0 => 0,
                    d if d > 0 => {
                        let n = d / 5;
                        if n == 0 {
                            1
                        } else {
                            n
                        }
                    }
                    d => {
                        let n = d / 5; // negative
                        if n == 0 {
                            -1
                        } else {
                            n
                        }
                    }
                };
                // Cap one IPC to 2 notches — parallel forkExec can't jump 50%.
                let notches = notches.clamp(-2, 2);
                let mut pct = cur;
                if notches > 0 {
                    for _ in 0..notches {
                        pct = Self::snap_hw_notch(pct, true);
                    }
                } else {
                    for _ in 0..(-notches) {
                        pct = Self::snap_hw_notch(pct, false);
                    }
                }
                match self.apply_master_hw_volume(pct) {
                    Ok(()) => {
                        crate::mixer_api::touch_mixer_tick();
                        Response::Ok {
                            message: format!("hw vol {pct}%"),
                            status: Some(self.build_status()),
                            sessions: None,
                            snapshot: None,
                            session: None,
                        }
                    }
                    Err(error) => Response::Err { error },
                }
            }
            Request::SetHwMute { mute } => match self.apply_master_hw_mute(mute) {
                Ok(()) => {
                    crate::mixer_api::touch_mixer_tick();
                    Response::Ok {
                        message: "ok".into(),
                        status: Some(self.build_status()),
                        sessions: None,
                        snapshot: None,
                        session: None,
                    }
                }
                Err(error) => Response::Err { error },
            },
            Request::SetSinkInputVolume { index, pct } => {
                self.patch_sink_input(index, Some(pct), None);
                self.cmds
                    .send(Command::SetSinkInputVolume { index, pct });
                crate::mixer_api::touch_mixer_tick();
                Response::Ok {
                    message: "ok".into(),
                    status: None,
                    sessions: None,
                    snapshot: None,
                    session: None,
                }
            }
            Request::SetSinkInputMute { index, mute } => {
                self.patch_sink_input(index, None, Some(mute));
                self.cmds
                    .send(Command::SetSinkInputMute { index, mute });
                crate::mixer_api::touch_mixer_tick();
                Response::Ok {
                    message: "ok".into(),
                    status: None,
                    sessions: None,
                    snapshot: None,
                    session: None,
                }
            }
            Request::MoveSinkInput { index, sink } => {
                self.cmds.send(Command::MoveSinkInput { index, sink });
                crate::mixer_api::touch_mixer_tick();
                Response::Ok {
                    message: "ok".into(),
                    status: None,
                    sessions: None,
                    snapshot: None,
                    session: None,
                }
            }
            Request::SetDefaultSink { name } => {
                self.cmds.send(Command::SetDefaultSink(name.clone()));
                self.session.preferred_default_sink = Some(name);
                let _ = self.session.save();
                Response::Ok {
                    message: "default sink set".into(),
                    status: Some(self.build_status()),
                    sessions: None,
                    snapshot: None,
                    session: Some(self.session.clone()),
                }
            }
            Request::SetDefaultSource { name } => {
                self.cmds.send(Command::SetDefaultSource(name));
                Response::Ok {
                    message: "default source set".into(),
                    status: Some(self.build_status()),
                    sessions: None,
                    snapshot: None,
                    session: None,
                }
            }
            Request::SetMasterHw { name } => {
                let desc = self
                    .snapshot
                    .sinks
                    .iter()
                    .find(|s| s.name == name)
                    .map(|s| s.description.clone())
                    .unwrap_or_else(|| name.clone());
                self.session.master_output = Some(name.clone());
                self.session.master_output_desc = Some(desc.clone());
                crate::audio::graph::remember_desktop_hw(
                    &mut self.session,
                    &name,
                    Some(desc.as_str()),
                );
                self.hw_vol_cache = None;
                let _ = self.session.save();
                // Light relink only — never ApplySession / ForceRespawn.
                self.cmds.send(Command::SetMasterHw {
                    name: name.clone(),
                    desc: Some(desc),
                });
                Response::Ok {
                    message: format!("master hw → {name}"),
                    status: Some(self.build_status()),
                    sessions: None,
                    snapshot: None,
                    session: Some(self.session.clone()),
                }
            }
            Request::SetSinkVolume { name, pct } => {
                // Master HW device row must share the hw-vol path — never a
                // second percent via async worker sink-vol (waybar/status desync).
                if self.is_master_hw_sink(&name) {
                    match self.apply_master_hw_volume(pct) {
                        Ok(()) => {
                            crate::mixer_api::touch_mixer_tick();
                            Response::Ok {
                                message: format!("hw vol {}%", pct.min(100)),
                                status: Some(self.build_status()),
                                sessions: None,
                                snapshot: None,
                                session: None,
                            }
                        }
                        Err(error) => Response::Err { error },
                    }
                } else {
                    self.patch_device_vol(true, &name, Some(pct.min(150)), None);
                    self.cmds.send(Command::SetSinkVolume { name, pct });
                    crate::mixer_api::touch_mixer_tick();
                    Response::Ok {
                        message: "ok".into(),
                        status: None,
                        sessions: None,
                        snapshot: None,
                        session: None,
                    }
                }
            }
            Request::SetSinkMute { name, mute } => {
                if self.is_master_hw_sink(&name) {
                    match self.apply_master_hw_mute(mute) {
                        Ok(()) => {
                            crate::mixer_api::touch_mixer_tick();
                            Response::Ok {
                                message: "ok".into(),
                                status: Some(self.build_status()),
                                sessions: None,
                                snapshot: None,
                                session: None,
                            }
                        }
                        Err(error) => Response::Err { error },
                    }
                } else {
                    self.patch_device_vol(true, &name, None, Some(mute));
                    self.cmds.send(Command::SetSinkMute { name, mute });
                    crate::mixer_api::touch_mixer_tick();
                    Response::Ok {
                        message: "ok".into(),
                        status: None,
                        sessions: None,
                        snapshot: None,
                        session: None,
                    }
                }
            }
            Request::SetSourceVolume { name, pct } => {
                self.patch_device_vol(false, &name, Some(pct), None);
                self.cmds.send(Command::SetSourceVolume { name, pct });
                crate::mixer_api::touch_mixer_tick();
                Response::Ok {
                    message: "ok".into(),
                    status: None,
                    sessions: None,
                    snapshot: None,
                    session: None,
                }
            }
            Request::SetSourceMute { name, mute } => {
                self.patch_device_vol(false, &name, None, Some(mute));
                self.cmds.send(Command::SetSourceMute { name, mute });
                crate::mixer_api::touch_mixer_tick();
                Response::Ok {
                    message: "ok".into(),
                    status: None,
                    sessions: None,
                    snapshot: None,
                    session: None,
                }
            }
            Request::SetTrackMixer {
                track_id,
                gain_db,
                mute,
            } => {
                let Some(track) = self.session.tracks.iter_mut().find(|t| t.id == track_id) else {
                    return Response::Err {
                        error: "track not found".into(),
                    };
                };
                if let Some(g) = gain_db {
                    track.gain_db = g.clamp(-48.0, 12.0);
                }
                if let Some(m) = mute {
                    track.mute = m;
                }
                let sink = track.expected_sink_name();
                let gain = track.gain_db;
                let muted = track.mute;
                let _ = self.session.save();
                // Tray mixer strips read AppState.session — push so faders/dB labels update.
                push_track_mixer_to_ui(track_id, gain, muted);
                let rev = track_mixer_write_rev(track_id);
                self.cmds.send(Command::SetTrackLevel {
                    sink,
                    gain_db: gain,
                    muted,
                    mixer_mute: muted,
                    rev,
                });
                crate::mixer_api::touch_mixer_tick();
                Response::Ok {
                    message: "ok".into(),
                    status: None,
                    sessions: None,
                    snapshot: None,
                    session: None,
                }
            }
            Request::SessionList => {
                let sessions = session::list_sessions()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|m| SessionListItem {
                        slug: m.slug,
                        name: m.name,
                    })
                    .collect();
                Response::Ok {
                    message: "ok".into(),
                    status: Some(self.build_status()),
                    sessions: Some(sessions),
                    snapshot: None,
                    session: None,
                }
            }
            Request::SessionSave => match self.session.save() {
                Ok(()) => Response::Ok {
                    message: format!("saved {}", self.session.slug),
                    status: Some(self.build_status()),
                    sessions: None,
                    snapshot: None,
                    session: None,
                },
                Err(e) => Response::Err {
                    error: format!("{e:#}"),
                },
            },
            Request::SessionSaveAs { name } => {
                let slug = session::store::slugify(&name);
                match session::store::save_session_as(&mut self.session, &slug, Some(&name)) {
                    Ok(()) => Response::Ok {
                        message: format!("saved {slug}"),
                        status: Some(self.build_status()),
                        sessions: None,
                        snapshot: None,
                        session: Some(self.session.clone()),
                    },
                    Err(e) => Response::Err {
                        error: format!("{e:#}"),
                    },
                }
            }
            Request::SessionLoad { slug } => match session::store::load_slug(&slug) {
                Ok(mut s) => {
                    let _ = session::store::write_active_slug(&slug);
                    self.snapshot = graph::refresh_snapshot();
                    let sinks: Vec<_> = self
                        .snapshot
                        .sinks
                        .iter()
                        .map(|x| (x.name.clone(), x.description.clone()))
                        .collect();
                    let sources: Vec<_> = self
                        .snapshot
                        .sources
                        .iter()
                        .map(|x| (x.name.clone(), x.description.clone()))
                        .collect();
                    let report = session::resolve_devices(&mut s, &sinks, &sources);
                    seed_track_mixer_authority_from_session(&s);
                    self.session = s;
                    self.cmds
                        .send(Command::ApplySession(self.session.clone()));
                    // ACK immediately — arm continues on the worker; never sleep here.
                    self.drain_events();
                    Response::Ok {
                        message: if report.messages.is_empty() {
                            format!("loaded {slug}")
                        } else {
                            report.join()
                        },
                        status: Some(self.build_status()),
                        sessions: None,
                        snapshot: Some(self.snapshot.clone()),
                        session: Some(self.session.clone()),
                    }
                }
                Err(e) => Response::Err {
                    error: format!("{e:#}"),
                },
            },
            Request::SessionDelete { slug } => match session::store::delete_slug(&slug) {
                Ok(()) => Response::Ok {
                    message: format!("deleted {slug}"),
                    status: Some(self.build_status()),
                    sessions: None,
                    snapshot: None,
                    session: None,
                },
                Err(e) => Response::Err {
                    error: format!("{e:#}"),
                },
            },
            Request::Apply => {
                self.cmds
                    .send(Command::ApplySession(self.session.clone()));
                Response::Ok {
                    message: "apply queued".into(),
                    status: Some(self.build_status()),
                    sessions: None,
                    snapshot: None,
                    session: None,
                }
            }
            Request::Shutdown => {
                let _ = self.session.save();
                if self.cmds.is_embedded() {
                    request_embedded_quit();
                } else {
                    self.cmds.send(Command::Shutdown);
                }
                Response::Ok {
                    message: "shutting down".into(),
                    status: None,
                    sessions: None,
                    snapshot: None,
                    session: None,
                }
            }
            Request::PopupPlayback => {
                // Never block the IPC mutex on GTK settle / egui fallthrough.
                crate::popup_launch::spawn_mixer_popup_async();
                Response::Ok {
                    message: "popup".into(),
                    status: None,
                    sessions: None,
                    snapshot: None,
                    session: None,
                }
            }
        }
    }
}

fn request_tag(req: &Request) -> &'static str {
    match req {
        Request::Ping => "ping",
        Request::GetStatus => "status",
        Request::AdjustHwVolume { .. } | Request::SetHwVolume { .. } | Request::SetHwMute { .. } => {
            "hw-vol"
        }
        Request::PopupPlayback => "popup",
        Request::GetSnapshot => "snapshot",
        Request::GetMixer => "mixer",
        _ => "other",
    }
}

fn handle_client(state: Arc<Mutex<DaemonState>>, mut stream: UnixStream) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                    let line = buf.drain(..=pos).collect::<Vec<u8>>();
                    let line = String::from_utf8_lossy(&line);
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let resp = match serde_json::from_str::<Request>(line) {
                        Ok(req) => {
                            let shutdown = matches!(req, Request::Shutdown);
                            let tag = request_tag(&req);
                            let t0 = Instant::now();
                            let mut g = state.lock().unwrap();
                            let lock_ms = t0.elapsed().as_millis();
                            let embedded = g.cmds.is_embedded();
                            // Embedded: session truth comes from worker Events only
                            // (never reload disk and overwrite UI-dirty state).
                            g.drain_events();
                            let _ = embedded;
                            let r = g.handle(req);
                            lat_trace(&format!(
                                "ipc {tag} lock_wait={lock_ms}ms hold={}ms",
                                t0.elapsed().as_millis()
                            ));
                            if shutdown && !embedded {
                                drop(g);
                                let _ = ipc::write_line(&mut stream, &r);
                                std::process::exit(0);
                            }
                            r
                        }
                        Err(e) => Response::Err {
                            error: format!("bad request: {e}"),
                        },
                    };
                    if ipc::write_line(&mut stream, &resp).is_err() {
                        return;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
}

fn serve_loop(state: Arc<Mutex<DaemonState>>, listener: std::os::unix::net::UnixListener) {
    // Snapshot refresh must not block accept (multi-pactl).
    let refresh_busy = Arc::new(AtomicBool::new(false));
    loop {
        let refresh = {
            let mut g = state.lock().unwrap();
            g.drain_events();
            drain_daemon_track_mixer_patches(&mut g.session);
            if g.last_idle.elapsed() >= Duration::from_secs(2) {
                g.last_idle = Instant::now();
                true
            } else {
                false
            }
        };
        if refresh && !refresh_busy.swap(true, Ordering::SeqCst) {
            let st = state.clone();
            let busy = refresh_busy.clone();
            thread::Builder::new()
                .name("buschain-observer".into())
                .spawn(move || {
                    let t0 = Instant::now();
                    let snap = graph::refresh_snapshot();
                    graph::publish_observer_snapshot(snap.clone());
                    let ms = t0.elapsed().as_millis();
                    lat_trace(&format!("refresh_snapshot {ms}ms"));
                    let mut g = st.lock().unwrap();
                    g.snapshot = snap;
                    // Keep Master HW optimistic cache aligned with status/waybar/GTK.
                    g.apply_hw_cache_to_snapshot();
                    // Embedded: UI owns the live session (commands update it).
                    g.soft_bind();
                    busy.store(false, Ordering::SeqCst);
                })
                .expect("spawn daemon observer");
        }

        match listener.accept() {
            Ok((stream, _)) => {
                let st = state.clone();
                std::thread::spawn(move || handle_client(st, stream));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                eprintln!("accept: {e}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// Run headless daemon forever (or until Shutdown). Legacy / debug only.
pub fn run() -> Result<()> {
    if ipc::Client::ping() {
        eprintln!("buschain-daemon: already running (tray UI or another daemon)");
        return Ok(());
    }

    let listener = ipc::bind_listener()?;
    let state = Arc::new(Mutex::new(DaemonState::new_headless()));

    eprintln!(
        "buschain-daemon listening on {}",
        ipc::socket_path().display()
    );
    crate::scroll_strip::spawn_scroll_strip_async();
    serve_loop(state, listener);
    #[allow(unreachable_code)]
    Ok(())
}

/// Embed ctl/waybar IPC into the tray UI process (same worker as the UI).
pub fn start_embedded(cmd_tx: Sender<Command>) -> Result<()> {
    if ipc::Client::ping() {
        eprintln!("buschain-control: IPC socket already taken — ctl may hit another process");
        return Ok(());
    }
    let listener = ipc::bind_listener()?;
    let state = Arc::new(Mutex::new(DaemonState::new_embedded(cmd_tx)));
    eprintln!(
        "buschain-control: embedded IPC on {}",
        ipc::socket_path().display()
    );
    std::thread::Builder::new()
        .name("buschain-ipc".into())
        .spawn(move || serve_loop(state, listener))
        .expect("spawn embedded ipc");
    // Master HW hover notches — BusChain-owned layer-shell strip (not Waybar on-scroll).
    crate::scroll_strip::spawn_scroll_strip_async();
    Ok(())
}
