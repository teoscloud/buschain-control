//! Named session snapshots under `~/.config/buschain-control/sessions/`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use super::Session;

/// Soft-bind warnings when HW/inputs are missing after load.
#[derive(Debug, Clone, Default)]
pub struct ResolveReport {
    pub messages: Vec<String>,
}

impl ResolveReport {
    pub fn push(&mut self, m: impl Into<String>) {
        self.messages.push(m.into());
    }

    pub fn join(&self) -> String {
        self.messages.join(" · ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub slug: String,
    pub name: String,
    pub path: PathBuf,
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("buschain-control")
}

pub fn sessions_dir() -> PathBuf {
    config_dir().join("sessions")
}

pub fn active_path() -> PathBuf {
    config_dir().join("active")
}

/// Legacy single-file path (migrated once into `sessions/default.json`).
pub fn legacy_session_path() -> PathBuf {
    config_dir().join("session.json")
}

pub fn slugify(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else if c.is_whitespace() || c == '-' || c == '_' {
                '-'
            } else {
                '-'
            }
        })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "session".into()
    } else {
        s
    }
}

pub fn session_path(slug: &str) -> PathBuf {
    sessions_dir().join(format!("{slug}.json"))
}

pub fn read_active_slug() -> String {
    fs::read_to_string(active_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".into())
}

pub fn write_active_slug(slug: &str) -> Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    fs::write(active_path(), slug.trim())?;
    Ok(())
}

/// One-time migrate `session.json` → `sessions/default.json`.
pub fn migrate_legacy_if_needed() -> Result<()> {
    fs::create_dir_all(sessions_dir())?;
    let legacy = legacy_session_path();
    let default = session_path("default");
    if legacy.is_file() && !default.is_file() {
        fs::copy(&legacy, &default).with_context(|| format!("migrate {legacy:?}"))?;
        let _ = write_active_slug("default");
    }
    if !active_path().is_file() {
        let _ = write_active_slug("default");
    }
    Ok(())
}

