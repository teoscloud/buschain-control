//! App icon bytes shared by the egui window and the StatusNotifier tray.

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use egui::IconData;
use image::imageops::FilterType;
use image::RgbaImage;

const APP_ICON_PNG: &[u8] = include_bytes!("../../assets/icons/buschain-control.png");
const TRAY_SIZES: [u32; 3] = [22, 32, 48];

pub fn window_icon() -> Option<IconData> {
    let img = decode_rgba()?;
    let (w, h) = img.dimensions();
    Some(IconData {
        rgba: img.into_raw(),
        width: w,
        height: h,
    })
}

pub fn tray_icon_name() -> String {
    "buschain-control".into()
}

/// Extra icon-theme root so hosts that ignore pixmaps can still resolve the name.
pub fn tray_icon_theme_path() -> String {
    ensure_tray_theme_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn tray_icon_pixmap() -> Vec<ksni::Icon> {
    let _ = ensure_tray_theme_dir();
    static ICONS: OnceLock<Vec<ksni::Icon>> = OnceLock::new();
    ICONS.get_or_init(build_tray_pixmaps).clone()
}

fn decode_rgba() -> Option<RgbaImage> {
    image::load_from_memory(APP_ICON_PNG)
        .ok()
        .map(|img| img.into_rgba8())
}

fn scaled(size: u32) -> Option<RgbaImage> {
    let img = decode_rgba()?;
    Some(image::imageops::resize(
        &img,
        size,
        size,
        FilterType::Lanczos3,
    ))
}

fn rgba_to_argb32_be(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
    }
    out
}

fn build_tray_pixmaps() -> Vec<ksni::Icon> {
    TRAY_SIZES
        .into_iter()
        .filter_map(|size| {
            let img = scaled(size)?;
            Some(ksni::Icon {
                width: size as i32,
                height: size as i32,
                data: rgba_to_argb32_be(img.as_raw()),
            })
        })
        .collect()
}

fn ensure_tray_theme_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        install_hicolor_icons(dirs::data_dir().map(|p| p.join("icons")));

        let root = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let theme_root = root.join("buschain-control/icons");
        install_hicolor_icons(Some(theme_root.clone()));
        let named = theme_root.join("buschain-control.png");
        if !named.is_file() {
            if let Some(img) = scaled(32) {
                let _ = img.save(&named);
            }
        }
        Some(theme_root)
    })
    .clone()
}

fn install_hicolor_icons(icons_root: Option<PathBuf>) {
    let Some(icons_root) = icons_root else {
        return;
    };
    for size in TRAY_SIZES {
        let apps = icons_root
            .join("hicolor")
            .join(format!("{size}x{size}"))
            .join("apps");
        if fs::create_dir_all(&apps).is_err() {
            continue;
        }
        let dest = apps.join("buschain-control.png");
        if dest.is_file() {
            continue;
        }
        if let Some(img) = scaled(size) {
            let _ = img.save(dest);
        }
    }
}
