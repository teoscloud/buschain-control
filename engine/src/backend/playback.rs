//! One-shot session playback placement (Desired `bus_playback` → sink-inputs).

use std::collections::HashMap;

use anyhow::{anyhow, Result};

use crate::plan::DesiredState;

/// Move pinned apps onto their buses; reclaim unassigned user apps to preferred default.
/// Native registry pass when up (sees Internal-hosted streams); single pactl pass otherwise.
///
/// Callers must ensure pin-target buses are Pulse-visible (`pulse_export`) before this
/// runs — see `Engine::promote_pin_buses_pulse_export` / session `virtual_output`.
pub fn enforce_desired_playback(desired: &DesiredState) -> Result<u32> {
    let sinks = list_short_sinks();
    let inputs = if super::native::native_ready() {
        let native = list_sink_inputs_native();
        if native.is_empty() {
            list_sink_inputs(&sinks)?
        } else {
            native
        }
    } else {
        list_sink_inputs(&sinks)?
    };
    let mut moved = 0u32;

    let mut key_to_bus: HashMap<String, String> = HashMap::new();
    for (bus, keys) in &desired.bus_playback {
        if !sinks.iter().any(|(_, n)| n == bus) {
            continue;
        }
        // Skip pin move until the target is Pulse-visible (or Master).
        if !pin_bus_is_ready(bus, &sinks) {
            continue;
        }
        for k in keys {
            key_to_bus.insert(k.clone(), bus.clone());
        }
    }

    for si in &inputs {
        if is_anonymous_system_sound(si) {
            continue;
        }
        for (key, bus) in &key_to_bus {
            if !matches_key(si, key) {
                continue;
            }
            if si.sink != *bus && move_si(si.index, bus) {
                moved += 1;
            }
            break;
        }
    }

    let Some(pref) = desired.preferred_default.as_deref().filter(|s| !s.is_empty()) else {
        return Ok(moved);
    };
    if !desired.owns_system_default() {
        return Ok(moved);
    }
    if !sinks.iter().any(|(_, n)| n == pref) {
        // Preferred sink missing briefly — wait; do not force HW.
        return Ok(moved);
    }

    let pref_is_buschain = pref.starts_with("buschain_") || pref.starts_with("shadow_");

    for si in &inputs {
        // Hold is keepalive only — pull user streams off buschain_hold onto preferred.
        let on_hold = si.sink == "buschain_hold";
        let unlinked = si.sink.is_empty();
        if si.internal && !on_hold && !unlinked {
            continue;
        }
        if is_anonymous_system_sound(si) {
            continue;
        }
        if key_to_bus.keys().any(|k| matches_key(si, k)) {
            continue;
        }
        if si.sink == pref {
            continue;
        }
        let on_buschain =
            si.sink.starts_with("buschain_") || si.sink.starts_with("shadow_");
        // Reclaim Hold / other buschain / empty(unlinked) / HW → preferred when
        // preferred is a BusChain VO (stream-restore often leaves apps on HW after quit).
        if on_hold || on_buschain || unlinked || pref_is_buschain {
            if move_si(si.index, pref) {
                moved += 1;
            }
        }
    }
    Ok(moved)
}

struct Si {
    index: u32,
    sink: String,
    application: String,
    binary: Option<String>,
    app_id: Option<String>,
    node_name: Option<String>,
    media_role: Option<String>,
    internal: bool,
}

/// Anonymous System Sounds only — must not reclaim (strands corked copies).
/// App-owned streams that happen to set `media.role=event|notify` (Discord
/// notifications, Electron secondary streams) keep real `bin:` / `id:` / name
/// identity and must follow preferred / pins like the main stream.
fn is_anonymous_system_sound(si: &Si) -> bool {
    if si.application.eq_ignore_ascii_case("System Sounds") {
        return true;
    }
    let is_event = matches!(
        si.media_role
            .as_deref()
            .map(|r| r.to_ascii_lowercase())
            .as_deref(),
        Some("event" | "notify" | "notification" | "alert")
    );
    if !is_event {
        return false;
    }
    !has_app_identity(
        &si.application,
        si.binary.as_deref(),
        si.app_id.as_deref(),
    )
}

fn has_app_identity(application: &str, binary: Option<&str>, app_id: Option<&str>) -> bool {
    if let Some(b) = binary.filter(|s| !s.is_empty()) {
        if !binary_is_generic(b) {
            return true;
        }
    }
    if app_id.is_some_and(|id| !id.is_empty()) {
        return true;
    }
    !application.is_empty()
        && !name_is_generic(application)
        && !looks_like_stream_label(application)
}

