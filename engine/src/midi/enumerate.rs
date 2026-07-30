//! Enumerate PipeWire MIDI nodes (registry cache + pw-cli fallback).

use super::types::MidiDeviceLive;

fn is_midi_class(class: &str) -> bool {
    let c = class.to_ascii_lowercase();
    c.contains("midi") || c.contains("Midi")
}

/// List MIDI endpoints from the native PipeWire registry cache.
pub fn enumerate_pw() -> Vec<MidiDeviceLive> {
    let mut out: Vec<MidiDeviceLive> = crate::backend::list_midi_nodes()
        .into_iter()
        .map(|(id, description)| MidiDeviceLive {
            id,
            description,
            enabled: true,
            activity: 0.0,
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

/// pw-cli fallback when the registry plane is cold.
pub fn enumerate_pw_cli() -> Vec<MidiDeviceLive> {
    let Ok(out) = std::process::Command::new("pw-cli")
        .args(["ls", "Node"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut devices = Vec::new();
    let mut name = None::<String>;
    let mut desc = None::<String>;
    let mut class = None::<String>;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("id ") && name.is_some() {
            flush_node(&mut devices, &mut name, &mut desc, &mut class);
        }
        if let Some(v) = t.strip_prefix("node.name = \"") {
            name = Some(v.trim_end_matches('"').to_string());
        } else if let Some(v) = t.strip_prefix("node.description = \"") {
            desc = Some(v.trim_end_matches('"').to_string());
        } else if let Some(v) = t.strip_prefix("media.class = \"") {
            class = Some(v.trim_end_matches('"').to_string());
        }
    }
    flush_node(&mut devices, &mut name, &mut desc, &mut class);
    devices.sort_by(|a, b| a.id.cmp(&b.id));
    devices.dedup_by(|a, b| a.id == b.id);
    devices
}

fn flush_node(
    out: &mut Vec<MidiDeviceLive>,
    name: &mut Option<String>,
    desc: &mut Option<String>,
    class: &mut Option<String>,
) {
    let Some(id) = name.take() else {
        desc.take();
        class.take();
        return;
    };
    let media = class.take().unwrap_or_default();
    let description = desc.take().unwrap_or_else(|| id.clone());
    if !is_midi_class(&media) {
        return;
    }
    out.push(MidiDeviceLive {
        id,
        description,
        enabled: true,
        activity: 0.0,
    });
}

/// Merge persisted enable flags into a live device list.
pub fn merge_session_devices(
    live: Vec<MidiDeviceLive>,
    session: &[super::types::MidiDeviceInfo],
) -> Vec<MidiDeviceLive> {
    let mut out = live;
    for saved in session {
        if let Some(d) = out.iter_mut().find(|d| d.id == saved.id) {
            d.enabled = saved.enabled;
            if !saved.description.is_empty() {
                d.description = saved.description.clone();
            }
        } else {
            out.push(MidiDeviceLive {
                id: saved.id.clone(),
                description: saved.description.clone(),
                enabled: saved.enabled,
                activity: 0.0,
            });
        }
    }
    out.sort_by(|a, b| a.description.cmp(&b.description));
    out
}

/// Best-effort device list (PipeWire registry, then pw-cli).
pub fn enumerate_devices(session: &[super::types::MidiDeviceInfo]) -> Vec<MidiDeviceLive> {
    let live = {
        let pw = enumerate_pw();
        if pw.is_empty() {
            enumerate_pw_cli()
        } else {
            pw
        }
    };
    merge_session_devices(live, session)
}
