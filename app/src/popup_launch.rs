//! Mixer popup router for tray / ctl / waybar.
//!
//! Order (see `docs/HANDOVER-QUICKSHELL.md`):
//! 1. Quickshell — if `BUSCHAIN_CONTROL_QS_MIXER=1`, or toggle script exists, or `qs` is on PATH
//! 2. GTK layer-shell — when `buschain-mixer-gtk` is available (opt out: `USE_GTK_MIXER=0`)
//! 3. egui — `buschain-control --popup` (last resort)
//!
//! GTK open must never treat `--toggle` exit-0 (close) as “GTK failed → egui”.
//! Popup settle never blocks the daemon IPC mutex — `spawn_mixer_popup_async`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn env_truthy(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn env_falsy(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("0") | Ok("false") | Ok("FALSE") | Ok("no") | Ok("NO")
    )
}

fn lat_trace(msg: &str) {
    if !env_truthy("BUSCHAIN_CONTROL_LAT_TRACE") {
        return;
    }
    eprintln!("[buschain-lat] {msg}");
    append_spawn_log(&format!("lat {msg}"));
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("buschain-control")
}

fn mixer_pid_path() -> PathBuf {
    runtime_dir().join("mixer.pid")
}

fn mixer_spawn_log_path() -> PathBuf {
    runtime_dir().join("mixer-spawn.log")
}

fn append_spawn_log(line: &str) {
    let _ = fs::create_dir_all(runtime_dir());
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(mixer_spawn_log_path())
    {
        let _ = writeln!(f, "{line}");
    }
}

fn qs_mixer_script() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".config/quickshell/scripts/qs-mixer-toggle.sh");
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        for dir in std::env::split_paths(&paths) {
            let p = dir.join(bin);
            if p.is_file() {
                return Some(p);
            }
        }
        None
    })
}

