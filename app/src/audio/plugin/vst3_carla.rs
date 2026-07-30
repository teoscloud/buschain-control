//! VST3 discovery (filesystem walk — Carla optional/legacy name).
//! Audio always runs in the in-engine VST3 host, never via Carla.

use std::path::{Path, PathBuf};

use super::{PluginBackend, PluginDescriptor, PluginFormat, PluginId};

pub struct CarlaVst3Backend;

impl CarlaVst3Backend {
    pub fn new() -> Self {
        Self
    }
}

impl PluginBackend for CarlaVst3Backend {
    fn format(&self) -> PluginFormat {
        PluginFormat::Vst3
    }

    fn scan(&self) -> Vec<PluginDescriptor> {
        let mut plugins = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for root in vst3_search_roots() {
            if !root.is_dir() {
                continue;
            }
            walk_vst3_bundles(&root, &mut plugins, &mut seen, 0);
        }

        plugins.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        plugins
    }
}

fn vst3_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(env) = std::env::var("VST3_PATH") {
        for p in env.split(':').filter(|s| !s.is_empty()) {
            roots.push(PathBuf::from(p));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let h = PathBuf::from(home);
        roots.push(h.join(".vst3"));
        roots.push(h.join(".local/lib/vst3"));
    }
    roots.push(PathBuf::from("/usr/lib/vst3"));
    roots.push(PathBuf::from("/usr/local/lib/vst3"));
    roots.push(PathBuf::from("/usr/lib64/vst3"));
    roots
}

fn walk_vst3_bundles(
    dir: &Path,
    out: &mut Vec<PluginDescriptor>,
    seen: &mut std::collections::HashSet<String>,
    depth: u32,
) {
    if depth > 6 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let is_bundle = p
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("vst3"));
        if is_bundle {
            // Prefer the bundle directory (or file) path for the host loader.
            let path = if p.is_dir() || p.is_file() {
                p.clone()
            } else {
                continue;
            };
            // Skip empty / invalid bundles (no linux binary).
            if path.is_dir() && !bundle_has_linux_binary(&path) {
                continue;
            }
            let key = path.display().to_string();
            if !seen.insert(key.clone()) {
                continue;
            }
            let name = path
                .file_stem()
                .or_else(|| path.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("VST3")
                .to_string();
            out.push(PluginDescriptor {
                id: PluginId {
                    format: PluginFormat::Vst3,
                    id: key.clone(),
                },
                name,
                maker: "VST3".into(),
                path: Some(key),
            });
            continue;
        }
        if p.is_dir() {
            // Vendor folders (e.g. ~/.vst3/Vendor/Plugin.vst3)
            walk_vst3_bundles(&p, out, seen, depth + 1);
        }
    }
}

fn bundle_has_linux_binary(bundle: &Path) -> bool {
    let candidates = [
        bundle.join("Contents/x86_64-linux"),
        bundle.join("Contents/aarch64-linux"),
        bundle.join("Contents/i386-linux"),
    ];
    for dir in candidates {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("so") {
                    return true;
                }
            }
        }
    }
    // Some bundles ship a top-level .so
    if let Ok(rd) = std::fs::read_dir(bundle) {
        for e in rd.flatten() {
            if e.path().extension().and_then(|x| x.to_str()) == Some("so") {
                return true;
            }
        }
    }
    false
}
