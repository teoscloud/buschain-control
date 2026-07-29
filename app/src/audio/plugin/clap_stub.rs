use std::path::{Path, PathBuf};

use super::{PluginBackend, PluginDescriptor, PluginFormat, PluginId};

/// CLAP scanner (filesystem .clap bundles). Instantiation lands in a later pass.
pub struct ClapBackend {
    paths: Vec<String>,
}

impl ClapBackend {
    pub fn new(mut paths: Vec<String>) -> Self {
        if let Ok(env) = std::env::var("CLAP_PATH") {
            for p in env.split(':').filter(|s| !s.is_empty()) {
                paths.push(p.to_string());
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            paths.push(format!("{home}/.clap"));
            paths.push(format!("{home}/.local/lib/clap"));
        }
        paths.push("/usr/lib/clap".into());
        Self { paths }
    }
}

impl PluginBackend for ClapBackend {
    fn format(&self) -> PluginFormat {
        PluginFormat::Clap
    }

    fn scan(&self) -> Vec<PluginDescriptor> {
        let mut plugins = Vec::new();
        for p in &self.paths {
            let path = Path::new(p);
            if !path.is_dir() {
                continue;
            }
            walk_clap(path, &mut plugins);
        }
        plugins
    }
}

fn walk_clap(dir: &Path, out: &mut Vec<PluginDescriptor>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let path = e.path();
        if path.is_dir() {
            walk_clap(&path, out);
        } else if path.extension().and_then(|x| x.to_str()) == Some("clap") {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("clap")
                .to_string();
            out.push(PluginDescriptor {
                id: PluginId {
                    format: PluginFormat::Clap,
                    id: path.display().to_string(),
                },
                name,
                maker: "CLAP".into(),
                path: Some(path_buf_string(&path)),
            });
        }
    }
}

fn path_buf_string(p: &PathBuf) -> String {
    p.display().to_string()
}
