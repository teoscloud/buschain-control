//! Mixer popup router for tray / ctl / waybar.
//!
//! Order (see `docs/HANDOVER-GTK-WAYBAR.md`):
//! 1. Quickshell — only if `BUSCHAIN_CONTROL_QS_MIXER=1`
//! 2. GTK layer-shell — `buschain-mixer-gtk` (opt out: `BUSCHAIN_CONTROL_USE_GTK_MIXER=0`)
//! 3. egui — `buschain-control --popup`

use std::path::PathBuf;
use std::process::{Command, Stdio};

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

/// Spawn and confirm the child stays up briefly (catches `import gi` failures).
fn spawn_alive(cmd: &mut Command) -> bool {
    let mut child = match cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    std::thread::sleep(std::time::Duration::from_millis(280));
    match child.try_wait() {
        Ok(None) => {
            // Still running — detach (do not wait on Drop).
            std::mem::forget(child);
            true
        }
        _ => false,
    }
}

/// Quickshell only when explicitly opted in.
fn try_quickshell_mixer() -> bool {
    if !env_truthy("BUSCHAIN_CONTROL_QS_MIXER") {
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
                return true;
            }
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
            return status.success();
        }
    }
    false
}

fn resolve_gtk_mixer() -> Option<PathBuf> {
    // Prefer nix-wrapped bins (have gi). Bare checkout `python3` usually lacks gi
    // and would fall through to egui after spawn_alive fails.
    if let Ok(p) = std::env::var("BUSCHAIN_CONTROL_MIXER") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(p) = which("buschain-mixer-gtk").or_else(|| which("buschain-mixer")) {
        return Some(p);
    }
    // Hyprland / tray often omit ~/.local/bin from PATH.
    if let Ok(home) = std::env::var("HOME") {
        for rel in [
            ".local/bin/buschain-mixer-gtk",
            ".local/bin/buschain-mixer",
        ] {
            let p = PathBuf::from(&home).join(rel);
            if p.is_file() {
                return Some(p);
            }
        }
        // Checkout launcher last — needs `nix develop` python with gi.
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
    None
}

/// GTK layer-shell panel. Opt out with `BUSCHAIN_CONTROL_USE_GTK_MIXER=0`.
pub fn try_gtk_mixer() -> bool {
    if env_falsy("BUSCHAIN_CONTROL_USE_GTK_MIXER") {
        return false;
    }
    let Some(mixer) = resolve_gtk_mixer() else {
        return false;
    };
    // Ensure CSS for the checkout launcher / raw .py path.
    if std::env::var_os("BUSCHAIN_CONTROL_MIXER_CSS").is_none() {
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
    // Raw .py — use python3; wrapped nix bin has a working shebang + gi.
    let ok = if mixer
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("py"))
    {
        let mut cmd = Command::new("python3");
        cmd.arg(&mixer).arg("--toggle");
        spawn_alive(&mut cmd)
    } else {
        let mut cmd = Command::new(&mixer);
        cmd.arg("--toggle");
        spawn_alive(&mut cmd)
    };
    ok
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

/// Open the mixer popup for tray / ctl / waybar click.
pub fn spawn_mixer_popup() {
    if try_quickshell_mixer() {
        return;
    }
    if try_gtk_mixer() {
        return;
    }
    if try_egui_popup() {
        return;
    }
    eprintln!(
        "buschain-control: could not open mixer popup \
         (need buschain-mixer-gtk on PATH, or buschain-control --popup)"
    );
}
