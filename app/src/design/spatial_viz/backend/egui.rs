//! CPU egui Painter backend — the only place SpatialViz touches egui for 3D.

use egui::{Color32, Pos2, Rect, Shape, Stroke, Ui, epaint::CircleShape};

use crate::design::spatial_viz::backend::{BackendRect, SpatialBackend};
use crate::design::spatial_viz::draw_list::{
    GlyphKind, MeshKind, ShellCmd, SpatialCmd, SpatialDrawList,
};
use crate::design::spatial_viz::frame::SpatialFrame;
use crate::design::spatial_viz::math::{Mat4, Rgba, Vec3};

pub struct EguiPainterBackend {
    painter_shapes: Vec<Shape>,
    labels: Vec<(Pos2, &'static str, Color32)>,
    view_proj: Mat4,
    rect: BackendRect,
    dpi: f32,
}

impl Default for EguiPainterBackend {
    fn default() -> Self {
        Self {
            painter_shapes: Vec::new(),
            labels: Vec::new(),
            view_proj: Mat4::IDENTITY,
            rect: BackendRect {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 1.0,
                max_y: 1.0,
            },
            dpi: 1.0,
        }
    }
}

impl EguiPainterBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Paint accumulated shapes into an egui `Ui` rect (call after `end_frame`).
    pub fn paint_in_ui(&mut self, ui: &mut Ui, rect: Rect) {
        let painter = ui.painter_at(rect);
        for shape in self.painter_shapes.drain(..) {
            painter.add(shape);
        }
        for (pos, text, col) in self.labels.drain(..) {
            painter.text(
                pos,
                egui::Align2::CENTER_TOP,
                text,
                egui::FontId::proportional(10.0),
                col,
            );
        }
    }

    fn project(&self, p: Vec3) -> Option<Pos2> {
        let clip = self.view_proj.transform_point(p);
        // After perspective divide we expect NDC-ish; look_at*proj may leave z in view space.
        // Map x,y from roughly [-1,1] if w-handled; otherwise soft clamp.
        let nx = clip.x.clamp(-2.0, 2.0);
        let ny = clip.y.clamp(-2.0, 2.0);
        // Heuristic: treat as NDC when |x|,|y| <= 1.5 else orthographic-ish fallback
        let (sx, sy) = if nx.abs() <= 1.5 && ny.abs() <= 1.5 {
            ((nx + 1.0) * 0.5, (1.0 - ny) * 0.5)
        } else {
            // View-space fallback from camera-ish coords
            let z = (-clip.z).max(0.2);
            let fx = 1.2 / z;
            let fy = 1.2 / z;
            (0.5 + clip.x * fx * 0.35, 0.55 - clip.y * fy * 0.35)
        };
        let x = self.rect.min_x + sx * self.rect.width();
        let y = self.rect.min_y + sy * self.rect.height();
        Some(Pos2::new(x, y))
    }

    fn depth_key(&self, p: Vec3) -> f32 {
        let c = self.view_proj.transform_point(p);
        c.z
    }

    fn color(c: Rgba) -> Color32 {
        Color32::from_rgba_unmultiplied(
            (c.r.clamp(0.0, 1.0) * 255.0) as u8,
            (c.g.clamp(0.0, 1.0) * 255.0) as u8,
            (c.b.clamp(0.0, 1.0) * 255.0) as u8,
            (c.a.clamp(0.0, 1.0) * 255.0) as u8,
        )
    }

    fn heat_tint(base: Rgba, heat: f32) -> Rgba {
        // Cool nearly-invisible at low heat; orange only on true peaks.
        let h = heat.clamp(0.0, 1.0);
        let t = ((h - 0.18) / 0.82).clamp(0.0, 1.0).powf(1.9);
        Rgba::new(
            base.r * 0.55 * (1.0 - t) + 0.98 * t,
            base.g * 0.55 * (1.0 - t) + 0.38 * t,
            base.b * 0.58 * (1.0 - t) + 0.08 * t,
            (0.03 * (1.0 - t) + 0.78 * t).clamp(0.02, 0.8),
        )
    }

