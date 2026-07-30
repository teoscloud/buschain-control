//! Ask Hyprland to float VST3 surface windows (XWayland toplevels tile by default).

use std::process::Command;
use std::thread;
use std::time::Duration;

/// WM_CLASS instance/class we stamp on BusChain-owned VST3 surfaces.
pub const VST3_SURFACE_CLASS: &str = "buschain-vst3";

/// True when running under a Hyprland session.
pub fn is_hyprland() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

/// Best-effort: mark matching XWayland clients as floating.
///
/// Hyprland tiles new XWayland windows unless rules/hints say otherwise. We set
/// `WM_CLASS=buschain-vst3` on our surfaces and dispatch `setfloating` after map.
pub fn request_float_vst3_surfaces() {
    if !is_hyprland() {
        return;
    }
    thread::Builder::new()
        .name("buschain-hypr-float".into())
        .spawn(|| {
            // Give the compositor a beat to register the mapped XWayland client.
            for delay_ms in [50u64, 150, 400] {
                thread::sleep(Duration::from_millis(delay_ms));
                // Prefer setfloating (Hyprland ≥0.40); fall back to togglefloating.
                if hyprctl(&[
                    "dispatch",
                    "setfloating",
                    &format!("class:^{VST3_SURFACE_CLASS}$"),
                ]) {
                    return;
                }
                if hyprctl(&[
                    "dispatch",
                    "togglefloating",
                    &format!("class:^{VST3_SURFACE_CLASS}$"),
                ]) {
                    // togglefloating is a toggle — only OK if we detected tiled first; best-effort.
                    return;
                }
                // Title used by our overlay hole / vst3-host floaters.
                let _ = hyprctl(&[
                    "dispatch",
                    "setfloating",
                    "title:^(BusChain VST3|.* - VST3)$",
                ]);
            }
        })
        .ok();
}

/// Float clients owned by a specific PID (surface helper process).
pub fn request_float_pid(pid: u32) {
    if !is_hyprland() || pid == 0 {
        return;
    }
    thread::Builder::new()
        .name("buschain-hypr-float-pid".into())
        .spawn(move || {
            thread::sleep(Duration::from_millis(120));
            // Match via hyprctl clients JSON when available.
            let Ok(out) = Command::new("hyprctl")
                .args(["clients", "-j"])
                .output()
            else {
                request_float_vst3_surfaces();
                return;
            };
            let Ok(text) = String::from_utf8(out.stdout) else {
                return;
            };
            // Lightweight scan: find "pid": <pid> then nearby "address": "0x…"
            let pid_key = format!("\"pid\": {pid}");
            let pid_key_alt = format!("\"pid\":{pid}");
            for block in text.split('{') {
                if !(block.contains(&pid_key) || block.contains(&pid_key_alt)) {
                    continue;
                }
                if let Some(addr) = extract_address(block) {
                    if hyprctl(&["dispatch", "setfloating", &format!("address:{addr}")]) {
                        return;
                    }
                    let _ = hyprctl(&["dispatch", "togglefloating", &format!("address:{addr}")]);
                    return;
                }
            }
            request_float_vst3_surfaces();
        })
        .ok();
}

fn extract_address(block: &str) -> Option<String> {
    let key = "\"address\":";
    let i = block.find(key)?;
    let rest = &block[i + key.len()..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn hyprctl(args: &[&str]) -> bool {
    Command::new("hyprctl")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
