use egui::{
    Align, Color32, CornerRadius, Frame, Layout, Rect, RichText, Sense, Stroke, Ui, Vec2,
};

use super::tokens::Theme;

pub fn panel(ui: &mut Ui, theme: &dyn Theme, add: impl FnOnce(&mut Ui)) {
    Frame::NONE
        .fill(theme.bg_panel())
        .stroke(Stroke::new(1.0_f32, theme.border_soft()))
        .corner_radius(theme.rounding())
        .inner_margin(egui::Margin::same(theme.space_md() as i8))
        .show(ui, add);
}

pub fn button(ui: &mut Ui, theme: &dyn Theme, label: &str, primary: bool) -> egui::Response {
    let fill = if primary { theme.accent() } else { theme.bg_elevated() };
    let text = if primary { Color32::WHITE } else { theme.text() };
    let stroke = Stroke::new(
        1.0_f32,
        if primary { theme.accent_dim() } else { theme.border() },
    );
    ui.add(
        egui::Button::new(RichText::new(label).size(12.0).color(text))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(theme.rounding()),
    )
}

/// Hide the OS cursor while a drag interaction is held (focus on the control).
pub fn hide_cursor_on_drag(ui: &Ui, resp: &egui::Response) {
    if resp.dragged() || (resp.is_pointer_button_down_on() && resp.hovered()) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
    }
}

/// Hide the OS cursor while `active` (orbit / fine drag).
///
/// Intentionally avoids `CursorGrab::Locked` — on Wayland that clears the
/// pointer position and egui aborts the drag on the next frame.
pub fn capture_cursor_while(ui: &Ui, active: bool, _state_id: egui::Id) {
    if active {
        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        ui.ctx().request_repaint();
    }
}

/// DAW-style fine control: Shift = 0.1×, Alt = 0.05×. Ctrl reserved for shortcuts.
pub fn drag_sensitivity(ui: &Ui) -> f32 {
    ui.input(|i| {
        if i.modifiers.alt {
            0.05
        } else if i.modifiers.shift {
            0.1
        } else {
            1.0
        }
    })
}

/// Mouse-wheel adjust for hovered knobs / faders / sliders.
/// Consumes scroll so parent `ScrollArea`s don't also move. Shift/Alt fine-tune.
/// Wide positive ranges (e.g. Hz) step in log space.
pub fn apply_wheel_to_value(
    ui: &mut Ui,
    resp: &mut egui::Response,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    if !resp.hovered() {
        return false;
    }
    let (raw_y, smooth_y) = ui.input(|i| (i.raw_scroll_delta.y, i.smooth_scroll_delta.y));
    let dy = if raw_y.abs() > 0.0 { raw_y } else { smooth_y };
    if dy.abs() < 0.01 {
        return false;
    }
    // Stop the channel-rack / plugin scroll from eating the gesture.
    ui.ctx().input_mut(|i| {
        i.smooth_scroll_delta = Vec2::ZERO;
    });

    let lo = *range.start();
    let hi = *range.end();
    let span = (hi - lo).max(1e-9);
    let sens = drag_sensitivity(ui);
    // ~14 px raw ≈ one physical notch on many mice.
    let steps = if raw_y.abs() > 0.0 {
        (raw_y / 14.0).clamp(-4.0, 4.0)
    } else {
        (smooth_y / 48.0).clamp(-2.5, 2.5)
    };
    if steps.abs() < 1e-4 {
        return false;
    }

    let logarithmic = lo > 0.0 && hi / lo >= 16.0;
    if logarithmic {
        let v = (*value).clamp(lo, hi).max(lo);
        let t = (v.ln() - lo.ln()) / (hi.ln() - lo.ln());
        let nt = (t + steps * 0.03 * sens).clamp(0.0, 1.0);
        *value = (lo.ln() + nt * (hi.ln() - lo.ln())).exp();
    } else {
        *value = (*value + steps * span * 0.03 * sens).clamp(lo, hi);
    }
    resp.mark_changed();
    true
}

/// Compact icon button (Phosphor glyph). Always shows a real icon — never an empty square.
pub fn icon_button(
    ui: &mut Ui,
    theme: &dyn Theme,
    icon: &str,
    enabled: bool,
) -> egui::Response {
    let fill = if enabled {
        theme.bg_elevated()
    } else {
        theme.bg_well()
    };
    let text = if enabled {
        theme.text_dim()
    } else {
        theme.text_muted()
    };
    let stroke = Stroke::new(1.0_f32, theme.border_soft());
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(icon).size(14.0).color(text))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(theme.rounding())
            .min_size(Vec2::new(26.0, 24.0)),
    )
}

/// Compact text control (↑ ↓ ×) using the default UI font — never a tofu square.
pub fn text_tool_button(ui: &mut Ui, theme: &dyn Theme, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(label)
                .size(13.0)
                .strong()
                .color(theme.text_dim()),
        )
        .fill(theme.bg_elevated())
        .stroke(Stroke::new(1.0_f32, theme.border_soft()))
        .corner_radius(theme.rounding())
        .min_size(Vec2::new(26.0, 24.0)),
    )
}

/// Green/red LED tone matched to the selection accent's value + saturation.
fn accent_matched_led(accent: Color32, green: bool) -> Color32 {
    let ar = accent.r() as f32 / 255.0;
    let ag = accent.g() as f32 / 255.0;
    let ab = accent.b() as f32 / 255.0;
    let amax = ar.max(ag).max(ab).max(1e-3_f32);
    let amin = ar.min(ag).min(ab);
    let sat = ((amax - amin) / amax).clamp(0.15_f32, 0.85_f32);
    let val = amax.clamp(0.35_f32, 0.92_f32);

    // Unit green / red hue directions (console, not neon).
    let (hr, hg, hb) = if green {
        (0.38_f32, 0.78_f32, 0.52_f32)
    } else {
        (0.82_f32, 0.36_f32, 0.32_f32)
    };
    let hmax = hr.max(hg).max(hb);
    let nr = hr / hmax;
    let ng = hg / hmax;
    let nb = hb / hmax;

    let mix = |n: f32| ((1.0 - sat) * val + sat * n * val).clamp(0.0, 1.0);
    Color32::from_rgb(
        (mix(nr) * 255.0) as u8,
        (mix(ng) * 255.0) as u8,
        (mix(nb) * 255.0) as u8,
    )
}

/// Analog LED jewel — green when lit, red when dark. No bezel / border / text.
fn analog_led_jewel(
    ui: &mut Ui,
    theme: &dyn Theme,
    on: &mut bool,
    size: Vec2,
    hover_on: &str,
    hover_off: &str,
) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    let lit = *on;
    let led = accent_matched_led(theme.accent(), lit);

    let r = (rect.width().min(rect.height()) * 0.36).clamp(5.0, 8.0);
    let c = rect.center();
    // Tight outer glow (kept smaller than the jewel face)
    painter.circle_filled(
        c,
        r * 1.22,
        Color32::from_rgba_unmultiplied(led.r(), led.g(), led.b(), if lit { 36 } else { 24 }),
    );
    painter.circle_filled(c, r, led);
    painter.circle_filled(
        egui::pos2(c.x - r * 0.28, c.y - r * 0.32),
        r * 0.30,
        Color32::from_rgba_unmultiplied(255, 255, 255, if lit { 90 } else { 40 }),
    );

    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.on_hover_text(if lit { hover_on } else { hover_off })
}

/// Power / enable toggle for inserts. `on == true` means the plugin is active (not bypassed).
pub fn power_toggle(
    ui: &mut Ui,
    theme: &dyn Theme,
    on: &mut bool,
) -> egui::Response {
    analog_led_jewel(
        ui,
        theme,
        on,
        Vec2::new(28.0, 24.0),
        "Disable insert",
        "Enable insert",
    )
}

pub fn toggle_chip(
    ui: &mut Ui,
    theme: &dyn Theme,
    label: &str,
    on: &mut bool,
    active: Color32,
) -> egui::Response {
    let fill = if *on { active } else { theme.bg_well() };
    let text = if *on { Color32::WHITE } else { theme.text_dim() };
    let stroke = Stroke::new(1.0_f32, if *on { active } else { theme.border() });
    let mut resp = ui.add(
        egui::Button::new(RichText::new(label).size(11.0).strong().color(text))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(theme.rounding())
            .min_size(Vec2::new(22.0, 22.0)),
    );
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp
}

/// Console Mute / Solo — taller pad, inset bezel, lit when engaged.
pub fn mixer_pad(
    ui: &mut Ui,
    theme: &dyn Theme,
    label: &str,
    on: &mut bool,
    lit: Color32,
) -> egui::Response {
    mixer_pad_sized(ui, theme, label, on, lit, Vec2::new(32.0, 26.0), true)
}

/// Same footprint as `mixer_pad`, non-interactive (Master solo slot, etc.).
pub fn mixer_pad_inert(ui: &mut Ui, theme: &dyn Theme, label: &str) -> egui::Response {
    let mut off = false;
    mixer_pad_sized(
        ui,
        theme,
        label,
        &mut off,
        theme.border(),
        Vec2::new(32.0, 26.0),
        false,
    )
}

/// Track live LED — green when audible (`on`), red when muted. Just the light.
pub fn track_on_led(ui: &mut Ui, theme: &dyn Theme, on: &mut bool) -> egui::Response {
    analog_led_jewel(
        ui,
        theme,
        on,
        Vec2::new(34.0, 28.0),
        "Mute track output",
        "Unmute track",
    )
}

fn mixer_pad_sized(
    ui: &mut Ui,
    theme: &dyn Theme,
    label: &str,
    on: &mut bool,
    lit: Color32,
    size: Vec2,
    interactive: bool,
) -> egui::Response {
    let sense = if interactive {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, mut resp) = ui.allocate_exact_size(size, sense);
    let painter = ui.painter();

    // Outer bezel
    painter.rect_filled(rect, CornerRadius::same(4), theme.bg_well());
    painter.rect_stroke(
        rect,
        CornerRadius::same(4),
        Stroke::new(1.0_f32, theme.border()),
        egui::StrokeKind::Outside,
    );

    let inner = rect.shrink(2.0);
    let fill = if *on && interactive {
        lit.gamma_multiply(0.75)
    } else if interactive {
        theme.bg_elevated()
    } else {
        theme.bg_well()
    };
    painter.rect_filled(inner, CornerRadius::same(3), fill);
    if *on && interactive {
        painter.rect_stroke(
            inner,
            CornerRadius::same(3),
            Stroke::new(1.0_f32, lit),
            egui::StrokeKind::Inside,
        );
    }

    // Status LED
    let led_c = if !interactive {
        theme.border_soft()
    } else if *on {
        lit
    } else {
        Color32::from_rgb(0x2a, 0x2c, 0x30)
    };
    painter.circle_filled(egui::pos2(inner.right() - 5.0, inner.top() + 5.0), 2.2, led_c);
    if *on && interactive {
        painter.circle_filled(
            egui::pos2(inner.right() - 5.0, inner.top() + 5.0),
            1.0,
            Color32::WHITE,
        );
    }

    painter.text(
        inner.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(12.0),
        if *on && interactive {
            Color32::WHITE
        } else if interactive {
            theme.text_dim()
        } else {
            theme.text_muted()
        },
    );

    if interactive && resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp
}

/// Small console util button (reorder / remove) — Phosphor icon glyph.
pub fn mixer_util_btn(
    ui: &mut Ui,
    theme: &dyn Theme,
    icon: &str,
    enabled: bool,
) -> egui::Response {
    icon_button(ui, theme, icon, enabled)
}

pub fn chip(ui: &mut Ui, theme: &dyn Theme, label: &str, filled: bool) {
    let fill = if filled {
        Color32::from_rgb(0x26, 0x3b, 0x53)
    } else {
        theme.bg_well()
    };
    let stroke = Stroke::new(
        1.0_f32,
        if filled {
            theme.accent().gamma_multiply(0.45)
        } else {
            theme.border_soft()
        },
    );
    let color = if filled {
        Color32::from_rgb(0xb3, 0xd0, 0xf5)
    } else {
        theme.text_muted()
    };
    Frame::NONE
        .fill(fill)
        .stroke(stroke)
        .corner_radius(theme.rounding())
        .inner_margin(egui::Margin::symmetric(4, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(9.0).color(color));
        });
}

/// Mixer strip fader + meter share this range so 0 dB lines align.
pub const STRIP_DB_MIN: f32 = -48.0;
pub const STRIP_DB_MAX: f32 = 12.0;