fn looks_like_stream_label(s: &str) -> bool {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("Stream ") {
        return rest.chars().all(|c| c.is_ascii_digit());
    }
    t.chars().all(|c| c.is_ascii_digit()) && !t.is_empty()
}

/// Generic runtimes whose binary is useless as an app identity.
/// Must match the app-side `binary_is_generic` (graph.rs) so pins stored by
/// the UI resolve to the same key here.
fn binary_is_generic(b: &str) -> bool {
    matches!(
        b.to_ascii_lowercase().as_str(),
        "electron"
            | "chrome"
            | "chromium"
            | "chrome-sandbox"
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

fn name_is_generic(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "chromium" | "chrome" | "electron" | "playback"
    )
}

/// Same key cascade as app-side `StreamNode::app_key`:
/// non-generic binary → id → non-generic name → generic binary → node → stream.
fn si_app_key(si: &Si) -> String {
    if let Some(b) = si.binary.as_deref().filter(|s| !s.is_empty()) {
        if !binary_is_generic(b) {
            return format!("bin:{b}");
        }
    }
    if let Some(id) = si.app_id.as_deref().filter(|s| !s.is_empty()) {
        return format!("id:{id}");
    }
    if !si.application.is_empty() && !name_is_generic(&si.application) {
        return format!("name:{}", si.application);
    }
    if let Some(b) = si.binary.as_deref().filter(|s| !s.is_empty()) {
        return format!("bin:{b}");
    }
    if let Some(n) = si.node_name.as_deref().filter(|s| !s.is_empty()) {
        return format!("node:{n}");
    }
    format!("stream:{}", si.index)
}

fn matches_key(si: &Si, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let app_key = si_app_key(si);
    if token == app_key {
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
        return si.node_name.as_deref() == Some(n);
    }
    if let Some(id) = token.strip_prefix("stream:") {
        return si.index.to_string() == id;
    }
    if si.index.to_string() == token {
        return true;
    }
    let tok = token.to_lowercase();
    si.application.to_lowercase() == tok
        || si.binary.as_deref().is_some_and(|b| b.to_lowercase() == tok)
        || si.app_id.as_deref().is_some_and(|id| id.to_lowercase() == tok)
}

fn list_short_sinks() -> Vec<(u32, String)> {
    let mut v = Vec::new();
    if let Ok(out) = std::process::Command::new("pactl")
        .args(["list", "short", "sinks"])
        .output()
    {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut p = line.split('\t');
            let Some(idx) = p.next().and_then(|s| s.parse().ok()) else {
                continue;
            };
            let Some(name) = p.next().map(|s| s.to_string()) else {
                continue;
            };
            v.push((idx, name));
        }
    }
    // Non-exported / helper buses may be absent from pactl — merge registry names
    // so PlaceApp/reclaim can still target non-VO tracks after promote.
    if super::native::native_ready() {
        for name in super::native::list_sink_names() {
            if name.starts_with("buschain_") && !v.iter().any(|(_, n)| n == &name) {
                v.push((0, name));
            }
        }
    }
    v
}

/// Pulse-visible = appears in `pactl list short sinks`, or Master present in registry.
fn pin_bus_is_ready(bus: &str, sinks: &[(u32, String)]) -> bool {
    if let Ok(out) = std::process::Command::new("pactl")
        .args(["list", "short", "sinks"])
        .output()
    {
        if String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|line| line.split('\t').nth(1) == Some(bus))
        {
            return true;
        }
    }
    // Master may briefly only exist in native registry during bring-up.
    bus == "buschain_master" && sinks.iter().any(|(_, n)| n == bus)
}

