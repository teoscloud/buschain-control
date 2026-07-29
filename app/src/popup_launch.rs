//! Decide which mixer popup to open: Quickshell → egui → optional GTK legacy.

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn qs_mixer_script() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".config/quickshell/scripts/qs-mixer-toggle.sh");
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn qs_enabled() -> bool {
    matches!(
        std::env::var("BUSCHAIN_CONTROL_QS_MIXER").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    ) || qs_mixer_script().is_some()
        || which("qs").is_some()
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

fn spawn_detached(cmd: &mut Command) {
    let _ = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Open the portable/primary mixer popup for tray / ctl / waybar click.
pub fn spawn_mixer_popup() {
    // 1) Quickshell panel (when BUSCHAIN_CONTROL_QS_MIXER=1)
    if qs_enabled() {
        if let Some(script) = qs_mixer_script() {
            spawn_detached(&mut Command::new(script));
            return;
        }
        if which("qs").is_some() {
            let mut cmd = Command::new("qs");
            cmd.args(["ipc", "call", "mixer", "toggle"]);
            spawn_detached(&mut cmd);
            return;
        }
    }

    // 2) Explicit GTK legacy only when forced
    if matches!(
        std::env::var("BUSCHAIN_CONTROL_USE_GTK_MIXER").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    ) {
        let mixer = std::env::var("BUSCHAIN_CONTROL_MIXER")
            .map(PathBuf::from)
            .ok()
            .or_else(|| which("buschain-mixer-gtk"))
            .or_else(|| which("buschain-mixer"));
        if let Some(mixer) = mixer {
            let mut cmd = Command::new(mixer);
            cmd.arg("--toggle");
            spawn_detached(&mut cmd);
            return;
        }
    }

    // 3) egui portable popup (default fallback)
    if let Ok(exe) = std::env::current_exe() {
        let mut cmd = Command::new(exe);
        cmd.args(["--popup", "playback"]);
        spawn_detached(&mut cmd);
        return;
    }
    let mut cmd = Command::new("buschain-control");
    cmd.args(["--popup", "playback"]);
    spawn_detached(&mut cmd);
}
