//! Shared 3D transfer-plot helpers for Soft Clipper / Limiter.
//!
//! Frequency axis shows which spectral bands currently carry energy onto the
//! (memoryless) transfer surface — not per-band DSP. Fed by host pre-FX FFT.

use egui::{Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::spatial_viz::math::{Mat4, Vec3};
use super::tokens::Theme;

pub const BAND_N: usize = 40;
const F_MIN_HZ: f32 = 20.0;
const F_MAX_HZ: f32 = 20_000.0;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum TransferVizMode {
    #[default]
    TwoD,
    ThreeD,
}

#[derive(Clone, Copy)]
pub struct Xfer3dCamera {
    pub yaw: f32,
    pub pitch: f32,
}

impl Default for Xfer3dCamera {
    fn default() -> Self {
        Self {
            yaw: 0.62,
            pitch: 0.48,
        }
    }
}

/// Side readout when hovering a frequency slice.
#[derive(Clone, Copy)]
pub struct FreqSliceHover {
    pub band: usize,
    pub hz: f32,
    pub xin: f32,
    pub yout: f32,
    pub gr_db: f32,
}

/// Reduce host FFT mags into log-spaced peak bands (linear amplitude).
pub fn fill_log_bands_linear(
    sample_rate: u32,
    mags: &[f32],
    out_hz: &mut [f32; BAND_N],
    out_lin: &mut [f32; BAND_N],
) {
    let sr = sample_rate.max(1) as f32;
    let nyq = (sr * 0.5).min(F_MAX_HZ);
    let f_hi = nyq.max(F_MIN_HZ * 1.01);
    let n_bins = mags.len().max(1);
    let hz_per_bin = sr / ((n_bins.saturating_sub(1).max(1) * 2) as f32);
    // MAG_N = FFT_N/2+1 → bin i ≈ i * sr / FFT_N = i * sr / (2*(MAG_N-1))
    let hz_per = if n_bins > 1 {
        sr / (2.0 * (n_bins - 1) as f32)
    } else {
        hz_per_bin
    };

    for i in 0..BAND_N {
        let t0 = i as f32 / BAND_N as f32;
        let t1 = (i + 1) as f32 / BAND_N as f32;
        let f0 = F_MIN_HZ * (f_hi / F_MIN_HZ).powf(t0);
        let f1 = F_MIN_HZ * (f_hi / F_MIN_HZ).powf(t1);
        out_hz[i] = (f0 * f1).sqrt();
        let mut peak = 0.0_f32;
        let i0 = ((f0 / hz_per).floor() as usize).min(n_bins.saturating_sub(1));
        let i1 = ((f1 / hz_per).ceil() as usize).min(n_bins.saturating_sub(1)).max(i0);
        for bi in i0..=i1 {
            peak = peak.max(mags.get(bi).copied().unwrap_or(0.0));
        }
        out_lin[i] = peak;
    }
}

/// Compact 2D | 3D segmented control. Returns true when mode changed.
pub fn transfer_viz_mode_toggle(
    ui: &mut Ui,
    theme: &dyn Theme,
    mode: &mut TransferVizMode,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let mk = |ui: &mut Ui, label: &str, active: bool| {
            let fill = if active {
                theme.accent().gamma_multiply(0.85)
            } else {
                theme.bg_elevated()
            };
            let text = if active {
                Color32::WHITE
            } else {
                theme.text_dim()
            };
            ui.add(
                egui::Button::new(
                    egui::RichText::new(label)
                        .size(10.0)
                        .color(text)
                        .strong(),
                )
                .fill(fill)
                .stroke(Stroke::new(1.0, theme.border_soft()))
                .corner_radius(3.0)
                .min_size(Vec2::new(28.0, 18.0)),
            )
        };
        if mk(ui, "2D", *mode == TransferVizMode::TwoD).clicked() {
            if *mode != TransferVizMode::TwoD {
                *mode = TransferVizMode::TwoD;
                changed = true;
            }
        }
        if mk(ui, "3D", *mode == TransferVizMode::ThreeD).clicked() {
            if *mode != TransferVizMode::ThreeD {
                *mode = TransferVizMode::ThreeD;
                changed = true;
            }
        }
    });
    changed
}