    /// Within-face falloff: panels near the source read hotter.
    fn panel_heat(face_h: f32, panel: Vec3, anchor: Vec3, room_span: f32) -> f32 {
        let dist = (panel - anchor).length();
        let fall = (1.0 - dist / room_span.max(0.4)).clamp(0.0, 1.0);
        let fall = fall.powf(1.8);
        (face_h * (0.2 + 0.8 * fall)).clamp(0.0, 1.0)
    }

    fn line3(&mut self, a: Vec3, b: Vec3, stroke: Stroke) {
        if let (Some(pa), Some(pb)) = (self.project(a), self.project(b)) {
            self.painter_shapes
                .push(Shape::line_segment([pa, pb], stroke));
        }
    }

    fn quad_fill(&mut self, pts: [Vec3; 4], fill: Color32) {
        let mut screen = Vec::new();
        for p in pts {
            if let Some(q) = self.project(p) {
                screen.push(q);
            }
        }
        if screen.len() == 4 {
            self.painter_shapes
                .push(Shape::convex_polygon(screen, fill, Stroke::NONE));
        }
    }

    fn submit_shell(&mut self, s: &ShellCmd) {
        let w = s.half_extents.x;
        let h = s.half_extents.y * 2.0;
        let d = s.half_extents.z;
        let segs = (s.segments as usize).clamp(8, 32);
        let span = (w * 2.0).max(h).max(d * 2.0);
        let wire = Stroke::new(
            1.0 * self.dpi,
            Self::color(s.material.albedo.scale_rgb(0.55).with_alpha(0.85)),
        );
        let base = s.material.albedo;

        match s.room_type {
            1 => {
                // Cylinder
                let r = w.min(d);
                for ring_y in [0.0f32, h * 0.5, h] {
                    let mut prev = None;
                    for i in 0..=segs {
                        let a = (i as f32 / segs as f32) * std::f32::consts::TAU;
                        let p = Vec3::new(r * a.cos(), ring_y, r * a.sin());
                        if let Some(q) = prev {
                            self.line3(q, p, wire);
                        }
                        prev = Some(p);
                    }
                }
                for i in 0..segs {
                    let a = (i as f32 / segs as f32) * std::f32::consts::TAU;
                    let x = r * a.cos();
                    let z = r * a.sin();
                    if i % (segs / 8).max(1) == 0 {
                        self.line3(Vec3::new(x, 0.0, z), Vec3::new(x, h, z), wire);
                    }
                    let band = ((a / std::f32::consts::FRAC_PI_2).floor() as usize) % 4;
                    let a1 = a;
                    let a2 = a + std::f32::consts::TAU / segs as f32;
                    let mid = Vec3::new(
                        r * ((a1 + a2) * 0.5).cos(),
                        h * 0.5,
                        r * ((a1 + a2) * 0.5).sin(),
                    );
                    let ph = Self::panel_heat(s.face_heat[band], mid, s.heat_anchor, span);
                    self.quad_fill(
                        [
                            Vec3::new(r * a1.cos(), 0.0, r * a1.sin()),
                            Vec3::new(r * a2.cos(), 0.0, r * a2.sin()),
                            Vec3::new(r * a2.cos(), h, r * a2.sin()),
                            Vec3::new(r * a1.cos(), h, r * a1.sin()),
                        ],
                        Self::color(Self::heat_tint(base, ph)),
                    );
                }
                // Roof disc tint
                let roof = Self::color(Self::heat_tint(base, s.face_heat[5]));
                let mut roof_pts = Vec::new();
                for i in 0..8 {
                    let a = (i as f32 / 8.0) * std::f32::consts::TAU;
                    if let Some(q) = self.project(Vec3::new(r * 0.9 * a.cos(), h, r * 0.9 * a.sin()))
                    {
                        roof_pts.push(q);
                    }
                }
                if roof_pts.len() >= 3 {
                    self.painter_shapes
                        .push(Shape::convex_polygon(roof_pts, roof, Stroke::NONE));
                }
            }
            2 => {
                // Barrel vault: rect walls + arched roof along X
                self.quad_fill(
                    [
                        Vec3::new(-w, 0.0, -d),
                        Vec3::new(w, 0.0, -d),
                        Vec3::new(w, h * 0.55, -d),
                        Vec3::new(-w, h * 0.55, -d),
                    ],
                    Self::color(Self::heat_tint(base, s.face_heat[3])),
                );
                self.quad_fill(
                    [
                        Vec3::new(-w, 0.0, d),
                        Vec3::new(w, 0.0, d),
                        Vec3::new(w, h * 0.55, d),
                        Vec3::new(-w, h * 0.55, d),
                    ],
                    Self::color(Self::heat_tint(base, s.face_heat[2])),
                );
                for side in [-1.0f32, 1.0] {
                    let x = side * w;
                    let fi = if side > 0.0 { 0 } else { 1 };
                    self.quad_fill(
                        [
                            Vec3::new(x, 0.0, -d),
                            Vec3::new(x, 0.0, d),
                            Vec3::new(x, h * 0.55, d),
                            Vec3::new(x, h * 0.55, -d),
                        ],
                        Self::color(Self::heat_tint(base, s.face_heat[fi])),
                    );
                }
                let arch_n = segs / 2;
                for zi in 0..arch_n {
                    let z0 = -d + 2.0 * d * (zi as f32 / arch_n as f32);
                    let z1 = -d + 2.0 * d * ((zi + 1) as f32 / arch_n as f32);
                    let mut prev0 = None;
                    let mut prev1 = None;
                    for i in 0..=arch_n {
                        let t = i as f32 / arch_n as f32;
                        let ang = std::f32::consts::PI * t;
                        let y = h * 0.55 + (h * 0.45) * ang.sin();
                        let x = -w + 2.0 * w * t;
                        let p0 = Vec3::new(x, y, z0);
                        let p1 = Vec3::new(x, y, z1);
                        if let (Some(a), Some(b)) = (prev0, Some(p0)) {
                            self.line3(a, b, wire);
                        }
                        if let (Some(a), Some(b)) = (prev1, Some(p1)) {
                            self.line3(a, b, wire);
                        }
                        if let (Some(a0), Some(a1)) = (prev0, prev1) {
                            self.quad_fill(
                                [a0, p0, p1, a1],
                                Self::color(Self::heat_tint(base, s.face_heat[5] * 0.85 + 0.1)),
                            );
                        }
                        prev0 = Some(p0);
                        prev1 = Some(p1);
                    }
                }
            }
            3 | 4 => {
                // Cone / pyramid
                let apex = Vec3::new(0.0, h, 0.0);
                let base_pts = [
                    Vec3::new(-w, 0.0, -d),
                    Vec3::new(w, 0.0, -d),
                    Vec3::new(w, 0.0, d),
                    Vec3::new(-w, 0.0, d),
                ];
                for i in 0..4 {
                    let a = base_pts[i];
                    let b = base_pts[(i + 1) % 4];
                    self.line3(a, b, wire);
                    self.line3(a, apex, wire);
                    let mid = Vec3::new((a.x + b.x) * 0.5, 0.0, (a.z + b.z) * 0.5);
                    // Approximate face index from mid direction
                    let fi = face_index_approx(mid);
                    let fill = Self::color(Self::heat_tint(base, s.face_heat[fi]));
                    if let (Some(pa), Some(pb), Some(pc)) =
                        (self.project(a), self.project(b), self.project(apex))
                    {
                        self.painter_shapes.push(Shape::convex_polygon(
                            vec![pa, pb, pc],
                            fill,
                            Stroke::NONE,
                        ));
                    }
                }
            }
            5 => {
                // Dome: cylindrical lower + hemisphere
                let r = w.min(d);
                let wall_h = h * 0.45;
                for i in 0..segs {
                    let a0 = (i as f32 / segs as f32) * std::f32::consts::TAU;
                    let a1 = ((i + 1) as f32 / segs as f32) * std::f32::consts::TAU;
                    let band = ((a0 / std::f32::consts::FRAC_PI_2).floor() as usize) % 4;
                    self.quad_fill(
                        [
                            Vec3::new(r * a0.cos(), 0.0, r * a0.sin()),
                            Vec3::new(r * a1.cos(), 0.0, r * a1.sin()),
                            Vec3::new(r * a1.cos(), wall_h, r * a1.sin()),
                            Vec3::new(r * a0.cos(), wall_h, r * a0.sin()),
                        ],
                        Self::color(Self::heat_tint(base, s.face_heat[band])),
                    );
                    self.line3(
                        Vec3::new(r * a0.cos(), 0.0, r * a0.sin()),
                        Vec3::new(r * a0.cos(), wall_h, r * a0.sin()),
                        wire,
                    );
                }
                let lat_n = (segs / 3).max(4);
                for j in 0..lat_n {
                    let v0 = (j as f32 / lat_n as f32) * std::f32::consts::FRAC_PI_2;
                    let v1 = ((j + 1) as f32 / lat_n as f32) * std::f32::consts::FRAC_PI_2;
                    for i in 0..segs {
                        let u0 = (i as f32 / segs as f32) * std::f32::consts::TAU;
                        let u1 = ((i + 1) as f32 / segs as f32) * std::f32::consts::TAU;
                        let p = |u: f32, v: f32| {
                            Vec3::new(
                                r * v.cos() * u.cos(),
                                wall_h + r * v.sin() * (h - wall_h) / r.max(0.1) * 0.55,
                                r * v.cos() * u.sin(),
                            )
                        };
                        // Keep dome inside height
                        let p00 = {
                            let mut q = p(u0, v0);
                            q.y = q.y.min(h);
                            q
                        };
                        let p10 = {
                            let mut q = p(u1, v0);
                            q.y = q.y.min(h);
                            q
                        };
                        let p11 = {
                            let mut q = p(u1, v1);
                            q.y = q.y.min(h);
                            q
                        };
                        let p01 = {
                            let mut q = p(u0, v1);
                            q.y = q.y.min(h);
                            q
                        };
                        self.quad_fill(
                            [p00, p10, p11, p01],
                            Self::color(Self::heat_tint(base, s.face_heat[5])),
                        );
                        if j == lat_n - 1 || i % 2 == 0 {
                            self.line3(p00, p10, wire);
                        }
                    }
                }
            }
            6 => {
                // Tunnel: horizontal cylinder along Z
                let r = w.min(h * 0.5);
                let cy = r;
                for zi in 0..=4 {
                    let z = -d + 2.0 * d * (zi as f32 / 4.0);
                    let mut prev = None;
                    for i in 0..=segs {
                        let a = (i as f32 / segs as f32) * std::f32::consts::TAU;
                        let p = Vec3::new(r * a.cos(), cy + r * a.sin(), z);
                        if let Some(q) = prev {
                            self.line3(q, p, wire);
                        }
                        prev = Some(p);
                    }
                }
                for i in 0..segs {
                    let a0 = (i as f32 / segs as f32) * std::f32::consts::TAU;
                    let a1 = ((i + 1) as f32 / segs as f32) * std::f32::consts::TAU;
                    let band = ((a0 / std::f32::consts::FRAC_PI_2).floor() as usize) % 4;
                    self.quad_fill(
                        [
                            Vec3::new(r * a0.cos(), cy + r * a0.sin(), -d),
                            Vec3::new(r * a1.cos(), cy + r * a1.sin(), -d),
                            Vec3::new(r * a1.cos(), cy + r * a1.sin(), d),
                            Vec3::new(r * a0.cos(), cy + r * a0.sin(), d),
                        ],
                        Self::color(Self::heat_tint(base, s.face_heat[band])),
                    );
                }
            }
            _ => {
                // Shoebox — subdivide each wall so heat varies toward the source.
                let span = (w * 2.0).max(h).max(d * 2.0);
                let div = (segs as usize / 6).clamp(2, 4);
                let walls: [(usize, Vec3, Vec3, Vec3); 5] = [
                    // fi, origin (u=0,v=0), u-axis, v-axis
                    (
                        0,
                        Vec3::new(w, 0.0, -d),
                        Vec3::new(0.0, 0.0, 2.0 * d),
                        Vec3::new(0.0, h, 0.0),
                    ),
                    (
                        1,
                        Vec3::new(-w, 0.0, -d),
                        Vec3::new(0.0, 0.0, 2.0 * d),
                        Vec3::new(0.0, h, 0.0),
                    ),
                    (
                        2,
                        Vec3::new(-w, 0.0, d),
                        Vec3::new(2.0 * w, 0.0, 0.0),
                        Vec3::new(0.0, h, 0.0),
                    ),
                    (
                        3,
                        Vec3::new(-w, 0.0, -d),
                        Vec3::new(2.0 * w, 0.0, 0.0),
                        Vec3::new(0.0, h, 0.0),
                    ),
                    (
                        5,
                        Vec3::new(-w, h, -d),
                        Vec3::new(2.0 * w, 0.0, 0.0),
                        Vec3::new(0.0, 0.0, 2.0 * d),
                    ),
                ];
                for (fi, origin, u_ax, v_ax) in walls {
                    let fh = s.face_heat[fi];
                    for iu in 0..div {
                        for iv in 0..div {
                            let u0 = iu as f32 / div as f32;
                            let u1 = (iu + 1) as f32 / div as f32;
                            let v0 = iv as f32 / div as f32;
                            let v1 = (iv + 1) as f32 / div as f32;
                            let p00 = origin + u_ax * u0 + v_ax * v0;
                            let p10 = origin + u_ax * u1 + v_ax * v0;
                            let p11 = origin + u_ax * u1 + v_ax * v1;
                            let p01 = origin + u_ax * u0 + v_ax * v1;
                            let mid = (p00 + p11) * 0.5;
                            let ph = Self::panel_heat(fh, mid, s.heat_anchor, span);
                            self.quad_fill([p00, p10, p11, p01], Self::color(Self::heat_tint(base, ph)));
                        }
                    }
                    let c0 = origin;
                    let c1 = origin + u_ax;
                    let c2 = origin + u_ax + v_ax;
                    let c3 = origin + v_ax;
                    self.line3(c0, c1, wire);
                    self.line3(c1, c2, wire);
                    self.line3(c2, c3, wire);
                    self.line3(c3, c0, wire);
                }
                // Cool floor outline only (heatmap on walls/ceiling).
                self.line3(Vec3::new(-w, 0.0, -d), Vec3::new(w, 0.0, -d), wire);
                self.line3(Vec3::new(w, 0.0, -d), Vec3::new(w, 0.0, d), wire);
                self.line3(Vec3::new(w, 0.0, d), Vec3::new(-w, 0.0, d), wire);
                self.line3(Vec3::new(-w, 0.0, d), Vec3::new(-w, 0.0, -d), wire);
            }
        }
    }
}

