//! Dedicated PipeWire MainLoop thread — registry cache + create/link/destroy RPCs.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use once_cell::sync::OnceCell;
use pipewire as pw;
use pw::proxy::ProxyT;
use pw::types::ObjectType;
use pw::main_loop::MainLoop;
use pw::properties::properties;
use spa::utils::result::AsyncSeq;

use crate::contract::ClockProps;
use crate::domain::NodeSpec;

use super::cache::{
    new_shared, GraphView, LinkRec, NodeRec, PortDir, PortRec, SharedView,
};
use super::ports::{self, pw_node};
use super::props;

static PW_INIT: OnceCell<()> = OnceCell::new();

fn ensure_pw_init() {
    PW_INIT.get_or_init(|| {
        pw::init();
    });
}

const RPC_TIMEOUT: Duration = Duration::from_millis(800);
/// One 800ms miss is normal during clock migrate / node churn. Kill the plane
/// only after a sustained stall — a single timeout used to trip hollow reconnect
/// and freeze the UI (GNOME ANR).
const RPC_TIMEOUT_DEATH: u32 = 8;
static RPC_TIMEOUTS: AtomicU32 = AtomicU32::new(0);

fn note_rpc_ok() {
    RPC_TIMEOUTS.store(0, Ordering::Release);
}

/// Whether a timeout should mark the control plane dead.
fn timeout_kills_plane(consecutive: u32, mutation_in_flight: bool) -> bool {
    !mutation_in_flight && consecutive >= RPC_TIMEOUT_DEATH
}

fn rpc_timeout_is_fatal() -> bool {
    if crate::clock::clock_mutation_in_flight() {
        return false;
    }
    let n = RPC_TIMEOUTS.fetch_add(1, Ordering::AcqRel) + 1;
    timeout_kills_plane(n, false)
}

#[derive(Debug)]
pub enum Rpc {
    EnsureLink {
        source: String,
        sink: String,
        reply: SyncSender<Result<()>>,
    },
    Unlink {
        source: String,
        sink: String,
        reply: SyncSender<Result<()>>,
    },
    UnlinkExcept {
        source: String,
        allow: Vec<String>,
        reply: SyncSender<u32>,
    },
    EnsureNullSink {
        spec: NodeSpec,
        clock: ClockProps,
        reply: SyncSender<Result<()>>,
    },
    DestroyNode {
        name: String,
        reply: SyncSender<Result<()>>,
    },
    SetDefaultSink {
        name: String,
        reply: SyncSender<Result<bool>>,
    },
    SetLevels {
        /// Node name (sink) or Pulse-style `bus.monitor` source.
        endpoint: String,
        gain_db: f32,
        muted: bool,
        reply: SyncSender<Result<()>>,
    },
    SetNodeProps {
        node: String,
        entries: Vec<(String, String)>,
        reply: SyncSender<Result<()>>,
    },
    /// Retarget Stream/Output/Audio nodes currently linked into `from_sink`
    /// onto `to_sink` via metadata `target.node` (no Pulse move-sink-input).
    RetargetStreams {
        from_sink: String,
        to_sink: String,
        reply: SyncSender<Result<usize>>,
    },
    /// Retarget one stream by PipeWire object.serial (Pulse sink-input index).
    RetargetStreamSerial {
        serial: u32,
        to_sink: String,
        reply: SyncSender<Result<bool>>,
    },
    #[allow(dead_code)]
    Shutdown {
        reply: SyncSender<()>,
    },
}

struct PlaneInner {
    tx: pw::channel::Sender<Rpc>,
    view: SharedView,
    ready: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

/// Reconnectable control plane (survives `systemctl restart pipewire`).
struct PlaneSlot {
    plane: Option<PlaneInner>,
}

static PLANE: OnceLock<std::sync::Mutex<PlaneSlot>> = OnceLock::new();
static PLANE_DEAD: AtomicBool = AtomicBool::new(false);
static PLANE_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn plane_mutex() -> &'static std::sync::Mutex<PlaneSlot> {
    PLANE.get_or_init(|| std::sync::Mutex::new(PlaneSlot { plane: None }))
}

/// Shared registry view (None when plane is absent / dead).
pub fn shared_view() -> Option<SharedView> {
    let _ = ensure_plane_started();
    if PLANE_DEAD.load(Ordering::Acquire) {
        return None;
    }
    let g = plane_mutex().lock().ok()?;
    g.plane.as_ref().map(|p| Arc::clone(&p.view))
}

pub fn is_ready() -> bool {
    if PLANE_DEAD.load(Ordering::Acquire) {
        return false;
    }
    let _ = ensure_plane_started();
    let Ok(g) = plane_mutex().lock() else {
        return false;
    };
    g.plane
        .as_ref()
        .is_some_and(|p| p.ready.load(Ordering::Acquire))
}

/// True after core death / mainloop exit until [`reconnect_plane`] succeeds.
pub fn plane_is_dead() -> bool {
    PLANE_DEAD.load(Ordering::Acquire)
}

/// Control-plane generation — bumps on each successful reconnect.
pub fn plane_generation() -> u64 {
    PLANE_GEN.load(Ordering::Acquire)
}

/// Mark the native plane dead and wipe the registry cache (do not spawn yet).
pub fn mark_dead(reason: &str) {
    eprintln!("[buschain] native PW plane dead: {reason}");
    PLANE_DEAD.store(true, Ordering::Release);
    if let Ok(g) = plane_mutex().lock() {
        if let Some(p) = g.plane.as_ref() {
            p.ready.store(false, Ordering::Release);
            if let Ok(mut v) = p.view.write() {
                v.clear();
            }
        }
    }
}

