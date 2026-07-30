//! Session graph orchestration (worker thread only).
//!
//! Node create / stereo routes / teardown links go through [`crate::audio::engine_handle`]
//! → `buschain-engine`. Do not add new `pactl`/`pw-link` capabilities here — extend the engine.

use std::process::Command;

use anyhow::{anyhow, Context, Result};

use crate::session::Session;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PwSnapshot {
    pub sinks: Vec<DeviceNode>,
    pub sources: Vec<DeviceNode>,
    pub sink_inputs: Vec<StreamNode>,
    pub source_outputs: Vec<StreamNode>,
    pub default_sink: Option<String>,
    pub default_source: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceNode {
    pub index: u32,
    pub name: String,
    pub description: String,
    pub volume_pct: u32,
    pub mute: bool,
    /// Live sample rate (Hz) when known.
    pub sample_rate: Option<u32>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StreamNode {
    pub index: u32,
    /// media.name (stream title) when available
    pub name: String,
    /// Best human label (never a bare "Stream 123" if anything better exists)
    pub application: String,
    /// application.process.binary — stable when not a generic runtime
    pub binary: Option<String>,
    /// application.id when present
    pub app_id: Option<String>,
    pub node_name: Option<String>,
    pub icon_name: Option<String>,
    /// PipeWire filter / virtual helper (hide from app-assign UI)
    pub internal: bool,
    pub volume_pct: u32,
    pub mute: bool,
    pub sink_or_source: String,
}

fn binary_is_generic(b: &str) -> bool {
    matches!(
        b.to_ascii_lowercase().as_str(),
        "electron"
            | "java"
            | "python"
            | "python3"
            | "python3.10"
            | "python3.11"
            | "python3.12"
            | "python3.13"
            | "node"
            | "nodejs"
            | "wine"
            | "wine64"
            | "mono"
            | "dotnet"
            | "perl"
            | "ruby"
    )
}

fn looks_like_stream_id_label(s: &str) -> bool {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("Stream ") {
        return rest.chars().all(|c| c.is_ascii_digit());
    }
    t.chars().all(|c| c.is_ascii_digit()) && !t.is_empty()
}

impl StreamNode {
    /// Stable assignment key — survives stream-index churn.
    /// Prefer real process binary; for Electron/etc. prefer application name.
    pub fn app_key(&self) -> String {
        if let Some(b) = self.binary.as_deref().filter(|s| !s.is_empty()) {
            if !binary_is_generic(b) {
                return format!("bin:{b}");
            }
        }
        if !self.application.is_empty() && !looks_like_stream_id_label(&self.application) {
            return format!("name:{}", self.application);
        }
        if let Some(id) = self.app_id.as_deref().filter(|s| !s.is_empty()) {
            return format!("id:{id}");
        }
        if let Some(b) = self.binary.as_deref().filter(|s| !s.is_empty()) {
            return format!("bin:{b}");
        }
        if let Some(n) = self.node_name.as_deref().filter(|s| !s.is_empty()) {
            return format!("node:{n}");
        }
        format!("stream:{}", self.index)
    }

    pub fn display_name(&self) -> String {
        if !self.application.is_empty() && !looks_like_stream_id_label(&self.application) {
            if let Some(b) = self.binary.as_deref().filter(|s| !s.is_empty()) {
                if binary_is_generic(b)
                    || b.to_lowercase() != self.application.to_lowercase()
                {
                    return format!("{} ({b})", self.application);
                }
            }
            return self.application.clone();
        }
        if let Some(b) = self.binary.as_deref().filter(|s| !s.is_empty()) {
            return b.to_string();
        }
        if !self.name.is_empty()
            && self.name != "Playback"
            && self.name != "buschain-control"
            && !looks_like_stream_id_label(&self.name)
        {
            return self.name.clone();
        }
        if let Some(n) = self.node_name.as_deref().filter(|s| {
            !s.is_empty() && *s != "buschain-control" && !s.starts_with("buschain_fx_")
        }) {
            return n.to_string();
        }
        format!("Stream {}", self.index)
    }

    /// Real user apps only — not BusChain / foreign filter-chain helpers.
    pub fn is_user_app(&self) -> bool {
        !self.internal
    }
}

fn run(cmd: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("spawn {cmd}"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "{cmd} {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn run_ok(cmd: &str, args: &[&str]) -> Result<()> {
    run(cmd, args).map(|_| ())
}

pub fn refresh_snapshot() -> PwSnapshot {
    let mut snap = PwSnapshot::default();
    match list_sinks() {
        Ok(v) => snap.sinks = v,
        Err(e) => snap.status = format!("sinks: {e}"),
    }
    match list_sources() {
        Ok(v) => snap.sources = v,
        Err(e) => {
            if !snap.status.is_empty() {
                snap.status.push_str(" · ");
            }
            snap.status.push_str(&format!("sources: {e}"));
        }
    }
    match list_sink_inputs() {
        Ok(v) => snap.sink_inputs = v,
        Err(e) => {
            if !snap.status.is_empty() {
                snap.status.push_str(" · ");
            }
            snap.status.push_str(&format!("playback: {e}"));
        }
    }
    match list_source_outputs() {
        Ok(v) => snap.source_outputs = v,
        Err(e) => {
            if !snap.status.is_empty() {
                snap.status.push_str(" · ");
            }
            snap.status.push_str(&format!("recording: {e}"));
        }
    }
    snap.default_sink = pactl_info_default("Default Sink:");
    snap.default_source = pactl_info_default("Default Source:");
    if snap.status.is_empty() {
        snap.status = format!(
            "{} sinks · {} sources · {} apps",
            snap.sinks.len(),
            snap.sources.len(),
            snap.sink_inputs.len()
        );
    }
    snap
}

fn pactl_info_default(key: &str) -> Option<String> {
    let ok = run("pactl", &["info"]).ok()?;
    for line in ok.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn parse_pactl_list(kind: &str) -> Result<Vec<(u32, Vec<(String, String)>)>> {
    let text = run("pactl", &["list", kind])?;
    let mut blocks = Vec::new();
    let mut cur_idx: Option<u32> = None;
    let mut props: Vec<(String, String)> = Vec::new();

    let header = match kind {
        "sinks" => "Sink #",
        "sources" => "Source #",
        "sink-inputs" => "Sink Input #",
        "source-outputs" => "Source Output #",
        _ => return Err(anyhow!("unknown list kind")),
    };

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(header) {
            if let Some(idx) = cur_idx.take() {
                blocks.push((idx, std::mem::take(&mut props)));
            }
            cur_idx = rest.trim().parse().ok();
            continue;
        }
        if cur_idx.is_none() {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "Properties:" {
            continue;
        }
        // Prefer `key = "value"` (PipeWire Properties) over `Key: value`
        // so values like `…application-name:Brave` don't confuse the colon split.
        if trimmed.contains(" = ") {
            if let Some((k, v)) = trimmed.split_once('=') {
                let k = k.trim();
                let v = v
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\\')
                    .trim_matches('"');
                if !k.is_empty() {
                    props.push((k.to_string(), v.to_string()));
                }
            }
            continue;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            let k = k.trim();
            // Allow spaces — "Owner Module", "Sample Specification", "Channel Map".
            // (Previously skipped, so loopback liveness checks never saw Owner Module.)
            if !k.is_empty() {
                props.push((k.to_string(), v.trim().to_string()));
            }
        }
    }
    if let Some(idx) = cur_idx {
        blocks.push((idx, props));
    }
    Ok(blocks)
}

fn prop_key<'a>(props: &'a [(String, String)], key: &str) -> Option<&'a str> {
    props
        .iter()
        .find(|(k, _)| k == key)
        .or_else(|| props.iter().find(|(k, _)| k.ends_with(key)))
        .map(|(_, v)| v.as_str())
}

fn volume_from_props(props: &[(String, String)]) -> (u32, bool) {
    let mut vol = 100u32;
    let mut mute = false;
    for (k, v) in props {
        if k == "Mute" {
            mute = v == "yes";
        }
        if k == "Volume" {
            // front-left: 65536 / 100%
            if let Some(pct) = v.split('%').next().and_then(|s| {
                s.split_whitespace()
                    .rev()
                    .find_map(|t| t.trim_end_matches('%').parse().ok())
            }) {
                vol = pct;
            } else if let Some(p) = v.find('/').and_then(|_| {
                v.split('/').nth(1)?.trim().trim_end_matches('%').parse().ok()
            }) {
                vol = p;
            }
        }
    }
    (vol, mute)
}

fn prop<'a>(props: &'a [(String, String)], key: &str) -> &'a str {
    props
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

pub fn list_sinks() -> Result<Vec<DeviceNode>> {
    let blocks = parse_pactl_list("sinks")?;
    // Prefer pactl Sample Specification (already in the list). Expensive ALSA/pw
    // probes only for non-shadow HW so snapshot refresh stays off the UI thread
    // and doesn't spawn N×pw-dump.
    Ok(blocks
        .into_iter()
        .map(|(index, props)| {
            let (volume_pct, mute) = volume_from_props(&props);
            let name = prop(&props, "Name").to_string();
            let from_props = sample_rate_from_props(&props);
            let sample_rate = if name.starts_with("buschain_") {
                from_props
            } else {
                buschain_engine::probe_sink_running_rate(&name).or(from_props)
            };
            DeviceNode {
                index,
                name,
                description: prop(&props, "Description").to_string(),
                volume_pct,
                mute,
                sample_rate,
            }
        })
        .collect())
}

pub fn list_sources() -> Result<Vec<DeviceNode>> {
    let blocks = parse_pactl_list("sources")?;
    Ok(blocks
        .into_iter()
        .filter(|(_, props)| !prop(props, "Name").ends_with(".monitor"))
        .map(|(index, props)| {
            let (volume_pct, mute) = volume_from_props(&props);
            let name = prop(&props, "Name").to_string();
            let from_props = sample_rate_from_props(&props);
            let sample_rate = if name.starts_with("buschain_") {
                from_props
            } else {
                buschain_engine::probe_source_running_rate(&name).or(from_props)
            };
            DeviceNode {
                index,
                name,
                description: prop(&props, "Description").to_string(),
                volume_pct,
                mute,
                sample_rate,
            }
        })
        .collect())
}

fn sample_rate_from_props(props: &[(String, String)]) -> Option<u32> {
    let spec = prop(props, "Sample Specification");
    for tok in spec.split_whitespace() {
        if let Some(hz) = tok.strip_suffix("Hz") {
            if let Ok(r) = hz.parse::<u32>() {
                if r >= 8_000 {
                    return Some(r);
                }
            }
        }
    }
    None
}

fn clean_pw_str(s: &str) -> String {
    s.trim()
        .trim_matches('"')
        .replace("\\\"", "")
        .trim_matches('"')
        .to_string()
}

fn stream_from_props(index: u32, props: &[(String, String)], sink_key: &str) -> StreamNode {
    let (volume_pct, mute) = volume_from_props(props);
    let app_name = prop_key(props, "application.name").map(clean_pw_str).filter(|s| !s.is_empty());
    let binary = prop_key(props, "application.process.binary")
        .map(clean_pw_str)
        .filter(|s| !s.is_empty());
    let app_id = prop_key(props, "application.id")
        .map(clean_pw_str)
        .filter(|s| !s.is_empty());
    let media = prop_key(props, "media.name")
        .map(clean_pw_str)
        .filter(|s| !s.is_empty());
    let device_desc = prop_key(props, "device.description")
        .map(clean_pw_str)
        .filter(|s| !s.is_empty());
    let node_name = prop_key(props, "node.name")
        .map(clean_pw_str)
        .filter(|s| !s.is_empty());
    let icon_name = prop_key(props, "application.icon_name")
        .map(clean_pw_str)
        .filter(|s| !s.is_empty());
    let virtual_node = prop_key(props, "node.virtual").is_some_and(|v| v == "true");
    let media_class = prop_key(props, "media.class").unwrap_or("");
    let node_group = prop_key(props, "node.group").unwrap_or("");
    let restore = prop_key(props, "module-stream-restore.id").unwrap_or("");

    // Label cascade — never invent "Stream N" if PW gave us anything useful
    let application = app_name
        .clone()
        .or_else(|| {
            device_desc
                .clone()
                .filter(|d| !d.eq_ignore_ascii_case("buschain-control"))
        })
        .or_else(|| {
            media
                .clone()
                .filter(|m| {
                    let ml = m.to_lowercase();
                    ml != "playback"
                        && ml != "buschain-control"
                        && !ml.starts_with("audiostream")
                })
        })
        .or_else(|| {
            node_name.clone().filter(|n| {
                let nl = n.to_lowercase();
                !nl.starts_with("buschain_fx_")
                    && nl != "buschain-control"
                    && !nl.contains("filter-chain")
            })
        })
        .or_else(|| {
            // sink-input-by-application-name:Brave
            restore
                .strip_prefix("sink-input-by-application-name:")
                .or_else(|| restore.strip_prefix("sink-input-by-media-name:"))
                .map(|s| s.to_string())
                .filter(|s| {
                    let sl = s.to_lowercase();
                    sl != "buschain-control" && sl != "playback"
                })
        })
        .or_else(|| icon_name.clone().map(|i| i.replace("-browser", "").replace('-', " ")))
        .unwrap_or_else(|| format!("Stream {index}"));

    let name = media.clone().unwrap_or_else(|| application.clone());

    let internal = virtual_node
        || node_name.as_deref().is_some_and(|n| {
            let nl = n.to_lowercase();
            n.starts_with("buschain_")
                || n.starts_with("buschain_fx_")
                || n.starts_with("buschain_rs_")
                || n.starts_with("eedn_")
                || nl.starts_with("loopback")
                || n.contains("easyeffects")
                || n.contains("filter-chain")
        })
        || media.as_deref().is_some_and(|m| {
            let ml = m.to_lowercase();
            ml == "buschain-control"
                || ml == "buschain-keepalive"
                || ml.starts_with("eedn")
                || ml.contains("easyeffects")
        })
        || device_desc.as_deref().is_some_and(|d| {
            d.starts_with("BusChainControl") || d.contains("EEDN") || d.contains("Easy Effects")
        })
        || node_group.contains("filter-chain")
        || {
            let dest = prop(props, sink_key);
            dest.starts_with("buschain_fx_")
                || dest.starts_with("buschain_post_")
                || dest.starts_with("buschain_mid_")
                || dest.starts_with("buschain_rs_")
                || dest == "buschain_hold"
        }
        || (media_class.contains("Stream/") && app_name.is_none() && binary.is_none() && virtual_node);

    StreamNode {
        index,
        name,
        application,
        binary,
        app_id,
        node_name,
        icon_name,
        internal,
        volume_pct,
        mute,
        sink_or_source: prop(props, sink_key).to_string(),
    }
}

pub fn list_sink_inputs() -> Result<Vec<StreamNode>> {
    let sinks = list_sinks().unwrap_or_default();
    let blocks = parse_pactl_list("sink-inputs")?;
    Ok(blocks
        .into_iter()
        .map(|(index, props)| {
            let mut s = stream_from_props(index, &props, "Sink");
            // PipeWire often reports Sink as a numeric index — resolve to name
            if let Ok(idx) = s.sink_or_source.parse::<u32>() {
                if let Some(dev) = sinks.iter().find(|d| d.index == idx) {
                    s.sink_or_source = dev.name.clone();
                }
            }
            s
        })
        .collect())
}

pub fn list_source_outputs() -> Result<Vec<StreamNode>> {
    let blocks = parse_pactl_list("source-outputs")?;
    Ok(blocks
        .into_iter()
        .map(|(index, props)| stream_from_props(index, &props, "Source"))
        .collect())
}

pub fn set_sink_volume(name_or_index: &str, pct: u32) -> Result<()> {
    // Prefer engine native levels (linear from percent). Preserve unmute.
    let gain_db = if pct == 0 {
        -120.0
    } else {
        20.0 * (pct as f32 / 100.0).log10()
    };
    if crate::audio::engine_handle::set_levels(name_or_index, gain_db, false).is_ok() {
        return Ok(());
    }
    run_ok("pactl", &["set-sink-volume", name_or_index, &format!("{pct}%")])
}

pub fn set_sink_mute(name_or_index: &str, mute: bool) -> Result<()> {
    // Never force gain to 0 dB — that wiped every fader drag after set_sink_volume.
    if crate::audio::engine_handle::set_mute(name_or_index, mute).is_ok() {
        return Ok(());
    }
    run_ok(
        "pactl",
        &["set-sink-mute", name_or_index, if mute { "1" } else { "0" }],
    )
}

pub fn sink_exists(name: &str) -> bool {
    list_sinks()
        .map(|s| s.iter().any(|n| n.name == name))
        .unwrap_or(false)
}

/// True when module args name this sink exactly (not a `__stg` / prefix sibling).
fn module_args_sink_name(args: &str, name: &str) -> bool {
    for tok in args.split_whitespace() {
        if let Some(v) = tok.strip_prefix("sink_name=") {
            if v == name {
                return true;
            }
        }
    }
    false
}

/// Unload a BusChain `module-null-sink` by sink name so a filter-chain can reuse it.
pub fn unload_named_null_sink(name: &str) -> Result<()> {
    let text = run("pactl", &["list", "modules", "short"]).unwrap_or_default();
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let Some(idx) = parts.next() else { continue };
        let Some(mod_name) = parts.next() else { continue };
        let args = parts.next().unwrap_or("");
        // Exact token — substring match also hit `buschain_post_*__stg` A/B posts.
        if mod_name == "module-null-sink" && module_args_sink_name(args, name) {
            let _ = run_ok("pactl", &["unload-module", idx]);
            std::thread::sleep(std::time::Duration::from_millis(40));
            return Ok(());
        }
    }
    Ok(())
}

