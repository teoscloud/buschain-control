//! VST3 surface helper — one process hosts DSP (SHM) + native editor (same Plugin).
//!
//! Ctrl socket lines (from host):
//! - `P <idx> <val>` param
//! - `N/F/C …` MIDI
//! - `E embed <xid>` open editor into parent X11 window
//! - `E float` open floating editor toplevel
//! - `E close` close editor only (DSP keeps running)
//! - `R <w> <h>` resize hint (best-effort)
//! - `Q` quit
//!
//! On user close of the float window we detach the editor (DSP stays up) and write
//! `EDITOR_CLOSED <slot>` on the editor sock so the host can re-attach later.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use buschain_engine::EditorParamEvent;
use uuid::Uuid;
use vst3_host::audio::AudioBuffers;
use vst3_host::audio::SpeakerArrangement;
use vst3_host::host::Vst3Host;
use vst3_host::plugin::{Plugin, WindowHandle};
use xcb::x;
use xcb::{Connection, Xid};

enum UiCmd {
    Embed(u32),
    Float,
    CloseEditor,
    Resize {
        #[allow(dead_code)]
        w: u16,
        #[allow(dead_code)]
        h: u16,
    },
    Quit,
}

/// Host-owned X11 float window (STRUCTURE_NOTIFY + WM_DELETE_WINDOW).
struct FloatEditor {
    conn: Connection,
    window: x::Window,
    wm_delete: x::Atom,
    closed: bool,
}

impl FloatEditor {
    fn open(plugin: &Arc<Mutex<Plugin>>, title: &str) -> Result<Self> {
        let (conn, screen_num) = Connection::connect(None)
            .map_err(|e| anyhow::anyhow!("X11 connect: {e}"))?;
        let setup = conn.get_setup();
        let screen = setup
            .roots()
            .nth(screen_num as usize)
            .ok_or_else(|| anyhow::anyhow!("no X11 screen"))?;

        let (width, height) = plugin
            .lock()
            .ok()
            .and_then(|p| p.get_editor_size().ok())
            .unwrap_or((800, 600));
        let width = width.clamp(200, 4096) as u16;
        let height = height.clamp(120, 4096) as u16;

        let window = conn.generate_id::<x::Window>();
        conn.send_and_check_request(&x::CreateWindow {
            depth: x::COPY_FROM_PARENT as u8,
            wid: window,
            parent: screen.root(),
            x: 64,
            y: 64,
            width,
            height,
            border_width: 0,
            class: x::WindowClass::InputOutput,
            visual: screen.root_visual(),
            value_list: &[
                x::Cw::BackPixel(screen.black_pixel()),
                x::Cw::EventMask(
                    x::EventMask::EXPOSURE
                        | x::EventMask::STRUCTURE_NOTIFY
                        | x::EventMask::KEY_PRESS,
                ),
            ],
        })
        .map_err(|e| anyhow::anyhow!("CreateWindow: {e}"))?;

        // Title
        conn.send_request(&x::ChangeProperty {
            mode: x::PropMode::Replace,
            window,
            property: x::ATOM_WM_NAME,
            r#type: x::ATOM_STRING,
            data: title.as_bytes(),
        });

        // WM_CLASS = buschain-vst3 / buschain-vst3 (Hyprland float rules)
        let class = b"buschain-vst3\0buschain-vst3\0";
        let wm_class = intern_atom(&conn, b"WM_CLASS")?;
        conn.send_request(&x::ChangeProperty {
            mode: x::PropMode::Replace,
            window,
            property: wm_class,
            r#type: x::ATOM_STRING,
            data: class,
        });

        // WM_DELETE_WINDOW so we can detach the editor cleanly (no BadDrawable paint).
        let wm_protocols = intern_atom(&conn, b"WM_PROTOCOLS")?;
        let wm_delete = intern_atom(&conn, b"WM_DELETE_WINDOW")?;
        conn.send_request(&x::ChangeProperty {
            mode: x::PropMode::Replace,
            window,
            property: wm_protocols,
            r#type: x::ATOM_ATOM,
            data: &[wm_delete],
        });

        conn.send_request(&x::MapWindow { window });
        let _ = conn.flush();

        let handle = WindowHandle::from_x11(window.resource_id());
        plugin
            .lock()
            .map_err(|_| anyhow::anyhow!("plugin lock poisoned"))?
            .open_editor(handle)
            .map_err(|e| anyhow::anyhow!("open_editor: {e}"))?;

        Ok(Self {
            conn,
            window,
            wm_delete,
            closed: false,
        })
    }