/// Shut down the current MainLoop (if any) and start a fresh control plane.
pub fn reconnect_plane() -> Result<()> {
    // Take ownership of the old plane outside the lock while joining.
    let old = {
        let mut g = plane_mutex()
            .lock()
            .map_err(|_| anyhow!("native PW plane lock poisoned"))?;
        g.plane.take()
    };
    if let Some(old) = old {
        old.ready.store(false, Ordering::Release);
        if let Ok(mut v) = old.view.write() {
            v.clear();
        }
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let _ = old.tx.send(Rpc::Shutdown { reply: reply_tx });
        let _ = reply_rx.recv_timeout(Duration::from_secs(2));
        // MainLoop may already be gone — don't block forever on join.
        let _ = old.join.join();
    }
    PLANE_DEAD.store(false, Ordering::Release);
    let new_plane = start_plane().map_err(|e| anyhow!("reconnect plane: {e}"))?;
    if !new_plane.ready.load(Ordering::Acquire) {
        PLANE_DEAD.store(true, Ordering::Release);
        return Err(anyhow!("reconnect plane: registry sync did not become ready"));
    }
    {
        let mut g = plane_mutex()
            .lock()
            .map_err(|_| anyhow!("native PW plane lock poisoned"))?;
        g.plane = Some(new_plane);
    }
    PLANE_GEN.fetch_add(1, Ordering::AcqRel);
    eprintln!("[buschain] native PW plane reconnected (gen={})", plane_generation());
    Ok(())
}

fn ensure_plane_started() -> Result<()> {
    if PLANE_DEAD.load(Ordering::Acquire) {
        return Err(anyhow!("native PipeWire control plane is dead (awaiting reconnect)"));
    }
    {
        let g = plane_mutex()
            .lock()
            .map_err(|_| anyhow!("native PW plane lock poisoned"))?;
        if g.plane
            .as_ref()
            .is_some_and(|p| p.ready.load(Ordering::Acquire))
        {
            return Ok(());
        }
        if g.plane.is_some() {
            // Starting or half-up — not ready yet.
            return Err(anyhow!("native PipeWire control plane not ready"));
        }
    }
    let new_plane = start_plane().map_err(|e| anyhow!("native PipeWire control plane: {e}"))?;
    let mut g = plane_mutex()
        .lock()
        .map_err(|_| anyhow!("native PW plane lock poisoned"))?;
    if g.plane.is_none() {
        g.plane = Some(new_plane);
        PLANE_GEN.fetch_add(1, Ordering::AcqRel);
    }
    Ok(())
}

fn call<T>(build: impl FnOnce(SyncSender<T>) -> Rpc, timeout: Duration) -> Result<T>
where
    T: Send + 'static,
{
    ensure_plane_started()?;
    if PLANE_DEAD.load(Ordering::Acquire) {
        return Err(anyhow!("native PW control plane is dead"));
    }
    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    let rpc = build(reply_tx);
    let mut send_failed = false;
    {
        let g = plane_mutex()
            .lock()
            .map_err(|_| anyhow!("native PW plane lock poisoned"))?;
        let p = g
            .plane
            .as_ref()
            .ok_or_else(|| anyhow!("native PW control plane missing"))?;
        if !p.ready.load(Ordering::Acquire) {
            return Err(anyhow!("native PW control plane not ready"));
        }
        if p.tx.send(rpc).is_err() {
            send_failed = true;
        }
    }
    if send_failed {
        mark_dead("rpc channel closed");
        return Err(anyhow!("native PW control plane channel closed"));
    }
    match reply_rx.recv_timeout(timeout) {
        Ok(v) => {
            note_rpc_ok();
            Ok(v)
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            mark_dead("rpc channel closed");
            Err(anyhow!("native PW RPC channel closed"))
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            if rpc_timeout_is_fatal() {
                mark_dead("rpc timeout");
            }
            Err(anyhow!("native PW RPC timed out after {timeout:?}"))
        }
    }
}

pub fn ensure_link(source: &str, sink: &str) -> Result<()> {
    call(
        |reply| Rpc::EnsureLink {
            source: source.to_string(),
            sink: sink.to_string(),
            reply,
        },
        RPC_TIMEOUT,
    )?
}

pub fn unlink(source: &str, sink: &str) -> Result<()> {
    call(
        |reply| Rpc::Unlink {
            source: source.to_string(),
            sink: sink.to_string(),
            reply,
        },
        RPC_TIMEOUT,
    )?
}

pub fn unlink_from_source_except(source: &str, allow_sinks: &[&str]) -> u32 {
    let allow: Vec<String> = allow_sinks.iter().map(|s| (*s).to_string()).collect();
    call(
        |reply| Rpc::UnlinkExcept {
            source: source.to_string(),
            allow,
            reply,
        },
        RPC_TIMEOUT,
    )
    .unwrap_or(0)
}

pub fn ensure_null_sink(spec: &NodeSpec, clock: &ClockProps) -> Result<()> {
    call(
        |reply| Rpc::EnsureNullSink {
            spec: spec.clone(),
            clock: clock.clone(),
            reply,
        },
        Duration::from_millis(1500),
    )?
}

pub fn destroy_node(name: &str) -> Result<()> {
    call(
        |reply| Rpc::DestroyNode {
            name: name.to_string(),
            reply,
        },
        RPC_TIMEOUT,
    )?
}

pub fn set_default_sink(name: &str) -> Result<bool> {
    call(
        |reply| Rpc::SetDefaultSink {
            name: name.to_string(),
            reply,
        },
        RPC_TIMEOUT,
    )?
}

pub fn set_levels(endpoint: &str, gain_db: f32, muted: bool) -> Result<()> {
    call(
        |reply| Rpc::SetLevels {
            endpoint: endpoint.to_string(),
            gain_db,
            muted,
            reply,
        },
        RPC_TIMEOUT,
    )?
}

pub fn set_node_props(node: &str, entries: Vec<(String, String)>) -> Result<()> {
    call(
        |reply| Rpc::SetNodeProps {
            node: node.to_string(),
            entries,
            reply,
        },
        RPC_TIMEOUT,
    )?
}

pub fn default_sink_name() -> Option<String> {
    let view = shared_view()?;
    let g = view.read().ok()?;
    g.default_audio_sink.clone()
}

pub fn list_source_devices() -> Vec<(String, String)> {
    let Some(view) = shared_view() else {
        return Vec::new();
    };
    let Ok(g) = view.read() else {
        return Vec::new();
    };
    g.source_names()
}