pub fn set_source_volume(name_or_index: &str, pct: u32) -> Result<()> {
    run_ok(
        "pactl",
        &["set-source-volume", name_or_index, &format!("{pct}%")],
    )
}

pub fn set_source_mute(name_or_index: &str, mute: bool) -> Result<()> {
    run_ok(
        "pactl",
        &[
            "set-source-mute",
            name_or_index,
            if mute { "1" } else { "0" },
        ],
    )
}

pub fn set_sink_input_volume(index: u32, pct: u32) -> Result<()> {
    run_ok(
        "pactl",
        &["set-sink-input-volume", &index.to_string(), &format!("{pct}%")],
    )
}

pub fn set_sink_input_mute(index: u32, mute: bool) -> Result<()> {
    run_ok(
        "pactl",
        &[
            "set-sink-input-mute",
            &index.to_string(),
            if mute { "1" } else { "0" },
        ],
    )
}

pub fn move_sink_input(index: u32, sink: &str) -> Result<()> {
    run_ok(
        "pactl",
        &["move-sink-input", &index.to_string(), sink],
    )
}

/// Move only when the stream is not already on `sink`.
/// Chromium/YouTube often pause on every `move-sink-input`, even no-ops.
pub fn move_sink_input_if_needed(index: u32, sink: &str, current: &str) -> Result<bool> {
    if current == sink {
        return Ok(false);
    }
    // Numeric Sink: index from pactl — resolve via list if needed
    if let Ok(idx) = current.parse::<u32>() {
        if let Ok(sinks) = list_sinks() {
            if sinks.iter().any(|s| s.index == idx && s.name == sink) {
                return Ok(false);
            }
        }
    }
    move_sink_input(index, sink)?;
    Ok(true)
}