    /// Pump X events. Returns `false` when the window was closed by the user/WM.
    fn pump(&mut self) -> bool {
        if self.closed {
            return false;
        }
        while let Some(event) = self.conn.poll_for_event().ok().flatten() {
            match event {
                xcb::Event::X(x::Event::ClientMessage(ev)) => {
                    if let x::ClientMessageData::Data32([atom, ..]) = ev.data() {
                        if atom == self.wm_delete.resource_id() {
                            self.closed = true;
                        }
                    }
                }
                xcb::Event::X(x::Event::DestroyNotify(_)) => {
                    self.closed = true;
                }
                xcb::Event::X(x::Event::UnmapNotify(_)) => {
                    // Ignore transient unmaps; DestroyNotify / WM_DELETE are authoritative.
                }
                _ => {}
            }
        }
        // Fallback: window vanished without an event we saw (aggressive WM).
        if !self.closed && !window_exists(&self.conn, self.window) {
            self.closed = true;
        }
        !self.closed
    }

    fn close(mut self, plugin: &Arc<Mutex<Plugin>>) {
        self.closed = true;
        if let Ok(mut g) = plugin.lock() {
            let _ = g.close_editor();
        }
        let _ = self.conn.send_and_check_request(&x::UnmapWindow {
            window: self.window,
        });
        let _ = self.conn.send_and_check_request(&x::DestroyWindow {
            window: self.window,
        });
        let _ = self.conn.flush();
    }
}

fn intern_atom(conn: &Connection, name: &[u8]) -> Result<x::Atom> {
    let cookie = conn.send_request(&x::InternAtom {
        only_if_exists: false,
        name,
    });
    let reply = conn
        .wait_for_reply(cookie)
        .map_err(|e| anyhow::anyhow!("InternAtom: {e}"))?;
    Ok(reply.atom())
}

fn window_exists(conn: &Connection, window: x::Window) -> bool {
    let cookie = conn.send_request(&x::GetGeometry {
        drawable: x::Drawable::Window(window),
    });
    conn.wait_for_reply(cookie).is_ok()
}

