//! Reverb room scene — emits SpatialDrawList only (no egui / wgpu).
//! Shell geometry, speaker / ear markers, ER bounce paths, wall heat.

use crate::design::spatial_viz::draw_list::{
    GlyphKind, SpatialDrawList, SpatialMaterial, SpatialMeshId,
};
use crate::design::spatial_viz::field;
use crate::design::spatial_viz::frame::{SpatialFrame, VizQuality};
use crate::design::spatial_viz::math::{Rgba, Vec3};
use crate::design::spatial_viz::mesh;
use crate::design::spatial_viz::metrics::{
    SpatialMetricBus, METRIC_DUCK_GR, METRIC_ECHO_DENSITY, METRIC_ER_TAIL, METRIC_RT60,
};
use crate::design::spatial_viz::rays;
use crate::design::spatial_viz::scene::SpatialScene;
use crate::design::spatial_viz::sparks;

#[derive(Clone, Debug)]
pub struct ReverbRoomParams {
    pub size: f32,
    pub shape: f32,
    pub predelay_ms: f32,
    pub rt60: f32,
    pub character: f32,
    pub er_level: f32,
    pub diffusion: f32,
    pub mix: f32,
    pub decay_lo: f32,
    pub decay_hi: f32,
    pub freeze: f32,
    pub gate_time_ms: f32,
    pub room_type: u8,
    pub source_x: f32,
    pub source_y: f32,
    pub source_z: f32,
    pub listener_x: f32,
    pub listener_y: f32,
    pub listener_z: f32,
    pub source_spacing: f32,
    pub source_yaw_deg: f32,
    pub face_lock: f32,
    pub listener_spacing: f32,
    pub ear_angle_deg: f32,
    /// Optional host-provided heat; zeros → scene derives from ER model.
    pub wall_heat: [f32; 6],
}

