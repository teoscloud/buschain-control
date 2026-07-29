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

/// Power / enable toggle for inserts. `on == true` means the plugin is active (not bypassed).
pub fn power_toggle(
    ui: &mut Ui,
    theme: &dyn Theme,
    on: &mut bool,
) -> egui::Response {
    let fill = if *on {
        theme.success().gamma_multiply(0.35)
    } else {
        theme.bg_well()
    };
    let stroke = Stroke::new(
        1.0_f32,
        if *on {
            theme.success()
        } else {
            theme.border()
        },
    );
    let icon = egui_phosphor::regular::POWER;
    let color = if *on {
        theme.success()
    } else {
        theme.text_muted()
    };
    let mut resp = ui
        .add(
            egui::Button::new(RichText::new(icon).size(15.0).color(color))
                .fill(fill)
                .stroke(stroke)
                .corner_radius(theme.rounding())
                .min_size(Vec2::new(28.0, 24.0)),
        )
        .on_hover_text(if *on {
            "Disable insert"
        } else {
            "Enable insert"
        });
    if resp.clicked() {
        *on = !*on;
        // Buttons don't set changed by default — callers rely on .changed().
        resp.mark_changed();
    }
    resp
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
    let track_w = (size.x * 0.22).clamp(4.0, 7.0);
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
    if lo < 0.0 && hi > 0.0 && track.height() > 40.0 {
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

    // Metallic console thumb — light gray aluminum; accent only on the 0 dB edge.
    let at_zero = value_db.abs() < 0.05;
    let above_zero = *value_db > 0.05;
    let cap_fill = if above_zero {
        Color32::from_rgb(0xd0, 0xb0, 0x70)
    } else if at_zero {
        Color32::from_rgb(0xf2, 0xf3, 0xf5)
    } else {
        theme.fader_cap()
    };
    let cap_w = (size.x * 0.85).clamp(16.0, 22.0);
    let cap = Rect::from_center_size(egui::pos2(track.center().x, y), Vec2::new(cap_w, 9.0));
    painter.rect_filled(cap, CornerRadius::same(2), cap_fill);
    // Soft highlight on top edge of thumb
    painter.hline(
        egui::Rangef::new(cap.left() + 2.0, cap.right() - 2.0),
        cap.top() + 1.5,
        Stroke::new(1.0_f32, Color32::from_rgb(0xff, 0xff, 0xff).gamma_multiply(0.35)),
    );
    painter.rect_stroke(
        cap,
        CornerRadius::same(2),
        Stroke::new(
            1.0_f32,
            if at_zero {
                theme.accent()
            } else if above_zero {
                theme.warning()
            } else {
                Color32::from_rgb(0x6a, 0x6e, 0x74)
            },
        ),
        egui::StrokeKind::Outside,
    );
    // Cap grip line
    painter.hline(
        egui::Rangef::new(cap.left() + 3.0, cap.right() - 3.0),
        cap.center().y,
        Stroke::new(1.0_f32, Color32::from_rgb(0x55, 0x58, 0x5e)),
    );

    // Redraw 0 dB rail on top of fill so it stays visible under the trough
    if lo < 0.0 && hi > 0.0 {
        let y0 = db_to_y(0.0);
        painter.hline(
            egui::Rangef::new(rect.left() + 1.0, rect.right() - 1.0),
            y0,
            Stroke::new(1.75_f32, theme.accent().gamma_multiply(0.95)),
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
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (i, name) in tabs.iter().enumerate() {
            let on = *selected == i;
            let text = if on {
                theme.text()
            } else {
                theme.text_muted()
            };
            let resp = ui.add(
                egui::Button::new(RichText::new(*name).size(12.0).strong().color(text))
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::NONE)
                    .corner_radius(CornerRadius::ZERO),
            );
            if on {
                let r = resp.rect;
                ui.painter().hline(
                    egui::Rangef::new(r.left() + 2.0, r.right() - 2.0),
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
        let resp = ui.add(egui::Slider::new(value, range).show_value(true));
        hide_cursor_on_drag(ui, &resp);
        resp
    })
    .inner
}

/// egui slider that hides the cursor while dragging.
pub fn slider_drag(
    ui: &mut Ui,
    slider: egui::Slider<'_>,
) -> egui::Response {
    let resp = ui.add(slider);
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
            let r = size * 0.42;
            painter.circle_filled(rect.center(), r, theme.bg_well());
            painter.circle_stroke(rect.center(), r, Stroke::new(1.5_f32, theme.border()));
            let t = (*value - *range.start()) / (*range.end() - *range.start()).max(1e-6);
            let ang = -std::f32::consts::FRAC_PI_2 - 0.75 * std::f32::consts::PI
                + t.clamp(0.0, 1.0) * 1.5 * std::f32::consts::PI;
            let tip = rect.center() + Vec2::angled(ang) * (r * 0.75);
            painter.line_segment(
                [rect.center(), tip],
                Stroke::new(2.5_f32, Color32::WHITE),
            );
            painter.circle_filled(rect.center(), 2.5, Color32::WHITE);
            if resp.dragged() {
                let sens = drag_sensitivity(ui);
                let span = *range.end() - *range.start();
                let px_for_full = (size * 1.4).max(48.0);
                let delta = -resp.drag_delta().y * (span / px_for_full) * sens;
                *value = (*value + delta).clamp(*range.start(), *range.end());
                resp.mark_changed();
            }
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

/// Soft-clipper transfer plot with a **fixed** 0 dB reference (does not rescale Y to gain).
/// Drag horizontally to set Threshold.
pub fn softclip_transfer_plot(
    ui: &mut Ui,
    theme: &dyn Theme,
    threshold: &mut f32,
    post: f32,
    size: Vec2,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), Color32::from_rgb(0x36, 0x47, 0x4f));
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0_f32, Color32::from_rgb(0x4a, 0x5c, 0x66)),
        egui::StrokeKind::Inside,
    );

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
    // Fixed input span past 0 dBFS; Y is fixed to +6 dB (2×) unless gain is hotter.
    let view_in = 1.2_f32;
    let view_out = post.max(2.0) * 1.02; // at least 0…+6 dB so 0 dB never sits on the top edge
    let to_px = |x: f32, y: f32| {
        egui::pos2(
            plot.left() + (x / view_in).clamp(0.0, 1.0) * plot.width(),
            plot.bottom() - (y / view_out).clamp(0.0, 1.0) * plot.height(),
        )
    };

    // Hot zone above 0 dBFS out
    let zero_y = to_px(0.0, 1.0).y;
    if 1.0 < view_out {
        let hot = Rect::from_min_max(
            egui::pos2(plot.left(), plot.top()),
            egui::pos2(plot.right(), zero_y),
        );
        painter.rect_filled(
            hot,
            0.0,
            Color32::from_rgba_unmultiplied(180, 60, 40, 28),
        );
    }

    let grid = Color32::from_rgba_unmultiplied(180, 200, 210, 40);
    for i in 0..=4 {
        let t = i as f32 / 4.0;
        let x = plot.left() + plot.width() * t;
        painter.line_segment(
            [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
            Stroke::new(1.0_f32, grid),
        );
    }
    // Horizontal grid at useful dB marks: -12, -6, 0, +6 (and +12 if in view)
    for db in [-12.0_f32, -6.0, 0.0, 6.0, 12.0] {
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
                if is_zero { 1.5 } else { 1.0 },
                if is_zero {
                    Color32::from_rgb(0xe8, 0xc4, 0x4a)
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
                Color32::from_rgb(0xe8, 0xc4, 0x4a)
            } else {
                theme.text_muted()
            },
        );
    }

    // 0 dBFS input
    let zero_x = to_px(1.0, 0.0).x;
    painter.vline(
        zero_x,
        plot.y_range(),
        Stroke::new(1.5_f32, Color32::from_rgb(0xe8, 0xc4, 0x4a)),
    );

    // Ceiling (post × full-scale clipped output)
    let ceil = post;
    if ceil > 1e-4 {
        let cy = to_px(0.0, ceil.min(view_out)).y;
        painter.hline(
            plot.x_range(),
            cy,
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(240, 240, 245, 90)),
        );
    }

    // Unity reference (y = x) below threshold region, faint
    {
        let a = to_px(0.0, 0.0);
        let b = to_px(1.0_f32.min(view_in), 1.0_f32.min(view_out));
        painter.line_segment(
            [a, b],
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(160, 180, 190, 55)),
        );
    }

    let mut pts = Vec::with_capacity(97);
    for i in 0..=96 {
        let x = view_in * (i as f32 / 96.0);
        let y = softclip_xfer(x, thres, post);
        pts.push((x, y, to_px(x, y.clamp(0.0, view_out))));
    }
    for w in pts.windows(2) {
        let above = w[0].1 > 1.0 || w[1].1 > 1.0;
        let col = if above {
            Color32::from_rgb(0xf0, 0x8a, 0x6a) // hot when above 0 dBFS out
        } else {
            Color32::from_rgb(0xf2, 0xf5, 0xf7)
        };
        painter.line_segment([w[0].2, w[1].2], Stroke::new(2.4_f32, col));
    }

    let knee = to_px(thres, softclip_xfer(thres, thres, post).clamp(0.0, view_out));
    painter.circle_filled(knee, 3.2, Color32::from_rgb(0xe8, 0xec, 0xf0));

    // Axis captions + gain readout
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
        Color32::from_rgb(0xe8, 0xc4, 0x4a),
    );
    let gain_txt = format!("ceil {}", lin_to_db_label(post.max(1e-6)));
    let gain_col = if post > 1.001 {
        Color32::from_rgb(0xf0, 0x8a, 0x6a)
    } else if post < 0.999 {
        theme.text_dim()
    } else {
        Color32::from_rgb(0xe8, 0xc4, 0x4a)
    };
    painter.text(
        egui::pos2(plot.right() - 2.0, rect.top() + 2.0),
        egui::Align2::RIGHT_TOP,
        gain_txt,
        egui::FontId::proportional(10.0),
        gain_col,
    );

    let mut changed = false;
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
    hide_cursor_on_drag(ui, &resp);
    let _ = theme;
    changed
}

