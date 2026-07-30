//! One-shot cleanup of leftover out-of-process helpers / A/B staging nodes.

use std::sync::atomic::{AtomicBool, Ordering};

static SWEPT: AtomicBool = AtomicBool::new(false);

/// Kill orphan filter-chain helper processes once per process lifetime.
pub fn sweep_orphan_helpers_once() {
    if SWEPT.swap(true, Ordering::AcqRel) {
        return;
    }
    sweep_orphan_helpers();
}

pub fn sweep_orphan_helpers() {
    let has = std::process::Command::new("pgrep")
        .args(["-f", "pipewire -c"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()
        .is_some_and(|s| s.success());
    if has {
        let _ = std::process::Command::new("pkill")
            .args(["-f", "pipewire -c .*/buschain_fx_"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    destroy_staging_nodes();
}

/// Retired A/B `__stg` posts/fx steal Master→HW if left behind.
fn destroy_staging_nodes() {
    let Ok(out) = std::process::Command::new("pw-cli")
        .args(["ls", "Node"])
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut names: Vec<String> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if !t.contains("node.name") {
            continue;
        }
        for part in t.split('"').skip(1).step_by(2) {
            if part.starts_with("buschain_") && part.ends_with("__stg") {
                names.push(part.to_string());
            }
        }
    }
    names.sort();
    names.dedup();
    for name in names {
        let _ = std::process::Command::new("pw-cli")
            .args(["destroy", &name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}
