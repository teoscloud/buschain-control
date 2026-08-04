//! Named platform themes under `~/.config/buschain-control/themes/`.

use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use super::store::{config_dir, slugify, valid_slug};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemePreset {
    pub name: String,
    pub accent_rgb: [u8; 3],
}

#[derive(Debug, Clone)]
pub struct ThemeMeta {
    pub slug: String,
    pub name: String,
    pub path: PathBuf,
}

pub fn themes_dir() -> PathBuf {
    config_dir().join("themes")
}

pub fn theme_path(slug: &str) -> PathBuf {
    themes_dir().join(format!("{slug}.json"))
}

pub fn list_themes() -> Result<Vec<ThemeMeta>> {
    let dir = themes_dir();
    fs::create_dir_all(&dir)?;
    let mut out = Vec::new();
    for ent in fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let ent = ent?;
        let path = ent.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let slug = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("theme")
            .to_string();
        let raw = fs::read_to_string(&path).unwrap_or_default();
        let name = serde_json::from_str::<ThemePreset>(&raw)
            .map(|t| t.name)
            .unwrap_or_else(|_| slug.clone());
        out.push(ThemeMeta { slug, name, path });
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

pub fn save_theme(name: &str, accent_rgb: [u8; 3]) -> Result<ThemeMeta> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow!("theme name required"));
    }
    let slug = slugify(name);
    let dir = themes_dir();
    fs::create_dir_all(&dir)?;
    let path = theme_path(&slug);
    let preset = ThemePreset {
        name: name.to_string(),
        accent_rgb,
    };
    let json = serde_json::to_string_pretty(&preset)?;
    fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(ThemeMeta {
        slug,
        name: preset.name,
        path,
    })
}

pub fn load_theme(slug: &str) -> Result<ThemePreset> {
    let slug = slug.trim();
    if !valid_slug(slug) {
        return Err(anyhow!("invalid theme slug: {slug}"));
    }
    let path = theme_path(slug);
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

pub fn delete_theme(slug: &str) -> Result<()> {
    let slug = slug.trim();
    if !valid_slug(slug) {
        return Err(anyhow!("invalid theme slug: {slug}"));
    }
    let path = theme_path(slug);
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}