/// Custom vertical fader (fixed rect — avoids egui Slider layout blowups).
pub fn fader_db(
    ui: &mut Ui,
    theme: &dyn Theme,
    value_db: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    size: Vec2,
) -> egui::Response {
    // Focusable drag target — steals pointer from parent strip widgets.
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    if resp.dragged() || (resp.is_pointer_button_down_on() && resp.hovered()) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
    } else if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let painter = ui.painter();

    let lo = *range.start();
    let hi = *range.end();
    let span = (hi - lo).max(0.001);
    let db_to_y = |db: f32| {
        let t = ((db - lo) / span).clamp(0.0, 1.0);
        // Track inset matches fill geometry below
        let top = rect.top() + 4.0;
        let bot = rect.bottom() - 4.0;
        bot - t * (bot - top)
    };

    // Traditional console fader: narrow steel track, light metallic throw.
    // Compact when the hit rect is tray-popup narrow (caps must not dominate).
    let compact = size.x < 36.0;
    let track_w = if compact {
        (size.x * 0.18).clamp(3.0, 5.0)
    } else {
        (size.x * 0.22).clamp(4.0, 7.0)
    };
    let track = Rect::from_center_size(
        rect.center(),
        Vec2::new(track_w, rect.height() - 8.0),
    );
    painter.rect_filled(track, CornerRadius::same(2), Color32::from_rgb(0x1a, 0x1b, 0x1d));
    painter.rect_stroke(
        track,
        CornerRadius::same(2),
        Stroke::new(1.0_f32, Color32::from_rgb(0x8a, 0x8e, 0x94)),
        egui::StrokeKind::Outside,
    );

    // Scale ticks (behind fill/cap) — 0 dB is the major rail.
    // Skip side ticks in compact popup strips (they skew visual centering).
    if !compact && lo < 0.0 && hi > 0.0 && track.height() > 40.0 {
        let marks: &[(f32, bool)] = if span >= 36.0 {
            &[
                (12.0, false),
                (6.0, false),
                (0.0, true),
                (-6.0, false),
                (-12.0, false),
                (-24.0, false),
                (-36.0, false),
            ]
        } else {
            &[(6.0, false), (0.0, true), (-6.0, false), (-12.0, false)]
        };
        for &(db, is_zero) in marks {
            if db < lo || db > hi {
                continue;
            }
            let y = db_to_y(db);
            if is_zero {
                // Full-width unity rail — easy to see on tall strips
                painter.hline(
                    egui::Rangef::new(rect.left() + 1.0, rect.right() - 1.0),
                    y,
                    Stroke::new(2.0_f32, theme.accent()),
                );
                if rect.width() >= 40.0 {
                    painter.text(
                        egui::pos2(rect.left() + 1.0, y),
                        egui::Align2::LEFT_CENTER,
                        "0",
                        egui::FontId::proportional(9.0),
                        theme.accent(),
                    );
                }
            } else {
                let tick_w = if db.abs() >= 24.0 { 3.0 } else { 4.5 };
                painter.hline(
                    egui::Rangef::new(track.left() - tick_w, track.left() - 1.0),
                    y,
                    Stroke::new(1.0_f32, theme.border()),
                );
                painter.hline(
                    egui::Rangef::new(track.right() + 1.0, track.right() + tick_w),
                    y,
                    Stroke::new(1.0_f32, theme.border()),
                );
            }
        }
    }

    let t = ((*value_db - lo) / span).clamp(0.0, 1.0);
    // 1.0 at top (hot), 0.0 at bottom (cold) — inverted like a mixer fader
    let y = track.bottom() - t * track.height();

    // Fill from cap to bottom — steel trough, not accent blue
    let fill = Rect::from_min_max(
        egui::pos2(track.left(), y),
        egui::pos2(track.right(), track.bottom()),
    );
    painter.rect_filled(fill, CornerRadius::same(2), theme.fader_fill());

    // Skeuomorphic console fader cap — tall rectangle, bevel + grip grooves.
    let at_zero = value_db.abs() < 0.05;
    let above_zero = *value_db > 0.05;
    let (cap_w, cap_h) = if compact {
        let w = (size.x * 0.50).clamp(9.0, 12.0);
        let h = (w * 1.35).clamp(12.0, 16.0);
        (w, h)
    } else {
        let w = (size.x * 0.82).clamp(15.0, 20.0);
        let h = (w * 1.85).clamp(28.0, 38.0);
        (w, h)
    };
    let cap = Rect::from_center_size(egui::pos2(track.center().x, y), Vec2::new(cap_w, cap_h));
    let r = CornerRadius::same(if compact { 1 } else { 2 });

    // Soft drop shadow (bottom-right)
    let shadow = cap.translate(egui::vec2(1.5, 2.0));
    painter.rect_filled(
        shadow,
        r,
        Color32::from_rgba_unmultiplied(0, 0, 0, 70),
    );

    let body = if above_zero {
        Color32::from_rgb(0xd4, 0xb4, 0x78)
    } else if at_zero {
        Color32::from_rgb(0xf0, 0xf1, 0xf3)
    } else {
        Color32::from_rgb(0xe4, 0xe6, 0xea) // pale aluminum
    };
    painter.rect_filled(cap, r, body);

    // Bevel: top/left highlight, bottom/right shade
    let bevel_hi =
        Color32::from_rgba_unmultiplied(255, 255, 255, if above_zero { 70 } else { 110 });
    let bevel_lo =
        Color32::from_rgba_unmultiplied(40, 42, 48, if above_zero { 90 } else { 80 });
    painter.hline(
        egui::Rangef::new(cap.left() + 1.5, cap.right() - 1.5),
        cap.top() + 1.0,
        Stroke::new(1.25_f32, bevel_hi),
    );
    painter.vline(
        cap.left() + 1.0,
        egui::Rangef::new(cap.top() + 1.5, cap.bottom() - 1.5),
        Stroke::new(1.0_f32, bevel_hi.gamma_multiply(0.85)),
    );
    painter.hline(
        egui::Rangef::new(cap.left() + 1.5, cap.right() - 1.5),
        cap.bottom() - 1.0,
        Stroke::new(1.25_f32, bevel_lo),
    );
    painter.vline(
        cap.right() - 1.0,
        egui::Rangef::new(cap.top() + 1.5, cap.bottom() - 1.5),
        Stroke::new(1.0_f32, bevel_lo),
    );

    // Grip grooves — fewer on compact popup caps.
    let grip = Color32::from_rgb(0x5a, 0x5e, 0x64);
    let grip_hi = Color32::from_rgba_unmultiplied(255, 255, 255, 45);
    let cy = cap.center().y;
    let grip_span = if compact {
        (cap_h * 0.22).clamp(3.0, 5.0)
    } else {
        (cap_h * 0.28).clamp(5.5, 9.0)
    };
    let groove_range = if compact { -1..=1 } else { -2..=2 };
    let inset = if compact { 2.0 } else { 3.0 };
    for i in groove_range {
        let dy = i as f32 * (grip_span * 0.5);
        let gy = cy + dy;
        painter.hline(
            egui::Rangef::new(cap.left() + inset, cap.right() - inset),
            gy,
            Stroke::new(if compact { 1.0_f32 } else { 1.35_f32 }, grip),
        );
        if !compact {
            painter.hline(
                egui::Rangef::new(cap.left() + inset, cap.right() - inset),
                gy + 1.0,
                Stroke::new(1.0_f32, grip_hi),
            );
        }
    }

    painter.rect_stroke(
        cap,
        r,
        Stroke::new(
            1.0_f32,
            if at_zero {
                theme.accent().gamma_multiply(0.85)
            } else if above_zero {
                theme.warning().gamma_multiply(0.75)
            } else {
                Color32::from_rgb(0x6e, 0x72, 0x78)
            },
        ),
        egui::StrokeKind::Outside,
    );

    // Redraw 0 dB rail on top of fill so it stays visible under the trough.
    // Compact: tick only across the track (full-width rail skews popup strips).
    if lo < 0.0 && hi > 0.0 {
        let y0 = db_to_y(0.0);
        let x0 = if compact {
            track.left() - 1.0
        } else {
            rect.left() + 1.0
        };
        let x1 = if compact {
            track.right() + 1.0
        } else {
            rect.right() - 1.0
        };
        painter.hline(
            egui::Rangef::new(x0, x1),
            y0,
            Stroke::new(
                if compact { 1.25_f32 } else { 1.75_f32 },
                theme.accent().gamma_multiply(0.95),
            ),
        );
    }

    let snap_zero = |v: &mut f32| {
        if v.abs() < 0.75 {
            *v = 0.0;
        }
    };

    // Absolute Y→dB while held (follows cursor). Shift/Alt = fine relative drag.
    let sens = drag_sensitivity(ui);
    let holding = resp.dragged() || (resp.is_pointer_button_down_on() && resp.contains_pointer());
    if holding {
        if sens < 0.99 {
            *value_db = (*value_db
                - resp.drag_delta().y * (span / track.height().max(1.0)) * sens)
                .clamp(lo, hi);
        } else if let Some(pos) = ui.ctx().pointer_interact_pos() {
            let nt = ((track.bottom() - pos.y) / track.height().max(1.0)).clamp(0.0, 1.0);
            *value_db = lo + nt * span;
        }
        snap_zero(value_db);
        resp.mark_changed();
    } else if resp.clicked() {
        if let Some(pos) = ui.ctx().pointer_interact_pos() {
            let nt = ((track.bottom() - pos.y) / track.height().max(1.0)).clamp(0.0, 1.0);
            *value_db = lo + nt * span;
            snap_zero(value_db);
            resp.mark_changed();
        }
    }
    if resp.double_clicked() {
        *value_db = 0.0;
        resp.mark_changed();
    }
    if apply_wheel_to_value(ui, &mut resp, value_db, lo..=hi) {
        snap_zero(value_db);
    }
    resp
}

/// Live peak meter in dBFS. Same dB span as strip faders so 0 dB lines align.
/// Pass −90 or lower when silent / unknown.
/// `id_salt` keeps the last-peak hold line stable per strip across frames.
pub fn meter(ui: &mut Ui, theme: &dyn Theme, level_db: f32, size: Vec2, id_salt: impl std::hash::Hash) {
    meter_range(
        ui,
        theme,
        level_db,
        size,
        STRIP_DB_MIN,
        STRIP_DB_MAX,
        id_salt,
    );
}

#[derive(Clone, Copy)]
struct MeterPeakHold {
    db: f32,
    /// Hold until this `ctx.input.time` (seconds).
    held_until: f64,
    last_t: f64,
}

/// Peak meter with an explicit dB window (must match sibling fader if paired).
pub fn meter_range(
    ui: &mut Ui,
    theme: &dyn Theme,
    level_db: f32,
    size: Vec2,
    min_db: f32,
    max_db: f32,
    id_salt: impl std::hash::Hash,
) {
    let id = ui.id().with("meter_peak_hold").with(id_salt);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(1), theme.bg_well());
    painter.rect_stroke(
        rect,
        CornerRadius::same(1),
        Stroke::new(1.0_f32, theme.border_soft()),
        egui::StrokeKind::Inside,
    );

    let span = (max_db - min_db).max(0.001);
    let zero_t = ((0.0 - min_db) / span).clamp(0.0, 1.0);
    let inner = Rect::from_min_max(
        egui::pos2(rect.left() + 1.0, rect.top() + 1.0),
        egui::pos2(rect.right() - 1.0, rect.bottom() - 1.0),
    );
    let y0 = inner.bottom() - inner.height() * zero_t;

    let db = level_db.clamp(min_db, max_db);
    if db > min_db + 0.5 {
        let t = (db - min_db) / span;
        let fill_h = inner.height() * t;
        let yellow = theme.meter_yellow();
        let orange = theme.meter_orange();
        let red = theme.meter_red();

        const SLICES: i32 = 48;
        let slice_h = fill_h / SLICES as f32;
        for i in 0..SLICES {
            let y1 = inner.bottom() - (i as f32 + 1.0) * slice_h;
            let yb = inner.bottom() - i as f32 * slice_h;
            if yb <= inner.bottom() - fill_h {
                break;
            }
            let y_mid = (yb + y1) * 0.5;
            let pos = ((inner.bottom() - y_mid) / inner.height()).clamp(0.0, 1.0);
            // Red from 0 dBFS up. Below that: yellow → amber → brick (ease-in).
            let amber = Color32::from_rgb(0xe0, 0x8a, 0x2a);
            let color = if pos >= zero_t {
                red
            } else {
                let u = (pos / zero_t.max(0.001)).clamp(0.0, 1.0);
                if u < 0.50 {
                    lerp_color(yellow, amber, u / 0.50)
                } else {
                    // Last half of the climb: hard into brick/terracotta.
                    let v = ((u - 0.50) / 0.50).clamp(0.0, 1.0);
                    let v = v * v; // ease-in
                    lerp_color(amber, orange, v)
                }
            };
            let band = Rect::from_min_max(
                egui::pos2(inner.left(), y1.max(inner.bottom() - fill_h)),
                egui::pos2(inner.right(), yb),
            );
            if band.height() > 0.2 {
                painter.rect_filled(band, CornerRadius::ZERO, color);
            }
        }
    }

    // Last-peak hold tick (soft chalk gray).
    const HOLD_SECS: f64 = 1.4;
    const FALL_DB_PER_SEC: f32 = 18.0;
    let now = ui.input(|i| i.time);
    let hold_db = {
        let mut hold = ui.ctx().data_mut(|d| {
            d.get_temp::<MeterPeakHold>(id).unwrap_or(MeterPeakHold {
                db: min_db,
                held_until: 0.0,
                last_t: now,
            })
        });
        let dt = (now - hold.last_t).clamp(0.0, 0.1) as f32;
        hold.last_t = now;
        if db >= hold.db - 0.05 {
            hold.db = db.max(hold.db);
            hold.held_until = now + HOLD_SECS;
        } else if now >= hold.held_until {
            hold.db = (hold.db - FALL_DB_PER_SEC * dt).max(db);
        }
        let out = hold.db;
        ui.ctx().data_mut(|d| d.insert_temp(id, hold));
        out
    };
    if hold_db > min_db + 1.0 {
        let ht = ((hold_db - min_db) / span).clamp(0.0, 1.0);
        let y_hold = inner.bottom() - inner.height() * ht;
        painter.hline(
            inner.x_range(),
            y_hold,
            Stroke::new(1.5_f32, theme.meter_peak_hold()),
        );
    }

    // Unity rail — matches fader 0 dB
    painter.hline(
        inner.x_range(),
        y0,
        Stroke::new(1.75_f32, theme.accent()),
    );
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    Color32::from_rgb(
        (a.r() as f32 + (b.r() as f32 - a.r() as f32) * t) as u8,
        (a.g() as f32 + (b.g() as f32 - a.g() as f32) * t) as u8,
        (a.b() as f32 + (b.b() as f32 - a.b() as f32) * t) as u8,
    )
}

pub fn tab_bar(
    ui: &mut Ui,
    theme: &dyn Theme,
    tabs: &[&str],
    selected: &mut usize,
) {
    let hints = [
        "Channel strips & inserts",
        "Apps playing audio",
        "Apps capturing audio",
        "Speakers & sinks",
        "Mics & sources",
        "Controllers & CC maps",
        "Save / load layouts",
        "Preferences",
    ];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (i, name) in tabs.iter().enumerate() {
            let on = *selected == i;
            let text = if on {
                theme.text()
            } else {
                theme.text_muted()
            };
            let fill = if on {
                Color32::from_rgba_unmultiplied(
                    theme.accent().r(),
                    theme.accent().g(),
                    theme.accent().b(),
                    28,
                )
            } else {
                Color32::TRANSPARENT
            };
            let resp = ui
                .add(
                    egui::Button::new(RichText::new(*name).size(12.0).strong().color(text))
                        .fill(fill)
                        .stroke(Stroke::NONE)
                        .corner_radius(theme.rounding()),
                )
                .on_hover_text(hints.get(i).copied().unwrap_or(""));
            if on {
                let r = resp.rect;
                ui.painter().hline(
                    egui::Rangef::new(r.left() + 4.0, r.right() - 4.0),
                    r.bottom() - 1.0,
                    Stroke::new(2.0_f32, theme.accent()),
                );
            }
            if resp.clicked() {
                *selected = i;
            }
        }
    });
}

pub fn section_label(ui: &mut Ui, theme: &dyn Theme, text: &str) {
    ui.label(RichText::new(text).size(10.0).color(theme.text_muted()));
}

pub fn h_slider(
    ui: &mut Ui,
    theme: &dyn Theme,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    label: &str,
) -> egui::Response {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(11.0).color(theme.text_dim()));
        let mut resp = ui.add(egui::Slider::new(value, range.clone()).show_value(true));
        apply_wheel_to_value(ui, &mut resp, value, range);
        hide_cursor_on_drag(ui, &resp);
        resp
    })
    .inner
}

