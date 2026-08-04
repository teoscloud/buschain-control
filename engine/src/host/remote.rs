//! Per-track DSP sandbox (P6) — SHM audio rings + child `buschain-plugin-dsp`.
//!
//! Trusted default remains in-process. When sandboxed, this adapter owns the
//! SHM rings and a child process; a crash kills that track's FX only.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use once_cell::sync::Lazy;
use uuid::Uuid;

use super::processor::{AudioProcessor, MidiEvent};

/// Ctrl sockets for surface helpers (editor open / resize commands from UI thread).
static SURFACE_CTRL: Lazy<Mutex<HashMap<Uuid, UnixStream>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Send a control line to a surface helper (`E embed <xid>`, `E float`, `R w h`, …).
pub fn surface_send_ctrl(slot_id: Uuid, line: &str) -> bool {
    let Ok(mut g) = SURFACE_CTRL.lock() else {
        return false;
    };
    let Some(s) = g.get_mut(&slot_id) else {
        return false;
    };
    let msg = if line.ends_with('\n') {
        line.to_string()
    } else {
        format!("{line}\n")
    };
    s.write_all(msg.as_bytes()).is_ok()
}

/// True when the surface helper ctrl socket is connected (DSP+GUI helper is up).
pub fn surface_ctrl_ready(slot_id: Uuid) -> bool {
    SURFACE_CTRL
        .lock()
        .map(|g| g.contains_key(&slot_id))
        .unwrap_or(false)
}

pub fn surface_forget_ctrl(slot_id: Uuid) {
    if let Ok(mut g) = SURFACE_CTRL.lock() {
        g.remove(&slot_id);
    }
}

const HDR_MAGIC: u32 = 0x4253_4658; // "BSFX"
const MAX_BLOCK: usize = 4096;

#[repr(C)]
struct ShmHeader {
    magic: AtomicU32,
    seq_in: AtomicU32,
    seq_out: AtomicU32,
    frames: AtomicU32,
    alive: AtomicU32,
    _pad: AtomicU32,
}

/// Sandboxed remote processor — SHM stereo rings + control IPC.
pub struct RemoteProcessor {
    pub track_bus: String,
    surface_slot: Option<Uuid>,
    controls: Vec<f32>,
    latency: u32,
    child: Option<Child>,
    crashed: Arc<AtomicBool>,
    map: Option<memmap2::MmapMut>,
    ctrl: Option<UnixStream>,
    max_block: usize,
}

impl RemoteProcessor {
    pub fn new(track_bus: impl Into<String>) -> Self {
        Self {
            track_bus: track_bus.into(),
            surface_slot: None,
            controls: Vec::new(),
            latency: 0,
            child: None,
            crashed: Arc::new(AtomicBool::new(false)),
            map: None,
            ctrl: None,
            max_block: 256,
        }
    }

    /// Spawn sandbox child hosting `plugin_path` / `plugin_key` / `format`.
    pub fn spawn_for_plugin(
        track_bus: &str,
        plugin_key: &str,
        plugin_path: &str,
        format: &str,
        sample_rate: u32,
        max_block: u32,
    ) -> Self {
        let mut remote = Self::new(track_bus);
        remote.max_block = (max_block as usize).clamp(64, MAX_BLOCK);
        if let Err(e) = remote.start(plugin_key, plugin_path, format, sample_rate, None, None) {
            eprintln!("buschain sandbox: {e}");
            remote.crashed.store(true, Ordering::Relaxed);
        }
        remote
    }

    /// Spawn `buschain-plugin-surface` — same-instance DSP + native editor (meters live).
    pub fn spawn_for_surface(
        track_bus: &str,
        slot_id: Uuid,
        plugin_key: &str,
        plugin_path: &str,
        format: &str,
        state_blob: Option<Vec<u8>>,
        sample_rate: u32,
        max_block: u32,
    ) -> Self {
        let mut remote = Self::new(track_bus);
        remote.surface_slot = Some(slot_id);
        remote.max_block = (max_block as usize).clamp(64, MAX_BLOCK);
        if let Err(e) = remote.start(
            plugin_key,
            plugin_path,
            format,
            sample_rate,
            Some(slot_id),
            state_blob,
        ) {
            eprintln!("buschain surface: {e}");
            remote.crashed.store(true, Ordering::Relaxed);
        }
        remote
    }

    pub fn set_latency(&mut self, samples: u32) {
        self.latency = samples;
    }

    pub fn crashed(&self) -> bool {
        self.crashed.load(Ordering::Relaxed)
    }

