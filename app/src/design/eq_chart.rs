//! Shared Equalizer / Analyzer chart — real FFT spectrum + interactive EQ curve.

use egui::epaint::{Mesh, PathShape, PathStroke};
use egui::{Color32, CornerRadius, Pos2, Rect, RichText, Sense, Shape, Stroke, Ui, Vec2};

use super::tokens::Theme;
use super::widgets::{hide_cursor_on_drag, peq_mag_db_at, PeqBand, PEQ_BAND_COLORS};

/// Log-spaced display columns. Kept modest — paint cost scales with this.
pub const EQ_DISPLAY_COLS: usize = 256;
const F_MIN: f32 = 20.0;
const F_MAX: f32 = 20_000.0;
/// Interactive EQ gain window (insert editor).
const G_MIN: f32 = -24.0;
const G_MAX: f32 = 24.0;
/// Absolute silence floor for FFT samples.
const SPEC_FLOOR_DB: f32 = -96.0;
/// Analyzer / scope Y window — full dynamic range so bass-heavy spectra still breathe.
const SPEC_VIEW_MIN: f32 = -78.0;
const SPEC_VIEW_MAX: f32 = 2.0;

#[derive(Clone, Copy, Debug)]
pub struct EqChartChrome {
    pub post: bool,
    pub peak_hold: bool,
    pub freeze: bool,
    /// Mixer bottom scope — edge-to-edge, solid well, no frame/wash.
    pub full_bleed: bool,
}

impl Default for EqChartChrome {
    fn default() -> Self {
        Self {
            post: true,
            peak_hold: true,
            freeze: false,
            full_bleed: false,
        }
    }
}

/// Toolbar: Pre/Post · PeakHold · Freeze · peak readout.
pub fn eq_chart_chrome(
    ui: &mut Ui,
    theme: &dyn Theme,
    track_name: &str,
    peak_db: f32,
    chrome: &mut EqChartChrome,
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(track_name)
                .size(11.0)
                .color(theme.text_dim()),
        );
        ui.add_space(8.0);
        let pre = !chrome.post;
        if ui
            .selectable_label(pre, RichText::new("Pre").size(10.0))
            .on_hover_text("Spectrum before inserts")
            .clicked()
        {
            chrome.post = false;
        }
        if ui
            .selectable_label(chrome.post, RichText::new("Post").size(10.0))
            .on_hover_text("Spectrum after FX rack")
            .clicked()
        {
            chrome.post = true;
        }
        ui.add_space(6.0);
        if ui
            .selectable_label(chrome.peak_hold, RichText::new("Hold").size(10.0))
            .on_hover_text("Peak-hold ghost")
            .clicked()
        {
            chrome.peak_hold = !chrome.peak_hold;
        }
        if ui
            .selectable_label(chrome.freeze, RichText::new("Freeze").size(10.0))
            .on_hover_text("Freeze spectrum ballistics")
            .clicked()
        {
            chrome.freeze = !chrome.freeze;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let txt = if peak_db <= -89.0 {
                "— dB".into()
            } else {
                format!("{peak_db:+.0} dB")
            };
            ui.label(
                RichText::new(txt)
                    .size(10.0)
                    .monospace()
                    .color(theme.text_muted()),
            );
        });
    });
}

const PHOS_HIST: usize = 10;

#[derive(Clone)]
struct SpecPaintState {
    cols: Vec<f32>,
    hold: Vec<f32>,
    /// Per-column phosphor glow (0…1) — decays when quiet, pops on attack.
    glow: Vec<f32>,
    /// Rolling crest history: `PHOS_HIST * N` dB samples for mint trails.
    hist: Vec<f32>,
    hist_i: u8,
    /// Last resampled FFT targets (only rebuilt when `last_gen` changes).
    targets: Vec<f32>,
    ctrl_log: Vec<f32>,
    ctrl_db: Vec<f32>,
    last_gen: u64,
    last_t: f64,
}

/// Spectrum bin energy → phosphor heat (crest-colored columns, not dB floor bands).
/// Quiet bins stay cool mint; hot bins push amber → brick → lava.
fn spec_heat(db: f32) -> f32 {
    let db = db.clamp(SPEC_FLOOR_DB, SPEC_VIEW_MAX + 6.0);
    if db <= -48.0 {
        0.10
    } else if db <= -24.0 {
        0.10 + ((db + 48.0) / 24.0) * 0.18 // → 0.28 mint
    } else if db <= -12.0 {
        0.28 + ((db + 24.0) / 12.0) * 0.14 // → 0.42 amber
    } else if db <= -3.0 {
        0.42 + ((db + 12.0) / 9.0) * 0.20 // → 0.62 brick
    } else if db <= 0.0 {
        0.62 + ((db + 3.0) / 3.0) * 0.16 // → 0.78 fire
    } else {
        0.78 + (db / 6.0).clamp(0.0, 1.0) * 0.22 // lava
    }
}

