//! Map session [`PluginRef`] racks → sealed engine [`ChainSpec`] black boxes.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use buschain_engine::{
    normalize_ladspa_label, ChainEnsureMode, ChainSpec, ChainState, InsertFormat, InsertSlot,
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
        "../plugins/buschain-reverb/build",
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
    match plug.id.format {
        PluginFormat::Ladspa => {
            let label = normalize_label(&plug.id.id).to_string();
            let key = normalize_ladspa_label(&label).to_string();
            Some(InsertSlot {
                slot_id: plug.slot_id,
                plugin_key: key.clone(),
                plugin_so: resolve_plugin_so(&key),
                controls: plug.effective_control_params(),
                format: InsertFormat::Ladspa,
                state_blob: None,
                sidechain_from: plug.sidechain_from.clone(),
            })
        }
        PluginFormat::Clap => {
            let path = resolve_clap_path(&plug.id.id);
            Some(InsertSlot {
                slot_id: plug.slot_id,
                plugin_key: plug.id.id.clone(),
                plugin_so: path,
                controls: plug.effective_control_params(),
                format: InsertFormat::Clap,
                state_blob: plug.state_blob.clone(),
                sidechain_from: plug.sidechain_from.clone(),
            })
        }
        PluginFormat::Vst3 => {
            let path = resolve_vst3_path(&plug.id.id);
            Some(InsertSlot {
                slot_id: plug.slot_id,
                plugin_key: plug.id.id.clone(),
                plugin_so: path,
                controls: plug.effective_control_params(),
                format: InsertFormat::Vst3,
                state_blob: plug.state_blob.clone(),
                sidechain_from: plug.sidechain_from.clone(),
            })
        }
        PluginFormat::Lv2 => {
            let (uri, bundle) = resolve_lv2_identity(&plug.id.id);
            Some(InsertSlot {
                slot_id: plug.slot_id,
                plugin_key: uri,
                plugin_so: bundle,
                controls: plug.effective_control_params(),
                format: InsertFormat::Lv2,
                state_blob: plug.state_blob.clone(),
                sidechain_from: plug.sidechain_from.clone(),
            })
        }
    }
}

/// Props-only slot — controls matter; skip FS `.so` walks on every knob flush.
pub fn plugin_ref_to_controls_slot(plug: &PluginRef) -> Option<InsertSlot> {
    match plug.id.format {
        PluginFormat::Ladspa => {
            let label = normalize_label(&plug.id.id).to_string();
            let key = normalize_ladspa_label(&label).to_string();
            Some(InsertSlot {
                slot_id: plug.slot_id,
                plugin_key: key,
                plugin_so: String::new(),
                controls: plug.effective_control_params(),
                format: InsertFormat::Ladspa,
                state_blob: None,
                sidechain_from: plug.sidechain_from.clone(),
            })
        }
        PluginFormat::Clap | PluginFormat::Vst3 | PluginFormat::Lv2 => plugin_ref_to_slot(plug),
    }
}

fn resolve_lv2_identity(id: &str) -> (String, String) {
    if let Some(path) = id.strip_prefix("bundle:") {
        return (String::new(), path.to_string());
    }
    (id.to_string(), String::new())
}

fn resolve_clap_path(id: &str) -> String {
    if std::path::Path::new(id).is_file() {
        return id.to_string();
    }
    let mut dirs = Vec::new();
    if let Ok(env) = std::env::var("CLAP_PATH") {
        for p in env.split(':').filter(|s| !s.is_empty()) {
            dirs.push(std::path::PathBuf::from(p));
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let h = std::path::PathBuf::from(home);
        dirs.push(h.join(".clap"));
        dirs.push(h.join(".local/lib/clap"));
    }
    dirs.push(std::path::PathBuf::from("/usr/lib/clap"));
    let file = if id.ends_with(".clap") {
        id.to_string()
    } else {
        format!("{id}.clap")
    };
    for dir in &dirs {
        let p = dir.join(&file);
        if p.is_file() {
            return p.display().to_string();
        }
        // Nested vendor folders
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let cand = e.path().join(&file);
                if cand.is_file() {
                    return cand.display().to_string();
                }
            }
        }
    }
    id.to_string()
}

fn resolve_vst3_path(id: &str) -> String {
    let p = std::path::Path::new(id);
    if p.exists() {
        return id.to_string();
    }
    // Bare name / stem → search standard VST3 roots (+ VST3_PATH).
    let mut roots = Vec::new();
    if let Ok(env) = std::env::var("VST3_PATH") {
        for part in env.split(':').filter(|s| !s.is_empty()) {
            roots.push(std::path::PathBuf::from(part));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let h = std::path::PathBuf::from(home);
        roots.push(h.join(".vst3"));
        roots.push(h.join(".local/lib/vst3"));
    }
    roots.push(std::path::PathBuf::from("/usr/lib/vst3"));
    roots.push(std::path::PathBuf::from("/usr/local/lib/vst3"));
    roots.push(std::path::PathBuf::from("/usr/lib64/vst3"));

    let file = if id.ends_with(".vst3") {
        id.to_string()
    } else {
        format!("{id}.vst3")
    };
    for root in &roots {
        let direct = root.join(&file);
        if direct.exists() {
            return direct.display().to_string();
        }
        // Nested vendor folders
        if let Ok(rd) = std::fs::read_dir(root) {
            for e in rd.flatten() {
                let cand = e.path().join(&file);
                if cand.exists() {
                    return cand.display().to_string();
                }
            }
        }
    }
    id.to_string()
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

/// Primary wet egress for a track's FX chain (Master→HW or first output target).
/// Empty when the track has no Output to… (hold-only); FX spine still builds,
/// but post→dest is not armed until a destination is added.
pub fn primary_fx_dest(session: &Session, track_id: uuid::Uuid, hw_sink: &str) -> String {
    let Some(track) = session.tracks.iter().find(|t| t.id == track_id) else {
        return "buschain_master".into();
    };
    if track.kind.is_master() {
        return hw_sink.to_string();
    }
    let master_id = session.master_id();
    let bus = track.expected_sink_name();
    let mut targets = track.output_targets.clone();
    if track.listen {
        if let Some(mid) = master_id {
            if !targets.contains(&mid) {
                targets.push(mid);
            }
        }
    }
    targets.sort();
    targets.dedup();
    for tid in targets {
        if master_id == Some(tid) {
            return "buschain_master".into();
        }
        if let Some(t) = session.tracks.iter().find(|t| t.id == tid) {
            let dest = t.expected_sink_name();
            if dest != bus {
                return dest;
            }
        }
    }
    // Device-only stem: an empty dest makes prune_parallel_fx_routes skip the
    // bus entirely, so the wet post→device hop would never be pruned or healed.
    for out in &track.output_devices {
        let dest = out.device.trim();
        if !dest.is_empty()
            && !dest.starts_with("buschain_")
            && !dest.starts_with("shadow_")
        {
            return dest.to_string();
        }
    }
    String::new()
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