/// egui slider with wheel support + cursor hide while dragging.
///
/// ```ignore
/// design::slider_drag(ui, &mut freq, 20.0..=20_000.0, |s| s.logarithmic(true).suffix(" Hz"));
/// ```
pub fn slider_drag(
    ui: &mut Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    configure: impl FnOnce(egui::Slider<'_>) -> egui::Slider<'_>,
) -> egui::Response {
    let mut resp = {
        let slider = configure(egui::Slider::new(value, range.clone()));
        ui.add(slider)
    };
    apply_wheel_to_value(ui, &mut resp, value, range);
    hide_cursor_on_drag(ui, &resp);
    resp
}

pub fn knob(
    ui: &mut Ui,
    theme: &dyn Theme,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    label: &str,
) -> egui::Response {
    knob_sized(ui, theme, value, range, label, 36.0)
}

pub fn knob_sized(
    ui: &mut Ui,
    theme: &dyn Theme,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    label: &str,
    size: f32,
) -> egui::Response {
    ui.vertical(|ui| {
        ui.with_layout(Layout::top_down(Align::Center), |ui| {
            if !label.is_empty() {
                ui.label(
                    RichText::new(label)
                        .size(10.0)
                        .strong()
                        .color(theme.text_dim()),
                );
            }
            let (rect, mut resp) =
                ui.allocate_exact_size(Vec2::splat(size), Sense::click_and_drag());
            let painter = ui.painter();
            let c = rect.center();
            let r = size * 0.40;

            // Modern console knob — flat dark disc, single indicator line.
            let body = Color32::from_rgb(0x32, 0x34, 0x38);
            let face = Color32::from_rgb(0x3e, 0x41, 0x46);
            let rim = Color32::from_rgb(0x22, 0x24, 0x28);
            let pip = Color32::from_rgb(0xe6, 0xe7, 0xe9);

            painter.circle_filled(
                c + egui::vec2(0.8, 1.2),
                r,
                Color32::from_rgba_unmultiplied(0, 0, 0, 55),
            );
            painter.circle_filled(c, r, body);
            painter.circle_filled(c, r * 0.86, face);
            painter.circle_stroke(c, r, Stroke::new(1.15_f32, rim));

            let t = (*value - *range.start()) / (*range.end() - *range.start()).max(1e-6);
            let ang = -std::f32::consts::FRAC_PI_2 - 0.75 * std::f32::consts::PI
                + t.clamp(0.0, 1.0) * 1.5 * std::f32::consts::PI;
            let outer = c + Vec2::angled(ang) * (r * 0.72);
            let inner = c + Vec2::angled(ang) * (r * 0.18);
            painter.line_segment([inner, outer], Stroke::new(2.2_f32, pip));

            if resp.dragged() {
                let sens = drag_sensitivity(ui);
                let span = *range.end() - *range.start();
                let px_for_full = (size * 1.4).max(48.0);
                let delta = -resp.drag_delta().y * (span / px_for_full) * sens;
                *value = (*value + delta).clamp(*range.start(), *range.end());
                resp.mark_changed();
            }
            apply_wheel_to_value(ui, &mut resp, value, range.clone());
            hide_cursor_on_drag(ui, &resp);
            resp
        })
        .inner
    })
    .inner
}

/// Fruity Soft Clipper transfer (matches DSP): linear to T, then soft knee → 1, × post.
fn softclip_xfer(x: f32, thres: f32, post: f32) -> f32 {
    let t = thres.clamp(0.05, 0.999);
    let ax = x.abs();
    let y = if ax <= t {
        ax
    } else {
        let denom = (1.0 - t).max(1e-6);
        let over = (ax - t) / denom;
        t + denom * over.tanh()
    };
    y.copysign(x) * post.clamp(0.0, 4.0)
}

fn lin_to_db_label(lin: f32) -> String {
    if lin <= 1e-6 {
        "-∞ dB".into()
    } else {
        format!("{:+.1} dB", 20.0 * lin.log10())
    }
}

#[derive(Clone, Copy)]
struct SoftclipVizState {
    /// Input-level histogram (linear bins 0…view_in).
    dens: [f32; 48],
    /// Recent operating-point trail (input linear).
    trail: [f32; 24],
    trail_i: u8,
    /// Smoothed input linear peak.
    in_smooth: f32,
    /// Peak-hold input (slower release) for GR readout.
    in_hold: f32,
    last_t: f64,
    mode: super::TransferVizMode,
    cam: super::Xfer3dCamera,
    band_smooth: [f32; super::BAND_N],
}

/// Soft-clipper transfer plot + live signal visualization.
///
/// Shows where program material sits on the knee: input density along X,
/// a phosphor trail of recent operating points on the curve, and GR when
/// the live peak is past threshold. Drag / scroll horizontally to set Threshold.
///
/// `mode` selects 2D vs 3D (log-frequency) view. Pass pre-FX `spectrum` in 3D.
pub fn softclip_transfer_plot(
    ui: &mut Ui,
    theme: &dyn Theme,
    threshold: &mut f32,
    post: f32,
    peak_db: f32,
    size: Vec2,
    mode: super::TransferVizMode,
    spectrum: Option<&buschain_engine::host::SpectrumFrame>,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());

    // Leave room for axis labels
    let pad_l = 28.0;
    let pad_b = 16.0;
    let pad_t = 14.0;
    let pad_r = 6.0;
    let plot = Rect::from_min_max(
        egui::pos2(rect.left() + pad_l, rect.top() + pad_t),
        egui::pos2(rect.right() - pad_r, rect.bottom() - pad_b),
    );
    if plot.width() < 8.0 || plot.height() < 8.0 {
        return false;
    }

    let thres = threshold.clamp(0.05, 0.999);
    let post = post.clamp(0.0, 4.0);
    // Fixed input span; Y tops out at +2 dB — hotter ceilings still clip visually at the top.
    let view_in = 1.2_f32;
    let view_out = 10f32.powf(2.0 / 20.0); // +2 dBFS
    let to_px = |x: f32, y: f32| {
        egui::pos2(
            plot.left() + (x / view_in).clamp(0.0, 1.0) * plot.width(),
            plot.bottom() - (y / view_out).clamp(0.0, 1.0) * plot.height(),
        )
    };

    // ---- Live signal ballistics (peak → linear in) ----
    let in_inst = if peak_db <= -88.0 {
        0.0
    } else {
        10f32.powf(peak_db / 20.0).clamp(0.0, view_in)
    };
    let viz_id = ui.id().with("softclip_viz");
    let now = ui.input(|i| i.time);
    let mut viz = ui.ctx().data_mut(|d| {
        d.get_temp::<SoftclipVizState>(viz_id).unwrap_or(SoftclipVizState {
            dens: [0.0; 48],
            trail: [0.0; 24],
            trail_i: 0,
            in_smooth: 0.0,
            in_hold: 0.0,
            last_t: now,
            mode: super::TransferVizMode::TwoD,
            cam: super::Xfer3dCamera::default(),
            band_smooth: [0.0; super::BAND_N],
        })
    });
    viz.mode = mode;
    let dt = (now - viz.last_t).clamp(0.0, 0.08) as f32;
    viz.last_t = now;
    let atk = 1.0 - (-dt * 40.0).exp();
    let rel = 1.0 - (-dt * 6.0).exp();
    let hold_rel = 1.0 - (-dt * 1.8).exp();
    if in_inst > viz.in_smooth {
        viz.in_smooth += (in_inst - viz.in_smooth) * atk;
    } else {
        viz.in_smooth += (in_inst - viz.in_smooth) * rel;
    }
    if in_inst > viz.in_hold {
        viz.in_hold = in_inst;
    } else {
        viz.in_hold += (in_inst - viz.in_hold) * hold_rel;
    }
    // Density: bump bin under current peak, decay the rest.
    let dens_n = viz.dens.len();
    let decay = (-dt * 2.2).exp();
    for b in &mut viz.dens {
        *b *= decay;
    }
    if in_inst > 1e-4 {
        let bi = ((in_inst / view_in) * (dens_n as f32 - 1e-3))
            .clamp(0.0, (dens_n - 1) as f32) as usize;
        viz.dens[bi] = (viz.dens[bi] + 0.55).min(1.0);
        // Soft neighbor bleed so the ridge reads as continuous program energy.
        if bi > 0 {
            viz.dens[bi - 1] = (viz.dens[bi - 1] + 0.18).min(1.0);
        }
        if bi + 1 < dens_n {
            viz.dens[bi + 1] = (viz.dens[bi + 1] + 0.18).min(1.0);
        }
    }
    // Trail sample ~every frame while audio is present.
    if in_inst > 1e-4 || viz.in_smooth > 0.02 {
        let i = viz.trail_i as usize % viz.trail.len();
        viz.trail[i] = viz.in_smooth;
        viz.trail_i = viz.trail_i.wrapping_add(1);
    }

    // ---- 3D frequency view (pre-FX spectrum → needles on extruded knee) ----
    if matches!(viz.mode, super::TransferVizMode::ThreeD) {
        let mut band_hz = [0.0_f32; super::BAND_N];
        let mut band_raw = [0.0_f32; super::BAND_N];
        if let Some(frame) = spectrum {
            super::fill_log_bands_linear(
                frame.sample_rate,
                &frame.mags,
                &mut band_hz,
                &mut band_raw,
            );
        } else {
            for i in 0..super::BAND_N {
                let t = (i as f32 + 0.5) / super::BAND_N as f32;
                band_hz[i] = 20.0 * (20_000.0_f32 / 20.0).powf(t);
            }
        }
        let atk_b = 1.0 - (-dt * 28.0).exp();
        let rel_b = 1.0 - (-dt * 5.0).exp();
        for i in 0..super::BAND_N {
            let t = band_raw[i];
            if t > viz.band_smooth[i] {
                viz.band_smooth[i] += (t - viz.band_smooth[i]) * atk_b;
            } else {
                viz.band_smooth[i] += (t - viz.band_smooth[i]) * rel_b;
            }
        }
        let max_b = viz
            .band_smooth
            .iter()
            .copied()
            .fold(0.0_f32, f32::max)
            .max(1e-8);
        let peak_scale = viz.in_smooth.max(in_inst).max(1e-4);
        let mut band_xin = [0.0_f32; super::BAND_N];
        for i in 0..super::BAND_N {
            band_xin[i] = (viz.band_smooth[i] / max_b) * peak_scale;
        }
        let thres_c = thres;
        let post_c = post;
        let xfer = |xin: f32| softclip_xfer(xin, thres_c, post_c);
        let gr = |xin: f32| {
            let y = softclip_xfer(xin, thres_c, 1.0);
            if xin > 1e-6 && y > 1e-6 {
                20.0 * (y / xin).log10()
            } else {
                0.0
            }
        };
        let past = |xin: f32| xin >= thres_c;
        let _hover = super::paint_dynamics_xfer_3d(
            ui,
            theme,
            plot,
            &mut viz.cam,
            0.0,
            view_in,
            0.0,
            view_out,
            &band_hz,
            &band_xin,
            &xfer,
            &gr,
            &past,
            false,
        );
        // Frame chrome labels outside plot
        let painter = ui.painter();
        painter.rect_stroke(
            rect,
            CornerRadius::same(2),
            Stroke::new(1.0_f32, theme.border_soft()),
            egui::StrokeKind::Inside,
        );
        painter.text(
            egui::pos2(plot.center().x, rect.bottom() - 2.0),
            egui::Align2::CENTER_BOTTOM,
            "in · out · freq",
            egui::FontId::proportional(9.0),
            theme.text_muted(),
        );
        let mut changed = false;
        let mut resp = resp;
        // Orbit uses plot interact; scroll still sets Threshold.
        if apply_wheel_to_value(ui, &mut resp, threshold, 0.05..=0.999) {
            changed = true;
        }
        ui.ctx().data_mut(|d| d.insert_temp(viz_id, viz));
        ui.ctx().request_repaint();
        return changed;
    }

    let dens = viz.dens;
    let trail = viz.trail;
    let trail_i = viz.trail_i;
    let in_smooth = viz.in_smooth;
    let in_hold = viz.in_hold;
    ui.ctx().data_mut(|d| d.insert_temp(viz_id, viz));
    if in_inst > 1e-4 || dens.iter().any(|&d| d > 0.02) {
        ui.ctx().request_repaint();
    }

    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), theme.bg_chart());
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0_f32, theme.border_soft()),
        egui::StrokeKind::Inside,
    );

    // Hot zone above 0 dBFS out
    let zero_y = to_px(0.0, 1.0).y;
    if 1.0 < view_out {
        let hot = Rect::from_min_max(
            egui::pos2(plot.left(), plot.top()),
            egui::pos2(plot.right(), zero_y),
        );
        let d = theme.danger();
        painter.rect_filled(
            hot,
            0.0,
            Color32::from_rgba_unmultiplied(d.r(), d.g(), d.b(), 28),
        );
    }

    // Soft clipping region (input ≥ threshold) — subtle wash so the knee is obvious.
    {
        let x0 = to_px(thres, 0.0).x;
        let wash = Rect::from_min_max(
            egui::pos2(x0, plot.top()),
            egui::pos2(plot.right(), plot.bottom()),
        );
        let a = theme.accent();
        painter.rect_filled(
            wash,
            0.0,
            Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), 14),
        );
    }

    let grid = theme.border_soft().gamma_multiply(0.9);
    for i in 0..=4 {
        let t = i as f32 / 4.0;
        let x = plot.left() + plot.width() * t;
        painter.line_segment(
            [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
            Stroke::new(1.0_f32, grid),
        );
    }
    for db in [-12.0_f32, -6.0, 0.0, 2.0] {
        let lin = 10f32.powf(db / 20.0);
        if lin > view_out * 1.001 {
            continue;
        }
        let y = to_px(0.0, lin).y;
        let is_zero = (db - 0.0).abs() < 0.01;
        painter.hline(
            plot.x_range(),
            y,
            Stroke::new(
                if is_zero { 1.5_f32 } else { 1.0_f32 },
                if is_zero {
                    theme.accent().gamma_multiply(0.85)
                } else {
                    grid
                },
            ),
        );
        painter.text(
            egui::pos2(plot.left() - 2.0, y),
            egui::Align2::RIGHT_CENTER,
            if is_zero {
                "0 dB".into()
            } else {
                format!("{db:+.0}")
            },
            egui::FontId::proportional(9.0),
            if is_zero {
                theme.accent()
            } else {
                theme.text_muted()
            },
        );
    }

    // 0 dBFS input rail
    let zero_x = to_px(1.0, 0.0).x;
    painter.vline(
        zero_x,
        plot.y_range(),
        Stroke::new(1.5_f32, theme.accent().gamma_multiply(0.85)),
    );

    // Ceiling (post makeup)
    if post > 1e-4 {
        let cy = to_px(0.0, post.min(view_out)).y;
        painter.hline(
            plot.x_range(),
            cy,
            Stroke::new(1.0_f32, theme.text().gamma_multiply(0.35)),
        );
    }

    // Unity reference (y = x)
    {
        let a = to_px(0.0, 0.0);
        let b = to_px(1.0_f32.min(view_in), 1.0_f32.min(view_out));
        painter.line_segment(
            [a, b],
            Stroke::new(1.0_f32, theme.text_muted().gamma_multiply(0.55)),
        );
    }

    // ---- Input density “ridge” under the curve (where audio lives) ----
    let dens_max = dens.iter().copied().fold(0.0_f32, f32::max).max(0.08);
    let a = theme.accent();
    let bin_w = plot.width() / dens_n as f32;
    for (i, &d) in dens.iter().enumerate() {
        if d < 0.02 {
            continue;
        }
        let t = d / dens_max;
        let x0 = plot.left() + i as f32 * bin_w;
        let h = plot.height() * 0.22 * t;
        let bar = Rect::from_min_max(
            egui::pos2(x0 + 0.5, plot.bottom() - h),
            egui::pos2(x0 + bin_w - 0.5, plot.bottom()),
        );
        let past_knee = (i as f32 + 0.5) / dens_n as f32 * view_in >= thres;
        let col = if past_knee {
            theme.meter_orange()
        } else {
            a
        };
        painter.rect_filled(
            bar,
            CornerRadius::same(1),
            Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), (40.0 + t * 110.0) as u8),
        );
    }

    // Transfer curve
    let mut pts = Vec::with_capacity(97);
    for i in 0..=96 {
        let x = view_in * (i as f32 / 96.0);
        let y = softclip_xfer(x, thres, post);
        pts.push((x, y, to_px(x, y.clamp(0.0, view_out))));
    }
    for w in pts.windows(2) {
        let above = w[0].1 > 1.0 || w[1].1 > 1.0;
        let col = if above {
            theme.meter_orange()
        } else {
            theme.text()
        };
        painter.line_segment([w[0].2, w[1].2], Stroke::new(2.4_f32, col));
    }

    let knee = to_px(thres, softclip_xfer(thres, thres, post).clamp(0.0, view_out));
    painter.circle_filled(knee, 3.2, theme.accent());
    painter.circle_stroke(knee, 3.2, Stroke::new(1.0_f32, theme.border()));

    // ---- Phosphor trail of recent operating points on the curve ----
    let n_trail = trail.len();
    for k in 0..n_trail {
        // Oldest → newest
        let age = n_trail - 1 - k;
        let idx = trail_i.wrapping_sub(1).wrapping_sub(age as u8) as usize % n_trail;
        let xin = trail[idx];
        if xin < 1e-4 {
            continue;
        }
        let yout = softclip_xfer(xin, thres, post);
        let p = to_px(xin, yout.clamp(0.0, view_out));
        let fade = (k as f32 / (n_trail as f32 - 1.0)).clamp(0.0, 1.0);
        let alpha = (18.0 + fade * 140.0) as u8;
        let r = 1.4 + fade * 2.2;
        let col = if xin >= thres {
            theme.meter_orange()
        } else {
            a
        };
        painter.circle_filled(
            p,
            r,
            Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha),
        );
    }

    // ---- Live operating point + GR readout ----
    if in_hold > 1e-4 {
        let xin = in_smooth.max(in_hold * 0.85);
        let y_unity = xin; // pre-curve (pre-post would be xin; post applied in xfer)
        let y_out = softclip_xfer(xin, thres, post);
        let p_op = to_px(xin, y_out.clamp(0.0, view_out));
        let p_unity = to_px(xin, y_unity.clamp(0.0, view_out));

        // Vertical probe at current input
        painter.vline(
            p_op.x,
            plot.y_range(),
            Stroke::new(
                1.0_f32,
                Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), 55),
            ),
        );

        // GR: gap between unity path and clipped path (pre-makeup comparison).
        let y_curve_pre = softclip_xfer(xin, thres, 1.0);
        let gr_db = if xin > 1e-6 && y_curve_pre > 1e-6 {
            20.0 * (y_curve_pre / xin).log10()
        } else {
            0.0
        };
        if xin > thres && gr_db < -0.05 {
            // Shade GR between unity and curve at this X
            let top = p_unity.y.min(p_op.y);
            let bot = p_unity.y.max(p_op.y);
            let gr_rect = Rect::from_min_max(
                egui::pos2(p_op.x - 3.0, top),
                egui::pos2(p_op.x + 3.0, bot),
            );
            let o = theme.meter_orange();
            painter.rect_filled(
                gr_rect,
                CornerRadius::same(1),
                Color32::from_rgba_unmultiplied(o.r(), o.g(), o.b(), 90),
            );
            painter.text(
                egui::pos2(p_op.x + 6.0, (top + bot) * 0.5),
                egui::Align2::LEFT_CENTER,
                format!("{gr_db:.1} dB"),
                egui::FontId::proportional(10.0),
                theme.meter_orange(),
            );
        }

        // Operating-point jewel
        let op_col = if xin >= thres {
            theme.meter_orange()
        } else {
            theme.success()
        };
        painter.circle_filled(p_op, 5.0, op_col);
        painter.circle_filled(p_op, 2.0, Color32::WHITE);
        painter.circle_stroke(p_op, 5.0, Stroke::new(1.0_f32, theme.border()));
    }

    // Axis captions + readouts
    painter.text(
        egui::pos2(plot.center().x, rect.bottom() - 2.0),
        egui::Align2::CENTER_BOTTOM,
        "in →",
        egui::FontId::proportional(9.0),
        theme.text_muted(),
    );
    painter.text(
        egui::pos2(zero_x, rect.bottom() - 2.0),
        egui::Align2::CENTER_BOTTOM,
        "0dB",
        egui::FontId::proportional(9.0),
        theme.accent(),
    );
    let gain_txt = format!("ceil {}", lin_to_db_label(post.max(1e-6)));
    let gain_col = if post > 1.001 {
        theme.meter_orange()
    } else if post < 0.999 {
        theme.text_dim()
    } else {
        theme.accent()
    };
    painter.text(
        egui::pos2(plot.right() - 2.0, rect.top() + 2.0),
        egui::Align2::RIGHT_TOP,
        gain_txt,
        egui::FontId::proportional(10.0),
        gain_col,
    );
    if in_hold > 1e-4 {
        painter.text(
            egui::pos2(plot.left() + 4.0, rect.top() + 2.0),
            egui::Align2::LEFT_TOP,
            format!("in {}", lin_to_db_label(in_hold)),
            egui::FontId::proportional(10.0),
            theme.text_dim(),
        );
    }

    let mut changed = false;
    let mut resp = resp;
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let nx = ((pos.x - plot.left()) / plot.width()).clamp(0.0, 1.0);
            let new_t = (nx * view_in).clamp(0.05, 0.999);
            if (*threshold - new_t).abs() > 0.001 {
                *threshold = new_t;
                changed = true;
            }
        }
    }
    if apply_wheel_to_value(ui, &mut resp, threshold, 0.05..=0.999) {
        changed = true;
    }
    hide_cursor_on_drag(ui, &resp);
    changed
}

