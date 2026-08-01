//! Background PipeWire worker — never block the UI thread.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::audio::filter_chain::FilterChainRuntime;
use crate::audio::graph::{self, PwSnapshot};
use crate::session::Session;

/// Structural FX ensure runs off the Props/levels thread (class A HOL kill).
struct FxEnsureJob {
    session: Session,
    track_id: uuid::Uuid,
    queued_at: Instant,
}

struct FxEnsureDone {
    session: Session,
    track_id: uuid::Uuid,
    queued_at: Instant,
    result: Result<String, String>,
}

/// After structural ensure: take rack order/membership from the job snapshot,
/// overlay fresher mix/bypass/params already patched into `last_session`.
fn merge_ensure_track(
    last_session: &mut Option<Session>,
    done: &Session,
    track_id: uuid::Uuid,
) {
    let Some(from_t) = done.tracks.iter().find(|t| t.id == track_id) else {
        return;
    };
    let Some(into) = last_session.as_mut() else {
        *last_session = Some(done.clone());
        return;
    };
    let fresher: HashMap<uuid::Uuid, crate::audio::plugin::PluginRef> = into
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .map(|t| t.inserts.iter().map(|p| (p.slot_id, p.clone())).collect())
        .unwrap_or_default();
    if let Some(into_t) = into.tracks.iter_mut().find(|t| t.id == track_id) {
        into_t.inserts = from_t
            .inserts
            .iter()
            .map(|p| {
                if let Some(old) = fresher.get(&p.slot_id) {
                    let mut m = p.clone();
                    m.params = old.params.clone();
                    m.mix = old.mix;
                    m.bypass = old.bypass;
                    m
                } else {
                    p.clone()
                }
            })
            .collect();
        if from_t.sink_name.is_some() {
            into_t.sink_name = from_t.sink_name.clone();
        }
    } else {
        into.tracks.push(from_t.clone());
    }
}

fn spawn_fx_ensure_thread(job_rx: Receiver<FxEnsureJob>, done_tx: Sender<FxEnsureDone>) {
    thread::Builder::new()
        .name("buschain-fx-ensure".into())
        .spawn(move || {
            let mut stub = FilterChainRuntime::new();
            while let Ok(first) = job_rx.recv() {
                // Coalesce: latest job per track wins (rapid reorder).
                let mut latest: HashMap<uuid::Uuid, FxEnsureJob> = HashMap::new();
                latest.insert(first.track_id, first);
                while let Ok(more) = job_rx.try_recv() {
                    latest.insert(more.track_id, more);
                }
                for (_, mut job) in latest {
                    let age = job.queued_at.elapsed().as_millis();
                    if age > 2000 {
                        buschain_engine::fx_trace::log(
                            "FxEnsureQueuedAge",
                            &format!("track={} WARN", job.track_id),
                            age,
                        );
                    }
                    let span = buschain_engine::fx_trace::span("FxEnsureThread");
                    crate::audio::engine_handle::sync_profile(&job.session.performance);
                    let result = match graph::rewire_track_fx(
                        &mut job.session,
                        &mut stub,
                        job.track_id,
                    ) {
                        Ok(message) => Ok(message),
                        Err(e) => Err(format!("{e:#}")),
                    };
                    span.end(format!(
                        "track={} ok={}",
                        job.track_id,
                        result.is_ok()
                    ));
                    let _ = done_tx.send(FxEnsureDone {
                        session: job.session,
                        track_id: job.track_id,
                        queued_at: job.queued_at,
                        result,
                    });
                }
            }
        })
        .expect("spawn buschain-fx-ensure");
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Command {
    Refresh,
    /// Emergency: unload all BusChain modules (fixes stacked-loopback crackling).
    Teardown,
    /// Bind GraphClock from session.performance, then rewire routes.
    BindMasterClock(Session),
    /// Per-device clock Apply (Output/Input panels). When bind_buschain, also ClockBind.
    BindDeviceClock {
        session: Session,
        device: String,
        sample_rate: u32,
        quantum: u32,
        soft_quantum: bool,
        bind_buschain: bool,
    },
    /// Idle reconcile / cold bring-up (LiveChange::Reconcile). Never moves streams.
    ApplySession(Session),
    /// Link-only route rewire for every track (never respawn FX).
    RewireSessionRoutes(Session),
    /// Per-insert slot sync for one track (add/remove/reorder).
    RewireTrackFx { session: Session, track_id: uuid::Uuid },
    /// Legacy aliases — same as RewireSessionRoutes / RewireTrackFx.
    HotplugSession(Session),
    HotplugTrack { session: Session, track_id: uuid::Uuid },
    /// Live knobs / mix / insert power — Props on slots, never restarts FX.
    /// Prefer [`PushFxControls`] (no full session clone over IPC).
    PushFxParams { session: Session, track_id: uuid::Uuid },
    /// Fast live params — bus + insert controls only (instant knob path).
    PushFxControls {
        bus: String,
        inserts: Vec<buschain_engine::InsertSlot>,
    },
    /// Bring one new track bus online (no other-track FX reload).
    EnsureTrack { session: Session, track_id: uuid::Uuid },
    /// Create / tear down system virtual input for one track.
    VirtualInput {
        session: Session,
        track_id: uuid::Uuid,
    },
    /// Tear down a removed track's bus only.
    PruneTrack {
        session: Session,
        removed_bus: String,
    },
    /// Volume/mute only — no module load.
    ApplyLevels(Session),
    /// Fast fader path — single bus, usually one pactl call.
    SetTrackLevel {
        sink: String,
        gain_db: f32,
        muted: bool,
    },
    SetSinkVolume { name: String, pct: u32 },
    SetSinkMute { name: String, mute: bool },
    SetSourceVolume { name: String, pct: u32 },
    SetSourceMute { name: String, mute: bool },
    SetSinkInputVolume { index: u32, pct: u32 },
    SetSinkInputMute { index: u32, mute: bool },
    MoveSinkInput { index: u32, sink: String },
    /// Fresh-list place: move every stream matching app_key onto sink (not snapshot-stale).
    /// `session` syncs pin truth into `last_session` without ApplyLevels HOL.
    PlaceApp {
        session: Session,
        app_key: String,
        sink: String,
    },
    /// One-shot enforce all session playback pins (`Intent::SyncPlayback`).
    SyncPlayback(Session),
    /// In-rack capture delta (add / mute-row / remove) — Class B, never Route.
    SyncCaptureDelta {
        session: Session,
        track_id: uuid::Uuid,
    },
    SetDefaultSink(String),
    SetDefaultSource(String),
    /// Light Master HW switch — relink only, never ApplySession / ForceRespawn.
    SetMasterHw {
        name: String,
        desc: Option<String>,
    },
    /// Update `device.description` on an existing bus (track rename).
    SetBusDescription { name: String, description: String },
    /// Re-scan PipeWire MIDI nodes.
    RefreshMidi,
    /// Apply session MIDI maps/routes to the engine runtime.
    ApplyMidiConfig(Session),
    /// Engine MIDI intent (list/connect/map/learn — never aconnect).
    Midi(buschain_engine::MidiIntent),
    Shutdown,
}

/// Why a session snapshot was pushed to the UI (typed — no string matching).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAppliedKind {
    Full,
    Ensure,
    FxRewire,
    Route,
    Clock,
    Levels,
    Other,
}

pub enum Event {
    Snapshot(PwSnapshot),
    /// Light PlaceApp refresh — merge sink-inputs only (keep sinks/sources).
    SinkInputs(Vec<crate::audio::graph::StreamNode>),
    Status(String),
    SessionApplied {
        session: Session,
        message: String,
        kind: SessionAppliedKind,
    },
    /// Host FX gen-swap finished for a track bus.
    FxReady {
        track_id: uuid::Uuid,
        bus: String,
    },
    Error(String),
    MidiSnapshot(buschain_engine::MidiSnapshot),
    MidiLearnBound { map: buschain_engine::MidiCcMap },
}

pub struct AudioWorker {
    tx: Sender<Command>,
    rx: Receiver<Event>,
    /// True when commands are forwarded to `buschain-daemon` (no local PW supervisor).
    pub via_daemon: bool,
}