pub fn set_default_sink(name: &str) -> Result<()> {
    if crate::audio::engine_handle::set_default_sink(name).unwrap_or(false) {
        return Ok(());
    }
    match run_ok("pactl", &["set-default-sink", name]) {
        Ok(()) => {
            std::thread::sleep(std::time::Duration::from_millis(40));
            if pactl_info_default("Default Sink:").as_deref() == Some(name) {
                return Ok(());
            }
            run_ok("pactl", &["set-default-sink", name])
        }
        Err(e) => {
            if let Some(id) = sink_index_by_name(name) {
                let _ = Command::new("wpctl")
                    .args(["set-default", &id.to_string()])
                    .status();
                if pactl_info_default("Default Sink:").as_deref() == Some(name) {
                    return Ok(());
                }
            }
            Err(e)
        }
    }
}

fn sink_index_by_name(name: &str) -> Option<u32> {
    let text = run("pactl", &["list", "short", "sinks"]).ok()?;
    for line in text.lines() {
        let mut parts = line.split('\t');
        let idx = parts.next()?.parse().ok()?;
        let n = parts.next()?;
        if n == name {
            return Some(idx);
        }
    }
    None
}

/// Set default sink only when it differs. Returns `true` when live default matches `name`.
/// Avoids reasserting the same default (can cork Chromium / move streams).
pub fn set_default_sink_if_needed(name: &str) -> Result<bool> {
    if pactl_info_default("Default Sink:").as_deref() == Some(name) {
        return Ok(true);
    }
    set_default_sink(name)?;
    Ok(pactl_info_default("Default Sink:").as_deref() == Some(name))
}

pub fn set_default_source(name: &str) -> Result<()> {
    run_ok("pactl", &["set-default-source", name])
}

fn db_to_pct(db: f32) -> u32 {
    // Map -48..+12 dB roughly onto 0..150%
    let lin = 10f32.powf(db / 20.0);
    ((lin * 100.0).clamp(0.0, 150.0)) as u32
}

/// Legacy Pulse loopback padding (unused for PW Links; kept for call-site compat).
const BUS_LATENCY_MS: u32 = 0;
const MONITOR_LATENCY_MS: u32 = 0;

fn is_buschain_sink_name(name: &str) -> bool {
    name.starts_with("buschain_master")
        || name.starts_with("buschain_track_")
        || name.starts_with("buschain_fx_")
        || name.starts_with("buschain_mid_")
        || name.starts_with("buschain_post_")
        || name.starts_with("buschain_rs_")
        || name == "buschain_hold"
}

/// Silence every BusChain sink + its `.monitor` source.
/// PipeWire null-sink mute alone often does NOT silence monitors that loopbacks read.
fn silence_buschain_nodes() {
    if let Ok(sinks) = list_sinks() {
        for s in sinks {
            if !is_buschain_sink_name(&s.name) {
                continue;
            }
            let _ = set_sink_mute(&s.name, true);
            let _ = set_sink_volume(&s.name, 0);
            let mon = format!("{}.monitor", s.name);
            let _ = set_source_mute(&mon, true);
            let _ = set_source_volume(&mon, 0);
        }
    }
}

/// Unmute (or silence) one sink + its `.monitor` by exact name.
fn set_named_sink_audible(name: &str, muted: bool) {
    if muted {
        let _ = set_sink_mute(name, true);
        let _ = set_sink_volume(name, 0);
        let mon = format!("{name}.monitor");
        let _ = set_source_mute(&mon, true);
        let _ = set_source_volume(&mon, 0);
    } else {
        let _ = set_sink_volume(name, 100);
        let _ = set_sink_mute(name, false);
        let mon = format!("{name}.monitor");
        let _ = set_source_mute(&mon, false);
        let _ = set_source_volume(&mon, 100);
    }
}

/// Open or silence FX helpers on a bus (monolithic FX + leftover slot/mid nodes).
/// Open path: live A/B generation only (never both gens — dual post→dest sums loud).
/// Mute path: silence every gen still present.
fn set_slot_chain_audible(bus: &str, muted: bool) {
    let live_fx = buschain_engine::live_fx_name(bus);
    let live_post = buschain_engine::live_post_name(bus);
    let can_fx = crate::audio::filter_chain::fx_name_for_bus(bus);
    let can_post = crate::audio::filter_chain::post_name_for_bus(bus);
    let stg_fx = format!("{can_fx}__stg");
    let stg_post = format!("{can_post}__stg");
    if muted {
        for name in [&live_fx, &live_post, &can_fx, &can_post, &stg_fx, &stg_post] {
            if sink_exists(name) {
                set_named_sink_audible(name, true);
            }
        }
    } else {
        if sink_exists(&live_fx) {
            set_named_sink_audible(&live_fx, false);
        }
        if sink_exists(&live_post) {
            set_named_sink_audible(&live_post, false);
        }
    }
    // Leftover per-slot nodes from older builds (never A/B gens — handled above).
    let fx_prefix = crate::audio::filter_chain::slot_fx_prefix(bus);
    let mid_prefix = crate::audio::filter_chain::slot_mid_prefix(bus);
    for sink in list_sinks().unwrap_or_default() {
        let n = &sink.name;
        if n.as_str() == live_fx
            || n.as_str() == can_fx
            || n.as_str() == stg_fx
            || n.as_str() == live_post
            || n.as_str() == can_post
            || n.as_str() == stg_post
        {
            continue;
        }
        if n.starts_with(&fx_prefix) || n.starts_with(&mid_prefix) {
            set_named_sink_audible(n, muted);
        }
    }
}

