//! SpatialViz — backend-agnostic 3D audio graphs.
//!
//! Scenes emit [`SpatialDrawList`] only. Never import `egui` / `wgpu` in scene modules.

pub mod backend;
pub mod draw_list;
pub mod field;
pub mod frame;
pub mod hit;
pub mod math;
pub mod mesh;
pub mod metrics;
pub mod rays;
pub mod scene;
pub mod scenes;
pub mod sparks;

pub use backend::{BackendRect, SpatialBackend};
pub use backend::egui::EguiPainterBackend;
pub use draw_list::{SpatialDrawList, SpatialMaterial, SpatialMeshId};
pub use frame::{SpatialCamera, SpatialFrame, SpatialTheme, SpatialViewport, VizQuality};
pub use hit::{SpatialHit, SpatialRay};
pub use metrics::{
    SpatialMetricBus, METRIC_BAND_T60_HI, METRIC_BAND_T60_LO, METRIC_BAND_T60_MID, METRIC_DUCK_GR,
    METRIC_ECHO_DENSITY, METRIC_ER_TAIL, METRIC_RT60, METRIC_WET_PEAK, REVERB_METRIC_IDS,
};
pub use scene::SpatialScene;
pub use scenes::reverb_room::ReverbRoomParams;
pub use scenes::{ReverbRoomScene, TapFieldScene};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_field_stub_builds_without_backend() {
        let scene = TapFieldScene {
            taps_ms: vec![20.0, 80.0, 160.0],
            feedback: 0.4,
        };
        let frame = SpatialFrame {
            camera: SpatialCamera::default(),
            theme: SpatialTheme::default(),
            quality: VizQuality::Essential,
            time_s: 0.0,
            viewport: SpatialViewport {
                width_px: 400.0,
                height_px: 240.0,
                dpi: 1.0,
            },
        };
        let metrics = SpatialMetricBus::new();
        let list = scene.build(&frame, &metrics);
        assert!(!list.cmds.is_empty());
    }

    #[test]
    fn reverb_room_scene_is_egui_free_draw_list() {
        let scene = ReverbRoomScene::default();
        let frame = SpatialFrame {
            camera: SpatialCamera::default(),
            theme: SpatialTheme::default(),
            quality: VizQuality::Cinematic,
            time_s: 1.0,
            viewport: SpatialViewport {
                width_px: 600.0,
                height_px: 360.0,
                dpi: 1.0,
            },
        };
        let mut metrics = SpatialMetricBus::new();
        metrics.set(METRIC_RT60, 2.0);
        metrics.set(METRIC_ECHO_DENSITY, 0.7);
        let list = scene.build(&frame, &metrics);
        assert!(list.cmds.len() > 5);
    }
}
