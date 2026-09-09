//! Local-dev bootstrap so plain `cargo run` is enough.
//!
//! Builds plugins + `buschain-ctl` (waybar), clears leftover headless daemons,
//! and points LADSPA/LV2 at the checkout plugin trees.
//!
//! Skipped for `--popup` / packaged runs (`BUSCHAIN_CONTROL_SKIP_BOOTSTRAP=1`).
//! Enabled for debug builds, or when `BUSCHAIN_CONTROL_DEV=1`.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

fn env_flag(name: &str) -> bool {
    matches!(
        env::var(name).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn should_run() -> bool {
    if env_flag("BUSCHAIN_CONTROL_SKIP_BOOTSTRAP") {
        return false;
    }
    if env_flag("BUSCHAIN_CONTROL_DEV") {
        return true;
    }
    cfg!(debug_assertions)
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."))
}

fn target_dir(root: &Path) -> PathBuf {
    env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
}

fn prepend_path(var: &str, dirs: &[PathBuf]) {
    let extra: Vec<String> = dirs
        .iter()
        .filter(|p| p.is_dir())
        .map(|p| p.display().to_string())
        .collect();
    if extra.is_empty() {
        return;
    }
    let joined = extra.join(":");
    match env::var_os(var) {
        Some(prev) => {
            let prev = prev.to_string_lossy();
            if !prev.split(':').any(|p| extra.iter().any(|e| e == p)) {
                env::set_var(var, format!("{joined}:{prev}"));
            }
        }
        None => env::set_var(var, &joined),
    }
}

fn ensure_plugins(root: &Path) {
    let builtins = root.join("plugins/buschain-builtins/build/buschain_builtins.so");
    let denoiser = root.join("plugins/buschain-denoiser/build/buschain_denoiser.so");
    let reverb = root.join("plugins/buschain-reverb/build/buschain_reverb.so");
    // Rebuild when any core .so is missing (reverb is new — don't skip on builtins alone).
    if builtins.is_file() && denoiser.is_file() && reverb.is_file() {
        return;
    }
    eprintln!("buschain-control: building LADSPA plugins…");
    let status = Command::new("make")
        .arg("-C")
        .arg(root)
        .arg("plugins")
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => eprintln!("buschain-control: make plugins exited {s}"),
        Err(e) => eprintln!("buschain-control: make plugins failed: {e}"),
    }
}

fn ensure_ctl(root: &Path, target: &Path) {
    let ctl = target.join("debug/buschain-ctl");
    // Incremental — no-op when already fresh; keeps waybar's absolute path working.
    eprintln!("buschain-control: ensuring buschain-ctl (waybar / QS)…");
    let status = Command::new("cargo")
        .current_dir(root)
        .args(["build", "-p", "buschain-tools", "--bin", "buschain-ctl"])
        .status();
    match status {
        Ok(s) if s.success() => {
            if ctl.is_file() {
                env::set_var("BUSCHAIN_CONTROL_CTL", &ctl);
            }
        }
        Ok(s) => eprintln!("buschain-control: cargo build buschain-ctl exited {s}"),
        Err(e) => eprintln!("buschain-control: cargo build buschain-ctl failed: {e}"),
    }
}

fn stop_leftover_daemons(root: &Path) {
    let _ = root;
    // A live tray instance still owns the graph. Stealing its socket used to
    // start a second worker on leftover buschain_* nodes (silent desktop +
    // stuck Loading overlay). Ask it to Quit+restore first.
    if buschain_control::ipc::Client::ping() {
        eprintln!("buschain-control: previous instance is live — asking it to quit…");
        let _ = buschain_control::ipc::Client::call(&buschain_control::ipc::Request::Shutdown);
        let deadline = std::time::Instant::now() + Duration::from_secs(6);
        while std::time::Instant::now() < deadline {
            if !buschain_control::ipc::Client::ping() {
                break;
            }
            thread::sleep(Duration::from_millis(120));
        }
        if buschain_control::ipc::Client::ping() {
            eprintln!("buschain-control: previous instance wedged — restoring desktop audio");
            let _ = buschain_control::audio::graph::restore_desktop_now(None);
        }
    }
    // Headless daemon only — never pkill buschain-control (would kill us / tray).
    let _ = Command::new("pkill").args(["-x", "buschain-daemon"]).status();
    let _ = Command::new("pkill").args(["-f", "/buschain-daemon"]).status();
    let _ = Command::new("pkill")
        .args(["-f", "target/.*/buschain-daemon"])
        .status();
    if let Ok(out) = Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", "buschain-control.service"])
        .status()
    {
        if out.success() {
            eprintln!("buschain-control: stopping leftover buschain-control.service");
            let _ = Command::new("systemctl")
                .args(["--user", "stop", "buschain-control.service"])
                .status();
        }
    }
    thread::sleep(Duration::from_millis(200));
    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    if let Some(runtime) = runtime {
        let sock = runtime.join("buschain-control/daemon.sock");
        if !buschain_control::ipc::Client::ping() {
            let _ = std::fs::remove_file(&sock);
        }
    }
}

/// Run once at process start for local `cargo run` workflows.
pub fn run() {
    if !should_run() {
        // Still expose checkout plugins when present (nix develop / release-from-tree).
        let root = workspace_root();
        let plugin_dirs = [
            root.join("plugins/buschain-denoiser/build"),
            root.join("plugins/buschain-gate/build"),
            root.join("plugins/buschain-reverb/build"),
            root.join("plugins/buschain-builtins/build"),
        ];
        prepend_path("LADSPA_PATH", &plugin_dirs);
        prepend_path("LV2_PATH", &plugin_dirs);
        return;
    }

    let root = workspace_root();
    let target = target_dir(&root);

    stop_leftover_daemons(&root);
    ensure_plugins(&root);
    ensure_ctl(&root, &target);

    let plugin_dirs = [
        root.join("plugins/buschain-denoiser/build"),
        root.join("plugins/buschain-gate/build"),
        root.join("plugins/buschain-reverb/build"),
        root.join("plugins/buschain-builtins/build"),
    ];
    prepend_path("LADSPA_PATH", &plugin_dirs);
    prepend_path("LV2_PATH", &plugin_dirs);

    let debug_bin = target.join("debug");
    let waybar_dir = root.join("packaging/waybar");
    if debug_bin.is_dir() {
        prepend_path("PATH", &[debug_bin, waybar_dir]);
    } else {
        prepend_path("PATH", &[waybar_dir]);
    }

    let mixer_css = root.join("packaging/mixer/legacy/style.css");
    let mixer_local = root.join("packaging/mixer/buschain-mixer-gtk");
    // Checkout launcher + nix-develop Python (BUSCHAIN_CONTROL_PYTHON). Override a
    // stale nix-store mixer left in the environment from an older shellHook.
    let store_stale = env::var("BUSCHAIN_CONTROL_MIXER")
        .map(|p| p.contains("/nix/store/"))
        .unwrap_or(false);
    if mixer_local.is_file()
        && (env::var_os("BUSCHAIN_CONTROL_MIXER").is_none() || store_stale)
    {
        env::set_var("BUSCHAIN_CONTROL_MIXER", &mixer_local);
    } else if env::var_os("BUSCHAIN_CONTROL_MIXER").is_none() {
        // Packaged install: first buschain-mixer-gtk on PATH.
        if let Ok(path) = env::var("PATH") {
            for dir in env::split_paths(&path) {
                let p = dir.join("buschain-mixer-gtk");
                if p.is_file() {
                    env::set_var("BUSCHAIN_CONTROL_MIXER", &p);
                    break;
                }
            }
        }
    }
    // Always prefer checkout CSS next to the local launcher.
    if mixer_css.is_file()
        && (env::var_os("BUSCHAIN_CONTROL_MIXER_CSS").is_none()
            || env::var("BUSCHAIN_CONTROL_MIXER_CSS")
                .map(|p| p.contains("/nix/store/"))
                .unwrap_or(false))
    {
        env::set_var("BUSCHAIN_CONTROL_MIXER_CSS", &mixer_css);
    }
    // Prefer GTK layer-shell popup when the checkout mixer is on PATH (QS still wins).
    if env::var_os("BUSCHAIN_CONTROL_USE_GTK_MIXER").is_none() {
        env::set_var("BUSCHAIN_CONTROL_USE_GTK_MIXER", "1");
    }

    let strip_local = root.join("packaging/scroll-strip/buschain-scroll-strip");
    let strip_stale = env::var("BUSCHAIN_CONTROL_SCROLL_STRIP_BIN")
        .map(|p| p.contains("/nix/store/"))
        .unwrap_or(false);
    if strip_local.is_file()
        && (env::var_os("BUSCHAIN_CONTROL_SCROLL_STRIP_BIN").is_none() || strip_stale)
    {
        env::set_var("BUSCHAIN_CONTROL_SCROLL_STRIP_BIN", &strip_local);
    }

    if let Ok(ctl) = env::var("BUSCHAIN_CONTROL_CTL") {
        eprintln!("buschain-control: waybar ctl → {ctl}");
    }
    if let Ok(mix) = env::var("BUSCHAIN_CONTROL_MIXER") {
        eprintln!("buschain-control: mixer popup → {mix}");
    }
    if let Ok(strip) = env::var("BUSCHAIN_CONTROL_SCROLL_STRIP_BIN") {
        eprintln!("buschain-control: scroll strip → {strip}");
    }
    if let Ok(py) = env::var("BUSCHAIN_CONTROL_PYTHON") {
        eprintln!("buschain-control: mixer python → {py}");
    }
}
