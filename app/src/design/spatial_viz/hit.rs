use super::draw_list::{MeshKind, SpatialCmd, SpatialDrawList};
use super::frame::SpatialFrame;
use super::math::Vec3;

#[derive(Clone, Copy, Debug)]
pub struct SpatialRay {
    pub origin: Vec3,
    pub dir: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpatialHitKind {
    Mesh,
    Wall,
    Source,
    Listener,
}

#[derive(Clone, Debug)]
pub struct SpatialHit {
    pub kind: SpatialHitKind,
    pub mesh_id: Option<u32>,
    pub t: f32,
    pub point: Vec3,
}

/// Camera ray through a normalized screen point in [-1,1] NDC (y up).
pub fn camera_ray(frame: &SpatialFrame, ndc_x: f32, ndc_y: f32) -> SpatialRay {
    let eye = frame.camera.eye;
    let target = frame.camera.target;
    let up = frame.camera.up;
    let f = (target - eye).normalize();
    let s = f.cross(up).normalize();
    let u = s.cross(f);
    let tan = (frame.camera.fovy_deg.to_radians() * 0.5).tan();
    let aspect = frame.viewport.aspect();
    let dir = (f + s * (ndc_x * tan * aspect) + u * (ndc_y * tan)).normalize();
    SpatialRay { origin: eye, dir }
}

/// Intersect ray with y = plane_y floor; returns world point if hit in front.
pub fn intersect_floor(ray: SpatialRay, plane_y: f32) -> Option<Vec3> {
    if ray.dir.y.abs() < 1e-6 {
        return None;
    }
    let t = (plane_y - ray.origin.y) / ray.dir.y;
    if t <= 0.0 {
        return None;
    }
    Some(ray.origin + ray.dir * t)
}

/// CPU raycast against draw-list geometry (works for both backends).
/// Glyphs win over walls when both are near the ray.
pub fn pick(list: &SpatialDrawList, ray: SpatialRay) -> Option<SpatialHit> {
    let mut best_wall: Option<SpatialHit> = None;
    let mut best_glyph: Option<SpatialHit> = None;
    for cmd in &list.cmds {
        match cmd {
            SpatialCmd::Mesh(m) if matches!(m.kind, MeshKind::Box | MeshKind::Plane) => {
                let center = m.transform.translation;
                let he = m.transform.scale * 0.5;
                if let Some(t) = ray_aabb(ray, center - he, center + he) {
                    let point = ray.origin + ray.dir * t;
                    let hit = SpatialHit {
                        kind: SpatialHitKind::Wall,
                        mesh_id: Some(m.id.0),
                        t,
                        point,
                    };
                    if best_wall.as_ref().map(|b| t < b.t).unwrap_or(true) {
                        best_wall = Some(hit);
                    }
                }
            }
            SpatialCmd::Glyph(g) => {
                let d = g.pos - ray.origin;
                let t = d.dot(ray.dir);
                if t > 0.0 {
                    let closest = ray.origin + ray.dir * t;
                    // Slightly generous for floor-drag UX.
                    if (closest - g.pos).length() < 0.38 {
                        let kind = match g.kind {
                            super::draw_list::GlyphKind::Source => SpatialHitKind::Source,
                            super::draw_list::GlyphKind::Listener => SpatialHitKind::Listener,
                        };
                        let hit = SpatialHit {
                            kind,
                            mesh_id: None,
                            t,
                            point: g.pos,
                        };
                        if best_glyph.as_ref().map(|b| t < b.t).unwrap_or(true) {
                            best_glyph = Some(hit);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    best_glyph.or(best_wall)
}

fn ray_aabb(ray: SpatialRay, min: Vec3, max: Vec3) -> Option<f32> {
    let inv = |d: f32| if d.abs() < 1e-8 { f32::INFINITY } else { 1.0 / d };
    let mut tmin = 0.0f32;
    let mut tmax = f32::INFINITY;
    for (o, d, a, b) in [
        (ray.origin.x, ray.dir.x, min.x, max.x),
        (ray.origin.y, ray.dir.y, min.y, max.y),
        (ray.origin.z, ray.dir.z, min.z, max.z),
    ] {
        let inv_d = inv(d);
        let mut t0 = (a - o) * inv_d;
        let mut t1 = (b - o) * inv_d;
        if inv_d < 0.0 {
            std::mem::swap(&mut t0, &mut t1);
        }
        tmin = tmin.max(t0);
        tmax = tmax.min(t1);
        if tmax < tmin {
            return None;
        }
    }
    if tmin >= 0.0 {
        Some(tmin)
    } else if tmax >= 0.0 {
        Some(tmax)
    } else {
        None
    }
}
