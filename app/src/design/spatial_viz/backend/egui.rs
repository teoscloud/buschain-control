//! CPU egui Painter backend — the only place SpatialViz touches egui for 3D.

use egui::{Color32, Pos2, Rect, Shape, Stroke, Ui, epaint::CircleShape};

use crate::design::spatial_viz::backend::{BackendRect, SpatialBackend};
use crate::design::spatial_viz::draw_list::{GlyphKind, MeshKind, SpatialCmd, SpatialDrawList};
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
                SpatialCmd::Glyph(g) => {
                    if let Some(center) = self.project(g.pos) {
                        let col = Self::color(g.rgba.with_alpha(0.95));
                        let stroke = Stroke::new(1.4_f32 * self.dpi, col);
                        match g.kind {
                            GlyphKind::Source => {
                                // Compact speaker: body + cone
                                let s = 5.0 * self.dpi;
                                let body = [
                                    Pos2::new(center.x - s * 0.9, center.y - s * 0.55),
                                    Pos2::new(center.x - s * 0.15, center.y - s * 0.55),
                                    Pos2::new(center.x - s * 0.15, center.y + s * 0.55),
                                    Pos2::new(center.x - s * 0.9, center.y + s * 0.55),
                                ];
                                self.painter_shapes.push(Shape::convex_polygon(
                                    body.to_vec(),
                                    col,
                                    Stroke::NONE,
                                ));
                                self.painter_shapes.push(Shape::convex_polygon(
                                    vec![
                                        Pos2::new(center.x - s * 0.1, center.y - s * 0.35),
                                        Pos2::new(center.x + s * 1.1, center.y - s * 0.95),
                                        Pos2::new(center.x + s * 1.1, center.y + s * 0.95),
                                        Pos2::new(center.x - s * 0.1, center.y + s * 0.35),
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