fn env_flag(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(
            v.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

fn arg_flag(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

impl AudioWorker {
    /// Graph ownership (monolithic in-process model):
    /// - **default** → always local in-process worker (instant faders)
    /// - `--daemon-client` / `BUSCHAIN_CONTROL_USE_DAEMON` → thin client (debug only)
    ///
    /// Never auto-attach when a leftover socket answers — that made production
    /// knobs multi-second while `./scripts/dev --inprocess` stayed snappy.
    pub fn spawn() -> Self {
        let force_daemon = env_flag("BUSCHAIN_CONTROL_USE_DAEMON") || arg_flag("--daemon-client");
        if force_daemon {
            let _ = crate::ipc::ensure_daemon();
            if let Some(w) = Self::connect_daemon() {
                eprintln!("buschain-control: connected to daemon (--daemon-client)");
                return w;
            }
            eprintln!("buschain-control: daemon unavailable — falling back in-process");
        }
        eprintln!("buschain-control: in-process audio worker");
        Self::spawn_local()
    }

    pub fn spawn_local() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
        let (ev_tx, ev_rx) = mpsc::channel::<Event>();
        let (int_tx, int_rx) = mpsc::channel::<Command>();
        let (sup_tx, sup_rx) = mpsc::channel::<Command>();
        let (obs_tx, obs_rx) = mpsc::channel::<Command>();
        let shared_session: Arc<Mutex<SharedSessionSlot>> =
            Arc::new(Mutex::new(SharedSessionSlot::new()));

        let int_session = Arc::clone(&shared_session);
        let sup_session = Arc::clone(&shared_session);
        let ev_int = ev_tx.clone();
        let ev_sup = ev_tx.clone();
        let ev_obs = ev_tx;

        thread::Builder::new()
            .name("buschain-router".into())
            .spawn(move || control_plane_router(cmd_rx, int_tx, sup_tx, obs_tx))
            .expect("spawn control-plane router");

        thread::Builder::new()
            .name("buschain-interactive".into())
            .spawn(move || interactive_loop(int_rx, ev_int, int_session))
            .expect("spawn interactive lane");

        thread::Builder::new()
            .name("buschain-supervisor".into())
            .spawn(move || supervisor_loop(sup_rx, ev_sup, sup_session))
            .expect("spawn supervisor lane");

        thread::Builder::new()
            .name("buschain-observer".into())
            .spawn(move || observer_loop(obs_rx, ev_obs))
            .expect("spawn observer lane");

        eprintln!("buschain-control: dual-thread control plane (interactive/supervisor/observer)");

        Self {
            tx: cmd_tx,
            rx: ev_rx,
            via_daemon: false,
        }
    }

    /// Clone the command sender for an embedded IPC server (ctl / waybar / GTK mixer).
    pub fn clone_sender(&self) -> Sender<Command> {
        self.tx.clone()
    }

    fn connect_daemon() -> Option<Self> {
        if !crate::ipc::Client::ping() {
            return None;
        }
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
        let (ev_tx, ev_rx) = mpsc::channel::<Event>();
        thread::Builder::new()
            .name("buschain-daemon-client".into())
            .spawn(move || daemon_client_loop(cmd_rx, ev_tx))
            .ok()?;
        Some(Self {
            tx: cmd_tx,
            rx: ev_rx,
            via_daemon: true,
        })
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    pub fn poll(&self) -> Vec<Event> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(e) => out.push(e),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }
}

fn daemon_client_loop(cmd_rx: Receiver<Command>, ev_tx: Sender<Event>) {
    use crate::ipc::{Client, Request, Response};
    use std::sync::mpsc::RecvTimeoutError;

    // Initial snapshot + session — UI must adopt daemon inserts/tracks.
    if let Ok(Response::Ok {
        snapshot: Some(s),
        message,
        status,
        session,
        ..
    }) = Client::call(&Request::GetSnapshot)
    {
        let _ = ev_tx.send(Event::Snapshot(s));
        if let Some(sess) = session {
            let _ = ev_tx.send(Event::SessionApplied {
                session: sess,
                message: if message.is_empty() {
                    "adopted daemon session".into()
                } else {
                    message
                },
                kind: SessionAppliedKind::Full,
            });
        } else if let Some(st) = status {
            let _ = ev_tx.send(Event::Status(st.message));
        } else if !message.is_empty() {
            let _ = ev_tx.send(Event::Status(message));
        }
    }

    // Snapshot poll is intentionally slow — daemon caches anyway; avoid IPC storms.
    let mut idle_ticks: u32 = 0;
    loop {
        match cmd_rx.recv_timeout(Duration::from_millis(400)) {
            Ok(Command::Shutdown) => {
                // Leave the daemon running — UI exit must not tear the graph.
                break;
            }
            Ok(cmd) => {
                // Fast path: never serialize a full Session for knob/mix/power.
                let req = match cmd {
                    Command::PushFxControls { bus, inserts } => {
                        Request::PushFxControls { bus, inserts }
                    }
                    Command::PushFxParams { session, track_id } => {
                        match crate::audio::insert_map::ladspa_slots_for(
                            &session, track_id,
                        ) {
                            Some((bus, inserts)) => Request::PushFxControls { bus, inserts },
                            None => Request::Exec {
                                cmd: Command::PushFxParams { session, track_id },
                            },
                        }
                    }
                    other => Request::Exec { cmd: other },
                };
                let is_fx = matches!(req, Request::PushFxControls { .. });
                let result = if is_fx {
                    match Client::call_fast(&req) {
                        Ok(r) => Ok(r),
                        // One retry — never silent-drop Props on a brief IPC stall.
                        Err(_) => Client::call_fast(&req),
                    }
                } else {
                    Client::call(&req)
                };
                match result {
                    Ok(Response::Ok {
                        message,
                        session,
                        snapshot,
                        status,
                        ..
                    }) => {
                        if let Some(s) = snapshot {
                            let _ = ev_tx.send(Event::Snapshot(s));
                        }
                        if let Some(sess) = session {
                            let _ = ev_tx.send(Event::SessionApplied {
                                session: sess,
                                message: message.clone(),
                                kind: SessionAppliedKind::Other,
                            });
                        } else if let Some(st) = status {
                            let _ = ev_tx.send(Event::Status(st.message));
                        } else if !message.is_empty() {
                            let _ = ev_tx.send(Event::Status(message));
                        }
                    }
                    Ok(Response::Mixer { .. }) => {}
                    Ok(Response::Err { error }) => {
                        let _ = ev_tx.send(Event::Error(error));
                    }
                    Err(e) => {
                        if is_fx {
                            let _ = ev_tx.send(Event::Status(format!(
                                "Live params skipped (daemon: {e:#})"
                            )));
                        } else {
                            let _ = ev_tx.send(Event::Error(format!("daemon: {e:#}")));
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                idle_ticks = idle_ticks.wrapping_add(1);
                // ~every 2s: light status; full snapshot every other tick (~4s).
                if idle_ticks % 5 == 0 {
                    if idle_ticks % 10 == 0 {
                        if let Ok(Response::Ok {
                            snapshot: Some(s), ..
                        }) = Client::call(&Request::GetSnapshot)
                        {
                            let _ = ev_tx.send(Event::Snapshot(s));
                        }
                    } else if let Ok(Response::Ok {
                        status: Some(st), ..
                    }) = Client::call(&Request::GetStatus)
                    {
                        let _ = ev_tx.send(Event::Status(st.message));
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Command lane for the dual-thread control plane.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lane {
    Interactive,
    Supervisor,
    Observer,
}

fn command_lane(cmd: &Command) -> Lane {
    match cmd {
        Command::Refresh => Lane::Observer,
        Command::Shutdown
        | Command::Teardown
        | Command::BindMasterClock(_)
        | Command::BindDeviceClock { .. }
        | Command::ApplySession(_)
        | Command::RewireSessionRoutes(_)
        | Command::HotplugSession(_)
        | Command::RewireTrackFx { .. }
        | Command::HotplugTrack { .. }
        | Command::EnsureTrack { .. }
        | Command::VirtualInput { .. }
        | Command::PruneTrack { .. } => Lane::Supervisor,
        _ => Lane::Interactive,
    }
}

fn control_plane_router(
    cmd_rx: Receiver<Command>,
    int_tx: Sender<Command>,
    sup_tx: Sender<Command>,
    obs_tx: Sender<Command>,
) {
    while let Ok(cmd) = cmd_rx.recv() {
        if matches!(cmd, Command::Shutdown) {
            let _ = int_tx.send(Command::Shutdown);
            let _ = sup_tx.send(Command::Shutdown);
            let _ = obs_tx.send(Command::Shutdown);
            break;
        }
        match command_lane(&cmd) {
            Lane::Interactive => {
                let _ = int_tx.send(cmd);
            }
            Lane::Supervisor => {
                let _ = sup_tx.send(cmd);
            }
            Lane::Observer => {
                let _ = obs_tx.send(cmd);
            }
        }
    }
}

fn observer_loop(rx: Receiver<Command>, tx: Sender<Event>) {
    // Topology observation only — never mutates the graph / ENGINE mute path.
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Command::Shutdown => break,
            Command::Refresh => {
                let t0 = Instant::now();
                let snap = graph::refresh_snapshot();
                graph::publish_observer_snapshot(snap.clone());
                let ms = t0.elapsed().as_millis();
                if matches!(
                    std::env::var("BUSCHAIN_CONTROL_LAT_TRACE").as_deref(),
                    Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
                ) {
                    eprintln!("[lat] C observer_snapshot {ms}ms");
                }
                let _ = tx.send(Event::Snapshot(snap));
            }
            _ => {}
        }
    }
}

/// Collapse queued updates into Class A → B → C lanes.
/// Class A (mute/fader/Props) always drains before Capture/PlaceApp and Route.
fn coalesce_commands(mut cmds: Vec<Command>) -> Vec<Command> {
    if cmds.iter().any(|c| matches!(c, Command::Shutdown)) {
        return vec![Command::Shutdown];
    }

    let mut levels: HashMap<String, (f32, bool)> = HashMap::new();
    let mut full_levels: Option<Session> = None;
    let mut fx_params: HashMap<uuid::Uuid, Session> = HashMap::new();
    let mut fx_controls: HashMap<String, Vec<buschain_engine::InsertSlot>> = HashMap::new();
    let mut capture: HashMap<uuid::Uuid, Session> = HashMap::new();
    // Latest RewireTrackFx per track — rapid reorder must not stack ForceRespawns.
    let mut rewire_fx: HashMap<uuid::Uuid, Session> = HashMap::new();
    let mut session_routes: Option<Session> = None;
    let mut class_a = Vec::new();
    let mut class_b = Vec::new();
    let mut ensure = Vec::new();
    let mut class_c = Vec::new();

    for cmd in cmds.drain(..) {
        match cmd {
            Command::SetTrackLevel {
                sink,
                gain_db,
                muted,
            } => {
                full_levels = None;
                levels.insert(sink, (gain_db, muted));
            }
            Command::ApplyLevels(s) => {
                levels.clear();
                full_levels = Some(s);
            }
            Command::PushFxParams { session, track_id } => {
                fx_params.insert(track_id, session);
            }
            Command::PushFxControls { bus, inserts } => {
                fx_controls.insert(bus, inserts);
            }
            Command::SyncCaptureDelta { session, track_id } => {
                capture.insert(track_id, session);
            }
            m @ (Command::EnsureTrack { .. }
                | Command::VirtualInput { .. }
                | Command::PruneTrack { .. }) => {
                ensure.push(m);
            }
            Command::PlaceApp {
                session,
                app_key,
                sink,
            } => {
                class_b.retain(|c| {
                    !matches!(c, Command::PlaceApp { app_key: k, .. } if k == &app_key)
                });
                class_b.push(Command::PlaceApp {
                    session,
                    app_key,
                    sink,
                });
            }
            Command::SyncPlayback(s) => {
                class_b.retain(|c| !matches!(c, Command::SyncPlayback(_)));
                class_b.push(Command::SyncPlayback(s));
            }
            m @ (Command::MoveSinkInput { .. }
            | Command::SetDefaultSink(_)
            | Command::SetDefaultSource(_)
            | Command::SetMasterHw { .. }
            | Command::SetBusDescription { .. }
            | Command::SetSinkMute { .. }
            | Command::SetSinkVolume { .. }
            | Command::SetSourceMute { .. }
            | Command::SetSourceVolume { .. }
            | Command::SetSinkInputMute { .. }
            | Command::SetSinkInputVolume { .. }
            | Command::Refresh
            | Command::RefreshMidi
            | Command::Midi(_)) => {
                class_a.push(m);
            }
            Command::RewireSessionRoutes(s) | Command::HotplugSession(s) => {
                session_routes = Some(s);
            }
            Command::RewireTrackFx {
                session,
                track_id,
            }
            | Command::HotplugTrack {
                session,
                track_id,
            } => {
                rewire_fx.insert(track_id, session);
            }
            other => class_c.push(other),
        }
    }

    if let Some(s) = full_levels {
        class_a.push(Command::ApplyLevels(s));
    } else {
        for (sink, (gain_db, muted)) in levels {
            class_a.push(Command::SetTrackLevel {
                sink,
                gain_db,
                muted,
            });
        }
    }
    for (bus, inserts) in fx_controls {
        class_a.push(Command::PushFxControls { bus, inserts });
    }
    for (track_id, session) in fx_params {
        if let Some((bus, inserts)) =
            crate::audio::insert_map::ladspa_slots_for(&session, track_id)
        {
            if !class_a.iter().any(|c| {
                matches!(c, Command::PushFxControls { bus: b, .. } if b == &bus)
            }) {
                class_a.push(Command::PushFxControls { bus, inserts });
            }
        } else {
            class_a.push(Command::PushFxParams { session, track_id });
        }
    }
    for (track_id, session) in capture {
        class_b.insert(0, Command::SyncCaptureDelta { session, track_id });
    }
    // Buses / prune before PlaceApp (cold bus → place fail).
    let mut out = class_a;
    out.append(&mut ensure);
    out.append(&mut class_b);
    for (track_id, session) in rewire_fx {
        class_c.push(Command::RewireTrackFx { session, track_id });
    }
    if let Some(s) = session_routes {
        class_c.push(Command::RewireSessionRoutes(s));
    }
    crate::audio::adaptive::AdaptivePolicy::global().set_queue_depth(out.len() + class_c.len());
    out.append(&mut class_c);
    out
}

fn sync_engine_clock(session: &Session) {
    crate::audio::engine_handle::sync_profile(&session.performance);
}

/// Quiet Props retry after rebuild/topology defer (no UI status spam).
struct PropsRetry {
    inserts: Vec<buschain_engine::InsertSlot>,
    after: Instant,
    attempts: u8,
}

fn flush_props_retries(props_retry: &mut HashMap<String, PropsRetry>) {
    let due: Vec<String> = props_retry
        .iter()
        .filter(|(_, r)| Instant::now() >= r.after)
        .map(|(b, _)| b.clone())
        .collect();
    for bus in due {
        let Some(retry) = props_retry.remove(&bus) else {
            continue;
        };
        match crate::audio::insert_map::push_bus_controls(&bus, retry.inserts.clone()) {
            Ok(_) => {}
            Err(e) => {
                let msg = format!("{e:#}");
                let deferred = msg.contains("rebuilding")
                    || msg.contains("props deferred")
                    || msg.contains("not found");
                if deferred && retry.attempts < 8 {
                    props_retry.insert(
                        bus,
                        PropsRetry {
                            inserts: retry.inserts,
                            after: Instant::now()
                                + Duration::from_millis(120 + u64::from(retry.attempts) * 40),
                            attempts: retry.attempts + 1,
                        },
                    );
                }
            }
        }
    }
}

fn schedule_props_retry(
    props_retry: &mut HashMap<String, PropsRetry>,
    bus: String,
    inserts: Vec<buschain_engine::InsertSlot>,
) {
    let attempts = props_retry.get(&bus).map(|r| r.attempts).unwrap_or(0);
    if attempts >= 8 {
        return;
    }
    props_retry.insert(
        bus,
        PropsRetry {
            inserts,
            after: Instant::now() + Duration::from_millis(120 + u64::from(attempts) * 40),
            attempts: attempts + 1,
        },
    );
}

/// Deferred PlaceApp — bus may still be spawning; never block the command loop.
struct PlaceRetry {
    sink: String,
    after: Instant,
    attempts: u8,
    last_err: String,
}

fn flush_place_retries(
    place_retry: &mut HashMap<String, PlaceRetry>,
    tx: &Sender<Event>,
) {
    let due: Vec<String> = place_retry
        .iter()
        .filter(|(_, r)| Instant::now() >= r.after)
        .map(|(k, _)| k.clone())
        .collect();
    for app_key in due {
        let Some(retry) = place_retry.remove(&app_key) else {
            continue;
        };
        match graph::place_app_on_sink(&app_key, &retry.sink) {
            Ok(n) => {
                let _ = tx.send(Event::Status(format!(
                    "Placed {n} stream(s) → {}",
                    retry.sink
                )));
                if n > 0 {
                    if let Ok(inputs) = graph::refresh_sink_inputs_only() {
                        let _ = tx.send(Event::SinkInputs(inputs));
                    }
                }
            }
            Err(e) if retry.attempts < 6 => {
                place_retry.insert(
                    app_key,
                    PlaceRetry {
                        sink: retry.sink,
                        after: Instant::now()
                            + Duration::from_millis(80 + u64::from(retry.attempts) * 40),
                        attempts: retry.attempts + 1,
                        last_err: format!("{e:#}"),
                    },
                );
            }
            Err(e) => {
                let _ = tx.send(Event::Error(format!(
                    "place app: {} (retry: {e:#})",
                    retry.last_err
                )));
            }
        }
    }
}

/// Cross-lane session mirror with monotonic generation so Interactive cannot
/// clobber a newer Supervisor Apply (preferred default / virtual_output).
struct SharedSessionSlot {
    session: Option<Session>,
    gen: u64,
}

impl SharedSessionSlot {
    fn new() -> Self {
        Self {
            session: None,
            gen: 0,
        }
    }
}

/// Non-authoritative publish: write session only when `local_gen` is not behind
/// the shared slot (never overwrite a newer Supervisor Apply).
fn publish_shared_session(
    shared: &Arc<Mutex<SharedSessionSlot>>,
    session: &Option<Session>,
    local_gen: u64,
) {
    let Ok(mut g) = shared.lock() else {
        return;
    };
    if g.gen > local_gen {
        return;
    }
    g.session = session.clone();
}

/// Authoritative publish (Full Apply / SetDefaultSink preferred): bump gen.
fn publish_shared_session_authority(
    shared: &Arc<Mutex<SharedSessionSlot>>,
    session: &Option<Session>,
    local_gen: &mut u64,
) {
    let Ok(mut g) = shared.lock() else {
        return;
    };
    let next = g.gen.saturating_add(1).max(local_gen.saturating_add(1));
    g.gen = next;
    g.session = session.clone();
    *local_gen = next;
}

/// Adopt shared session when it is newer than `local_gen`.
fn pull_shared_session_if_newer(
    shared: &Arc<Mutex<SharedSessionSlot>>,
    last_session: &mut Option<Session>,
    local_gen: &mut u64,
) {
    let Ok(g) = shared.lock() else {
        return;
    };
    if g.gen > *local_gen {
        *last_session = g.session.clone();
        *local_gen = g.gen;
    }
}

fn interactive_loop(
    rx: Receiver<Command>,
    tx: Sender<Event>,
    shared_session: Arc<Mutex<SharedSessionSlot>>,
) {
    // Interactive: Class A/B only (mute/fader/Props/Capture/PlaceApp).
    let mut fx = FilterChainRuntime::new();
    let mut last_session: Option<Session> = None;
    let mut session_gen: u64 = 0;
    let mut place_retry: HashMap<String, PlaceRetry> = HashMap::new();
    let mut default_reclaim_until: Option<Instant> = None;
    let midi_session: Arc<Mutex<Option<Session>>> = Arc::new(Mutex::new(None));
    let ms_sink = Arc::clone(&midi_session);
    let ms_bus = Arc::clone(&midi_session);
    let track_sink: Arc<dyn Fn(uuid::Uuid) -> Option<String> + Send + Sync> =
        Arc::new(move |id| {
            let guard = ms_sink.lock().ok()?;
            let session = guard.as_ref()?;
            if id.is_nil() {
                return session
                    .master_id()
                    .and_then(|mid| session.tracks.iter().find(|t| t.id == mid))
                    .map(|t| t.expected_sink_name());
            }
            session
                .tracks
                .iter()
                .find(|t| t.id == id)
                .map(|t| t.expected_sink_name())
        });
    let track_bus: Arc<dyn Fn(uuid::Uuid) -> Option<String> + Send + Sync> =
        Arc::new(move |id| {
            let guard = ms_bus.lock().ok()?;
            let session = guard.as_ref()?;
            session
                .tracks
                .iter()
                .find(|t| t.id == id)
                .map(|t| t.expected_sink_name())
        });
    buschain_engine::midi::ensure_runtime(track_sink, track_bus);
    // Buses currently mixer-muted — volume drags skip the heavy gate path.
    let mut muted_buses: HashMap<String, bool> = HashMap::new();
    let mut props_retry: HashMap<String, PropsRetry> = HashMap::new();
    // Interactive never ForceRespawns — blackhole FX job channel.
    let (fx_job_tx, fx_job_rx) = mpsc::channel::<FxEnsureJob>();
    drop(fx_job_rx);
    // Do not tear down BusChain on launch — leave buses / FX running across UI restart.
    // Always sweep pre-rebrand Shadow Audio leftovers (wrong media.name escaped teardown).
    match graph::teardown_legacy_shadow_graph() {
        Ok(msg) if !msg.is_empty() => {
            let _ = tx.send(Event::Status(msg));
        }
        Err(e) => {
            let _ = tx.send(Event::Error(format!("legacy shadow cleanup: {e:#}")));
        }
        _ => {}
    }
    let _ = tx.send(Event::Status(
        "Worker ready — interactive lane online".into(),
    ));
    // Initial topology via Observer-style publish (not on mute path).
    {
        let snap = graph::refresh_snapshot();
        graph::publish_observer_snapshot(snap.clone());
        let _ = tx.send(Event::Snapshot(snap));
    }

    loop {
        flush_props_retries(&mut props_retry);
        flush_place_retries(&mut place_retry, &tx);
        pull_shared_session_if_newer(&shared_session, &mut last_session, &mut session_gen);
        if let Ok(mut g) = midi_session.lock() {
            *g = last_session.clone();
        }
        publish_shared_session(&shared_session, &last_session, session_gen);
        poll_midi_actions(&tx, &last_session, &mut muted_buses);

        let first = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(c) => c,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Props priority: never start idle work if a cmd is already waiting.
                if let Ok(c) = rx.try_recv() {
                    let mut batch = vec![c];
                    while let Ok(more) = rx.try_recv() {
                        batch.push(more);
                    }
                    let batch = coalesce_commands(batch);
                    crate::audio::engine_handle::note_class_a();
                    if process_command_batch(
                        batch,
                        &mut fx,
                        &mut last_session,
                        &mut session_gen,
                        &shared_session,
                        &mut muted_buses,
                        &mut props_retry,
                        &mut place_retry,
                        &mut default_reclaim_until,
                        &tx,
                        &fx_job_tx,
                        true,
                    ) {
                        publish_shared_session(&shared_session, &last_session, session_gen);
                        return;
                    }
                    publish_shared_session(&shared_session, &last_session, session_gen);
                    continue;
                }
                // Interactive idle: retries only. Supervisor = reconcile; Observer = snapshot.
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };

        let mut batch = vec![first];
        while let Ok(c) = rx.try_recv() {
            batch.push(c);
        }
        let batch = coalesce_commands(batch);
        crate::audio::engine_handle::note_class_a();
        if process_command_batch(
            batch,
            &mut fx,
            &mut last_session,
            &mut session_gen,
            &shared_session,
            &mut muted_buses,
            &mut props_retry,
            &mut place_retry,
            &mut default_reclaim_until,
            &tx,
            &fx_job_tx,
            true,
        ) {
            publish_shared_session(&shared_session, &last_session, session_gen);
            return;
        }
        publish_shared_session(&shared_session, &last_session, session_gen);
    }
}

fn supervisor_loop(
    rx: Receiver<Command>,
    tx: Sender<Event>,
    shared_session: Arc<Mutex<SharedSessionSlot>>,
) {
    let mut fx = FilterChainRuntime::new();
    let mut last_session: Option<Session> = None;
    let mut session_gen: u64 = 0;
    let mut muted_buses: HashMap<String, bool> = HashMap::new();
    let mut props_retry: HashMap<String, PropsRetry> = HashMap::new();
    let mut place_retry: HashMap<String, PlaceRetry> = HashMap::new();
    let (fx_job_tx, fx_job_rx) = mpsc::channel::<FxEnsureJob>();
    let (fx_done_tx, fx_done_rx) = mpsc::channel::<FxEnsureDone>();
    spawn_fx_ensure_thread(fx_job_rx, fx_done_tx);
    let mut idle_ticks: u32 = 0;
    let mut last_cmd_at = Instant::now();
    // After Full Apply, reclaim a few more times while buschain sinks finish registering.
    let mut default_reclaim_until: Option<Instant> = None;
    let _ = tx.send(Event::Status(
        "Worker ready — supervisor lane online".into(),
    ));

    loop {
        pull_shared_session_if_newer(&shared_session, &mut last_session, &mut session_gen);
        while let Ok(done) = fx_done_rx.try_recv() {
            match done.result {
                Ok(message) => {
                    merge_ensure_track(&mut last_session, &done.session, done.track_id);
                    publish_shared_session(&shared_session, &last_session, session_gen);
                    let bus = done
                        .session
                        .tracks
                        .iter()
                        .find(|t| t.id == done.track_id)
                        .map(|t| t.expected_sink_name())
                        .unwrap_or_default();
                    let _ = tx.send(Event::FxReady {
                        track_id: done.track_id,
                        bus,
                    });
                    let _ = tx.send(Event::SessionApplied {
                        session: done.session,
                        message,
                        kind: SessionAppliedKind::FxRewire,
                    });
                }
                Err(e) => {
                    let _ = tx.send(Event::Error(format!("FX rewire: {e}")));
                }
            }
            last_cmd_at = Instant::now();
        }

        let first = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(c) => c,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(c) = rx.try_recv() {
                    let mut batch = vec![c];
                    while let Ok(more) = rx.try_recv() {
                        batch.push(more);
                    }
                    let batch = coalesce_commands(batch);
                    last_cmd_at = Instant::now();
                    if process_command_batch(
                        batch,
                        &mut fx,
                        &mut last_session,
                        &mut session_gen,
                        &shared_session,
                        &mut muted_buses,
                        &mut props_retry,
                        &mut place_retry,
                        &mut default_reclaim_until,
                        &tx,
                        &fx_job_tx,
                        false,
                    ) {
                        return;
                    }
                    publish_shared_session(&shared_session, &last_session, session_gen);
                    continue;
                }
                idle_ticks = idle_ticks.wrapping_add(1);
                if last_cmd_at.elapsed() < Duration::from_secs(2)
                    || crate::audio::engine_handle::class_a_is_recent()
                {
                    continue;
                }
                if let Some(ref session) = last_session {
                    if idle_ticks % 16 == 0 && rx.try_recv().is_err() {
                        sync_engine_clock(session);
                        let hw = session
                            .master_output
                            .clone()
                            .filter(|n| !n.is_empty() && !n.starts_with("buschain_"))
                            .or_else(|| graph::resolve_hardware_output(session).ok());
                        if let Some(hw) = hw {
                            let _ = crate::audio::engine_handle::with_engine_supervisor(|eng| {
                                eng.set_profile(&session.performance);
                                eng.remember_master_hw(&hw);
                                let _ = eng.reconcile_light();
                            });
                        }
                    }
                    let burst_reclaim = default_reclaim_until
                        .is_some_and(|until| Instant::now() < until);
                    if default_reclaim_until.is_some_and(|until| Instant::now() >= until) {
                        default_reclaim_until = None;
                    }
                    let reclaim_now = if burst_reclaim {
                        idle_ticks % 4 == 0
                    } else {
                        idle_ticks % 120 == 0
                    };
                    if reclaim_now && rx.try_recv().is_err() {
                        // Burst: reassert Pulse default + reclaim apps onto sticky preferred.
                        if burst_reclaim {
                            if let Some(pref) = session.preferred_default_sink.as_deref() {
                                match graph::set_default_sink_if_needed(pref) {
                                    Ok(true) => {}
                                    Ok(false) => {
                                        let _ = tx.send(Event::Status(format!(
                                            "preferred default did not stick — retrying ({pref})"
                                        )));
                                    }
                                    Err(e) => {
                                        let _ = tx.send(Event::Error(format!(
                                            "preferred default: {e:#}"
                                        )));
                                    }
                                }
                            }
                        }
                        match crate::audio::engine_handle::sync_playback(session) {
                            Ok(msg) if msg.contains("placed") => {
                                let _ = tx.send(Event::Status(msg));
                                if let Ok(inputs) = graph::refresh_sink_inputs_only() {
                                    let _ = tx.send(Event::SinkInputs(inputs));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };

        let mut batch = vec![first];
        while let Ok(c) = rx.try_recv() {
            batch.push(c);
        }
        let batch = coalesce_commands(batch);
        last_cmd_at = Instant::now();
        if process_command_batch(
            batch,
            &mut fx,
            &mut last_session,
            &mut session_gen,
            &shared_session,
            &mut muted_buses,
            &mut props_retry,
            &mut place_retry,
            &mut default_reclaim_until,
            &tx,
            &fx_job_tx,
            false,
        ) {
            return;
        }
        publish_shared_session(&shared_session, &last_session, session_gen);
    }
}

/// Returns true when the worker should exit (Shutdown).
fn process_command_batch(
    batch: Vec<Command>,
    mut fx: &mut FilterChainRuntime,
    last_session: &mut Option<Session>,
    session_gen: &mut u64,
    shared_session: &Arc<Mutex<SharedSessionSlot>>,
    muted_buses: &mut HashMap<String, bool>,
    props_retry: &mut HashMap<String, PropsRetry>,
    place_retry: &mut HashMap<String, PlaceRetry>,
    default_reclaim_until: &mut Option<Instant>,
    tx: &Sender<Event>,
    fx_job_tx: &Sender<FxEnsureJob>,
    restore_on_shutdown: bool,
) -> bool {
        for cmd in batch {
            match cmd {
                Command::Shutdown => {
                    fx.stop_all();
                    if restore_on_shutdown {
                        // Quit must not leave WirePlumber on a dead buschain_* default or a
                        // muted Master HW — that silences YouTube / the whole desktop.
                        let hw = last_session
                            .as_ref()
                            .and_then(|s| s.master_output.clone());
                        if let Err(e) = graph::restore_system_audio(hw.as_deref()) {
                            let _ = tx.send(Event::Error(format!("restore audio on quit: {e:#}")));
                        }
                    }
                    return true;
                }
                Command::Refresh => {
                    let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                }
                Command::Teardown => {
                    fx.stop_all();
                    // Full reset: hand audio back to HW + destroy linger nodes.
                    let hw = last_session
                        .as_ref()
                        .and_then(|s| s.master_output.clone());
                    match graph::restore_system_audio(hw.as_deref()) {
                        Ok(msg) => {
                            let _ = tx.send(Event::Status(msg));
                            let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Error(format!("teardown: {e:#}")));
                        }
                    }
                }
                Command::BindMasterClock(mut session) => {
                    sync_engine_clock(&session);
                    match crate::audio::engine_handle::bind_master_clock(&session.performance) {
                        Ok(clock_msg) => {
                            // Always ClockBind after bind — ForceRespawn FX at new GraphClock
                            // and exclusive-rewire inbound mics (rate bridges).
                            match graph::apply_session(
                                &mut session,
                                &mut fx,
                                graph::ApplyKind::ClockBind,
                            ) {
                                Ok(route_msg) => {
                                    let message = format!("{clock_msg} · {route_msg}");
                                    *last_session = Some(session.clone());
                                    let _ = tx.send(Event::SessionApplied { session, message, kind: SessionAppliedKind::Other });
                                    let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                                }
                                Err(e) => {
                                    let _ = tx.send(Event::Error(format!("clock bind: {e:#}")));
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Error(format!("BindMasterClock: {e:#}")));
                        }
                    }
                }
                Command::BindDeviceClock {
                    mut session,
                    device,
                    sample_rate,
                    quantum,
                    soft_quantum,
                    bind_buschain,
                } => {
                    sync_engine_clock(&session);
                    match crate::audio::engine_handle::bind_device_clock(
                        &device,
                        sample_rate,
                        quantum,
                        soft_quantum,
                        bind_buschain,
                    ) {
                        Ok(clock_msg) => {
                            if bind_buschain {
                                match graph::apply_session(
                                    &mut session,
                                    &mut fx,
                                    graph::ApplyKind::ClockBind,
                                ) {
                                    Ok(route_msg) => {
                                        let message = format!("{clock_msg} · {route_msg}");
                                        *last_session = Some(session.clone());
                                        let _ =
                                            tx.send(Event::SessionApplied { session, message, kind: SessionAppliedKind::Other });
                                        let _ =
                                            tx.send(Event::Snapshot(graph::refresh_snapshot()));
                                    }
                                    Err(e) => {
                                        let _ = tx.send(Event::Error(format!(
                                            "device clock bind: {e:#}"
                                        )));
                                    }
                                }
                            } else {
                                *last_session = Some(session.clone());
                                let _ = tx.send(Event::Status(clock_msg));
                                let _ = tx.send(Event::SessionApplied {
                                    session,
                                    message: format!("Device clock → {device}"),
                                    kind: SessionAppliedKind::Clock,
                                });
                                let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Error(format!("BindDeviceClock: {e:#}")));
                        }
                    }
                }
                Command::ApplySession(mut session) => {
                    sync_engine_clock(&session);
                    session.normalize();
                    if let Ok(msg) = graph::teardown_legacy_shadow_graph() {
                        if !msg.is_empty() {
                            let _ = tx.send(Event::Status(msg));
                        }
                    }
                    match graph::apply_session(&mut session, &mut fx, graph::ApplyKind::Full) {
                        Ok(message) => {
                            // Same post-steps as SetDefaultSink: preferred can already show as
                            // the live default in the UI while Master→HW / apps still need a kick.
                            if let Some(pref) = session.preferred_default_sink.clone() {
                                crate::audio::engine_handle::with_engine(|eng| {
                                    eng.desired_mut()
                                        .set_preferred_default(Some(pref.clone()));
                                });
                                let stuck = match graph::set_default_sink_if_needed(&pref) {
                                    Ok(true) => false,
                                    Ok(false) => {
                                        let _ = tx.send(Event::Status(format!(
                                            "preferred default did not stick — retrying ({pref})"
                                        )));
                                        true
                                    }
                                    Err(e) => {
                                        let _ = tx.send(Event::Error(format!(
                                            "preferred default: {e:#}"
                                        )));
                                        true
                                    }
                                };
                                if stuck {
                                    thread::sleep(Duration::from_millis(80));
                                    match graph::set_default_sink_if_needed(&pref) {
                                        Ok(true) => {}
                                        Ok(false) => {
                                            let _ = tx.send(Event::Status(format!(
                                                "preferred default still pending ({pref})"
                                            )));
                                        }
                                        Err(e) => {
                                            let _ = tx.send(Event::Error(format!(
                                                "preferred default retry: {e:#}"
                                            )));
                                        }
                                    }
                                }
                            }
                            match crate::audio::engine_handle::sync_playback(&session) {
                                Ok(msg) if msg.contains("placed") => {
                                    let _ = tx.send(Event::Status(msg));
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    let _ = tx.send(Event::Error(format!("place streams: {e:#}")));
                                }
                            }
                            // Kick Master→HW repair so a BusChain default is actually audible.
                            crate::audio::engine_handle::with_engine(|eng| {
                                let _ = eng.reconcile_light();
                            });
                            if session
                                .preferred_default_sink
                                .as_deref()
                                .is_some_and(|s| s.starts_with("buschain_"))
                            {
                                // Preferred sink / clients may appear a beat after Arm.
                                *default_reclaim_until =
                                    Some(Instant::now() + Duration::from_secs(10));
                            }
                            *last_session = Some(session.clone());
                            // Authoritative: Interactive must not overwrite preferred/VO.
                            publish_shared_session_authority(
                                shared_session,
                                last_session,
                                session_gen,
                            );
                            let _ = tx.send(Event::SessionApplied {
                                session,
                                message,
                                kind: SessionAppliedKind::Full,
                            });
                            let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Error(format!("apply: {e:#}")));
                        }
                    }
                }
                Command::RewireSessionRoutes(mut session)
                | Command::HotplugSession(mut session) => {
                    sync_engine_clock(&session);
                    session.normalize();
                    // One-shot capture + egress — never Hotplug N× rewire_track_route.
                    let hw = graph::resolve_hardware_output(&session)
                        .unwrap_or_else(|_| session.master_output.clone().unwrap_or_default());
                    for track in &session.tracks {
                        if crate::audio::graph::track_has_live_fx(track) {
                            let _ = crate::audio::insert_map::push_track_controls(
                                &session,
                                track.id,
                            );
                        }
                    }
                    match crate::audio::engine_handle::rewire_session_routes(&session, &hw) {
                        Ok(message) => {
                            // Volumes only — never N× gate_track_mute/arm after Route.
                            // (apply_track_levels re-armed every bus ~800ms each and
                            // made Add In / In-mute wait minutes behind the worker.)
                            for track in &session.tracks {
                                let sink = track.expected_sink_name();
                                let _ = graph::apply_one_track_volume(&sink, track.gain_db);
                            }
                            *last_session = Some(session.clone());
                            let _ = tx.send(Event::SessionApplied {
                                session,
                                message,
                                kind: SessionAppliedKind::Route,
                            });
                            let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Error(format!("route rewire: {e:#}")));
                        }
                    }
                }
                Command::RewireTrackFx {
                    session,
                    track_id,
                }
                | Command::HotplugTrack {
                    session,
                    track_id,
                } => {
                    // Async ForceRespawn — Props/levels keep running on this thread.
                    *last_session = Some(session.clone());
                    let _ = tx.send(Event::Status(
                        "Rewiring FX slots (async — controls stay live)…".into(),
                    ));
                    if fx_job_tx
                        .send(FxEnsureJob {
                            session,
                            track_id,
                            queued_at: Instant::now(),
                        })
                        .is_err()
                    {
                        let _ = tx.send(Event::Error(
                            "FX ensure thread died — restart BusChain Control".into(),
                        ));
                    }
                }
                Command::PushFxControls { bus, inserts } => {
                    let span = buschain_engine::fx_trace::span("WorkerPushFx");
                    // Keep last_session knob values current so idle sync_desired /
                    // ForceRespawn cannot revive stale Semitones=0 etc.
                    if let Some(session) = last_session.as_mut() {
                        if let Some(track) = session
                            .tracks
                            .iter_mut()
                            .find(|t| t.expected_sink_name() == bus)
                        {
                            for slot in &inserts {
                                if let Some(plug) = track
                                    .inserts
                                    .iter_mut()
                                    .find(|p| p.slot_id == slot.slot_id)
                                {
                                    for (k, v) in &slot.controls {
                                        plug.set_param(k, *v);
                                        if k == "Mix" {
                                            plug.mix = (*v).clamp(0.0, 1.0);
                                        }
                                        if k == "Bypass" {
                                            plug.bypass = *v >= 0.5;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // Hot path: no status spam on success (UI/layout cost every tick).
                    // Lock-free of Engine ForceRespawn (see engine_handle::push_fx_controls).
                    let r = crate::audio::insert_map::push_bus_controls(&bus, inserts.clone());
                    match r {
                        Ok(_) => {
                            props_retry.remove(&bus);
                            span.end_ok();
                        }
                        Err(e) => {
                            let msg = format!("{e:#}");
                            span.end(format!("skip {msg}"));
                            // Rebuild / A/B / topology miss — quiet retry, no status spam.
                            let deferred = msg.contains("rebuilding")
                                || msg.contains("props deferred")
                                || msg.contains("not found");
                            if deferred {
                                schedule_props_retry(props_retry, bus, inserts);
                            } else {
                                crate::audio::engine_handle::set_chain_wet_cached(&bus, false);
                                let _ = tx.send(Event::Status(format!(
                                    "Live params skipped ({msg})"
                                )));
                            }
                        }
                    }
                }
                Command::PushFxParams { session, track_id } => {
                    *last_session = Some(session.clone());
                    match crate::audio::insert_map::push_track_controls(&session, track_id) {
                        Ok(_) => {
                            if let Some(t) = session.tracks.iter().find(|t| t.id == track_id) {
                                props_retry.remove(&t.expected_sink_name());
                            }
                        }
                        Err(e) => {
                            let msg = format!("{e:#}");
                            let deferred = msg.contains("rebuilding")
                                || msg.contains("props deferred")
                                || msg.contains("not found");
                            if deferred {
                                if let Some((bus, inserts)) =
                                    crate::audio::insert_map::ladspa_slots_for(&session, track_id)
                                {
                                    schedule_props_retry(props_retry, bus, inserts);
                                }
                            } else {
                                if let Some(t) = session.tracks.iter().find(|t| t.id == track_id) {
                                    let bus = t.expected_sink_name();
                                    crate::audio::engine_handle::set_chain_wet_cached(&bus, false);
                                }
                                let _ = tx.send(Event::Status(format!(
                                    "Live params skipped ({msg})"
                                )));
                            }
                        }
                    }
                }
                Command::EnsureTrack {
                    mut session,
                    track_id,
                } => {
                    sync_engine_clock(&session);
                    // ensure_live_track syncs Desired + native-arms egress (no Pulse loopback).
                    match graph::ensure_live_track(&mut session, track_id) {
                        Ok(message) => {
                            // One-shot SyncPlayback now that the bus exists.
                            match crate::audio::engine_handle::sync_playback(&session) {
                                Ok(msg) if msg.contains("placed") => {
                                    let _ = tx.send(Event::Status(msg));
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    let _ = tx.send(Event::Error(format!("place streams: {e:#}")));
                                }
                            }
                            *last_session = Some(session.clone());
                            publish_shared_session_authority(
                                shared_session,
                                last_session,
                                session_gen,
                            );
                            let _ = tx.send(Event::SessionApplied {
                                session,
                                message,
                                kind: SessionAppliedKind::Ensure,
                            });
                            // Surface the new bus in Output / Playback immediately
                            // (idle full snapshot is ~30s).
                            let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Error(format!("ensure track: {e:#}")));
                        }
                    }
                }
                Command::VirtualInput {
                    session,
                    track_id,
                } => {
                    sync_engine_clock(&session);
                    match crate::audio::engine_handle::apply_virtual_input(&session, track_id) {
                        Ok(message) => {
                            *last_session = Some(session.clone());
                            let _ = tx.send(Event::SessionApplied {
                                session,
                                message,
                                kind: SessionAppliedKind::Ensure,
                            });
                            let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Error(format!("virtual input: {e:#}")));
                        }
                    }
                }
                Command::PruneTrack {
                    mut session,
                    removed_bus,
                } => match graph::prune_removed_track(&mut session, &mut fx, &removed_bus) {
                    Ok(message) => {
                        *last_session = Some(session.clone());
                        let _ = tx.send(Event::SessionApplied {
                            session,
                            message,
                            kind: SessionAppliedKind::Ensure,
                        });
                        let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                    }
                    Err(e) => {
                        let _ = tx.send(Event::Error(format!("prune track: {e:#}")));
                    }
                },
                Command::ApplyLevels(session) => {
                    *last_session = Some(session.clone());
                    if let Ok(hw) = graph::resolve_hardware_output(&session) {
                        crate::audio::engine_handle::sync_desired_from_session(&session, &hw);
                    }
                    // Mute transition only — never re-arm every unmuted bus.
                    let any_solo = session
                        .tracks
                        .iter()
                        .any(|t| t.solo && !t.kind.is_master());
                    for track in &session.tracks {
                        let sink = track.expected_sink_name();
                        let muted = track.mute
                            || (any_solo && !track.solo && !track.kind.is_master());
                        let was = muted_buses.get(&sink).copied().unwrap_or(false);
                        muted_buses.insert(sink.clone(), muted);
                        let result = if muted != was {
                            graph::apply_one_track_mute_gate(&sink, muted, track.gain_db)
                        } else {
                            graph::apply_one_track_volume(&sink, track.gain_db)
                        };
                        if let Err(e) = result {
                            let _ = tx.send(Event::Error(format!("levels: {e:#}")));
                        }
                    }
                }
                Command::SetTrackLevel {
                    sink,
                    gain_db,
                    muted,
                } => {
                    // Keep supervisor DesiredState in sync so idle reconcile
                    // does not snap volume/mute back to an old fader position.
                    if let Some(ref mut s) = last_session {
                        if let Some(t) = s
                            .tracks
                            .iter_mut()
                            .find(|t| t.expected_sink_name() == sink)
                        {
                            t.gain_db = gain_db;
                            t.mute = muted;
                        }
                    }
                    crate::audio::engine_handle::with_engine(|eng| {
                        use buschain_engine::BusLevel;
                        eng.desired_mut().set_bus_level(
                            &sink,
                            BusLevel {
                                gain_db,
                                mixer_mute: muted,
                            },
                        );
                    });
                    let was_muted = muted_buses.get(&sink).copied().unwrap_or(false);
                    let result = if muted {
                        muted_buses.insert(sink.clone(), true);
                        // Full gate — silence monitor/FX without corking apps.
                        graph::apply_one_track_mute_gate(&sink, true, gain_db)
                    } else if was_muted {
                        muted_buses.insert(sink.clone(), false);
                        // Unmute transition — reopen FX/post once, then volume.
                        graph::apply_one_track_mute_gate(&sink, false, gain_db)
                    } else {
                        muted_buses.insert(sink.clone(), false);
                        // Continuous fader: bus volume only (no list_sinks / ensure loops).
                        graph::apply_one_track_volume(&sink, gain_db)
                    };
                    if let Err(e) = result {
                        let _ = tx.send(Event::Error(format!("level: {e:#}")));
                    }
                }
                Command::SetSinkVolume { name, pct } => {
                    if let Err(e) = graph::set_sink_volume(&name, pct) {
                        let _ = tx.send(Event::Error(e.to_string()));
                    }
                    // No full snapshot — waybar/daemon keep optimistic HW cache.
                }
                Command::SetSinkMute { name, mute } => {
                    if let Err(e) = graph::set_sink_mute(&name, mute) {
                        let _ = tx.send(Event::Error(e.to_string()));
                    }
                }
                Command::SetSourceVolume { name, pct } => {
                    if let Err(e) = graph::set_source_volume(&name, pct) {
                        let _ = tx.send(Event::Error(e.to_string()));
                    }
                }
                Command::SetSourceMute { name, mute } => {
                    if let Err(e) = graph::set_source_mute(&name, mute) {
                        let _ = tx.send(Event::Error(e.to_string()));
                    }
                }
                Command::SetSinkInputVolume { index, pct } => {
                    if let Err(e) = graph::set_sink_input_volume(index, pct) {
                        let _ = tx.send(Event::Error(e.to_string()));
                    }
                }
                Command::SetSinkInputMute { index, mute } => {
                    if let Err(e) = graph::set_sink_input_mute(index, mute) {
                        let _ = tx.send(Event::Error(e.to_string()));
                    }
                }
                Command::MoveSinkInput { index, sink } => {
                    let current = graph::list_sink_inputs()
                        .ok()
                        .and_then(|inputs| {
                            inputs
                                .into_iter()
                                .find(|s| s.index == index)
                                .map(|s| s.sink_or_source)
                        })
                        .unwrap_or_default();
                    match graph::move_sink_input_if_needed(index, &sink, &current) {
                        Ok(true) => {
                            let _ =
                                tx.send(Event::Status(format!("Moved stream {index} → {sink}")));
                            let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                        }
                        Ok(false) => {}
                        Err(e) => {
                            let _ = tx.send(Event::Error(e.to_string()));
                        }
                    }
                }
                Command::SyncCaptureDelta { session, track_id } => {
                    *last_session = Some(session.clone());
                    match crate::audio::engine_handle::sync_capture_delta(&session, track_id) {
                        Ok(msg) => {
                            let _ = tx.send(Event::Status(if msg.is_empty() {
                                "Capture updated".into()
                            } else {
                                msg
                            }));
                            // Never refresh_snapshot on Capture — Class B budget.
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Error(format!("capture: {e:#}")));
                        }
                    }
                }
                Command::PlaceApp {
                    session,
                    app_key,
                    sink,
                } => {
                    // Patch bus_playback only — never full Desired rebuild.
                    *last_session = Some(session.clone());
                    if let Some(tid) = session
                        .tracks
                        .iter()
                        .find(|t| t.expected_sink_name() == sink)
                        .map(|t| t.id)
                    {
                        crate::audio::engine_handle::patch_bus_playback(&session, tid);
                    }
                    match graph::place_app_on_sink(&app_key, &sink) {
                        Ok(0) => {
                            let _ = tx.send(Event::Status(format!(
                                "App {app_key} — no live streams to move (play something)"
                            )));
                            // Skip full snapshot — idle tick will refresh.
                        }
                        Ok(n) => {
                            let _ = tx.send(Event::Status(format!(
                                "Placed {n} stream(s) → {sink}"
                            )));
                            if let Ok(inputs) = graph::refresh_sink_inputs_only() {
                                let _ = tx.send(Event::SinkInputs(inputs));
                            }
                        }
                        Err(e) => {
                            // Defer retry — never sleep on the Props/levels thread.
                            place_retry.insert(
                                app_key.clone(),
                                PlaceRetry {
                                    sink: sink.clone(),
                                    after: Instant::now() + Duration::from_millis(80),
                                    attempts: 1,
                                    last_err: format!("{e:#}"),
                                },
                            );
                        }
                    }
                }
                Command::SyncPlayback(session) => {
                    *last_session = Some(session.clone());
                    match crate::audio::engine_handle::sync_playback(&session) {
                        Ok(msg) => {
                            let _ = tx.send(Event::Status(msg));
                            if let Ok(inputs) = graph::refresh_sink_inputs_only() {
                                let _ = tx.send(Event::SinkInputs(inputs));
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Error(format!("sync playback: {e:#}")));
                        }
                    }
                }
                Command::SetDefaultSink(name) => {
                    if let Some(ref mut s) = last_session {
                        s.preferred_default_sink = Some(name.clone());
                    }
                    publish_shared_session_authority(shared_session, last_session, session_gen);
                    crate::audio::engine_handle::with_engine(|eng| {
                        eng.desired_mut()
                            .set_preferred_default(Some(name.clone()));
                    });
                    match graph::set_default_sink_if_needed(&name) {
                        Ok(true) => {
                            let _ = tx.send(Event::Status(format!("System default → {name}")));
                        }
                        Ok(false) => {
                            let _ = tx.send(Event::Status(format!(
                                "default did not stick — retrying ({name})"
                            )));
                            thread::sleep(Duration::from_millis(80));
                            let _ = graph::set_default_sink_if_needed(&name);
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Error(format!("set-default-sink: {e:#}")));
                        }
                    }
                    // Immediate reclaim so apps leave HW/legacy sinks when default is BusChain.
                    if let Some(session) = last_session.as_ref() {
                        match crate::audio::engine_handle::sync_playback(session) {
                            Ok(msg) if msg.contains("placed") => {
                                let _ = tx.send(Event::Status(msg));
                            }
                            Ok(_) => {}
                            Err(e) => {
                                let _ = tx.send(Event::Error(format!("place streams: {e:#}")));
                            }
                        }
                    }
                    // Kick Master→HW repair so BusChain default is audible.
                    crate::audio::engine_handle::with_engine(|eng| {
                        let _ = eng.reconcile_light();
                    });
                    if name.starts_with("buschain_") {
                        *default_reclaim_until =
                            Some(Instant::now() + Duration::from_secs(10));
                    }
                    let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                }
                Command::SetDefaultSource(name) => {
                    if let Err(e) = graph::set_default_source(&name) {
                        let _ = tx.send(Event::Error(e.to_string()));
                    }
                }
                Command::SetMasterHw { name, desc } => {
                    if let Some(ref mut s) = last_session {
                        s.master_output = Some(name.clone());
                        if let Some(d) = desc.clone() {
                            s.master_output_desc = Some(d);
                        }
                    }
                    match crate::audio::engine_handle::set_master_hw_light(&name) {
                        Ok(msg) => {
                            let _ = tx.send(Event::Status(if msg.is_empty() {
                                format!("Master HW → {name}")
                            } else {
                                msg
                            }));
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Error(format!("master hw: {e:#}")));
                        }
                    }
                }
                Command::SetBusDescription { name, description } => {
                    let _ = buschain_engine::backend::push_description(&name, &description);
                    let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                }
                Command::RefreshMidi => {
                    let devices = last_session
                        .as_ref()
                        .map(|s| s.midi_devices.as_slice())
                        .unwrap_or(&[]);
                    let snap = buschain_engine::midi::snapshot_global(devices);
                    let _ = tx.send(Event::MidiSnapshot(snap));
                }
                Command::ApplyMidiConfig(session) => {
                    // MIDI-only merge — never replace preferred/VO with a pre-Ensure clone.
                    match last_session.as_mut() {
                        Some(local) => {
                            local.midi_devices = session.midi_devices.clone();
                            local.midi_routes = session.midi_routes.clone();
                            local.midi_maps = session.midi_maps.clone();
                        }
                        None => {
                            *last_session = Some(session.clone());
                        }
                    }
                    buschain_engine::midi::apply_intent_global(
                        buschain_engine::MidiIntent::ApplyConfig {
                            devices: session.midi_devices.clone(),
                            routes: session.midi_routes.clone(),
                            maps: session.midi_maps.clone(),
                        },
                    );
                    let snap =
                        buschain_engine::midi::snapshot_global(&session.midi_devices);
                    let _ = tx.send(Event::MidiSnapshot(snap));
                }
                Command::Midi(intent) => {
                    buschain_engine::midi::apply_intent_global(intent);
                }
            }
        }
        false
}

fn poll_midi_actions(
    tx: &Sender<Event>,
    last_session: &Option<Session>,
    muted_buses: &mut HashMap<String, bool>,
) {
    for action in buschain_engine::midi::poll_actions_global() {
        match action {
            buschain_engine::MidiAction::SetTrackLevel { sink, gain_db } => {
                let muted = last_session
                    .as_ref()
                    .and_then(|s| {
                        s.tracks
                            .iter()
                            .find(|t| t.expected_sink_name() == sink)
                            .map(|t| t.mute)
                    })
                    .unwrap_or(false);
                muted_buses.insert(sink.clone(), muted);
                // Same path as UI faders — Desired + PW level together.
                let _ = graph::apply_one_track_volume(&sink, gain_db);
                let _ = crate::audio::engine_handle::set_levels(&sink, gain_db, muted);
            }
            buschain_engine::MidiAction::MapLearned { map } => {
                let _ = tx.send(Event::MidiLearnBound { map });
            }
        }
    }
}
