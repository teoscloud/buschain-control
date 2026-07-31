//! Aggregate mixer JSON for Quickshell / GTK / ctl (`get_mixer` / `buschain-ctl mixer`).

use serde_json::{json, Value};

use crate::audio::graph::PwSnapshot;
use crate::ipc::Status;
use crate::session::Session;

fn is_internal_helper_node(name: &str) -> bool {
    name.starts_with("easyeffects_")
        || name.contains("filter-chain")
        || name == "auto_null"
        // Graph helpers — not user-facing virtual I/O
        || name.starts_with("buschain_hold")
        || name.starts_with("buschain_post_")
        || name.starts_with("buschain_rs_")
        || name.starts_with("buschain_vinf_")
        || name.starts_with("buschain_fx_")
}

/// Track/master bus names that are app-facing virtual sinks.
/// Internal track buses always exist in PW; only session `virtual_output`
/// (plus master) should appear in shell Output / QS device lists.
fn is_exposed_virtual_sink(name: &str, session: &Session) -> bool {
    session.tracks.iter().any(|t| {
        t.expected_sink_name() == name && (t.kind.is_master() || t.virtual_output)
    })
}

/// App-facing virtual mics created via Create system virtual input.
fn is_exposed_virtual_source(name: &str, session: &Session) -> bool {
    session.tracks.iter().any(|t| {
        !t.kind.is_master() && t.virtual_input && t.expected_virtual_input_name() == name
    })
}

fn include_sink(name: &str, session: Option<&Session>) -> bool {
    if is_internal_helper_node(name) {
        return false;
    }
    if name.starts_with("buschain_") {
        return session
            .map(|s| is_exposed_virtual_sink(name, s))
            .unwrap_or(false);
    }
    true
}

fn include_source(name: &str, session: Option<&Session>) -> bool {
    if is_internal_helper_node(name) || name.contains(".monitor") {
        return false;
    }
    if name.starts_with("buschain_vin_") {
        return session
            .map(|s| is_exposed_virtual_source(name, s))
            .unwrap_or(false);
    }
    if name.starts_with("buschain_") {
        return false;
    }
    true
}

/// Stable poll payload for shell mixers (QS / GTK parity).
pub fn build_mixer_json(
    status: Option<&Status>,
    snapshot: Option<&PwSnapshot>,
    session: Option<&Session>,
) -> Value {
    let master = session
        .and_then(|s| s.master_output.clone())
        .or_else(|| status.and_then(|s| s.master_hw.clone()));
    let default_sink = snapshot.and_then(|s| s.default_sink.clone());
    let default_source = snapshot.and_then(|s| s.default_source.clone());

    let streams: Vec<Value> = snapshot
        .map(|snap| {
            snap.sink_inputs
                .iter()
                .filter(|s| s.is_user_app())
                .map(|s| {
                    json!({
                        "index": s.index,
                        "name": s.display_name(),
                        "meta": s.sink_or_source,
                        "volume_pct": s.volume_pct,
                        "mute": s.mute,
                        "icon_name": s.icon_name,
                        "binary": s.binary,
                        "app_id": s.app_id,
                        "application": s.application,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // Only session-exposed virtual buses (master + tracks with Create system
    // virtual output on). Other buschain_track_* sinks stay internal for routing.
    let sinks: Vec<Value> = snapshot
        .map(|snap| {
            snap.sinks
                .iter()
                .filter(|s| include_sink(&s.name, session))
                .map(|s| {
                    let is_virtual = session
                        .map(|sess| is_exposed_virtual_sink(&s.name, sess))
                        .unwrap_or(false);
                    json!({
                        "name": s.name,
                        "desc": s.description,
                        "volume_pct": s.volume_pct,
                        "mute": s.mute,
                        "is_master": master.as_deref() == Some(s.name.as_str()),
                        "is_default": default_sink.as_deref() == Some(s.name.as_str()),
                        "is_virtual": is_virtual,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let sources: Vec<Value> = snapshot
        .map(|snap| {
            snap.sources
                .iter()
                .filter(|s| include_source(&s.name, session))
                .map(|s| {
                    let is_virtual = session
                        .map(|sess| is_exposed_virtual_source(&s.name, sess))
                        .unwrap_or(false);
                    json!({
                        "name": s.name,
                        "desc": s.description,
                        "volume_pct": s.volume_pct,
                        "mute": s.mute,
                        "is_default": default_source.as_deref() == Some(s.name.as_str()),
                        "is_virtual": is_virtual,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let tracks: Vec<Value> = session
        .map(|s| {
            s.tracks
                .iter()
                .map(|t| {
                    let kind = if t.kind.is_master() {
                        "master"
                    } else {
                        "track"
                    };
                    json!({
                        "id": t.id.to_string(),
                        "name": t.name,
                        "kind": kind,
                        "gain_db": t.gain_db,
                        "mute": t.mute,
                        "bus": t.expected_sink_name(),
                        "virtual_output": t.virtual_output,
                        "virtual_input": t.virtual_input,
                        "virtual_input_source": if t.virtual_input && !t.kind.is_master() {
                            Some(t.expected_virtual_input_name())
                        } else {
                            None
                        },
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    json!({
        "status": status.map(|s| json!({
            "master_hw": s.master_hw,
            "master_hw_desc": s.master_hw_desc,
            "hw_volume_pct": s.hw_volume_pct.min(100),
            "hw_mute": s.hw_mute,
            "session_name": s.session_name,
            "session_slug": s.session_slug,
            "sample_rate": s.sample_rate,
            "quantum": s.quantum,
        })),
        "streams": streams,
        "sinks": sinks,
        "sources": sources,
        "tracks": tracks,
        "default_sink": default_sink,
        "default_source": default_source,
    })
}

/// Touch `$XDG_RUNTIME_DIR/buschain-control/mixer.tick` so QS can FileView-wake.
pub fn touch_mixer_tick() {
    let path = crate::ipc::runtime_dir().join("mixer.tick");
    let _ = std::fs::create_dir_all(crate::ipc::runtime_dir());
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .and_then(|f| {
            use std::io::Write;
            let mut f = f;
            f.write_all(b"1")
        });
}
