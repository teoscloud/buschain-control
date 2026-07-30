//! MIDI reader thread — ALSA sequencer (no `aconnect` from the app).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use uuid::Uuid;

use super::enumerate;
use super::event::MidiEvent;
use super::intent::MidiIntent;
use super::map::{apply_map_with, MidiAction};
use super::types::{
    MidiCcMap, MidiDeviceInfo, MidiDeviceLive, MidiMapTarget, MidiRoute, MidiSnapshot,
};

type TrackResolver = Arc<dyn Fn(Uuid) -> Option<String> + Send + Sync>;

struct RuntimeState {
    devices: Vec<MidiDeviceInfo>,
    routes: Vec<MidiRoute>,
    maps: Vec<MidiCcMap>,
    learn: Option<(Option<String>, MidiMapTarget)>,
    activity: HashMap<String, f32>,
    track_sink: TrackResolver,
    track_bus: TrackResolver,
}

impl RuntimeState {
    fn snapshot(&self) -> MidiSnapshot {
        let live = enumerate::enumerate_devices(&self.devices);
        let devices: Vec<MidiDeviceLive> = live
            .into_iter()
            .map(|mut d| {
                d.activity = self.activity.get(&d.id).copied().unwrap_or(0.0);
                d
            })
            .collect();
        MidiSnapshot {
            devices,
            status: if self.maps.is_empty() {
                "No CC maps — use Learn on a knob".into()
            } else {
                format!("{} CC map(s) active", self.maps.len())
            },
        }
    }
}

pub struct MidiRuntime {
    cmd_tx: Sender<MidiIntent>,
    action_rx: Receiver<MidiAction>,
    join: Option<JoinHandle<()>>,
    quit: Arc<AtomicBool>,
}

impl Default for MidiRuntime {
    fn default() -> Self {
        Self::new(Arc::new(|_| None), Arc::new(|_| None))
    }
}

