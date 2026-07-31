use super::draw_list::{
    MeshCmd, MeshKind, ShellCmd, SpatialCmd, SpatialDrawList, SpatialMaterial, SpatialMeshId,
};
use super::math::{SpatialTransform, Vec3};

pub fn push_box(
    list: &mut SpatialDrawList,
    id: SpatialMeshId,
    center: Vec3,
    half_extents: Vec3,
    material: SpatialMaterial,
    sort_key: f32,
) {
    list.push(SpatialCmd::Mesh(MeshCmd {
        id,
        transform: SpatialTransform {
            translation: center,
            scale: half_extents * 2.0,
            yaw: 0.0,
        },
        kind: MeshKind::Box,
        material,
        sort_key,
    }));
}

pub fn push_plane(
    list: &mut SpatialDrawList,
    id: SpatialMeshId,
    center: Vec3,
    size: Vec3,
    material: SpatialMaterial,
    sort_key: f32,
) {
    list.push(SpatialCmd::Mesh(MeshCmd {
        id,
        transform: SpatialTransform {
            translation: center,
            scale: size,
            yaw: 0.0,
        },
        kind: MeshKind::Plane,
        material,
        sort_key,
    }));
}

pub fn push_shell(
    list: &mut SpatialDrawList,
    room_type: u8,
    half_extents: Vec3,
    face_heat: [f32; 6],
    material: SpatialMaterial,
    segments: u8,
    sort_key: f32,
) {
    list.push(SpatialCmd::Shell(ShellCmd {
        room_type,
        half_extents,
        face_heat,
        material,
        segments,
        sort_key,
    }));
}
