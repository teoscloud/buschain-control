//! Unix-socket JSON line protocol for daemon ↔ UI / ctl / waybar.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::audio::graph::PwSnapshot;
use crate::audio::worker::Command;
use crate::session::Session;

/// `$XDG_RUNTIME_DIR/buschain-control` — never falls back to `/tmp`.
pub fn try_runtime_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            anyhow!("XDG_RUNTIME_DIR is unset — refusing /tmp fallback for BusChain IPC")
        })?;
    Ok(base.join("buschain-control"))
}

/// Create the runtime dir with mode `0700` (same class as Pulse/PipeWire runtime dirs).
pub fn ensure_runtime_dir() -> Result<PathBuf> {
    let dir = try_runtime_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create runtime dir {}", dir.display()))?;
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    Ok(dir)
}

/// Canonical runtime dir (requires `XDG_RUNTIME_DIR`).
pub fn runtime_dir() -> Result<PathBuf> {
    try_runtime_dir()
}

pub fn socket_path() -> Result<PathBuf> {
    Ok(try_runtime_dir()?.join("daemon.sock"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Ping,
    GetStatus,
    GetSnapshot,
    /// Aggregate mixer JSON for Quickshell / shell panels (`buschain-ctl mixer`).
    GetMixer,
    SetHwVolume { pct: u32 },
    AdjustHwVolume { delta: i32 },
    SetHwMute { mute: bool },
    SetSinkInputVolume { index: u32, pct: u32 },
    SetSinkInputMute { index: u32, mute: bool },
    MoveSinkInput { index: u32, sink: String },
    SetDefaultSink { name: String },
    SetDefaultSource { name: String },
    SetMasterHw { name: String },
    SetSinkVolume { name: String, pct: u32 },
    SetSinkMute { name: String, mute: bool },
    SetSourceVolume { name: String, pct: u32 },
    SetSourceMute { name: String, mute: bool },
    /// Mixer overlay: set a session track's gain (dB) and/or mute.
    SetTrackMixer {
        track_id: uuid::Uuid,
        #[serde(default)]
        gain_db: Option<f32>,
        #[serde(default)]
        mute: Option<bool>,
    },
    SessionList,
    SessionSave,
    SessionSaveAs { name: String },
    SessionLoad { slug: String },
    SessionDelete { slug: String },
    Apply,
    /// Forward a full worker [`Command`] (UI thin-client path).
    Exec { cmd: Command },
    /// Fast FX knob path — no session blob (keeps props pushes sub-frame).
    PushFxControls {
        bus: String,
        inserts: Vec<buschain_engine::InsertSlot>,
    },
    Shutdown,
    PopupPlayback,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub ok: bool,
    pub message: String,
    pub master_hw: Option<String>,
    pub master_hw_desc: Option<String>,
    pub hw_volume_pct: u32,
    pub hw_mute: bool,
    pub session_slug: String,
    pub session_name: String,
    pub sample_rate: u32,
    pub quantum: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Ok {
        #[serde(default)]
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<Status>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sessions: Option<Vec<SessionListItem>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        snapshot: Option<PwSnapshot>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<Session>,
    },
    /// Stable mixer aggregate for shell UIs (does not embed raw PwSnapshot).
    Mixer {
        mixer: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<Status>,
    },
    Err {
        error: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListItem {
    pub slug: String,
    pub name: String,
}

pub fn encode_line(v: &impl Serialize) -> Result<String> {
    Ok(serde_json::to_string(v)?)
}

pub fn write_line(stream: &mut UnixStream, v: &impl Serialize) -> Result<()> {
    let mut line = encode_line(v)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()?;
    Ok(())
}

pub fn read_line(stream: &mut UnixStream) -> Result<String> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.is_empty() {
        return Err(anyhow!("connection closed"));
    }
    Ok(line)
}

pub struct Client;

impl Client {
    pub fn connect() -> Result<UnixStream> {
        let path = socket_path()?;
        UnixStream::connect(&path).with_context(|| format!("connect {}", path.display()))
    }

    pub fn call(req: &Request) -> Result<Response> {
        let mut stream = Self::connect()?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        write_line(&mut stream, req)?;
        let line = read_line(&mut stream)?;
        let resp: Response = serde_json::from_str(line.trim())?;
        Ok(resp)
    }

    /// Short-timeout call for live knobs (must not sit behind 5s IPC stalls).
    pub fn call_fast(req: &Request) -> Result<Response> {
        let mut stream = Self::connect()?;
        stream.set_read_timeout(Some(Duration::from_millis(350)))?;
        stream.set_write_timeout(Some(Duration::from_millis(350)))?;
        write_line(&mut stream, req)?;
        let line = read_line(&mut stream)?;
        let resp: Response = serde_json::from_str(line.trim())?;
        Ok(resp)
    }

    /// Waybar hover-scroll: write Adjust and close — do **not** wait for a reply.
    /// Waiting (even 20ms) plus Waybar's `exec-on-event` status made the module
    /// drop wheel ticks so users had to spam the scroll wheel.
    pub fn poke(req: &Request) -> Result<()> {
        let mut stream = Self::connect()?;
        stream.set_write_timeout(Some(Duration::from_millis(50)))?;
        write_line(&mut stream, req)?;
        // Drop the socket; daemon may fail writing the response — that's fine.
        Ok(())
    }

    pub fn ping() -> bool {
        matches!(Self::call(&Request::Ping), Ok(Response::Ok { .. }))
    }
}

/// Reject peers whose UID does not match ours (SO_PEERCRED). Same-UID malware
/// still wins; this only blocks cross-user connects if the socket were somehow
/// reachable outside a private runtime dir.
pub fn peer_uid_ok(stream: &UnixStream) -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::mem::MaybeUninit;
        use std::os::fd::AsRawFd;
        let mut cred = MaybeUninit::<libc::ucred>::uninit();
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                cred.as_mut_ptr() as *mut libc::c_void,
                &mut len,
            )
        };
        if rc != 0 {
            return false;
        }
        let cred = unsafe { cred.assume_init() };
        cred.uid == unsafe { libc::getuid() }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = stream;
        true
    }
}

pub fn bind_listener() -> Result<UnixListener> {
    let dir = ensure_runtime_dir()?;
    let path = dir.join("daemon.sock");
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).with_context(|| format!("bind {}", path.display()))?;
    // Stale-socket friendly.
    listener.set_nonblocking(true)?;
    // Socket mode: owner-only (dir is already 0700).
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    Ok(listener)
}

/// Spawn `buschain-daemon` if the socket is not answering.
pub fn ensure_daemon() -> Result<()> {
    if Client::ping() {
        return Ok(());
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("buschain-daemon"));
        }
    }
    candidates.push(PathBuf::from("buschain-daemon"));
    if let Ok(path) = std::env::var("BUSCHAIN_CONTROL_DAEMON") {
        candidates.insert(0, PathBuf::from(path));
    }
    // Dev: CARGO_TARGET_DIR/debug|release
    if let Ok(dir) = std::env::var("CARGO_TARGET_DIR") {
        candidates.push(PathBuf::from(&dir).join("debug/buschain-daemon"));
        candidates.push(PathBuf::from(&dir).join("release/buschain-daemon"));
    }

    let mut started = false;
    for bin in &candidates {
        match std::process::Command::new(bin)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => {
                started = true;
                eprintln!("buschain-control: started daemon {}", bin.display());
                break;
            }
            Err(_) => continue,
        }
    }
    if !started {
        return Ok(());
    }
    for _ in 0..80 {
        if Client::ping() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}