fn format_hz(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{:.1} kHz", hz / 1000.0)
    } else {
        format!("{:.0} Hz", hz)
    }
}

struct Projector {
    view_proj: Mat4,
    plot: Rect,
}

impl Projector {
    fn new(plot: Rect, cam: &Xfer3dCamera) -> Self {
        let aspect = (plot.width() / plot.height().max(1.0)).clamp(0.5, 2.5);
        let eye_r = 2.55_f32;
        let yaw = cam.yaw;
        let pitch = cam.pitch.clamp(0.12, 1.35);
        let eye = Vec3::new(
            eye_r * pitch.cos() * yaw.sin(),
            eye_r * pitch.sin() + 0.15,
            eye_r * pitch.cos() * yaw.cos(),
        );
        let view = Mat4::look_at(eye, Vec3::new(0.0, 0.05, 0.0), Vec3::Y);
        let proj = Mat4::perspective(0.72, aspect, 0.15, 12.0);
        Self {
            view_proj: proj.mul(view),
            plot,
        }
    }

    /// Map unit cube coords (−0.5..0.5) to screen.
    fn project(&self, p: Vec3) -> Option<Pos2> {
        let clip = self.view_proj.transform_point(p);
        // After perspective divide in transform_point when w≠0; z in NDC-ish.
        if clip.z < -1.2 || clip.z > 1.2 {
            return None;
        }
        let x = self.plot.center().x + clip.x * self.plot.width() * 0.48;
        let y = self.plot.center().y - clip.y * self.plot.height() * 0.48;
        Some(egui::pos2(x, y))
    }
}

/// World mapping: X/Y/Z each in −0.5..0.5 cube.
fn world_of(nx: f32, ny: f32, nz: f32) -> Vec3 {
    Vec3::new(
        (nx.clamp(0.0, 1.0) - 0.5) * 1.15,
        (ny.clamp(0.0, 1.0) - 0.5) * 1.05,
        (nz.clamp(0.0, 1.0) - 0.5) * 1.25,
    )
}