    fn start(
        &mut self,
        plugin_key: &str,
        plugin_path: &str,
        format: &str,
        sample_rate: u32,
        surface_slot: Option<Uuid>,
        state_blob: Option<Vec<u8>>,
    ) -> anyhow::Result<()> {
        // Short unique names only — never embed plugin paths. Unix `sun_path` is
        // ~108 bytes; VST3 filesystem keys used to blow SUN_LEN and kill the surface helper.
        let uniq = Uuid::new_v4().simple();
        let pid = std::process::id();
        let shm_name = format!("/bcfx-{pid}-{uniq}");
        let bytes = std::mem::size_of::<ShmHeader>() + self.max_block * 4 * 4; // L/R in + L/R out
        let fd = shm_open_create(&shm_name, bytes)?;
        // SAFETY: fresh shm sized above.
        let map = unsafe { memmap2::MmapMut::map_mut(&fd)? };
        {
            let hdr = unsafe { &*(map.as_ptr() as *const ShmHeader) };
            hdr.magic.store(HDR_MAGIC, Ordering::Relaxed);
            hdr.seq_in.store(0, Ordering::Relaxed);
            hdr.seq_out.store(0, Ordering::Relaxed);
            hdr.frames.store(0, Ordering::Relaxed);
            hdr.alive.store(1, Ordering::Relaxed);
        }
        self.map = Some(map);

        let sock_path = runtime_dir()?.join(format!("bc-dsp-{pid}-{uniq}.sock"));
        ensure_unix_sock_path(&sock_path)?;
        let _ = std::fs::remove_file(&sock_path);

        let surface = surface_slot.is_some();
        let helper_name = if surface {
            "buschain-plugin-surface"
        } else {
            "buschain-plugin-dsp"
        };
        let helper = find_helper_binary(helper_name)
            .ok_or_else(|| anyhow::anyhow!("{helper_name} not found"))?;

        let mut cmd = Command::new(&helper);
        cmd.arg("--shm")
            .arg(&shm_name)
            .arg("--sock")
            .arg(&sock_path)
            .arg("--path")
            .arg(plugin_path)
            .arg("--key")
            .arg(plugin_key)
            .arg("--format")
            .arg(format)
            .arg("--rate")
            .arg(sample_rate.to_string())
            .arg("--block")
            .arg(self.max_block.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());

        if let Some(slot_id) = surface_slot {
            let editor_sock = runtime_dir()?.join(format!(
                "buschain-editor-{}.sock",
                slot_id.simple()
            ));
            cmd.arg("--slot-id")
                .arg(slot_id.to_string())
                .arg("--editor-sock")
                .arg(&editor_sock);
            if let Some(blob) = state_blob.filter(|b| !b.is_empty()) {
                let state_path =
                    runtime_dir()?.join(format!("buschain-state-{}.bin", slot_id.simple()));
                std::fs::write(&state_path, &blob)?;
                cmd.arg("--state-file").arg(&state_path);
            }
        }

        let child = cmd.spawn()?;
        self.child = Some(child);

        // Connect control socket (child listens).
        for _ in 0..100 {
            if let Ok(s) = UnixStream::connect(&sock_path) {
                let _ = s.set_nonblocking(true);
                if let Some(slot_id) = surface_slot {
                    if let Ok(clone) = s.try_clone() {
                        if let Ok(mut g) = SURFACE_CTRL.lock() {
                            g.insert(slot_id, clone);
                        }
                    }
                }
                self.ctrl = Some(s);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Ok(())
    }

    fn header_mut(&mut self) -> Option<&ShmHeader> {
        let map = self.map.as_ref()?;
        Some(unsafe { &*(map.as_ptr() as *const ShmHeader) })
    }

    fn audio_ptrs(&mut self) -> Option<(*mut f32, *mut f32, *mut f32, *mut f32)> {
        let map = self.map.as_mut()?;
        let base = unsafe { map.as_mut_ptr().add(std::mem::size_of::<ShmHeader>()) as *mut f32 };
        let n = self.max_block;
        unsafe {
            Some((
                base,
                base.add(n),
                base.add(n * 2),
                base.add(n * 3),
            ))
        }
    }
}

impl Drop for RemoteProcessor {
    fn drop(&mut self) {
        // Stop RT from touching SHM before we unmap / kill the child (avoids SIGBUS).
        self.crashed.store(true, Ordering::SeqCst);
        if let Some(slot) = self.surface_slot.take() {
            surface_forget_ctrl(slot);
        }
        if let Some(hdr) = self.header_mut() {
            hdr.alive.store(0, Ordering::SeqCst);
        }
        // Brief grace so an in-flight process() can observe `crashed` / `alive=0`.
        std::thread::sleep(std::time::Duration::from_millis(8));
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.map = None;
        self.ctrl = None;
    }
}

impl AudioProcessor for RemoteProcessor {
    fn prepare(&mut self, _sample_rate: u32, max_block: u32) {
        if (max_block as usize) > self.max_block {
            // Cannot grow SHM mid-flight; clamp.
        }
    }

    fn latency_samples(&self) -> u32 {
        self.latency
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.crashed.load(Ordering::SeqCst) {
            return; // identity / silent failure — audio passes through unchanged
        }
        // Watchdog: child exit → passthrough.
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => {
                    self.crashed.store(true, Ordering::SeqCst);
                    return;
                }
                Err(_) => {
                    self.crashed.store(true, Ordering::SeqCst);
                    return;
                }
                Ok(None) => {}
            }
        } else {
            return;
        }

        let n = left.len().min(right.len()).min(self.max_block);
        if n == 0 {
            return;
        }
        // Re-check after try_wait — Drop may have raced in.
        if self.crashed.load(Ordering::SeqCst) || self.map.is_none() {
            return;
        }
        let Some((in_l, in_r, out_l, out_r)) = self.audio_ptrs() else {
            return;
        };
        unsafe {
            std::ptr::copy_nonoverlapping(left.as_ptr(), in_l, n);
            std::ptr::copy_nonoverlapping(right.as_ptr(), in_r, n);
        }
        if let Some(hdr) = self.header_mut() {
            hdr.frames.store(n as u32, Ordering::Release);
            let seq = hdr.seq_in.fetch_add(1, Ordering::AcqRel) + 1;
            // Spin briefly for child (bounded — xrun-safe upper bound).
            for _ in 0..10_000 {
                if hdr.seq_out.load(Ordering::Acquire) >= seq {
                    break;
                }
                std::hint::spin_loop();
            }
            if hdr.seq_out.load(Ordering::Acquire) < seq {
                // Timeout — leave input as output (passthrough this block).
                return;
            }
        }
        unsafe {
            std::ptr::copy_nonoverlapping(out_l, left.as_mut_ptr(), n);
            std::ptr::copy_nonoverlapping(out_r, right.as_mut_ptr(), n);
        }
    }