fn spawn_detached(cmd: &mut Command) -> bool {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

/// True when `/proc/<pid>` is a non-zombie process.
fn proc_is_live(pid: u32) -> bool {
    let stat = PathBuf::from(format!("/proc/{pid}/stat"));
    let Ok(text) = fs::read_to_string(&stat) else {
        return false;
    };
    // `pid (comm) state ...` — comm may contain spaces/parens; state follows last `)`.
    let Some(rparen) = text.rfind(')') else {
        return false;
    };
    let rest = text.get(rparen + 2..).unwrap_or("");
    let state = rest.chars().next().unwrap_or('?');
    state != 'Z' && state != '?'
}

/// Pidfile owner looks like our mixer (rejects reused PIDs / zombies).
fn proc_looks_like_mixer(pid: u32) -> bool {
    let cmdline = PathBuf::from(format!("/proc/{pid}/cmdline"));
    let Ok(bytes) = fs::read(&cmdline) else {
        return false;
    };
    let s = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    s.contains("buschain-mixer") || s.contains("buschain_mixer")
}

/// Live mixer PID from pidfile, or `None` after clearing stale/zombie entries.
fn live_mixer_pid() -> Option<u32> {
    let path = mixer_pid_path();
    let Ok(text) = fs::read_to_string(&path) else {
        return None;
    };
    let Ok(pid) = text.trim().parse::<u32>() else {
        let _ = fs::remove_file(&path);
        return None;
    };
    if pid == 0 || !proc_is_live(pid) || !proc_looks_like_mixer(pid) {
        let _ = fs::remove_file(&path);
        append_spawn_log(&format!("cleared stale mixer.pid pid={pid}"));
        return None;
    }
    Some(pid)
}

fn clear_mixer_pidfile_if(pid: u32) {
    let path = mixer_pid_path();
    if let Ok(text) = fs::read_to_string(&path) {
        if text.trim().parse::<u32>().ok() == Some(pid) {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Close a live mixer. Success means the click was handled by GTK (do not egui).
fn close_live_mixer(pid: u32) -> bool {
    append_spawn_log(&format!("toggle-close pid={pid}"));
    let t0 = Instant::now();
    let status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !matches!(status, Ok(s) if s.success()) {
        // Process may already be gone — still treat as handled if no longer live.
        if proc_is_live(pid) {
            append_spawn_log(&format!("toggle-close kill failed pid={pid}"));
            return false;
        }
    }
    // Brief wait so the next open does not race the dying process.
    for _ in 0..20 {
        thread::sleep(Duration::from_millis(25));
        if !proc_is_live(pid) {
            break;
        }
    }
    clear_mixer_pidfile_if(pid);
    lat_trace(&format!(
        "toggle-close done pid={pid} {}ms",
        t0.elapsed().as_millis()
    ));
    true
}

/// Max wait for pidfile / still-running after spawn (replaces 450+500ms sleeps).
const OPEN_READY_CAP: Duration = Duration::from_millis(280);
/// Watch window for late GTK abort → egui fallthrough (async; does not block IPC).
const LATE_DEATH_WATCH: Duration = Duration::from_millis(700);

/// Spawn mixer **open** path; poll readiness instead of blind sleeps. Reaps child.
fn spawn_mixer_open(mixer: &Path) -> bool {
    let _ = fs::create_dir_all(runtime_dir());
    let log_path = mixer_spawn_log_path();
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();

    let mut cmd = if mixer
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("py"))
    {
        let py = std::env::var("BUSCHAIN_CONTROL_PYTHON")
            .ok()
            .filter(|p| Path::new(p).is_file())
            .unwrap_or_else(|| "python3".into());
        let mut c = Command::new(py);
        c.arg(mixer).arg("--open");
        c
    } else {
        let mut c = Command::new(mixer);
        c.arg("--open");
        c
    };

    append_spawn_log(&format!("spawn-open mixer={}", mixer.display()));
    let t0 = Instant::now();

    // Avoid user icon themes that abort on missing `image-missing` (WhiteSur-dark).
    cmd.env("GTK_ICON_THEME_NAME", "Adwaita");

    cmd.stdin(Stdio::null()).stdout(Stdio::null());
    if let Some(f) = log_file {
        cmd.stderr(Stdio::from(f));
    } else {
        cmd.stderr(Stdio::null());
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            append_spawn_log(&format!("spawn-open failed: {e}"));
            return false;
        }
    };
    lat_trace(&format!("spawn-open child pid={}", child.id()));

    // Readiness: pidfile (claim_pid before Gtk.init) or still-running at cap.
    loop {
        match child.try_wait() {
            Ok(None) => {}
            Ok(Some(status)) => {
                if status.success() && live_mixer_pid().is_some() {
                    lat_trace(&format!(
                        "spawn-open already-live {}ms",
                        t0.elapsed().as_millis()
                    ));
                    append_spawn_log("spawn-open: already live (no-op ok)");
                    return true;
                }
                append_spawn_log(&format!(
                    "spawn-open exited early status={status:?} (see {})",
                    log_path.display()
                ));
                lat_trace(&format!(
                    "spawn-open early-exit {}ms",
                    t0.elapsed().as_millis()
                ));
                return false;
            }
            Err(e) => {
                append_spawn_log(&format!("spawn-open try_wait err: {e}"));
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }

        if live_mixer_pid().is_some() {
            let pid = child.id();
            append_spawn_log(&format!(
                "spawn-open ok pid={pid} ready={}ms",
                t0.elapsed().as_millis()
            ));
            lat_trace(&format!("spawn-open ready {}ms", t0.elapsed().as_millis()));
            thread::spawn(move || {
                watch_mixer_child(child, &log_path);
            });
            return true;
        }

        if t0.elapsed() >= OPEN_READY_CAP {
            // Still running — optimistic success; watch for late abort → egui.
            let pid = child.id();
            append_spawn_log(&format!(
                "spawn-open ok pid={pid} optimistic={}ms",
                t0.elapsed().as_millis()
            ));
            lat_trace(&format!(
                "spawn-open optimistic {}ms",
                t0.elapsed().as_millis()
            ));
            thread::spawn(move || {
                watch_mixer_child(child, &log_path);
            });
            return true;
        }

        thread::sleep(Duration::from_millis(15));
    }
}

/// Reap child; if it dies soon without a live pidfile, fall through to egui.
fn watch_mixer_child(mut child: std::process::Child, log_path: &Path) {
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            Ok(None) => {
                if t0.elapsed() >= LATE_DEATH_WATCH {
                    // Stable — block on wait so we don't zombie.
                    let _ = child.wait();
                    return;
                }
                thread::sleep(Duration::from_millis(40));
            }
            Ok(Some(status)) => {
                if live_mixer_pid().is_some() {
                    return;
                }
                append_spawn_log(&format!(
                    "spawn-open died after ack status={status:?} (see {}) — egui fallback",
                    log_path.display()
                ));
                lat_trace("spawn-open late-death → egui");
                let _ = try_egui_popup();
                return;
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
    }
}

/// Prefer Quickshell when opted in **or** when a QS toggle bridge is installed.
fn qs_mixer_available() -> bool {
    if env_truthy("BUSCHAIN_CONTROL_QS_MIXER") {
        return true;
    }
    if qs_mixer_script().is_some() {
        return true;
    }
    which("qs").is_some()
}

fn try_quickshell_mixer() -> bool {
    if !qs_mixer_available() {
        return false;
    }
    if let Some(script) = qs_mixer_script() {
        if let Ok(status) = Command::new(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            if status.success() {
                append_spawn_log("qs mixer: toggle script ok");
                return true;
            }
            append_spawn_log("qs mixer: toggle script failed");
        }
    }
    if which("qs").is_some() {
        if let Ok(status) = Command::new("qs")
            .args(["ipc", "call", "mixer", "toggle"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            if status.success() {
                append_spawn_log("qs mixer: ipc call mixer toggle ok");
                return true;
            }
            append_spawn_log("qs mixer: ipc call failed");
        }
    }
    false
}

fn resolve_gtk_mixer() -> Option<PathBuf> {
    // 1) Env (dev bootstrap → checkout launcher; packaged wrap may set this too)
    // 2) Checkout path / cargo manifest sibling
    // 3) PATH (packaged install)
    // Do not hardcode ~/.local/bin.
    if let Ok(p) = std::env::var("BUSCHAIN_CONTROL_MIXER") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(&home)
            .join("Projects/buschain-control/packaging/mixer/buschain-mixer-gtk");
        if p.is_file() {
            return Some(p);
        }
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let p = manifest.join("../packaging/mixer/buschain-mixer-gtk");
    if let Ok(c) = p.canonicalize() {
        if c.is_file() {
            return Some(c);
        }
    }
    which("buschain-mixer-gtk").or_else(|| which("buschain-mixer"))
}

fn ensure_mixer_css(mixer: &Path) {
    if std::env::var_os("BUSCHAIN_CONTROL_MIXER_CSS").is_some() {
        return;
    }
    if let Some(dir) = mixer.parent() {
        let css = if dir.ends_with("legacy") {
            dir.join("style.css")
        } else {
            dir.join("legacy/style.css")
        };
        if css.is_file() {
            std::env::set_var("BUSCHAIN_CONTROL_MIXER_CSS", &css);
        }
    }
}

/// GTK layer-shell panel (general-desktop preferred when packaged / on PATH).
///
/// Opt out with `BUSCHAIN_CONTROL_USE_GTK_MIXER=0`. Toggle: close if live, else
/// `--open`. Never treats a successful close as GTK unavailability.
pub fn try_gtk_mixer() -> bool {
    if env_falsy("BUSCHAIN_CONTROL_USE_GTK_MIXER") {
        return false;
    }
    let Some(mixer) = resolve_gtk_mixer() else {
        append_spawn_log("try_gtk_mixer: no mixer binary");
        return false;
    };
    ensure_mixer_css(&mixer);

    if let Some(pid) = live_mixer_pid() {
        return close_live_mixer(pid);
    }

    spawn_mixer_open(&mixer)
}

fn resolve_app_bin() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if exe.is_file() {
            let name = exe.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name == "buschain-control" || name.starts_with("buschain-control") {
                return Some(exe);
            }
        }
    }
    if let Ok(ctl) = std::env::var("BUSCHAIN_CONTROL_CTL") {
        let ctl = PathBuf::from(ctl);
        if let Some(dir) = ctl.parent() {
            let sibling = dir.join("buschain-control");
            if sibling.is_file() {
                return Some(sibling);
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        for rel in [
            "Projects/buschain-control/target/debug/buschain-control",
            "Projects/buschain-control/target/release/buschain-control",
        ] {
            let p = PathBuf::from(&home).join(rel);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    which("buschain-control")
}

fn try_egui_popup() -> bool {
    let Some(bin) = resolve_app_bin() else {
        return false;
    };
    let mut cmd = Command::new(bin);
    cmd.args(["--popup", "playback"]);
    cmd.env("BUSCHAIN_CONTROL_SKIP_BOOTSTRAP", "1");
    spawn_detached(&mut cmd)
}

/// Open/toggle the mixer popup (blocking — for background worker only).
pub fn spawn_mixer_popup() {
    let t0 = Instant::now();
    lat_trace("spawn_mixer_popup start");
    if try_quickshell_mixer() {
        lat_trace(&format!(
            "spawn_mixer_popup qs {}ms",
            t0.elapsed().as_millis()
        ));
        return;
    }
    if try_gtk_mixer() {
        lat_trace(&format!(
            "spawn_mixer_popup gtk {}ms",
            t0.elapsed().as_millis()
        ));
        return;
    }
    if try_egui_popup() {
        append_spawn_log("fallback: egui popup (no QS / GTK)");
        lat_trace(&format!(
            "spawn_mixer_popup egui {}ms",
            t0.elapsed().as_millis()
        ));
        return;
    }
    eprintln!(
        "buschain-control: could not open mixer popup \
         (need Quickshell, buschain-mixer-gtk, or buschain-control --popup)"
    );
    append_spawn_log("fallback: all popup paths failed");
}

/// Fire-and-forget popup so `PopupPlayback` does not hold the daemon mutex.
pub fn spawn_mixer_popup_async() {
    lat_trace("spawn_mixer_popup_async");
    thread::spawn(|| {
        spawn_mixer_popup();
    });
}