/// Ignore BadDrawable/BadWindow from plugin Xlib paint after the WM destroyed the view.
fn install_xlib_error_handler() {
    unsafe extern "C" fn ignore(
        _dpy: *mut libc::c_void,
        _ev: *mut libc::c_void,
    ) -> libc::c_int {
        0
    }
    #[link(name = "X11")]
    unsafe extern "C" {
        fn XSetErrorHandler(
            handler: Option<
                unsafe extern "C" fn(*mut libc::c_void, *mut libc::c_void) -> libc::c_int,
            >,
        ) -> Option<unsafe extern "C" fn(*mut libc::c_void, *mut libc::c_void) -> libc::c_int>;
    }
    unsafe {
        XSetErrorHandler(Some(ignore));
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("buschain-plugin-surface: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut shm = String::new();
    let mut sock = String::new();
    let mut path = String::new();
    let mut key = String::new();
    let mut format = String::from("vst3");
    let mut rate = 48_000u32;
    let mut block = 256usize;
    let mut editor_sock = String::new();
    let mut slot_id = Uuid::nil();
    let mut state_file = String::new();

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--shm" => shm = args.next().unwrap_or_default(),
            "--sock" => sock = args.next().unwrap_or_default(),
            "--path" => path = args.next().unwrap_or_default(),
            "--key" => key = args.next().unwrap_or_default(),
            "--format" => format = args.next().unwrap_or_default(),
            "--rate" => rate = args.next().and_then(|s| s.parse().ok()).unwrap_or(rate),
            "--block" => block = args.next().and_then(|s| s.parse().ok()).unwrap_or(block),
            "--editor-sock" => editor_sock = args.next().unwrap_or_default(),
            "--slot-id" => {
                slot_id = args
                    .next()
                    .and_then(|s| Uuid::parse_str(&s).ok())
                    .unwrap_or(slot_id);
            }
            "--state-file" => state_file = args.next().unwrap_or_default(),
            _ => {}
        }
    }
    if shm.is_empty() || sock.is_empty() || path.is_empty() {
        bail!(
            "usage: buschain-plugin-surface --shm NAME --sock PATH --path PLUGIN --key ID \
             [--editor-sock PATH] [--slot-id UUID] [--state-file PATH]"
        );
    }
    if format != "vst3" {
        bail!("surface helper supports vst3 only (got {format})");
    }

    install_xlib_error_handler();

    let bytes = std::mem::size_of::<ShmHeader>() + block * 4 * 4;
    let file = open_shm(&shm, bytes)?;
    let map = unsafe { memmap2::MmapMut::map_mut(&file)? };

    let mut host = Vst3Host::builder()
        .sample_rate(rate as f64)
        .block_size(block)
        .input_channels(2)
        .output_channels(2)
        .build()
        .map_err(|e| anyhow::anyhow!("VST3 host: {e}"))?;

    let mut plugin = host
        .load_plugin(&path)
        .or_else(|_| host.load_plugin_class(&path, &key))
        .map_err(|e| anyhow::anyhow!("load {path}: {e}"))?;

    let _ = plugin.set_bus_arrangements(
        &[SpeakerArrangement::STEREO],
        &[SpeakerArrangement::STEREO],
    );

    if !state_file.is_empty() {
        if let Ok(blob) = std::fs::read(&state_file) {
            if !blob.is_empty() {
                let _ = plugin.load_state(&blob);
            }
        }
        let _ = std::fs::remove_file(&state_file);
    }
    plugin
        .start_processing()
        .map_err(|e| anyhow::anyhow!("start_processing: {e}"))?;

    let title = {
        let info = plugin.info();
        format!("{} - VST3", info.name)
    };

    let plugin = Arc::new(Mutex::new(plugin));
    let ui_cmds: Arc<Mutex<Vec<UiCmd>>> = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));

    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).context("bind ctrl sock")?;
    listener.set_nonblocking(true)?;

    // Audio + ctrl accept/read thread (owns the SHM mapping).
    {
        let plugin = Arc::clone(&plugin);
        let ui_cmds = Arc::clone(&ui_cmds);
        let stop = Arc::clone(&stop);
        thread::Builder::new()
            .name("surface-audio".into())
            .spawn(move || {
                let map = map;
                let hdr = unsafe { &*(map.as_ptr() as *const ShmHeader) };
                let base = unsafe {
                    map.as_ptr()
                        .add(std::mem::size_of::<ShmHeader>()) as *mut f32
                };
                let mut stream: Option<UnixStream> = None;
                let mut ctrl_buf = Vec::new();
                let mut last_seq = 0u32;
                let mut buffers = AudioBuffers::new(2, 2, block, rate as f64);

                while hdr.alive.load(Ordering::Relaxed) != 0 && !stop.load(Ordering::Relaxed) {
                    if stream.is_none() {
                        if let Ok((s, _)) = listener.accept() {
                            let _ = s.set_nonblocking(true);
                            stream = Some(s);
                        }
                    }
                    if let Some(ref mut s) = stream {
                        let mut buf = [0u8; 512];
                        match s.read(&mut buf) {
                            Ok(0) => stream = None,
                            Ok(n) => {
                                ctrl_buf.extend_from_slice(&buf[..n]);
                                while let Some(pos) = ctrl_buf.iter().position(|&b| b == b'\n') {
                                    let line =
                                        String::from_utf8_lossy(&ctrl_buf[..pos]).to_string();
                                    ctrl_buf.drain(..=pos);
                                    handle_ctrl(&plugin, &ui_cmds, &stop, line.trim());
                                }
                            }
                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                            Err(_) => stream = None,
                        }
                    }

                    let seq = hdr.seq_in.load(Ordering::Acquire);
                    if seq != last_seq {
                        let n = (hdr.frames.load(Ordering::Acquire) as usize).min(block);
                        if n > 0 {
                            let (in_l, in_r, out_l, out_r) = unsafe {
                                (
                                    std::slice::from_raw_parts(base, n),
                                    std::slice::from_raw_parts(base.add(block), n),
                                    std::slice::from_raw_parts_mut(base.add(block * 2), n),
                                    std::slice::from_raw_parts_mut(base.add(block * 3), n),
                                )
                            };
                            if let Ok(mut guard) = plugin.lock() {
                                buffers.inputs[0][..n].copy_from_slice(in_l);
                                buffers.inputs[1][..n].copy_from_slice(in_r);
                                if n < block {
                                    buffers.inputs[0][n..].fill(0.0);
                                    buffers.inputs[1][n..].fill(0.0);
                                }
                                for ch in &mut buffers.outputs {
                                    ch.fill(0.0);
                                }
                                let saved = buffers.block_size;
                                buffers.block_size = n;
                                for ch in &mut buffers.inputs {
                                    ch.truncate(n);
                                }
                                for ch in &mut buffers.outputs {
                                    ch.truncate(n);
                                }
                                let _ = guard.process_audio(&mut buffers);
                                if buffers.outputs.len() >= 2 {
                                    let on = buffers.outputs[0].len().min(n);
                                    out_l[..on].copy_from_slice(&buffers.outputs[0][..on]);
                                    out_r[..on].copy_from_slice(&buffers.outputs[1][..on]);
                                }
                                for ch in &mut buffers.inputs {
                                    ch.resize(saved, 0.0);
                                }
                                for ch in &mut buffers.outputs {
                                    ch.resize(saved, 0.0);
                                }
                                buffers.block_size = saved;
                            } else {
                                out_l.copy_from_slice(in_l);
                                out_r.copy_from_slice(in_r);
                            }
                            hdr.seq_out.store(seq, Ordering::Release);
                            last_seq = seq;
                        }
                    } else {
                        thread::sleep(Duration::from_micros(100));
                    }
                }
                stop.store(true, Ordering::Relaxed);
            })
            .ok();
    }

    // Editor param socket (host listens).
    let mut editor_stream: Option<UnixStream> = None;
    if !editor_sock.is_empty() {
        for _ in 0..200 {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match UnixStream::connect(&editor_sock) {
                Ok(s) => {
                    editor_stream = Some(s);
                    break;
                }
                Err(_) => thread::sleep(Duration::from_millis(25)),
            }
        }
    }

    let mut float_win: Option<FloatEditor> = None;
    let mut last: Vec<f64> = plugin
        .lock()
        .ok()
        .and_then(|p| p.get_parameters().ok())
        .map(|ps| ps.iter().map(|p| p.value).collect())
        .unwrap_or_default();

    eprintln!("buschain-plugin-surface: ready slot={slot_id} (await E embed|float)");

    let frame = Duration::from_millis(16);
    let mut next = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        let cmds: Vec<UiCmd> = ui_cmds
            .lock()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default();
        for cmd in cmds {
            match cmd {
                UiCmd::Quit => {
                    stop.store(true, Ordering::Relaxed);
                }
                UiCmd::CloseEditor => {
                    if let Some(w) = float_win.take() {
                        w.close(&plugin);
                        notify_editor_closed(&mut editor_stream, slot_id);
                        eprintln!("buschain-plugin-surface: editor closed (DSP running)");
                    } else if let Ok(mut g) = plugin.lock() {
                        let _ = g.close_editor();
                        notify_editor_closed(&mut editor_stream, slot_id);
                    }
                }
                UiCmd::Embed(xid) => {
                    if let Some(w) = float_win.take() {
                        w.close(&plugin);
                    }
                    if let Ok(mut guard) = plugin.lock() {
                        let handle = WindowHandle::from_x11(xid);
                        match guard.open_editor(handle) {
                            Ok(()) => {
                                eprintln!(
                                    "buschain-plugin-surface: embedded editor xid={xid:#x}"
                                );
                            }
                            Err(e) => {
                                eprintln!(
                                    "buschain-plugin-surface: embed failed ({e}) — trying float"
                                );
                                drop(guard);
                                match FloatEditor::open(&plugin, &title) {
                                    Ok(w) => {
                                        request_hyprland_float();
                                        float_win = Some(w);
                                        eprintln!("buschain-plugin-surface: floating editor");
                                    }
                                    Err(e2) => {
                                        eprintln!("buschain-plugin-surface: float failed ({e2})")
                                    }
                                }
                            }
                        }
                    }
                }
                UiCmd::Float => {
                    if float_win.is_some() {
                        // Already open.
                    } else {
                        match FloatEditor::open(&plugin, &title) {
                            Ok(w) => {
                                eprintln!("buschain-plugin-surface: floating editor");
                                request_hyprland_float();
                                float_win = Some(w);
                            }
                            Err(e) => eprintln!("buschain-plugin-surface: float failed ({e})"),
                        }
                    }
                }
                UiCmd::Resize { .. } => {}
            }
        }

        if let Ok(mut guard) = plugin.lock() {
            guard.service_run_loop();
        }

        if let Some(ref mut w) = float_win {
            if !w.pump() {
                if let Some(w) = float_win.take() {
                    w.close(&plugin);
                    notify_editor_closed(&mut editor_stream, slot_id);
                    eprintln!("buschain-plugin-surface: editor closed by user (DSP running)");
                }
            }
        }

        // Param feedback.
        if let (Some(ref mut stream), Ok(guard)) = (editor_stream.as_mut(), plugin.lock()) {
            if let Ok(cur) = guard.get_parameters() {
                drop(guard);
                if last.len() != cur.len() {
                    last.resize(cur.len(), 0.0);
                }
                for (i, p) in cur.iter().enumerate() {
                    if (p.value - last[i]).abs() > 1e-6 {
                        let ev = EditorParamEvent {
                            slot_id,
                            control_index: i as u32,
                            value: p.value as f32,
                            sample_offset: 0,
                        };
                        if let Ok(line) = serde_json::to_string(&ev) {
                            if writeln!(stream, "{line}").is_err() {
                                editor_stream = None;
                                break;
                            }
                        }
                        last[i] = p.value;
                    }
                }
            }
        }

        next += frame;
        let now = Instant::now();
        if next > now {
            thread::sleep(next - now);
        } else {
            next = now;
        }
    }

    if let Some(w) = float_win.take() {
        w.close(&plugin);
    }
    if let Ok(mut guard) = plugin.lock() {
        let _ = guard.stop_processing();
    }
    Ok(())
}