/// Streams from the native registry — sees Internal-hosted streams pactl hides.
fn list_sink_inputs_native() -> Vec<Si> {
    super::native::list_playback_streams()
        .into_iter()
        .map(|s| {
            let application = s
                .app_name
                .clone()
                .or_else(|| {
                    s.media_name.clone().filter(|m| {
                        let ml = m.to_lowercase();
                        ml != "playback" && ml != "buschain-control"
                    })
                })
                .unwrap_or_else(|| format!("Stream {}", s.serial));
            let media_role = s.media_role.clone();
            let binary = s.binary.clone();
            let app_id = s.app_id.clone();
            let node_name = Some(s.node_name.clone()).filter(|n| !n.is_empty());
            let anon = is_anonymous_system_sound(&Si {
                index: s.serial,
                sink: s.sink.clone(),
                application: application.clone(),
                binary: binary.clone(),
                app_id: app_id.clone(),
                node_name: node_name.clone(),
                media_role: media_role.clone(),
                internal: false,
            });
            let internal = s.node_virtual
                || s.sink.starts_with("buschain_fx_")
                || s.sink.starts_with("buschain_post_")
                || s.sink.starts_with("buschain_glc_")
                || s.sink.starts_with("buschain_rs_")
                || s.sink.starts_with("buschain_mtr_")
                || s.node_name.starts_with("buschain_")
                || s.node_name.contains("filter-chain")
                || s.media_name
                    .as_deref()
                    .is_some_and(|m| m.eq_ignore_ascii_case("buschain-control"))
                || application.to_lowercase().contains("buschain")
                || anon;
            Si {
                index: s.serial,
                sink: s.sink,
                application,
                binary,
                app_id,
                node_name,
                media_role,
                internal,
            }
        })
        .collect()
}

fn list_sink_inputs(sinks: &[(u32, String)]) -> Result<Vec<Si>> {
    let out = std::process::Command::new("pactl")
        .args(["list", "sink-inputs"])
        .output()
        .map_err(|e| anyhow!("pactl list sink-inputs: {e}"))?;
    if !out.status.success() {
        return Err(anyhow!("pactl list sink-inputs failed"));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut items = Vec::new();
    let mut cur: Option<u32> = None;
    let mut props: HashMap<String, String> = HashMap::new();

    let mut flush = |idx: Option<u32>, props: &HashMap<String, String>| {
        let Some(index) = idx else { return };
        let mut sink = props.get("Sink").cloned().unwrap_or_default();
        if let Ok(i) = sink.parse::<u32>() {
            if let Some((_, n)) = sinks.iter().find(|(idx, _)| *idx == i) {
                sink = n.clone();
            }
        }
        let g = |k: &str| props.get(k).map(|s| s.trim_matches('"').to_string());
        let application = g("application.name")
            .filter(|s| !s.is_empty())
            .or_else(|| {
                g("media.name").filter(|m| {
                    let ml = m.to_lowercase();
                    ml != "playback" && ml != "buschain-control"
                })
            })
            .unwrap_or_else(|| format!("Stream {index}"));
        let binary = g("application.process.binary").filter(|s| !s.is_empty());
        let app_id = g("application.id").filter(|s| !s.is_empty());
        let node_name = g("node.name").filter(|s| !s.is_empty());
        let media = g("media.name").unwrap_or_default();
        let media_role = g("media.role").filter(|s| !s.is_empty());
        let probe = Si {
            index,
            sink: sink.clone(),
            application: application.clone(),
            binary: binary.clone(),
            app_id: app_id.clone(),
            node_name: node_name.clone(),
            media_role: media_role.clone(),
            internal: false,
        };
        let anon = is_anonymous_system_sound(&probe);
        // Hold is parking/keepalive — user streams there must be reclaimable.
        // FX/post/rs + BusChain-owned media stay internal.
        // Anonymous System Sounds only — app-owned event roles stay reclaimable.
        let internal = sink.starts_with("buschain_fx_")
            || sink.starts_with("buschain_post_")
            || sink.starts_with("buschain_rs_")
            || node_name.as_deref().is_some_and(|n| {
                n.starts_with("buschain_") || n.contains("filter-chain")
            })
            || media.eq_ignore_ascii_case("buschain-control")
            || application.to_lowercase().contains("buschain")
            || anon;
        items.push(Si {
            index,
            sink,
            application,
            binary,
            app_id,
            node_name,
            media_role,
            internal,
        });
    };

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Sink Input #") {
            flush(cur.take(), &props);
            props.clear();
            cur = rest.trim().parse().ok();
            continue;
        }
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Sink:") {
            props.insert("Sink".into(), rest.trim().to_string());
            continue;
        }
        if let Some((k, v)) = t.split_once(" = ") {
            props.insert(
                k.trim().to_string(),
                v.trim().trim_matches('"').to_string(),
            );
        }
    }
    flush(cur, &props);
    Ok(items)
}

fn move_si(index: u32, sink: &str) -> bool {
    // Chromium/Electron pause on every retarget — never rewrite when already on sink.
    if super::native::native_ready() {
        if let Ok(true) = super::native::stream_targets_sink(index, sink) {
            return false;
        }
    }
    super::pulse_compat::move_sink_input(index, sink).unwrap_or(false)
}