fn face_index_approx(p: Vec3) -> usize {
    if p.x.abs() > p.z.abs() {
        if p.x > 0.0 { 0 } else { 1 }
    } else if p.z > 0.0 {
        2
    } else {
        3
    }
}

impl SpatialBackend for EguiPainterBackend {
    fn begin_frame(&mut self, frame: &SpatialFrame, viewport: BackendRect) {
        self.painter_shapes.clear();
        self.labels.clear();
        self.rect = viewport;
        self.dpi = frame.viewport.dpi;
        let (view, proj) = frame.view_proj();
        self.view_proj = proj.mul(view);
    }

    fn submit(&mut self, list: &SpatialDrawList) {
        let mut cmds = list.cmds.clone();
        cmds.sort_by(|a, b| {
            b.sort_key()
                .partial_cmp(&a.sort_key())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        for cmd in cmds {
            match cmd {
                SpatialCmd::Mesh(m) => {
                    let c = m.transform.translation;
                    let hx = m.transform.scale.x * 0.5;
                    let hy = m.transform.scale.y * 0.5;
                    let hz = m.transform.scale.z * 0.5;
                    let corners = match m.kind {
                        MeshKind::Box | MeshKind::Capsule => [
                            Vec3::new(c.x - hx, c.y - hy, c.z - hz),
                            Vec3::new(c.x + hx, c.y - hy, c.z - hz),
                            Vec3::new(c.x + hx, c.y + hy, c.z - hz),
                            Vec3::new(c.x - hx, c.y + hy, c.z - hz),
                            Vec3::new(c.x - hx, c.y - hy, c.z + hz),
                            Vec3::new(c.x + hx, c.y - hy, c.z + hz),
                            Vec3::new(c.x + hx, c.y + hy, c.z + hz),
                            Vec3::new(c.x - hx, c.y + hy, c.z + hz),
                        ],
                        MeshKind::Plane => [
                            Vec3::new(c.x - hx, c.y, c.z - hz),
                            Vec3::new(c.x + hx, c.y, c.z - hz),
                            Vec3::new(c.x + hx, c.y, c.z + hz),
                            Vec3::new(c.x - hx, c.y, c.z + hz),
                            Vec3::new(c.x - hx, c.y, c.z - hz),
                            Vec3::new(c.x + hx, c.y, c.z - hz),
                            Vec3::new(c.x + hx, c.y, c.z + hz),
                            Vec3::new(c.x - hx, c.y, c.z + hz),
                        ],
                    };
                    let mut pts = Vec::new();
                    for p in corners {
                        if let Some(q) = self.project(p) {
                            pts.push(q);
                        }
                    }
                    if pts.len() >= 4 {
                        let fill = Self::color(m.material.albedo.with_alpha(
                            m.material.albedo.a * (1.0 - m.material.absorption * 0.35),
                        ));
                        // Convex hull approx: draw faces as quads via triangles of projected hull
                        let stroke = Stroke::new(
                            1.0 * self.dpi,
                            Self::color(m.material.albedo.scale_rgb(0.55).with_alpha(0.85)),
                        );
                        // Wireframe edges of the box
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
                        // Soft fill from first four (front-ish)
                        if pts.len() >= 8 {
                            self.painter_shapes.push(Shape::convex_polygon(
                                vec![pts[0], pts[1], pts[2], pts[3]],
                                fill,
                                Stroke::NONE,
                            ));
                            let fill2 = Color32::from_rgba_unmultiplied(
                                fill.r(),
                                fill.g(),
                                fill.b(),
                                ((fill.a() as f32) * 0.85) as u8,
                            );
                            self.painter_shapes.push(Shape::convex_polygon(
                                vec![pts[4], pts[5], pts[6], pts[7]],
                                fill2,
                                Stroke::NONE,
                            ));
                        }
                        for (a, b) in edges {
                            if a < pts.len() && b < pts.len() {
                                self.painter_shapes.push(Shape::line_segment(
                                    [pts[a], pts[b]],
                                    stroke,
                                ));
                            }
                        }
                    }
                }
                SpatialCmd::Field(f) => {
                    // Soft volume as stacked translucent boxes (not giant bloom discs).
                    let slices = f.quality_slices.max(1).min(6) as usize;
                    for i in 0..slices {
                        let t = (i as f32 + 0.5) / slices as f32;
                        let y = f.center.y + (t - 0.5) * f.half_extents.y * 2.0;
                        let hx = f.half_extents.x * (0.85 + 0.15 * f.density);
                        let hz = f.half_extents.z * (0.85 + 0.15 * f.density);
                        let corners = [
                            Vec3::new(f.center.x - hx, y, f.center.z - hz),
                            Vec3::new(f.center.x + hx, y, f.center.z - hz),
                            Vec3::new(f.center.x + hx, y, f.center.z + hz),
                            Vec3::new(f.center.x - hx, y, f.center.z + hz),
                        ];
                        let mut pts = Vec::new();
                        for p in corners {
                            if let Some(q) = self.project(p) {
                                pts.push(q);
                            }
                        }
                        if pts.len() == 4 {
                            let a = f.rgba.a * f.density * (0.55 / slices as f32);
                            self.painter_shapes.push(Shape::convex_polygon(
                                pts,
                                Self::color(f.rgba.with_alpha(a)),
                                Stroke::NONE,
                            ));
                        }
                    }
                }
                SpatialCmd::Ray(r) => {
                    let mut screen = Vec::new();
                    for p in &r.points {
                        if let Some(q) = self.project(*p) {
                            screen.push(q);
                        }
                    }
                    if screen.len() >= 2 {
                        self.painter_shapes.push(Shape::line(
                            screen,
                            Stroke::new(r.width.max(1.0) * self.dpi, Self::color(r.rgba)),
                        ));
                    }
                }
                SpatialCmd::Spark(s) => {
                    if let Some(center) = self.project(s.pos) {
                        let r = (2.0 + s.life * 4.0) * self.dpi;
                        self.painter_shapes.push(Shape::Circle(CircleShape {
                            center,
                            radius: r,
                            fill: Self::color(s.rgba),
                            stroke: Stroke::NONE,
                        }));
                    }
                }
                SpatialCmd::Shell(s) => {
                    self.submit_shell(&s);
                }
                SpatialCmd::Glyph(g) => {
                    if let Some(center) = self.project(g.pos) {
                        let col = Self::color(g.rgba.with_alpha(0.95));
                        let stroke = Stroke::new(1.4_f32 * self.dpi, col);
                        match g.kind {
                            GlyphKind::Source => {
                                // Compact speaker: body + cone, oriented by world yaw
                                let s = 5.0 * self.dpi;
                                let tip_w = Vec3::new(
                                    g.pos.x + g.yaw.sin() * 0.35,
                                    g.pos.y,
                                    g.pos.z + g.yaw.cos() * 0.35,
                                );
                                let (cos_a, sin_a) = if let Some(tip) = self.project(tip_w) {
                                    let dx = tip.x - center.x;
                                    let dy = tip.y - center.y;
                                    let len = (dx * dx + dy * dy).sqrt().max(1e-3);
                                    (dx / len, dy / len)
                                } else {
                                    (1.0, 0.0)
                                };
                                let rot = |lx: f32, ly: f32| {
                                    Pos2::new(
                                        center.x + lx * cos_a - ly * sin_a,
                                        center.y + lx * sin_a + ly * cos_a,
                                    )
                                };
                                let body = [
                                    rot(-s * 0.9, -s * 0.55),
                                    rot(-s * 0.15, -s * 0.55),
                                    rot(-s * 0.15, s * 0.55),
                                    rot(-s * 0.9, s * 0.55),
                                ];
                                self.painter_shapes.push(Shape::convex_polygon(
                                    body.to_vec(),
                                    col,
                                    Stroke::NONE,
                                ));
                                self.painter_shapes.push(Shape::convex_polygon(
                                    vec![
                                        rot(-s * 0.1, -s * 0.35),
                                        rot(s * 1.1, -s * 0.95),
                                        rot(s * 1.1, s * 0.95),
                                        rot(-s * 0.1, s * 0.35),
                                    ],
                                    Self::color(g.rgba.with_alpha(0.55)),
                                    stroke,
                                ));
                                self.labels.push((
                                    Pos2::new(center.x, center.y + s * 1.8),
                                    "SPEAKER",
                                    col,
                                ));
                            }
                            GlyphKind::Listener => {
                                // Compact ear / listen mark
                                let s = 4.5 * self.dpi;
                                self.painter_shapes.push(Shape::Circle(CircleShape {
                                    center,
                                    radius: s * 0.55,
                                    fill: Color32::TRANSPARENT,
                                    stroke: Stroke::new(1.6_f32 * self.dpi, col),
                                }));
                                self.painter_shapes.push(Shape::line_segment(
                                    [
                                        Pos2::new(center.x - s, center.y),
                                        Pos2::new(center.x + s, center.y),
                                    ],
                                    stroke,
                                ));
                                self.painter_shapes.push(Shape::line_segment(
                                    [
                                        Pos2::new(center.x, center.y - s),
                                        Pos2::new(center.x, center.y + s),
                                    ],
                                    stroke,
                                ));
                                self.labels.push((
                                    Pos2::new(center.x, center.y + s * 1.7),
                                    "EAR",
                                    col,
                                ));
                            }
                        }
                        let _ = g.bloom;
                    }
                }
            }
        }
        let _ = self.depth_key(Vec3::ZERO);
    }

    fn end_frame(&mut self) {}
}
