use crate::design::spatial_viz::draw_list::SpatialDrawList;
use crate::design::spatial_viz::frame::SpatialFrame;

/// Pixel rect in host coordinates (backend maps to painter / swapchain).
#[derive(Clone, Copy, Debug)]
pub struct BackendRect {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

impl BackendRect {
    pub fn width(self) -> f32 {
        (self.max_x - self.min_x).max(1.0)
    }
    pub fn height(self) -> f32 {
        (self.max_y - self.min_y).max(1.0)
    }
}

pub trait SpatialBackend {
    fn begin_frame(&mut self, frame: &SpatialFrame, viewport: BackendRect);
    fn submit(&mut self, list: &SpatialDrawList);
    fn end_frame(&mut self);
}

pub mod egui;

#[cfg(feature = "spatial-wgpu")]
pub mod wgpu;
