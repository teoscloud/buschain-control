//! One-shot session playback placement (Desired `bus_playback` → sink-inputs).

use std::collections::HashMap;

use anyhow::{anyhow, Result};

use crate::plan::DesiredState;

/// Move pinned apps onto their buses; reclaim unassigned user apps to preferred default.
/// Single `pactl list sink-inputs` pass — never N× per-key list storms.
pub fn enforce_desired_playback(desired: &DesiredState) -> Result<u32> {
    let sinks = list_short_sinks();
    let inputs = list_sink_inputs(&sinks)?;
    let mut moved = 0u32;

    let mut key_to_bus: HashMap<String, String> = HashMap::new();
    for (bus, keys) in &desired.bus_playback {
        if !sinks.iter().any(|(_, n)| n == bus) {
            continue;
        }
        for k in keys {
            key_to_bus.insert(k.clone(), bus.clone());
        }
    }

    for si in &inputs {
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
        if si.internal && !on_hold {
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
        // Reclaim HW → preferred BusChain, other buschain sinks / Hold → preferred.
        if on_hold || on_buschain || pref_is_buschain {
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
    internal: bool,
}

fn matches_key(si: &Si, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let app_key = if let Some(b) = si.binary.as_deref().filter(|s| !s.is_empty()) {
        format!("bin:{b}")
    } else if let Some(id) = si.app_id.as_deref().filter(|s| !s.is_empty()) {
        format!("id:{id}")
    } else if !si.application.is_empty() {
        format!("name:{}", si.application)
    } else if let Some(n) = si.node_name.as_deref().filter(|s| !s.is_empty()) {
        format!("node:{n}")
    } else {
        format!("stream:{}", si.index)
    };
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
    let Ok(out) = std::process::Command::new("pactl")
        .args(["list", "short", "sinks"])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut p = line.split('\t');
            let idx = p.next()?.parse().ok()?;
            let name = p.next()?.to_string();
            Some((idx, name))
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
            .or_else(|| g("media.name").filter(|m| {
                let ml = m.to_lowercase();
                ml != "playback" && ml != "buschain-control"
            }))
            .unwrap_or_else(|| format!("Stream {index}"));
        let binary = g("application.process.binary").filter(|s| !s.is_empty());
        let app_id = g("application.id").filter(|s| !s.is_empty());
        let node_name = g("node.name").filter(|s| !s.is_empty());
        let media = g("media.name").unwrap_or_default();
        // Hold is parking/keepalive — user streams there must be reclaimable.
        // FX/post/rs + BusChain-owned media stay internal.
        let internal = sink.starts_with("buschain_fx_")
            || sink.starts_with("buschain_post_")
            || sink.starts_with("buschain_rs_")
            || node_name.as_deref().is_some_and(|n| {
                n.starts_with("buschain_") || n.contains("filter-chain")
            })
            || media.eq_ignore_ascii_case("buschain-control")
            || application.to_lowercase().contains("buschain");
        items.push(Si {
            index,
            sink,
            application,
            binary,
            app_id,
            node_name,
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
    std::process::Command::new("pactl")
        .args(["move-sink-input", &index.to_string(), sink])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
