//! Shared Equalizer / Analyzer chart — real FFT spectrum + interactive EQ curve.

use egui::epaint::{Mesh, PathShape, PathStroke};
use egui::{Color32, CornerRadius, Pos2, Rect, RichText, Sense, Shape, Stroke, Ui, Vec2};

use super::tokens::Theme;
use super::widgets::{hide_cursor_on_drag, peq_mag_db_at, PeqBand, PEQ_BAND_COLORS};

/// Log-spaced display columns. Kept modest — paint cost scales with this.
pub const EQ_DISPLAY_COLS: usize = 256;
const F_MIN: f32 = 20.0;
const F_MAX: f32 = 20_000.0;
const G_MIN: f32 = -24.0;
const G_MAX: f32 = 24.0;
const SPEC_FLOOR_DB: f32 = -96.0;

#[derive(Clone, Copy, Debug)]
pub struct EqChartChrome {
    pub post: bool,
    pub peak_hold: bool,
    pub freeze: bool,
}

impl Default for EqChartChrome {
    fn default() -> Self {
        Self {
            post: true,
            peak_hold: true,
            freeze: false,
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

#[derive(Clone)]
struct SpecPaintState {
    cols: Vec<f32>,
    hold: Vec<f32>,
    /// Last resampled FFT targets (only rebuilt when `last_gen` changes).
    targets: Vec<f32>,
    ctrl_log: Vec<f32>,
    ctrl_db: Vec<f32>,
    last_gen: u64,
    last_t: f64,
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
    {
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(3), theme.bg_chart());
        painter.rect_stroke(
            rect,
            CornerRadius::same(3),
            Stroke::new(1.0_f32, theme.border_soft()),
            egui::StrokeKind::Inside,
        );
    }

    let pad_l = 10.0;
    let pad_r = 30.0;
    let pad_t = 16.0;
    let pad_b = 18.0;
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
    let spec_db_to_y = |db: f32| {
        let t = ((db.clamp(SPEC_FLOOR_DB, 0.0) - SPEC_FLOOR_DB) / -SPEC_FLOOR_DB).clamp(0.0, 1.0);
        plot.bottom() - t * plot.height()
    };

    // Grid first (under spectrum + EQ) so it doesn't cut through the visuals.
    {
        let painter = ui.painter();
        let grid = theme.border_soft().gamma_multiply(0.45);
        let zero_line = theme.accent().gamma_multiply(0.35);
        for &f in &[50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0] {
            let x = freq_to_x(f);
            painter.line_segment(
                [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
                Stroke::new(1.0_f32, grid),
            );
        }
        for g in [-24i32, -12, -6, 0, 6, 12, 24] {
            let y = gain_to_y(g as f32);
            let col = if g == 0 { zero_line } else { grid };
            painter.line_segment(
                [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
                Stroke::new(if g == 0 { 1.2_f32 } else { 1.0 }, col),
            );
            if g % 12 == 0 || g == 0 {
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
        }
        for &(f, label) in &[
            (20.0, "20"),
            (100.0, "100"),
            (1000.0, "1k"),
            (10000.0, "10k"),
            (20000.0, "20k"),
        ] {
            painter.text(
                egui::pos2(freq_to_x(f), plot.bottom() + 2.0),
                egui::Align2::CENTER_TOP,
                label,
                egui::FontId::proportional(9.0),
                theme.text_muted(),
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

    if !chrome.freeze || changed {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(48));
    }
    changed
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
    const N: usize = EQ_DISPLAY_COLS;
    let spec_id = salt_id.with("eq_fft_cols");
    let now = ui.input(|i| i.time);
    let mut state = ui.ctx().data_mut(|d| {
        d.get_temp::<SpecPaintState>(spec_id)
            .unwrap_or_else(|| SpecPaintState {
                cols: vec![SPEC_FLOOR_DB; N],
                hold: vec![SPEC_FLOOR_DB; N],
                targets: vec![SPEC_FLOOR_DB; N],
                ctrl_log: Vec::new(),
                ctrl_db: Vec::new(),
                last_gen: u64::MAX,
                last_t: now,
            })
    });
    if state.cols.len() != N {
        state.cols = vec![SPEC_FLOOR_DB; N];
        state.hold = vec![SPEC_FLOOR_DB; N];
        state.targets = vec![SPEC_FLOOR_DB; N];
        state.last_gen = u64::MAX;
    }
    let dt = (now - state.last_t).clamp(0.0, 0.1) as f32;
    state.last_t = now;

    if !chrome.freeze {
        // Resample only when the host publishes a new FFT frame.
        if gen != state.last_gen {
            log_max_pool_into(mags, sr, &mut state);
            state.last_gen = gen;
        }
        let attack = 1.0 - (-dt * 22.0).exp();
        let release = 1.0 - (-dt * 4.2).exp();
        let hold_rel = 1.0 - (-dt * 0.35).exp();
        for i in 0..N {
            let tgt = state.targets[i];
            let cur = state.cols[i];
            let c = if tgt > cur { attack } else { release };
            state.cols[i] = cur + (tgt - cur) * c;
            if chrome.peak_hold {
                if tgt > state.hold[i] {
                    state.hold[i] = tgt;
                } else {
                    state.hold[i] += (SPEC_FLOOR_DB - state.hold[i]) * hold_rel;
                }
            }
        }
    }

    let a = theme.accent();
    let painter = ui.painter();
    let fill = Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), 70);
    let stroke_col = Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), 220);

    // One mesh for the mountain fill (was ~N convex_polygon shapes).
    {
        let mut mesh = Mesh::default();
        mesh.vertices.reserve(N.saturating_sub(1) * 4);
        mesh.indices.reserve(N.saturating_sub(1) * 6);
        for i in 0..N.saturating_sub(1) {
            let t0 = i as f32 / (N - 1) as f32;
            let t1 = (i + 1) as f32 / (N - 1) as f32;
            let x0 = plot.left() + t0 * plot.width();
            let x1 = plot.left() + t1 * plot.width();
            let y0 = spec_db_to_y(state.cols[i]);
            let y1 = spec_db_to_y(state.cols[i + 1]);
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(egui::pos2(x0, plot.bottom()), fill);
            mesh.colored_vertex(egui::pos2(x0, y0), fill);
            mesh.colored_vertex(egui::pos2(x1, y1), fill);
            mesh.colored_vertex(egui::pos2(x1, plot.bottom()), fill);
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
        painter.add(Shape::mesh(mesh));
    }

    // Crest + hold as single path strokes.
    {
        let mut crest = Vec::with_capacity(N);
        for (i, &db) in state.cols.iter().enumerate() {
            let t = i as f32 / (N - 1) as f32;
            crest.push(egui::pos2(
                plot.left() + t * plot.width(),
                spec_db_to_y(db),
            ));
        }
        painter.add(PathShape::line(crest, PathStroke::new(1.35, stroke_col)));
    }
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
            hold_pts,
            PathStroke::new(1.0, Color32::from_rgba_unmultiplied(0xf0, 0xf2, 0xf5, 110)),
        ));
    }

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
        state.ctrl_db.push(db.clamp(SPEC_FLOOR_DB, 6.0));
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
    v.clamp(SPEC_FLOOR_DB, 6.0)
}