/* ---- Limiter transfer + GR history (matches buschain_builtins lm_gain_db) ---- */

fn limiter_gr_db(level_db: f32, ceil_db: f32, knee_db: f32) -> f32 {
    if knee_db < 0.05 {
        if level_db > ceil_db {
            ceil_db - level_db
        } else {
            0.0
        }
    } else {
        let half = knee_db * 0.5;
        let lo = ceil_db - half;
        let hi = ceil_db + half;
        if level_db <= lo {
            0.0
        } else if level_db >= hi {
            ceil_db - level_db
        } else {
            let d = level_db - lo;
            let mut out_db = level_db - (d * d) / (2.0 * knee_db);
            if out_db > ceil_db {
                out_db = ceil_db;
            }
            out_db - level_db
        }
    }
}

fn limiter_xfer_db(level_db: f32, ceil_db: f32, knee_db: f32, makeup_db: f32) -> f32 {
    level_db + limiter_gr_db(level_db, ceil_db, knee_db) + makeup_db
}

#[derive(Clone, Copy)]
struct LimiterVizState {
    dens: [f32; 48],
    trail: [f32; 24],
    trail_i: u8,
    in_smooth: f32,
    in_hold: f32,
    /// Scrolling GR history (−dB, positive magnitude).
    gr_hist: [f32; 96],
    gr_i: u8,
    last_t: f64,
    mode: super::TransferVizMode,
    cam: super::Xfer3dCamera,
    band_smooth: [f32; super::BAND_N],
}

/// Limiter transfer plot + GR history strip.
///
/// Unity → ceiling fold (soft knee when set), density / phosphor trail, live GR.
/// Drag vertically (or scroll) to set Ceiling.
///
/// `mode` selects 2D vs 3D (log-frequency) view. GR history strip is 2D-only.
/// Pass pre-FX `spectrum` in 3D.
pub fn limiter_transfer_plot(
    ui: &mut Ui,
    theme: &dyn Theme,
    ceiling_db: &mut f32,
    knee_db: f32,
    input_db: f32,
    makeup_db: f32,
    peak_db: f32,
    size: Vec2,
    mode: super::TransferVizMode,
    spectrum: Option<&buschain_engine::host::SpectrumFrame>,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());

    let pad_l = 28.0;
    let pad_b = 4.0;
    let pad_t = 14.0;
    let pad_r = 6.0;
    let use_3d = matches!(mode, super::TransferVizMode::ThreeD);
    let gr_h = if use_3d {
        0.0
    } else {
        (rect.height() * 0.22).clamp(28.0, 44.0)
    };
    let gap = if use_3d { 0.0 } else { 4.0 };
    let plot = Rect::from_min_max(
        egui::pos2(rect.left() + pad_l, rect.top() + pad_t),
        egui::pos2(
            rect.right() - pad_r,
            rect.bottom() - pad_b - gr_h - gap,
        ),
    );
    let gr_strip = Rect::from_min_max(
        egui::pos2(rect.left() + pad_l, plot.bottom() + gap),
        egui::pos2(rect.right() - pad_r, rect.bottom() - pad_b),
    );
    if plot.width() < 8.0 || plot.height() < 8.0 {
        return false;
    }

    let ceil = ceiling_db.clamp(-24.0, 0.0);
    let knee = knee_db.clamp(0.0, 12.0);
    let makeup = makeup_db.clamp(-24.0, 24.0);
    let input_g = input_db.clamp(-24.0, 24.0);

    const DB_MIN: f32 = -36.0;
    const DB_MAX: f32 = 6.0;
    let db_span = DB_MAX - DB_MIN;
    let to_px = |xin_db: f32, yout_db: f32| {
        egui::pos2(
            plot.left() + ((xin_db - DB_MIN) / db_span).clamp(0.0, 1.0) * plot.width(),
            plot.bottom() - ((yout_db - DB_MIN) / db_span).clamp(0.0, 1.0) * plot.height(),
        )
    };

    // Peak is post-rack-ish; treat as detector input after Input gain.
    let in_inst = if peak_db <= -88.0 {
        DB_MIN
    } else {
        (peak_db + input_g).clamp(DB_MIN, DB_MAX)
    };

    let viz_id = ui.id().with("limiter_viz");
    let now = ui.input(|i| i.time);
    let mut viz = ui.ctx().data_mut(|d| {
        d.get_temp::<LimiterVizState>(viz_id).unwrap_or(LimiterVizState {
            dens: [0.0; 48],
            trail: [0.0; 24],
            trail_i: 0,
            in_smooth: DB_MIN,
            in_hold: DB_MIN,
            gr_hist: [0.0; 96],
            gr_i: 0,
            last_t: now,
            mode: super::TransferVizMode::TwoD,
            cam: super::Xfer3dCamera::default(),
            band_smooth: [0.0; super::BAND_N],
        })
    });
    viz.mode = mode;
    let dt = (now - viz.last_t).clamp(0.0, 0.08) as f32;
    viz.last_t = now;
    let atk = 1.0 - (-dt * 40.0).exp();
    let rel = 1.0 - (-dt * 6.0).exp();
    let hold_rel = 1.0 - (-dt * 1.8).exp();
    if in_inst > viz.in_smooth {
        viz.in_smooth += (in_inst - viz.in_smooth) * atk;
    } else {
        viz.in_smooth += (in_inst - viz.in_smooth) * rel;
    }
    if in_inst > viz.in_hold {
        viz.in_hold = in_inst;
    } else {
        viz.in_hold += (in_inst - viz.in_hold) * hold_rel;
    }

    let dens_n = viz.dens.len();
    let decay = (-dt * 2.2).exp();
    for b in &mut viz.dens {
        *b *= decay;
    }
    if in_inst > DB_MIN + 0.5 {
        let t = ((in_inst - DB_MIN) / db_span).clamp(0.0, 1.0 - 1e-3);
        let bi = (t * (dens_n as f32 - 1e-3)) as usize;
        viz.dens[bi] = (viz.dens[bi] + 0.55).min(1.0);
        if bi > 0 {
            viz.dens[bi - 1] = (viz.dens[bi - 1] + 0.18).min(1.0);
        }
        if bi + 1 < dens_n {
            viz.dens[bi + 1] = (viz.dens[bi + 1] + 0.18).min(1.0);
        }
    }
    if in_inst > DB_MIN + 0.5 || viz.in_smooth > DB_MIN + 1.0 {
        let i = viz.trail_i as usize % viz.trail.len();
        viz.trail[i] = viz.in_smooth;
        viz.trail_i = viz.trail_i.wrapping_add(1);
    }

    let gr_now = -limiter_gr_db(viz.in_smooth, ceil, knee);
    {
        let i = viz.gr_i as usize % viz.gr_hist.len();
        viz.gr_hist[i] = gr_now.max(0.0);
        viz.gr_i = viz.gr_i.wrapping_add(1);
    }

    if use_3d {
        let mut band_hz = [0.0_f32; super::BAND_N];
        let mut band_raw = [0.0_f32; super::BAND_N];
        if let Some(frame) = spectrum {
            super::fill_log_bands_linear(
                frame.sample_rate,
                &frame.mags,
                &mut band_hz,
                &mut band_raw,
            );
        } else {
            for i in 0..super::BAND_N {
                let t = (i as f32 + 0.5) / super::BAND_N as f32;
                band_hz[i] = 20.0 * (20_000.0_f32 / 20.0).powf(t);
            }
        }
        let atk_b = 1.0 - (-dt * 28.0).exp();
        let rel_b = 1.0 - (-dt * 5.0).exp();
        for i in 0..super::BAND_N {
            let t = band_raw[i];
            if t > viz.band_smooth[i] {
                viz.band_smooth[i] += (t - viz.band_smooth[i]) * atk_b;
            } else {
                viz.band_smooth[i] += (t - viz.band_smooth[i]) * rel_b;
            }
        }
        let max_b = viz
            .band_smooth
            .iter()
            .copied()
            .fold(0.0_f32, f32::max)
            .max(1e-8);
        let peak_span = (viz.in_smooth - DB_MIN).max(0.0);
        let mut band_xin = [DB_MIN; super::BAND_N];
        for i in 0..super::BAND_N {
            band_xin[i] = DB_MIN + (viz.band_smooth[i] / max_b) * peak_span;
        }
        let ceil_c = ceil;
        let knee_c = knee;
        let makeup_c = makeup;
        let xfer = |xin: f32| limiter_xfer_db(xin, ceil_c, knee_c, makeup_c);
        let gr = |xin: f32| limiter_gr_db(xin, ceil_c, knee_c);
        let past = |xin: f32| xin >= ceil_c - knee_c * 0.5;
        let _hover = super::paint_dynamics_xfer_3d(
            ui,
            theme,
            plot,
            &mut viz.cam,
            DB_MIN,
            DB_MAX,
            DB_MIN,
            DB_MAX,
            &band_hz,
            &band_xin,
            &xfer,
            &gr,
            &past,
            true,
        );
        let painter = ui.painter();
        painter.rect_stroke(
            rect,
            CornerRadius::same(2),
            Stroke::new(1.0_f32, theme.border_soft()),
            egui::StrokeKind::Inside,
        );
        painter.text(
            egui::pos2(plot.center().x, rect.bottom() - 2.0),
            egui::Align2::CENTER_BOTTOM,
            "in · out · freq (dB)",
            egui::FontId::proportional(9.0),
            theme.text_muted(),
        );
        let mut changed = false;
        let mut resp = resp;
        if apply_wheel_to_value(ui, &mut resp, ceiling_db, -24.0..=0.0) {
            changed = true;
        }
        ui.ctx().data_mut(|d| d.insert_temp(viz_id, viz));
        ui.ctx().request_repaint();
        return changed;
    }

    let dens = viz.dens;
    let trail = viz.trail;
    let trail_i = viz.trail_i;
    let in_smooth = viz.in_smooth;
    let in_hold = viz.in_hold;
    let gr_hist = viz.gr_hist;
    let gr_i = viz.gr_i;
    ui.ctx().data_mut(|d| d.insert_temp(viz_id, viz));
    if in_inst > DB_MIN + 0.5 || dens.iter().any(|&d| d > 0.02) || gr_now > 0.05 {
        ui.ctx().request_repaint();
    }

    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), theme.bg_chart());
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0_f32, theme.border_soft()),
        egui::StrokeKind::Inside,
    );

    // Hot zone above 0 dBFS out
    let zero_y = to_px(0.0, 0.0).y;
    if DB_MAX > 0.0 {
        let hot = Rect::from_min_max(
            egui::pos2(plot.left(), plot.top()),
            egui::pos2(plot.right(), zero_y),
        );
        let d = theme.danger();
        painter.rect_filled(
            hot,
            0.0,
            Color32::from_rgba_unmultiplied(d.r(), d.g(), d.b(), 28),
        );
    }

    // Limiting region (input ≥ ceiling − knee/2)
    {
        let start = (ceil - knee * 0.5).clamp(DB_MIN, DB_MAX);
        let x0 = to_px(start, 0.0).x;
        let wash = Rect::from_min_max(
            egui::pos2(x0, plot.top()),
            egui::pos2(plot.right(), plot.bottom()),
        );
        let a = theme.accent();
        painter.rect_filled(
            wash,
            0.0,
            Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), 14),
        );
    }

    let grid = theme.border_soft().gamma_multiply(0.9);
    for db in [-24.0_f32, -12.0, -6.0, 0.0] {
        let p = to_px(db, db);
        painter.vline(
            p.x,
            plot.y_range(),
            Stroke::new(1.0_f32, grid),
        );
        painter.hline(
            plot.x_range(),
            p.y,
            Stroke::new(
                if (db - 0.0).abs() < 0.01 {
                    1.5_f32
                } else {
                    1.0_f32
                },
                if (db - 0.0).abs() < 0.01 {
                    theme.accent().gamma_multiply(0.85)
                } else {
                    grid
                },
            ),
        );
        painter.text(
            egui::pos2(plot.left() - 2.0, p.y),
            egui::Align2::RIGHT_CENTER,
            format!("{db:+.0}"),
            egui::FontId::proportional(9.0),
            if (db - 0.0).abs() < 0.01 {
                theme.accent()
            } else {
                theme.text_muted()
            },
        );
    }

    // Unity reference
    {
        let a = to_px(DB_MIN, DB_MIN);
        let b = to_px(DB_MAX, DB_MAX);
        painter.line_segment(
            [a, b],
            Stroke::new(1.0_f32, theme.text_muted().gamma_multiply(0.55)),
        );
    }

    // Ceiling line (pre-makeup)
    {
        let cy = to_px(0.0, ceil + makeup).y;
        painter.hline(
            plot.x_range(),
            cy,
            Stroke::new(1.2_f32, theme.meter_orange().gamma_multiply(0.85)),
        );
        painter.text(
            egui::pos2(plot.right() - 2.0, cy),
            egui::Align2::RIGHT_BOTTOM,
            format!("ceil {ceil:+.1}"),
            egui::FontId::proportional(9.0),
            theme.meter_orange(),
        );
    }

    // Density ridge
    let dens_max = dens.iter().copied().fold(0.0_f32, f32::max).max(0.08);
    let a = theme.accent();
    let bin_w = plot.width() / dens_n as f32;
    for (i, &d) in dens.iter().enumerate() {
        if d < 0.02 {
            continue;
        }
        let t = d / dens_max;
        let x0 = plot.left() + i as f32 * bin_w;
        let h = plot.height() * 0.22 * t;
        let bar = Rect::from_min_max(
            egui::pos2(x0 + 0.5, plot.bottom() - h),
            egui::pos2(x0 + bin_w - 0.5, plot.bottom()),
        );
        let xin = DB_MIN + (i as f32 + 0.5) / dens_n as f32 * db_span;
        let past = xin >= ceil - knee * 0.5;
        let col = if past {
            theme.meter_orange()
        } else {
            a
        };
        painter.rect_filled(
            bar,
            CornerRadius::same(1),
            Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), (40.0 + t * 110.0) as u8),
        );
    }

    // Transfer curve
    let mut pts = Vec::with_capacity(97);
    for i in 0..=96 {
        let x = DB_MIN + db_span * (i as f32 / 96.0);
        let y = limiter_xfer_db(x, ceil, knee, makeup);
        pts.push((x, y, to_px(x, y.clamp(DB_MIN, DB_MAX))));
    }
    for w in pts.windows(2) {
        let limited = w[0].1 < w[0].0 + makeup - 0.05 || w[1].1 < w[1].0 + makeup - 0.05;
        let col = if limited {
            theme.meter_orange()
        } else {
            theme.text()
        };
        painter.line_segment([w[0].2, w[1].2], Stroke::new(2.4_f32, col));
    }

    let knee_pt = to_px(
        ceil,
        limiter_xfer_db(ceil, ceil, knee, makeup).clamp(DB_MIN, DB_MAX),
    );
    painter.circle_filled(knee_pt, 3.2, theme.accent());
    painter.circle_stroke(knee_pt, 3.2, Stroke::new(1.0_f32, theme.border()));

    // Phosphor trail
    let n_trail = trail.len();
    for k in 0..n_trail {
        let age = n_trail - 1 - k;
        let idx = trail_i.wrapping_sub(1).wrapping_sub(age as u8) as usize % n_trail;
        let xin = trail[idx];
        if xin < DB_MIN + 0.5 {
            continue;
        }
        let yout = limiter_xfer_db(xin, ceil, knee, makeup);
        let p = to_px(xin, yout.clamp(DB_MIN, DB_MAX));
        let fade = (k as f32 / (n_trail as f32 - 1.0)).clamp(0.0, 1.0);
        let alpha = (18.0 + fade * 140.0) as u8;
        let r = 1.4 + fade * 2.2;
        let col = if xin >= ceil - knee * 0.5 {
            theme.meter_orange()
        } else {
            a
        };
        painter.circle_filled(
            p,
            r,
            Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha),
        );
    }

    // Live operating point + GR
    if in_hold > DB_MIN + 0.5 {
        let xin = in_smooth.max(in_hold - 1.0);
        let y_out = limiter_xfer_db(xin, ceil, knee, makeup);
        let y_unity = xin + makeup;
        let p_op = to_px(xin, y_out.clamp(DB_MIN, DB_MAX));
        let p_unity = to_px(xin, y_unity.clamp(DB_MIN, DB_MAX));
        painter.vline(
            p_op.x,
            plot.y_range(),
            Stroke::new(
                1.0_f32,
                Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), 55),
            ),
        );
        let gr_db = limiter_gr_db(xin, ceil, knee);
        if gr_db < -0.05 {
            let top = p_unity.y.min(p_op.y);
            let bot = p_unity.y.max(p_op.y);
            let gr_rect = Rect::from_min_max(
                egui::pos2(p_op.x - 3.0, top),
                egui::pos2(p_op.x + 3.0, bot),
            );
            let o = theme.meter_orange();
            painter.rect_filled(
                gr_rect,
                CornerRadius::same(1),
                Color32::from_rgba_unmultiplied(o.r(), o.g(), o.b(), 90),
            );
            painter.text(
                egui::pos2(p_op.x + 6.0, (top + bot) * 0.5),
                egui::Align2::LEFT_CENTER,
                format!("{gr_db:.1} dB"),
                egui::FontId::proportional(10.0),
                theme.meter_orange(),
            );
        }
        let op_col = if gr_db < -0.05 {
            theme.meter_orange()
        } else {
            theme.success()
        };
        painter.circle_filled(p_op, 5.0, op_col);
        painter.circle_filled(p_op, 2.0, Color32::WHITE);
        painter.circle_stroke(p_op, 5.0, Stroke::new(1.0_f32, theme.border()));
    }

    painter.text(
        egui::pos2(plot.left() + 4.0, rect.top() + 2.0),
        egui::Align2::LEFT_TOP,
        if in_hold > DB_MIN + 0.5 {
            format!("in {in_hold:+.1} dB")
        } else {
            "in —".into()
        },
        egui::FontId::proportional(10.0),
        theme.text_dim(),
    );

    // GR history strip
    {
        painter.rect_filled(
            gr_strip,
            CornerRadius::same(1),
            theme.bg_well().gamma_multiply(0.85),
        );
        painter.text(
            egui::pos2(gr_strip.left() - 2.0, gr_strip.center().y),
            egui::Align2::RIGHT_CENTER,
            "GR",
            egui::FontId::proportional(9.0),
            theme.text_muted(),
        );
        const GR_MAX: f32 = 24.0;
        let n = gr_hist.len();
        let bw = gr_strip.width() / n as f32;
        let o = theme.meter_orange();
        for k in 0..n {
            let age = n - 1 - k;
            let idx = gr_i.wrapping_sub(1).wrapping_sub(age as u8) as usize % n;
            let g = gr_hist[idx].clamp(0.0, GR_MAX);
            if g < 0.05 {
                continue;
            }
            let h = (g / GR_MAX) * gr_strip.height();
            let x0 = gr_strip.left() + k as f32 * bw;
            let bar = Rect::from_min_max(
                egui::pos2(x0 + 0.3, gr_strip.bottom() - h),
                egui::pos2(x0 + bw - 0.3, gr_strip.bottom()),
            );
            let alpha = (50.0 + (g / GR_MAX) * 160.0) as u8;
            painter.rect_filled(
                bar,
                CornerRadius::ZERO,
                Color32::from_rgba_unmultiplied(o.r(), o.g(), o.b(), alpha),
            );
        }
        if gr_now > 0.05 {
            painter.text(
                egui::pos2(gr_strip.right() - 2.0, gr_strip.top() + 1.0),
                egui::Align2::RIGHT_TOP,
                format!("−{gr_now:.1} dB"),
                egui::FontId::proportional(9.0),
                theme.meter_orange(),
            );
        }
    }

    let mut changed = false;
    let mut resp = resp;
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            // Vertical drag sets ceiling (ignore GR strip).
            if pos.y <= plot.bottom() + gap * 0.5 {
                let ny = ((plot.bottom() - pos.y) / plot.height()).clamp(0.0, 1.0);
                let new_c = (DB_MIN + ny * db_span - makeup).clamp(-24.0, 0.0);
                if (*ceiling_db - new_c).abs() > 0.01 {
                    *ceiling_db = new_c;
                    changed = true;
                }
            }
        }
    }
    if apply_wheel_to_value(ui, &mut resp, ceiling_db, -24.0..=0.0) {
        changed = true;
    }
    hide_cursor_on_drag(ui, &resp);
    changed
}

