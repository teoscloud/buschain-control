use std::path::{Path, PathBuf};
use std::process::Command;

use super::{PluginBackend, PluginDescriptor, PluginFormat, PluginId};

pub struct LadspaBackend {
    paths: Vec<String>,
}

impl LadspaBackend {
    pub fn new(mut paths: Vec<String>) -> Self {
        if let Ok(env) = std::env::var("LADSPA_PATH") {
            for p in env.split(':').filter(|s| !s.is_empty()) {
                paths.push(p.to_string());
            }
        }
        // Builtins relative to cwd
        for rel in [
            "plugins/buschain-denoiser/build",
            "plugins/buschain-gate/build",
            "plugins/buschain-builtins/build",
        ] {
            paths.push(rel.into());
        }
        Self { paths }
    }

    fn list_sos(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for p in &self.paths {
            let path = Path::new(p);
            if path.is_dir() {
                if let Ok(rd) = std::fs::read_dir(path) {
                    for e in rd.flatten() {
                        let f = e.path();
                        if f.extension().and_then(|x| x.to_str()) == Some("so") {
                            out.push(f);
                        }
                    }
                }
            } else if path.extension().and_then(|x| x.to_str()) == Some("so") {
                out.push(path.to_path_buf());
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

impl PluginBackend for LadspaBackend {
    fn format(&self) -> PluginFormat {
        PluginFormat::Ladspa
    }

    fn scan(&self) -> Vec<PluginDescriptor> {
        let mut plugins = Vec::new();
        // Prefer listplugins if available
        if let Ok(out) = Command::new("listplugins").output() {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                for line in text.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    // Typical: "buschain_denoiser (392001/0)" or similar — keep whole line as id
                    plugins.push(PluginDescriptor {
                        id: PluginId {
                            format: PluginFormat::Ladspa,
                            id: line.to_string(),
                        },
                        name: line.to_string(),
                        maker: "LADSPA".into(),
                        path: None,
                    });
                }
            }
        }

        // Always add known BusChain labels from discovered .so basenames
        for so in self.list_sos() {
            let stem = so
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            if plugins.iter().any(|p| p.id.id.contains(&stem)) {
                continue;
            }
            plugins.push(PluginDescriptor {
                id: PluginId {
                    format: PluginFormat::Ladspa,
                    id: stem.clone(),
                },
                name: stem.replace('_', " "),
                maker: "BusChain Control".into(),
                path: Some(so.display().to_string()),
            });
        }

        // Guarantee builtins show even before build
        for (id, name) in [
            ("buschain_denoiser", "BusChain Denoiser"),
            ("buschain_gate", "BusChain Gate"),
            ("buschain_eq8", "Parametric EQ"),
            ("buschain_eq", "BusChain EQ 1-Band"),
            ("buschain_compressor", "BusChain Compressor"),
            ("buschain_limiter", "BusChain Limiter"),
            ("buschain_softclip", "Soft Clipper"),
            ("buschain_overdrive", "Theatre Drive"),
            ("buschain_pitch", "BusChain Pitch"),
        ] {
            if !plugins.iter().any(|p| p.id.id == id || p.id.id.contains(id)) {
                plugins.push(PluginDescriptor {
                    id: PluginId {
                        format: PluginFormat::Ladspa,
                        id: id.into(),
                    },
                    name: name.into(),
                    maker: "BusChain Control".into(),
                    path: None,
                });
            }
        }
        plugins
    }
}