    fn set_control(&mut self, index: usize, value: f32) {
        if index >= self.controls.len() {
            self.controls.resize(index + 1, 0.0);
        }
        self.controls[index] = value;
        if let Some(ref mut s) = self.ctrl {
            let line = format!("P {index} {value}\n");
            let _ = s.write_all(line.as_bytes());
        }
    }

    fn control_count(&self) -> usize {
        self.controls.len()
    }

    fn control_name(&self, _index: usize) -> Option<&str> {
        None
    }

    fn feed_midi(&mut self, events: &[MidiEvent]) {
        if let Some(ref mut s) = self.ctrl {
            for ev in events {
                let line = match *ev {
                    MidiEvent::NoteOn {
                        channel,
                        note,
                        velocity,
                    } => format!("N {channel} {note} {velocity}\n"),
                    MidiEvent::NoteOff {
                        channel,
                        note,
                        velocity,
                    } => format!("F {channel} {note} {velocity}\n"),
                    MidiEvent::Cc {
                        channel,
                        controller,
                        value,
                    } => format!("C {channel} {controller} {value}\n"),
                };
                let _ = s.write_all(line.as_bytes());
            }
        }
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(48)
        .collect()
}

/// Linux `sockaddr_un.sun_path` is 108 bytes incl. NUL → max path length 107.
fn ensure_unix_sock_path(path: &Path) -> anyhow::Result<()> {
    const SUN_PATH_MAX: usize = 107;
    let s = path.to_string_lossy();
    if s.len() > SUN_PATH_MAX {
        anyhow::bail!(
            "unix sock path too long ({} > {SUN_PATH_MAX}): {s}",
            s.len()
        );
    }
    Ok(())
}

fn runtime_dir() -> anyhow::Result<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            anyhow::anyhow!("XDG_RUNTIME_DIR is unset — refusing temp-dir fallback for DSP IPC")
        })?;
    let dir = base; // keep socks under XDG_RUNTIME_DIR (not a shared /tmp tree)
    Ok(dir)
}

fn find_helper_binary(name: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let cand = dir.join(name);
            if cand.is_file() {
                return Some(cand);
            }
        }
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

fn shm_open_create(name: &str, size: usize) -> anyhow::Result<std::fs::File> {
    // Prefer memfd when available; fall back to file-backed map in runtime dir.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::FromRawFd;
        let cname = std::ffi::CString::new(name.trim_start_matches('/'))?;
        let fd = unsafe {
            libc::shm_open(
                cname.as_ptr(),
                libc::O_CREAT | libc::O_RDWR | libc::O_TRUNC,
                0o600,
            )
        };
        if fd >= 0 {
            if unsafe { libc::ftruncate(fd, size as libc::off_t) } != 0 {
                unsafe {
                    libc::close(fd);
                    libc::shm_unlink(cname.as_ptr());
                }
                anyhow::bail!("ftruncate shm failed");
            }
            return Ok(unsafe { std::fs::File::from_raw_fd(fd) });
        }
    }
    let path = runtime_dir()?.join(format!("buschain-{}.shm", sanitize(name)));
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)?;
    f.set_len(size as u64)?;
    Ok(f)
}