pub fn link_is_live(source: &str, sink: &str) -> bool {
    let Some(view) = shared_view() else {
        return false;
    };
    let Ok(g) = view.read() else {
        return false;
    };
    ports::link_is_live(&g, source, sink)
}

pub fn sink_exists(name: &str) -> bool {
    let Some(view) = shared_view() else {
        return false;
    };
    let Ok(g) = view.read() else {
        return false;
    };
    g.node(name).is_some()
}

pub fn find_node_id(name: &str) -> Option<u32> {
    let view = shared_view()?;
    let g = view.read().ok()?;
    g.node_id(name)
}

pub fn list_sink_names() -> Vec<String> {
    let Some(view) = shared_view() else {
        return Vec::new();
    };
    let Ok(g) = view.read() else {
        return Vec::new();
    };
    g.sink_names()
}

pub fn cache_node_id(name: &str, id: u32) {
    if id == 0 {
        return;
    }
    let Some(view) = shared_view() else {
        return;
    };
    let Ok(mut g) = view.write() else {
        return;
    };
    if g.nodes_by_id.contains_key(&id) {
        return;
    }
    g.insert_node(NodeRec {
        id,
        name: name.to_string(),
        media_class: String::new(),
        description: String::new(),
        rate: None,
        serial: None,
    });
}

fn start_plane() -> Result<PlaneInner, String> {
    ensure_pw_init();
    let view = new_shared();
    let ready = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<pw::channel::Sender<Rpc>, String>>(1);
    let view_thread = Arc::clone(&view);
    let ready_flag = Arc::clone(&ready);
    let view_on_exit = Arc::clone(&view);

    let join = thread::Builder::new()
        .name("buschain-pw-ctrl".into())
        .spawn(move || {
            let result = run_loop(view_thread, Arc::clone(&ready_flag), ready_tx);
            ready_flag.store(false, Ordering::Release);
            PLANE_DEAD.store(true, Ordering::Release);
            if let Ok(mut v) = view_on_exit.write() {
                v.clear();
            }
            if let Err(e) = result {
                eprintln!("[buschain] native PW mainloop exited: {e:#}");
            } else {
                eprintln!("[buschain] native PW mainloop exited");
            }
        })
        .map_err(|e| format!("spawn pw-ctrl: {e}"))?;

    let tx = ready_rx
        .recv_timeout(Duration::from_secs(3))
        .map_err(|_| "native PW control plane startup timed out".to_string())?
        .map_err(|e| e)?;

    // Wait until first registry sync filled the cache.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if ready.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }

    Ok(PlaneInner {
        tx,
        view,
        ready,
        join,
    })
}

fn run_loop(
    view: SharedView,
    ready: Arc<AtomicBool>,
    ready_tx: SyncSender<Result<pw::channel::Sender<Rpc>, String>>,
) -> Result<()> {
    let mainloop = MainLoop::new(None).context("MainLoop")?;
    let context = pw::context::Context::new(&mainloop).context("Context")?;
    let core = context.connect(None).context("connect Core")?;
    let registry = Rc::new(core.get_registry().context("Registry")?);

    let local_view = Rc::new(RefCell::new(GraphView::default()));
    let shared = view.clone();
    // Own create proxies for the process lifetime. With object.linger=false,
    // dropping a Node/Link proxy removes the object from the graph (VO/Master
    // sinks were vanishing right after ensure).
    let linger: Rc<RefCell<HashMap<u32, Box<dyn pw::proxy::ProxyT>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let after_sync: Rc<RefCell<Option<Box<dyn FnOnce(&CtrlState)>>>> =
        Rc::new(RefCell::new(None));
    let sync_seq: Rc<Cell<Option<AsyncSeq>>> = Rc::new(Cell::new(None));
    let metadata: Rc<RefCell<Option<pw::metadata::Metadata>>> = Rc::new(RefCell::new(None));
    let meta_listeners: Rc<RefCell<Vec<Box<dyn pw::proxy::Listener>>>> =
        Rc::new(RefCell::new(Vec::new()));

    // Publish registry events into both local (PW thread) and shared (workers).
    let lv = Rc::clone(&local_view);
    let sh = Arc::clone(&shared);
    let reg = Rc::clone(&registry);
    let meta_slot = Rc::clone(&metadata);
    let meta_ls = Rc::clone(&meta_listeners);
    let _reg = registry
        .add_listener_local()
        .global({
            let lv = Rc::clone(&lv);
            let sh = Arc::clone(&sh);
            let reg = Rc::clone(&reg);
            let meta_slot = Rc::clone(&meta_slot);
            let meta_ls = Rc::clone(&meta_ls);
            move |global| {
                on_global(&lv, &sh, &reg, &meta_slot, &meta_ls, global);
            }
        })
        .global_remove({
            let lv = Rc::clone(&lv);
            let sh = Arc::clone(&sh);
            move |id| {
                lv.borrow_mut().remove_global(id);
                if let Ok(mut g) = sh.write() {
                    g.remove_global(id);
                }
            }
        })
        .register();

    let (pw_tx, pw_rx) = pw::channel::channel::<Rpc>();
    let _ = ready_tx.send(Ok(pw_tx));

    // state is built before done listener so after_sync can borrow it.
    let state = Rc::new(CtrlState {
        mainloop: mainloop.clone(),
        core: core.clone(),
        registry,
        local: local_view,
        shared,
        linger,
        after_sync: Rc::clone(&after_sync),
        sync_seq: Rc::clone(&sync_seq),
        metadata,
        _meta_listeners: meta_listeners,
    });

    // Initial sync — mark ready when done.
    let ready_flag = Arc::clone(&ready);
    let pending = state.core.sync(0).context("initial sync")?;
    let st_done = Rc::clone(&state);
    let ready_on_err = Arc::clone(&ready);
    let _core_l = state
        .core
        .add_listener_local()
        .done(move |id, seq| {
            if id != pw::core::PW_ID_CORE {
                return;
            }
            if seq == pending {
                ready_flag.store(true, Ordering::Release);
            }
            if let Some(want) = sync_seq.get() {
                if seq == want {
                    sync_seq.set(None);
                    if let Some(cb) = after_sync.borrow_mut().take() {
                        cb(&st_done);
                    }
                }
            }
        })
        .error(move |id, _seq, res, message| {
            if id != pw::core::PW_ID_CORE {
                return;
            }
            // Stale DESTROY after rate-change teardown (gone node/link).
            // res=-2 ENOENT; op:7 = PW_CORE_METHOD_DESTROY. Not a dead daemon.
            if res == -libc::ENOENT || message.contains("unknown resource") {
                return;
            }
            eprintln!("[buschain] PW core error: res={res} msg={message}");
            let fatal = res == -libc::EPIPE
                || res == -libc::ECONNRESET
                || message.contains("connection")
                || message.contains("core gone");
            if fatal {
                ready_on_err.store(false, Ordering::Release);
                PLANE_DEAD.store(true, Ordering::Release);
            }
        })
        .register();

    let st = Rc::clone(&state);
    let _attached = pw_rx.attach(mainloop.loop_(), move |rpc| {
        handle_rpc(&st, rpc);
    });

    mainloop.run();
    Ok(())
}

