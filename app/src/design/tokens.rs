//! BusChain Control design tokens — console / control-surface chrome.

use egui::{Color32, CornerRadius, Margin, Stroke, Style, Visuals};

pub trait Theme: Send + Sync {
    fn accent(&self) -> Color32;
    fn accent_hover(&self) -> Color32;
    fn accent_dim(&self) -> Color32;
    /// Fader trough fill (hot side) — never the tab/selection accent blue.
    fn fader_fill(&self) -> Color32;
    fn fader_cap(&self) -> Color32;
    fn bg_app(&self) -> Color32;
    fn bg_panel(&self) -> Color32;
    fn bg_elevated(&self) -> Color32;
    fn bg_well(&self) -> Color32;
    fn border(&self) -> Color32;
    fn border_soft(&self) -> Color32;
    fn text(&self) -> Color32;
    fn text_dim(&self) -> Color32;
    fn text_muted(&self) -> Color32;
    fn danger(&self) -> Color32;
    fn warning(&self) -> Color32;
    fn success(&self) -> Color32;
    fn meter_green(&self) -> Color32;
    fn meter_yellow(&self) -> Color32;
    fn meter_orange(&self) -> Color32;
    fn meter_red(&self) -> Color32;
    /// Last-peak hold tick — soft chalk / software gray (theme text family).
    fn meter_peak_hold(&self) -> Color32 {
        self.text_dim()
    }
    fn rounding(&self) -> CornerRadius;
    fn space_xs(&self) -> f32 {
        4.0
    }
    fn space_sm(&self) -> f32 {
        6.0
    }
    fn space_md(&self) -> f32 {
        12.0
    }
    fn apply_egui(&self, ctx: &egui::Context);
}

fn lighten(c: Color32, t: f32) -> Color32 {
    Color32::from_rgb(
        (c.r() as f32 + (255.0 - c.r() as f32) * t).clamp(0.0, 255.0) as u8,
        (c.g() as f32 + (255.0 - c.g() as f32) * t).clamp(0.0, 255.0) as u8,
        (c.b() as f32 + (255.0 - c.b() as f32) * t).clamp(0.0, 255.0) as u8,
    )
}

fn darken(c: Color32, t: f32) -> Color32 {
    Color32::from_rgb(
        (c.r() as f32 * (1.0 - t)).clamp(0.0, 255.0) as u8,
        (c.g() as f32 * (1.0 - t)).clamp(0.0, 255.0) as u8,
        (c.b() as f32 * (1.0 - t)).clamp(0.0, 255.0) as u8,
    )
}

/// Premium dark console — warm charcoal; accent is user-customizable.
#[derive(Clone, Copy)]
pub struct SpectrumTheme {
    pub accent: Color32,
}

impl Default for SpectrumTheme {
    fn default() -> Self {
        Self {
            accent: Color32::from_rgb(0xc9, 0xa2, 0x6b),
        }
    }
}

impl SpectrumTheme {
    pub fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self {
            accent: Color32::from_rgb(r, g, b),
        }
    }

    pub fn from_session_rgb(rgb: [u8; 3]) -> Self {
        Self::from_rgb(rgb[0], rgb[1], rgb[2])
    }
}

impl Theme for SpectrumTheme {
    fn accent(&self) -> Color32 {
        self.accent
    }
    fn accent_hover(&self) -> Color32 {
        lighten(self.accent, 0.22)
    }
    fn accent_dim(&self) -> Color32 {
        darken(self.accent, 0.35)
    }
    fn fader_fill(&self) -> Color32 {
        // Brushed steel trough (cooler gray, not accent-warm).
        Color32::from_rgb(0x9a, 0x9e, 0xa4)
    }
    fn fader_cap(&self) -> Color32 {
        // Light aluminum metallic thumb.
        Color32::from_rgb(0xe8, 0xea, 0xed)
    }
    fn bg_app(&self) -> Color32 {
        Color32::from_rgb(0x18, 0x1a, 0x1d)
    }
    fn bg_panel(&self) -> Color32 {
        Color32::from_rgb(0x22, 0x24, 0x28)
    }
    fn bg_elevated(&self) -> Color32 {
        Color32::from_rgb(0x2a, 0x2d, 0x32)
    }
    fn bg_well(&self) -> Color32 {
        // Plots / insert wells — lifted off pure black for readability
        Color32::from_rgb(0x16, 0x18, 0x1c)
    }
    fn border(&self) -> Color32 {
        Color32::from_rgb(0x3a, 0x3d, 0x42)
    }
    fn border_soft(&self) -> Color32 {
        Color32::from_rgb(0x2a, 0x2c, 0x30)
    }
    fn text(&self) -> Color32 {
        Color32::from_rgb(0xe6, 0xe7, 0xe9)
    }
    fn text_dim(&self) -> Color32 {
        Color32::from_rgb(0xa8, 0xaa, 0xae)
    }
    fn text_muted(&self) -> Color32 {
        Color32::from_rgb(0x6e, 0x71, 0x76)
    }
    fn danger(&self) -> Color32 {
        Color32::from_rgb(0xd4, 0x45, 0x3a)
    }
    fn warning(&self) -> Color32 {
        Color32::from_rgb(0xd4, 0x9a, 0x2a)
    }
    fn success(&self) -> Color32 {
        Color32::from_rgb(0x3c, 0xa8, 0x72)
    }
    fn meter_green(&self) -> Color32 {
        Color32::from_rgb(0x3c, 0xa8, 0x72)
    }
    fn meter_yellow(&self) -> Color32 {
        Color32::from_rgb(0xe6, 0xc2, 0x3a)
    }
    fn meter_orange(&self) -> Color32 {
        // Brick / terracotta — used near 0 dBFS before hard red
        Color32::from_rgb(0xc4, 0x4a, 0x28)
    }
    fn meter_red(&self) -> Color32 {
        Color32::from_rgb(0xe0, 0x2e, 0x28)
    }
    fn meter_peak_hold(&self) -> Color32 {
        // Soft chalk white — sits with text, not neon
        Color32::from_rgb(0xd0, 0xd2, 0xd6)
    }
    fn rounding(&self) -> CornerRadius {
        CornerRadius::same(3)
    }

    fn apply_egui(&self, ctx: &egui::Context) {
        let mut style = Style::default();
        style.spacing.item_spacing = egui::vec2(self.space_sm(), self.space_sm());
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.window_margin = Margin::same(0);
        let r = self.rounding();
        let mut v = Visuals::dark();
        v.dark_mode = true;
        v.override_text_color = Some(self.text());
        v.panel_fill = self.bg_app();
        v.window_fill = self.bg_panel();
        v.extreme_bg_color = self.bg_well();
        v.faint_bg_color = self.bg_elevated();
        v.hyperlink_color = self.accent();
        let a = self.accent();
        v.selection.bg_fill = Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), 48);
        v.selection.stroke = Stroke::new(1.0_f32, self.accent());
        v.window_corner_radius = r;
        v.menu_corner_radius = r;
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = r;
        }
        v.widgets.noninteractive.bg_fill = self.bg_panel();
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, self.border_soft());
        v.widgets.inactive.bg_fill = self.bg_elevated();
        v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, self.border());
        v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, self.accent_hover());
        v.widgets.active.bg_fill = self.accent_dim();
        v.widgets.active.bg_stroke = Stroke::new(1.0_f32, self.accent());
        style.visuals = v;
        ctx.set_style(style);
    }
}