/// Retry-open live FX + post after spawn (pactl can race the new node briefly).
fn ensure_fx_path_open(bus: &str) {
    let fx = buschain_engine::live_fx_name(bus);
    let post = buschain_engine::live_post_name(bus);
    for _ in 0..8 {
        let mut ok = true;
        if sink_exists(&fx) {
            if set_sink_volume(&fx, 100).is_err() || set_sink_mute(&fx, false).is_err() {
                ok = false;
            }
        }
        if sink_exists(&post) {
            if set_sink_volume(&post, 100).is_err() || set_sink_mute(&post, false).is_err() {
                ok = false;
            }
            let mon = format!("{post}.monitor");
            let _ = set_source_mute(&mon, false);
            let _ = set_source_volume(&mon, 100);
        }
        if ok {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

/// Gate outbound audio (monitor + FX + post) without muting the app bus.
/// Muting the app-facing sink corks Chromium/YouTube — never do that for surgery.
pub fn gate_bus_output(bus: &str, gated: bool) {
    let mon = format!("{bus}.monitor");
    if gated {
        let _ = set_source_mute(&mon, true);
        let _ = set_source_volume(&mon, 0);
        set_slot_chain_audible(bus, true);
    } else {
        let _ = set_source_mute(&mon, false);
        let _ = set_source_volume(&mon, 100);
        set_slot_chain_audible(bus, false);
        ensure_fx_path_open(bus);
    }
}

/// Apply mute/volume to a track bus, FX chain, and post-FX meter sink.
fn set_track_audible(sink: &str, muted: bool, gain_db: f32) -> Result<()> {
    let mon = format!("{sink}.monitor");
    // Never mute/zero the *app-facing sink* for user mute — that corks Chromium.
    // Silence the outbound monitor / FX chain only; keep sink open for streams.
    // Always apply fader with muted=false on the app sink.
    let _ = crate::audio::engine_handle::set_levels(sink, gain_db, false);
    if muted {
        let _ = set_source_mute(&mon, true);
        let _ = set_source_volume(&mon, 0);
        set_slot_chain_audible(sink, true);
        let _ = set_sink_volume(sink, db_to_pct(gain_db));
        let _ = run_ok("pactl", &["set-sink-mute", sink, "0"]);
    } else {
        let _ = set_sink_volume(sink, db_to_pct(gain_db));
        let _ = run_ok("pactl", &["set-sink-mute", sink, "0"]);
        let _ = set_source_mute(&mon, false);
        let _ = set_source_volume(&mon, 100);
        set_slot_chain_audible(sink, false);
        ensure_fx_path_open(sink);
    }
    Ok(())
}

/// Continuous fader path — **bus volume only**.
///
/// Passes dB straight to the engine (no pct round-trip, no mute side-effect).
/// Never call `ensure_fx_path_open` / `list_sinks` here: that was blocking the
/// worker for hundreds of ms per tick and made faders jump while starving Props.
pub fn apply_one_track_volume(sink: &str, gain_db: f32) -> Result<()> {
    if crate::audio::engine_handle::set_levels(sink, gain_db, false).is_ok() {
        return Ok(());
    }
    set_sink_volume(sink, db_to_pct(gain_db))?;
    let _ = run_ok(
        "pactl",
        &["set-sink-mute", sink, "0"],
    );
    Ok(())
}

/// Mute / unmute transition — gates monitor + FX/post. Not for fader drags.
pub fn apply_one_track_mute_gate(sink: &str, muted: bool, gain_db: f32) -> Result<()> {
    set_track_audible(sink, muted, gain_db)
}

/// Fast fader path. `mute_changed` true → full gate; else volume-only.
pub fn apply_one_track_level(sink: &str, muted: bool, gain_db: f32) -> Result<()> {
    // Legacy: treat as mute-aware (safe but slower). Prefer the split APIs.
    set_track_audible(sink, muted, gain_db)
}

fn collect_buschain_modules(loopbacks_only: bool) -> Vec<String> {
    let text = run("pactl", &["list", "modules", "short"]).unwrap_or_default();
    let mut to_unload: Vec<String> = Vec::new();
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let Some(idx) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        let args = parts.next().unwrap_or("");
        // Keepalives must survive soft rebuilds — without a monitor reader PipeWire
        // suspends the null-sink and Chromium corks/pauses YouTube.
        if is_keepalive_loopback(args) {
            continue;
        }
        if name == "module-null-sink" && args.contains("sink_name=buschain_hold") {
            continue;
        }
        let shadowy = args.contains("buschain_")
            || args.contains("BusChainControl")
            || args.contains("shadow.audio")
            || args.contains("shadow-audio")
            || args.contains("shadow-fx")
            || args.contains("ShadowAudio")
            || args.contains("sink_name=shadow_")
            || args.contains("media.name=buschain-control")
            || args.contains("media.name=shadow-audio")
            || args.contains("media.name=shadow-fx");
        let legacy_loop = name == "module-loopback"
            && (args.contains("buschain_master")
                || args.contains("buschain_track_")
                || args.contains("buschain_fx_")
                || args.contains("shadow_master")
                || args.contains("shadow_track_")
                || args.contains("shadow_fx_"));
        let unload = if loopbacks_only {
            (name == "module-loopback" && (shadowy || legacy_loop))
                || (name == "module-ladspa-sink" && shadowy)
        } else {
            match name {
                "module-null-sink" | "module-ladspa-sink" | "module-loopback" => {
                    shadowy || legacy_loop
                }
                _ => false,
            }
        };
        if unload {
            to_unload.push(idx.to_string());
        }
    }
    to_unload
}

/// Unload loopbacks / PW links only — keeps null-sink track buses so apps stay attached.
pub fn teardown_buschain_loopbacks() -> Result<String> {
    crate::audio::engine_handle::teardown_links();
    let to_unload = collect_buschain_modules(true);
    let mut unloaded = 0u32;
    for idx in to_unload {
        if run_ok("pactl", &["unload-module", &idx]).is_ok() {
            unloaded += 1;
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(40));
    Ok(format!(
        "Refreshed PW links + {unloaded} legacy module(s); track sinks kept (media should not pause)."
    ))
}

/// Unload every Pulse/PipeWire module BusChain Control created (full reset).
pub fn teardown_buschain_graph() -> Result<String> {
    silence_buschain_nodes();
    crate::audio::engine_handle::teardown_links();
    std::thread::sleep(std::time::Duration::from_millis(40));
    let to_unload = collect_buschain_modules(false);
    let mut unloaded = 0u32;
    for idx in to_unload {
        if run_ok("pactl", &["unload-module", &idx]).is_ok() {
            unloaded += 1;
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(60));
    Ok(format!(
        "Tore down PW links + {unloaded} BusChain Control module(s). System audio clean — next live edit auto bring-up the mixer."
    ))
}

/// True for pre-rebrand Shadow Audio nodes (`shadow_*` / ShadowAudio_*).
pub fn is_legacy_shadow_sink(name: &str) -> bool {
    name.starts_with("shadow_")
}

fn buschain_counterpart(shadow_name: &str) -> Option<String> {
    if let Some(rest) = shadow_name.strip_prefix("shadow_") {
        Some(format!("buschain_{rest}"))
    } else {
        None
    }
}

/// Move apps off legacy `shadow_*` sinks onto matching `buschain_*` buses, then unload
/// every Shadow Audio Pulse module. Safe to call repeatedly (no-op when clean).
pub fn teardown_legacy_shadow_graph() -> Result<String> {
    let sinks = list_sinks().unwrap_or_default();
    let legacy: Vec<_> = sinks
        .iter()
        .filter(|s| is_legacy_shadow_sink(&s.name))
        .cloned()
        .collect();
    if legacy.is_empty() {
        return Ok(String::new());
    }

    let mut moved = 0u32;
    let inputs = list_sink_inputs().unwrap_or_default();
    for si in &inputs {
        let cur = if is_legacy_shadow_sink(&si.sink_or_source) {
            si.sink_or_source.clone()
        } else {
            si.sink_or_source
                .parse::<u32>()
                .ok()
                .and_then(|idx| sinks.iter().find(|s| s.index == idx).map(|s| s.name.clone()))
                .unwrap_or_default()
        };
        if !is_legacy_shadow_sink(&cur) {
            continue;
        }
        let dest = buschain_counterpart(&cur)
            .filter(|d| sink_exists(d))
            .unwrap_or_else(|| "buschain_master".into());
        if sink_exists(&dest)
            && move_sink_input_if_needed(si.index, &dest, &si.sink_or_source).unwrap_or(false)
        {
            moved += 1;
        }
    }

    // Drop PW links that still touch shadow_* endpoints.
    if let Ok(out) = std::process::Command::new("pw-link").arg("-l").output() {
        let text = String::from_utf8_lossy(&out.stdout);
        let mut pairs: Vec<(String, String)> = Vec::new();
        let mut cur_out: Option<String> = None;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("  |-> ") {
                if let Some(src) = cur_out.as_ref() {
                    if src.contains("shadow_") || rest.contains("shadow_") {
                        pairs.push((src.clone(), rest.trim().to_string()));
                    }
                }
            } else if let Some(rest) = line.strip_prefix("  |<- ") {
                if let Some(dst) = cur_out.as_ref() {
                    if dst.contains("shadow_") || rest.contains("shadow_") {
                        pairs.push((rest.trim().to_string(), dst.clone()));
                    }
                }
            } else if !line.starts_with(' ') && line.contains(':') {
                cur_out = Some(line.trim().to_string());
            }
        }
        for (s, d) in pairs {
            let _ = run_ok("pw-link", &["-d", &s, &d]);
        }
    }

    let mut unloaded = 0u32;
    let modules = run("pactl", &["list", "modules", "short"]).unwrap_or_default();
    for line in modules.lines() {
        let mut parts = line.splitn(3, '\t');
        let Some(idx) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        let args = parts.next().unwrap_or("");
        let hit = args.contains("shadow-audio")
            || args.contains("shadow-fx")
            || args.contains("ShadowAudio")
            || args.contains("sink_name=shadow_")
            || args.contains("shadow_master")
            || args.contains("shadow_track_")
            || args.contains("shadow_hold")
            || args.contains("shadow_post_")
            || args.contains("shadow_rs_")
            || args.contains("shadow_fx_")
            || (name == "module-loopback" && args.contains("shadow_"));
        if hit && run_ok("pactl", &["unload-module", idx]).is_ok() {
            unloaded += 1;
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(80));
    Ok(format!(
        "Legacy Shadow Audio removed ({unloaded} modules, moved {moved} stream(s))"
    ))
}

/// True when sink reports a stereo front-left/front-right (or FL/FR) map.
fn sink_is_stereo_fl_fr(name: &str) -> bool {
    let Ok(blocks) = parse_pactl_list("sinks") else {
        return false;
    };
    for (_, props) in blocks {
        if prop(&props, "Name") != name {
            continue;
        }
        let map = prop(&props, "Channel Map").to_ascii_lowercase();
        return (map.contains("front-left") && map.contains("front-right"))
            || (map.split(',').any(|c| c.trim() == "fl")
                && map.split(',').any(|c| c.trim() == "fr"));
    }
    false
}

fn ensure_null_sink(name: &str, description: &str) -> Result<()> {
    // Clocked null-sink via buschain-engine (rate/quantum from GraphClock).
    crate::audio::engine_handle::ensure_bus(name, description)
}

fn load_loopback(source: &str, sink: &str, _latency_msec: u32) -> Result<()> {
    // Engine route — inserts buschain_rs_* when endpoint rates differ from GraphClock.
    crate::audio::engine_handle::ensure_route(source, sink)
}

fn load_loopback_named(source: &str, sink: &str, _latency_msec: u32, _media_name: &str) -> Result<()> {
    crate::audio::engine_handle::ensure_route(source, sink)
}

fn modules_short() -> String {
    run("pactl", &["list", "modules", "short"]).unwrap_or_default()
}

fn is_keepalive_loopback(args: &str) -> bool {
    args.contains("sink=buschain_hold")
        || args.contains("media.name=buschain-keepalive")
}

fn loopback_line(source: &str, sink: &str) -> Option<(String, String)> {
    let src = format!("source={source}");
    let snk = format!("sink={sink}");
    for line in modules_short().lines() {
        let mut parts = line.splitn(3, '\t');
        let Some(idx) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        let args = parts.next().unwrap_or("");
        if name == "module-loopback" && args.contains(&src) && args.contains(&snk) {
            return Some((idx.to_string(), args.to_string()));
        }
    }
    None
}

fn loopback_exists(source: &str, sink: &str) -> bool {
    loopback_line(source, sink).is_some()
}

/// True when the live loopback was loaded with an explicit stereo map (post-fix).
fn loopback_is_stereo_ok(args: &str) -> bool {
    !args.contains("remix=false")
        && (args.contains("channels=2")
            || args.contains("front-left")
            || args.contains("channel_map"))
}

/// True when `sink` has at least one live sink-input (loopback / fx_out attached).
pub fn sink_has_input(sink: &str) -> bool {
    list_sink_inputs()
        .map(|inputs| inputs.iter().any(|i| i.sink_or_source == sink))
        .unwrap_or(false)
}

/// True when a native PipeWire link (or leftover legacy loopback) carries source→sink.
fn loopback_is_live(source: &str, sink: &str) -> bool {
    if crate::audio::engine_handle::link_is_live(source, sink) {
        return true;
    }
    let Some((mod_idx, args)) = loopback_line(source, sink) else {
        return false;
    };
    if !loopback_is_stereo_ok(&args) {
        return false;
    }
    let Ok(blocks) = parse_pactl_list("sink-inputs") else {
        return false;
    };
    let sinks = list_sinks().unwrap_or_default();
    let sink_index = sinks.iter().find(|s| s.name == sink).map(|s| s.index);
    for (_si_idx, props) in blocks {
        let owner = prop_key(&props, "Owner Module")
            .or_else(|| prop_key(&props, "owner.module"))
            .unwrap_or("");
        if owner != mod_idx {
            continue;
        }
        let sink_field = prop_key(&props, "Sink").unwrap_or("");
        if sink_field == "4294967295" || sink_field.is_empty() {
            return false;
        }
        if let Some(want) = sink_index {
            if sink_field.parse::<u32>().ok() == Some(want) {
                return true;
            }
        }
        if sink_field == sink {
            return true;
        }
        if let Ok(idx) = sink_field.parse::<u32>() {
            if sinks.iter().any(|s| s.index == idx && s.name == sink) {
                return true;
            }
        }
    }
    false
}

/// Ensure source→sink hop via native PipeWire Links (not module-loopback).
fn ensure_loopback(source: &str, sink: &str, _latency_msec: u32) -> Result<()> {
    for _ in 0..6 {
        if sink_exists(sink) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    crate::audio::engine_handle::ensure_route(source, sink)
        .with_context(|| format!("link {source}→{sink}"))
}

/// Dummy consumer so bus.monitor always has a reader → null-sink never suspends.
fn ensure_bus_keepalive(bus: &str) -> Result<()> {
    ensure_null_sink("buschain_hold", "BusChainControl_Hold")?;
    let _ = set_sink_mute("buschain_hold", true);
    let _ = set_sink_volume("buschain_hold", 0);
    let src = format!("{bus}.monitor");
    // Prefer native PW link; drop legacy keepalive loopback if present.
    if let Some((idx, _)) = loopback_line(&src, "buschain_hold") {
        let _ = run_ok("pactl", &["unload-module", &idx]);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    crate::audio::engine_handle::ensure_route(&src, "buschain_hold")?;
    let _ = run_ok("pactl", &["suspend-sink", bus, "0"]);
    Ok(())
}

/// Unload loopbacks owned by one track bus (outbound + mic + slot FX/mids + post).
/// Never removes keepalive readers.
fn unload_loopbacks_for_track_bus(bus: &str, _fx_name: &str) -> u32 {
    let post = crate::audio::filter_chain::post_name_for_bus(bus);
    let post_stg = format!("{post}__stg");
    let fx_prefix = crate::audio::filter_chain::slot_fx_prefix(bus);
    let mid_prefix = crate::audio::filter_chain::slot_mid_prefix(bus);
    let legacy_fx = crate::audio::filter_chain::fx_name_for_bus(bus);
    let live_fx = buschain_engine::live_fx_name(bus);
    let stg_fx = format!("{legacy_fx}__stg");
    let text = run("pactl", &["list", "modules", "short"]).unwrap_or_default();
    let mon = format!("source={bus}.monitor");
    let post_mon = format!("source={post}.monitor");
    let post_stg_mon = format!("source={post_stg}.monitor");
    let sink_bus = format!("sink={bus}");
    let sink_post = format!("sink={post}");
    let sink_post_stg = format!("sink={post_stg}");
    let mut unloaded = 0u32;
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let Some(idx) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        let args = parts.next().unwrap_or("");
        if name != "module-loopback" {
            continue;
        }
        if is_keepalive_loopback(args) {
            continue;
        }
        let outbound = args.contains(&mon);
        let fx_touch = args.contains(&fx_prefix)
            || args.contains(&legacy_fx)
            || args.contains(&live_fx)
            || args.contains(&stg_fx)
            || args.contains(&mid_prefix);
        let post_touch = args.contains(&post_mon)
            || args.contains(&post_stg_mon)
            || args.contains(&sink_post)
            || args.contains(&sink_post_stg)
            || args.contains(&post)
            || args.contains(&post_stg);
        let mic_in = args.contains(&sink_bus) && !args.contains("source=buschain_");
        if !(outbound || fx_touch || post_touch || mic_in) {
            continue;
        }
        if run_ok("pactl", &["unload-module", idx]).is_ok() {
            unloaded += 1;
        }
    }
    if unloaded > 0 {
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    unloaded
}

fn unload_mids_for_bus(bus: &str) {
    let prefix = crate::audio::filter_chain::slot_mid_prefix(bus);
    for sink in list_sinks().unwrap_or_default() {
        if sink.name.starts_with(&prefix) {
            let _ = unload_named_null_sink(&sink.name);
        }
    }
}

fn unload_post_sink_for_bus(bus: &str) {
    let post = crate::audio::filter_chain::post_name_for_bus(bus);
    let _ = unload_named_null_sink(&post);
    let _ = unload_named_null_sink(&format!("{post}__stg"));
}

/// Map `buschain_post_*` (+ optional `__stg`) → owning bus name.
fn bus_for_post_sink(name: &str) -> Option<String> {
    let base = name.strip_suffix("__stg").unwrap_or(name);
    if base == "buschain_post_master" {
        Some("buschain_master".into())
    } else {
        base.strip_prefix("buschain_post_")
            .map(|id| format!("buschain_track_{id}"))
    }
}

/// Remove track buses / FX helpers that no longer belong to any session track.
/// Rehomes any sink-inputs still on an orphan bus before unload (avoids browser pause).
pub fn prune_orphan_buschain_track_sinks(
    session: &Session,
    fx: &mut crate::audio::filter_chain::FilterChainRuntime,
) {
    // Gate orphans first so teardown never blasts open speakers.
    let mut keep_bus: std::collections::HashSet<String> = std::collections::HashSet::new();
    keep_bus.insert("buschain_master".into());
    for t in &session.tracks {
        keep_bus.insert(t.expected_sink_name());
    }

    let fallback = session
        .preferred_default_sink
        .as_ref()
        .filter(|s| keep_bus.contains(s.as_str()) && sink_exists(s))
        .cloned()
        .or_else(|| {
            if sink_exists("buschain_master") {
                Some("buschain_master".into())
            } else {
                None
            }
        });

    for sink in list_sinks().unwrap_or_default() {
        let name = sink.name;
        if name.starts_with("buschain_track_") && !keep_bus.contains(&name) {
            gate_bus_output(&name, true);
            std::thread::sleep(std::time::Duration::from_millis(20));
            if let (Some(dest), Ok(inputs)) = (fallback.as_ref(), list_sink_inputs()) {
                for si in inputs {
                    if si.sink_or_source == name {
                        let _ = move_sink_input_if_needed(si.index, dest, &si.sink_or_source);
                    }
                }
            }
            crate::audio::filter_chain::stop_slots_for_bus(fx, &name);
            let fx_name = crate::audio::filter_chain::fx_name_for_bus(&name);
            let _ = unload_loopbacks_for_track_bus(&name, &fx_name);
            unload_mids_for_bus(&name);
            unload_post_sink_for_bus(&name);
            // Legacy: FC used to *be* the track sink
            fx.stop_one(&name);
            let _ = unload_named_null_sink(&name);
        }
        // Orphan slot mids (not belonging to any live bus)
        if name.starts_with("buschain_mid_") {
            let owned = keep_bus.iter().any(|b| {
                name.starts_with(&crate::audio::filter_chain::slot_mid_prefix(b))
            });
            if !owned {
                let _ = unload_named_null_sink(&name);
            }
        }
        // Orphan post-FX meter sinks (canonical + `__stg` A/B generation).
        if name.starts_with("buschain_post_") {
            if let Some(bus) = bus_for_post_sink(&name) {
                if !keep_bus.contains(&bus) {
                    let _ = unload_named_null_sink(&name);
                }
            }
        }
    }
}

/// Live level update — never reloads modules (safe while dragging faders).
pub fn apply_track_levels(session: &Session) -> Result<()> {
    let any_solo = session
        .tracks
        .iter()
        .any(|t| t.solo && !t.kind.is_master());
    for track in &session.tracks {
        // Always use deterministic bus name — never skip when sink_name is None
        // (that left Master at create-mute@0 after Tear down / failed FX).
        let sink = track.expected_sink_name();
        let muted = track.mute || (any_solo && !track.solo && !track.kind.is_master());
        set_track_audible(&sink, muted, track.gain_db)?;
    }
    Ok(())
}

/// Fuzzy-match a device by saved name and/or description (PipeWire renames often).
pub fn resolve_device(
    preferred_name: Option<&str>,
    preferred_desc: Option<&str>,
    devices: &[DeviceNode],
    allow: impl Fn(&DeviceNode) -> bool,
) -> Option<String> {
    let devices: Vec<&DeviceNode> = devices.iter().filter(|d| allow(d)).collect();
    if devices.is_empty() {
        return None;
    }
    if let Some(name) = preferred_name {
        if let Some(d) = devices.iter().find(|d| d.name == name) {
            return Some(d.name.clone());
        }
    }
    if let Some(desc) = preferred_desc {
        let desc_l = desc.to_lowercase();
        if let Some(d) = devices.iter().find(|d| d.description == *desc) {
            return Some(d.name.clone());
        }
        if let Some(d) = devices
            .iter()
            .find(|d| d.description.to_lowercase() == desc_l)
        {
            return Some(d.name.clone());
        }
        if let Some(d) = devices.iter().find(|d| {
            let dd = d.description.to_lowercase();
            !desc_l.is_empty() && (dd.contains(&desc_l) || desc_l.contains(&dd))
        }) {
            return Some(d.name.clone());
        }
    }
    if let Some(name) = preferred_name {
        // alsa_output.pci-0000_0x.HiFi__hw_xxx__sink ↔ partial / reordered
        let name_l = name.to_lowercase();
        if let Some(d) = devices.iter().find(|d| {
            let n = d.name.to_lowercase();
            n.contains(&name_l) || name_l.contains(&n)
        }) {
            return Some(d.name.clone());
        }
        // Match stable USB id chunks
        for part in name.split(['.', '_', '-']).filter(|p| p.len() >= 6) {
            let p = part.to_lowercase();
            if let Some(d) = devices.iter().find(|d| d.name.to_lowercase().contains(&p)) {
                return Some(d.name.clone());
            }
        }
    }
    None
}

/// Real hardware (or non-BusChain) sink Master should play to.
/// Never returns a `buschain_*` sink — those are mixer buses, not devices.
pub fn resolve_hardware_output(session: &Session) -> Result<String> {
    let sinks = list_sinks().unwrap_or_default();
    let is_hw = |d: &DeviceNode| {
        !d.name.starts_with("buschain_") && !d.name.is_empty()
    };

    if let Some(name) = resolve_device(
        session.master_output.as_deref(),
        session.master_output_desc.as_deref(),
        &sinks,
        is_hw,
    ) {
        return Ok(name);
    }
    if let Some(def) = pactl_info_default("Default Sink:") {
        if !def.starts_with("buschain_") && sinks.iter().any(|s| s.name == def) {
            return Ok(def);
        }
    }
    sinks
        .into_iter()
        .find(|s| is_hw(s))
        .map(|s| s.name)
        .ok_or_else(|| {
            anyhow!(
                "No hardware output sink found. Pick one under Output devices → Master HW out \
                 (do not use a BusChain track as the system default)."
            )
        })
}

fn resolve_input_source(track: &crate::session::Track, sources: &[DeviceNode]) -> Option<String> {
    let name = track.input_source.as_deref()?;
    if name.is_empty() || name == "(none)" || name.contains("buschain_") {
        return None;
    }
    resolve_device(
        Some(name),
        track.input_source_desc.as_deref(),
        sources,
        |d| !d.name.ends_with(".monitor") && !d.name.starts_with("buschain_"),
    )
    .or_else(|| {
        // Last resort: exact name if still listed
        sources
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.name.clone())
    })
}

/// Whether a live sink-input belongs to an assigned app key (`bin:…` / `name:…` / legacy).
pub fn stream_matches_app_key(si: &StreamNode, token: &str) -> bool {
    sink_input_matches_assignment(si, token)
}

/// Move every live stream matching `app_key` onto `sink` (fresh `pactl` list — not UI snapshot).
pub fn place_app_on_sink(app_key: &str, sink: &str) -> Result<u32> {
    if !sink_exists(sink) {
        return Err(anyhow!("sink `{sink}` not found for app place"));
    }
    let inputs = list_sink_inputs()?;
    let mut moved = 0u32;
    for si in inputs {
        if !stream_matches_app_key(&si, app_key) {
            continue;
        }
        if si.sink_or_source == sink {
            continue;
        }
        if move_sink_input_if_needed(si.index, sink, &si.sink_or_source)? {
            moved += 1;
        }
    }
    Ok(moved)
}

/// Enforce assigned apps → track buses, then unassigned user apps → preferred default.
///
/// Without this, `module-stream-restore` leaves Chromium/Brave on Master while the
/// system-default bus sits idle (empty meter) even though Pulse default is correct.
pub fn enforce_playback_placements(session: &Session) -> Result<u32> {
    let mut moved = 0u32;
    let mut assigned_keys: Vec<String> = Vec::new();

    for track in &session.tracks {
        let sink = track.expected_sink_name();
        if !sink_exists(&sink) {
            continue;
        }
        for key in &track.assigned_playback {
            assigned_keys.push(key.clone());
            match place_app_on_sink(key, &sink) {
                Ok(n) => moved += n,
                Err(_) => {}
            }
        }
    }

    let Some(pref) = session.preferred_default_sink.as_deref() else {
        return Ok(moved);
    };
    if pref.is_empty() || !sink_exists(pref) {
        return Ok(moved);
    }

    let sinks = list_sinks().unwrap_or_default();
    let inputs = list_sink_inputs().unwrap_or_default();
    for si in inputs {
        if !si.is_user_app() {
            continue;
        }
        // Already claimed by a track assignment — leave it (handled above).
        if assigned_keys
            .iter()
            .any(|k| stream_matches_app_key(&si, k))
        {
            continue;
        }
        if si.sink_or_source == pref {
            continue;
        }
        // Resolve current sink name (Pulse may report index or name).
        let cur_name = if sinks.iter().any(|s| s.name == si.sink_or_source) {
            si.sink_or_source.clone()
        } else {
            si.sink_or_source
                .parse::<u32>()
                .ok()
                .and_then(|idx| sinks.iter().find(|s| s.index == idx).map(|s| s.name.clone()))
                .unwrap_or_else(|| si.sink_or_source.clone())
        };
        let on_buschain = cur_name.starts_with("buschain_") || is_legacy_shadow_sink(&cur_name);
        // When system default is a BusChain bus, reclaim unassigned user apps from
        // HW too — otherwise stream-restore leaves Chromium on Scarlett forever.
        let pref_is_buschain = pref.starts_with("buschain_");
        if !on_buschain && !(pref_is_buschain && si.is_user_app()) {
            continue;
        }
        if move_sink_input_if_needed(si.index, pref, &si.sink_or_source).unwrap_or(false) {
            moved += 1;
        }
    }
    Ok(moved)
}

fn sink_input_matches_assignment(si: &StreamNode, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    // Preferred stable keys
    if token == si.app_key() {
        return true;
    }
    if let Some(bin) = token.strip_prefix("bin:") {
        return si.binary.as_deref() == Some(bin);
    }
    if let Some(id) = token.strip_prefix("id:") {
        return si.app_id.as_deref() == Some(id);
    }
    if let Some(name) = token.strip_prefix("name:") {
        return si.application.eq_ignore_ascii_case(name);
    }
    if let Some(n) = token.strip_prefix("node:") {
        return si.node_name.as_deref().is_some_and(|x| x == n);
    }
    if let Some(id) = token.strip_prefix("stream:") {
        return si.index.to_string() == id;
    }
    // Legacy: bare stream index / application name / binary
    if si.index.to_string() == token {
        return true;
    }
    let tok = token.to_lowercase();
    if si.application.to_lowercase() == tok {
        return true;
    }
    if si.binary.as_deref().is_some_and(|b| b.to_lowercase() == tok) {
        return true;
    }
    if si.app_id.as_deref().is_some_and(|id| id.to_lowercase() == tok) {
        return true;
    }
    false
}

/// How aggressively Apply touches playback streams / system default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyKind {
    /// Explicit Apply graph: place assigned apps, reassert preferred default.
    Full,
    /// Plugin / routing hotplug: rebuild FX + loopbacks only.
    /// Never move sink-inputs or change the system default (Chromium pauses on those).
    Hotplug,
    /// After BindMasterClock: ForceRespawn all insert chains at the new GraphClock.
    ClockBind,
}

/// True when the track has a filter-chain (any LADSPA insert in the session rack).
/// Power off is graph-free (Bypass/Mix) — the FX node stays; meters still tap post.
pub fn track_has_live_fx(track: &crate::session::Track) -> bool {
    track
        .inserts
        .iter()
        .any(|p| p.id.format == crate::audio::plugin::PluginFormat::Ladspa)
}

/// LADSPA inserts in rack order (including powered-off). Each has a stable `slot_id`.
fn live_ladspa_inserts(track: &crate::session::Track) -> Vec<crate::audio::plugin::PluginRef> {
    track
        .inserts
        .iter()
        .filter(|p| p.id.format == crate::audio::plugin::PluginFormat::Ladspa)
        .cloned()
        .map(|mut p| {
            p.ensure_params();
            if p.params.iter().any(|(k, _)| k == "Mix") {
                p.set_param("Mix", p.mix.clamp(0.0, 1.0));
            }
            p
        })
        .collect()
}

/// Drop loopbacks from `source` that do not target one of `allow_sinks`.
/// Used to kill dry escape routes that would bypass FX (heard ≠ metered).
fn unload_loopbacks_from_source_except(source: &str, allow_sinks: &[&str]) -> u32 {
    crate::audio::engine_handle::unlink_from_source_except(source, allow_sinks)
}

/// Live knob/param/power push — Props on the track FX filter-chain (no unload).
pub fn push_track_fx_params(session: &Session, track_id: uuid::Uuid) -> Result<String> {
    crate::audio::insert_map::push_track_controls(session, track_id).map_err(|e| anyhow!("{e:#}"))
}

/// Pin every session bus awake *before* tearing FX/loopbacks.
pub fn pin_session_buses(session: &Session) -> Result<()> {
    let master_name = "buschain_master";
    ensure_null_sink(master_name, "BusChainControl_Master")?;
    let _ = ensure_bus_keepalive(master_name);
    let _ = run_ok("pactl", &["suspend-sink", master_name, "0"]);
    for track in &session.tracks {
        if track.kind.is_master() {
            continue;
        }
        let sink = track.expected_sink_name();
        let desc = format!("BusChainControl_{}", track.name.replace(' ', "_"));
        ensure_null_sink(&sink, &desc)?;
        let _ = ensure_bus_keepalive(&sink);
        let _ = run_ok("pactl", &["suspend-sink", &sink, "0"]);
    }
    Ok(())
}

/// True when the wet path is actually carrying audio (worker / routing).
/// Probes PipeWire and refreshes the UI wet cache. **Do not call from the UI
/// thread** — use `engine_handle::chain_is_wet_cached` for meters.
pub fn fx_path_audible(bus: &str) -> bool {
    crate::audio::engine_handle::chain_is_wet(bus)
}

/// True when inserts are declared and the sealed wet path is audible.
fn fx_topology_live(bus: &str, inserts: &[crate::audio::plugin::PluginRef]) -> bool {
    !inserts.is_empty() && fx_path_audible(bus)
}

/// Apply session routing — **idle reconcile**, never nuclear.
///
/// If buses/FX/loopbacks are already correct, Apply is a no-op for PipeWire topology
/// (keepalives + ensure missing links only). That is the only reliable way to stop
/// Chromium/Spotify pausing. Stream moves and default-sink changes are never done here.
pub fn apply_session(
    session: &mut Session,
    fx: &mut crate::audio::filter_chain::FilterChainRuntime,
    kind: ApplyKind,
) -> Result<String> {
    let _ = kind; // both kinds are stream-safe now
    session.normalize();
    for track in &mut session.tracks {
        for plug in &mut track.inserts {
            plug.ensure_params();
        }
    }

    pin_session_buses(session)?;

    let hw_sink = resolve_hardware_output(session)?;
    crate::audio::engine_handle::remember_master_hw(&hw_sink);
    if let Ok(all_sinks) = list_sinks() {
        if let Some(d) = all_sinks.iter().find(|s| s.name == hw_sink) {
            session.master_output = Some(hw_sink.clone());
            if !d.description.is_empty() {
                session.master_output_desc = Some(d.description.clone());
            }
        } else {
            session.master_output = Some(hw_sink.clone());
        }
    } else {
        session.master_output = Some(hw_sink.clone());
    }

    // Orphans only — never touches live session buses / streams on them.
    prune_orphan_buschain_track_sinks(session, fx);

    // Open faders *before* FX spawn — FX can take seconds; never leave mute@0
    // while waiting on filter-chain.
    for track in &mut session.tracks {
        track.sink_name = Some(track.expected_sink_name());
    }
    let _ = apply_track_levels(session);

    let ids: Vec<_> = session.tracks.iter().map(|t| t.id).collect();
    let mut kept = 0u32;
    let mut rebuilt = 0u32;
    let mut fx_count = 0u32;
    let mut warnings: Vec<String> = Vec::new();

    // Full: light prepare only — ArmSession owns FX + egress. The old path called
    // rewire_track_route → fx_path_audible (engine + pw-link storms) per track and
    // burned ~minute before the first ForceRespawn log line appeared.
    crate::audio::engine_handle::sync_desired_from_session(session, &hw_sink);

    // Hotplug/Route: links + Props only — NEVER ForceRespawn (that starved knobs
    // whenever wet probes flapped). Structural FX = RewireTrackFx / ClockBind.
    // ClockBind: ForceRespawn every insert rack so FX/post match the new GraphClock.
    for id in ids {
        let result = match kind {
            ApplyKind::Full => prepare_track_for_arm(session, id).map(|m| (false, m)),
            ApplyKind::Hotplug => {
                // Links + Props only — NEVER ForceRespawn (wet flaps stole minutes).
                let msg = rewire_track_route(session, id, &hw_sink, false)?;
                let want_fx = session
                    .tracks
                    .iter()
                    .find(|t| t.id == id)
                    .map(track_has_live_fx)
                    .unwrap_or(false);
                if want_fx {
                    let _ = crate::audio::insert_map::push_track_controls(session, id);
                }
                Ok((false, msg))
            }
            ApplyKind::ClockBind => {
                let has_fx = session
                    .tracks
                    .iter()
                    .find(|t| t.id == id)
                    .map(track_has_live_fx)
                    .unwrap_or(false);
                if has_fx {
                    rewire_track_fx(session, fx, id).map(|m| (true, m))
                } else {
                    rewire_track_route(session, id, &hw_sink, false).map(|m| (false, m))
                }
            }
        };
        match result {
            Ok((true, m)) => {
                rebuilt += 1;
                if m.contains("FX") {
                    fx_count += 1;
                }
            }
            Ok((false, m)) => {
                kept += 1;
                if m.contains("FX") {
                    fx_count += 1;
                }
            }
            Err(e) => warnings.push(format!("{e:#}")),
        }
    }

    // Full Apply: warm-adopt when PW graph already matches session; else sealed arm.
    // force_fx=false → Idempotent (only rebuild tracks that aren't already wet).
    // force_fx=true ForceRespawn'd every rack on UI restart (~10s×N under CLI load).
    // Hotplug / ClockBind: supervisor repair (ClockBind ForceRespawns FX in the loop above).
    let supervisor = match kind {
        ApplyKind::Full => {
            crate::audio::engine_handle::arm_session(session, &hw_sink, false)
        }
        ApplyKind::Hotplug | ApplyKind::ClockBind => {
            crate::audio::engine_handle::reconcile(session, &hw_sink)
        }
    };
    match supervisor {
        Ok(m) => {
            if m.contains("FX") || m.contains("wet") {
                fx_count = fx_count.max(1);
            }
            if m.contains("did not stick") || m.contains("WARN") {
                warnings.push(m);
            } else if m.contains("barrier") || m.contains("armed") {
                // surface bring-up summary lightly
            }
        }
        Err(e) => warnings.push(format!("supervisor: {e:#}")),
    }

    apply_track_levels(session)?;

    // Stamp sink_name so UI/meters attach even if a route step warned.
    for track in &mut session.tracks {
        track.sink_name = Some(track.expected_sink_name());
    }

    if let Some(pref) = session.preferred_default_sink.clone() {
        match set_default_sink_if_needed(&pref) {
            Ok(true) => {}
            Ok(false) => warnings.push(format!("preferred default did not stick: {pref}")),
            Err(e) => warnings.push(format!("preferred default: {e:#}")),
        }
    }

    let mut msg = format!(
        "Graph OK · buses→Master→{hw_sink} · kept {kept} · rebuilt {rebuilt} · {fx_count} FX · streams untouched"
    );
    if !warnings.is_empty() {
        msg.push_str(" · WARN: ");
        msg.push_str(&warnings.join(" | "));
    }
    Ok(msg)
}

/// Compatibility alias — slot rewire (add/remove/reorder).
pub fn hotplug_track_fx(
    session: &mut Session,
    fx: &mut crate::audio::filter_chain::FilterChainRuntime,
    track_id: uuid::Uuid,
) -> Result<String> {
    rewire_track_fx(session, fx, track_id)
}

/// Full-apply prepare: keepalive + input only — no wet probes / arm storms.
fn prepare_track_for_arm(session: &mut Session, track_id: uuid::Uuid) -> Result<String> {
    let (is_master, bus, input_source) = {
        let Some(track) = session.tracks.iter().find(|t| t.id == track_id) else {
            return Err(anyhow!("track not found"));
        };
        let sources = list_sources().unwrap_or_default();
        (
            track.kind.is_master(),
            track.expected_sink_name(),
            resolve_input_source(track, &sources),
        )
    };
    let _ = ensure_bus_keepalive(&bus);
    if let Some(src) = &input_source {
        if !is_master {
            let _ = ensure_loopback(src, &bus, MONITOR_LATENCY_MS);
        }
    }
    if let Some(t) = session.tracks.iter_mut().find(|t| t.id == track_id) {
        t.sink_name = Some(bus.clone());
        if let Some(src) = input_source {
            t.input_source = Some(src);
        }
    }
    Ok(format!("Prepare {bus}"))
}

/// Destination / input links.
///
/// When `allow_fx_ensure` is false (caller already ran `rewire_track_fx`), this
/// is link-only — never a second ForceRespawn.
pub fn rewire_track_route(
    session: &mut Session,
    track_id: uuid::Uuid,
    hw_sink: &str,
    allow_fx_ensure: bool,
) -> Result<String> {
    session.normalize();
    let _master_name = "buschain_master";
    let (is_master, bus, inserts, input_source) = {
        let Some(track) = session.tracks.iter().find(|t| t.id == track_id) else {
            return Err(anyhow!("track not found for route rewire"));
        };
        let sources = list_sources().unwrap_or_default();
        (
            track.kind.is_master(),
            track.expected_sink_name(),
            live_ladspa_inserts(track),
            resolve_input_source(track, &sources),
        )
    };

    // Sync DesiredState egress before any arm intent.
    crate::audio::engine_handle::sync_desired_from_session(session, hw_sink);

    // Helper alive (canonical or `__stg`) counts as FX present even if post→dest
    // briefly flaps — otherwise every track dry-bypassed plugins after A/B.
    // Prefer any_gen_live / sink presence — never fx_path_audible here (that
    // path holds the engine mutex + pw-link storms and made Hotplug feel dead).
    let helper_live = buschain_engine::any_gen_live(&bus);
    let mut has_fx = !inserts.is_empty() && helper_live;
    let mut warnings = Vec::new();
    // Route/Hotplug must never ForceRespawn — only RewireTrackFx / ClockBind.
    if allow_fx_ensure && !inserts.is_empty() && !has_fx {
        buschain_engine::fx_trace::log(
            "FORCE_RESPAWN_LEAK",
            &format!("rewire_track_route allow_fx_ensure bus={bus}"),
            0,
        );
        eprintln!(
            "[buschain] BUG: Route path requested ForceRespawn on {bus} — refused (hold-only)"
        );
        crate::audio::engine_handle::disarm_track_egress(&bus, true);
        warnings.push(format!(
            "FX not wet on {bus} — hold-only (Route must not ForceRespawn)"
        ));
        has_fx = false;
    } else if !inserts.is_empty() && !has_fx {
        // Link-only: keep hold-only — do not open dry bypass while Building.
        crate::audio::engine_handle::disarm_track_egress(&bus, true);
        warnings.push(format!(
            "FX not wet on {bus} with {} insert(s) — hold-only (awaiting wet)",
            inserts.len()
        ));
    }

    let _ = ensure_bus_keepalive(&bus);

    // Master→HW is owned by ArmSession / reconcile (session barrier). Never arm here.
    if is_master {
        crate::audio::engine_handle::remember_master_hw(hw_sink);
        // Still re-assert wet Master egress when inserts/helpers are live.
        if !inserts.is_empty() && has_fx {
            if let Err(e) = crate::audio::engine_handle::arm_track_egress(&bus, true) {
                warnings.push(format!("arm wet master: {e:#}"));
            }
        }
    } else if !inserts.is_empty() {
        if has_fx || helper_live {
            if let Err(e) = crate::audio::engine_handle::arm_track_egress(&bus, true) {
                warnings.push(format!("arm wet: {e:#}"));
            }
        } else if allow_fx_ensure {
            // Ensure already ran restore_dry — arm dry only when no helper exists.
            if let Err(e) = crate::audio::engine_handle::arm_track_egress(&bus, false) {
                warnings.push(format!("arm dry fallback: {e:#}"));
            }
        }
        // else: hold-only (Building) — no dry flash
    } else if let Err(e) = crate::audio::engine_handle::arm_track_egress(&bus, false) {
        warnings.push(format!("arm dry: {e:#}"));
    }

    if let Some(src) = &input_source {
        if !is_master {
            if let Err(e) = ensure_loopback(src, &bus, MONITOR_LATENCY_MS) {
                warnings.push(format!("input: {e:#}"));
            }
        }
    }

    if let Some(t) = session.tracks.iter_mut().find(|t| t.id == track_id) {
        t.sink_name = Some(bus.clone());
        if let Some(src) = input_source {
            t.input_source = Some(src);
        }
    }

    apply_track_levels(session)?;
    let mut msg = format!(
        "Route {} · {}",
        bus,
        if has_fx { "post→dest" } else { "dry" }
    );
    if !warnings.is_empty() {
        msg.push_str(" · WARN: ");
        msg.push_str(&warnings.join(" | "));
    }
    Ok(msg)
}

/// Open this track’s audible path only (faster than full-session levels).
fn open_track_audio(session: &Session, track_id: uuid::Uuid, bus: &str) -> Result<()> {
    gate_bus_output(bus, false);
    let muted = session
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .map(|t| {
            let any_solo = session.tracks.iter().any(|x| x.solo);
            t.mute || (any_solo && !t.solo && !t.kind.is_master())
        })
        .unwrap_or(false);
    let gain = session
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .map(|t| t.gain_db)
        .unwrap_or(0.0);
    set_track_audible(bus, muted, gain)
}

/// Sealed FX: mixer declares rack; engine owns spawn + exclusive wet wire.
pub fn rewire_track_fx(
    session: &mut Session,
    _fx: &mut crate::audio::filter_chain::FilterChainRuntime,
    track_id: uuid::Uuid,
) -> Result<String> {
    session.normalize();
    for track in &mut session.tracks {
        for plug in &mut track.inserts {
            plug.ensure_params();
        }
    }

    let master_name = "buschain_master";
    let hw_sink = resolve_hardware_output(session)?;
    crate::audio::engine_handle::remember_master_hw(&hw_sink);
    ensure_null_sink(master_name, "BusChainControl_Master")?;
    let _ = ensure_bus_keepalive(master_name);

    let Some(track) = session.tracks.iter().find(|t| t.id == track_id) else {
        return Err(anyhow!("track not found for FX rewire"));
    };
    let is_master = track.kind.is_master();
    let bus = track.expected_sink_name();
    let desc = if is_master {
        "BusChainControl_Master".to_string()
    } else {
        format!("BusChainControl_{}", track.name.replace(' ', "_"))
    };
    if !is_master {
        ensure_null_sink(&bus, &desc)?;
        let _ = ensure_bus_keepalive(&bus);
    }

    // Clean legacy per-slot helpers if any remain — never touch A/B live/staging.
    let mid_prefix = crate::audio::filter_chain::slot_mid_prefix(&bus);
    let fx_slot_prefix = crate::audio::filter_chain::slot_fx_prefix(&bus);
    let live_fx = buschain_engine::live_fx_name(&bus);
    let can_fx = crate::audio::filter_chain::fx_name_for_bus(&bus);
    let stg_fx = format!("{can_fx}__stg");
    for s in list_sinks().unwrap_or_default() {
        if s.name.ends_with("__stg") || s.name == live_fx || s.name == can_fx || s.name == stg_fx
        {
            continue;
        }
        if s.name.starts_with(&mid_prefix)
            || (s.name.starts_with(&fx_slot_prefix) && s.name != can_fx)
        {
            let _ = unload_named_null_sink(&s.name);
        }
    }

    // Keep Desired egress in sync before cutover so multi-target / track→track
    // racks arm the correct primary dest (not always Master).
    crate::audio::engine_handle::sync_desired_from_session(session, &hw_sink);
    let dest = crate::audio::insert_map::primary_fx_dest(session, track_id, &hw_sink);

    let mode = buschain_engine::ChainEnsureMode::ForceRespawn;
    let fx_result =
        crate::audio::insert_map::ensure_track_fx(session, track_id, &dest, mode);

    // Always re-assert wet post→dest after ensure. Skipping Master used to leave
    // dry bus→HW after idle reconcile chased the wrong A/B generation.
    // arm_track_egress honors speakers_armed for Master (session barrier).
    if is_master {
        crate::audio::engine_handle::remember_master_hw(&hw_sink);
    }
    let route_msg = match crate::audio::engine_handle::arm_track_egress(&bus, true) {
        Ok(()) => format!("Route {bus} · post→dest"),
        Err(e) => format!("Route {bus} · arm warn: {e:#}"),
    };
    open_track_audio(session, track_id, &bus)?;

    if let Some(t) = session.tracks.iter_mut().find(|t| t.id == track_id) {
        t.sink_name = Some(bus.clone());
    }

    match fx_result {
        Ok((_, fx_msg)) => Ok(format!("{fx_msg} · {route_msg}")),
        Err(e) => {
            crate::audio::engine_handle::set_chain_wet_cached(&bus, false);
            Err(anyhow!("FX failed (dry fallback): {e:#} · {route_msg}"))
        }
    }
}

/// Bring one new track bus online without touching other tracks' FX.
pub fn ensure_live_track(
    session: &mut Session,
    track_id: uuid::Uuid,
) -> Result<String> {
    session.normalize();
    let master_name = "buschain_master";
    ensure_null_sink(master_name, "BusChainControl_Master")?;
    let _ = ensure_bus_keepalive(master_name);

    let Some(track) = session.tracks.iter().find(|t| t.id == track_id) else {
        return Err(anyhow!("track not found"));
    };
    if track.kind.is_master() {
        return Ok("Master already live".into());
    }
    let bus = track.expected_sink_name();
    let desc = format!("BusChainControl_{}", track.name.replace(' ', "_"));
    let master_id = session.master_id();
    let mut targets = track.output_targets.clone();

    ensure_null_sink(&bus, &desc)?;
    let _ = ensure_bus_keepalive(&bus);
    gate_bus_output(&bus, true);

    // Drop any stale outbound links for this bus, then dry-route to Master.
    let fx_name = crate::audio::filter_chain::fx_name_for_bus(&bus);
    let _ = unload_loopbacks_for_track_bus(&bus, &fx_name);

    if targets.is_empty() {
        if let Some(mid) = master_id {
            targets.push(mid);
        }
    }
    let from = format!("{bus}.monitor");
    for tid in &targets {
        if master_id == Some(*tid) {
            let _ = load_loopback(&from, master_name, BUS_LATENCY_MS);
        } else if let Some(t) = session.tracks.iter().find(|t| t.id == *tid) {
            let dest = t.expected_sink_name();
            if dest != bus && sink_exists(&dest) {
                let _ = load_loopback(&from, &dest, BUS_LATENCY_MS);
            }
        }
    }

    if let Some(t) = session.tracks.iter_mut().find(|t| t.id == track_id) {
        t.sink_name = Some(bus.clone());
    }
    std::thread::sleep(std::time::Duration::from_millis(60));
    apply_track_levels(session)?;
    Ok(format!("Ensure track bus live · {bus}"))
}

/// After a track is removed from the session: gate + tear its leftover PW nodes only.
pub fn prune_removed_track(
    session: &Session,
    fx: &mut crate::audio::filter_chain::FilterChainRuntime,
    removed_bus: &str,
) -> Result<String> {
    gate_bus_output(removed_bus, true);
    std::thread::sleep(std::time::Duration::from_millis(30));
    let fx_name = crate::audio::filter_chain::fx_name_for_bus(removed_bus);
    crate::audio::filter_chain::stop_slots_for_bus(fx, removed_bus);
    let _ = unload_loopbacks_for_track_bus(removed_bus, &fx_name);
    unload_mids_for_bus(removed_bus);
    unload_post_sink_for_bus(removed_bus);
    // Move any stragglers off the doomed bus before unload.
    let fallback = session
        .preferred_default_sink
        .clone()
        .filter(|s| sink_exists(s))
        .or_else(|| {
            if sink_exists("buschain_master") {
                Some("buschain_master".into())
            } else {
                None
            }
        });
    if let (Some(dest), Ok(inputs)) = (fallback, list_sink_inputs()) {
        for si in inputs {
            if si.sink_or_source == removed_bus {
                let _ = move_sink_input_if_needed(si.index, &dest, &si.sink_or_source);
            }
        }
    }
    let _ = unload_named_null_sink(removed_bus);
    prune_orphan_buschain_track_sinks(session, fx);
    apply_track_levels(session)?;
    Ok(format!("Removed bus {removed_bus}"))
}