impl MidiRuntime {
    pub fn new(track_sink: TrackResolver, track_bus: TrackResolver) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (action_tx, action_rx) = mpsc::channel();
        let quit = Arc::new(AtomicBool::new(false));
        let quit_t = Arc::clone(&quit);
        let join = thread::Builder::new()
            .name("buschain-midi".into())
            .spawn(move || midi_thread(cmd_rx, action_tx, quit_t, track_sink, track_bus))
            .ok();
        Self {
            cmd_tx,
            action_rx,
            join,
            quit,
        }
    }

    pub fn send(&self, intent: MidiIntent) {
        let _ = self.cmd_tx.send(intent);
    }

    pub fn poll_actions(&self) -> Vec<MidiAction> {
        let mut out = Vec::new();
        loop {
            match self.action_rx.try_recv() {
                Ok(a) => out.push(a),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }

    pub fn snapshot(&self) -> MidiSnapshot {
        self.send(MidiIntent::RefreshDevices);
        MidiSnapshot {
            devices: enumerate::enumerate_devices(&[]),
            status: "MIDI".into(),
        }
    }
}

impl Drop for MidiRuntime {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Release);
        let _ = self.cmd_tx.send(MidiIntent::StopLearn);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn midi_thread(
    cmd_rx: Receiver<MidiIntent>,
    action_tx: Sender<MidiAction>,
    quit: Arc<AtomicBool>,
    track_sink: TrackResolver,
    track_bus: TrackResolver,
) {
    let state = Arc::new(Mutex::new(RuntimeState {
        devices: Vec::new(),
        routes: Vec::new(),
        maps: Vec::new(),
        learn: None,
        activity: HashMap::new(),
        track_sink,
        track_bus,
    }));

    let alsa = open_alsa_reader();

    while !quit.load(Ordering::Acquire) {
        if let Some(ref reader) = alsa {
            for (port_name, ev) in reader.poll_events() {
                handle_event(&state, &action_tx, &port_name, ev);
            }
        }

        match cmd_rx.recv_timeout(Duration::from_millis(16)) {
            Ok(intent) => apply_intent(&state, intent),
            Err(mpsc::RecvTimeoutError::Timeout) => decay_activity(&state),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn apply_intent(state: &Arc<Mutex<RuntimeState>>, intent: MidiIntent) {
    let mut st = state.lock().unwrap();
    match intent {
        MidiIntent::RefreshDevices => {
            let live = enumerate::enumerate_devices(&st.devices);
            for d in &live {
                st.activity.entry(d.id.clone()).or_insert(0.0);
            }
        }
        MidiIntent::SetDeviceEnabled { device_id, enabled } => {
            if let Some(d) = st.devices.iter_mut().find(|d| d.id == device_id) {
                d.enabled = enabled;
            } else {
                st.devices.push(MidiDeviceInfo {
                    id: device_id,
                    description: String::new(),
                    enabled,
                });
            }
        }
        MidiIntent::SetRoute { route } => {
            st.routes.retain(|r| {
                !(r.device_id == route.device_id && r.target.key() == route.target.key())
            });
            st.routes.push(route);
        }
        MidiIntent::RemoveRoute {
            device_id,
            target_key,
        } => {
            st.routes
                .retain(|r| !(r.device_id == device_id && r.target.key() == target_key));
        }
        MidiIntent::MapCc { map } => {
            st.maps.retain(|m| {
                !(m.device_id == map.device_id
                    && m.channel == map.channel
                    && m.controller == map.controller)
            });
            st.maps.push(map);
        }
        MidiIntent::UnmapCc {
            device_id,
            channel,
            controller,
        } => {
            st.maps.retain(|m| {
                !(m.device_id == device_id
                    && m.channel == channel
                    && m.controller == controller)
            });
        }
        MidiIntent::ApplyConfig {
            devices,
            routes,
            maps,
        } => {
            st.devices = devices;
            st.routes = routes;
            st.maps = maps;
        }
        MidiIntent::StartLearn { device_id, pending } => {
            st.learn = Some((device_id, pending));
        }
        MidiIntent::StopLearn => st.learn = None,
    }
}

fn handle_event(
    state: &Arc<Mutex<RuntimeState>>,
    action_tx: &Sender<MidiAction>,
    device_id: &str,
    event: MidiEvent,
) {
    let mut st = state.lock().unwrap();
    if let Some(d) = st.devices.iter().find(|d| d.id == device_id) {
        if !d.enabled {
            return;
        }
    }
    st.activity.insert(device_id.to_string(), 1.0);

    let learn_target = st.learn.as_ref().and_then(|(dev, target)| {
        if dev.as_ref().is_none_or(|d| d == device_id) {
            Some(target.clone())
        } else {
            None
        }
    });

    if let Some(target) = learn_target {
        let MidiEvent::Cc {
            channel,
            controller,
            ..
        } = event
        else {
            return;
        };
        let map = MidiCcMap {
            device_id: device_id.to_string(),
            channel,
            controller,
            target,
        };
        st.maps.retain(|m| {
            !(m.device_id == map.device_id
                && m.channel == map.channel
                && m.controller == map.controller)
        });
        st.maps.push(map.clone());
        st.learn = None;
        let sink = Arc::clone(&st.track_sink);
        let bus = Arc::clone(&st.track_bus);
        apply_map_with(&map, cc_value(&event), &*sink, &*bus);
        let _ = action_tx.send(MidiAction::MapLearned { map });
        return;
    }

    let maps = st.maps.clone();
    let routes = st.routes.clone();
    let sink = Arc::clone(&st.track_sink);
    let bus = Arc::clone(&st.track_bus);
    drop(st);

    let map = maps.iter().find(|m| {
        if let MidiEvent::Cc {
            controller,
            channel,
            ..
        } = event
        {
            m.device_id == device_id
                && m.controller == controller
                && (m.channel == 255 || m.channel == channel)
        } else {
            false
        }
    });
    if let Some(m) = map {
        if routes
            .iter()
            .any(|r| r.enabled && r.device_id == device_id)
            || !maps.is_empty()
        {
            if let Some(action) = apply_map_with(m, cc_value(&event), &*sink, &*bus) {
                let _ = action_tx.send(action);
            }
        }
    }
}

fn cc_value(event: &MidiEvent) -> u8 {
    match event {
        MidiEvent::Cc { value, .. } => *value,
        _ => 0,
    }
}

fn decay_activity(state: &Arc<Mutex<RuntimeState>>) {
    let mut st = state.lock().unwrap();
    for v in st.activity.values_mut() {
        *v = (*v * 0.85).max(0.0);
    }
}

struct AlsReader {
    #[cfg(feature = "midi-alsa")]
    seq: alsa::seq::Seq,
    #[cfg(feature = "midi-alsa")]
    _port_id: i32,
}

impl AlsReader {
    fn open() -> Option<Self> {
        #[cfg(feature = "midi-alsa")]
        {
            use alsa::seq::{AllocType, OpenMode, PortCap, PortInfo, PortSubscribed, PortType};
            let mut seq = alsa::seq::Seq::new(None, OpenMode::Duplex, AllocType::Auto).ok()?;
            seq.set_client_name("buschain-control").ok()?;
            let port_id = seq
                .create_simple_port(
                    "midi-in",
                    PortCap::WRITE | PortCap::SUBS_WRITE,
                    PortType::MIDI_GENERIC | PortType::APPLICATION,
                )
                .ok()?;
            subscribe_all_ports(&mut seq, port_id);
            Some(Self {
                seq,
                _port_id: port_id,
            })
        }
        #[cfg(not(feature = "midi-alsa"))]
        {
            None
        }
    }

    fn poll_events(&self) -> Vec<(String, MidiEvent)> {
        #[cfg(feature = "midi-alsa")]
        {
            use alsa::seq::EventType;
            let mut out: Vec<(String, MidiEvent)> = Vec::new();
            while let Ok(true) = self.seq.event_input_pending(0) {
                if let Ok(ev) = self.seq.event_input() {
                    let port_name = ev
                        .get_addr()
                        .map(|a| format!("client:{}:port:{}", a.client, a.port))
                        .unwrap_or_else(|| "unknown".into());
                    if let Some(parsed) = parse_alsa_event(&ev) {
                        out.push((port_name, parsed));
                    }
                    if ev.get_type() == EventType::None {
                        break;
                    }
                } else {
                    break;
                }
            }
            out
        }
        #[cfg(not(feature = "midi-alsa"))]
        {
            Vec::new()
        }
    }
}

#[cfg(feature = "midi-alsa")]
fn subscribe_all_ports(seq: &mut alsa::seq::Seq, our_port: i32) {
    use alsa::seq::{PortCap, PortInfo, PortSubscribed, QuerySubscribe, SubscribeMode};
    let this = seq.client_id().unwrap_or(0);
    let mut sub = QuerySubscribe::default();
    sub.set_client(this);
    sub.set_port(our_port);
    let mut port_info = PortInfo::default();
    let mut client = -1;
    while seq.query_next_client(&mut client).is_ok() {
        if client < 0 {
            break;
        }
        if client == this {
            continue;
        }
        let mut port = -1;
        while seq.query_next_port(client, &mut port_info).is_ok() {
            if port < 0 {
                break;
            }
            if port_info.get_capability().contains(PortCap::READ) {
                sub.set_root(client, port);
                sub.set_subs(PortSubscribed::Addr);
                sub.set_queue(0, 0);
                let _ = seq.subscribe_port(&mut sub, SubscribeMode::Normal);
            }
        }
    }
}

#[cfg(feature = "midi-alsa")]
fn parse_alsa_event(ev: &alsa::seq::Event) -> Option<MidiEvent> {
    use alsa::seq::EventType;
    match ev.get_type() {
        EventType::Controller => {
            let data = ev.get_control().ok()?;
            Some(MidiEvent::Cc {
                channel: data.channel,
                controller: data.param,
                value: data.value,
            })
        }
        EventType::Noteon => {
            let data = ev.get_note().ok()?;
            if data.velocity == 0 {
                Some(MidiEvent::NoteOff {
                    channel: data.channel,
                    note: data.note,
                    velocity: 0,
                })
            } else {
                Some(MidiEvent::NoteOn {
                    channel: data.channel,
                    note: data.note,
                    velocity: data.velocity,
                })
            }
        }
        EventType::Noteoff => {
            let data = ev.get_note().ok()?;
            Some(MidiEvent::NoteOff {
                channel: data.channel,
                note: data.note,
                velocity: data.velocity,
            })
        }
        _ => None,
    }
}

fn open_alsa_reader() -> Option<AlsReader> {
    AlsReader::open()
}

static RUNTIME: once_cell::sync::OnceCell<Mutex<MidiRuntime>> = once_cell::sync::OnceCell::new();

pub fn runtime() -> Option<&'static Mutex<MidiRuntime>> {
    RUNTIME.get()
}

pub fn ensure_runtime(track_sink: TrackResolver, track_bus: TrackResolver) -> &'static Mutex<MidiRuntime> {
    RUNTIME.get_or_init(|| Mutex::new(MidiRuntime::new(track_sink, track_bus)))
}

pub fn apply_intent_global(intent: MidiIntent) {
    if let Some(rt) = RUNTIME.get() {
        if let Ok(r) = rt.lock() {
            r.send(intent);
        }
    }
}

pub fn poll_actions_global() -> Vec<MidiAction> {
    RUNTIME
        .get()
        .and_then(|rt| rt.lock().ok().map(|r| r.poll_actions()))
        .unwrap_or_default()
}

pub fn snapshot_global(session_devices: &[MidiDeviceInfo]) -> MidiSnapshot {
    let live = enumerate::enumerate_devices(session_devices);
    MidiSnapshot {
        devices: live,
        status: "MIDI".into(),
    }
}