/* ---- Theatre Drive waveshaper viz (matches buschain_builtins od_shape) ---- */

fn od_shape(x: f32, character: i32) -> f32 {
    match character {
        1 => (x * 0.85).tanh() * 1.08, // Soft
        2 => {
            // Hard
            let a = x.abs();
            let y = if a < 1.0 {
                a - a * a * a / 3.0
            } else {
                0.666_666_7
            };
            y.copysign(x) * 1.15
        }
        3 => {
            // Diode (asymmetric)
            let pos = if x >= 0.0 {
                1.0 - (-x * 1.4).exp()
            } else {
                0.0
            };
            let neg = if x < 0.0 {
                -(0.65 * (1.0 - (x * 1.8).exp()))
            } else {
                0.0
            };
            (pos + neg) * 1.2
        }
        _ => {
            // Tube
            let y = x.tanh();
            y + 0.04 * y * y * y
        }
    }
}

fn od_xfer(x: f32, drive: f32, boost: bool, bias: f32, character: i32, postg: f32) -> f32 {
    let mut gain = 1.0 + drive * drive * 36.0;
    if boost {
        gain *= 10.0;
    }
    let bias_amt = bias * 0.22;
    let makeup = postg * (1.15 / (0.35 + drive * 0.9 + if boost { 0.8 } else { 0.0 }));
    od_shape(x * gain + bias_amt, character) * makeup
}

#[derive(Clone, Copy)]
struct DriveVizState {
    dens: [f32; 40],
    trail: [f32; 28],
    trail_i: u8,
    in_smooth: f32,
    in_hold: f32,
    last_t: f64,
}

/// Theatre Drive transfer plot — bipolar waveshaper with live density / phosphor trail / GR.
pub fn theatre_drive_plot(
    ui: &mut Ui,
    theme: &dyn Theme,
    drive: f32,
    boost: bool,
    bias: f32,
    character: i32,
    postg: f32,
    peak_db: f32,
    size: Vec2,
) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let pad_l = 26.0;
    let pad_b = 14.0;
    let pad_t = 14.0;
    let pad_r = 6.0;
    let plot = Rect::from_min_max(
        egui::pos2(rect.left() + pad_l, rect.top() + pad_t),
        egui::pos2(rect.right() - pad_r, rect.bottom() - pad_b),
    );
    if plot.width() < 8.0 || plot.height() < 8.0 {
        return;
    }

    let drive = drive.clamp(0.0, 1.0);
    let bias = bias.clamp(-1.0, 1.0);
    let postg = postg.clamp(0.0, 1.0);
    let character = character.clamp(0, 3);
    let view = 1.35_f32; // ±view on both axes

    let to_px = |x: f32, y: f32| {
        egui::pos2(
            plot.left() + ((x / view) * 0.5 + 0.5).clamp(0.0, 1.0) * plot.width(),
            plot.bottom() - ((y / view) * 0.5 + 0.5).clamp(0.0, 1.0) * plot.height(),
        )
    };

    let in_inst = if peak_db <= -88.0 {
        0.0
    } else {
        10f32.powf(peak_db / 20.0).clamp(0.0, view)
    };
    let viz_id = ui.id().with("theatre_drive_viz");
    let now = ui.input(|i| i.time);
    let mut viz = ui.ctx().data_mut(|d| {
        d.get_temp::<DriveVizState>(viz_id).unwrap_or(DriveVizState {
            dens: [0.0; 40],
            trail: [0.0; 28],
            trail_i: 0,
            in_smooth: 0.0,
            in_hold: 0.0,
            last_t: now,
        })
    });
    let dt = (now - viz.last_t).clamp(0.0, 0.08) as f32;
    viz.last_t = now;
    let atk = 1.0 - (-dt * 40.0).exp();
    let rel = 1.0 - (-dt * 6.0).exp();
    let hold_rel = 1.0 - (-dt * 1.8).exp();
    if in_inst > viz.in_smooth {
        viz.in_smooth += (in_inst - viz.in_smooth) * atk;
    } else {
        viz.in_smooth += (in_inst - viz.in_smooth) * rel;
    }
    if in_inst > viz.in_hold {
        viz.in_hold = in_inst;
    } else {
        viz.in_hold += (in_inst - viz.in_hold) * hold_rel;
    }
    let dens_n = viz.dens.len();
    let decay = (-dt * 2.2).exp();
    for b in &mut viz.dens {
        *b *= decay;
    }
    if in_inst > 1e-4 {
        let bi = ((in_inst / view) * (dens_n as f32 - 1e-3))
            .clamp(0.0, (dens_n - 1) as f32) as usize;
        viz.dens[bi] = (viz.dens[bi] + 0.55).min(1.0);
        if bi > 0 {
            viz.dens[bi - 1] = (viz.dens[bi - 1] + 0.18).min(1.0);
        }
        if bi + 1 < dens_n {
            viz.dens[bi + 1] = (viz.dens[bi + 1] + 0.18).min(1.0);
        }
    }
    if in_inst > 1e-4 || viz.in_smooth > 0.02 {
        let i = viz.trail_i as usize % viz.trail.len();
        viz.trail[i] = viz.in_smooth;
        viz.trail_i = viz.trail_i.wrapping_add(1);
    }
    let dens = viz.dens;
    let trail = viz.trail;
    let trail_i = viz.trail_i;
    let in_smooth = viz.in_smooth;
    let in_hold = viz.in_hold;
    ui.ctx().data_mut(|d| d.insert_temp(viz_id, viz));
    if in_inst > 1e-4 || dens.iter().any(|&d| d > 0.02) {
        ui.ctx().request_repaint();
    }

    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), theme.bg_chart());
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0_f32, theme.border_soft()),
        egui::StrokeKind::Inside,
    );

    let a = theme.accent();
    let o = theme.meter_orange();
    let grid = theme.border_soft().gamma_multiply(0.9);

    // Soft saturation wash outside ±1 (hard clip territory for some chars)
    {
        let p_lo = to_px(-1.0, -view);
        let p_hi = to_px(1.0, view);
        let left = Rect::from_min_max(
            egui::pos2(plot.left(), plot.top()),
            egui::pos2(p_lo.x, plot.bottom()),
        );
        let right = Rect::from_min_max(
            egui::pos2(p_hi.x, plot.top()),
            egui::pos2(plot.right(), plot.bottom()),
        );
        let wash = Color32::from_rgba_unmultiplied(o.r(), o.g(), o.b(), 16);
        painter.rect_filled(left, 0.0, wash);
        painter.rect_filled(right, 0.0, wash);
    }

    // Grid + axes through origin
    let origin = to_px(0.0, 0.0);
    for v in [-1.0_f32, -0.5, 0.5, 1.0] {
        let p = to_px(v, 0.0);
        painter.vline(p.x, plot.y_range(), Stroke::new(1.0_f32, grid));
        let q = to_px(0.0, v);
        painter.hline(plot.x_range(), q.y, Stroke::new(1.0_f32, grid));
    }
    painter.vline(
        origin.x,
        plot.y_range(),
        Stroke::new(1.25_f32, a.gamma_multiply(0.7)),
    );
    painter.hline(
        plot.x_range(),
        origin.y,
        Stroke::new(1.25_f32, a.gamma_multiply(0.7)),
    );

    // Unity diagonal
    painter.line_segment(
        [to_px(-view, -view), to_px(view, view)],
        Stroke::new(1.0_f32, theme.text_muted().gamma_multiply(0.5)),
    );

    // Input |x| density as mirrored bars from the X axis (energy at each magnitude)
    let dens_max = dens.iter().copied().fold(0.0_f32, f32::max).max(0.08);
    let bin_w = (plot.width() * 0.5) / dens_n as f32;
    for (i, &d) in dens.iter().enumerate() {
        if d < 0.02 {
            continue;
        }
        let t = d / dens_max;
        let xin = (i as f32 + 0.5) / dens_n as f32 * view;
        let h = plot.height() * 0.14 * t;
        for sign in [-1.0_f32, 1.0] {
            let cx = to_px(xin * sign, 0.0).x;
            let bar = Rect::from_min_max(
                egui::pos2(cx - bin_w * 0.4, origin.y - h * 0.15),
                egui::pos2(cx + bin_w * 0.4, origin.y + h),
            );
            // Color by how hard the shaper hits at this level
            let y_drive = od_xfer(xin, drive, boost, bias, character, postg).abs();
            let hot = y_drive < xin * 0.92; // compressing vs unity
            let col = if hot { o } else { a };
            painter.rect_filled(
                bar,
                CornerRadius::same(1),
                Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), (36.0 + t * 100.0) as u8),
            );
        }
    }

    // Waveshaper curve
    let mut pts = Vec::with_capacity(129);
    for i in 0..=128 {
        let x = -view + (2.0 * view) * (i as f32 / 128.0);
        let y = od_xfer(x, drive, boost, bias, character, postg);
        pts.push(to_px(x, y.clamp(-view, view)));
    }
    for w in pts.windows(2) {
        painter.line_segment([w[0], w[1]], Stroke::new(2.35_f32, theme.text()));
    }

    // Phosphor trail (±peaks on the curve)
    let n_trail = trail.len();
    for k in 0..n_trail {
        let age = n_trail - 1 - k;
        let idx = trail_i.wrapping_sub(1).wrapping_sub(age as u8) as usize % n_trail;
        let xin = trail[idx];
        if xin < 1e-4 {
            continue;
        }
        let fade = (k as f32 / (n_trail as f32 - 1.0)).clamp(0.0, 1.0);
        let alpha = (16.0 + fade * 130.0) as u8;
        let r = 1.3 + fade * 2.0;
        for sign in [-1.0_f32, 1.0] {
            let x = xin * sign;
            let y = od_xfer(x, drive, boost, bias, character, postg);
            let p = to_px(x, y.clamp(-view, view));
            let hot = y.abs() < xin * 0.92;
            let col = if hot { o } else { a };
            painter.circle_filled(
                p,
                r,
                Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha),
            );
        }
    }

    // Live operating points (±)
    if in_hold > 1e-4 {
        let xin = in_smooth.max(in_hold * 0.85);
        for sign in [-1.0_f32, 1.0] {
            let x = xin * sign;
            let y = od_xfer(x, drive, boost, bias, character, postg);
            let p = to_px(x, y.clamp(-view, view));
            let p_u = to_px(x, x.clamp(-view, view));
            painter.vline(
                p.x,
                plot.y_range(),
                Stroke::new(
                    1.0_f32,
                    Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), 40),
                ),
            );
            // Harmonic “squash” marker when |out| < |in| (pre-makeup-ish via unity compare)
            if y.abs() + 1e-4 < xin {
                let top = p.y.min(p_u.y);
                let bot = p.y.max(p_u.y);
                painter.rect_filled(
                    Rect::from_min_max(
                        egui::pos2(p.x - 2.5, top),
                        egui::pos2(p.x + 2.5, bot),
                    ),
                    CornerRadius::same(1),
                    Color32::from_rgba_unmultiplied(o.r(), o.g(), o.b(), 85),
                );
            }
            let op_col = if y.abs() < xin * 0.92 {
                o
            } else {
                theme.success()
            };
            painter.circle_filled(p, 4.5, op_col);
            painter.circle_filled(p, 1.8, Color32::WHITE);
            painter.circle_stroke(p, 4.5, Stroke::new(1.0_f32, theme.border()));
        }
        let y_pos = od_xfer(xin, drive, boost, bias, character, postg);
        let squash_db = if xin > 1e-6 {
            20.0 * (y_pos.abs() / xin).log10()
        } else {
            0.0
        };
        if squash_db < -0.15 {
            painter.text(
                egui::pos2(plot.right() - 4.0, plot.top() + 14.0),
                egui::Align2::RIGHT_TOP,
                format!("sat {squash_db:.1} dB"),
                egui::FontId::proportional(10.0),
                o,
            );
        }
    }

    let char_name = ["Tube", "Soft", "Hard", "Diode"]
        .get(character as usize)
        .copied()
        .unwrap_or("?");
    painter.text(
        egui::pos2(plot.left() + 4.0, rect.top() + 2.0),
        egui::Align2::LEFT_TOP,
        if in_hold > 1e-4 {
            format!("in {} · {char_name}", lin_to_db_label(in_hold))
        } else {
            char_name.into()
        },
        egui::FontId::proportional(10.0),
        theme.text_dim(),
    );
    painter.text(
        egui::pos2(plot.right() - 2.0, rect.top() + 2.0),
        egui::Align2::RIGHT_TOP,
        if boost { "×10" } else { "drive" },
        egui::FontId::proportional(10.0),
        if boost { o } else { a },
    );
    painter.text(
        egui::pos2(plot.center().x, rect.bottom() - 2.0),
        egui::Align2::CENTER_BOTTOM,
        "in ↔ out",
        egui::FontId::proportional(9.0),
        theme.text_muted(),
    );
}

