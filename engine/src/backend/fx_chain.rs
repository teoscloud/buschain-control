//! PipeWire graph probes + LADSPA search path (FX DSP lives in `host/`).

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Cache `pw-cli ls Node` lookups — that command is the dominant cost of live knob pushes.
fn node_id_cache() -> &'static Mutex<HashMap<String, (u32, Instant)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (u32, Instant)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Legacy stub — in-process hosts do not spawn `pipewire -c` children.
#[derive(Default)]
pub struct FilterChainRuntime;

impl FilterChainRuntime {
    pub fn new() -> Self {
        Self
    }

    pub fn stop_all(&mut self) {}

    pub fn stop_one(&mut self, _fx_name: &str) {}

    pub fn owns_live(&mut self, _fx_name: &str) -> bool {
        false
    }

    pub fn spawn_in_backoff(&mut self, _fx_name: &str) -> bool {
        false
    }
}

/// Collect LADSPA plugin directories (engine-relative `../plugins/*/build` first).
/// Cached process-wide — rebuilt only when `LADSPA_PATH` changes are not expected at runtime.
pub fn ladspa_search_path() -> String {
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            let mut dirs: Vec<PathBuf> = Vec::new();
            let manifest_plugins = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../plugins");
            if let Ok(rd) = fs::read_dir(&manifest_plugins) {
                for e in rd.flatten() {
                    let build = e.path().join("build");
                    if build.is_dir() {
                        dirs.push(build);
                    }
                }
            }
            if let Ok(rd) = fs::read_dir("plugins") {
                for e in rd.flatten() {
                    let build = e.path().join("build");
                    if build.is_dir() {
                        dirs.push(build);
                    }
                }
            }
            if let Some(home) = std::env::var_os("HOME") {
                dirs.push(PathBuf::from(home).join(".local/lib/ladspa"));
            }
            if let Ok(env) = std::env::var("LADSPA_PATH") {
                for p in env.split(':').filter(|s| !s.is_empty()) {
                    dirs.push(PathBuf::from(p));
                }
            }
            dirs.retain(|d| d.exists());
            let mut out: Vec<String> = dirs
                .into_iter()
                .filter_map(|d| d.canonicalize().ok())
                .map(|d| d.display().to_string())
                .collect();
            out.sort();
            out.dedup();
            out.join(":")
        })
        .clone()
}

pub fn sink_exists(name: &str) -> bool {
    if super::native::native_ready() && super::native::native_sink_exists(name) {
        return true;
    }
    sink_index(name).is_some()
}

/// Pulse short-sinks index for `name` (first column) — cached via cli TTL.
fn sink_index(name: &str) -> Option<u32> {
    let text = super::cli::pactl_short_sinks()?;
    for line in text.lines() {
        let mut parts = line.split('\t');
        let idx = parts.next()?.parse::<u32>().ok()?;
        let n = parts.next()?;
        if n == name {
            return Some(idx);
        }
    }
    None
}

/// True when any sink-input is playing into this sink.
///
/// `pactl list short sink-inputs` columns are:
/// `input_index \t sink_index \t …` — **not** the sink name. Matching on name
/// always failed, so every FX spawn waited the full timeout and blocked the worker.
pub fn sink_has_input(name: &str) -> bool {
    let Some(want) = sink_index(name) else {
        return false;
    };
    let Ok(out) = super::cli::run_capture(
        "pactl",
        &["list", "short", "sink-inputs"],
        super::cli::CLI_TIMEOUT,
    ) else {
        return false;
    };
    out.lines().any(|l| {
        l.split('\t')
            .nth(1)
            .and_then(|s| s.parse::<u32>().ok())
            == Some(want)
    })
}

/// Seed cache when the host already knows the PipeWire node id (no `pw-cli`).
pub fn cache_node_id(node_name: &str, id: u32) {
    if id == 0 {
        return;
    }
    super::native::native_cache_node_id(node_name, id);
    if let Ok(mut g) = node_id_cache().lock() {
        g.insert(node_name.to_string(), (id, Instant::now()));
    }
}

pub fn find_node_id_by_name(node_name: &str) -> Option<u32> {
    if let Some(id) = super::native::native_find_node_id(node_name) {
        return Some(id);
    }
    const TTL: Duration = Duration::from_secs(45);
    let stale = node_id_cache()
        .lock()
        .ok()
        .and_then(|g| g.get(node_name).map(|(id, at)| (*id, *at)));
    if let Some((id, at)) = stale {
        if at.elapsed() < TTL {
            return Some(id);
        }
        if let Some(id) = find_node_id_by_name_uncached(node_name) {
            if let Ok(mut g) = node_id_cache().lock() {
                g.insert(node_name.to_string(), (id, Instant::now()));
            }
            return Some(id);
        }
        if sink_exists(node_name) {
            return Some(id);
        }
        return None;
    }
    let id = find_node_id_by_name_uncached(node_name)?;
    if let Ok(mut g) = node_id_cache().lock() {
        g.insert(node_name.to_string(), (id, Instant::now()));
    }
    Some(id)
}

fn find_node_id_by_name_uncached(node_name: &str) -> Option<u32> {
    let text = super::cli::run_capture("pw-cli", &["ls", "Node"], super::cli::CLI_TIMEOUT).ok()?;
    let mut current_id: Option<u32> = None;
    let needle = format!("\"{node_name}\"");
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("id ") {
            current_id = rest
                .split(',')
                .next()
                .and_then(|s| s.trim().parse::<u32>().ok());
        }
        if t.contains("node.name") && t.contains(&needle) {
            return current_id;
        }
    }
    None
}
