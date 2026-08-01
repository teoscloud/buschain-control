//! Shared 3D transfer-plot helpers for Soft Clipper / Limiter.
//!
//! Frequency axis shows which spectral bands currently carry energy onto the
//! (memoryless) transfer surface — not per-band DSP. Fed by host pre-FX FFT.

use egui::epaint::{Mesh, Shape};
use egui::{Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::spatial_viz::math::{Mat4, Vec3};
use super::tokens::Theme;

/// Toned-down mint for phosphor trails (not accent / highlight).
fn phosphor_mint() -> Color32 {
    Color32::from_rgb(0x6e, 0xaa, 0x96)
}

/// Cool→hot heatmap: deep teal → mint → amber → brick (high chroma, early warm).
fn heat_color(t: f32, theme: &dyn Theme) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let cool = Color32::from_rgb(0x14, 0x36, 0x3c);
    let mint = Color32::from_rgb(0x4e, 0xc4, 0xa0); // brighter than phosphor mint
    let amber = Color32::from_rgb(0xf0, 0xc0, 0x3a); // punchier than meter yellow
    let brick = theme.meter_orange();
    let fire = Color32::from_rgb(0xe0, 0x4a, 0x2a); // hot brick tip
    let lerp = |a: Color32, b: Color32, u: f32| -> Color32 {
        let u = u.clamp(0.0, 1.0);
        // Slight gamma on the mix so mid-stops don't wash to mud.
        let u = u.powf(0.92);
        Color32::from_rgb(
            (a.r() as f32 + (b.r() as f32 - a.r() as f32) * u) as u8,
            (a.g() as f32 + (b.g() as f32 - a.g() as f32) * u) as u8,
            (a.b() as f32 + (b.b() as f32 - a.b() as f32) * u) as u8,
        )
    };
    // Early amber / brick so typical program levels leave mint quickly.
    if t < 0.16 {
        lerp(cool, mint, t / 0.16)
    } else if t < 0.38 {
        lerp(mint, amber, (t - 0.16) / 0.22)
    } else if t < 0.62 {
        lerp(amber, brick, (t - 0.38) / 0.24)
    } else {
        lerp(brick, fire, (t - 0.62) / 0.38)
    }
}

/// How close this band's operating point sits to the soft-clipper / limiter tail.
/// Biased hot: mid-span drive already reads amber; GR / near-ceiling → brick.
fn band_tail_heat(xin: f32, x_min: f32, x_span: f32, silence: f32, gr: f32) -> f32 {
    if xin <= silence {
        return 0.0;
    }
    let drive = ((xin - x_min) / x_span.max(1e-6)).clamp(0.0, 1.0);
    // Ignore quiet floor; stretch the useful 15%…100% span into 0…1, then lift mids.
    let drive_hot = ((drive - 0.12) / 0.88).clamp(0.0, 1.0).powf(0.52);
    let gr_amt = (-gr / 6.0).clamp(0.0, 1.0).powf(0.75);
    (drive_hot * 0.70 + gr_amt * 0.55).clamp(0.0, 1.0)
}

/// Log-spaced spectral bands — near EQ analyzer column density (`EQ_DISPLAY_COLS` = 256).
pub const BAND_N: usize = 224;
/// Dense EQ / heatmap curve — interpolated FFT along log-Hz (kills LF bin stairs).
pub const EQ_CURVE_N: usize = 640;
/// Short per-band phosphor history (xin samples) for the connected EQ trail.
pub const BAND_PHOS_HIST: usize = 10;
const F_MIN_HZ: f32 = 20.0;
const F_MAX_HZ: f32 = 20_000.0;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum TransferVizMode {
    #[default]
    TwoD,
    ThreeD,
}

/// Same orbit/zoom model as Reverb `ReverbVizCam` (MMB orbit · scroll distance zoom).
#[derive(Clone, Copy)]
pub struct Xfer3dCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
}

