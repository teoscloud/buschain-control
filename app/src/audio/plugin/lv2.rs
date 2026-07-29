use std::path::{Path, PathBuf};
use std::process::Command;

use super::{PluginBackend, PluginDescriptor, PluginFormat, PluginId};

pub struct Lv2Backend {
    paths: Vec<String>,
}

impl Lv2Backend {
    pub fn new(mut paths: Vec<String>) -> Self {
        if let Ok(env) = std::env::var("LV2_PATH") {
            for p in env.split(':').filter(|s| !s.is_empty()) {
                paths.push(p.to_string());
            }
        }
        for rel in [
            "plugins/buschain-denoiser/build",
            "plugins/buschain-gate/build",
            "plugins/buschain-builtins/build",
            "/usr/lib/lv2",
            "/usr/local/lib/lv2",
        ] {
            paths.push(rel.into());
        }
        if let Ok(home) = std::env::var("HOME") {
            paths.push(format!("{home}/.lv2"));
            paths.push(format!("{home}/.local/lib/lv2"));
        }
        Self { paths }
    }

    fn bundles(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for p in &self.paths {
            let path = Path::new(p);
            if !path.is_dir() {
                continue;
            }
            // path itself may be a bundle
            if path.join("manifest.ttl").exists() {
                out.push(path.to_path_buf());
            }
            if let Ok(rd) = std::fs::read_dir(path) {
                for e in rd.flatten() {
                    let d = e.path();
                    if d.is_dir() && d.join("manifest.ttl").exists() {
                        out.push(d.clone());
                    }
                    // nested build/foo.lv2
                    if d.is_dir() {
                        if let Ok(rd2) = std::fs::read_dir(&d) {
                            for e2 in rd2.flatten() {
                                let d2 = e2.path();
                                if d2.is_dir() && d2.join("manifest.ttl").exists() {
                                    out.push(d2);
                                }
                            }
                        }
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

impl PluginBackend for Lv2Backend {
    fn format(&self) -> PluginFormat {
        PluginFormat::Lv2
    }

    fn scan(&self) -> Vec<PluginDescriptor> {
        let mut plugins = Vec::new();
        if let Ok(out) = Command::new("lv2ls").output() {
            if out.status.success() {
                for uri in String::from_utf8_lossy(&out.stdout).lines() {
                    let uri = uri.trim();
                    if uri.is_empty() {
                        continue;
                    }
                    plugins.push(PluginDescriptor {
                        id: PluginId {
                            format: PluginFormat::Lv2,
                            id: uri.to_string(),
                        },
                        name: uri.rsplit(['#', '/']).next().unwrap_or(uri).to_string(),
                        maker: "LV2".into(),
                        path: None,
                    });
                }
            }
        }
        for bundle in self.bundles() {
            let name = bundle
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("lv2")
                .to_string();
            let id = format!("bundle:{}", bundle.display());
            if plugins.iter().any(|p| p.name == name) {
                continue;
            }
            plugins.push(PluginDescriptor {
                id: PluginId {
                    format: PluginFormat::Lv2,
                    id,
                },
                name,
                maker: "LV2".into(),
                path: Some(bundle.display().to_string()),
            });
        }
        plugins
    }
}