/// Draw interactive (or read-only) EQ chart with real FFT backdrop.
/// `bands == None` → analyzer-only. Returns true if bands changed.
pub fn eq_chart(
    ui: &mut Ui,
    theme: &dyn Theme,
    bands: Option<&mut [PeqBand]>,
    out_gain_db: f32,
    selected: &mut usize,
    size: Vec2,
    spectrum_mags: Option<&[f32]>,
    spectrum_sr: f32,
    spectrum_gen: u64,
    chrome: &EqChartChrome,
    id_salt: impl std::hash::Hash,
) -> bool {
    let salt_id = ui.id().with(("eq_chart_salt", id_salt));
    let editable = bands.is_some();
    let sense = if editable {
        Sense::click_and_drag()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let full_bleed = chrome.full_bleed && bands.is_none();
    {
        let painter = ui.painter();
        // Solid well near the axis color — no vignette / double-frame.
        let fill = if bands.is_some() {
            theme.bg_chart()
        } else {
            // Quiet well — near panel chrome, not a vivid chart black.
            Color32::from_rgb(0x1a, 0x1c, 0x20)
        };
        let r = if full_bleed {
            CornerRadius::ZERO
        } else {
            theme.rounding()
        };
        painter.rect_filled(rect, r, fill);
        if !full_bleed {
            painter.rect_stroke(
                rect,
                r,
                Stroke::new(1.0_f32, theme.border_soft()),
                egui::StrokeKind::Inside,
            );
        }
    }

    let (pad_l, pad_r, pad_t, pad_b) = if full_bleed {
        (4.0, 28.0, 4.0, 16.0)
    } else {
        (10.0, 34.0, 16.0, 20.0)
    };
    let plot = Rect::from_min_max(
        egui::pos2(rect.left() + pad_l, rect.top() + pad_t),
        egui::pos2(rect.right() - pad_r, rect.bottom() - pad_b),
    );
    if plot.width() < 16.0 || plot.height() < 16.0 {
        return false;
    }

    let sr = if spectrum_sr > 1.0 {
        spectrum_sr
    } else {
        48_000.0
    };

    let analyzer = bands.is_none();
    let freq_to_x = |f: f32| {
        let t = ((f.max(F_MIN).ln() - F_MIN.ln()) / (F_MAX.ln() - F_MIN.ln())).clamp(0.0, 1.0);
        plot.left() + t * plot.width()
    };
    let x_to_freq = |x: f32| {
        let t = ((x - plot.left()) / plot.width()).clamp(0.0, 1.0);
        (F_MIN.ln() + t * (F_MAX.ln() - F_MIN.ln())).exp()
    };
    // Insert EQ: ±24 gain. Analyzer/scope: track-level dBFS (−48…+1).
    let gain_to_y = |g: f32| {
        let t = ((g - G_MIN) / (G_MAX - G_MIN)).clamp(0.0, 1.0);
        plot.bottom() - t * plot.height()
    };
    let y_to_gain = |y: f32| {
        let t = ((plot.bottom() - y) / plot.height()).clamp(0.0, 1.0);
        G_MIN + t * (G_MAX - G_MIN)
    };
    let spec_span = (SPEC_VIEW_MAX - SPEC_VIEW_MIN).max(0.001);
    let spec_db_to_y = |db: f32| {
        let t = ((db.clamp(SPEC_VIEW_MIN, SPEC_VIEW_MAX) - SPEC_VIEW_MIN) / spec_span)
            .clamp(0.0, 1.0);
        plot.bottom() - t * plot.height()
    };
    let y_to_spec_db = |y: f32| {
        let t = ((plot.bottom() - y) / plot.height()).clamp(0.0, 1.0);
        SPEC_VIEW_MIN + t * spec_span
    };
    let db_grid_to_y = |db: f32| {
        if analyzer {
            spec_db_to_y(db)
        } else {
            gain_to_y(db)
        }
    };

    // Axis label sizes (~+10% vs prior 8/9) and slightly stronger chroma.
    let axis_font = egui::FontId::proportional(if full_bleed { 9.0 } else { 10.0 });
    let axis_col = if full_bleed {
        Color32::from_rgba_unmultiplied(
            theme.text_dim().r(),
            theme.text_dim().g(),
            theme.text_dim().b(),
            150,
        )
    } else {
        theme.text_dim()
    };
    let axis_col_zero = if full_bleed {
        Color32::from_rgba_unmultiplied(
            theme.text().r(),
            theme.text().g(),
            theme.text().b(),
            175,
        )
    } else if analyzer {
        theme.text()
    } else {
        theme.accent()
    };

    // Grid first (under spectrum + EQ) so it doesn't cut through the visuals.
    {
        let painter = ui.painter();
        // Quieter rails — labels carry orientation, not the mesh.
        let grid = if full_bleed {
            Color32::from_rgba_unmultiplied(
                theme.text().r(),
                theme.text().g(),
                theme.text().b(),
                5,
            )
        } else if analyzer {
            Color32::from_rgba_unmultiplied(
                theme.border().r(),
                theme.border().g(),
                theme.border().b(),
                12,
            )
        } else {
            theme.border_soft().gamma_multiply(0.20)
        };
        let zero_line = if full_bleed {
            Color32::from_rgba_unmultiplied(
                theme.text().r(),
                theme.text().g(),
                theme.text().b(),
                14,
            )
        } else if analyzer {
            Color32::from_rgba_unmultiplied(
                theme.text().r(),
                theme.text().g(),
                theme.text().b(),
                28,
            )
        } else {
            theme.accent().gamma_multiply(0.20)
        };
        for &f in &[
            50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0,
        ] {
            let x = freq_to_x(f);
            painter.line_segment(
                [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
                Stroke::new(1.0_f32, grid),
            );
        }
        // Analyzer: full-range dBFS. Insert EQ: ±24 gain rails.
        let db_marks: &[f32] = if analyzer {
            &[-78.0, -48.0, -24.0, -12.0, 0.0, 2.0]
        } else {
            &[-24.0, -12.0, -6.0, 0.0, 6.0, 12.0, 24.0]
        };
        for &g in db_marks {
            let y = db_grid_to_y(g);
            let is_zero = g.abs() < 0.05;
            let col = if is_zero { zero_line } else { grid };
            painter.line_segment(
                [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
                Stroke::new(if is_zero { 1.0_f32 } else { 1.0 }, col),
            );
            let label_major = if analyzer {
                is_zero
                    || g == -78.0
                    || g == -48.0
                    || g == -24.0
                    || g == 2.0
            } else {
                is_zero || g % 12.0 == 0.0 || g.abs() == 6.0
            };
            if label_major {
                let label = if analyzer && (g - 2.0).abs() < 0.05 {
                    "+2".into()
                } else {
                    format!("{g:+.0}")
                };
                painter.text(
                    egui::pos2(plot.right() + 3.0, y),
                    egui::Align2::LEFT_CENTER,
                    label,
                    axis_font.clone(),
                    if is_zero { axis_col_zero } else { axis_col },
                );
            }
        }
        for &(f, label) in &[
            (20.0, "20"),
            (50.0, "50"),
            (100.0, "100"),
            (200.0, "200"),
            (500.0, "500"),
            (1000.0, "1k"),
            (2000.0, "2k"),
            (5000.0, "5k"),
            (10000.0, "10k"),
            (20000.0, "20k"),
        ] {
            painter.text(
                egui::pos2(freq_to_x(f), plot.bottom() + 2.0),
                egui::Align2::CENTER_TOP,
                label,
                axis_font.clone(),
                axis_col,
            );
        }
    }

    // Spectrum above grid.
    paint_fft_columns(
        ui,
        plot,
        theme,
        spectrum_mags,
        spectrum_gen,
        sr,
        chrome,
        salt_id,
        spec_db_to_y,
    );

    let painter = ui.painter();
    let Some(bands) = bands else {
        if spectrum_mags.is_none() {
            painter.text(
                plot.center(),
                egui::Align2::CENTER_CENTER,
                "no host spectrum",
                egui::FontId::proportional(12.0),
                theme.text_muted(),
            );
        }
        paint_eq_crosshair(
            ui,
            &resp,
            plot,
            theme,
            &axis_font,
            |x| x_to_freq(x),
            y_to_spec_db,
        );
        if !chrome.freeze {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(48));
        }
        return false;
    };

    // Selected-band ghost only (was every band × 96 mag evals).
    if let Some(b) = bands.get(*selected).filter(|b| b.on) {
        let col = PEQ_BAND_COLORS[(*selected) % PEQ_BAND_COLORS.len()].gamma_multiply(0.4);
        let single = [*b];
        let mut ghost = Vec::with_capacity(49);
        for k in 0..48 {
            let t = k as f32 / 47.0;
            let f = (F_MIN.ln() + t * (F_MAX.ln() - F_MIN.ln())).exp();
            let db = peq_mag_db_at(&single, 0.0, f, sr).clamp(G_MIN, G_MAX);
            ghost.push(egui::pos2(freq_to_x(f), gain_to_y(db)));
        }
        painter.add(PathShape::line(ghost, PathStroke::new(1.0, col)));
    }

    // Composite response.
    const CURVE_N: usize = 96;
    let mut pts = Vec::with_capacity(CURVE_N);
    for i in 0..CURVE_N {
        let t = i as f32 / (CURVE_N - 1) as f32;
        let f = (F_MIN.ln() + t * (F_MAX.ln() - F_MIN.ln())).exp();
        let db = peq_mag_db_at(bands, out_gain_db, f, sr).clamp(G_MIN, G_MAX);
        pts.push(egui::pos2(freq_to_x(f), gain_to_y(db)));
    }
    // Soft fill under curve toward 0 dB — one mesh, not N quads.
    let zero_y = gain_to_y(0.0);
    let fill = theme.accent().gamma_multiply(0.12);
    {
        let mut mesh = Mesh::default();
        for w in pts.windows(2) {
            let i = mesh.vertices.len() as u32;
            mesh.colored_vertex(w[0], fill);
            mesh.colored_vertex(w[1], fill);
            mesh.colored_vertex(egui::pos2(w[1].x, zero_y), fill);
            mesh.colored_vertex(egui::pos2(w[0].x, zero_y), fill);
            mesh.add_triangle(i, i + 1, i + 2);
            mesh.add_triangle(i, i + 2, i + 3);
        }
        painter.add(Shape::mesh(mesh));
    }
    let curve = Color32::from_rgb(0xf2, 0xf4, 0xf8);
    painter.add(PathShape::line(
        pts.clone(),
        PathStroke::new(2.25, curve),
    ));
    painter.add(PathShape::line(
        pts,
        PathStroke::new(1.0, theme.accent().gamma_multiply(0.85)),
    ));

    let mut changed = false;
    if editable {
        let pick_nearest = |bands: &[PeqBand], pos: Pos2, max_d: f32| -> Option<usize> {
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

        let drag_key = salt_id.with("eq_drag");
        let alt_q_key = salt_id.with("eq_alt_q");
        if resp.drag_started() {
            let hit = resp
                .interact_pointer_pos()
                .and_then(|pos| pick_nearest(bands, pos, 22.0));
            if let Some(i) = hit {
                *selected = i;
                ui.ctx().data_mut(|d| d.insert_temp(drag_key, i));
                let alt = ui.input(|i| i.modifiers.alt);
                ui.ctx().data_mut(|d| d.insert_temp(alt_q_key, alt));
            } else {
                ui.ctx().data_mut(|d| d.remove_temp::<usize>(drag_key));
            }
        }
        let active_drag: Option<usize> = ui.ctx().data(|d| d.get_temp(drag_key));
        let alt_q: bool = ui.ctx().data(|d| d.get_temp(alt_q_key)).unwrap_or(false);
        if resp.dragged() {
            if let (Some(i), Some(pos)) = (active_drag, resp.interact_pointer_pos()) {
                if let Some(b) = bands.get_mut(i) {
                    if alt_q {
                        let dy = resp.drag_delta().y;
                        let nq = (b.q * (1.0 - dy * 0.02)).clamp(0.1, 10.0);
                        if (nq - b.q).abs() > 1e-3 {
                            b.q = nq;
                            changed = true;
                        }
                    } else {
                        b.freq = x_to_freq(pos.x).clamp(20.0, 20_000.0);
                        let mut g = y_to_gain(pos.y).clamp(G_MIN, G_MAX);
                        if g.abs() < 0.45 {
                            g = 0.0;
                        }
                        b.gain_db = g;
                        changed = true;
                    }
                }
            }
        }
        if resp.drag_stopped() {
            ui.ctx().data_mut(|d| {
                d.remove_temp::<usize>(drag_key);
                d.remove_temp::<bool>(alt_q_key);
            });
        }
        if resp.double_clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                if let Some(i) = pick_nearest(bands, pos, 18.0) {
                    if let Some(b) = bands.get_mut(i) {
                        b.gain_db = 0.0;
                        *selected = i;
                        changed = true;
                    }
                }
            }
        } else if resp.clicked() && !resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                if let Some(i) = pick_nearest(bands, pos, 16.0) {
                    *selected = i;
                }
            }
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                let i = resp
                    .hover_pos()
                    .and_then(|pos| pick_nearest(bands, pos, 28.0))
                    .unwrap_or(*selected)
                    .min(bands.len().saturating_sub(1));
                *selected = i;
                if let Some(b) = bands.get_mut(i) {
                    let step = if scroll > 0.0 { -0.08 } else { 0.08 };
                    let nq = (b.q + step).clamp(0.1, 10.0);
                    if (nq - b.q).abs() > 1e-3 {
                        b.q = nq;
                        changed = true;
                    }
                }
            }
        }
        hide_cursor_on_drag(ui, &resp);
    }

    // Nodes
    for (i, b) in bands.iter().enumerate() {
        let col = PEQ_BAND_COLORS[i % PEQ_BAND_COLORS.len()];
        let p = egui::pos2(freq_to_x(b.freq), gain_to_y(b.gain_db.clamp(G_MIN, G_MAX)));
        let r = if *selected == i { 7.5 } else { 5.75 };
        if b.on {
            painter.circle_filled(p, r + 2.5, col.gamma_multiply(0.35));
            painter.circle_filled(p, r, col);
            painter.circle_stroke(
                p,
                r,
                Stroke::new(
                    1.35_f32,
                    if *selected == i {
                        theme.accent()
                    } else {
                        theme.border()
                    },
                ),
            );
        } else {
            painter.circle_stroke(p, r, Stroke::new(1.5_f32, col.gamma_multiply(0.55)));
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

    paint_eq_crosshair(
        ui,
        &resp,
        plot,
        theme,
        &axis_font,
        |x| x_to_freq(x),
        y_to_gain,
    );

    if !chrome.freeze || changed {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(48));
    }
    changed
}

