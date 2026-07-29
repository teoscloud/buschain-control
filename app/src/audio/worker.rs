//! Background PipeWire worker — never block the UI thread.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use crate::audio::filter_chain::FilterChainRuntime;
use crate::audio::graph::{self, PwSnapshot};
use crate::session::Session;

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
    PlaceApp { app_key: String, sink: String },
    SetDefaultSink(String),
    SetDefaultSource(String),
    /// Light Master HW switch — relink only, never ApplySession / ForceRespawn.
    SetMasterHw {
        name: String,
        desc: Option<String>,
    },
    Shutdown,
}

pub enum Event {
    Snapshot(PwSnapshot),
    Status(String),
    SessionApplied { session: Session, message: String },
    Error(String),
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

        thread::Builder::new()
            .name("buschain-pw".into())
            .spawn(move || worker_loop(cmd_rx, ev_tx))
            .expect("spawn audio worker");

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
                            });
                        } else if let Some(st) = status {
                            let _ = ev_tx.send(Event::Status(st.message));
                        } else if !message.is_empty() {
                            let _ = ev_tx.send(Event::Status(message));
                        }
                    }
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

/// Collapse queued level updates so fader drags never backlog behind themselves.
/// Fast path (default / levels / moves) runs *before* ApplySession / FX rewire
/// so Master HW and system default are not stuck behind a multi-second FX spawn.
fn coalesce_commands(mut cmds: Vec<Command>) -> Vec<Command> {
    if cmds.iter().any(|c| matches!(c, Command::Shutdown)) {
        return vec![Command::Shutdown];
    }

    let mut levels: HashMap<String, (f32, bool)> = HashMap::new();
    let mut full_levels: Option<Session> = None;
    let mut fx_params: HashMap<uuid::Uuid, Session> = HashMap::new();
    let mut fx_controls: HashMap<String, Vec<buschain_engine::InsertSlot>> = HashMap::new();
    let mut fast = Vec::new();
    let mut heavy = Vec::new();

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
            m @ (Command::MoveSinkInput { .. }
            | Command::PlaceApp { .. }
            | Command::SetDefaultSink(_)
            | Command::SetDefaultSource(_)
            | Command::SetMasterHw { .. }
            | Command::SetSinkMute { .. }
            | Command::SetSinkVolume { .. }
            | Command::SetSourceMute { .. }
            | Command::SetSourceVolume { .. }
            | Command::SetSinkInputMute { .. }
            | Command::SetSinkInputVolume { .. }
            | Command::Refresh) => {
                fast.push(m);
            }
            // Route rewires are heavy — never ahead of Props/levels (knob HOL).
            m @ (Command::RewireSessionRoutes(_) | Command::HotplugSession(_)) => {
                heavy.push(m);
            }
            other => heavy.push(other),
        }
    }

    if let Some(s) = full_levels {
        fast.push(Command::ApplyLevels(s));
    } else {
        for (sink, (gain_db, muted)) in levels {
            fast.push(Command::SetTrackLevel {
                sink,
                gain_db,
                muted,
            });
        }
    }
    for (bus, inserts) in fx_controls {
        fast.push(Command::PushFxControls { bus, inserts });
    }
    for (track_id, session) in fx_params {
        // Prefer controls extracted from session so we don't ship the blob twice.
        if let Some((bus, inserts)) =
            crate::audio::insert_map::ladspa_slots_for(&session, track_id)
        {
            if !fast.iter().any(|c| {
                matches!(c, Command::PushFxControls { bus: b, .. } if b == &bus)
            }) {
                fast.push(Command::PushFxControls { bus, inserts });
            }
        } else {
            fast.push(Command::PushFxParams { session, track_id });
        }
    }
    fast.append(&mut heavy);
    fast
}

fn sync_engine_clock(session: &Session) {
    crate::audio::engine_handle::sync_profile(&session.performance);
}

