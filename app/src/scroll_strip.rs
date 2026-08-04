//! Always-on Master HW scroll strip (GtkLayerShell hit target).
//!
//! Waybar custom `on-scroll` cannot guarantee 1 wheel notch → 1 Adjust: it
//! forkExecs once per coalesced SMOOTH event and discards magnitude. The strip
//! receives the real delta and pokes `AdjustHwVolume` directly.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use crate::ipc;

fn env_truthy(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES") | Ok("on") | Ok("ON")
    )
}

/// GTK scroll strip is opt-in (`BUSCHAIN_CONTROL_SCROLL_STRIP=1`). QS strip unchanged.
fn strip_enabled() -> bool {
    env_truthy("BUSCHAIN_CONTROL_SCROLL_STRIP")
}

/// Quickshell owns the Master HW hit target — do not spawn the GTK strip.
fn qs_owns_strip() -> bool {
    env_truthy("BUSCHAIN_CONTROL_QS_STRIP") || env_truthy("BUSCHAIN_CONTROL_QS_MIXER")
}

fn resolve_scroll_strip() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("BUSCHAIN_CONTROL_SCROLL_STRIP_BIN") {
        let pb = PathBuf::from(&p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(h) = &home {
        candidates.push(
            h.join("Projects/buschain-control/packaging/scroll-strip/buschain-scroll-strip"),
        );
    }
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        candidates.push(
            PathBuf::from(manifest)
                .join("../packaging/scroll-strip/buschain-scroll-strip"),
        );
    }
    // Walk up from current_exe (target/debug/buschain-control → repo root).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(
                dir.join("../../packaging/scroll-strip/buschain-scroll-strip"),
            );
            candidates.push(dir.join("buschain-scroll-strip"));
        }
    }
    candidates.push(PathBuf::from("buschain-scroll-strip"));
    for c in candidates {
        if let Ok(canon) = c.canonicalize() {
            if canon.is_file() {
                return Some(canon);
            }
        } else if c.is_file() {
            return Some(c);
        }
    }
    // PATH
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join("buschain-scroll-strip");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn kill_existing_strip() {
    let Ok(dir) = ipc::runtime_dir() else {
        return;
    };
    let path = dir.join("scroll-strip.pid");
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(pid) = text.trim().parse::<i32>() {
            if pid > 1 {
                let _ = Command::new("kill")
                    .args(["-TERM", &pid.to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
        let _ = std::fs::remove_file(&path);
    }
    // Also reap orphans (pidfile missing / stale).
    let _ = Command::new("pkill")
        .args(["-f", "buschain-scroll-strip.py"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Spawn the scroll strip after IPC is up (non-blocking).
pub fn spawn_scroll_strip_async() {
    if qs_owns_strip() {
        // Reap any leftover GTK strip so QS can own the hit target cleanly.
        thread::spawn(|| {
            kill_existing_strip();
            eprintln!("buschain-control: scroll strip → Quickshell (BUSCHAIN_CONTROL_QS_STRIP)");
        });
        return;
    }
    if !strip_enabled() {
        return;
    }
    thread::spawn(|| {
        kill_existing_strip();
        // Let the unix listener bind before the strip pokes the socket.
        thread::sleep(Duration::from_millis(200));
        let Some(bin) = resolve_scroll_strip() else {
            eprintln!(
                "buschain-control: scroll strip not found (set BUSCHAIN_CONTROL_SCROLL_STRIP_BIN)"
            );
            return;
        };
        let mut cmd = Command::new(&bin);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Ok(py) = std::env::var("BUSCHAIN_CONTROL_PYTHON") {
            cmd.env("BUSCHAIN_CONTROL_PYTHON", py);
        }
        if let Ok(ctl) = std::env::var("BUSCHAIN_CONTROL_CTL") {
            cmd.env("BUSCHAIN_CONTROL_CTL", ctl);
        }
        cmd.env("GTK_ICON_THEME_NAME", "Adwaita");
        match cmd.spawn() {
            Ok(_) => eprintln!("buschain-control: scroll strip → {}", bin.display()),
            Err(e) => eprintln!("buschain-control: scroll strip spawn: {e}"),
        }
    });
}
