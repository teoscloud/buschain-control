use super::draw_list::{FieldCmd, SpatialCmd, SpatialDrawList};
use super::math::{Rgba, Vec3};

pub fn push_fog(
    list: &mut SpatialDrawList,
    center: Vec3,
    half_extents: Vec3,
    density: f32,
    rgba: Rgba,
    slices: u8,
    sort_key: f32,
) {
    list.push(SpatialCmd::Field(FieldCmd {
        center,
        half_extents,
        density: density.clamp(0.0, 1.0),
        rgba,
        quality_slices: slices.clamp(1, 16),
        sort_key,
    }));
}
