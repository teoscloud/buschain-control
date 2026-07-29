//! PipeWire filter-chain insert hosting — monolithic multi-plugin FX per bus.
//!
//! Apps stay on null-sink buses; FX is a separate `buschain_fx_*` helper → `buschain_post_*`.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};

use crate::domain::{normalize_ladspa_label, ChainSpec, InsertSlot};

/// Cache `pw-cli ls Node` lookups — that command is the dominant cost of live knob pushes.
fn node_id_cache() -> &'static Mutex<HashMap<String, (u32, Instant)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (u32, Instant)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn invalidate_node_id_cache(fx_name: Option<&str>) {
    let Ok(mut g) = node_id_cache().lock() else {
        return;
    };
    match fx_name {
        Some(name) => {
            g.remove(name);
        }
        None => g.clear(),
    }
}

/// Seed cache after spawn so the first knob skips a full `pw-cli ls Node`.
pub fn warm_node_id_cache(fx_name: &str) {
    let _ = find_node_id_by_name(fx_name);
}

/// Stable FX conf dir — never use nix-shell `$TMPDIR` (it vanishes / changes).
pub fn fx_conf_dir() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(runtime).join("buschain-control-fx");
        if fs::create_dir_all(&p).is_ok() {
            return p;
        }
    }
    let p = PathBuf::from("/tmp/buschain-control-fx");
    let _ = fs::create_dir_all(&p);
    p
}

pub struct FilterChainRuntime {
    /// Keyed by FX sink name (`buschain_fx_…`).
    children: Vec<(String, Child)>,
    conf_dir: PathBuf,
    /// After a hard spawn failure, skip respawn until this instant (per FX name).
    fail_backoff: Vec<(String, std::time::Instant)>,
}

impl Default for FilterChainRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl FilterChainRuntime {
    pub fn new() -> Self {
        let conf_dir = fx_conf_dir();
        let _ = fs::create_dir_all(&conf_dir);
        Self {
            children: Vec::new(),
            conf_dir,
            fail_backoff: Vec::new(),
        }
    }

    pub fn mark_spawn_failed(&mut self, fx_name: &str) {
        let until = std::time::Instant::now() + Duration::from_secs(20);
        if let Some(e) = self.fail_backoff.iter_mut().find(|(n, _)| n == fx_name) {
            e.1 = until;
        } else {
            self.fail_backoff.push((fx_name.to_string(), until));
        }
    }

    pub fn clear_spawn_failed(&mut self, fx_name: &str) {
        self.fail_backoff.retain(|(n, _)| n != fx_name);
    }

    /// True when a recent spawn failure says "don't block the worker again yet".
    pub fn spawn_in_backoff(&mut self, fx_name: &str) -> bool {
        let now = std::time::Instant::now();
        self.fail_backoff.retain(|(_, until)| *until > now);
        self.fail_backoff.iter().any(|(n, _)| n == fx_name)
    }