/// Paint 3D transfer surface + spectral needles. Returns hover slice if any.
///
/// `xfer(xin) -> yout` and ranges use the same units as the 2D plot.
/// `gr_db(xin)` returns gain reduction in dB (≤0).
/// `past_knee(xin)` tints bands that are already in the nonlinear region.
/// `levels_are_db` only affects the side readout formatting.
#[allow(clippy::too_many_arguments)]
pub fn paint_dynamics_xfer_3d(
    ui: &mut Ui,
    theme: &dyn Theme,
    plot: Rect,
    cam: &mut Xfer3dCamera,
    x_min: f32,
    x_max: f32,
    y_min: f32,
    y_max: f32,
    band_hz: &[f32; BAND_N],
    band_xin: &[f32; BAND_N],
    xfer: &dyn Fn(f32) -> f32,
    gr_db: &dyn Fn(f32) -> f32,
    past_knee: &dyn Fn(f32) -> bool,
    levels_are_db: bool,
) -> Option<FreqSliceHover> {
    let painter = ui.painter_at(plot);
    painter.rect_filled(plot, 2.0, theme.bg_chart());
    painter.rect_stroke(
        plot,
        2.0,
        Stroke::new(1.0, theme.border_soft()),
        egui::StrokeKind::Inside,
    );

    let resp = ui.interact(plot, ui.id().with("xfer3d_orbit"), Sense::click_and_drag());
    if resp.dragged() {
        let d = resp.drag_delta();
        cam.yaw += d.x * 0.01;
        cam.pitch = (cam.pitch + d.y * 0.008).clamp(0.12, 1.35);
    }

    let x_span = (x_max - x_min).max(1e-6);
    let y_span = (y_max - y_min).max(1e-6);
    let nx = |x: f32| ((x - x_min) / x_span).clamp(0.0, 1.0);
    let ny = |y: f32| ((y - y_min) / y_span).clamp(0.0, 1.0);

    let proj = Projector::new(plot, cam);
    let line = |a: Vec3, b: Vec3, stroke: Stroke| {
        if let (Some(pa), Some(pb)) = (proj.project(a), proj.project(b)) {
            painter.line_segment([pa, pb], stroke);
        }
    };

    let grid_col = theme.border_soft().gamma_multiply(0.7);
    let axis_col = theme.text_muted();

    // Unit box edges
    let corners = [
        (0.0, 0.0, 0.0),
        (1.0, 0.0, 0.0),
        (1.0, 1.0, 0.0),
        (0.0, 1.0, 0.0),
        (0.0, 0.0, 1.0),
        (1.0, 0.0, 1.0),
        (1.0, 1.0, 1.0),
        (0.0, 1.0, 1.0),
    ];
    let edges = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    for (a, b) in edges {
        let (ax, ay, az) = corners[a];
        let (bx, by, bz) = corners[b];
        line(
            world_of(ax, ay, az),
            world_of(bx, by, bz),
            Stroke::new(1.0, grid_col),
        );
    }

    // Axis captions (screen-anchored near projected ends)
    if let Some(p) = proj.project(world_of(1.0, 0.0, 0.0)) {
        painter.text(p, egui::Align2::LEFT_CENTER, "in", FontId::proportional(9.0), axis_col);
    }
    if let Some(p) = proj.project(world_of(0.0, 1.0, 0.0)) {
        painter.text(p, egui::Align2::CENTER_BOTTOM, "out", FontId::proportional(9.0), axis_col);
    }
    if let Some(p) = proj.project(world_of(0.0, 0.0, 1.0)) {
        painter.text(p, egui::Align2::LEFT_CENTER, "Hz", FontId::proportional(9.0), theme.accent());
    }

    // Transfer surface: curves along Z (freq), wire mesh
    const X_STEPS: usize = 24;
    const Z_STEPS: usize = 12;
    let accent = theme.accent();
    let orange = theme.meter_orange();
    for zi in 0..=Z_STEPS {
        let zn = zi as f32 / Z_STEPS as f32;
        let mut prev: Option<Pos2> = None;
        for xi in 0..=X_STEPS {
            let xn = xi as f32 / X_STEPS as f32;
            let xin = x_min + xn * x_span;
            let yout = xfer(xin).clamp(y_min, y_max);
            let w = world_of(nx(xin), ny(yout), zn);
            let Some(p) = proj.project(w) else {
                prev = None;
                continue;
            };
            if let Some(pp) = prev {
                let hot = past_knee(xin);
                let col = if hot { orange } else { theme.text() };
                painter.line_segment(
                    [pp, p],
                    Stroke::new(
                        1.15,
                        Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 55),
                    ),
                );
            }
            prev = Some(p);
        }
    }
    // Cross ribs along X at a few input levels
    for xi in (0..=X_STEPS).step_by(4) {
        let xn = xi as f32 / X_STEPS as f32;
        let xin = x_min + xn * x_span;
        let yout = xfer(xin).clamp(y_min, y_max);
        let mut prev: Option<Pos2> = None;
        for zi in 0..=Z_STEPS {
            let zn = zi as f32 / Z_STEPS as f32;
            let w = world_of(nx(xin), ny(yout), zn);
            let Some(p) = proj.project(w) else {
                prev = None;
                continue;
            };
            if let Some(pp) = prev {
                painter.line_segment(
                    [pp, p],
                    Stroke::new(
                        1.0,
                        Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 28),
                    ),
                );
            }
            prev = Some(p);
        }
    }

    // Hover: nearest band by projected depth along freq axis / pointer proximity
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let mut hover: Option<FreqSliceHover> = None;
    let mut best_d = f32::MAX;
    if let Some(pos) = pointer {
        if plot.contains(pos) {
            for i in 0..BAND_N {
                let zn = (i as f32 + 0.5) / BAND_N as f32;
                // Sample mid of freq axis on floor
                let Some(p_axis) = proj.project(world_of(0.0, 0.0, zn)) else {
                    continue;
                };
                let xin = band_xin[i];
                let yout = xfer(xin).clamp(y_min, y_max);
                let Some(p_op) = proj.project(world_of(nx(xin), ny(yout), zn)) else {
                    continue;
                };
                let d = (pos - p_axis).length().min((pos - p_op).length());
                if d < best_d {
                    best_d = d;
                    hover = Some(FreqSliceHover {
                        band: i,
                        hz: band_hz[i],
                        xin,
                        yout,
                        gr_db: gr_db(xin),
                    });
                }
            }
            if best_d > 28.0 {
                hover = None;
            }
        }
    }

    // Highlighted frequency slice
    if let Some(h) = hover {
        let zn = (h.band as f32 + 0.5) / BAND_N as f32;
        let slice_col = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 70);
        let c0 = world_of(0.0, 0.0, zn);
        let c1 = world_of(1.0, 0.0, zn);
        let c2 = world_of(1.0, 1.0, zn);
        let c3 = world_of(0.0, 1.0, zn);
        for (a, b) in [(c0, c1), (c1, c2), (c2, c3), (c3, c0), (c0, c2), (c1, c3)] {
            line(a, b, Stroke::new(1.6, slice_col));
        }
    }

    // Spectral needles + dots
    for i in 0..BAND_N {
        let xin = band_xin[i];
        if !xin.is_finite() {
            continue;
        }
        // Skip near-silent bands (units: linear floor or dB floor handled by caller scaling)
        let zn = (i as f32 + 0.5) / BAND_N as f32;
        let yout = xfer(xin).clamp(y_min, y_max);
        let hot = past_knee(xin);
        let base = world_of(nx(xin), ny(y_min), zn);
        let tip = world_of(nx(xin), ny(yout), zn);
        let Some(pb) = proj.project(base) else {
            continue;
        };
        let Some(pt) = proj.project(tip) else {
            continue;
        };
        let highlighted = hover.is_some_and(|h| h.band == i);
        let col = if hot { orange } else { accent };
        let alpha = if highlighted {
            220
        } else if hot {
            170
        } else {
            110
        };
        painter.line_segment(
            [pb, pt],
            Stroke::new(
                if highlighted { 2.4 } else { 1.4 },
                Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha),
            ),
        );
        let r = if highlighted { 4.2 } else { 2.4 };
        painter.circle_filled(
            pt,
            r,
            Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha),
        );
        if highlighted {
            painter.circle_filled(pt, 1.5, Color32::WHITE);
        }
    }

    // Side readout
    if let Some(h) = hover {
        let gr_txt = if h.gr_db < -0.05 {
            format!(" · GR {:+.1} dB", h.gr_db)
        } else {
            String::new()
        };
        let label = if levels_are_db {
            format!(
                "{} · in {:+.1} · out {:+.1}{}",
                format_hz(h.hz),
                h.xin,
                h.yout,
                gr_txt
            )
        } else {
            format!(
                "{} · in {:.2} · out {:.2}{}",
                format_hz(h.hz),
                h.xin,
                h.yout,
                gr_txt
            )
        };
        painter.rect_filled(
            Rect::from_min_size(
                egui::pos2(plot.right() - 168.0, plot.top() + 4.0),
                Vec2::new(164.0, 18.0),
            ),
            3.0,
            Color32::from_rgba_unmultiplied(0, 0, 0, 140),
        );
        painter.text(
            egui::pos2(plot.right() - 6.0, plot.top() + 6.0),
            egui::Align2::RIGHT_TOP,
            label,
            FontId::proportional(10.0),
            theme.text(),
        );
    } else {
        painter.text(
            egui::pos2(plot.left() + 6.0, plot.top() + 4.0),
            egui::Align2::LEFT_TOP,
            "drag orbit · scroll param · hover Hz",
            FontId::proportional(9.0),
            theme.text_muted(),
        );
    }

    if hover.is_some() || resp.dragged() {
        ui.ctx().request_repaint();
    }

    hover
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_bands_cover_spectrum_range() {
        let mut hz = [0.0; BAND_N];
        let mut lin = [0.0; BAND_N];
        let mut mags = vec![0.0_f32; 4097];
        // Spike ~1 kHz bin at 48 kHz / 8192 FFT → bin ≈ 1000 * 8192 / 48000 ≈ 170
        let bin = ((1000.0 * 8192.0) / 48_000.0) as usize;
        mags[bin] = 0.8;
        fill_log_bands_linear(48_000, &mags, &mut hz, &mut lin);
        assert!(hz[0] < 50.0);
        assert!(hz[BAND_N - 1] > 8_000.0);
        let peak_i = lin
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        assert!(
            (800.0..1_400.0).contains(&hz[peak_i]),
            "peak band hz={}",
            hz[peak_i]
        );
        assert!(lin[peak_i] > 0.5);
    }
}