fn worker_loop(rx: Receiver<Command>, tx: Sender<Event>) {
    let mut fx = FilterChainRuntime::new();
    // Last session seen by the worker — used for idle GraphSupervisor ticks.
    let mut last_session: Option<Session> = None;
    let mut idle_ticks: u32 = 0;
    // When Props/fast cmds last ran — skip Idempotent FX respawn briefly after.
    let mut last_cmd_at = Instant::now();
    // Buses currently mixer-muted — volume drags skip the heavy gate path.
    let mut muted_buses: HashMap<String, bool> = HashMap::new();
    // Do not tear down on launch — leave existing BusChain buses / FX chains running
    // so restarting the UI doesn't cut audio. Advanced "Reset graph" still cleans up.
    let _ = tx.send(Event::Status(
        "Worker ready — buschain-engine supervisor online".into(),
    ));
    let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));

    loop {
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
                    last_cmd_at = Instant::now();
                    if process_command_batch(
                        batch,
                        &mut fx,
                        &mut last_session,
                        &mut muted_buses,
                        &tx,
                    ) {
                        return;
                    }
                    continue;
                }
                idle_ticks = idle_ticks.wrapping_add(1);
                // Any recent knob/level/cmd: skip ALL idle graph work. Even
                // reconcile_light_no_fx probes wetness (pactl/pw-link) and can
                // block Props for hundreds of ms–seconds between sparse drags.
                let recent_cmds = last_cmd_at.elapsed() < Duration::from_secs(2);
                if recent_cmds {
                    continue;
                }
                if let Some(ref mut session) = last_session {
                    // Light reconcile ~every 4s when fully idle (Master egress repair).
                    if idle_ticks % 16 == 0 {
                        if let Ok(c) = rx.try_recv() {
                            let mut batch = vec![c];
                            while let Ok(more) = rx.try_recv() {
                                batch.push(more);
                            }
                            let batch = coalesce_commands(batch);
                            last_cmd_at = Instant::now();
                            if process_command_batch(batch, &mut fx, &mut last_session, &mut muted_buses, &tx) {
                                return;
                            }
                            continue;
                        }
                        sync_engine_clock(session);
                        let hw = session
                            .master_output
                            .clone()
                            .filter(|n| !n.is_empty() && !n.starts_with("buschain_"))
                            .or_else(|| graph::resolve_hardware_output(session).ok());
                        if let Some(hw) = hw {
                            if let Ok(c) = rx.try_recv() {
                                let mut batch = vec![c];
                                while let Ok(more) = rx.try_recv() {
                                    batch.push(more);
                                }
                                let batch = coalesce_commands(batch);
                                last_cmd_at = Instant::now();
                                if process_command_batch(batch, &mut fx, &mut last_session, &mut muted_buses, &tx)
                                {
                                    return;
                                }
                                continue;
                            }
                            crate::audio::engine_handle::sync_desired_from_session(session, &hw);
                            crate::audio::engine_handle::with_engine(|eng| {
                                let _ = eng.reconcile_light();
                            });
                        }
                    }
                    // Placements ~every 30s and only when channel empty.
                    if idle_ticks % 120 == 0 {
                        if let Ok(c) = rx.try_recv() {
                            let mut batch = vec![c];
                            while let Ok(more) = rx.try_recv() {
                                batch.push(more);
                            }
                            let batch = coalesce_commands(batch);
                            last_cmd_at = Instant::now();
                            if process_command_batch(batch, &mut fx, &mut last_session, &mut muted_buses, &tx) {
                                return;
                            }
                            continue;
                        }
                        if let Ok(n) = graph::enforce_playback_placements(session) {
                            if n > 0 {
                                let _ = tx.send(Event::Status(format!(
                                    "Placed {n} stream(s) onto session buses"
                                )));
                                if rx.try_recv().is_err() {
                                    let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                                }
                            }
                        }
                    }
                }
                // Full snapshot ~every 30s when idle (never starve Props).
                if idle_ticks % 120 == 0 {
                    if let Ok(c) = rx.try_recv() {
                        let mut batch = vec![c];
                        while let Ok(more) = rx.try_recv() {
                            batch.push(more);
                        }
                        let batch = coalesce_commands(batch);
                        last_cmd_at = Instant::now();
                        if process_command_batch(batch, &mut fx, &mut last_session, &mut muted_buses, &tx) {
                            return;
                        }
                        continue;
                    }
                    let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
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
        if process_command_batch(batch, &mut fx, &mut last_session, &mut muted_buses, &tx) {
            return;
        }
    }
}