fn notify_editor_closed(stream: &mut Option<UnixStream>, slot_id: Uuid) {
    if let Some(ref mut s) = stream {
        let _ = writeln!(s, "EDITOR_CLOSED {slot_id}");
        let _ = s.flush();
    }
}

fn handle_ctrl(
    plugin: &Arc<Mutex<Plugin>>,
    ui_cmds: &Arc<Mutex<Vec<UiCmd>>>,
    stop: &Arc<AtomicBool>,
    line: &str,
) {
    if line.is_empty() {
        return;
    }
    let mut parts = line.split_whitespace();
    match parts.next() {
        Some("P") => {
            let idx: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let val: f32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            if let Ok(mut g) = plugin.lock() {
                if let Ok(params) = g.get_parameters() {
                    if let Some(p) = params.get(idx as usize) {
                        let id = p.id;
                        let _ = g.set_parameter(id, val.clamp(0.0, 1.0) as f64);
                    }
                }
            }
        }
        Some("E") => match parts.next() {
            Some("embed") => {
                let xid = parts
                    .next()
                    .and_then(|s| {
                        s.strip_prefix("0x")
                            .and_then(|h| u32::from_str_radix(h, 16).ok())
                            .or_else(|| s.parse().ok())
                    })
                    .unwrap_or(0);
                if xid != 0 {
                    if let Ok(mut q) = ui_cmds.lock() {
                        q.push(UiCmd::Embed(xid));
                    }
                }
            }
            Some("float") => {
                if let Ok(mut q) = ui_cmds.lock() {
                    q.push(UiCmd::Float);
                }
            }
            Some("close") => {
                if let Ok(mut q) = ui_cmds.lock() {
                    q.push(UiCmd::CloseEditor);
                }
            }
            _ => {}
        },
        Some("R") => {
            let w = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0u16);
            let h = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0u16);
            if w > 0 && h > 0 {
                if let Ok(mut q) = ui_cmds.lock() {
                    q.push(UiCmd::Resize { w, h });
                }
            }
        }
        Some("Q") => {
            stop.store(true, Ordering::Relaxed);
            if let Ok(mut q) = ui_cmds.lock() {
                q.push(UiCmd::Quit);
            }
        }
        Some("N") | Some("F") | Some("C") => {}
        _ => {}
    }
}