pub fn list_sessions() -> Result<Vec<SessionMeta>> {
    migrate_legacy_if_needed()?;
    let dir = sessions_dir();
    let mut out = Vec::new();
    let rd = match fs::read_dir(&dir) {
        Ok(r) => r,
        Err(_) => return Ok(out),
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let slug = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("session")
            .to_string();
        let name = fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Session>(&t).ok())
            .map(|s| {
                if s.name.is_empty() {
                    slug.clone()
                } else {
                    s.name.clone()
                }
            })
            .unwrap_or_else(|| slug.clone());
        out.push(SessionMeta { slug, name, path });
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

pub fn load_slug(slug: &str) -> Result<Session> {
    migrate_legacy_if_needed()?;
    let path = session_path(slug);
    let mut session = if path.is_file() {
        let text = fs::read_to_string(&path)?;
        serde_json::from_str(&text).unwrap_or_default()
    } else if slug == "default" {
        // Fall back to legacy or fresh default.
        if legacy_session_path().is_file() {
            let text = fs::read_to_string(legacy_session_path())?;
            serde_json::from_str(&text).unwrap_or_default()
        } else {
            Session::default()
        }
    } else {
        return Err(anyhow!("session not found: {slug}"));
    };
    if session.slug.is_empty() {
        session.slug = slug.to_string();
    }
    if session.name.is_empty() {
        session.name = slug.to_string();
    }
    let dirty = session.normalize();
    session.clamp_performance_to_device();
    if dirty {
        // Persist shadow_* → buschain_* insert migrations so FX conf never reloads dead labels.
        let slug = session.slug.clone();
        let _ = save_session_as(&mut session, &slug, None);
    }
    Ok(session)
}

pub fn load_active() -> Session {
    let _ = migrate_legacy_if_needed();
    let slug = read_active_slug();
    match load_slug(&slug) {
        Ok(s) => s,
        Err(_) => {
            let mut s = Session::default();
            s.slug = "default".into();
            s.name = "Default".into();
            let _ = s.normalize();
            s
        }
    }
}

pub fn save_session_as(session: &mut Session, slug: &str, name: Option<&str>) -> Result<()> {
    migrate_legacy_if_needed()?;
    let slug = slugify(slug);
    session.slug = slug.clone();
    if let Some(n) = name {
        session.name = n.to_string();
    } else if session.name.is_empty() {
        session.name = slug.clone();
    }
    session.prune_for_persist();
    session.touch_saved_at();
    let path = session_path(&slug);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(session)?;
    fs::write(&path, text)?;
    write_active_slug(&slug)?;
    // Keep legacy path in sync for older tools.
    let _ = fs::write(legacy_session_path(), serde_json::to_string_pretty(session)?);
    Ok(())
}

pub fn save_active(session: &mut Session) -> Result<()> {
    let slug = if session.slug.is_empty() {
        read_active_slug()
    } else {
        session.slug.clone()
    };
    save_session_as(session, &slug, None)
}

pub fn delete_slug(slug: &str) -> Result<()> {
    let slug = slug.trim();
    if slug.is_empty() {
        return Err(anyhow!("empty slug"));
    }
    let path = session_path(slug);
    if path.is_file() {
        fs::remove_file(&path)?;
    }
    let remaining = list_sessions()?;
    if remaining.is_empty() {
        let mut s = Session::default();
        s.slug = "default".into();
        s.name = "Default".into();
        save_session_as(&mut s, "default", Some("Default"))?;
    } else if read_active_slug() == slug {
        write_active_slug(&remaining[0].slug)?;
    }
    Ok(())
}

pub fn rename_slug(old: &str, new_name: &str) -> Result<String> {
    let mut session = load_slug(old)?;
    let new_slug = slugify(new_name);
    if new_slug != old && session_path(&new_slug).is_file() {
        return Err(anyhow!("session already exists: {new_slug}"));
    }
    session.name = new_name.to_string();
    save_session_as(&mut session, &new_slug, Some(new_name))?;
    if new_slug != old {
        let _ = fs::remove_file(session_path(old));
    }
    Ok(new_slug)
}

/// Soft-bind Master HW / inputs / device_clocks to currently available nodes.
pub fn resolve_devices(
    session: &mut Session,
    sink_names: &[(String, String)],
    source_names: &[(String, String)],
) -> ResolveReport {
    let mut report = ResolveReport::default();

    // Master HW out
    if let Some(name) = session.master_output.clone() {
        if !sink_names.iter().any(|(n, _)| n == &name) {
            let desc = session.master_output_desc.clone().unwrap_or_default();
            let fallback = find_by_desc(sink_names, &desc)
                .or_else(|| first_hw_sink(sink_names));
            match fallback {
                Some((n, d)) => {
                    report.push(format!(
                        "Master HW rebound {name} → {d}"
                    ));
                    session.master_output = Some(n);
                    session.master_output_desc = Some(d);
                }
                None => {
                    report.push(format!("Master HW missing ({name}) — cleared"));
                    session.master_output = None;
                }
            }
        }
    } else if let Some((n, d)) = first_hw_sink(sink_names) {
        session.master_output = Some(n);
        session.master_output_desc = Some(d);
        report.push("Master HW auto-picked first hardware sink");
    }

    // Preferred default — drop if gone (don't force to HW)
    if let Some(pref) = session.preferred_default_sink.clone() {
        if !sink_names.iter().any(|(n, _)| n == &pref) {
            report.push(format!("preferred default missing ({pref}) — cleared"));
            session.preferred_default_sink = None;
        }
    }

    // Track inputs
    for t in &mut session.tracks {
        if let Some(src) = t.input_source.clone() {
            if !source_names.iter().any(|(n, _)| n == &src) {
                let desc = t.input_source_desc.clone().unwrap_or_default();
                if let Some((n, d)) = find_by_desc(source_names, &desc) {
                    report.push(format!("input rebound on {}: {src} → {d}", t.name));
                    t.input_source = Some(n);
                    t.input_source_desc = Some(d);
                } else {
                    report.push(format!("input missing on {} ({src}) — cleared", t.name));
                    t.input_source = None;
                }
            }
        }
    }

    // device_clocks — drop absent keys
    let live: std::collections::HashSet<&str> = sink_names
        .iter()
        .map(|(n, _)| n.as_str())
        .chain(source_names.iter().map(|(n, _)| n.as_str()))
        .collect();
    let stale: Vec<String> = session
        .device_clocks
        .keys()
        .filter(|k| !live.contains(k.as_str()))
        .cloned()
        .collect();
    for k in stale {
        session.device_clocks.remove(&k);
        report.push(format!("dropped clock prefs for missing device {k}"));
    }

    report
}

fn first_hw_sink(sinks: &[(String, String)]) -> Option<(String, String)> {
    sinks
        .iter()
        .find(|(n, _)| !n.starts_with("buschain_"))
        .cloned()
}

fn find_by_desc(nodes: &[(String, String)], desc: &str) -> Option<(String, String)> {
    if desc.is_empty() {
        return None;
    }
    let dl = desc.to_ascii_lowercase();
    nodes
        .iter()
        .find(|(_, d)| d.eq_ignore_ascii_case(desc))
        .cloned()
        .or_else(|| {
            nodes
                .iter()
                .find(|(_, d)| {
                    let x = d.to_ascii_lowercase();
                    x.contains(&dl) || dl.contains(&x)
                })
                .cloned()
        })
}

pub fn ensure_sessions_dir() -> Result<()> {
    fs::create_dir_all(sessions_dir())?;
    Ok(())
}

/// Validate slug is a single path segment.
pub fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && !slug.contains('/')
        && !slug.contains('\\')
        && !slug.contains("..")
        && Path::new(slug).file_name().map(|s| s == slug).unwrap_or(false)
}