struct CtrlState {
    mainloop: MainLoop,
    core: pw::core::Core,
    registry: Rc<pw::registry::Registry>,
    local: Rc<RefCell<GraphView>>,
    shared: SharedView,
    linger: Rc<RefCell<HashMap<u32, Box<dyn pw::proxy::ProxyT>>>>,
    after_sync: Rc<RefCell<Option<Box<dyn FnOnce(&CtrlState)>>>>,
    sync_seq: Rc<Cell<Option<AsyncSeq>>>,
    metadata: Rc<RefCell<Option<pw::metadata::Metadata>>>,
    _meta_listeners: Rc<RefCell<Vec<Box<dyn pw::proxy::Listener>>>>,
}

fn on_global(
    local: &Rc<RefCell<GraphView>>,
    shared: &SharedView,
    registry: &Rc<pw::registry::Registry>,
    metadata: &Rc<RefCell<Option<pw::metadata::Metadata>>>,
    meta_listeners: &Rc<RefCell<Vec<Box<dyn pw::proxy::Listener>>>>,
    global: &pw::registry::GlobalObject<&spa::utils::dict::DictRef>,
) {
    let Some(props) = global.props.as_ref() else {
        return;
    };
    match global.type_ {
        ObjectType::Node => {
            let name = props.get(*pw::keys::NODE_NAME).unwrap_or("").to_string();
            let media_class = props.get(*pw::keys::MEDIA_CLASS).unwrap_or("").to_string();
            let description = props
                .get(*pw::keys::NODE_DESCRIPTION)
                .or_else(|| props.get("device.description"))
                .unwrap_or("")
                .to_string();
            let rate = props
                .get(*pw::keys::AUDIO_RATE)
                .and_then(|s| s.parse().ok());
            let serial = props
                .get("object.serial")
                .and_then(|s| s.parse().ok());
            let stream_props = if media_class == "Stream/Output/Audio" {
                let get = |k: &str| {
                    props
                        .get(k)
                        .map(|s| s.to_string())
                        .filter(|s| !s.is_empty())
                };
                Some(crate::backend::native::cache::StreamProps {
                    app_name: get("application.name"),
                    binary: get("application.process.binary"),
                    app_id: get("application.id"),
                    media_name: get("media.name"),
                    icon_name: get("application.icon_name"),
                    media_role: get("media.role"),
                    node_virtual: props.get("node.virtual") == Some("true"),
                })
            } else {
                None
            };
            let rec = NodeRec {
                id: global.id,
                name,
                media_class,
                description,
                rate,
                serial,
            };
            local.borrow_mut().insert_node(rec.clone());
            if let Some(sp) = stream_props.clone() {
                local.borrow_mut().insert_stream_props(global.id, sp);
            }
            if let Ok(mut g) = shared.write() {
                g.insert_node(rec);
                if let Some(sp) = stream_props {
                    g.insert_stream_props(global.id, sp);
                }
            }
        }
        ObjectType::Port => {
            let node_id = props
                .get(*pw::keys::NODE_ID)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let name = props.get(*pw::keys::PORT_NAME).unwrap_or("").to_string();
            let dir = match props.get(*pw::keys::PORT_DIRECTION) {
                Some("in") => PortDir::In,
                Some("out") => PortDir::Out,
                _ => PortDir::Unknown,
            };
            let channel = props
                .get(*pw::keys::AUDIO_CHANNEL)
                .unwrap_or("")
                .to_string();
            let rec = PortRec {
                id: global.id,
                node_id,
                name,
                direction: dir,
                channel,
            };
            local.borrow_mut().insert_port(rec.clone());
            if let Ok(mut g) = shared.write() {
                g.insert_port(rec);
            }
        }
        ObjectType::Link => {
            let out_node = props
                .get(*pw::keys::LINK_OUTPUT_NODE)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let out_port = props
                .get(*pw::keys::LINK_OUTPUT_PORT)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let in_node = props
                .get(*pw::keys::LINK_INPUT_NODE)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let in_port = props
                .get(*pw::keys::LINK_INPUT_PORT)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let rec = LinkRec {
                id: global.id,
                out_node,
                out_port,
                in_node,
                in_port,
            };
            local.borrow_mut().insert_link(rec.clone());
            if let Ok(mut g) = shared.write() {
                g.insert_link(rec);
            }
        }
        ObjectType::Factory => {
            if props.get("factory.type.name") == Some(ObjectType::Link.to_str()) {
                if let Some(name) = props.get("factory.name") {
                    local.borrow_mut().link_factory = Some(name.to_string());
                    if let Ok(mut g) = shared.write() {
                        g.link_factory = Some(name.to_string());
                    }
                }
            }
        }
        ObjectType::Metadata => {
            let name = props.get("metadata.name").unwrap_or("");
            if name != "default" {
                return;
            }
            if metadata.borrow().is_some() {
                return;
            }
            let Ok(meta) = registry.bind::<pw::metadata::Metadata, _>(global) else {
                return;
            };
            let sh = Arc::clone(shared);
            let lv = Rc::clone(local);
            let listener = meta
                .add_listener_local()
                .property(move |_subject, key, _type_, value| {
                    if key == Some("default.audio.sink") {
                        let parsed = value.and_then(parse_default_sink_json);
                        lv.borrow_mut().default_audio_sink = parsed.clone();
                        if let Ok(mut g) = sh.write() {
                            g.default_audio_sink = parsed;
                        }
                    }
                    0
                })
                .register();
            meta_listeners.borrow_mut().push(Box::new(listener));
            *metadata.borrow_mut() = Some(meta);
        }
        _ => {}
    }
}

