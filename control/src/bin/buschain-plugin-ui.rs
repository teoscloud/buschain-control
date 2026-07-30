//! Out-of-process plugin editor helper (P5).
//!
//! Does not process audio. Opens a native VST3 editor when possible and streams
//! `EditorParamEvent` JSON lines to the host unix socket.
//!
//! Linux: must call `Plugin::service_run_loop` + `PluginWindow::service_platform_events`
//! every frame — VSTGUI/JUCE editors stay white without that timer/fd pump.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use buschain_engine::{EditorParamEvent, OpenEditorRequest};

fn main() {
    // Prefer X11 / XWayland for VST3 embed (kPlatformTypeX11EmbedWindowID).
    // Pure Wayland parents are not supported by the Steinberg Linux plug-view API.
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!(
            "buschain-plugin-ui: DISPLAY unset — native VST3 editors need X11/XWayland \
             (on Wayland: ensure XWayland is running, or unset WAYLAND_DISPLAY for this helper)"
        );
    }

    if let Err(e) = run() {
        eprintln!("buschain-plugin-ui: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut request_json = None;
    while let Some(a) = args.next() {
        if a == "--request" {
            request_json = args.next();
        }
    }
    let Some(json) = request_json else {
        bail!("usage: buschain-plugin-ui --request <OpenEditorRequest json>");
    };
    let req: OpenEditorRequest =
        serde_json::from_str(&json).context("parse OpenEditorRequest")?;

    let mut stream = connect_retry(&req.socket_path, 40)?;
    let fmt = req.format.to_ascii_lowercase();
    let path = req.plugin_path.clone();

    if fmt.contains("vst3") || path.ends_with(".vst3") {
        run_vst3_editor(&req, &mut stream)?;
    } else {
        eprintln!(
            "buschain-plugin-ui: headless editor for {} ({}) — use egui rack params",
            req.plugin_key, req.format
        );
        loop {
            thread::sleep(Duration::from_secs(1));
            if stream.write_all(b"\n").is_err() {
                break;
            }
        }
    }
    Ok(())
}

fn connect_retry(path: &str, attempts: u32) -> Result<UnixStream> {
    let mut last = None;
    for _ in 0..attempts {
        match UnixStream::connect(path) {
            Ok(s) => return Ok(s),
            Err(e) => {
                last = Some(e);
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Err(last
        .map(Into::into)
        .unwrap_or_else(|| anyhow::anyhow!("connect {path} failed")))
}

fn run_vst3_editor(req: &OpenEditorRequest, stream: &mut UnixStream) -> Result<()> {
    use vst3_host::host::Vst3Host;
    use vst3_host::window::PluginWindow;

    let mut host = Vst3Host::builder()
        .sample_rate(48_000.0)
        .block_size(256)
        .input_channels(2)
        .output_channels(2)
        .build()
        .map_err(|e| anyhow::anyhow!("VST3 host: {e}"))?;

    let mut plugin = host
        .load_plugin(&req.plugin_path)
        .or_else(|_| host.load_plugin_class(&req.plugin_path, &req.plugin_key))
        .map_err(|e| anyhow::anyhow!("load {}: {e}", req.plugin_path))?;

    if let Some(ref blob) = req.state_blob {
        if !blob.is_empty() {
            if let Err(e) = plugin.load_state(blob) {
                eprintln!("buschain-plugin-ui: load_state failed ({e}) — defaults");
            }
        }
    }

    let plugin = Arc::new(Mutex::new(plugin));
    let mut window = PluginWindow::new(Arc::clone(&plugin));
    let editor_open = match window.open() {
        Ok(()) => {
            eprintln!(
                "buschain-plugin-ui: VST3 editor for {} (pumping IRunLoop ~60fps)",
                req.plugin_key
            );
            true
        }
        Err(e) => {
            eprintln!("buschain-plugin-ui: native editor unavailable ({e}) — param poll only");
            false
        }
    };

    let mut last: Vec<f64> = plugin
        .lock()
        .ok()
        .and_then(|p| p.get_parameters().ok())
        .map(|ps| ps.iter().map(|p| p.value).collect())
        .unwrap_or_default();

    let frame = Duration::from_millis(16);
    let mut next = Instant::now();

    loop {
        // 1) Linux IRunLoop — timers + fd handlers (paint / idle for VSTGUI/JUCE).
        if let Ok(mut guard) = plugin.lock() {
            guard.service_run_loop();
        }

        // 2) Host-side window resize / DPI work from IPlugFrame.
        let _ = window.service_platform_events();

        // 3) Param feedback → ControlQueue via unix socket.
        if let Ok(guard) = plugin.lock() {
            if let Ok(cur) = guard.get_parameters() {
                drop(guard);
                if last.len() != cur.len() {
                    last.resize(cur.len(), 0.0);
                }
                for (i, p) in cur.iter().enumerate() {
                    let prev = last[i];
                    if (p.value - prev).abs() > 1e-6 {
                        let ev = EditorParamEvent {
                            slot_id: req.slot_id,
                            control_index: i as u32,
                            value: p.value as f32,
                            sample_offset: 0,
                        };
                        if writeln!(stream, "{}", serde_json::to_string(&ev)?).is_err() {
                            window.close();
                            return Ok(());
                        }
                        last[i] = p.value;
                    }
                }
            }
        }

        if editor_open && !window.is_open() {
            window.close();
            return Ok(());
        }

        // Pace ~60fps without busy-spinning.
        next += frame;
        let now = Instant::now();
        if next > now {
            thread::sleep(next - now);
        } else {
            next = now;
        }
    }
}