/// BusChain in-house plugin shell — compact chrome, minimal name (no meter header bar).
pub fn inhouse_shell(
    ui: &mut Ui,
    theme: &dyn Theme,
    title: &str,
    tagline: &str,
    peak_db: f32,
    add_contents: impl FnOnce(&mut Ui),
) {
    let _ = peak_db;
    inhouse_shell_ex(ui, theme, title, tagline, peak_db, false, add_contents);
}

/// Same as [`inhouse_shell`]. `header_meters` is ignored — keep the name row minimal.
pub fn inhouse_shell_ex(
    ui: &mut Ui,
    theme: &dyn Theme,
    title: &str,
    tagline: &str,
    peak_db: f32,
    _header_meters: bool,
    add_contents: impl FnOnce(&mut Ui),
) {
    let _ = peak_db;
    Frame::NONE
        .fill(theme.bg_elevated())
        .stroke(Stroke::new(1.0_f32, theme.border_soft()))
        .corner_radius(theme.rounding())
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            // Size to content — do NOT stretch to the window width (avoids right padding).
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(title)
                        .size(11.0)
                        .strong()
                        .color(theme.accent()),
                );
                if !tagline.is_empty() {
                    ui.label(
                        RichText::new(tagline)
                            .size(9.0)
                            .color(theme.text_muted()),
                    );
                }
            });
            add_contents(ui);
        });
}

/// Page chrome for misc tabs (Playback / IO / MIDI / Settings).
pub fn page_header(ui: &mut Ui, theme: &dyn Theme, title: &str, blurb: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(title)
                .size(14.0)
                .strong()
                .color(theme.text()),
        );
    });
    if !blurb.is_empty() {
        ui.add_space(2.0);
        ui.label(
            RichText::new(blurb)
                .size(11.0)
                .color(theme.text_muted()),
        );
    }
    ui.add_space(8.0);
    let w = ui.available_width();
    let (div, _) = ui.allocate_exact_size(Vec2::new(w, 1.0), Sense::hover());
    ui.painter().hline(
        div.x_range(),
        div.center().y,
        Stroke::new(1.0_f32, theme.border_soft()),
    );
    ui.add_space(10.0);
}

/// Title strip with live stereo meters (shared by all BusChain builtins).
pub fn inhouse_title_row(
    ui: &mut Ui,
    theme: &dyn Theme,
    title: &str,
    tagline: &str,
    peak_db: f32,
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(
                RichText::new(title)
                    .size(12.0)
                    .strong()
                    .color(theme.accent()),
            );
            if !tagline.is_empty() {
                ui.label(
                    RichText::new(tagline)
                        .size(9.0)
                        .color(theme.text_muted()),
                );
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            plugin_stereo_meters(
                ui,
                theme,
                peak_db,
                Vec2::new(26.0, 44.0),
                ("inhouse_hdr", title),
            );
            ui.add_space(4.0);
            let txt = if peak_db <= -89.0 {
                "— dB".into()
            } else {
                format!("{peak_db:+.0} dB")
            };
            ui.label(
                RichText::new(txt)
                    .size(10.0)
                    .monospace()
                    .color(if peak_db > -0.5 {
                        theme.meter_red()
                    } else if peak_db > -6.0 {
                        theme.meter_orange()
                    } else {
                        theme.text_dim()
                    }),
            );
        });
    });
}

/// Narrow stereo peak meters for in-house plugin chrome.
pub fn plugin_stereo_meters(
    ui: &mut Ui,
    theme: &dyn Theme,
    level_db: f32,
    size: Vec2,
    id_salt: impl std::hash::Hash,
) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), theme.bg_well());
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0_f32, theme.border_soft()),
        egui::StrokeKind::Inside,
    );

    let min_db = -48.0_f32;
    let max_db = 6.0_f32;
    let span = max_db - min_db;
    let zero_t = ((0.0 - min_db) / span).clamp(0.0, 1.0);
    let t = ((level_db.clamp(min_db, max_db) - min_db) / span).clamp(0.0, 1.0);
    let gap = 3.0;
    let bar_w = ((rect.width() - gap - 4.0) * 0.5).max(3.0);

    // Peak-hold state (shared for both bars — mono display of bus peak).
    let hold_id = ui.id().with("plugin_stereo_hold").with(id_salt);
    let now = ui.input(|i| i.time);
    let hold_t = {
        let mut hold = ui.ctx().data_mut(|d| {
            d.get_temp::<MeterPeakHold>(hold_id).unwrap_or(MeterPeakHold {
                db: min_db,
                held_until: 0.0,
                last_t: now,
            })
        });
        let dt = (now - hold.last_t).clamp(0.0, 0.1) as f32;
        hold.last_t = now;
        let db = level_db.clamp(min_db, max_db);
        if db >= hold.db - 0.05 {
            hold.db = db.max(hold.db);
            hold.held_until = now + 1.4;
        } else if now >= hold.held_until {
            hold.db = (hold.db - 18.0 * dt).max(db);
        }
        let ht = ((hold.db - min_db) / span).clamp(0.0, 1.0);
        ui.ctx().data_mut(|d| d.insert_temp(hold_id, hold));
        ht
    };

    for i in 0..2 {
        let x0 = rect.left() + 2.0 + i as f32 * (bar_w + gap);
        let well = Rect::from_min_max(
            egui::pos2(x0, rect.top() + 3.0),
            egui::pos2(x0 + bar_w, rect.bottom() - 3.0),
        );
        painter.rect_filled(well, CornerRadius::same(1), theme.bg_app());

        if t > 0.01 {
            const SLICES: i32 = 32;
            let fill_h = well.height() * t;
            let slice_h = fill_h / SLICES as f32;
            for s in 0..SLICES {
                let y1 = well.bottom() - (s as f32 + 1.0) * slice_h;
                let yb = well.bottom() - s as f32 * slice_h;
                let y_mid = (yb + y1) * 0.5;
                let pos = ((well.bottom() - y_mid) / well.height()).clamp(0.0, 1.0);
                let color = if pos >= zero_t {
                    theme.meter_red()
                } else {
                    let u = (pos / zero_t.max(0.001)).clamp(0.0, 1.0);
                    if u < 0.55 {
                        lerp_color(theme.meter_green(), theme.meter_yellow(), u / 0.55)
                    } else {
                        lerp_color(
                            theme.meter_yellow(),
                            theme.meter_orange(),
                            ((u - 0.55) / 0.45).clamp(0.0, 1.0),
                        )
                    }
                };
                let band = Rect::from_min_max(
                    egui::pos2(well.left(), y1.max(well.bottom() - fill_h)),
                    egui::pos2(well.right(), yb),
                );
                if band.height() > 0.2 {
                    painter.rect_filled(band, CornerRadius::ZERO, color);
                }
            }
        }

        let py = well.bottom() - well.height() * hold_t;
        painter.hline(
            egui::Rangef::new(well.left(), well.right()),
            py,
            Stroke::new(1.5_f32, theme.meter_peak_hold()),
        );
        // 0 dBFS tick
        let zy = well.bottom() - well.height() * zero_t;
        painter.hline(
            egui::Rangef::new(well.left(), well.right()),
            zy,
            Stroke::new(1.0_f32, theme.accent().gamma_multiply(0.45)),
        );
    }

    if level_db > -85.0 {
        ui.ctx().request_repaint();
    }
}

/// Narrow stereo output meters (legacy name — themed BusChain chrome).
pub fn softclip_meters(ui: &mut Ui, theme: &dyn Theme, level_db: f32, size: Vec2) {
    plugin_stereo_meters(ui, theme, level_db, size, "softclip_meters");
}

/// Band markers — muted champagne / steel (BusChain console, not neon FL).
pub const PEQ_BAND_COLORS: [Color32; 8] = [
    Color32::from_rgb(0xc9, 0xa2, 0x6b), // champagne
    Color32::from_rgb(0xb0, 0x9a, 0x7a), // sand
    Color32::from_rgb(0x8e, 0x92, 0x98), // steel
    Color32::from_rgb(0xa8, 0xaa, 0xae), // silver
    Color32::from_rgb(0x9a, 0x8a, 0x72), // bronze
    Color32::from_rgb(0x7a, 0x82, 0x88), // slate
    Color32::from_rgb(0xc0, 0xb0, 0x90), // light sand
    Color32::from_rgb(0x6e, 0x72, 0x78), // fader steel
];

#[derive(Clone, Copy)]
pub struct PeqBand {
    pub on: bool,
    pub freq: f32,
    pub gain_db: f32,
    pub q: f32,
    /// 0 peak, 1 low shelf, 2 high shelf, 3 HP, 4 LP
    pub mode: i32,
}

pub fn peq_biquad_coeffs(b: &PeqBand, sr: f32) -> (f32, f32, f32, f32, f32) {
    let w0 = 2.0 * std::f32::consts::PI * b.freq.clamp(20.0, sr * 0.45) / sr;
    let cosw = w0.cos();
    let sinw = w0.sin();
    let a = 10f32.powf(b.gain_db / 40.0);
    let q = b.q.max(0.1);
    let alpha = sinw / (2.0 * q);
    let (b0, b1, b2, a0, a1, a2) = match b.mode {
        3 => {
            // HP
            let b0 = (1.0 + cosw) * 0.5;
            (b0, -(1.0 + cosw), b0, 1.0 + alpha, -2.0 * cosw, 1.0 - alpha)
        }
        4 => {
            // LP
            let b0 = (1.0 - cosw) * 0.5;
            (b0, 1.0 - cosw, b0, 1.0 + alpha, -2.0 * cosw, 1.0 - alpha)
        }
        1 => {
            // LS
            let t = 2.0 * a.sqrt() * alpha;
            (
                a * ((a + 1.0) - (a - 1.0) * cosw + t),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cosw),
                a * ((a + 1.0) - (a - 1.0) * cosw - t),
                (a + 1.0) + (a - 1.0) * cosw + t,
                -2.0 * ((a - 1.0) + (a + 1.0) * cosw),
                (a + 1.0) + (a - 1.0) * cosw - t,
            )
        }
        2 => {
            // HS
            let t = 2.0 * a.sqrt() * alpha;
            (
                a * ((a + 1.0) + (a - 1.0) * cosw + t),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cosw),
                a * ((a + 1.0) + (a - 1.0) * cosw - t),
                (a + 1.0) - (a - 1.0) * cosw + t,
                2.0 * ((a - 1.0) - (a + 1.0) * cosw),
                (a + 1.0) - (a - 1.0) * cosw - t,
            )
        }
        _ => {
            // Peak
            (
                1.0 + alpha * a,
                -2.0 * cosw,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cosw,
                1.0 - alpha / a,
            )
        }
    };
    (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
}

pub fn peq_mag_db_at(bands: &[PeqBand], out_gain_db: f32, freq_hz: f32, sr: f32) -> f32 {
    let w = 2.0 * std::f32::consts::PI * freq_hz / sr;
    let (zr, zi) = (w.cos(), -w.sin());
    // z^{-1}, z^{-2}
    let z1r = zr;
    let z1i = zi;
    let z2r = zr * zr - zi * zi;
    let z2i = 2.0 * zr * zi;

    let mut hr = 1.0_f32;
    let mut hi = 0.0_f32;
    for b in bands {
        if !b.on {
            continue;
        }
        let (b0, b1, b2, a1, a2) = peq_biquad_coeffs(b, sr);
        let num_r = b0 + b1 * z1r + b2 * z2r;
        let num_i = b1 * z1i + b2 * z2i;
        let den_r = 1.0 + a1 * z1r + a2 * z2r;
        let den_i = a1 * z1i + a2 * z2i;
        let den_n = den_r * den_r + den_i * den_i;
        if den_n < 1e-20 {
            continue;
        }
        let br = (num_r * den_r + num_i * den_i) / den_n;
        let bi = (num_i * den_r - num_r * den_i) / den_n;
        let nr = hr * br - hi * bi;
        let ni = hr * bi + hi * br;
        hr = nr;
        hi = ni;
    }
    let mag = (hr * hr + hi * hi).sqrt().max(1e-12);
    20.0 * mag.log10() + out_gain_db
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
struct PeakSpectrumState {
    bins: [f32; 48],
    last_t: f64,
}

/// Legacy peak-shaped bars (unused by Equalizer/Analyzer — kept for Denoiser-era sketches).
#[allow(dead_code)]
fn paint_peak_spectrum(
    ui: &mut Ui,
    plot: Rect,
    theme: &dyn Theme,
    peak_db: f32,
    f_min: f32,
    f_max: f32,
    id_salt: impl std::hash::Hash,
) {
    const N: usize = 48;
    let spec_id = ui.id().with(("peak_spectrum", id_salt));
    let now = ui.input(|i| i.time);
    let energy = if peak_db <= -88.0 {
        0.0
    } else {
        // Map −60..0 dB → 0..1
        ((peak_db + 60.0) / 60.0).clamp(0.0, 1.2)
    };
    let mut state = ui.ctx().data_mut(|d| {
        d.get_temp::<PeakSpectrumState>(spec_id)
            .unwrap_or(PeakSpectrumState {
                bins: [0.0; N],
                last_t: now,
            })
    });
    let dt = (now - state.last_t).clamp(0.0, 0.08) as f32;
    state.last_t = now;
    let attack = 1.0 - (-dt * 28.0).exp();
    let release = 1.0 - (-dt * 5.5).exp();

    for i in 0..N {
        let t = i as f32 / (N - 1) as f32;
        let f = (f_min.ln() + t * (f_max.ln() - f_min.ln())).exp();
        // Static pink-ish contour (no time-varying wobble).
        let tilt = ((180.0 / f).sqrt() * (f / 10_000.0).sqrt().clamp(0.4, 1.0)).clamp(0.25, 1.5);
        let hash = (((i as u32).wrapping_mul(2654435761)) >> 17) as f32 / 32767.0;
        let target = (energy * tilt * (0.82 + 0.18 * hash)).clamp(0.0, 1.0);
        let cur = state.bins[i];
        let coeff = if target > cur { attack } else { release };
        state.bins[i] = cur + (target - cur) * coeff;
    }
    let bins = state.bins;
    ui.ctx().data_mut(|d| d.insert_temp(spec_id, state));

    let a = theme.accent();
    let bar_w = (plot.width() / N as f32).max(1.0);
    let painter = ui.painter();
    for (i, &bin) in bins.iter().enumerate() {
        if bin < 0.008 {
            continue;
        }
        let t = i as f32 / (N - 1) as f32;
        let x = plot.left() + t * plot.width();
        let h = plot.height() * bin;
        let bar = Rect::from_min_max(
            egui::pos2(x - bar_w * 0.36, plot.bottom() - h),
            egui::pos2(x + bar_w * 0.36, plot.bottom()),
        );
        let alpha = (55.0 + bin * 120.0).clamp(40.0, 175.0) as u8;
        painter.rect_filled(
            bar,
            CornerRadius::same(1),
            Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), alpha),
        );
    }
    if energy > 0.02 {
        ui.ctx().request_repaint();
    }
}

