//! Out-of-process plugin UI bridge (P5).
//!
//! Editors run in a helper process (`buschain-plugin-ui`); parameter changes
//! flow back as [`EditorParamEvent`] → [`ControlMsg`] → [`ControlQueue`].
//! Audio stays in `PwFxNode` — never corked by the UI process.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::host::registry;

/// Request to open a plugin editor for a slot (UI process ≠ audio process).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenEditorRequest {
    pub bus: String,
    pub slot_id: Uuid,
    pub plugin_key: String,
    pub plugin_path: String,
    /// `clap` | `vst3` | other — helper picks a backend.
    #[serde(default)]
    pub format: String,
    /// Socket path the helper should connect to (host creates the listener).
    #[serde(default)]
    pub socket_path: String,
    /// Opaque plugin state so the editor instance matches the audio instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_blob: Option<Vec<u8>>,
    /// When set, helper embeds into this X11 window id (`kPlatformTypeX11EmbedWindowID`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_xid: Option<u64>,
    /// Prefer in-rect embed when parent_xid is set; otherwise float.
    #[serde(default)]
    pub embed: bool,
}

/// Parameter change from an out-of-process editor → ControlQueue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorParamEvent {
    pub slot_id: Uuid,
    pub control_index: u32,
    pub value: f32,
    #[serde(default)]
    pub sample_offset: u32,
}

static EDITOR_THREADS: Mutex<Vec<EditorHandle>> = Mutex::new(Vec::new());
/// Editor → session mirror (drained on the UI tick).
static PENDING_EDITOR_PARAMS: Mutex<Vec<EditorParamEvent>> = Mutex::new(Vec::new());
/// Native float editor closed by user (surface helper); UI clears attach flags.
static PENDING_EDITOR_CLOSED: Mutex<Vec<Uuid>> = Mutex::new(Vec::new());

struct EditorHandle {
    slot_id: Uuid,
    stop: Arc<AtomicBool>,
    child: Option<std::process::Child>,
}

/// Drain parameter events from out-of-process editors for session write-back.
pub fn drain_editor_param_events() -> Vec<EditorParamEvent> {
    PENDING_EDITOR_PARAMS
        .lock()
        .map(|mut g| std::mem::take(&mut *g))
        .unwrap_or_default()
}

/// Drain “native editor closed” notices from surface helpers.
pub fn drain_editor_closed_events() -> Vec<Uuid> {
    PENDING_EDITOR_CLOSED
        .lock()
        .map(|mut g| std::mem::take(&mut *g))
        .unwrap_or_default()
}

/// Bind the editor param socket and start the IPC accept thread (no helper spawn).
/// Used with `buschain-plugin-surface`, which connects as a client.
pub fn ensure_editor_ipc(bus: &str, slot_id: Uuid) {
    close_editor(slot_id);
    let socket_path = editor_socket_path(slot_id);
    let _ = std::fs::remove_file(&socket_path);
    let listener = match UnixListener::bind(&socket_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("buschain editor: bind {}: {e}", socket_path.display());
            return;
        }
    };
    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = Arc::clone(&stop);
    let bus = bus.to_string();
    let sock_for_thread = socket_path.clone();
    thread::Builder::new()
        .name("buschain-editor-ipc".into())
        .spawn(move || {
            editor_ipc_loop(listener, stop_t, bus, slot_id, sock_for_thread);
        })
        .ok();
    if let Ok(mut g) = EDITOR_THREADS.lock() {
        g.push(EditorHandle {
            slot_id,
            stop,
            child: None,
        });
    }
}

