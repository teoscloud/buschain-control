//! Timed PipeWire/Pulse CLI helpers + short TTL caches for probe hot paths.
//!
//! Untimeout'd `pactl` / `pw-link` in tight loops is the main amplifier of
//! multi-minute worker stalls. Every interactive probe should go through here.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};

/// Default wall-clock budget for interactive probes.
pub const CLI_TIMEOUT: Duration = Duration::from_millis(400);
const SHORT_SINKS_TTL: Duration = Duration::from_millis(150);
const PW_LINK_L_TTL: Duration = Duration::from_millis(80);
const PW_LINK_PORTS_TTL: Duration = Duration::from_millis(100);

fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(anyhow!("process timed out after {timeout:?}"));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(anyhow!("wait: {e}"));
            }
        }
    }
}

/// Capture stdout with a hard timeout.
pub fn run_capture(bin: &str, args: &[&str], timeout: Duration) -> Result<String> {
    let mut child = Command::new(bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn {bin}"))?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let status = wait_with_timeout(&mut child, timeout)
        .with_context(|| format!("{bin} {:?}", args))?;
    let mut out = String::new();
    let mut err = String::new();
    if let Some(ref mut s) = stdout {
        let _ = s.read_to_string(&mut out);
    }
    if let Some(ref mut s) = stderr {
        let _ = s.read_to_string(&mut err);
    }
    if !status.success() {
        return Err(anyhow!("{bin} {:?} failed: {err}", args));
    }
    Ok(out)
}

pub fn run_status(bin: &str, args: &[&str], timeout: Duration) -> Result<()> {
    run_capture(bin, args, timeout).map(|_| ())
}

fn short_sinks_cache() -> &'static Mutex<Option<(Instant, String)>> {
    static C: OnceLock<Mutex<Option<(Instant, String)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

fn pw_link_l_cache() -> &'static Mutex<Option<(Instant, String)>> {
    static C: OnceLock<Mutex<Option<(Instant, String)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

fn pw_link_o_cache() -> &'static Mutex<Option<(Instant, String)>> {
    static C: OnceLock<Mutex<Option<(Instant, String)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

fn pw_link_i_cache() -> &'static Mutex<Option<(Instant, String)>> {
    static C: OnceLock<Mutex<Option<(Instant, String)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

pub fn invalidate_probe_caches() {
    if let Ok(mut g) = short_sinks_cache().lock() {
        *g = None;
    }
    if let Ok(mut g) = pw_link_l_cache().lock() {
        *g = None;
    }
    if let Ok(mut g) = pw_link_o_cache().lock() {
        *g = None;
    }
    if let Ok(mut g) = pw_link_i_cache().lock() {
        *g = None;
    }
}

/// Cached `pactl list short sinks` (TTL ~150ms).
///
/// On CLI timeout, return the last good listing (fail-open) so Props do not
/// treat a wedged `pactl` as “FX node missing” and storm-retries for minutes.
pub fn pactl_short_sinks() -> Option<String> {
    if let Ok(g) = short_sinks_cache().lock() {
        if let Some((at, text)) = g.as_ref() {
            if at.elapsed() < SHORT_SINKS_TTL {
                return Some(text.clone());
            }
        }
    }
    match run_capture("pactl", &["list", "short", "sinks"], CLI_TIMEOUT) {
        Ok(text) => {
            if let Ok(mut g) = short_sinks_cache().lock() {
                *g = Some((Instant::now(), text.clone()));
            }
            Some(text)
        }
        Err(_) => {
            // Stale-but-known beats false “missing FX” under probe storms.
            if let Ok(g) = short_sinks_cache().lock() {
                if let Some((_, text)) = g.as_ref() {
                    return Some(text.clone());
                }
            }
            None
        }
    }
}

/// Cached `pw-link -l` (TTL ~80ms) — dominant cost inside spine polls.
pub fn pw_link_listing() -> Option<String> {
    if let Ok(g) = pw_link_l_cache().lock() {
        if let Some((at, text)) = g.as_ref() {
            if at.elapsed() < PW_LINK_L_TTL {
                return Some(text.clone());
            }
        }
    }
    let text = run_capture("pw-link", &["-l"], CLI_TIMEOUT).ok()?;
    if let Ok(mut g) = pw_link_l_cache().lock() {
        *g = Some((Instant::now(), text.clone()));
    }
    Some(text)
}

/// Cached `pw-link -o` (output ports) — used by every `link_is_live` probe.
pub fn pw_link_outputs() -> Option<String> {
    if let Ok(g) = pw_link_o_cache().lock() {
        if let Some((at, text)) = g.as_ref() {
            if at.elapsed() < PW_LINK_PORTS_TTL {
                return Some(text.clone());
            }
        }
    }
    let text = run_capture("pw-link", &["-o"], CLI_TIMEOUT).ok()?;
    if let Ok(mut g) = pw_link_o_cache().lock() {
        *g = Some((Instant::now(), text.clone()));
    }
    Some(text)
}

/// Cached `pw-link -i` (input ports).
pub fn pw_link_inputs() -> Option<String> {
    if let Ok(g) = pw_link_i_cache().lock() {
        if let Some((at, text)) = g.as_ref() {
            if at.elapsed() < PW_LINK_PORTS_TTL {
                return Some(text.clone());
            }
        }
    }
    let text = run_capture("pw-link", &["-i"], CLI_TIMEOUT).ok()?;
    if let Ok(mut g) = pw_link_i_cache().lock() {
        *g = Some((Instant::now(), text.clone()));
    }
    Some(text)
}