/* ---- Denoiser parametric NR (bell nodes → 6-band filterbank) ---- */

/// One surgical NR bell — depth at `freq`, width from `q` (higher = pinchier).
#[derive(Clone, Copy, Debug)]
pub struct NrNode {
    pub on: bool,
    pub freq: f32,
    pub depth_db: f32,
    pub q: f32,
}

pub const DN_NR_MAX_NODES: usize = 8;
/// Filterbank analysis centers — NR curve is sampled here for the DSP.
pub const DN_ANALYSIS_HZ: [f32; 6] = [120.0, 240.0, 600.0, 1580.0, 3000.0, 12000.0];

/// Log-frequency Gaussian bell depth at `f` (sum of enabled nodes).
pub fn nr_bell_depth_at(nodes: &[NrNode], f: f32) -> f32 {
    let f = f.max(20.0);
    let mut d = 0.0_f32;
    for n in nodes {
        if !n.on || n.depth_db <= 0.05 {
            continue;
        }
        let q = n.q.clamp(0.3, 12.0);
        let x = (f.ln() - n.freq.max(20.0).ln()) * q;
        d += n.depth_db * (-0.5 * x * x).exp();
    }
    d.clamp(0.0, 48.0)
}

/// Map parametric NR bells onto the 6 DSP band ranges/centers.
/// Active nodes become band centers (so you can park on a hiss tone);
/// empty bands stay at 0 dB pull so nature content isn't expanded for free.
/// Low-Q (wide) bells still bleed onto nearby band centers via sampling.
pub fn nr_nodes_to_bands(nodes: &[NrNode]) -> ([f32; 6], [f32; 6]) {
    let mut active: Vec<NrNode> = nodes
        .iter()
        .copied()
        .filter(|n| n.on && n.depth_db > 0.05)
        .collect();
    active.sort_by(|a, b| {
        b.depth_db
            .partial_cmp(&a.depth_db)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if active.len() > 6 {
        active.truncate(6);
    }
    active.sort_by(|a, b| {
        a.freq
            .partial_cmp(&b.freq)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut freqs = DN_ANALYSIS_HZ;
    let mut ranges = [0.0_f32; 6];
    for (i, n) in active.iter().enumerate() {
        freqs[i] = n.freq.clamp(20.0, 20_000.0);
        ranges[i] = n.depth_db.clamp(0.0, 48.0);
    }
    if active.len() < 6 {
        let mut fillers: Vec<f32> = DN_ANALYSIS_HZ.to_vec();
        for n in &active {
            if let Some((idx, _)) = fillers.iter().enumerate().min_by(|(_, a), (_, b)| {
                let da = (a.ln() - n.freq.max(20.0).ln()).abs();
                let db = (b.ln() - n.freq.max(20.0).ln()).abs();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            }) {
                fillers.remove(idx);
            }
        }
        for (j, i) in (active.len()..6).enumerate() {
            if let Some(&f) = fillers.get(j) {
                freqs[i] = f;
            }
            ranges[i] = 0.0;
        }
    }
    for i in 0..6 {
        ranges[i] = ranges[i].max(nr_bell_depth_at(nodes, freqs[i]));
    }
    (ranges, freqs)
}

const DN_SPEC_BINS: usize = 64;

#[derive(Clone, Copy)]
struct HiResSpecState {
    bins: [f32; DN_SPEC_BINS],
    norm_hold: f32,
    last_t: f64,
}

/// Peak-norm spectrum as discrete frequency buckets (no waves / fills / shimmer).
/// Light bars = in (pre), darker bars = out (NR preview). Lower plot band only.
fn paint_dual_peak_norm(
    ui: &mut Ui,
    plot: Rect,
    _theme: &dyn Theme,
    in_peak_db: f32,
    out_peak_db: f32,
    nodes: &[NrNode],
    f_min: f32,
    f_max: f32,
) {
    const NS: usize = DN_SPEC_BINS;
    const SPEC_H: f32 = 0.55;
    let spec_id = ui.id().with("denoiser_raw_buckets_v4");
    let now = ui.input(|i| i.time);
    let energy_in = if in_peak_db <= -88.0 {
        0.0
    } else {
        ((in_peak_db + 96.0) / 96.0).clamp(0.03, 1.15)
    };
    let energy_out = if out_peak_db <= -88.0 {
        0.0
    } else {
        ((out_peak_db + 96.0) / 96.0).clamp(0.0, 1.15)
    };
    let mut state = ui.ctx().data_mut(|d| {
        d.get_temp::<HiResSpecState>(spec_id)
            .unwrap_or(HiResSpecState {
                bins: [0.0; DN_SPEC_BINS],
                norm_hold: 1.0,
                last_t: now,
            })
    });
    let dt = (now - state.last_t).clamp(0.0, 0.08) as f32;
    state.last_t = now;
    let attack = 1.0 - (-dt * 28.0).exp();
    let release = 1.0 - (-dt * 6.0).exp();

    for i in 0..NS {
        let t = (i as f32 + 0.5) / NS as f32;
        let f = (f_min.ln() + t * (f_max.ln() - f_min.ln())).exp();
        // Static bucket weights only — no animation / blur / sine shimmer.
        let pink = (200.0 / f).sqrt().clamp(0.4, 1.6);
        let mid = (-((f.ln() - 1600f32.ln()) / 1.1).powi(2)).exp() * 0.30;
        let air = ((f / 5_000.0).log10()).clamp(0.0, 1.0) * 0.40;
        let shape = (pink * 0.55 + mid + air).clamp(0.15, 2.0);
        let target = energy_in * shape;
        let cur = state.bins[i];
        let coeff = if target > cur { attack } else { release };
        state.bins[i] = cur + (target - cur) * coeff;
    }
    let raw_max = state.bins.iter().copied().fold(0.0_f32, f32::max).max(1e-4);
    let norm_atk = 1.0 - (-dt * 18.0).exp();
    let norm_rel = 1.0 - (-dt * 1.4).exp();
    if raw_max > state.norm_hold {
        state.norm_hold += (raw_max - state.norm_hold) * norm_atk;
    } else {
        state.norm_hold += (raw_max - state.norm_hold) * norm_rel;
    }
    let scale = 1.0 / state.norm_hold.max(1e-4);
    let bins = state.bins;
    ui.ctx().data_mut(|d| d.insert_temp(spec_id, state));

    let painter = ui.painter();
    let out_scale = if energy_in > 1e-4 {
        (energy_out / energy_in).clamp(0.08, 1.0)
    } else {
        0.0
    };
    let slot = plot.width() / NS as f32;
    let gap = (slot * 0.12).clamp(0.5, 2.0);

    for i in 0..NS {
        let t = (i as f32 + 0.5) / NS as f32;
        let f = (f_min.ln() + t * (f_max.ln() - f_min.ln())).exp();
        let n_in = (bins[i] * scale).clamp(0.0, 1.0);
        let depth = nr_bell_depth_at(nodes, f);
        let keep = (1.0 - 0.90 * (depth / 48.0)).clamp(0.05, 1.0);
        let n_out = (n_in * keep * out_scale.max(0.25 + keep * 0.5)).clamp(0.0, 1.0);

        let x0 = plot.left() + i as f32 * slot + gap * 0.5;
        let x1 = plot.left() + (i + 1) as f32 * slot - gap * 0.5;
        if x1 <= x0 {
            continue;
        }

        // Light = input bucket.
        if n_in > 0.008 {
            let h = plot.height() * SPEC_H * n_in;
            painter.rect_filled(
                Rect::from_min_max(
                    egui::pos2(x0, plot.bottom() - h),
                    egui::pos2(x1, plot.bottom()),
                ),
                CornerRadius::ZERO,
                Color32::from_rgba_unmultiplied(0xC0, 0xC0, 0xC0, 150),
            );
        }
        // Dark = output bucket (inset so both read clearly).
        if n_out > 0.008 {
            let h = plot.height() * SPEC_H * n_out;
            let inset = ((x1 - x0) * 0.18).clamp(0.5, 2.5);
            painter.rect_filled(
                Rect::from_min_max(
                    egui::pos2(x0 + inset, plot.bottom() - h),
                    egui::pos2(x1 - inset, plot.bottom()),
                ),
                CornerRadius::ZERO,
                Color32::from_rgba_unmultiplied(0x58, 0x58, 0x58, 200),
            );
        }
    }

    if energy_in > 0.02 || energy_out > 0.02 {
        ui.ctx().request_repaint();
    }
}

#[derive(Clone, Copy, PartialEq)]
enum NrDragKind {
    None,
    Threshold,
    Node(usize),
    /// Empty-plot drag creates a node (freq + depth).
    NewDepth,
}

/// Parametric NR graph — dual spectra + addable depth bells (freq / depth / Q).
/// Threshold guide is always drawn/draggable (NR dig floor).
/// Double-click empty space to add a node. Delete/Backspace removes selected.
/// Returns true if nodes or threshold changed.
pub fn denoiser_param_graph(
    ui: &mut Ui,
    theme: &dyn Theme,
    threshold_db: &mut f32,
    nodes: &mut Vec<NrNode>,
    selected: &mut usize,
    in_peak_db: f32,
    out_peak_db: f32,
    size: Vec2,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let pad_l = 36.0;
    let pad_r = 10.0;
    let pad_t = 16.0;
    let pad_b = 20.0;
    let plot = Rect::from_min_max(
        egui::pos2(rect.left() + pad_l, rect.top() + pad_t),
        egui::pos2(rect.right() - pad_r, rect.bottom() - pad_b),
    );
    if plot.width() < 16.0 || plot.height() < 16.0 {
        return false;
    }

    const F_MIN: f32 = 20.0;
    const F_MAX: f32 = 20_000.0;
    const D_TOP: f32 = 0.0;   // 0 dB pull at top
    const D_BOT: f32 = 48.0;  // max pull at bottom

    let freq_to_x = |f: f32| {
        let t = ((f.max(F_MIN).ln() - F_MIN.ln()) / (F_MAX.ln() - F_MIN.ln())).clamp(0.0, 1.0);
        plot.left() + t * plot.width()
    };
    let x_to_freq = |x: f32| {
        let t = ((x - plot.left()) / plot.width()).clamp(0.0, 1.0);
        (F_MIN.ln() + t * (F_MAX.ln() - F_MIN.ln())).exp()
    };
    // Depth axis: 0 at top, 48 at bottom (how hard we pull).
    let depth_to_y = |d: f32| {
        let t = (d.clamp(D_TOP, D_BOT) / D_BOT).clamp(0.0, 1.0);
        plot.top() + t * plot.height()
    };
    let y_to_depth = |y: f32| {
        let t = ((y - plot.top()) / plot.height()).clamp(0.0, 1.0);
        t * D_BOT
    };
    // Threshold as a faint guide in the upper zone (mapped −140..0 → top third).
    let thr_to_y = |db: f32| {
        let t = ((db.clamp(-140.0, 0.0) + 140.0) / 140.0).clamp(0.0, 1.0);
        plot.top() + (1.0 - t) * plot.height() * 0.35
    };

    {
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(2), theme.bg_chart());
        painter.rect_stroke(
            rect,
            CornerRadius::same(2),
            Stroke::new(1.0_f32, theme.border_soft()),
            egui::StrokeKind::Inside,
        );
    }

    paint_dual_peak_norm(
        ui,
        plot,
        theme,
        in_peak_db,
        out_peak_db,
        nodes,
        F_MIN,
        F_MAX,
    );

    let painter = ui.painter();
    let a = theme.accent();
    let o = theme.meter_orange();
    let grid = theme.border_soft().gamma_multiply(0.85);

    for &d in &[0.0_f32, 12.0, 24.0, 36.0, 48.0] {
        let y = depth_to_y(d);
        painter.hline(plot.x_range(), y, Stroke::new(1.0_f32, grid));
        painter.text(
            egui::pos2(rect.left() + 4.0, y),
            egui::Align2::LEFT_CENTER,
            if d < 0.5 {
                "0".into()
            } else {
                format!("−{d:.0}")
            },
            egui::FontId::proportional(9.0),
            theme.text_muted(),
        );
    }
    for &(f, lab) in &[(100.0_f32, "100"), (1_000.0, "1k"), (10_000.0, "10k")] {
        let x = freq_to_x(f);
        painter.vline(x, plot.y_range(), Stroke::new(1.0_f32, grid));
        painter.text(
            egui::pos2(x, rect.bottom() - 3.0),
            egui::Align2::CENTER_BOTTOM,
            lab,
            egui::FontId::proportional(9.0),
            theme.text_muted(),
        );
    }

    // NR threshold guide — dig below this level; drag to set.
    {
        let y = thr_to_y(*threshold_db);
        painter.hline(
            plot.x_range(),
            y,
            Stroke::new(1.5_f32, a.gamma_multiply(0.85)),
        );
        painter.text(
            egui::pos2(plot.right() - 2.0, y - 2.0),
            egui::Align2::RIGHT_BOTTOM,
            format!("thresh {threshold_db:+.0}"),
            egui::FontId::proportional(9.0),
            a,
        );
    }

    // Composite NR depth fill (sum of bells).
    {
        let mut pts = Vec::with_capacity(97);
        pts.push(egui::pos2(plot.left(), depth_to_y(0.0)));
        for i in 0..=96 {
            let t = i as f32 / 96.0;
            let f = (F_MIN.ln() + t * (F_MAX.ln() - F_MIN.ln())).exp();
            let d = nr_bell_depth_at(nodes, f);
            pts.push(egui::pos2(freq_to_x(f), depth_to_y(d)));
        }
        pts.push(egui::pos2(plot.right(), depth_to_y(0.0)));
        // Fill under curve toward top (0 depth).
        for w in pts.windows(2).skip(1).take(96) {
            let x0 = w[0].x;
            let x1 = w[1].x;
            let y0 = w[0].y;
            let y1 = w[1].y;
            let top = depth_to_y(0.0);
            let poly = [
                egui::pos2(x0, top),
                egui::pos2(x1, top),
                egui::pos2(x1, y1),
                egui::pos2(x0, y0),
            ];
            painter.add(egui::Shape::convex_polygon(
                poly.to_vec(),
                Color32::from_rgba_unmultiplied(o.r(), o.g(), o.b(), 38),
                Stroke::NONE,
            ));
        }
        for w in pts[1..pts.len() - 1].windows(2) {
            painter.line_segment([w[0], w[1]], Stroke::new(2.0_f32, o));
        }
    }

    if !nodes.is_empty() {
        *selected = (*selected).min(nodes.len() - 1);
    }

    // Node handles.
    for (i, n) in nodes.iter().enumerate() {
        if !n.on {
            continue;
        }
        let p = egui::pos2(freq_to_x(n.freq), depth_to_y(n.depth_db));
        let sel = *selected == i;
        let r = if sel { 6.0 } else { 4.2 };
        painter.circle_filled(p, r, if sel { a } else { o });
        painter.circle_stroke(p, r, Stroke::new(1.0_f32, theme.border()));
        if sel {
            painter.circle_stroke(
                p,
                r + 3.0,
                Stroke::new(1.0_f32, a.gamma_multiply(0.6)),
            );
        }
        // Q whiskers — wider = lower Q.
        let q = n.q.clamp(0.3, 12.0);
        let f_lo = (n.freq * (-0.55 / q).exp()).clamp(F_MIN, F_MAX);
        let f_hi = (n.freq * (0.55 / q).exp()).clamp(F_MIN, F_MAX);
        let y = p.y;
        painter.line_segment(
            [egui::pos2(freq_to_x(f_lo), y), egui::pos2(freq_to_x(f_hi), y)],
            Stroke::new(
                1.0_f32,
                Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), if sel { 160 } else { 70 }),
            ),
        );
    }

    painter.text(
        egui::pos2(plot.left() + 4.0, rect.top() + 2.0),
        egui::Align2::LEFT_TOP,
        "buckets: in/out · orange=NR · line=thresh",
        egui::FontId::proportional(10.0),
        theme.text_dim(),
    );
    painter.text(
        egui::pos2(plot.right() - 2.0, rect.top() + 2.0),
        egui::Align2::RIGHT_TOP,
        "dbl-click add · scroll depth · Alt+scroll Q",
        egui::FontId::proportional(9.0),
        theme.text_muted(),
    );
    painter.text(
        egui::pos2(rect.left() + 2.0, rect.top() + 14.0),
        egui::Align2::LEFT_TOP,
        "pull",
        egui::FontId::proportional(8.0),
        theme.text_muted(),
    );

    let drag_id = ui.id().with("dn_nr_drag");
    let mut drag = ui
        .ctx()
        .data(|d| d.get_temp::<NrDragKind>(drag_id))
        .unwrap_or(NrDragKind::None);
    let mut changed = false;
    let mut pending_new: Option<(f32, f32)> = None;

    let hit_node = |pos: egui::Pos2, nodes: &[NrNode]| -> Option<usize> {
        let mut best = None;
        let mut best_d = 14.0_f32;
        for (i, n) in nodes.iter().enumerate() {
            if !n.on {
                continue;
            }
            let p = egui::pos2(freq_to_x(n.freq), depth_to_y(n.depth_db));
            let d = pos.distance(p);
            if d < best_d {
                best_d = d;
                best = Some(i);
            }
        }
        best
    };

    if resp.drag_started() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let y_thr = thr_to_y(*threshold_db);
            if (pos.y - y_thr).abs() < 7.0 {
                drag = NrDragKind::Threshold;
            } else if let Some(i) = hit_node(pos, nodes) {
                *selected = i;
                drag = NrDragKind::Node(i);
            } else if nodes.len() < DN_NR_MAX_NODES {
                drag = NrDragKind::NewDepth;
                pending_new = Some((x_to_freq(pos.x), y_to_depth(pos.y)));
            } else {
                drag = NrDragKind::None;
            }
        }
    }

    // Create node on new-depth drag start.
    if matches!(drag, NrDragKind::NewDepth) {
        if let Some((f, d)) = pending_new.take() {
            nodes.push(NrNode {
                on: true,
                freq: f.clamp(20.0, 20_000.0),
                depth_db: d.clamp(0.0, 48.0),
                q: 2.0,
            });
            *selected = nodes.len() - 1;
            drag = NrDragKind::Node(*selected);
            changed = true;
        }
    }

    if resp.dragged() {
        if let Some(pos) = resp.interact_pointer_pos() {
            match drag {
                NrDragKind::Threshold => {
                    // Map upper-zone drag back to threshold dB.
                    let t = 1.0 - ((pos.y - plot.top()) / (plot.height() * 0.35)).clamp(0.0, 1.0);
                    let db = (-140.0 + t * 140.0).clamp(-140.0, 0.0);
                    if (db - *threshold_db).abs() > 0.05 {
                        *threshold_db = db;
                        changed = true;
                    }
                }
                NrDragKind::Node(i) => {
                    if let Some(n) = nodes.get_mut(i) {
                        let f = x_to_freq(pos.x).clamp(20.0, 20_000.0);
                        let d = y_to_depth(pos.y).clamp(0.0, 48.0);
                        if (f - n.freq).abs() > 0.5 || (d - n.depth_db).abs() > 0.05 {
                            n.freq = f;
                            n.depth_db = d;
                            n.on = true;
                            changed = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    if resp.drag_stopped() {
        drag = NrDragKind::None;
    }

    if resp.double_clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            if hit_node(pos, nodes).is_none() && nodes.len() < DN_NR_MAX_NODES {
                nodes.push(NrNode {
                    on: true,
                    freq: x_to_freq(pos.x).clamp(20.0, 20_000.0),
                    depth_db: y_to_depth(pos.y).clamp(0.5, 48.0),
                    q: 2.2,
                });
                *selected = nodes.len() - 1;
                changed = true;
            }
        }
    } else if resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            if let Some(i) = hit_node(pos, nodes) {
                *selected = i;
            }
        }
    }

    // Scroll: depth on node; Alt+scroll = Q.
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        let alt = ui.input(|i| i.modifiers.alt);
        if scroll.abs() > 0.0 && !nodes.is_empty() {
            let i = if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                hit_node(pos, nodes).unwrap_or(*selected)
            } else {
                *selected
            };
            *selected = i.min(nodes.len().saturating_sub(1));
            if let Some(n) = nodes.get_mut(*selected) {
                if alt {
                    let step = if scroll > 0.0 { -0.12 } else { 0.12 };
                    let nq = (n.q + step).clamp(0.3, 12.0);
                    if (nq - n.q).abs() > 1e-3 {
                        n.q = nq;
                        changed = true;
                    }
                } else {
                    let step = if scroll > 0.0 { -0.4 } else { 0.4 };
                    let nd = (n.depth_db + step).clamp(0.0, 48.0);
                    if (nd - n.depth_db).abs() > 1e-3 {
                        n.depth_db = nd;
                        changed = true;
                    }
                }
            }
        }
    }

    // Delete selected node.
    let focused = ui.ctx().data(|d| d.get_temp::<bool>(ui.id().with("dn_nr_focus")).unwrap_or(false));
    if (resp.hovered() || focused)
        && !nodes.is_empty()
        && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
    {
        if true {
            let i = (*selected).min(nodes.len() - 1);
            nodes.remove(i);
            if !nodes.is_empty() {
                *selected = i.min(nodes.len() - 1);
            } else {
                *selected = 0;
            }
            changed = true;
        }
    }
    if resp.hovered() || resp.dragged() || resp.clicked() {
        ui.ctx()
            .data_mut(|d| d.insert_temp(ui.id().with("dn_nr_focus"), true));
    }

    ui.ctx().data_mut(|d| d.insert_temp(drag_id, drag));
    if drag != NrDragKind::None {
        hide_cursor_on_drag(ui, &resp);
        ui.ctx().request_repaint();
    }
    let _ = pending_new;
    changed
}

