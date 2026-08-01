//! Isolated Pulse CLI for ops without a native path yet (one-release bridge).

use anyhow::{anyhow, Context, Result};

/// Move all sink-inputs currently on `from_sink` to `to_sink`.
/// Prefers native metadata retarget (works without Pulse sink visibility).
pub fn move_sink_inputs(from_sink: &str, to_sink: &str) -> Result<usize> {
    if super::native::native_ready() {
        match super::native::retarget_streams(from_sink, to_sink) {
            Ok(n) if n > 0 => return Ok(n),
            Ok(_) | Err(_) => {
                // Zero or failed — fall through to Pulse when both ends are visible.
            }
        }
    }
    // Pulse path only when both ends appear as Pulse sinks.
    let Some(from_idx) = sink_index(from_sink) else {
        return Ok(0);
    };
    if sink_index(to_sink).is_none() {
        return Ok(0);
    }
    let inputs = sink_input_indices_on(&from_idx);
    for idx in &inputs {
        run_ok("pactl", &["move-sink-input", idx, to_sink])?;
    }
    Ok(inputs.len())
}

/// Move one Pulse sink-input (object.serial) onto `to_sink`.
pub fn move_sink_input(index: u32, to_sink: &str) -> Result<bool> {
    if super::native::native_ready() {
        if let Ok(true) = super::native::retarget_stream_serial(index, to_sink) {
            return Ok(true);
        }
    }
    if sink_index(to_sink).is_none() {
        return Ok(false);
    }
    let out = std::process::Command::new("pactl")
        .args(["move-sink-input", &index.to_string(), to_sink])
        .output()
        .with_context(|| "spawn pactl move-sink-input")?;
    Ok(out.status.success())
}

fn sink_index(name: &str) -> Option<String> {
    let out = std::process::Command::new("pactl")
        .args(["list", "short", "sinks"])
        .output()
        .ok()?;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.split('\t');
        let idx = parts.next()?;
        let n = parts.next()?;
        if n == name {
            return Some(idx.to_string());
        }
    }
    None
}

fn sink_input_indices_on(sink_idx: &str) -> Vec<String> {
    let Ok(out) = std::process::Command::new("pactl")
        .args(["list", "short", "sink-inputs"])
        .output()
    else {
        return Vec::new();
    };
    let mut idxs = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.split('\t');
        let Some(idx) = parts.next() else { continue };
        let Some(si) = parts.next() else { continue };
        if si == sink_idx {
            idxs.push(idx.to_string());
        }
    }
    idxs
}

fn run_ok(bin: &str, args: &[&str]) -> Result<()> {
    let out = std::process::Command::new(bin)
        .args(args)
        .output()
        .with_context(|| format!("spawn {bin}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "{bin} {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}
