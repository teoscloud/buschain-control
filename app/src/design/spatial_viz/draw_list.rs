use super::math::{Rgba, SpatialTransform, Vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SpatialMeshId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshKind {
    Box,
    Plane,
    Capsule,
}

#[derive(Clone, Copy, Debug)]
pub struct SpatialMaterial {
    pub albedo: Rgba,
    pub roughness: f32,
    pub emission: f32,
    pub absorption: f32,
}

impl Default for SpatialMaterial {
    fn default() -> Self {
        Self {
            albedo: Rgba::new(0.5, 0.5, 0.5, 1.0),
            roughness: 0.7,
            emission: 0.0,
            absorption: 0.2,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MeshCmd {
    pub id: SpatialMeshId,
    pub transform: SpatialTransform,
    pub kind: MeshKind,
    pub material: SpatialMaterial,
    pub sort_key: f32,
}

#[derive(Clone, Debug)]
pub struct FieldCmd {
    pub center: Vec3,
    pub half_extents: Vec3,
    pub density: f32,
    pub rgba: Rgba,
    pub quality_slices: u8,
    pub sort_key: f32,
}

#[derive(Clone, Debug)]
pub struct RayCmd {
    pub points: Vec<Vec3>,
    pub width: f32,
    pub rgba: Rgba,
    pub sort_key: f32,
}

#[derive(Clone, Debug)]
pub struct SparkCmd {
    pub pos: Vec3,
    pub vel: Vec3,
    pub life: f32,
    pub rgba: Rgba,
    pub sort_key: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlyphKind {
    Source,
    Listener,
}

#[derive(Clone, Debug)]
pub struct GlyphCmd {
    pub kind: GlyphKind,
    pub pos: Vec3,
    /// World yaw in radians (0 = +Z); used to orient Source cones.
    pub yaw: f32,
    pub bloom: f32,
    pub rgba: Rgba,
    pub sort_key: f32,
}

/// Room shell — room_type matches BusChain Room LADSPA enum (0–6).
/// `half_extents`: (half_width, half_height, half_depth); floor at y=0, roof at y=2*hy.
/// `face_heat`: 0–1 energy per face (+X −X +Z −Z floor roof / band proxies).
#[derive(Clone, Debug)]
pub struct ShellCmd {
    pub room_type: u8,
    pub half_extents: Vec3,
    pub face_heat: [f32; 6],
    pub material: SpatialMaterial,
    pub segments: u8,
    pub sort_key: f32,
}

#[derive(Clone, Debug)]
pub enum SpatialCmd {
    Mesh(MeshCmd),
    Field(FieldCmd),
    Ray(RayCmd),
    Spark(SparkCmd),
    Glyph(GlyphCmd),
    Shell(ShellCmd),
}

impl SpatialCmd {
    pub fn sort_key(&self) -> f32 {
        match self {
            Self::Mesh(c) => c.sort_key,
            Self::Field(c) => c.sort_key,
            Self::Ray(c) => c.sort_key,
            Self::Spark(c) => c.sort_key,
            Self::Glyph(c) => c.sort_key,
            Self::Shell(c) => c.sort_key,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SpatialDrawList {
    pub cmds: Vec<SpatialCmd>,
}

impl SpatialDrawList {
    pub fn new() -> Self {
        Self { cmds: Vec::new() }
    }

    pub fn push(&mut self, cmd: SpatialCmd) {
        self.cmds.push(cmd);
    }

    pub fn sort_by_depth(&mut self) {
        self.cmds
            .sort_by(|a, b| {
                a.sort_key()
                    .partial_cmp(&b.sort_key())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
    }
}
