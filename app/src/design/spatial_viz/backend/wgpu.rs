//! Placeholder WebGPU backend — enable with `spatial-wgpu`. Scenes stay unchanged.

use crate::design::spatial_viz::backend::{BackendRect, SpatialBackend};
use crate::design::spatial_viz::draw_list::SpatialDrawList;
use crate::design::spatial_viz::frame::SpatialFrame;

pub struct WgpuBackend;

impl SpatialBackend for WgpuBackend {
    fn begin_frame(&mut self, _frame: &SpatialFrame, _viewport: BackendRect) {
        // Wire swapchain / egui-wgpu here in a future PR.
    }

    fn submit(&mut self, _list: &SpatialDrawList) {
        // Translate SpatialDrawList → GPU buffers.
    }

    fn end_frame(&mut self) {}
}