impl Default for Xfer3dCamera {
    fn default() -> Self {
        // Side view: low Hz left → high Hz right; look up the in→out ramp
        // (soft-clipper / limiter fold furthest away). Angled off face-on;
        // zoomed out for full cube.
        Self {
            yaw: -std::f32::consts::FRAC_PI_2 + 0.55,
            pitch: 0.39,
            zoom: 1.38,
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

/// Same dens / phosphor trail / live OP ballistics as the 2D transfer plots,
/// placed on the live peak-frequency slice (and per-band phosphor on every freq).
pub struct PhosphorTrail3d<'a> {
    pub dens: &'a [f32],
    pub trail: &'a [f32],
    /// Band index (0..BAND_N) for each trail sample — follows the live max band.
    pub trail_band: &'a [u16],
    pub trail_i: u8,
    pub in_smooth: f32,
    pub in_hold: f32,
    /// Values at or below this are silence (e.g. `1e-4` linear, `DB_MIN+0.5` dB).
    pub silence: f32,
    /// Smoothed Z ∈ [0,1] of the current max-energy frequency (live OP / dens / trail).
    pub peak_z: f32,
    /// Per-band input levels (phosphor tips / peak jewel).
    pub band_xin: &'a [f32],
    /// Dense log-Hz EQ curve (interpolated FFT) for polyline + heatmap.
    pub curve_xin: &'a [f32],
    /// Per-band phosphor intensity 0..1.
    pub band_phos: &'a [f32],
    /// Flat `[band * BAND_PHOS_HIST + hist_i]` xin history for per-freq trails.
    pub band_hist: &'a [f32],
    pub band_hist_i: u8,
}

/// Linear-interpolate FFT magnitude at an arbitrary Hz (avoids LF bin plateaus).
fn interp_mag_at_hz(mags: &[f32], hz: f32, hz_per: f32) -> f32 {
    if mags.is_empty() || hz_per <= 0.0 {
        return 0.0;
    }
    let x = (hz / hz_per).clamp(0.0, (mags.len() - 1) as f32);
    let i0 = x.floor() as usize;
    let i1 = (i0 + 1).min(mags.len() - 1);
    let t = x - i0 as f32;
    let m0 = mags[i0];
    let m1 = mags[i1];
    m0 + (m1 - m0) * t
}

/// Reduce host FFT mags into log-spaced peak bands (linear amplitude).
///
/// Low bands often span <1 FFT bin; peak-of-bin alone makes long LF staircases.
/// Sub-sample each log band with interpolated mags so adjacent bands can differ.
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
        // More subsamples at LF where one FFT bin covers many log bands.
        let width_bins = ((f1 - f0) / hz_per).max(0.05);
        let sub_n = if width_bins < 1.0 {
            8
        } else if width_bins < 3.0 {
            5
        } else {
            3
        };
        let mut peak = 0.0_f32;
        for s in 0..sub_n {
            let u = (s as f32 + 0.5) / sub_n as f32;
            let f = f0 * (f1 / f0).powf(u);
            peak = peak.max(interp_mag_at_hz(mags, f, hz_per));
        }
        // Also include true bin peaks when the band spans multiple bins.
        let i0 = ((f0 / hz_per).floor() as usize).min(n_bins.saturating_sub(1));
        let i1 = ((f1 / hz_per).ceil() as usize)
            .min(n_bins.saturating_sub(1))
            .max(i0);
        if i1 > i0 {
            for bi in i0..=i1 {
                peak = peak.max(mags.get(bi).copied().unwrap_or(0.0));
            }
        }
        out_lin[i] = peak;
    }

    // Light log-neighbor blend — kills remaining one-bin plateaus without smearing peaks.
    let mut blended = *out_lin;
    for i in 1..BAND_N - 1 {
        blended[i] = out_lin[i - 1] * 0.15 + out_lin[i] * 0.70 + out_lin[i + 1] * 0.15;
    }
    *out_lin = blended;
}

fn hz_per_bin(sample_rate: u32, n_bins: usize) -> f32 {
    let sr = sample_rate.max(1) as f32;
    if n_bins > 1 {
        sr / (2.0 * (n_bins - 1) as f32)
    } else {
        sr / 2.0
    }
}

/// Wide Gaussian along log-index at LF (FFT bins ~5–6 Hz); tight at HF.
fn smooth_log_curve_lf(y: &mut [f32]) {
    let n = y.len();
    if n < 3 {
        return;
    }
    let src: Vec<f32> = y.to_vec();
    for i in 0..n {
        let t = i as f32 / (n - 1) as f32;
        // ~14 samples at 20 Hz → ~1.5 near Nyquist (log index).
        let radius = (14.0 * (1.0 - t).powf(1.4) + 1.4).round().max(1.0) as usize;
        let sigma = (radius as f32 * 0.42).max(0.55);
        let mut acc = 0.0_f32;
        let mut wsum = 0.0_f32;
        let lo = i.saturating_sub(radius);
        let hi = (i + radius).min(n - 1);
        for j in lo..=hi {
            let d = (j as isize - i as isize).unsigned_abs() as f32;
            let w = (-0.5 * (d / sigma).powi(2)).exp();
            acc += src[j] * w;
            wsum += w;
        }
        y[i] = acc / wsum.max(1e-9);
    }
}