/// Narrow stereo output meters (FL Soft Clipper center strip).
pub fn softclip_meters(ui: &mut Ui, level_db: f32, size: Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), Color32::from_rgb(0x22, 0x26, 0x2a));

    let green = Color32::from_rgb(0x95, 0xd6, 0x4a);
    let peak_col = Color32::from_rgb(0xc4, 0x8a, 0x3a);
    let min_db = -48.0_f32;
    let max_db = 6.0_f32;
    let t = ((level_db.clamp(min_db, max_db) - min_db) / (max_db - min_db)).clamp(0.0, 1.0);
    let gap = 3.0;
    let bar_w = ((rect.width() - gap) * 0.5).max(3.0);
    for i in 0..2 {
        let x0 = rect.left() + 2.0 + i as f32 * (bar_w + gap);
        let well = Rect::from_min_max(
            egui::pos2(x0, rect.top() + 3.0),
            egui::pos2(x0 + bar_w, rect.bottom() - 3.0),
        );
        painter.rect_filled(well, CornerRadius::same(1), Color32::from_rgb(0x18, 0x1c, 0x20));
        let fill_h = well.height() * t;
        let fill = Rect::from_min_max(
            egui::pos2(well.left(), well.bottom() - fill_h),
            egui::pos2(well.right(), well.bottom()),
        );
        if fill_h > 0.5 {
            painter.rect_filled(fill, CornerRadius::same(1), green);
        }
        // Peak tick
        let py = well.bottom() - fill_h;
        painter.hline(
            egui::Rangef::new(well.left(), well.right()),
            py,
            Stroke::new(1.5_f32, peak_col),
        );
    }
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