impl Default for ReverbRoomParams {
    fn default() -> Self {
        Self {
            size: 1.0,
            shape: 1.0,
            predelay_ms: 20.0,
            rt60: 1.8,
            character: 0.55,
            er_level: 0.55,
            diffusion: 0.65,
            mix: 0.25,
            decay_lo: 1.0,
            decay_hi: 0.7,
            freeze: 0.0,
            gate_time_ms: 0.0,
            room_type: 0,
            source_x: 0.28,
            source_y: 0.55,
            source_z: 0.30,
            listener_x: 0.72,
            listener_y: 0.50,
            listener_z: 0.70,
            source_spacing: 0.35,
            source_yaw_deg: 0.0,
            face_lock: 1.0,
            listener_spacing: 0.35,
            ear_angle_deg: 180.0,
            wall_heat: [0.0; 6],
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ReverbRoomScene {
    pub params: ReverbRoomParams,
}

/// Half-extents (w, h, d) with floor at y=0 and roof at y=2h.
pub fn room_half_extents(size: f32, shape: f32, room_type: u8) -> Vec3 {
    let size = size.clamp(0.1, 4.0);
    let shape = shape.clamp(0.5, 2.0);
    let mut w = 1.0 * size * shape.sqrt();
    let h = 0.65 * size;
    let mut d = 1.25 * size / shape.sqrt();
    match room_type {
        1 | 5 => {
            let r = w.min(d);
            w = r;
            d = r;
        }
        6 => {
            d *= 1.45;
            w *= 0.75;
        }
        _ => {}
    }
    Vec3::new(w, h, d)
}

pub fn norm_to_world(nx: f32, ny: f32, nz: f32, he: Vec3) -> Vec3 {
    Vec3::new(
        (nx.clamp(0.0, 1.0) * 2.0 - 1.0) * he.x * 0.9,
        ny.clamp(0.05, 0.95) * he.y * 2.0,
        (nz.clamp(0.0, 1.0) * 2.0 - 1.0) * he.z * 0.9,
    )
}

pub fn world_xz_to_norm(p: Vec3, he: Vec3) -> (f32, f32) {
    let x = ((p.x / (he.x * 0.9).max(1e-4) + 1.0) * 0.5).clamp(0.02, 0.98);
    let z = ((p.z / (he.z * 0.9).max(1e-4) + 1.0) * 0.5).clamp(0.02, 0.98);
    (x, z)
}

/// Bearing in degrees from source center toward listener (0 = +Z, CCW).
pub fn bearing_yaw_deg(from: Vec3, to: Vec3) -> f32 {
    let dx = to.x - from.x;
    let dz = to.z - from.z;
    if dx * dx + dz * dz < 1e-8 {
        return 0.0;
    }
    dx.atan2(dz).to_degrees()
}

/// Stereo pair: (left, right, center) world positions + aim yaw radians per speaker.
pub fn stereo_speakers(p: &ReverbRoomParams, he: Vec3) -> (Vec3, Vec3, Vec3, f32, f32) {
    let center = norm_to_world(p.source_x, p.source_y, p.source_z, he);
    let lst = norm_to_world(p.listener_x, p.listener_y, p.listener_z, he);
    let spacing = p.source_spacing.clamp(0.0, 1.0);
    let face_lock = p.face_lock >= 0.5;
    let (fwd_x, fwd_z) = if face_lock {
        let dx = lst.x - center.x;
        let dz = lst.z - center.z;
        let fl = (dx * dx + dz * dz).sqrt();
        if fl < 1e-4 {
            (0.0, 1.0)
        } else {
            (dx / fl, dz / fl)
        }
    } else {
        let yaw = p.source_yaw_deg.to_radians();
        (yaw.sin(), yaw.cos())
    };
    let base_x = -fwd_z;
    let base_z = fwd_x;
    let half = spacing * he.x.min(he.z) * 0.45;
    let left = Vec3::new(center.x - base_x * half, center.y, center.z - base_z * half);
    let right = Vec3::new(center.x + base_x * half, center.y, center.z + base_z * half);
    let yaw_l = if face_lock {
        bearing_yaw_deg(left, lst).to_radians()
    } else {
        p.source_yaw_deg.to_radians()
    };
    let yaw_r = if face_lock {
        bearing_yaw_deg(right, lst).to_radians()
    } else {
        p.source_yaw_deg.to_radians()
    };
    (left, right, center, yaw_l, yaw_r)
}

/// Dual ears: (left, right, head center, yaw_l, yaw_r) — head faces source center.
pub fn stereo_ears(p: &ReverbRoomParams, he: Vec3) -> (Vec3, Vec3, Vec3, f32, f32) {
    let head = norm_to_world(p.listener_x, p.listener_y, p.listener_z, he);
    let src = norm_to_world(p.source_x, p.source_y, p.source_z, he);
    let mut hfx = src.x - head.x;
    let mut hfz = src.z - head.z;
    let hfl = (hfx * hfx + hfz * hfz).sqrt();
    if hfl < 1e-4 {
        hfx = 0.0;
        hfz = -1.0;
    } else {
        hfx /= hfl;
        hfz /= hfl;
    }
    let iax = -hfz;
    let iaz = hfx;
    let ear_half = (0.04 + p.listener_spacing.clamp(0.0, 1.0) * 0.14) * he.x.min(he.z);
    let left = Vec3::new(head.x + iax * ear_half, head.y, head.z + iaz * ear_half);
    let right = Vec3::new(head.x - iax * ear_half, head.y, head.z - iaz * ear_half);
    let flare = ((180.0 - p.ear_angle_deg.clamp(90.0, 180.0)) * 0.5).to_radians();
    let (cf, sf) = (flare.cos(), flare.sin());
    let yaw_l = (iax * cf + hfx * sf).atan2(iaz * cf + hfz * sf);
    let yaw_r = (-iax * cf + hfx * sf).atan2(-iaz * cf + hfz * sf);
    (left, right, head, yaw_l, yaw_r)
}

fn shell_bounce(room: u8, he: Vec3, src: Vec3, i: usize, spread: f32) -> Vec3 {
    let ang = i as f32 * (std::f32::consts::TAU / 6.0) + spread * 0.7;
    let elev = -0.25 + 0.55 * ((i % 5) as f32 / 4.0);
    let dx = ang.cos() * elev.cos();
    let dy = elev.sin();
    let dz = ang.sin() * elev.cos();
    let w = he.x;
    let h = he.y * 2.0;
    let d = he.z;
    let t = match room {
        1 | 5 => {
            let r = w.min(d);
            let a = dx * dx + dz * dz;
            let th = if a > 1e-8 { r / a.sqrt() } else { r };
            let ty = if dy.abs() > 1e-6 {
                if dy > 0.0 {
                    (h - src.y) / dy
                } else {
                    -src.y / dy
                }
            } else {
                1e6
            };
            th.max(0.05).min(ty.max(0.05)).min(8.0)
        }
        6 => {
            let r = w.min(he.y);
            let a = dx * dx + dy * dy;
            let th = if a > 1e-8 { r / a.sqrt() } else { r };
            let tz = if dz.abs() > 1e-6 {
                (if dz > 0.0 { d } else { -d } - src.z) / dz
            } else {
                1e6
            };
            th.max(0.05).min(tz.max(0.05)).min(8.0)
        }
        3 | 4 => {
            let tw = if dx.abs() > 1e-6 {
                (if dx > 0.0 { w } else { -w } - src.x) / dx
            } else {
                1e6
            };
            let td = if dz.abs() > 1e-6 {
                (if dz > 0.0 { d } else { -d } - src.z) / dz
            } else {
                1e6
            };
            let th = if dy.abs() > 1e-6 {
                (if dy > 0.0 { h } else { 0.0 } - src.y) / dy
            } else {
                1e6
            };
            (tw.min(td).min(th) * 0.85).clamp(0.1, 8.0)
        }
        _ => {
            let tw = if dx.abs() > 1e-6 {
                (if dx > 0.0 { w } else { -w } - src.x) / dx
            } else {
                1e6
            };
            let td = if dz.abs() > 1e-6 {
                (if dz > 0.0 { d } else { -d } - src.z) / dz
            } else {
                1e6
            };
            let th = if dy.abs() > 1e-6 {
                (if dy > 0.0 { h } else { 0.0 } - src.y) / dy
            } else {
                1e6
            };
            tw.min(td).min(th).clamp(0.1, 8.0)
        }
    };
    Vec3::new(
        (src.x + dx * t).clamp(-w, w),
        (src.y + dy * t).clamp(0.0, h),
        (src.z + dz * t).clamp(-d, d),
    )
}

fn face_index(room: u8, he: Vec3, p: Vec3) -> usize {
    let w = he.x;
    let h = he.y * 2.0;
    let d = he.z;
    match room {
        1 | 5 | 6 => {
            let az = p.z.atan2(p.x);
            let band = (((az + std::f32::consts::PI) / std::f32::consts::FRAC_PI_2).floor() as i32)
                .rem_euclid(4) as usize;
            if p.y > h * 0.85 {
                5
            } else if p.y < h * 0.12 {
                4
            } else {
                band.min(3)
            }
        }
        _ => {
            let dx = (p.x.abs() - w).abs();
            let dz = (p.z.abs() - d).abs();
            let dy0 = p.y;
            let dy1 = (h - p.y).abs();
            if dy0 < 0.08 {
                4
            } else if dy1 < 0.08 {
                5
            } else if dx < dz {
                if p.x > 0.0 { 0 } else { 1 }
            } else if p.z > 0.0 {
                2
            } else {
                3
            }
        }
    }
}

fn derive_heat(p: &ReverbRoomParams, he: Vec3, src: Vec3, lst: Vec3) -> [f32; 6] {
    if p.wall_heat.iter().any(|v| *v > 1e-4) {
        return p.wall_heat;
    }
    let mut heat = [0.0f32; 6];
    let n = 4 + (p.er_level * 8.0).round() as usize;
    for i in 0..n.min(12) {
        let b = shell_bounce(p.room_type, he, src, i, p.er_level);
        let fi = face_index(p.room_type, he, b);
        let dist = (b - src).length() + (lst - b).length();
        let e = (p.er_level * 0.9 + p.diffusion * 0.2) / (0.4 + dist * 0.22);
        heat[fi] += e;
    }
    // Prox bias only when clearly near a face (keeps far walls cool).
    let near = [
        (1.0 - (he.x - src.x).abs() / he.x.max(0.1)).clamp(0.0, 1.0),
        (1.0 - (he.x + src.x).abs() / he.x.max(0.1)).clamp(0.0, 1.0),
        (1.0 - (he.z - src.z).abs() / he.z.max(0.1)).clamp(0.0, 1.0),
        (1.0 - (he.z + src.z).abs() / he.z.max(0.1)).clamp(0.0, 1.0),
        (1.0 - src.y / (he.y * 2.0).max(0.1)).clamp(0.0, 1.0) * 0.35,
        (src.y / (he.y * 2.0).max(0.1)).clamp(0.0, 1.0) * 0.25,
    ];
    for (j, n) in near.iter().enumerate() {
        let w = (*n - 0.55).max(0.0) / 0.45; // only close proximity
        heat[j] += w * w * (0.2 + p.er_level * 0.45);
    }
    let late = (p.rt60 / 12.0) * (0.15 + p.diffusion * 0.3) * (0.3 + p.mix);
    let peak = heat.iter().cloned().fold(0.0f32, f32::max).max(1e-6);
    for h in &mut heat {
        if *h > peak * 0.35 {
            *h += late * (*h / peak) * 0.2;
        }
    }
    // Peak-normalize (not min–max): cool faces stay near 0, hottest → 1.
    // Power > 1 compresses mids so concentration reads as sparse hot spots.
    let max_h = heat.iter().cloned().fold(0.0f32, f32::max).max(1e-6);
    for h in &mut heat {
        let t = (*h / max_h).clamp(0.0, 1.0);
        *h = t.powf(1.65);
    }
    heat
}

impl SpatialScene for ReverbRoomScene {
    fn build(&self, frame: &SpatialFrame, metrics: &SpatialMetricBus) -> SpatialDrawList {
        let mut list = SpatialDrawList::new();
        let p = &self.params;
        let he = room_half_extents(p.size, p.shape, p.room_type);
        let w = he.x;
        let h = he.y;
        let d = he.z;

        let rt60_m = metrics.get(METRIC_RT60).max(p.rt60 * 0.25);
        let echo = metrics.get(METRIC_ECHO_DENSITY).max(p.diffusion * 0.5);
        let er_tail = metrics.get(METRIC_ER_TAIL).max(p.er_level);
        let duck = metrics.get(METRIC_DUCK_GR);

        let abs_lo = (2.0 - p.decay_lo).clamp(0.05, 1.5) * 0.35;
        let abs_hi = (2.0 - p.decay_hi).clamp(0.05, 1.5) * 0.45;
        let wall_albedo = frame.theme.concrete.scale_rgb(0.75 + p.character * 0.25);
        let sheen = (1.0 - abs_hi).clamp(0.0, 1.0);

        let (spk_l, spk_r, src, yaw_l, yaw_r) = stereo_speakers(p, he);
        let (ear_l, ear_r, lst, ear_yaw_l, ear_yaw_r) = stereo_ears(p, he);
        let heat = derive_heat(p, he, src, lst);
        let stereo = p.source_spacing > 0.02;
        let binaural = p.listener_spacing > 0.02;

        let segs = match frame.quality {
            VizQuality::Cinematic => 24,
            VizQuality::Essential => 12,
        };

        // Floor (hit target + grid base)
        mesh::push_plane(
            &mut list,
            SpatialMeshId(1),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(w * 2.0, 0.02, d * 2.0),
            SpatialMaterial {
                albedo: wall_albedo.scale_rgb(0.7).with_alpha(0.45),
                roughness: 0.9,
                emission: heat[4] * 0.15,
                absorption: abs_lo,
            },
            12.0,
        );

        mesh::push_shell(
            &mut list,
            p.room_type,
            he,
            heat,
            SpatialMaterial {
                albedo: wall_albedo.with_alpha(0.32 + sheen * 0.15),
                roughness: 0.75,
                emission: sheen * 0.05,
                absorption: (abs_lo + abs_hi) * 0.5,
            },
            segs,
            10.0,
        );

        // Floor grid
        let grid_a = 0.28;
        for i in -2..=2 {
            let t = i as f32 / 2.0;
            let x = t * w;
            let z = t * d;
            rays::push_polyline(
                &mut list,
                vec![Vec3::new(x, 0.01, -d), Vec3::new(x, 0.01, d)],
                0.8,
                frame.theme.concrete.with_alpha(grid_a),
                13.0,
            );
            rays::push_polyline(
                &mut list,
                vec![Vec3::new(-w, 0.01, z), Vec3::new(w, 0.01, z)],
                0.8,
                frame.theme.concrete.with_alpha(grid_a),
                13.0,
            );
        }

        // Crosshair under center / listener on floor
        for (px, pz, col) in [
            (src.x, src.z, frame.theme.tungsten.with_alpha(0.45)),
            (lst.x, lst.z, frame.theme.measure_cyan.with_alpha(0.45)),
        ] {
            let s = 0.12 + p.size * 0.02;
            rays::push_polyline(
                &mut list,
                vec![Vec3::new(px - s, 0.02, pz), Vec3::new(px + s, 0.02, pz)],
                1.0,
                col,
                2.0,
            );
            rays::push_polyline(
                &mut list,
                vec![Vec3::new(px, 0.02, pz - s), Vec3::new(px, 0.02, pz + s)],
                1.0,
                col,
                2.0,
            );
        }

        if stereo {
            // Spacing baseline between L–R
            rays::push_polyline(
                &mut list,
                vec![
                    Vec3::new(spk_l.x, 0.03, spk_l.z),
                    Vec3::new(spk_r.x, 0.03, spk_r.z),
                ],
                1.4,
                frame.theme.tungsten.with_alpha(0.55),
                1.8,
            );
            sparks::push_glyph_yaw(
                &mut list,
                GlyphKind::Source,
                spk_l,
                yaw_l,
                0.0,
                frame.theme.tungsten,
                1.0,
            );
            sparks::push_glyph_yaw(
                &mut list,
                GlyphKind::Source,
                spk_r,
                yaw_r,
                0.0,
                frame.theme.tungsten.scale_rgb(0.85),
                1.0,
            );
            for (spk, yaw) in [(spk_l, yaw_l), (spk_r, yaw_r)] {
                let tip = Vec3::new(
                    spk.x + yaw.sin() * (0.28 + p.size * 0.06),
                    spk.y,
                    spk.z + yaw.cos() * (0.28 + p.size * 0.06),
                );
                rays::push_polyline(
                    &mut list,
                    vec![spk, tip],
                    2.0,
                    frame.theme.tungsten.with_alpha(0.9),
                    1.5,
                );
            }
        } else {
            sparks::push_glyph_yaw(
                &mut list,
                GlyphKind::Source,
                src,
                yaw_l,
                0.0,
                frame.theme.tungsten,
                1.0,
            );
            let tip = Vec3::new(
                src.x + yaw_l.sin() * (0.28 + p.size * 0.06),
                src.y,
                src.z + yaw_l.cos() * (0.28 + p.size * 0.06),
            );
            rays::push_polyline(
                &mut list,
                vec![src, tip],
                2.0,
                frame.theme.tungsten.with_alpha(0.9),
                1.5,
            );
        }

        if binaural {
            rays::push_polyline(
                &mut list,
                vec![
                    Vec3::new(ear_l.x, 0.03, ear_l.z),
                    Vec3::new(ear_r.x, 0.03, ear_r.z),
                ],
                1.2,
                frame.theme.measure_cyan.with_alpha(0.5),
                1.7,
            );
            for (ear, eyaw) in [(ear_l, ear_yaw_l), (ear_r, ear_yaw_r)] {
                sparks::push_glyph_yaw(
                    &mut list,
                    GlyphKind::Listener,
                    ear,
                    eyaw,
                    0.0,
                    frame.theme.measure_cyan,
                    1.0,
                );
                let tip = Vec3::new(
                    ear.x + eyaw.sin() * 0.18,
                    ear.y,
                    ear.z + eyaw.cos() * 0.18,
                );
                rays::push_polyline(
                    &mut list,
                    vec![ear, tip],
                    1.6,
                    frame.theme.measure_cyan.with_alpha(0.85),
                    1.4,
                );
            }
        } else {
            sparks::push_glyph(
                &mut list,
                GlyphKind::Listener,
                lst,
                0.0,
                frame.theme.measure_cyan,
                1.0,
            );
        }

        let n_rays = 2 + (p.er_level * 4.0).round() as usize;
        let emitters: &[Vec3] = if stereo {
            &[spk_l, spk_r][..]
        } else {
            &[src][..]
        };
        let receivers: &[Vec3] = if binaural {
            &[ear_l, ear_r][..]
        } else {
            &[lst][..]
        };
        for (ei, em) in emitters.iter().enumerate() {
            for (ri, recv) in receivers.iter().enumerate() {
                for i in 0..(n_rays.min(3)) {
                    let bounce =
                        shell_bounce(p.room_type, he, *em, i + ei * 3 + ri * 5, p.er_level);
                    let alpha = (0.28 + er_tail * 0.35)
                        * (1.0 - duck * 0.4)
                        * (1.0 - i as f32 * 0.08)
                        * 0.75;
                    rays::push_polyline(
                        &mut list,
                        vec![*em, bounce, *recv],
                        1.0,
                        frame.theme.measure_cyan.with_alpha(alpha),
                        3.0 + i as f32 * 0.02 + ei as f32 * 0.01 + ri as f32 * 0.015,
                    );
                }
            }
        }

        if matches!(frame.quality, VizQuality::Cinematic) {
            let fog_a = if p.gate_time_ms > 1.0 && p.freeze < 0.5 {
                0.04
            } else {
                let base = 0.10 + p.mix * 0.12 + (rt60_m / 12.0) * 0.15;
                if p.freeze > 0.5 {
                    base * 1.1
                } else {
                    base
                }
            };
            field::push_fog(
                &mut list,
                Vec3::new(0.0, h, 0.0),
                Vec3::new(w * 0.85, h * 0.75, d * 0.85),
                (echo * 0.55 + p.diffusion * 0.35).clamp(0.15, 0.9),
                frame.theme.fog.with_alpha(fog_a),
                4,
                20.0,
            );
        }

        let _ = Rgba::new(0.0, 0.0, 0.0, 0.0);
        let _ = p.predelay_ms;
        list.sort_by_depth();
        list
    }
}