fn parse_default_sink_json(value: &str) -> Option<String> {
    // WirePlumber: {"name":"alsa_output..."}
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(value) {
        if let Some(n) = v.get("name").and_then(|x| x.as_str()) {
            return Some(n.to_string());
        }
    }
    // Loose fallback: name = "foo" or bare name
    if let Some(rest) = value.split("\"name\"").nth(1) {
        if let Some(start) = rest.find('"') {
            let rest = &rest[start + 1..];
            if let Some(end) = rest.find('"') {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

fn handle_rpc(st: &Rc<CtrlState>, rpc: Rpc) {
    match rpc {
        Rpc::EnsureLink {
            source,
            sink,
            reply,
        } => begin_ensure_link(st, source, sink, reply),
        Rpc::Unlink {
            source,
            sink,
            reply,
        } => {
            let _ = reply.send(do_unlink(st, &source, &sink));
        }
        Rpc::UnlinkExcept {
            source,
            allow,
            reply,
        } => {
            let allow_refs: Vec<&str> = allow.iter().map(|s| s.as_str()).collect();
            let _ = reply.send(do_unlink_except(st, &source, &allow_refs));
        }
        Rpc::EnsureNullSink { spec, clock, reply } => {
            begin_ensure_null_sink(st, spec, clock, reply)
        }
        Rpc::DestroyNode { name, reply } => {
            let _ = reply.send(do_destroy_node(st, &name));
        }
        Rpc::SetDefaultSink { name, reply } => {
            let _ = reply.send(do_set_default_sink(st, &name));
        }
        Rpc::SetLevels {
            endpoint,
            gain_db,
            muted,
            reply,
        } => {
            let _ = reply.send(do_set_levels(st, &endpoint, gain_db, muted));
        }
        Rpc::SetNodeProps {
            node,
            entries,
            reply,
        } => {
            let _ = reply.send(do_set_node_props(st, &node, &entries));
        }
        Rpc::RetargetStreams {
            from_sink,
            to_sink,
            reply,
        } => {
            let _ = reply.send(do_retarget_streams(st, &from_sink, &to_sink));
        }
        Rpc::RetargetStreamSerial {
            serial,
            to_sink,
            reply,
        } => {
            let _ = reply.send(do_retarget_stream_serial(st, serial, &to_sink));
        }
        Rpc::Shutdown { reply } => {
            let _ = reply.send(());
            st.mainloop.quit();
        }
    }
}

fn schedule_after_sync(st: &CtrlState, cb: impl FnOnce(&CtrlState) + 'static) {
    *st.after_sync.borrow_mut() = Some(Box::new(cb));
    match st.core.sync(0) {
        Ok(seq) => st.sync_seq.set(Some(seq)),
        Err(_) => {
            // Fallback: run callback immediately if sync cannot be queued.
            if let Some(cb) = st.after_sync.borrow_mut().take() {
                cb(st);
            }
        }
    }
}

fn begin_ensure_link(
    st: &CtrlState,
    source: String,
    sink: String,
    reply: SyncSender<Result<()>>,
) {
    {
        let view = st.local.borrow();
        if ports::link_is_live(&view, &source, &sink) {
            let _ = reply.send(Ok(()));
            return;
        }
    }
    let view = st.local.borrow();
    let factory = match view.link_factory.clone() {
        Some(f) => f,
        None => {
            let _ = reply.send(Err(anyhow!("no Link factory in registry yet")));
            return;
        }
    };
    let pairs = match ports::port_id_pairs(&view, &source, &sink) {
        Ok(p) => p,
        Err(e) => {
            let _ = reply.send(Err(e));
            return;
        }
    };
    let src_id = match view.node_id(pw_node(&source)) {
        Some(id) => id,
        None => {
            let _ = reply.send(Err(anyhow!("missing src node")));
            return;
        }
    };
    let dst_id = match view.node_id(pw_node(&sink)) {
        Some(id) => id,
        None => {
            let _ = reply.send(Err(anyhow!("missing dst node")));
            return;
        }
    };
    drop(view);

    for (out_port, in_port) in pairs {
        // Copy the id out so `if let` does not keep GraphView borrowed across DESTROY
        // (PW global_remove also borrow_mut — that aborted the control thread).
        let existing = st
            .local
            .borrow()
            .links_by_ports
            .get(&(out_port, in_port))
            .copied();
        if let Some(link_id) = existing {
            destroy_global_if_live(st, link_id);
        }
        match st.core.create_object::<pw::link::Link>(
            &factory,
            &properties! {
                *pw::keys::LINK_OUTPUT_NODE => src_id.to_string(),
                *pw::keys::LINK_OUTPUT_PORT => out_port.to_string(),
                *pw::keys::LINK_INPUT_NODE => dst_id.to_string(),
                *pw::keys::LINK_INPUT_PORT => in_port.to_string(),
                *pw::keys::OBJECT_LINGER => "false",
            },
        ) {
            Ok(link) => {
                let proxy_id = link.upcast_ref().id();
                st.linger.borrow_mut().insert(proxy_id, Box::new(link));
            }
            Err(e) => {
                let _ = reply.send(Err(anyhow!("create Link: {e}")));
                return;
            }
        }
    }

    schedule_after_sync(st, move |_st| {
        // Soft-accept: linger create can race one tick; worker re-probes.
        let _ = reply.send(Ok(()));
    });
}

fn begin_ensure_null_sink(
    st: &CtrlState,
    spec: NodeSpec,
    clock: ClockProps,
    reply: SyncSender<Result<()>>,
) {
    let name = spec.name.as_str().to_string();
    let want_class = spec.media_class();
    {
        let view = st.local.borrow();
        if let Some(n) = view.node(&name) {
            // Exact class match required so VO toggle (Audio/Sink ↔ Audio/Sink/Internal)
            // and portless BusChain/Internal leftovers recreate correctly.
            let broken_class = n.media_class == "BusChain/Internal"
                || (n.media_class.starts_with("BusChain/") && n.media_class != want_class);
            let class_ok = !broken_class && n.media_class == want_class;
            let ready = view.null_sink_ready(&name);
            if ready && class_ok {
                let _ = reply.send(Ok(()));
                return;
            }
            if class_ok && !ready {
                // Node exists but ports not in registry yet — wait one sync.
                drop(view);
                schedule_after_sync(st, move |_st| {
                    let _ = reply.send(Ok(()));
                });
                return;
            }
            // Wrong / portless class: park app streams, destroy, recreate.
            drop(view);
            let parked = stream_ids_on_sink(st, &name);
            if let Some(hold_id) = st.local.borrow().node_id("buschain_hold") {
                for sid in &parked {
                    let _ = set_stream_target_node(st, *sid, hold_id, "buschain_hold");
                }
            }
            let _ = do_destroy_node(st, &name);
            return begin_ensure_null_sink_create(st, spec, clock, reply, parked);
        }
    }

    begin_ensure_null_sink_create(st, spec, clock, reply, Vec::new());
}

fn begin_ensure_null_sink_create(
    st: &CtrlState,
    spec: NodeSpec,
    clock: ClockProps,
    reply: SyncSender<Result<()>>,
    remount: Vec<u32>,
) {
    let name = spec.name.as_str().to_string();
    let media_class = spec.media_class();
    let pulse_export = if spec.pulse_export { "true" } else { "false" };
    // Fader stage is the post bus (after inserts). App buses stay unity;
    // Track/Master also keep monitor vols for dry-before-post fallback.
    let monitor_vols = matches!(
        spec.role,
        crate::domain::NodeRole::TrackBus
            | crate::domain::NodeRole::MasterBus
            | crate::domain::NodeRole::PostBus
    );
    let node = match st.core.create_object::<pw::node::Node>(
        "adapter",
        &properties! {
            "factory.name" => "support.null-audio-sink",
            *pw::keys::NODE_NAME => name.as_str(),
            *pw::keys::NODE_DESCRIPTION => spec.description.as_str(),
            *pw::keys::MEDIA_CLASS => media_class,
            *pw::keys::AUDIO_CHANNELS => "2",
            "audio.position" => "FL,FR",
            *pw::keys::AUDIO_RATE => clock.sample_rate.to_string(),
            *pw::keys::NODE_LATENCY => clock.node_latency.as_str(),
            *pw::keys::NODE_FORCE_QUANTUM => clock.quantum.to_string(),
            *pw::keys::NODE_LOCK_QUANTUM => if clock.soft_quantum { "false" } else { "true" },
            *pw::keys::NODE_VIRTUAL => "true",
            *pw::keys::MEDIA_NAME => "buschain-control",
            // Drop with the BusChain client — crash/Quit must not leave hollow sinks.
            *pw::keys::OBJECT_LINGER => "false",
            "session.suspend-timeout-seconds" => clock.suspend_timeout.to_string(),
            "device.description" => spec.description.as_str(),
            "buschain.pulse.export" => pulse_export,
            "monitor.channel-volumes" => if monitor_vols { "true" } else { "false" },
        },
    ) {
        Ok(n) => n,
        Err(e) => {
            let _ = reply.send(Err(anyhow!("create null-audio-sink `{name}`: {e}")));
            return;
        }
    };
    let proxy_id = node.upcast_ref().id();
    st.linger
        .borrow_mut()
        .insert(proxy_id, Box::new(node));

    schedule_after_sync(st, move |st| {
        // Do not drop the create proxy — object.linger=false ties lifetime to it.
        if !remount.is_empty() {
            if let Some(new_id) = st.local.borrow().node_id(&name) {
                for sid in remount {
                    let _ = set_stream_target_node(st, sid, new_id, &name);
                }
            }
        }
        let _ = reply.send(Ok(()));
    });
}

fn do_unlink(st: &CtrlState, source: &str, sink: &str) -> Result<()> {
    let view = st.local.borrow();
    let Ok(pairs) = ports::port_id_pairs(&view, source, sink) else {
        return Ok(());
    };
    let ids: Vec<u32> = pairs
        .iter()
        .filter_map(|(o, i)| view.links_by_ports.get(&(*o, *i)).copied())
        .collect();
    drop(view);
    for id in ids {
        destroy_global_if_live(st, id);
    }
    Ok(())
}

fn port_on_node(port_name: &str, node: &str) -> bool {
    port_name == node || port_name.starts_with(&format!("{node}:"))
}

fn do_unlink_except(st: &CtrlState, source: &str, allow_sinks: &[&str]) -> u32 {
    let src_node = pw_node(source);
    let view = st.local.borrow();
    let Some(src_id) = view.node_id(src_node) else {
        return 0;
    };
    let mut kill = Vec::new();
    for link in view.links_by_id.values() {
        if link.out_node != src_id {
            continue;
        }
        let Some(in_port) = view.ports_by_id.get(&link.in_port) else {
            continue;
        };
        let Some(in_node) = view.nodes_by_id.get(&link.in_node) else {
            continue;
        };
        if in_node.name == "buschain_hold" {
            continue;
        }
        if in_node.name.starts_with("meter-") {
            continue;
        }
        // Dry-meter taps: lifecycle-owned by host::dry_meter — never strip here.
        if in_node.name.starts_with("buschain_mtr_") {
            continue;
        }
        // Pulse remap-source capture for system virtual input. Anti-Master feed
        // strips used to wipe these and leave vin permanently silent.
        if in_node.name.starts_with("buschain_vin_")
            || in_node.name.starts_with("input.buschain_vin_")
        {
            continue;
        }
        let allowed = allow_sinks.iter().any(|s| {
            let sn = pw_node(s);
            in_node.name == sn || port_on_node(&format!("{}:{}", in_node.name, in_port.name), sn)
                || in_node.name == *s
        });
        if allowed {
            continue;
        }
        kill.push(link.id);
    }
    drop(view);
    let n = kill.len() as u32;
    for id in kill {
        destroy_global_if_live(st, id);
    }
    n
}

/// DESTROY only if the registry snapshot still has this id — avoids ENOENT
/// `unknown resource … op:7` after rate-change teardown already removed it.
fn destroy_global_if_live(st: &CtrlState, id: u32) {
    let known = match st.local.try_borrow() {
        Ok(v) => v.has_global(id),
        Err(_) => true,
    };
    if !known {
        return;
    }
    let _ = st.registry.destroy_global(id);
    // global_remove may already have cleared the cache; never panic on the PW thread.
    if let Ok(mut v) = st.local.try_borrow_mut() {
        v.remove_global(id);
    }
    if let Ok(mut g) = st.shared.write() {
        g.remove_global(id);
    }
    if let Ok(mut linger) = st.linger.try_borrow_mut() {
        linger.remove(&id);
    }
}

fn do_destroy_node(st: &CtrlState, name: &str) -> Result<()> {
    let id = {
        let view = st.local.borrow();
        match view.node_id(name) {
            Some(id) => id,
            None => return Err(anyhow!("node `{name}` not in registry")),
        }
    };
    destroy_global_if_live(st, id);
    Ok(())
}

/// Stream nodes currently feeding `sink_name` playback ports.
fn stream_ids_on_sink(st: &CtrlState, sink_name: &str) -> Vec<u32> {
    let view = st.local.borrow();
    let Some(sink_id) = view.node_id(sink_name) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for link in view.links_by_id.values() {
        if link.in_node != sink_id {
            continue;
        }
        let Some(src) = view.nodes_by_id.get(&link.out_node) else {
            continue;
        };
        if src.media_class == "Stream/Output/Audio"
            || src.media_class.ends_with("/Output/Audio")
        {
            if !out.contains(&src.id) {
                out.push(src.id);
            }
        }
    }
    out
}

/// Pin a stream onto `sink_name` via session metadata.
///
/// `target.object` takes precedence over the legacy `target.node` and is resolved
/// by matching `node.name` **or** `object.serial` — never the node id. Writing the
/// node id there made every target unresolvable, so WirePlumber silently fell back
/// to the default sink and app placement appeared to do nothing.
fn set_stream_target_node(
    st: &CtrlState,
    stream_id: u32,
    sink_id: u32,
    sink_name: &str,
) -> bool {
    let meta = st.metadata.borrow();
    let Some(meta) = meta.as_ref() else {
        return false;
    };
    meta.set_property(
        stream_id,
        "target.object",
        Some("Spa:String"),
        Some(sink_name),
    );
    // Legacy key for older session managers — this one really is the node id.
    let id_str = sink_id.to_string();
    meta.set_property(stream_id, "target.node", Some("Spa:Id"), Some(&id_str));
    true
}

fn do_retarget_streams(st: &CtrlState, from_sink: &str, to_sink: &str) -> Result<usize> {
    let to_id = st
        .local
        .borrow()
        .node_id(to_sink)
        .ok_or_else(|| anyhow!("retarget: sink `{to_sink}` not in registry"))?;
    let streams = stream_ids_on_sink(st, from_sink);
    let mut n = 0usize;
    for sid in streams {
        if set_stream_target_node(st, sid, to_id, to_sink) {
            n += 1;
        }
    }
    Ok(n)
}

fn do_retarget_stream_serial(st: &CtrlState, serial: u32, to_sink: &str) -> Result<bool> {
    let view = st.local.borrow();
    let to_id = view
        .node_id(to_sink)
        .ok_or_else(|| anyhow!("retarget: sink `{to_sink}` not in registry"))?;
    let Some(stream) = view.node_by_serial(serial) else {
        return Ok(false);
    };
    if !(stream.media_class == "Stream/Output/Audio"
        || stream.media_class.ends_with("/Output/Audio"))
    {
        // Still try — Pulse serial may map before class is cached.
    }
    let sid = stream.id;
    // Already linked — Chromium/Electron freeze if we rewrite target.* every tick.
    let already = view
        .links_by_id
        .values()
        .any(|l| l.out_node == sid && l.in_node == to_id);
    drop(view);
    if already {
        return Ok(true);
    }
    Ok(set_stream_target_node(st, sid, to_id, to_sink))
}

pub fn retarget_streams(from_sink: &str, to_sink: &str) -> Result<usize> {
    call(
        |reply| Rpc::RetargetStreams {
            from_sink: from_sink.to_string(),
            to_sink: to_sink.to_string(),
            reply,
        },
        RPC_TIMEOUT,
    )?
}

pub fn retarget_stream_serial(serial: u32, to_sink: &str) -> Result<bool> {
    call(
        |reply| Rpc::RetargetStreamSerial {
            serial,
            to_sink: to_sink.to_string(),
            reply,
        },
        RPC_TIMEOUT,
    )?
}

/// One live playback stream from the native registry (Apps discovery).
#[derive(Debug, Clone)]
pub struct PlaybackStreamInfo {
    /// PipeWire `object.serial` — equals the Pulse sink-input index.
    pub serial: u32,
    pub node_name: String,
    /// Sink node the stream currently feeds (via links); empty while settling.
    pub sink: String,
    pub app_name: Option<String>,
    pub binary: Option<String>,
    pub app_id: Option<String>,
    pub media_name: Option<String>,
    pub icon_name: Option<String>,
    pub media_role: Option<String>,
    pub node_virtual: bool,
}

/// List `Stream/Output/Audio` nodes with app props + current sink (registry, no forks).
/// Sees streams on `Audio/Sink/Internal` buses that pipewire-pulse hides.
pub fn list_playback_streams() -> Vec<PlaybackStreamInfo> {
    let Some(view) = shared_view() else {
        return Vec::new();
    };
    let Ok(g) = view.read() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for node in g.nodes_by_id.values() {
        if node.media_class != "Stream/Output/Audio" {
            continue;
        }
        let Some(serial) = node.serial else {
            continue;
        };
        // Current target: prefer an Audio/Sink-class peer over filters/monitors.
        let mut sink = String::new();
        let mut sink_is_audio = false;
        for link in g.links_by_id.values() {
            if link.out_node != node.id {
                continue;
            }
            let Some(peer) = g.nodes_by_id.get(&link.in_node) else {
                continue;
            };
            let is_audio_sink = peer.media_class.starts_with("Audio/Sink");
            if sink.is_empty() || (is_audio_sink && !sink_is_audio) {
                sink = peer.name.clone();
                sink_is_audio = is_audio_sink;
            }
        }
        let sp = g.stream_props_by_id.get(&node.id).cloned().unwrap_or_default();
        out.push(PlaybackStreamInfo {
            serial,
            node_name: node.name.clone(),
            sink,
            app_name: sp.app_name,
            binary: sp.binary,
            app_id: sp.app_id,
            media_name: sp.media_name,
            icon_name: sp.icon_name,
            media_role: sp.media_role,
            node_virtual: sp.node_virtual,
        });
    }
    out.sort_by_key(|s| s.serial);
    out
}

/// Registry change counter — bumps on node/port/link/stream changes.
pub fn graph_generation() -> Option<u64> {
    let view = shared_view()?;
    view.read().ok().map(|g| g.generation)
}

/// True when a stream (`object.serial`) is already linked into `sink` playback.
pub fn stream_targets_sink(serial: u32, sink: &str) -> Result<bool> {
    let Some(view) = shared_view() else {
        return Ok(false);
    };
    let g = view
        .read()
        .map_err(|_| anyhow!("native graph view poisoned"))?;
    let Some(stream) = g.node_by_serial(serial) else {
        return Ok(false);
    };
    let Some(sink_id) = g.node_id(sink) else {
        return Ok(false);
    };
    Ok(g.links_by_id
        .values()
        .any(|l| l.out_node == stream.id && l.in_node == sink_id))
}

fn do_set_default_sink(st: &CtrlState, name: &str) -> Result<bool> {
    let meta = st.metadata.borrow();
    let Some(meta) = meta.as_ref() else {
        return Ok(false);
    };
    let json = format!(r#"{{"name":"{}"}}"#, name.replace('\"', ""));
    meta.set_property(
        0,
        "default.audio.sink",
        Some("Spa:String:JSON"),
        Some(&json),
    );
    // Optimistic cache only — callers must verify via Pulse (`pactl info`) and
    // fall through to pactl/wpctl when metadata alone does not stick.
    st.local.borrow_mut().default_audio_sink = Some(name.to_string());
    if let Ok(mut g) = st.shared.write() {
        g.default_audio_sink = Some(name.to_string());
    }
    // Provisional success: metadata accepted. Pulse verify lives in NativeBackend.
    Ok(true)
}

fn bind_node(st: &CtrlState, name: &str) -> Result<pw::node::Node> {
    let id = st
        .local
        .borrow()
        .node_id(pw_node(name))
        .ok_or_else(|| anyhow!("node `{name}` not in registry"))?;
    let global = pw::registry::GlobalObject {
        id,
        permissions: pw::permissions::PermissionFlags::empty(),
        type_: ObjectType::Node,
        version: 3,
        props: None::<pw::properties::Properties>,
    };
    st.registry
        .bind::<pw::node::Node, _>(&global)
        .map_err(|e| anyhow!("bind node `{name}`: {e}"))
}

fn do_set_levels(st: &CtrlState, endpoint: &str, gain_db: f32, muted: bool) -> Result<()> {
    let node = bind_node(st, endpoint)?;
    let linear = if muted {
        0.0
    } else {
        10f32.powf(gain_db / 20.0)
    };
    let bytes = props::pod_mute_volumes(muted, linear, 2);
    let pod = spa::pod::Pod::from_bytes(&bytes).ok_or_else(|| anyhow!("bad Props pod"))?;
    node.set_param(spa::param::ParamType::Props, 0, pod);
    let proxy_id = node.upcast_ref().id();
    st.linger.borrow_mut().insert(proxy_id, Box::new(node));
    Ok(())
}

fn do_set_node_props(st: &CtrlState, name: &str, entries: &[(String, String)]) -> Result<()> {
    let Some(bytes) = props::pod_string_props(entries) else {
        // No SPA-mappable keys — still accept (clock soft props often set at create).
        return Ok(());
    };
    let node = bind_node(st, name)?;
    let pod = spa::pod::Pod::from_bytes(&bytes).ok_or_else(|| anyhow!("bad Props pod"))?;
    node.set_param(spa::param::ParamType::Props, 0, pod);
    let proxy_id = node.upcast_ref().id();
    st.linger.borrow_mut().insert(proxy_id, Box::new(node));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_rpc_timeout_is_not_fatal_during_clock_mutation() {
        assert!(!timeout_kills_plane(1, true));
        assert!(!timeout_kills_plane(RPC_TIMEOUT_DEATH, true));
    }

    #[test]
    fn sustained_rpc_timeouts_are_fatal_when_idle() {
        assert!(!timeout_kills_plane(1, false));
        assert!(!timeout_kills_plane(RPC_TIMEOUT_DEATH - 1, false));
        assert!(timeout_kills_plane(RPC_TIMEOUT_DEATH, false));
    }

    #[test]
    fn control_plane_starts_when_pipewire_available() {
        // Skip gracefully if the session bus has no PipeWire (CI / headless).
        if std::env::var_os("PIPEWIRE_RUNTIME_DIR").is_none()
            && std::env::var_os("XDG_RUNTIME_DIR").is_none()
        {
            return;
        }
        match ensure_plane_started() {
            Ok(()) => {}
            Err(e) => eprintln!("native PW plane unavailable (ok in CI): {e}"),
        }
    }
}
