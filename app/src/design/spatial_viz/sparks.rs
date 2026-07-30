use super::draw_list::{GlyphCmd, GlyphKind, SpatialCmd, SpatialDrawList, SparkCmd};
use super::math::{Rgba, Vec3};

pub fn push_spark(
    list: &mut SpatialDrawList,
    pos: Vec3,
    vel: Vec3,
    life: f32,
    rgba: Rgba,
    sort_key: f32,
) {
    list.push(SpatialCmd::Spark(SparkCmd {
        pos,
        vel,
        life,
        rgba,
        sort_key,
    }));
}

pub fn push_glyph(
    list: &mut SpatialDrawList,
    kind: GlyphKind,
    pos: Vec3,
    bloom: f32,
    rgba: Rgba,
    sort_key: f32,
) {
    list.push(SpatialCmd::Glyph(GlyphCmd {
        kind,
        pos,
        bloom,
        rgba,
        sort_key,
    }));
}