fn peq_biquad_coeffs(b: &PeqBand, sr: f32) -> (f32, f32, f32, f32, f32) {
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

fn peq_mag_db_at(bands: &[PeqBand], out_gain_db: f32, freq_hz: f32, sr: f32) -> f32 {
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

/// Interactive parametric EQ graph (BusChain console theme). Returns true if any band changed.
/// `selected` is 0..n bands (which node is focused for side controls).
pub fn peq_graph(
    ui: &mut Ui,
    theme: &dyn Theme,
    bands: &mut [PeqBand],
    out_gain_db: f32,
    selected: &mut usize,
    size: Vec2,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), theme.bg_well());
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0_f32, theme.border_soft()),
        egui::StrokeKind::Inside,
    );

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
            Stroke::new(if g == 0 { 1.25 } else { 1.0 }, col),
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

    // Response curve — champagne accent, not neon white
    let mut pts = Vec::with_capacity(128);
    for i in 0..128 {
        let t = i as f32 / 127.0;
        let f = (F_MIN.ln() + t * (F_MAX.ln() - F_MIN.ln())).exp();
        let db = peq_mag_db_at(bands, out_gain_db, f, sr).clamp(G_MIN, G_MAX);
        pts.push(egui::pos2(freq_to_x(f), gain_to_y(db)));
    }
    let curve = theme.accent().gamma_multiply(0.92);
    for w in pts.windows(2) {
        painter.line_segment([w[0], w[1]], Stroke::new(2.0_f32, curve));
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
