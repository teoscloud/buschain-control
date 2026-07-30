use super::math::{Mat4, Rgba, Vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VizQuality {
    /// Mesh + rays + metrics — readability first.
    Essential,
    /// Soft fog, bloom discs, sparks, sheen, slow camera drift.
    Cinematic,
}

impl Default for VizQuality {
    fn default() -> Self {
        Self::Cinematic
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SpatialTheme {
    pub concrete: Rgba,
    pub tungsten: Rgba,
    pub measure_cyan: Rgba,
    pub fog: Rgba,
    pub emission: Rgba,
}

impl Default for SpatialTheme {
    fn default() -> Self {
        Self {
            concrete: Rgba::new(0.42, 0.40, 0.38, 1.0),
            tungsten: Rgba::new(0.92, 0.72, 0.42, 1.0),
            measure_cyan: Rgba::new(0.25, 0.78, 0.82, 1.0),
            fog: Rgba::new(0.55, 0.52, 0.48, 0.12),
            emission: Rgba::new(0.95, 0.78, 0.45, 0.55),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SpatialCamera {
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub fovy_deg: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for SpatialCamera {
    fn default() -> Self {
        Self {
            eye: Vec3::new(2.8, 1.6, 3.4),
            target: Vec3::new(0.0, 0.4, 0.0),
            up: Vec3::Y,
            fovy_deg: 42.0,
            near: 0.05,
            far: 40.0,
        }
    }
}

impl SpatialCamera {
    pub fn view_proj(&self, aspect: f32) -> (Mat4, Mat4) {
        let view = Mat4::look_at(self.eye, self.target, self.up);
        let proj = Mat4::perspective(self.fovy_deg.to_radians(), aspect, self.near, self.far);
        (view, proj)
    }
}

/// Host viewport contract — scenes never assume egui.
#[derive(Clone, Copy, Debug)]
pub struct SpatialViewport {
    pub width_px: f32,
    pub height_px: f32,
    pub dpi: f32,
}

impl SpatialViewport {
    pub fn aspect(self) -> f32 {
        (self.width_px / self.height_px.max(1.0)).max(0.2)
    }
}

#[derive(Clone, Debug)]
pub struct SpatialFrame {
    pub camera: SpatialCamera,
    pub theme: SpatialTheme,
    pub quality: VizQuality,
    pub time_s: f64,
    pub viewport: SpatialViewport,
}

impl SpatialFrame {
    pub fn view_proj(&self) -> (Mat4, Mat4) {
        self.camera.view_proj(self.viewport.aspect())
    }
}