/// Returns true when the worker should exit (Shutdown).
fn process_command_batch(
    batch: Vec<Command>,
    mut fx: &mut FilterChainRuntime,
    last_session: &mut Option<Session>,
    muted_buses: &mut HashMap<String, bool>,
    tx: &Sender<Event>,
) -> bool {
        for cmd in batch {
            match cmd {
                Command::Shutdown => {
                    fx.stop_all();
                    return true;
                }
                Command::Refresh => {
                    let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                }
                Command::Teardown => {
                    fx.stop_all();
                    // Clear DesiredState + speakers_armed so the next ApplySession
                    // re-runs ArmSession instead of idle-disarming Master→HW forever.
                    let _ = crate::audio::engine_handle::engine_teardown();
                    match graph::teardown_buschain_graph() {
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
                                    let _ = tx.send(Event::SessionApplied { session, message });
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
                                            tx.send(Event::SessionApplied { session, message });
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
                    match graph::apply_session(&mut session, &mut fx, graph::ApplyKind::Full) {
                        Ok(message) => {
                            *last_session = Some(session.clone());
                            let _ = tx.send(Event::SessionApplied { session, message });
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
                    crate::audio::engine_handle::teardown_rate_bridges();
                    match graph::apply_session(
                        &mut session,
                        &mut fx,
                        graph::ApplyKind::Hotplug,
                    ) {
                        Ok(message) => {
                            *last_session = Some(session.clone());
                            let _ = tx.send(Event::SessionApplied { session, message });
                            let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Error(format!("route rewire: {e:#}")));
                        }
                    }
                }
                Command::RewireTrackFx {
                    mut session,
                    track_id,
                }
                | Command::HotplugTrack {
                    mut session,
                    track_id,
                } => {
                    sync_engine_clock(&session);
                    match graph::rewire_track_fx(&mut session, &mut fx, track_id) {
                        Ok(message) => {
                            *last_session = Some(session.clone());
                            let _ = tx.send(Event::SessionApplied { session, message });
                            let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Error(format!("FX rewire: {e:#}")));
                            let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                        }
                    }
                }
                Command::PushFxControls { bus, inserts } => {
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
                    if let Err(e) =
                        crate::audio::insert_map::push_bus_controls(&bus, inserts)
                    {
                        crate::audio::engine_handle::set_chain_wet_cached(&bus, false);
                        let _ = tx.send(Event::Status(format!(
                            "Live params skipped ({e:#})"
                        )));
                    }
                }
                Command::PushFxParams { session, track_id } => {
                    *last_session = Some(session.clone());
                    if let Err(e) =
                        crate::audio::insert_map::push_track_controls(&session, track_id)
                    {
                        if let Some(t) = session.tracks.iter().find(|t| t.id == track_id) {
                            let bus = t.expected_sink_name();
                            crate::audio::engine_handle::set_chain_wet_cached(&bus, false);
                        }
                        let _ = tx.send(Event::Status(format!(
                            "Live params skipped ({e:#})"
                        )));
                    }
                }
                Command::EnsureTrack {
                    mut session,
                    track_id,
                } => {
                    sync_engine_clock(&session);
                    match graph::ensure_live_track(&mut session, track_id) {
                        Ok(message) => {
                            *last_session = Some(session.clone());
                            let _ = tx.send(Event::SessionApplied { session, message });
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Error(format!("ensure track: {e:#}")));
                        }
                    }
                }
                Command::PruneTrack {
                    session,
                    removed_bus,
                } => match graph::prune_removed_track(&session, &mut fx, &removed_bus) {
                    Ok(message) => {
                        let _ = tx.send(Event::SessionApplied { session, message });
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
                    if let Err(e) = graph::apply_track_levels(&session) {
                        let _ = tx.send(Event::Error(format!("levels: {e:#}")));
                    }
                }
                Command::SetTrackLevel {
                    sink,
                    gain_db,
                    muted,
                } => {
                    // Keep supervisor DesiredState in sync so idle reconcile
                    // does not snap volume back to an old fader position.
                    if let Some(ref mut s) = last_session {
                        if let Some(t) = s
                            .tracks
                            .iter_mut()
                            .find(|t| t.expected_sink_name() == sink)
                        {
                            t.gain_db = gain_db;
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
                Command::PlaceApp { app_key, sink } => {
                    match graph::place_app_on_sink(&app_key, &sink) {
                        Ok(0) => {
                            let _ = tx.send(Event::Status(format!(
                                "App {app_key} — no live streams to move (play something)"
                            )));
                            let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                        }
                        Ok(n) => {
                            let _ = tx.send(Event::Status(format!(
                                "Placed {n} stream(s) → {sink}"
                            )));
                            let _ = tx.send(Event::Snapshot(graph::refresh_snapshot()));
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Error(format!("place app: {e:#}")));
                        }
                    }
                }
                Command::SetDefaultSink(name) => {
                    if let Some(ref mut s) = last_session {
                        s.preferred_default_sink = Some(name.clone());
                    }
                    crate::audio::engine_handle::with_engine(|eng| {
                        eng.desired_mut()
                            .set_preferred_default(Some(name.clone()));
                    });
                    // Light path only — placements/full snapshot are deferred (idle).
                    match graph::set_default_sink_if_needed(&name) {
                        Ok(true) => {
                            let _ = tx.send(Event::Status(format!("System default → {name}")));
                        }
                        Ok(false) => {
                            let _ = tx.send(Event::Status(format!(
                                "default did not stick — retrying ({name})"
                            )));
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Error(format!("set-default-sink: {e:#}")));
                        }
                    }
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
            }
        }
        false
}
