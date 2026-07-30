//! IPC runtime for ctl / waybar / GTK mixer.
//!
//! - Headless: [`run`] owns a local worker (legacy `buschain-daemon`).
//! - Embedded: [`start_embedded`] forwards into the tray UI's in-process worker.

use std::io::Read;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::audio::graph::{self, PwSnapshot};
use crate::audio::worker::{AudioWorker, Command, Event};
use crate::ipc::{self, Request, Response, SessionListItem, Status};
use crate::session::{self, Session};

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
    /// Optimistic Master HW vol/mute so rapid waybar scrolls accumulate.
    hw_vol_cache: Option<(u32, bool, Instant)>,
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
        }
    }

    /// Refresh PipeWire snapshot at most every `ttl` (or when empty).
    fn ensure_snapshot(&mut self, ttl: Duration) {
        self.drain_events();
        let empty = self.snapshot.sinks.is_empty() && self.snapshot.sink_inputs.is_empty();
        if empty || self.last_snapshot_at.elapsed() >= ttl {
            self.snapshot = graph::refresh_snapshot();
            self.last_snapshot_at = Instant::now();
            // Re-apply optimistic HW vol after a fresh list.
            if let Some((pct, mute, at)) = self.hw_vol_cache {
                if at.elapsed() < Duration::from_millis(600) {
                    let name = self.session.master_output.clone();
                    if let Some(name) = name {
                        if let Some(sink) =
                            self.snapshot.sinks.iter_mut().find(|x| x.name == name)
                        {
                            sink.volume_pct = pct;
                            sink.mute = mute;
                        }
                    }
                }
            }
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
                    // Keep optimistic HW vol if a scroll just happened (snapshot may be stale).
                    if let Some((pct, mute, at)) = self.hw_vol_cache {
                        if at.elapsed() < Duration::from_millis(600) {
                            let name = self.session.master_output.clone();
                            if let Some(name) = name {
                                if let Some(sink) =
                                    self.snapshot.sinks.iter_mut().find(|x| x.name == name)
                                {
                                    sink.volume_pct = pct;
                                    sink.mute = mute;
                                }
                            }
                        } else {
                            self.hw_vol_cache = None;
                        }
                    }
                    self.soft_bind();
                }
                Event::Status(s) | Event::Error(s) => self.status_msg = s,
                Event::MidiSnapshot(_) | Event::MidiLearnBound { .. } => {}
                Event::SessionApplied { session, message } => {
                    // Worker applied session is authoritative (tracks + inserts + sink names).
                    self.session = session;
                    self.status_msg = message;
                }
            }
        }
    }

    fn hw_sink(&self) -> Option<&str> {
        self.session.master_output.as_deref()
    }

    fn hw_volume(&self) -> (u32, bool) {
        // Keep optimistic cache long enough that waybar scroll bursts + snapshot
        // refresh cannot snap the percent back mid-gesture.
        if let Some((pct, mute, at)) = self.hw_vol_cache {
            if at.elapsed() < Duration::from_millis(1800) {
                return (pct, mute);
            }
        }
        let Some(name) = self.hw_sink() else {
            return (0, false);
        };
        self.snapshot
            .sinks
            .iter()
            .find(|s| s.name == name)
            .map(|s| (s.volume_pct, s.mute))
            .unwrap_or((0, false))
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
        let mute = mute.unwrap_or_else(|| self.hw_volume().1);
        let pct = pct.min(100);
        self.hw_vol_cache = Some((pct, mute, Instant::now()));
        let name = self.session.master_output.clone();
        if let Some(name) = name {
            if let Some(s) = self.snapshot.sinks.iter_mut().find(|s| s.name == name) {
                s.volume_pct = pct;
                s.mute = mute;
            }
        }
    }

    fn build_status(&self) -> Status {
        let (pct, mute) = self.hw_volume();
        // Never advertise HW boost in status/waybar/QS labels.
        let pct = pct.min(100);
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
            Request::Exec { cmd } => {
                // Only structural graph ops block IPC / return a session for UI adopt.
                // PushFxParams / ApplyLevels / levels must be fire-and-forget (sub-frame).
                let structural = matches!(
                    cmd,
                    Command::ApplySession(_)
                        | Command::BindMasterClock(_)
                        | Command::BindDeviceClock { .. }
                        | Command::RewireSessionRoutes(_)
                        | Command::RewireTrackFx { .. }
                        | Command::HotplugSession(_)
                        | Command::HotplugTrack { .. }
                        | Command::EnsureTrack { .. }
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
                    | Command::PruneTrack { session, .. } => {
                        self.session = session.clone();
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
            Request::SetHwVolume { pct } => {
                let pct = pct.min(100);
                if let Some(name) = self.hw_sink().map(|s| s.to_string()) {
                    self.patch_hw_volume(pct, None);
                    // Apply on the IPC thread (pavucontrol-snappy). Skip worker queue.
                    let _ = graph::set_sink_volume(&name, pct);
                    Response::Ok {
                        message: format!("hw vol {pct}%"),
                        status: Some(self.build_status()),
                        sessions: None,
                        snapshot: None,
                        session: None,
                    }
                } else {
                    Response::Err {
                        error: "no Master HW out".into(),
                    }
                }
            }
            Request::AdjustHwVolume { delta } => {
                let (cur, _) = self.hw_volume();
                // Treat |delta|/5 as snap-aware notches (waybar sends ±5 per wheel tick).
                // Remainder still counts as one notch so fine deltas aren't dropped.
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
                if let Some(name) = self.hw_sink().map(|s| s.to_string()) {
                    self.patch_hw_volume(pct, None);
                    let _ = graph::set_sink_volume(&name, pct);
                    Response::Ok {
                        message: format!("hw vol {pct}%"),
                        status: Some(self.build_status()),
                        sessions: None,
                        snapshot: None,
                        session: None,
                    }
                } else {
                    Response::Err {
                        error: "no Master HW out".into(),
                    }
                }
            }
            Request::SetHwMute { mute } => {
                let name = self.session.master_output.clone();
                if let Some(name) = name {
                    let pct = self.hw_volume().0;
                    self.patch_hw_volume(pct, Some(mute));
                    let _ = graph::set_sink_mute(&name, mute);
                    Response::Ok {
                        message: "ok".into(),
                        status: Some(self.build_status()),
                        sessions: None,
                        snapshot: None,
                        session: None,
                    }
                } else {
                    Response::Err {
                        error: "no Master HW out".into(),
                    }
                }
            }
            Request::SetSinkInputVolume { index, pct } => {
                self.patch_sink_input(index, Some(pct), None);
                self.cmds
                    .send(Command::SetSinkInputVolume { index, pct });
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
                self.patch_device_vol(true, &name, Some(pct), None);
                self.cmds.send(Command::SetSinkVolume { name, pct });
                Response::Ok {
                    message: "ok".into(),
                    status: None,
                    sessions: None,
                    snapshot: None,
                    session: None,
                }
            }
            Request::SetSinkMute { name, mute } => {
                self.patch_device_vol(true, &name, None, Some(mute));
                self.cmds.send(Command::SetSinkMute { name, mute });
                Response::Ok {
                    message: "ok".into(),
                    status: None,
                    sessions: None,
                    snapshot: None,
                    session: None,
                }
            }
            Request::SetSourceVolume { name, pct } => {
                self.patch_device_vol(false, &name, Some(pct), None);
                self.cmds.send(Command::SetSourceVolume { name, pct });
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
                self.cmds.send(Command::SetTrackLevel {
                    sink,
                    gain_db: gain,
                    muted,
                });
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
                crate::popup_launch::spawn_mixer_popup();
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
                            let mut g = state.lock().unwrap();
                            let embedded = g.cmds.is_embedded();
                            // Keep ctl session fresh with UI disk saves.
                            if embedded {
                                g.session = Session::load();
                            }
                            g.drain_events();
                            let r = g.handle(req);
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
    loop {
        // Never hold the mutex across pactl — that stalled call_fast knobs.
        let refresh = {
            let mut g = state.lock().unwrap();
            g.drain_events();
            if g.last_idle.elapsed() >= Duration::from_secs(2) {
                g.last_idle = Instant::now();
                true
            } else {
                false
            }
        };
        if refresh {
            let snap = graph::refresh_snapshot();
            let mut g = state.lock().unwrap();
            g.snapshot = snap;
            // Embedded: UI owns the live session (commands update it). Reloading
            // disk every tick fought unsaved edits and lagged behind toggles.
            g.soft_bind();
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
    Ok(())
}
