//! Stub delay/tap scene — proves SpatialViz is not reverb-shaped.

use crate::design::spatial_viz::draw_list::{SpatialDrawList, SpatialMaterial, SpatialMeshId};
use crate::design::spatial_viz::frame::SpatialFrame;
use crate::design::spatial_viz::math::{Rgba, Vec3};
use crate::design::spatial_viz::mesh;
use crate::design::spatial_viz::metrics::SpatialMetricBus;
use crate::design::spatial_viz::rays;
use crate::design::spatial_viz::scene::SpatialScene;

#[derive(Clone, Debug, Default)]
pub struct TapFieldScene {
    pub taps_ms: Vec<f32>,
    pub feedback: f32,
}

impl SpatialScene for TapFieldScene {
    fn build(&self, frame: &SpatialFrame, _metrics: &SpatialMetricBus) -> SpatialDrawList {
        let mut list = SpatialDrawList::new();
        mesh::push_plane(
            &mut list,
            SpatialMeshId(100),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(3.0, 0.02, 1.2),
            SpatialMaterial {
                albedo: frame.theme.concrete.with_alpha(0.9),
                roughness: 0.9,
                emission: 0.0,
                absorption: 0.3,
            },
            10.0,
        );
        let cyan = frame.theme.measure_cyan;
        for (i, &ms) in self.taps_ms.iter().enumerate() {
            let x = -1.4 + (ms / 500.0).clamp(0.0, 1.0) * 2.8;
            let y = 0.15 + self.feedback * 0.4;
            rays::push_polyline(
                &mut list,
                vec![Vec3::new(x, 0.0, 0.0), Vec3::new(x, y, 0.0)],
                2.0,
                cyan.with_alpha(0.7),
                2.0 + i as f32 * 0.01,
            );
        }
        let _ = Rgba::new(0.0, 0.0, 0.0, 0.0);
        list
    }
}
