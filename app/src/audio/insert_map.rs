//! Map session [`PluginRef`] racks → sealed engine [`ChainSpec`] black boxes.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use buschain_engine::{
    normalize_ladspa_label, ChainEnsureMode, ChainSpec, ChainState, InsertSlot,
    NodeName,
};

use crate::audio::plugin::{normalize_label, plugin_file_for, PluginFormat, PluginRef};
use crate::session::{Session, Track};

fn so_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resolve absolute LADSPA `.so` for a label (engine prefers full paths).
/// Cached process-wide so Props flushes never re-walk `LADSPA_PATH`.
fn resolve_plugin_so(label: &str) -> String {
    if let Ok(cache) = so_cache().lock() {
        if let Some(hit) = cache.get(label) {
            return hit.clone();
        }
    }
    let resolved = resolve_plugin_so_uncached(label);
    if let Ok(mut cache) = so_cache().lock() {
        cache.insert(label.to_string(), resolved.clone());
    }
    resolved
}

fn resolve_plugin_so_uncached(label: &str) -> String {
    let stem = plugin_file_for(label);
    let file = format!("{stem}.so");
    // Match engine search order roughly — prefer env LADSPA_PATH + common dirs.
    let mut dirs = Vec::new();
    if let Ok(env) = std::env::var("LADSPA_PATH") {
        for p in env.split(':').filter(|s| !s.is_empty()) {
            dirs.push(std::path::PathBuf::from(p));
        }
    }
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for sub in [
        "../plugins/buschain-denoiser/build",
        "../plugins/buschain-gate/build",
        "../plugins/buschain-builtins/build",
    ] {
        dirs.push(manifest.join(sub));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(std::path::PathBuf::from(home).join(".local/lib/ladspa"));
    }
    for dir in &dirs {
        let p = dir.join(&file);
        if p.is_file() {
            return p.display().to_string();
        }
    }
    stem.to_string()
}

pub fn plugin_ref_to_slot(plug: &PluginRef) -> Option<InsertSlot> {
    if plug.id.format != PluginFormat::Ladspa {
        return None;
    }
    let label = normalize_label(&plug.id.id).to_string();
    let key = normalize_ladspa_label(&label).to_string();
    Some(InsertSlot {
        slot_id: plug.slot_id,
        plugin_key: key.clone(),
        plugin_so: resolve_plugin_so(&key),
        controls: plug.effective_control_params(),
    })
}

/// Props-only slot — controls matter; skip FS `.so` walks on every knob flush.
pub fn plugin_ref_to_controls_slot(plug: &PluginRef) -> Option<InsertSlot> {
    if plug.id.format != PluginFormat::Ladspa {
        return None;
    }
    let label = normalize_label(&plug.id.id).to_string();
    let key = normalize_ladspa_label(&label).to_string();
    Some(InsertSlot {
        slot_id: plug.slot_id,
        plugin_key: key,
        plugin_so: String::new(),
        controls: plug.effective_control_params(),
    })
}

pub fn ladspa_slots(track: &Track) -> Vec<InsertSlot> {
    track
        .inserts
        .iter()
        .filter_map(plugin_ref_to_slot)
        .collect()
}

/// Controls-only rack for live Props (no `resolve_plugin_so` per flush).
pub fn controls_only_slots(track: &Track) -> Vec<InsertSlot> {
    track
        .inserts
        .iter()
        .filter_map(plugin_ref_to_controls_slot)
        .collect()
}

pub fn chain_spec_for_track(session: &Session, track_id: uuid::Uuid, dest: &str) -> Option<ChainSpec> {
    let track = session.tracks.iter().find(|t| t.id == track_id)?;
    let bus = track.expected_sink_name();
    let inserts = ladspa_slots(track);
    Some(ChainSpec {
        bus: NodeName::new(bus),
        inserts,
        dest: dest.to_string(),
    })
}

pub fn ensure_track_fx(
    session: &Session,
    track_id: uuid::Uuid,
    dest: &str,
    mode: ChainEnsureMode,
) -> anyhow::Result<(ChainState, String)> {
    let spec = chain_spec_for_track(session, track_id, dest)
        .ok_or_else(|| anyhow::anyhow!("track not found"))?;
    let n = spec.inserts.len();
    let state = crate::audio::engine_handle::ensure_fx_chain(spec, mode)?;
    let msg = match &state {
        ChainState::Dry => "FX dry (no LADSPA inserts)".into(),
        ChainState::Building => "FX building…".into(),
        ChainState::Wet(w) => format!(
            "FX wet · {n} inserts · {} → {}",
            w.fx_sink.as_str(),
            w.dest
        ),
        ChainState::Failed(e) => return Err(anyhow::anyhow!("FX failed: {e}")),
    };
    Ok((state, msg))
}

/// Bus + controls-only inserts for Props flush (skips FS walks).
pub fn ladspa_slots_for(
    session: &Session,
    track_id: uuid::Uuid,
) -> Option<(String, Vec<InsertSlot>)> {
    let track = session.tracks.iter().find(|t| t.id == track_id)?;
    let bus = track.expected_sink_name();
    let inserts = controls_only_slots(track);
    Some((bus, inserts))
}

pub fn push_track_controls(session: &Session, track_id: uuid::Uuid) -> anyhow::Result<String> {
    let (bus, inserts) = ladspa_slots_for(session, track_id)
        .ok_or_else(|| anyhow::anyhow!("track not found"))?;
    if inserts.is_empty() {
        return Ok(format!("No LADSPA inserts on {bus}"));
    }
    crate::audio::engine_handle::push_fx_controls(&bus, inserts)?;
    Ok(format!(
        "Live params → {}",
        buschain_engine::live_fx_name(&bus)
    ))
}

pub fn push_bus_controls(bus: &str, inserts: Vec<InsertSlot>) -> anyhow::Result<String> {
    if inserts.is_empty() {
        return Ok(format!("No LADSPA inserts on {bus}"));
    }
    crate::audio::engine_handle::push_fx_controls(bus, inserts)?;
    Ok(format!(
        "Live params → {}",
        buschain_engine::live_fx_name(bus)
    ))
}