    pub fn stop_all(&mut self) {
        for (_, mut child) in self.children.drain(..) {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.fail_backoff.clear();
        let _ = Command::new("pkill")
            .args(["-f", "pipewire -c .*/buschain-control-fx/"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if let Ok(rd) = fs::read_dir(&self.conf_dir) {
            for e in rd.flatten() {
                let _ = fs::remove_file(e.path());
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    /// Kill the helper and wipe conf/sig (prepare for a clean respawn).
    pub fn stop_one(&mut self, fx_name: &str) {
        self.stop_one_inner(fx_name, true);
    }

    /// Kill the helper but keep conf/log for diagnosis after a failed ensure.
    pub fn stop_one_keep_artifacts(&mut self, fx_name: &str) {
        self.stop_one_inner(fx_name, false);
    }

    fn stop_one_inner(&mut self, fx_name: &str, wipe_conf: bool) {
        invalidate_node_id_cache(Some(fx_name));
        let mut left = Vec::new();
        for (name, mut child) in self.children.drain(..) {
            if name == fx_name {
                let _ = child.kill();
                let _ = child.wait();
            } else {
                left.push((name, child));
            }
        }
        self.children = left;
        let pattern = format!("pipewire -c .*/buschain-control-fx/{fx_name}\\.conf");
        let _ = Command::new("pkill")
            .args(["-f", &pattern])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if wipe_conf {
            let conf = self.conf_dir.join(format!("{fx_name}.conf"));
            let _ = fs::remove_file(&conf);
            let _ = fs::remove_file(signature_path(&self.conf_dir, fx_name));
        }
        let deadline = std::time::Instant::now() + Duration::from_millis(800);
        while std::time::Instant::now() < deadline {
            if !sink_exists(fx_name) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(30));
    }

    /// True if this runtime still owns a live `pipewire -c` child for `fx_name`.
    pub fn owns_live(&mut self, fx_name: &str) -> bool {
        self.children.retain_mut(|(n, child)| {
            if n != fx_name {
                return true;
            }
            match child.try_wait() {
                Ok(None) => true,
                _ => false,
            }
        });
        self.children.iter().any(|(n, _)| n == fx_name)
    }

    pub fn conf_dir(&self) -> &Path {
        &self.conf_dir
    }
}

/// Collect LADSPA plugin directories (engine-relative `../plugins/*/build` first).
pub fn ladspa_search_path() -> String {
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
}

pub fn sink_exists(name: &str) -> bool {
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

pub fn find_node_id_by_name(node_name: &str) -> Option<u32> {
    const TTL: Duration = Duration::from_secs(45);
    if let Ok(g) = node_id_cache().lock() {
        if let Some((id, at)) = g.get(node_name) {
            if at.elapsed() < TTL {
                return Some(*id);
            }
        }
    }
    let id = find_node_id_by_name_uncached(node_name)?;
    if let Ok(mut g) = node_id_cache().lock() {
        g.insert(node_name.to_string(), (id, Instant::now()));
    }
    Some(id)
}

fn find_node_id_by_name_uncached(node_name: &str) -> Option<u32> {
    let out = Command::new("pw-cli")
        .args(["ls", "Node"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
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

fn signature_path(conf_dir: &Path, fx_name: &str) -> PathBuf {
    conf_dir.join(format!("{fx_name}.sig"))
}

pub fn write_signature(conf_dir: &Path, fx_name: &str, signature: &str) -> Result<()> {
    if let Some(parent) = conf_dir.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir_all(conf_dir)?;
    fs::write(signature_path(conf_dir, fx_name), signature)
        .with_context(|| format!("write signature for {fx_name}"))?;
    Ok(())
}

pub fn read_signature(fx_name: &str) -> Option<String> {
    let text = fs::read_to_string(signature_path(&fx_conf_dir(), fx_name)).ok()?;
    Some(text.trim().to_string())
}

pub fn signature_matches(fx_name: &str, want: &str) -> bool {
    match read_signature(fx_name) {
        Some(have) => have == want,
        None => sink_exists(fx_name),
    }
}

fn escape_spa_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn format_spa_float(v: f32) -> String {
    if v.is_finite() && (v.fract()).abs() < 1e-6 {
        format!("{}", v as i32)
    } else {
        format!("{v:.4}")
    }
}

fn format_control_block(insert: &InsertSlot) -> String {
    let mut lines = Vec::new();
    for (key, val) in &insert.controls {
        lines.push(format!(
            "              \"{}\" = {}",
            escape_spa_string(key),
            format_spa_float(*val)
        ));
    }
    if lines.is_empty() {
        "{}".into()
    } else {
        format!("{{\n{}\n            }}", lines.join("\n"))
    }
}

fn resolve_plugin_so(insert: &InsertSlot) -> String {
    if !insert.plugin_so.is_empty() {
        let p = Path::new(&insert.plugin_so);
        if p.is_file() {
            return p
                .canonicalize()
                .unwrap_or_else(|_| p.to_path_buf())
                .display()
                .to_string();
        }
    }
    let label = normalize_ladspa_label(&insert.plugin_key);
    // Builtins share one .so; never look for buschain_pitch.so etc.
    let stem = if label.starts_with("buschain_") && label != "buschain_denoiser" && label != "buschain_gate"
    {
        "buschain_builtins".to_string()
    } else {
        label.replace(':', "_")
    };
    let file = format!("{stem}.so");
    for dir in ladspa_search_path().split(':').filter(|s| !s.is_empty()) {
        let p = Path::new(dir).join(&file);
        if p.is_file() {
            return p
                .canonicalize()
                .unwrap_or(p)
                .display()
                .to_string();
        }
    }
    // Last resort — LADSPA_PATH on the helper process may still resolve the stem.
    stem
}

pub fn write_fx_sink_conf(
    conf_path: &Path,
    fx_name: &str,
    description: &str,
    playback_target: &str,
    inserts: &[InsertSlot],
    clock_fragment: &str,
) -> Result<()> {
    if inserts.is_empty() {
        return Err(anyhow!("filter-chain requires at least one insert"));
    }

    let mut nodes = String::new();
    let mut links = String::new();
    for (i, plug) in inserts.iter().enumerate() {
        let label = normalize_ladspa_label(&plug.plugin_key);
        let plugin = escape_spa_string(&resolve_plugin_so(plug));
        let name = format!("n{i}");
        let control = format_control_block(plug);
        nodes.push_str(&format!(
            r#"
          {{
            type = ladspa
            name = {name}
            plugin = "{plugin}"
            label = {label}
            control = {control}
          }}"#
        ));
        if i + 1 < inserts.len() {
            let next = format!("n{}", i + 1);
            links.push_str(&format!(
                r#"
          {{ output = "{name}:Output L" input = "{next}:Input L" }}
          {{ output = "{name}:Output R" input = "{next}:Input R" }}"#
            ));
        }
    }

    let first = "n0";
    let last = format!("n{}", inserts.len() - 1);
    let desc = escape_spa_string(description);
    let fx = escape_spa_string(fx_name);
    let tgt = escape_spa_string(playback_target);
    // Unique per FX — WirePlumber stream-restore on shared "buschain-control"
    // previously retargeted FX-out to a dead `buschain_post_master` (Sink=invalid).
    let media_suffix = fx_name.strip_prefix("buschain_fx_").unwrap_or(fx_name);
    let media = escape_spa_string(&format!("buschain-fx-{media_suffix}"));

    let conf = format!(
        r#"# Generated by BusChain Control — FX sink (do not edit)
context.properties = {{ log.level = 2 }}
context.spa-libs = {{
    audio.convert.* = audioconvert/libspa-audioconvert
    support.*       = support/libspa-support
}}
context.modules = [
  {{ name = libpipewire-module-rt
    args = {{ nice.level = -11 rt.prio = 88 rt.time.soft = 200000 rt.time.hard = 200000 }}
    flags = [ ifexists nofail ]
  }}
  {{ name = libpipewire-module-protocol-native }}
  {{ name = libpipewire-module-client-node }}
  {{ name = libpipewire-module-adapter }}
  {{ name = libpipewire-module-filter-chain
    args = {{
      node.description = "{desc}"
      media.name       = "{media}"
      filter.graph = {{
        nodes = [{nodes}
        ]
        links = [{links}
        ]
        inputs  = [ "{first}:Input L" "{first}:Input R" ]
        outputs = [ "{last}:Output L" "{last}:Output R" ]
      }}
      capture.props = {{
        node.name             = "{fx}"
        node.description      = "{desc}"
        media.class           = "Audio/Sink"
        media.name            = "{media}"
        audio.channels        = 2
        audio.position        = [ FL FR ]
        node.passive          = false
        node.virtual          = true
{clock}
      }}
      playback.props = {{
        node.name             = "{fx}_out"
        node.description      = "{desc} Out"
        media.class           = "Stream/Output/Audio"
        media.name            = "{media}"
        target.object         = "{tgt}"
        node.target           = "{tgt}"
        audio.channels        = 2
        audio.position        = [ FL FR ]
        node.passive          = true
        node.virtual          = true
        stream.dont-remix     = true
        node.dont-fallback    = true
        session.suspend-timeout-seconds = 0
{clock}
      }}
    }}
  }}
]
"#,
        desc = desc,
        media = media,
        nodes = nodes,
        links = links,
        first = first,
        last = last,
        fx = fx,
        tgt = tgt,
        clock = clock_fragment,
    );

    if let Some(parent) = conf_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = fs::File::create(conf_path)
        .with_context(|| format!("write {}", conf_path.display()))?;
    f.write_all(conf.as_bytes())?;
    Ok(())
}

fn push_props_on_node(fx_name: &str, entries: &[String]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut tried_refresh = false;
    loop {
        let Some(id) = find_node_id_by_name(fx_name) else {
            return Err(anyhow!("FX node `{fx_name}` not found for live param push"));
        };
        let params = entries.join(" ");
        let expr = format!("{{ params = [ {params} ] }}");
        let status = Command::new("pw-cli")
            .args(["s", &id.to_string(), "Props", &expr])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .with_context(|| format!("pw-cli set-param on {fx_name}"))?;
        if status.success() {
            return Ok(());
        }
        // Stale cached node id after respawn — refresh once.
        if tried_refresh {
            return Err(anyhow!(
                "pw-cli Props update failed for `{fx_name}` (status {status})"
            ));
        }
        invalidate_node_id_cache(Some(fx_name));
        tried_refresh = true;
    }
}

/// Live Props for a monolithic multi-plugin filter-chain (`n0:…`, `n1:…`, …).
pub fn push_insert_controls(fx_name: &str, inserts: &[InsertSlot]) -> Result<()> {
    if inserts.is_empty() {
        return Ok(());
    }
    // Hot path: do not gate on .sig / pactl. Stale topology is an ensure/rewire
    // concern — rejecting knobs here makes the UI feel multi-second delayed.
    let mut entries = Vec::new();
    for (i, plug) in inserts.iter().enumerate() {
        let node = format!("n{i}");
        for (key, val) in &plug.controls {
            let ctrl = escape_spa_string(&format!("{node}:{key}"));
            entries.push(format!("\"{ctrl}\" {}", format_spa_float(*val)));
        }
    }
    push_props_on_node(fx_name, &entries)
}

fn spawn_pipewire_conf(
    runtime: &mut FilterChainRuntime,
    fx_name: &str,
    conf_path: &Path,
    post_name: &str,
) -> Result<()> {
    let ladspa = ladspa_search_path();
    if ladspa.is_empty() {
        return Err(anyhow!(
            "No LADSPA plugin dirs found — run `make plugins` in the BusChain Control project root"
        ));
    }

    let log_path = runtime.conf_dir.join(format!("{fx_name}.log"));
    let log_file = fs::File::create(&log_path)
        .with_context(|| format!("create {}", log_path.display()))?;
    let log_err = log_file
        .try_clone()
        .with_context(|| format!("clone {}", log_path.display()))?;

    let child = Command::new("pipewire")
        .arg("-c")
        .arg(conf_path)
        .env("LADSPA_PATH", &ladspa)
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err))
        .spawn()
        .with_context(|| format!("spawn pipewire -c {}", conf_path.display()))?;

    runtime.children.push((fx_name.to_string(), child));

    // Wake post — SUSPENDED posts reject the FX out stream.
    let _ = Command::new("pactl")
        .args(["suspend-sink", post_name, "0"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = set_sink_volume(post_name, 100);
    let _ = set_sink_mute(post_name, false);

    // Post can be SUSPENDED until the first out-stream lands — give it time.
    // Keep this short: worker HOL behind a 4s poll is what made knobs feel dead.
    let deadline = std::time::Instant::now() + Duration::from_millis(800);
    let mut next_nudge = std::time::Instant::now();
    while std::time::Instant::now() < deadline {
        if sink_exists(fx_name) {
            let _ = set_sink_volume(fx_name, 100);
            let _ = set_sink_mute(fx_name, false);
            if sink_has_input(post_name) {
                warm_node_id_cache(fx_name);
                super::cli::invalidate_probe_caches();
                return Ok(());
            }
            if std::time::Instant::now() >= next_nudge {
                let _ = nudge_fx_out_to_post(fx_name, post_name);
                next_nudge = std::time::Instant::now() + Duration::from_millis(150);
            }
        }
        if let Some((_, child)) = runtime.children.iter_mut().find(|(n, _)| n == fx_name) {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let log = fs::read_to_string(&log_path).unwrap_or_default();
                    return Err(anyhow!(
                        "filter-chain for {fx_name} exited early ({status}). \
                         LADSPA_PATH={ladspa}\nlog: {}\n{log}",
                        log_path.display()
                    ));
                }
                Ok(None) => {}
                Err(e) => return Err(anyhow!("filter-chain wait: {e}")),
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let log = fs::read_to_string(&log_path).unwrap_or_default();
    if !sink_exists(fx_name) {
        return Err(anyhow!(
            "FX sink `{fx_name}` did not appear in time (LADSPA_PATH={ladspa})\nlog: {}\n{log}",
            log_path.display()
        ));
    }
    Err(anyhow!(
        "FX out stream never landed on `{post_name}` within 800ms (LADSPA_PATH={ladspa})\nlog: {}\n{log}",
        log_path.display()
    ))
}

/// Spawn monolithic multi-plugin FX for `spec.bus`. Playback targets post null-sink.
pub fn spawn_sidechain(
    runtime: &mut FilterChainRuntime,
    spec: &ChainSpec,
    clock_fragment: &str,
) -> Result<()> {
    let plan = spec.wire_plan();
    let fx_name = plan.fx_sink.as_str();
    let post_name = plan.post_sink.as_str();
    let description = format!("BusChainControl_FX_{}", crate::domain::bus_suffix(spec.bus.as_str()));

    runtime.stop_one(fx_name);

    let conf_path = runtime.conf_dir.join(format!("{fx_name}.conf"));
    write_fx_sink_conf(
        &conf_path,
        fx_name,
        &description,
        post_name,
        &spec.inserts,
        clock_fragment,
    )?;
    spawn_pipewire_conf(runtime, fx_name, &conf_path, post_name)?;
    write_signature(&runtime.conf_dir, fx_name, &spec.signature())?;
    Ok(())
}

/// Find the Pulse sink-input for `{fx_name}_out` and move it onto `post_name`.
fn nudge_fx_out_to_post(fx_name: &str, post_name: &str) -> Result<()> {
    let out_name = format!("{fx_name}_out");
    let list = Command::new("pactl")
        .args(["list", "sink-inputs"])
        .output()
        .context("pactl list sink-inputs")?;
    let text = String::from_utf8_lossy(&list.stdout);
    let mut current: Option<u32> = None;
    let mut matched_idx: Option<u32> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Sink Input #") {
            current = rest.trim().parse().ok();
        }
        if line.contains("node.name") && line.contains(&out_name) {
            matched_idx = current;
            break;
        }
    }
    let Some(idx) = matched_idx else {
        return Ok(());
    };
    let status = Command::new("pactl")
        .args(["move-sink-input", &idx.to_string(), post_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("pactl move-sink-input")?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "move-sink-input {idx} → {post_name} failed ({status})"
        ))
    }
}

fn set_sink_volume(name: &str, pct: u32) -> Result<()> {
    let out = Command::new("pactl")
        .args(["set-sink-volume", name, &format!("{pct}%")])
        .output()
        .context("pactl set-sink-volume")?;
    if out.status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "set-sink-volume failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

fn set_sink_mute(name: &str, muted: bool) -> Result<()> {
    let out = Command::new("pactl")
        .args(["set-sink-mute", name, if muted { "1" } else { "0" }])
        .output()
        .context("pactl set-sink-mute")?;
    if out.status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "set-sink-mute failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}