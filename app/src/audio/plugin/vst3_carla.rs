//! Optional discardable VST3 backend via Carla — VST3 ONLY.
//! Not compiled unless `--features vst3-carla`. Core must never import this unconditionally.

use std::process::Command;

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
        // Discovery only — never used for routing/mixer. Replaceable later with native VST3.
        let mut plugins = Vec::new();
        if Command::new("carla").arg("--version").output().is_err() {
            return plugins;
        }
        // Carla Discovery can be slow; keep lightweight path scan for *.vst3
        let home = std::env::var("HOME").unwrap_or_default();
        for root in [
            format!("{home}/.vst3"),
            format!("{home}/.local/lib/vst3"),
            "/usr/lib/vst3".into(),
        ] {
            let path = std::path::Path::new(&root);
            if !path.is_dir() {
                continue;
            }
            let Ok(rd) = std::fs::read_dir(path) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("vst3") || p.is_dir() {
                    let name = p
                        .file_stem()
                        .or_else(|| p.file_name())
                        .and_then(|s| s.to_str())
                        .unwrap_or("vst3")
                        .to_string();
                    plugins.push(PluginDescriptor {
                        id: PluginId {
                            format: PluginFormat::Vst3,
                            id: p.display().to_string(),
                        },
                        name,
                        maker: "VST3 (Carla adapter)".into(),
                        path: Some(p.display().to_string()),
                    });
                }
            }
        }
        plugins
    }
}
