use super::draw_list::{RayCmd, SpatialCmd, SpatialDrawList};
use super::math::{Rgba, Vec3};

pub fn push_polyline(
    list: &mut SpatialDrawList,
    points: Vec<Vec3>,
    width: f32,
    rgba: Rgba,
    sort_key: f32,
) {
    if points.len() < 2 {
        return;
    }
    list.push(SpatialCmd::Ray(RayCmd {
        points,
        width,
        rgba,
        sort_key,
    }));
}
