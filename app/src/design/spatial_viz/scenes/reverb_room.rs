//! Reverb shoebox scene — emits SpatialDrawList only (no egui / wgpu).
//! Readable architecture: room wireframe, speaker / ear markers, few ER paths.

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
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ReverbRoomScene {
    pub params: ReverbRoomParams,
}

impl SpatialScene for ReverbRoomScene {
    fn build(&self, frame: &SpatialFrame, metrics: &SpatialMetricBus) -> SpatialDrawList {
        let mut list = SpatialDrawList::new();
        let p = &self.params;
        let size = p.size.clamp(0.1, 4.0);
        let shape = p.shape.clamp(0.5, 2.0);
        // Half-extents of the shoebox (readable scale).
        let w = 1.0 * size * shape.sqrt();
        let h = 0.65 * size;
        let d = 1.25 * size / shape.sqrt();

        let rt60_m = metrics.get(METRIC_RT60).max(p.rt60 * 0.25);
        let echo = metrics.get(METRIC_ECHO_DENSITY).max(p.diffusion * 0.5);
        let er_tail = metrics.get(METRIC_ER_TAIL).max(p.er_level);
        let duck = metrics.get(METRIC_DUCK_GR);

        let abs_lo = (2.0 - p.decay_lo).clamp(0.05, 1.5) * 0.35;
        let abs_hi = (2.0 - p.decay_hi).clamp(0.05, 1.5) * 0.45;
        let wall_albedo = frame.theme.concrete.scale_rgb(0.75 + p.character * 0.25);
        let sheen = (1.0 - abs_hi).clamp(0.0, 1.0);

        // Floor
        mesh::push_plane(
            &mut list,
            SpatialMeshId(1),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(w * 2.0, 0.02, d * 2.0),
            SpatialMaterial {
                albedo: wall_albedo.scale_rgb(0.7).with_alpha(0.55),
                roughness: 0.9,
                emission: 0.0,
                absorption: abs_lo,
            },
            12.0,
        );

        // Ceiling plane (light wire)
        mesh::push_plane(
            &mut list,
            SpatialMeshId(6),
            Vec3::new(0.0, h * 2.0, 0.0),
            Vec3::new(w * 2.0, 0.02, d * 2.0),
            SpatialMaterial {
                albedo: wall_albedo.with_alpha(0.22),
                roughness: 0.95,
                emission: 0.0,
                absorption: abs_hi,
            },
            11.0,
        );

        // Four walls as thin boxes
        let wall_mat = SpatialMaterial {
            albedo: wall_albedo.with_alpha(0.35 + sheen * 0.2),
            roughness: 0.75,
            emission: sheen * 0.05,
            absorption: (abs_lo + abs_hi) * 0.5,
        };
        for (id, center, he) in [
            (2u32, Vec3::new(0.0, h, -d), Vec3::new(w, h, 0.03)),
            (3, Vec3::new(0.0, h, d), Vec3::new(w, h, 0.03)),
            (4, Vec3::new(-w, h, 0.0), Vec3::new(0.03, h, d)),
            (5, Vec3::new(w, h, 0.0), Vec3::new(0.03, h, d)),
        ] {
            mesh::push_box(&mut list, SpatialMeshId(id), center, he, wall_mat, 10.0);
        }

        // Floor grid (architecture, not particles)
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

        // Speaker (source) / ear (listener) — positions from Predelay
        let dist = (p.predelay_ms / 200.0).clamp(0.0, 1.0);
        let src = Vec3::new(-w * 0.45, h * 0.55, -d * (0.25 + dist * 0.45));
        let lst = Vec3::new(w * 0.4, h * 0.5, d * 0.4);

        // Compact markers (bloom ~0 → no giant discs)
        sparks::push_glyph(
            &mut list,
            GlyphKind::Source,
            src,
            0.0,
            frame.theme.tungsten,
            1.0,
        );
        sparks::push_glyph(
            &mut list,
            GlyphKind::Listener,
            lst,
            0.0,
            frame.theme.measure_cyan,
            1.0,
        );

        // Speaker facing cue (short cone)
        let face = (lst - src).normalize();
        let tip = src + face * (0.22 + size * 0.05);
        rays::push_polyline(
            &mut list,
            vec![src, tip],
            2.0,
            frame.theme.tungsten.with_alpha(0.9),
            1.5,
        );

        // Early reflections — few clean bounce paths (not a scribble)
        let n_rays = 2 + (p.er_level * 4.0).round() as usize;
        let bounce_pts = [
            Vec3::new(w * 0.9, h * 0.9, -d * 0.2),
            Vec3::new(-w * 0.85, h * 1.2, d * 0.3),
            Vec3::new(w * 0.3, h * 1.7, d * 0.85),
            Vec3::new(-w * 0.2, h * 0.35, -d * 0.9),
            Vec3::new(w * 0.7, h * 0.6, d * 0.7),
            Vec3::new(-w * 0.6, h * 1.1, -d * 0.5),
        ];
        for i in 0..n_rays.min(6) {
            let bounce = bounce_pts[i % bounce_pts.len()];
            let alpha = (0.35 + er_tail * 0.4) * (1.0 - duck * 0.4) * (1.0 - i as f32 * 0.08);
            rays::push_polyline(
                &mut list,
                vec![src, bounce, lst],
                1.1,
                frame.theme.measure_cyan.with_alpha(alpha),
                3.0 + i as f32 * 0.02,
            );
        }

        // Tail energy as a soft volume (cinematic only) — no sparkles
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
        list.sort_by_depth();
        list
    }
}