/// Soft pointer crosshair with Hz / dB labels on the chart axes.
fn paint_eq_crosshair(
    ui: &Ui,
    resp: &egui::Response,
    plot: Rect,
    theme: &dyn Theme,
    axis_font: &egui::FontId,
    x_to_freq: impl Fn(f32) -> f32,
    y_to_db: impl Fn(f32) -> f32,
) {
    let Some(pos) = resp.hover_pos().filter(|p| plot.contains(*p)) else {
        return;
    };
    let painter = ui.painter();
    let hair = Color32::from_rgba_unmultiplied(
        theme.text().r(),
        theme.text().g(),
        theme.text().b(),
        32,
    );
    let hub = Color32::from_rgba_unmultiplied(
        theme.text().r(),
        theme.text().g(),
        theme.text().b(),
        55,
    );
    painter.vline(pos.x, plot.y_range(), Stroke::new(1.0_f32, hair));
    painter.hline(plot.x_range(), pos.y, Stroke::new(1.0_f32, hair));
    painter.circle_filled(pos, 2.2, hub);

    let hz = x_to_freq(pos.x);
    let db = y_to_db(pos.y);
    let hz_txt = format_axis_hz(hz);
    let db_txt = format!("{db:+.1}");
    let label_col = Color32::from_rgba_unmultiplied(
        theme.text().r(),
        theme.text().g(),
        theme.text().b(),
        200,
    );
    // Axis readouts — sit on the printed axes, slightly brighter than static ticks.
    painter.text(
        egui::pos2(pos.x, plot.bottom() + 2.0),
        egui::Align2::CENTER_TOP,
        hz_txt,
        axis_font.clone(),
        label_col,
    );
    painter.text(
        egui::pos2(plot.right() + 3.0, pos.y),
        egui::Align2::LEFT_CENTER,
        db_txt,
        axis_font.clone(),
        label_col,
    );
}