#[repr(C)]
struct ShmHeader {
    magic: AtomicU32,
    seq_in: AtomicU32,
    seq_out: AtomicU32,
    frames: AtomicU32,
    alive: AtomicU32,
    _pad: AtomicU32,
}

/// Hyprland tiles fresh XWayland windows — request floating for this helper's clients.
fn request_hyprland_float() {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    let pid = std::process::id();
    thread::Builder::new()
        .name("surface-hypr-float".into())
        .spawn(move || {
            for delay in [80u64, 200, 500] {
                thread::sleep(Duration::from_millis(delay));
                if let Ok(out) = std::process::Command::new("hyprctl")
                    .args(["clients", "-j"])
                    .output()
                {
                    if let Ok(text) = String::from_utf8(out.stdout) {
                        let key = format!("\"pid\": {pid}");
                        let key2 = format!("\"pid\":{pid}");
                        for block in text.split('{') {
                            if !(block.contains(&key) || block.contains(&key2)) {
                                continue;
                            }
                            if let Some(addr) = block
                                .split("\"address\":")
                                .nth(1)
                                .and_then(|s| s.trim().strip_prefix('"'))
                                .and_then(|s| s.split('"').next())
                            {
                                let ok = std::process::Command::new("hyprctl")
                                    .args(["dispatch", "setfloating", &format!("address:{addr}")])
                                    .stdout(std::process::Stdio::null())
                                    .stderr(std::process::Stdio::null())
                                    .status()
                                    .map(|s| s.success())
                                    .unwrap_or(false);
                                if ok {
                                    eprintln!(
                                        "buschain-plugin-surface: hyprctl setfloating {addr}"
                                    );
                                    return;
                                }
                            }
                        }
                    }
                }
                let _ = std::process::Command::new("hyprctl")
                    .args(["dispatch", "setfloating", "class:^(buschain-vst3)$"])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        })
        .ok();
}

fn open_shm(name: &str, size: usize) -> Result<std::fs::File> {
    let _ = size;
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::FromRawFd;
        let cname = std::ffi::CString::new(name.trim_start_matches('/'))?;
        let fd = unsafe { libc::shm_open(cname.as_ptr(), libc::O_RDWR, 0o600) };
        if fd >= 0 {
            return Ok(unsafe { std::fs::File::from_raw_fd(fd) });
        }
    }
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("XDG_RUNTIME_DIR unset — refuse temp-dir shm fallback"))?;
    let path = base.join(format!("buschain-{}.shm", name.trim_start_matches('/')));
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("open shm {}", path.display()))
}