/// Legacy PEQ graph — superseded by [`crate::design::eq_chart`].
#[allow(dead_code)]
pub fn peq_graph(
    ui: &mut Ui,
    theme: &dyn Theme,
    bands: &mut [PeqBand],
    out_gain_db: f32,
    selected: &mut usize,
    size: Vec2,
    spectrum_peak_db: f32,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    {
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(2), theme.bg_chart());
        painter.rect_stroke(
            rect,
            CornerRadius::same(2),
            Stroke::new(1.0_f32, theme.border_soft()),
            egui::StrokeKind::Inside,
        );
    }

    let pad_l = 8.0;
    let pad_r = 28.0;
    let pad_t = 18.0;
    let pad_b = 18.0;
    let plot = Rect::from_min_max(
        egui::pos2(rect.left() + pad_l, rect.top() + pad_t),
        egui::pos2(rect.right() - pad_r, rect.bottom() - pad_b),
    );

    const F_MIN: f32 = 20.0;
    const F_MAX: f32 = 20000.0;
    const G_MIN: f32 = -18.0;
    const G_MAX: f32 = 18.0;
    let sr = 48000.0_f32;

    let freq_to_x = |f: f32| {
        let t = ((f.max(F_MIN).ln() - F_MIN.ln()) / (F_MAX.ln() - F_MIN.ln())).clamp(0.0, 1.0);
        plot.left() + t * plot.width()
    };
    let x_to_freq = |x: f32| {
        let t = ((x - plot.left()) / plot.width()).clamp(0.0, 1.0);
        (F_MIN.ln() + t * (F_MAX.ln() - F_MIN.ln())).exp()
    };
    let gain_to_y = |g: f32| {
        let t = ((g - G_MIN) / (G_MAX - G_MIN)).clamp(0.0, 1.0);
        plot.bottom() - t * plot.height()
    };
    let y_to_gain = |y: f32| {
        let t = ((plot.bottom() - y) / plot.height()).clamp(0.0, 1.0);
        G_MIN + t * (G_MAX - G_MIN)
    };

    paint_peak_spectrum(ui, plot, theme, spectrum_peak_db, F_MIN, F_MAX, "peq");

    let painter = ui.painter();
    let grid = theme.border_soft().gamma_multiply(0.85);
    let zero_line = theme.accent().gamma_multiply(0.55);
    for &f in &[50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0] {
        let x = freq_to_x(f);
        painter.line_segment(
            [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
            Stroke::new(1.0_f32, grid),
        );
    }
    for g in [-12i32, -6, 0, 6, 12] {
        let y = gain_to_y(g as f32);
        let col = if g == 0 { zero_line } else { grid };
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            Stroke::new(if g == 0 { 1.25_f32 } else { 1.0_f32 }, col),
        );
        painter.text(
            egui::pos2(plot.right() + 2.0, y),
            egui::Align2::LEFT_CENTER,
            format!("{g:+}"),
            egui::FontId::proportional(9.0),
            if g == 0 {
                theme.accent()
            } else {
                theme.text_muted()
            },
        );
    }
    for &(f, label) in &[
        (20.0, "20"),
        (100.0, "100"),
        (1000.0, "1k"),
        (10000.0, "10k"),
    ] {
        painter.text(
            egui::pos2(freq_to_x(f), plot.bottom() + 2.0),
            egui::Align2::CENTER_TOP,
            label,
            egui::FontId::proportional(9.0),
            theme.text_muted(),
        );
    }
    for &(label, f) in &[
        ("LOW", 60.0),
        ("MID", 1000.0),
        ("HIGH", 8000.0),
    ] {
        painter.text(
            egui::pos2(freq_to_x(f), plot.top() - 2.0),
            egui::Align2::CENTER_BOTTOM,
            label,
            egui::FontId::proportional(9.0),
            theme.text_dim(),
        );
    }

    // Frequency-response curve over the spectrum.
    let mut pts = Vec::with_capacity(128);
    for i in 0..128 {
        let t = i as f32 / 127.0;
        let f = (F_MIN.ln() + t * (F_MAX.ln() - F_MIN.ln())).exp();
        let db = peq_mag_db_at(bands, out_gain_db, f, sr).clamp(G_MIN, G_MAX);
        pts.push(egui::pos2(freq_to_x(f), gain_to_y(db)));
    }
    let curve = theme.accent().gamma_multiply(0.95);
    for w in pts.windows(2) {
        painter.line_segment([w[0], w[1]], Stroke::new(2.15_f32, curve));
    }

    let mut changed = false;
    let pick_nearest = |bands: &[PeqBand], pos: egui::Pos2, max_d: f32| -> Option<usize> {
        let mut best = None;
        let mut best_d = max_d;
        for (i, b) in bands.iter().enumerate() {
            let p = egui::pos2(freq_to_x(b.freq), gain_to_y(b.gain_db.clamp(G_MIN, G_MAX)));
            let d = pos.distance(p);
            if d < best_d {
                best_d = d;
                best = Some(i);
            }
        }
        best
    };

    let drag_key = ui.id().with("peq_drag_band");
    if resp.drag_started() {
        let hit = resp
            .interact_pointer_pos()
            .and_then(|pos| pick_nearest(bands, pos, 20.0));
        if let Some(i) = hit {
            *selected = i;
            ui.ctx().data_mut(|d| d.insert_temp(drag_key, i));
        } else {
            ui.ctx().data_mut(|d| d.remove_temp::<usize>(drag_key));
        }
    }
    let active_drag: Option<usize> = ui.ctx().data(|d| d.get_temp(drag_key));
    if resp.dragged() {
        if let (Some(i), Some(pos)) = (active_drag, resp.interact_pointer_pos()) {
            if let Some(b) = bands.get_mut(i) {
                b.freq = x_to_freq(pos.x).clamp(20.0, 20000.0);
                let mut g = y_to_gain(pos.y).clamp(-24.0, 24.0);
                if g.abs() < 0.5 {
                    g = 0.0; // snap to 0 dB
                }
                b.gain_db = g;
                changed = true;
            }
        }
    }
    if resp.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove_temp::<usize>(drag_key));
    }
    if resp.clicked() && !resp.dragged() {
        if let Some(pos) = resp.interact_pointer_pos() {
            if let Some(i) = pick_nearest(bands, pos, 16.0) {
                *selected = i;
            }
        }
    }
    hide_cursor_on_drag(ui, &resp);

    // Nodes — muted steel/champagne markers
    for (i, b) in bands.iter().enumerate() {
        let col = PEQ_BAND_COLORS[i % PEQ_BAND_COLORS.len()];
        let p = egui::pos2(freq_to_x(b.freq), gain_to_y(b.gain_db.clamp(G_MIN, G_MAX)));
        let r = if *selected == i { 7.0 } else { 5.5 };
        if b.on {
            painter.circle_filled(p, r, col);
            painter.circle_stroke(
                p,
                r,
                Stroke::new(
                    1.25_f32,
                    if *selected == i {
                        theme.accent()
                    } else {
                        theme.border()
                    },
                ),
            );
        } else {
            painter.circle_stroke(p, r, Stroke::new(1.5_f32, col.gamma_multiply(0.65)));
        }
        painter.text(
            p,
            egui::Align2::CENTER_CENTER,
            format!("{}", i + 1),
            egui::FontId::proportional(9.0),
            if b.on {
                theme.bg_app()
            } else {
                theme.text_muted()
            },
        );
    }

    changed
}

/// Legacy track analyzer — superseded by [`crate::design::eq_chart`].
#[allow(dead_code)]
pub fn track_analyzer(
    ui: &mut Ui,
    theme: &dyn Theme,
    peak_db: f32,
    bands: Option<&[PeqBand]>,
    out_gain_db: f32,
    size: Vec2,
) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    {
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(2), theme.bg_chart());
        painter.rect_stroke(
            rect,
            CornerRadius::same(2),
            Stroke::new(1.0_f32, theme.border_soft()),
            egui::StrokeKind::Inside,
        );
    }

    let plot = rect.shrink2(egui::vec2(8.0, 6.0));
    if plot.width() < 8.0 || plot.height() < 8.0 {
        return;
    }

    const F_MIN: f32 = 20.0;
    const F_MAX: f32 = 20000.0;
    let a = theme.accent();
    let grid = theme.border_soft().gamma_multiply(0.75);

    paint_peak_spectrum(ui, plot, theme, peak_db, F_MIN, F_MAX, "track");

    let painter = ui.painter();
    let freq_to_x = |f: f32| {
        let t = ((f.max(F_MIN).ln() - F_MIN.ln()) / (F_MAX.ln() - F_MIN.ln())).clamp(0.0, 1.0);
        plot.left() + t * plot.width()
    };
    // EQ overlay: 0 dB at top, −36 dB floor (same as spectrum ceiling).
    let eq_to_y = |db: f32| {
        let t = ((db.clamp(-36.0, 0.0) + 36.0) / 36.0).clamp(0.0, 1.0);
        plot.bottom() - t * plot.height()
    };

    for &f in &[100.0, 1000.0, 10000.0] {
        let x = freq_to_x(f);
        painter.line_segment(
            [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
            Stroke::new(1.0_f32, grid),
        );
    }
    painter.line_segment(
        [egui::pos2(plot.left(), plot.top()), egui::pos2(plot.right(), plot.top())],
        Stroke::new(1.25_f32, a.gamma_multiply(0.55)),
    );
    painter.text(
        egui::pos2(plot.right() - 2.0, plot.top() + 1.0),
        egui::Align2::RIGHT_TOP,
        "0 dB",
        egui::FontId::proportional(9.0),
        theme.text_muted(),
    );

    if let Some(bands) = bands {
        let mut pts = Vec::with_capacity(96);
        for i in 0..96 {
            let t = i as f32 / 95.0;
            let f = (F_MIN.ln() + t * (F_MAX.ln() - F_MIN.ln())).exp();
            let db = peq_mag_db_at(bands, out_gain_db, f, 48_000.0);
            pts.push(egui::pos2(freq_to_x(f), eq_to_y(db)));
        }
        let curve = Color32::from_rgb(0xf0, 0xf2, 0xf5);
        for w in pts.windows(2) {
            painter.line_segment([w[0], w[1]], Stroke::new(1.6_f32, curve));
        }
    }
}