fn format_axis_hz(f: f32) -> String {
    if f >= 1000.0 {
        let k = f / 1000.0;
        if (k - k.round()).abs() < 0.05 {
            format!("{}k", k.round() as i32)
        } else {
            format!("{k:.1}k")
        }
    } else if f >= 100.0 {
        format!("{:.0}", f)
    } else {
        format!("{:.1}", f)
    }
}

fn paint_fft_columns(
    ui: &mut Ui,
    plot: Rect,
    theme: &dyn Theme,
    mags: Option<&[f32]>,
    gen: u64,
    sr: f32,
    chrome: &EqChartChrome,
    salt_id: egui::Id,
    spec_db_to_y: impl Fn(f32) -> f32,
) {
    use super::dynamics_xfer_3d::phosphor_heat_color;

    const N: usize = EQ_DISPLAY_COLS;
    let spec_id = salt_id.with("eq_fft_cols_phos_v1");
    let now = ui.input(|i| i.time);
    let mut state = ui.ctx().data_mut(|d| {
        d.get_temp::<SpecPaintState>(spec_id)
            .unwrap_or_else(|| SpecPaintState {
                cols: vec![SPEC_FLOOR_DB; N],
                hold: vec![SPEC_FLOOR_DB; N],
                glow: vec![0.0; N],
                hist: vec![SPEC_FLOOR_DB; PHOS_HIST * N],
                hist_i: 0,
                targets: vec![SPEC_FLOOR_DB; N],
                ctrl_log: Vec::new(),
                ctrl_db: Vec::new(),
                last_gen: u64::MAX,
                last_t: now,
            })
    });
    if state.cols.len() != N || state.glow.len() != N || state.hist.len() != PHOS_HIST * N {
        state.cols = vec![SPEC_FLOOR_DB; N];
        state.hold = vec![SPEC_FLOOR_DB; N];
        state.glow = vec![0.0; N];
        state.hist = vec![SPEC_FLOOR_DB; PHOS_HIST * N];
        state.hist_i = 0;
        state.targets = vec![SPEC_FLOOR_DB; N];
        state.last_gen = u64::MAX;
    }
    let dt = (now - state.last_t).clamp(0.0, 0.1) as f32;
    state.last_t = now;

    if !chrome.freeze {
        if gen != state.last_gen {
            log_max_pool_into(mags, sr, &mut state);
            state.last_gen = gen;
        }
        let attack = 1.0 - (-dt * 22.0).exp();
        let release = 1.0 - (-dt * 4.2).exp();
        let hold_rel = 1.0 - (-dt * 0.35).exp();
        let glow_decay = (-dt * 1.6).exp(); // linger longer so trails read thick
        for i in 0..N {
            let tgt = state.targets[i];
            let cur = state.cols[i];
            let c = if tgt > cur { attack } else { release };
            state.cols[i] = cur + (tgt - cur) * c;
            // Glow pops hard on rising energy, decays slowly — phosphor persistence.
            let heat = spec_heat(state.cols[i]);
            if tgt > cur + 0.5 {
                state.glow[i] = (state.glow[i] + 0.35 + heat * 0.75).min(1.0);
            } else {
                state.glow[i] *= glow_decay;
                state.glow[i] = state.glow[i].max(heat * 0.45);
            }
            if chrome.peak_hold {
                if tgt > state.hold[i] {
                    state.hold[i] = tgt;
                } else {
                    state.hold[i] += (SPEC_FLOOR_DB - state.hold[i]) * hold_rel;
                }
            }
        }
        // Stamp crest into phosphor history once per frame.
        let hi = state.hist_i as usize % PHOS_HIST;
        for i in 0..N {
            state.hist[hi * N + i] = state.cols[i];
        }
        state.hist_i = state.hist_i.wrapping_add(1);
    }

    let painter = ui.painter();
    let floor_y = plot.bottom();

    // Crest-colored phosphor skirt — each column tinted by its bin energy (spectrum
    // theming), fading toward a cooler floor. Not horizontal dB meter bands.
    {
        let tint = |c: Color32, h: f32, floor: bool| {
            let base = if h > 0.05 {
                (145.0 + h * 95.0) * if floor { 0.42 } else { 1.0 }
            } else if floor {
                55.0
            } else {
                70.0
            };
            Color32::from_rgba_unmultiplied(
                c.r(),
                c.g(),
                c.b(),
                base.clamp(0.0, 240.0) as u8,
            )
        };
        let mut mesh = Mesh::default();
        mesh.vertices.reserve(N.saturating_sub(1) * 4);
        mesh.indices.reserve(N.saturating_sub(1) * 6);
        for i in 0..N.saturating_sub(1) {
            let t0 = i as f32 / (N - 1) as f32;
            let t1 = (i + 1) as f32 / (N - 1) as f32;
            let x0 = plot.left() + t0 * plot.width();
            let x1 = plot.left() + t1 * plot.width();
            let d0 = state.cols[i];
            let d1 = state.cols[i + 1];
            if d0 < SPEC_VIEW_MIN + 0.5 && d1 < SPEC_VIEW_MIN + 0.5 {
                continue;
            }
            let h0 = spec_heat(d0);
            let h1 = spec_heat(d1);
            let y0 = spec_db_to_y(d0);
            let y1 = spec_db_to_y(d1);
            let c0 = phosphor_heat_color(h0, theme);
            let c1 = phosphor_heat_color(h1, theme);
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(egui::pos2(x0, y0), tint(c0, h0, false));
            mesh.colored_vertex(egui::pos2(x1, y1), tint(c1, h1, false));
            mesh.colored_vertex(egui::pos2(x1, floor_y), tint(c1, h1, true));
            mesh.colored_vertex(egui::pos2(x0, floor_y), tint(c0, h0, true));
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
        painter.add(Shape::mesh(mesh));
    }

    // Mint crest + white tip (platform accent as signal, not chrome).
    let phos = Color32::from_rgb(0x7a, 0xd4, 0xb4);
    let phos_hi = Color32::from_rgb(0xe8, 0xf4, 0xee);

    // Peak-hold ghost — thicker cool phosphor ridge.
    if chrome.peak_hold {
        let mut hold_pts = Vec::with_capacity(N);
        for (i, &db) in state.hold.iter().enumerate() {
            let t = i as f32 / (N - 1) as f32;
            hold_pts.push(egui::pos2(
                plot.left() + t * plot.width(),
                spec_db_to_y(db),
            ));
        }
        painter.add(PathShape::line(
            hold_pts.clone(),
            PathStroke::new(
                3.0,
                Color32::from_rgba_unmultiplied(0xc8, 0xe8, 0xdc, 55),
            ),
        ));
        painter.add(PathShape::line(
            hold_pts,
            PathStroke::new(
                1.5,
                Color32::from_rgba_unmultiplied(0xc8, 0xe8, 0xdc, 130),
            ),
        ));
    }

    // Dense phosphor history beads / vertical trails — thick & bright.
    {
        let step = 2usize;
        for i in (0..N).step_by(step) {
            let glow = state.glow[i];
            if glow < 0.03 {
                continue;
            }
            let t = i as f32 / (N - 1) as f32;
            let x = plot.left() + t * plot.width();
            let mut prev: Option<Pos2> = None;
            for k in 0..PHOS_HIST {
                let age = PHOS_HIST - 1 - k;
                let hi = state.hist_i.wrapping_sub(1).wrapping_sub(age as u8) as usize % PHOS_HIST;
                let db = state.hist[hi * N + i];
                if db < SPEC_FLOOR_DB + 4.0 {
                    prev = None;
                    continue;
                }
                let p = egui::pos2(x, spec_db_to_y(db));
                let fade = (k as f32 / (PHOS_HIST as f32 - 1.0).max(1.0)).clamp(0.0, 1.0);
                let a = ((40.0 + fade * 180.0) * glow.sqrt().max(0.35)).clamp(0.0, 230.0) as u8;
                let r = 1.8 + fade * 3.2 * glow.max(0.4);
                if let Some(pp) = prev {
                    let a_trail = ((a as u16 * 2) / 3) as u8;
                    painter.line_segment(
                        [pp, p],
                        Stroke::new(
                            2.0 + fade * 2.2,
                            Color32::from_rgba_unmultiplied(phos.r(), phos.g(), phos.b(), a_trail),
                        ),
                    );
                }
                painter.circle_filled(
                    p,
                    r,
                    Color32::from_rgba_unmultiplied(phos.r(), phos.g(), phos.b(), a),
                );
                prev = Some(p);
            }
        }
    }

    // Crest: heavy bloom + thick tube + bright core (reads over heatmap).
    {
        let mut crest = Vec::with_capacity(N);
        for (i, &db) in state.cols.iter().enumerate() {
            let t = i as f32 / (N - 1) as f32;
            crest.push(egui::pos2(
                plot.left() + t * plot.width(),
                spec_db_to_y(db),
            ));
        }
        let live = state.cols.iter().any(|&d| d > SPEC_FLOOR_DB + 8.0);
        let (outer_a, mid_a, core_a) = if live {
            (90_u8, 200, 255)
        } else {
            (45, 110, 160)
        };
        // Wide outer halo
        painter.add(PathShape::line(
            crest.clone(),
            PathStroke::new(
                14.0,
                Color32::from_rgba_unmultiplied(phos.r(), phos.g(), phos.b(), outer_a / 2),
            ),
        ));
        // Mid bloom
        painter.add(PathShape::line(
            crest.clone(),
            PathStroke::new(
                8.0,
                Color32::from_rgba_unmultiplied(phos.r(), phos.g(), phos.b(), outer_a),
            ),
        ));
        // Tube body
        painter.add(PathShape::line(
            crest.clone(),
            PathStroke::new(
                4.5,
                Color32::from_rgba_unmultiplied(phos.r(), phos.g(), phos.b(), mid_a),
            ),
        ));
        // Hot core
        painter.add(PathShape::line(
            crest.clone(),
            PathStroke::new(
                2.4,
                Color32::from_rgba_unmultiplied(phos_hi.r(), phos_hi.g(), phos_hi.b(), core_a),
            ),
        ));
        // Hot tip sparkle on loudest column.
        if let Some((i_max, &db_max)) = state
            .cols
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        {
            if db_max > SPEC_FLOOR_DB + 14.0 {
                let t = i_max as f32 / (N - 1) as f32;
                let p = egui::pos2(plot.left() + t * plot.width(), spec_db_to_y(db_max));
                let h = spec_heat(db_max);
                let tip = phosphor_heat_color(h, theme);
                painter.circle_filled(
                    p,
                    4.0 + h * 3.0,
                    Color32::from_rgba_unmultiplied(phos.r(), phos.g(), phos.b(), 160),
                );
                painter.circle_filled(
                    p,
                    2.6 + h * 1.8,
                    Color32::from_rgba_unmultiplied(tip.r(), tip.g(), tip.b(), 220),
                );
                painter.circle_filled(p, 1.4, Color32::from_rgba_unmultiplied(0xf4, 0xfa, 0xf6, 245));
            }
        }
    }

    // Floor scanline — brighter CRT rice.
    painter.line_segment(
        [
            egui::pos2(plot.left(), floor_y - 0.5),
            egui::pos2(plot.right(), floor_y - 0.5),
        ],
        Stroke::new(
            1.5,
            Color32::from_rgba_unmultiplied(phos.r(), phos.g(), phos.b(), 70),
        ),
    );

    ui.ctx().data_mut(|d| d.insert_temp(spec_id, state));
}

/// Resample linear FFT bins onto log-spaced display columns (into `state.targets`).
fn log_max_pool_into(mags: Option<&[f32]>, sr: f32, state: &mut SpecPaintState) {
    let n_cols = state.targets.len();
    state.targets.fill(SPEC_FLOOR_DB);
    let Some(mags) = mags else {
        return;
    };
    if mags.len() < 4 || sr < 1.0 {
        return;
    }
    let n_fft = (mags.len() - 1) * 2;
    let log_span = F_MAX.ln() - F_MIN.ln();

    state.ctrl_log.clear();
    state.ctrl_db.clear();
    // Decimate control points — full 4k bins is wasted for 256 log columns.
    let stride = ((mags.len() / 512).max(1)) as usize;
    for i in (1..mags.len()).step_by(stride) {
        let f = (i as f32) * sr / (n_fft as f32);
        if f < F_MIN * 0.85 || f > F_MAX * 1.05 {
            continue;
        }
        // Peak within the stride window so we don't miss HF spikes.
        let mut peak_m = mags[i];
        let end = (i + stride).min(mags.len());
        for &m in &mags[i..end] {
            if m > peak_m {
                peak_m = m;
            }
        }
        let db = if peak_m > 1e-12 {
            20.0 * peak_m.log10()
        } else {
            SPEC_FLOOR_DB
        };
        state.ctrl_log.push(f.ln());
        state.ctrl_db.push(db.clamp(SPEC_FLOOR_DB, SPEC_VIEW_MAX + 6.0));
    }
    if state.ctrl_log.len() < 4 {
        return;
    }

    const SUB: usize = 2;
    for col in 0..n_cols {
        let t0 = col as f32 / n_cols as f32;
        let t1 = (col + 1) as f32 / n_cols as f32;
        let mut peak = SPEC_FLOOR_DB;
        for s in 0..SUB {
            let u = t0 + (t1 - t0) * (s as f32 + 0.5) / SUB as f32;
            let lf = F_MIN.ln() + u * log_span;
            peak = peak.max(catmull_db_at_logf(lf, &state.ctrl_log, &state.ctrl_db));
        }
        state.targets[col] = peak;
    }

    // In-place 3-tap blur — peak-preserving (reads next before overwrite).
    let mut prev = state.targets[0];
    let last = *state.targets.last().unwrap_or(&SPEC_FLOOR_DB);
    for i in 0..n_cols {
        let cur = state.targets[i];
        let next = if i + 1 < n_cols {
            state.targets[i + 1]
        } else {
            last
        };
        let blurred = prev * 0.25 + cur * 0.50 + next * 0.25;
        state.targets[i] = if cur > blurred + 1.5 {
            cur * 0.7 + blurred * 0.3
        } else {
            blurred
        };
        prev = cur;
    }
}

/// Catmull-Rom interpolate `ctrl_db` at log-frequency `lf`.
fn catmull_db_at_logf(lf: f32, ctrl_log: &[f32], ctrl_db: &[f32]) -> f32 {
    let n = ctrl_log.len();
    if n == 0 {
        return SPEC_FLOOR_DB;
    }
    if lf <= ctrl_log[0] {
        return ctrl_db[0];
    }
    if lf >= ctrl_log[n - 1] {
        return ctrl_db[n - 1];
    }
    // Binary search segment.
    let mut lo = 0usize;
    let mut hi = n - 1;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if ctrl_log[mid] <= lf {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let i1 = lo;
    let i2 = hi;
    let i0 = i1.saturating_sub(1);
    let i3 = (i2 + 1).min(n - 1);
    let span = (ctrl_log[i2] - ctrl_log[i1]).max(1e-9);
    let t = ((lf - ctrl_log[i1]) / span).clamp(0.0, 1.0);
    let p0 = ctrl_db[i0];
    let p1 = ctrl_db[i1];
    let p2 = ctrl_db[i2];
    let p3 = ctrl_db[i3];
    let t2 = t * t;
    let t3 = t2 * t;
    // Standard Catmull-Rom (tension 0.5).
    let v = 0.5
        * ((2.0 * p1)
            + (-p0 + p2) * t
            + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
            + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3);
    v.clamp(SPEC_FLOOR_DB, SPEC_VIEW_MAX + 6.0)
}