/// Dense log-Hz curve from linearly interpolated FFT mags (no peak-pool plateaus).
///
/// Peak-pooling into coarse bands makes long LF stairs when many log points share
/// one ~6 Hz bin; sampling the interpolated spectrum at each log Hz stays continuous.
pub fn fill_log_curve_linear(
    sample_rate: u32,
    mags: &[f32],
    out_hz: &mut [f32; EQ_CURVE_N],
    out_lin: &mut [f32; EQ_CURVE_N],
) {
    let sr = sample_rate.max(1) as f32;
    let nyq = (sr * 0.5).min(F_MAX_HZ);
    let f_hi = nyq.max(F_MIN_HZ * 1.01);
    let hz_per = hz_per_bin(sample_rate, mags.len().max(1));
    for i in 0..EQ_CURVE_N {
        let t = i as f32 / (EQ_CURVE_N - 1) as f32;
        let f = F_MIN_HZ * (f_hi / F_MIN_HZ).powf(t);
        out_hz[i] = f;
        out_lin[i] = interp_mag_at_hz(mags, f, hz_per);
    }
    smooth_log_curve_lf(out_lin);
}

/// Catmull-Rom interpolate a band series at normalized u ∈ [0,1].
fn catmull_band(samples: &[f32], u: f32) -> f32 {
    let n = samples.len();
    if n == 0 {
        return 0.0;
    }
    if n == 1 {
        return samples[0];
    }
    let x = u.clamp(0.0, 1.0) * (n - 1) as f32;
    let i = x.floor() as usize;
    let t = x - i as f32;
    let p0 = samples[i.saturating_sub(1)];
    let p1 = samples[i];
    let p2 = samples[(i + 1).min(n - 1)];
    let p3 = samples[(i + 2).min(n - 1)];
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
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
        let k = hz / 1000.0;
        if (k - k.round()).abs() < 0.05 {
            format!("{:.0} kHz", k.round())
        } else {
            format!("{:.1} kHz", k)
        }
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
        // Distance zoom (not FOV) — same idea as Reverb SpatialViz.
        let eye_r = 2.55_f32 * cam.zoom.clamp(0.35, 2.8);
        let yaw = cam.yaw;
        let pitch = cam.pitch.clamp(-0.12, 1.35);
        let (sp, cp) = pitch.sin_cos();
        let (sy, cy) = yaw.sin_cos();
        // Aim below cube center so the ramp floor stays in frame (~18% drop).
        let target = Vec3::new(0.0, -0.085, 0.0);
        let eye = Vec3::new(
            target.x + eye_r * cp * sy,
            target.y + eye_r * sp + 0.124,
            target.z + eye_r * cp * cy,
        );
        let view = Mat4::look_at(eye, target, Vec3::Y);
        // ~36° fovy like Reverb SpatialCamera (fixed; zoom is distance).
        let proj = Mat4::perspective(36.0_f32.to_radians(), aspect, 0.15, 12.0);
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
/// `phosphor` draws the same density ridge / trail / live OP as the 2D view.
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
    phosphor: Option<&PhosphorTrail3d<'_>>,
) -> Option<FreqSliceHover> {
    let painter = ui.painter_at(plot);
    painter.rect_filled(plot, 2.0, theme.bg_chart());
    painter.rect_stroke(
        plot,
        2.0,
        Stroke::new(1.0, theme.border_soft()),
        egui::StrokeKind::Inside,
    );

    // Reverb-parity camera: sticky MMB orbit · scroll zoom · Ctrl+scroll = param (caller).
    let resp = ui.interact(plot, ui.id().with("xfer3d_hit"), Sense::hover());
    let orbit_id = ui.id().with("xfer3d_orbiting");
    let grab_id = ui.id().with("xfer3d_grab");
    let (over, middle_down, middle_pressed, ptr_delta, ctrl_down) = ui.input(|i| {
        let pos = i.pointer.hover_pos().or_else(|| i.pointer.interact_pos());
        let over = pos.is_some_and(|p| plot.contains(p)) || resp.hovered();
        (
            over,
            i.pointer.middle_down(),
            i.pointer.button_pressed(egui::PointerButton::Middle),
            i.pointer.delta(),
            i.modifiers.ctrl,
        )
    });
    let mut orbiting = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(orbit_id).unwrap_or(false));
    if over && middle_pressed {
        orbiting = true;
    }
    if !middle_down {
        orbiting = false;
    }
    ui.ctx().data_mut(|d| d.insert_temp(orbit_id, orbiting));
    if orbiting {
        cam.yaw -= ptr_delta.x * 0.01;
        cam.pitch = (cam.pitch + ptr_delta.y * 0.008).clamp(-0.12, 1.35);
    }
    super::widgets::capture_cursor_while(ui, orbiting, grab_id);
    if over && !orbiting && !ctrl_down {
        let (raw_y, smooth_y) =
            ui.input(|i| (i.raw_scroll_delta.y, i.smooth_scroll_delta.y));
        let dy = if raw_y.abs() > 0.0 { raw_y } else { smooth_y };
        if dy.abs() > 0.01 {
            ui.ctx().input_mut(|i| {
                i.smooth_scroll_delta = Vec2::ZERO;
            });
            let step = if raw_y.abs() > 0.0 {
                (raw_y / 14.0).clamp(-4.0, 4.0)
            } else {
                (smooth_y / 48.0).clamp(-2.5, 2.5)
            };
            cam.zoom = (cam.zoom * (1.0 - step * 0.08)).clamp(0.35, 2.8);
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
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

    // Transfer surface — muted wire so spectrum / phosphor read clearly.
    const X_STEPS: usize = 36;
    const Z_STEPS: usize = 64;
    let accent = theme.accent();
    let orange = theme.meter_orange();
    let mesh_muted = theme.text_muted();
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
                let col = if hot { orange } else { mesh_muted };
                painter.line_segment(
                    [pp, p],
                    Stroke::new(
                        1.0,
                        Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), if hot { 38 } else { 18 }),
                    ),
                );
            }
            prev = Some(p);
        }
    }
    // Sparse cross ribs along frequency
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
                        0.9,
                        Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 16),
                    ),
                );
            }
            prev = Some(p);
        }
    }

    // Transfer curve on the live peak-frequency slice (reference knee shape).
    let peak_z = phosphor.map(|ph| ph.peak_z.clamp(0.0, 1.0)).unwrap_or(0.5);
    {
        let mut prev: Option<(Pos2, bool)> = None;
        for i in 0..=96 {
            let xin = x_min + x_span * (i as f32 / 96.0);
            let yout = xfer(xin).clamp(y_min, y_max);
            let hot = past_knee(xin);
            let Some(p) = proj.project(world_of(nx(xin), ny(yout), peak_z)) else {
                prev = None;
                continue;
            };
            if let Some((pp, _)) = prev {
                let col = if hot { orange } else { theme.text() };
                painter.line_segment(
                    [pp, p],
                    Stroke::new(
                        2.0,
                        Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 150),
                    ),
                );
            }
            prev = Some((p, hot));
        }
    }

    let mint = phosphor_mint();
    // Jewel OP values (for stats readout when not hovering).
    let mut jewel_stats: Option<(usize, f32, f32, f32)> = None; // band, xin, yout, gr

    if let Some(ph) = phosphor {
        let peak_band = ((peak_z * BAND_N as f32).floor() as usize).min(BAND_N.saturating_sub(1));
        let n_band = ph.band_xin.len().min(ph.band_phos.len()).min(BAND_N);
        let hist_len = BAND_PHOS_HIST;
        let hist_ok = ph.band_hist.len() >= n_band * hist_len;

        // Dense interpolated EQ curve (preferred) — falls back to band Catmull-Rom.
        let curve_n = ph.curve_xin.len().max(1);
        let use_curve = curve_n >= 32;
        let draw_n = if use_curve { curve_n - 1 } else { 512 };
        let xin_at = |u: f32| -> f32 {
            if use_curve {
                let x = u.clamp(0.0, 1.0) * (curve_n - 1) as f32;
                let i = x.floor() as usize;
                let t = x - i as f32;
                let a = ph.curve_xin[i.min(curve_n - 1)];
                let b = ph.curve_xin[(i + 1).min(curve_n - 1)];
                a + (b - a) * t
            } else {
                catmull_band(&ph.band_xin[..n_band], u)
            }
        };

        // Heatmap: (1) under EQ to floor, (2) along the transfer ramp up to each freq OP.
        // Opacity is intentional for the EQ fill — do not reuse the muted wireframe
        // ramp alphas (≈18–38); those stay dim so this layer can read clearly.
        {
            let mut mesh = Mesh::default();
            let eq_tint = |c: Color32, h: f32, scale: f32| {
                // Keep chroma readable: quiet still visible teal; hot almost solid.
                let base = if h > 0.02 {
                    (130.0 + h * 125.0) * scale
                } else {
                    55.0 * scale
                };
                Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), base.clamp(0.0, 245.0) as u8)
            };
            let mut prev: Option<(Pos2, Pos2, f32, f32)> = None; // tip, floor, heat, xin
            for di in 0..=draw_n {
                let u = di as f32 / draw_n as f32;
                let xin = xin_at(u);
                let heat = band_tail_heat(xin, x_min, x_span, ph.silence, gr_db(xin));
                let zn = u;
                let xin_draw = if xin > ph.silence { xin } else { x_min };
                let yout = xfer(xin_draw).clamp(y_min, y_max);
                let Some(pt) = proj.project(world_of(nx(xin_draw), ny(yout), zn)) else {
                    prev = None;
                    continue;
                };
                let Some(pf) = proj.project(world_of(nx(xin_draw), ny(y_min), zn)) else {
                    prev = None;
                    continue;
                };
                if let Some((ppt, ppf, ph_heat, prev_xin)) = prev {
                    let c0 = heat_color(ph_heat, theme);
                    let c1 = heat_color(heat, theme);
                    // Vertical skirt under EQ tip → floor.
                    let i0 = mesh.vertices.len() as u32;
                    mesh.colored_vertex(ppt, eq_tint(c0, ph_heat, 1.0));
                    mesh.colored_vertex(pt, eq_tint(c1, heat, 1.0));
                    mesh.colored_vertex(pf, eq_tint(c1, heat, 0.62));
                    mesh.colored_vertex(ppf, eq_tint(c0, ph_heat, 0.62));
                    mesh.add_triangle(i0, i0 + 1, i0 + 2);
                    mesh.add_triangle(i0, i0 + 2, i0 + 3);

                    // Ramp surface from low-in → OP xin (heatmap on the fold).
                    // Subsample along Z for fill cost; skirt above already uses every sample.
                    if di % 4 == 0 || di == draw_n {
                        const RAMP_X: usize = 10;
                        let x_hi0 = prev_xin.max(x_min + x_span * 0.01);
                        let x_hi1 = xin_draw.max(x_min + x_span * 0.01);
                        let zn0 = (u - 4.0 / draw_n as f32).clamp(0.0, 1.0);
                        for xi in 0..RAMP_X {
                            let t0 = xi as f32 / RAMP_X as f32;
                            let t1 = (xi + 1) as f32 / RAMP_X as f32;
                            let xa0 = x_min + (x_hi0 - x_min) * t0;
                            let xa1 = x_min + (x_hi0 - x_min) * t1;
                            let xb0 = x_min + (x_hi1 - x_min) * t0;
                            let xb1 = x_min + (x_hi1 - x_min) * t1;
                            let ya0 = xfer(xa0).clamp(y_min, y_max);
                            let ya1 = xfer(xa1).clamp(y_min, y_max);
                            let yb0 = xfer(xb0).clamp(y_min, y_max);
                            let yb1 = xfer(xb1).clamp(y_min, y_max);
                            let (Some(pa0), Some(pa1), Some(pb0), Some(pb1)) = (
                                proj.project(world_of(nx(xa0), ny(ya0), zn0)),
                                proj.project(world_of(nx(xa1), ny(ya1), zn0)),
                                proj.project(world_of(nx(xb0), ny(yb0), zn)),
                                proj.project(world_of(nx(xb1), ny(yb1), zn)),
                            ) else {
                                continue;
                            };
                            // Hotter toward the high-in / tail — ease-in so amber arrives early.
                            let h_a = ph_heat * t0.powf(0.55);
                            let h_b = heat * t1.powf(0.55);
                            let i = mesh.vertices.len() as u32;
                            mesh.colored_vertex(pa0, eq_tint(heat_color(h_a, theme), h_a, 1.15));
                            mesh.colored_vertex(pa1, eq_tint(heat_color(h_b, theme), h_b, 1.15));
                            mesh.colored_vertex(pb1, eq_tint(heat_color(h_b, theme), h_b, 1.15));
                            mesh.colored_vertex(pb0, eq_tint(heat_color(h_a, theme), h_a, 1.15));
                            mesh.add_triangle(i, i + 1, i + 2);
                            mesh.add_triangle(i, i + 2, i + 3);
                        }
                    }
                }
                prev = Some((pt, pf, heat, xin_draw));
            }
            if !mesh.is_empty() {
                painter.add(Shape::mesh(mesh));
            }
        }

        // Per-frequency phosphor trails (mint, toned down).
        for i in 0..n_band {
            let glow = ph.band_phos[i];
            if glow < 0.04 {
                continue;
            }
            let zn = (i as f32 + 0.5) / BAND_N as f32;
            let mut prev_pt: Option<Pos2> = None;
            if hist_ok {
                for k in 0..hist_len {
                    let age = hist_len - 1 - k;
                    let hi = ph.band_hist_i.wrapping_sub(1).wrapping_sub(age as u8) as usize
                        % hist_len;
                    let xin = ph.band_hist[i * hist_len + hi];
                    if xin <= ph.silence {
                        prev_pt = None;
                        continue;
                    }
                    let yout = xfer(xin).clamp(y_min, y_max);
                    let Some(p) = proj.project(world_of(nx(xin), ny(yout), zn)) else {
                        prev_pt = None;
                        continue;
                    };
                    let fade = (k as f32 / (hist_len as f32 - 1.0).max(1.0)).clamp(0.0, 1.0);
                    let a = ((18.0 + fade * 120.0) * glow.sqrt()).clamp(0.0, 170.0) as u8;
                    let r = 1.1 + fade * 1.8 * glow;
                    if let Some(pp) = prev_pt {
                        painter.line_segment(
                            [pp, p],
                            Stroke::new(
                                1.0 + fade * 0.8,
                                Color32::from_rgba_unmultiplied(mint.r(), mint.g(), mint.b(), a / 2),
                            ),
                        );
                    }
                    painter.circle_filled(
                        p,
                        r,
                        Color32::from_rgba_unmultiplied(mint.r(), mint.g(), mint.b(), a),
                    );
                    prev_pt = Some(p);
                }
            } else {
                let xin = ph.band_xin[i];
                if xin > ph.silence {
                    let yout = xfer(xin).clamp(y_min, y_max);
                    if let Some(p) = proj.project(world_of(nx(xin), ny(yout), zn)) {
                        let a = (40.0 + glow * 120.0) as u8;
                        painter.circle_filled(
                            p,
                            1.6 + glow * 2.0,
                            Color32::from_rgba_unmultiplied(mint.r(), mint.g(), mint.b(), a),
                        );
                    }
                }
            }
        }

        // Connected EQ spectrum polyline — dense interpolated log-Hz curve.
        {
            let mut prev: Option<Pos2> = None;
            for di in 0..=draw_n {
                let u = di as f32 / draw_n as f32;
                let xin = xin_at(u);
                let zn = u;
                let xin_draw = if xin > ph.silence { xin } else { x_min };
                let yout = xfer(xin_draw).clamp(y_min, y_max);
                let Some(p) = proj.project(world_of(nx(xin_draw), ny(yout), zn)) else {
                    prev = None;
                    continue;
                };
                let live = xin > ph.silence;
                if let Some(pp) = prev {
                    let a = if live { 235 } else { 55 };
                    painter.line_segment(
                        [pp, p],
                        Stroke::new(
                            if live { 2.25 } else { 1.0 },
                            Color32::from_rgba_unmultiplied(mint.r(), mint.g(), mint.b(), a),
                        ),
                    );
                }
                prev = Some(p);
            }
        }

        // Density ridge along X at the live peak-frequency Z (mint).
        let dens_n = ph.dens.len().max(1);
        let dens_max = ph.dens.iter().copied().fold(0.0_f32, f32::max).max(0.08);
        for (i, &d) in ph.dens.iter().enumerate() {
            if d < 0.02 {
                continue;
            }
            let t = d / dens_max;
            let xin = x_min + (i as f32 + 0.5) / dens_n as f32 * x_span;
            let y_curve = xfer(xin).clamp(y_min, y_max);
            let y0 = y_min + (y_curve - y_min) * 0.05;
            let y1 = y_min + (y_curve - y_min) * (0.08 + 0.22 * t);
            let Some(pb) = proj.project(world_of(nx(xin), ny(y0), peak_z)) else {
                continue;
            };
            let Some(pt) = proj.project(world_of(nx(xin), ny(y1), peak_z)) else {
                continue;
            };
            painter.line_segment(
                [pb, pt],
                Stroke::new(
                    2.0,
                    Color32::from_rgba_unmultiplied(
                        mint.r(),
                        mint.g(),
                        mint.b(),
                        (45.0 + t * 100.0) as u8,
                    ),
                ),
            );
        }

        // Hero phosphor trail (global xin) following the live max band over time.
        let n_trail = ph.trail.len();
        let mut prev_trail: Option<Pos2> = None;
        for k in 0..n_trail {
            let age = n_trail - 1 - k;
            let idx = ph.trail_i.wrapping_sub(1).wrapping_sub(age as u8) as usize % n_trail;
            let xin = ph.trail[idx];
            if xin <= ph.silence {
                prev_trail = None;
                continue;
            }
            let bi = (ph
                .trail_band
                .get(idx)
                .copied()
                .unwrap_or(peak_band as u16) as usize)
                .min(BAND_N.saturating_sub(1));
            let zn = (bi as f32 + 0.5) / BAND_N as f32;
            let yout = xfer(xin).clamp(y_min, y_max);
            let Some(p) = proj.project(world_of(nx(xin), ny(yout), zn)) else {
                prev_trail = None;
                continue;
            };
            let fade = (k as f32 / (n_trail as f32 - 1.0).max(1.0)).clamp(0.0, 1.0);
            let alpha = (28.0 + fade * 140.0) as u8;
            let r = 1.5 + fade * 2.2;
            if let Some(pp) = prev_trail {
                painter.line_segment(
                    [pp, p],
                    Stroke::new(
                        1.3 + fade,
                        Color32::from_rgba_unmultiplied(mint.r(), mint.g(), mint.b(), alpha / 2),
                    ),
                );
            }
            painter.circle_filled(
                p,
                r,
                Color32::from_rgba_unmultiplied(mint.r(), mint.g(), mint.b(), alpha),
            );
            prev_trail = Some(p);
        }

        // Live operating point jewel — only at the max-energy frequency.
        if ph.in_hold > ph.silence {
            let xin = if levels_are_db {
                ph.in_smooth.max(ph.in_hold - 1.0)
            } else {
                ph.in_smooth.max(ph.in_hold * 0.85)
            };
            let y_out = xfer(xin).clamp(y_min, y_max);
            let y_unity = if levels_are_db {
                (y_out - gr_db(xin)).clamp(y_min, y_max)
            } else {
                xin.clamp(y_min, y_max)
            };
            let g = gr_db(xin);
            jewel_stats = Some((peak_band, xin, y_out, g));
            if let (Some(p_op), Some(p_uni)) = (
                proj.project(world_of(nx(xin), ny(y_out), peak_z)),
                proj.project(world_of(nx(xin), ny(y_unity), peak_z)),
            ) {
                if g < -0.05 {
                    painter.line_segment(
                        [p_uni, p_op],
                        Stroke::new(
                            3.0,
                            Color32::from_rgba_unmultiplied(orange.r(), orange.g(), orange.b(), 120),
                        ),
                    );
                }
                let op_col = if past_knee(xin) || g < -0.05 {
                    orange
                } else {
                    theme.success()
                };
                painter.circle_filled(p_op, 5.0, op_col);
                painter.circle_filled(p_op, 2.0, Color32::WHITE);
                painter.circle_stroke(p_op, 5.0, Stroke::new(1.0, theme.border()));
            }
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

    // Hover: soft brick wash on that frequency's ramp (no wire box / cross).
    if let Some(h) = hover {
        let zn = (h.band as f32 + 0.5) / BAND_N as f32;
        let brick = theme.meter_orange();
        let light_brick = Color32::from_rgb(
            ((brick.r() as u16 * 3 + 0xff) / 4).min(255) as u8,
            ((brick.g() as u16 * 3 + 0xb0) / 4).min(255) as u8,
            ((brick.b() as u16 * 3 + 0x80) / 4).min(255) as u8,
        );
        let mut mesh = Mesh::default();
        const SLICE_X: usize = 24;
        for xi in 0..SLICE_X {
            let t0 = xi as f32 / SLICE_X as f32;
            let t1 = (xi + 1) as f32 / SLICE_X as f32;
            let xin0 = x_min + x_span * t0;
            let xin1 = x_min + x_span * t1;
            let y0 = xfer(xin0).clamp(y_min, y_max);
            let y1 = xfer(xin1).clamp(y_min, y_max);
            let (Some(pf0), Some(pf1), Some(pr0), Some(pr1)) = (
                proj.project(world_of(nx(xin0), ny(y_min), zn)),
                proj.project(world_of(nx(xin1), ny(y_min), zn)),
                proj.project(world_of(nx(xin0), ny(y0), zn)),
                proj.project(world_of(nx(xin1), ny(y1), zn)),
            ) else {
                continue;
            };
            // Brighter toward the high-in / tail side of the slice.
            let a0 = (36.0 + t0 * 70.0) as u8;
            let a1 = (36.0 + t1 * 70.0) as u8;
            let i = mesh.vertices.len() as u32;
            mesh.colored_vertex(
                pf0,
                Color32::from_rgba_unmultiplied(light_brick.r(), light_brick.g(), light_brick.b(), a0 / 2),
            );
            mesh.colored_vertex(
                pf1,
                Color32::from_rgba_unmultiplied(light_brick.r(), light_brick.g(), light_brick.b(), a1 / 2),
            );
            mesh.colored_vertex(
                pr1,
                Color32::from_rgba_unmultiplied(light_brick.r(), light_brick.g(), light_brick.b(), a1),
            );
            mesh.colored_vertex(
                pr0,
                Color32::from_rgba_unmultiplied(light_brick.r(), light_brick.g(), light_brick.b(), a0),
            );
            mesh.add_triangle(i, i + 1, i + 2);
            mesh.add_triangle(i, i + 2, i + 3);
        }
        if !mesh.is_empty() {
            painter.add(Shape::mesh(mesh));
        }
        // White tip on the EQ at this slice.
        if let Some(pt) = proj.project(world_of(nx(h.xin), ny(h.yout), zn)) {
            painter.circle_filled(
                pt,
                4.5,
                Color32::from_rgba_unmultiplied(255, 255, 255, 220),
            );
            painter.circle_filled(pt, 1.8, Color32::WHITE);
            painter.circle_stroke(
                pt,
                4.5,
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(brick.r(), brick.g(), brick.b(), 160)),
            );
        }
    }

    // Bottom-right stats: hover band, else live jewel.
    let stats_label: Option<String> = if let Some(h) = hover {
        let gr_txt = if h.gr_db < -0.05 {
            format!(" · GR {:+.1} dB", h.gr_db)
        } else {
            String::new()
        };
        Some(if levels_are_db {
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
        })
    } else if let Some((bi, xin, yout, g)) = jewel_stats {
        let hz = band_hz.get(bi).copied().unwrap_or(0.0);
        let gr_txt = if g < -0.05 {
            format!(" · GR {:+.1} dB", g)
        } else {
            String::new()
        };
        Some(if levels_are_db {
            format!(
                "{} · in {:+.1} · out {:+.1}{}",
                format_hz(hz),
                xin,
                yout,
                gr_txt
            )
        } else {
            format!(
                "{} · in {:.2} · out {:.2}{}",
                format_hz(hz),
                xin,
                yout,
                gr_txt
            )
        })
    } else {
        None
    };

    painter.text(
        egui::pos2(plot.left() + 6.0, plot.top() + 4.0),
        egui::Align2::LEFT_TOP,
        "MMB orbit · scroll zoom · Ctrl+scroll param · hover Hz",
        FontId::proportional(9.0),
        theme.text_muted(),
    );

    if let Some(label) = stats_label {
        let font = FontId::proportional(10.0);
        let galley = painter.layout_no_wrap(label, font, theme.text());
        let pad_x = 6.0;
        let pad_y = 3.0;
        let w = galley.size().x + pad_x * 2.0;
        let h = galley.size().y + pad_y * 2.0;
        let rect = Rect::from_min_size(
            egui::pos2(plot.right() - w - 4.0, plot.bottom() - h - 4.0),
            Vec2::new(w, h),
        );
        painter.rect_filled(
            rect,
            3.0,
            Color32::from_rgba_unmultiplied(0, 0, 0, 150),
        );
        painter.galley(
            egui::pos2(rect.left() + pad_x, rect.top() + pad_y),
            galley,
            theme.text(),
        );
    }

    if hover.is_some() || orbiting {
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