fn editor_ipc_loop(
    listener: UnixListener,
    stop_t: Arc<AtomicBool>,
    bus: String,
    slot_id: Uuid,
    sock_for_thread: PathBuf,
) {
    let _ = listener.set_nonblocking(true);
    let mut stream = None;
    for _ in 0..200 {
        if stop_t.load(Ordering::Relaxed) {
            break;
        }
        match listener.accept() {
            Ok((s, _)) => {
                stream = Some(s);
                break;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(_) => break,
        }
    }
    let Some(stream) = stream else {
        let _ = std::fs::remove_file(&sock_for_thread);
        return;
    };
    let _ = stream.set_nonblocking(false);
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        if stop_t.load(Ordering::Relaxed) {
            break;
        }
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "EDITOR_CLOSED" || line.starts_with("EDITOR_CLOSED") {
            let id = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| Uuid::parse_str(s).ok())
                .unwrap_or(slot_id);
            if let Ok(mut g) = PENDING_EDITOR_CLOSED.lock() {
                g.push(id);
            }
            continue;
        }
        if let Ok(ev) = serde_json::from_str::<EditorParamEvent>(line) {
            registry::push_host_param(&bus, ev.slot_id, ev.control_index, ev.value);
            if let Ok(mut g) = PENDING_EDITOR_PARAMS.lock() {
                g.push(ev);
            }
        }
    }
    let _ = std::fs::remove_file(&sock_for_thread);
}

/// Spawn legacy `buschain-plugin-ui` (duplicate instance — meters stay dead).
/// Prefer surface promote path for VST3.
pub fn request_open_editor(req: &OpenEditorRequest) {
    close_editor(req.slot_id);

    let socket_path = if req.socket_path.is_empty() {
        editor_socket_path(req.slot_id)
    } else {
        PathBuf::from(&req.socket_path)
    };
    let _ = std::fs::remove_file(&socket_path);

    let listener = match UnixListener::bind(&socket_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("buschain editor: bind {}: {e}", socket_path.display());
            return;
        }
    };

    let mut req = req.clone();
    req.socket_path = socket_path.display().to_string();
    let Ok(json) = serde_json::to_string(&req) else {
        return;
    };

    let helper = find_helper_binary("buschain-plugin-ui");
    let child = match helper {
        Some(bin) => Command::new(&bin)
            .arg("--request")
            .arg(&json)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                eprintln!("buschain editor: spawn {}: {e}", bin.display());
                e
            })
            .ok(),
        None => {
            eprintln!(
                "buschain editor: buschain-plugin-ui not found (PATH / next to exe / target/)"
            );
            None
        }
    };

    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = Arc::clone(&stop);
    let bus = req.bus.clone();
    let slot_id = req.slot_id;
    let sock_for_thread = socket_path.clone();
    thread::Builder::new()
        .name("buschain-editor-ipc".into())
        .spawn(move || {
            editor_ipc_loop(listener, stop_t, bus, slot_id, sock_for_thread);
        })
        .ok();

    if let Ok(mut g) = EDITOR_THREADS.lock() {
        g.push(EditorHandle {
            slot_id: req.slot_id,
            stop,
            child,
        });
    }
}

pub fn editor_event_to_control(ev: &EditorParamEvent) -> super::ControlMsg {
    super::ControlMsg::param(ev.slot_id, ev.control_index, ev.value)
        .with_offset(ev.sample_offset)
}

pub fn close_editor(slot_id: Uuid) {
    if let Ok(mut g) = EDITOR_THREADS.lock() {
        for h in g.iter_mut() {
            if h.slot_id == slot_id {
                h.stop.store(true, Ordering::Relaxed);
                if let Some(ref mut c) = h.child {
                    let _ = c.kill();
                }
            }
        }
        g.retain(|h| h.slot_id != slot_id);
    }
    let _ = std::fs::remove_file(editor_socket_path(slot_id));
}

fn editor_socket_path(slot_id: Uuid) -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            // Must not land under /tmp; helper open will fail loudly if unset.
            PathBuf::from("/run/user/invalid")
        });
    base.join(format!("buschain-editor-{}.sock", slot_id.simple()))
}

fn find_helper_binary(name: &str) -> Option<PathBuf> {
    if let Ok(p) = which_in_path(name) {
        return Some(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join(name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    for rel in ["target/debug", "target/release"] {
        let cand = Path::new(rel).join(name);
        if cand.is_file() {
            return Some(cand.canonicalize().unwrap_or(cand));
        }
    }
    None
}

fn which_in_path(name: &str) -> Result<PathBuf, ()> {
    let path = std::env::var_os("PATH").ok_or(())?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Ok(cand);
        }
    }
    Err(())
}
