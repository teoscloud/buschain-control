//! Resolve Freedesktop / theme app icons for the apps-on-track UI.

use std::path::{Path, PathBuf};

use egui::{ColorImage, TextureHandle, TextureOptions, Vec2};

use crate::app_state::AppState;

const ICON_PX: u16 = 24;
const DISPLAY_SIZE: f32 = 18.0;

/// Draw a small app icon if resolvable; returns whether something was drawn.
pub fn draw_app_icon(ui: &mut egui::Ui, state: &mut AppState, icon_name: Option<&str>) -> bool {
    let Some(name) = icon_name.filter(|s| !s.is_empty()) else {
        return false;
    };
    let tex = match ensure_icon(ui.ctx(), state, name) {
        Some(t) => t,
        None => return false,
    };
    ui.add(
        egui::Image::new(&tex)
            .fit_to_exact_size(Vec2::splat(DISPLAY_SIZE))
            .corner_radius(egui::CornerRadius::ZERO),
    );
    true
}

fn ensure_icon(
    ctx: &egui::Context,
    state: &mut AppState,
    icon_name: &str,
) -> Option<TextureHandle> {
    if let Some(t) = state.app_icon_cache.get(icon_name) {
        return Some(t.clone());
    }
    let path = resolve_icon_path(icon_name)?;
    let rgba = image::open(&path).ok()?.into_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let color = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
    let tex = ctx.load_texture(
        format!("shadow-app-icon:{icon_name}"),
        color,
        TextureOptions::LINEAR,
    );
    state.app_icon_cache.insert(icon_name.to_string(), tex.clone());
    Some(tex)
}

fn resolve_icon_path(icon_name: &str) -> Option<PathBuf> {
    let as_path = Path::new(icon_name);
    if as_path.is_absolute() && as_path.exists() {
        return Some(as_path.to_path_buf());
    }

    let sizes = [
        format!("{ICON_PX}x{ICON_PX}"),
        "22x22".into(),
        "32x32".into(),
        "16x16".into(),
        "48x48".into(),
        "64x64".into(),
    ];
    let themes = ["hicolor", "Adwaita", "WhiteSur", "breeze", "Papirus"];
    let categories = ["apps", "applications"];

    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(xdg) = std::env::var("XDG_DATA_DIRS") {
        for part in xdg.split(':').filter(|s| !s.is_empty()) {
            roots.push(PathBuf::from(part).join("icons"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        roots.push(PathBuf::from(home).join(".local/share/icons"));
    }
    roots.push(PathBuf::from("/run/current-system/sw/share/icons"));
    roots.push(PathBuf::from("/usr/share/icons"));

    let candidates = [
        icon_name.to_string(),
        icon_name.replace('_', "-"),
        icon_name.to_lowercase(),
    ];

    for root in &roots {
        if !root.exists() {
            continue;
        }
        for theme in themes {
            for size in &sizes {
                for cat in categories {
                    for name in &candidates {
                        let p = root
                            .join(theme)
                            .join(size)
                            .join(cat)
                            .join(format!("{name}.png"));
                        if p.is_file() {
                            return Some(p);
                        }
                    }
                }
            }
        }
    }
    None
}
