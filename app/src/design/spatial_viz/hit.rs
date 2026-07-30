use super::draw_list::{MeshKind, SpatialCmd, SpatialDrawList};
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

/// CPU raycast against draw-list geometry (works for both backends).
pub fn pick(list: &SpatialDrawList, ray: SpatialRay) -> Option<SpatialHit> {
    let mut best: Option<SpatialHit> = None;
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
                    if best.as_ref().map(|b| t < b.t).unwrap_or(true) {
                        best = Some(hit);
                    }
                }
            }
            SpatialCmd::Glyph(g) => {
                let d = g.pos - ray.origin;
                let t = d.dot(ray.dir);
                if t > 0.0 {
                    let closest = ray.origin + ray.dir * t;
                    if (closest - g.pos).length() < 0.25 {
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
                        if best.as_ref().map(|b| t < b.t).unwrap_or(true) {
                            best = Some(hit);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    best
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
