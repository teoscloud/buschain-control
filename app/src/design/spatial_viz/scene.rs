use super::draw_list::SpatialDrawList;
use super::frame::SpatialFrame;
use super::hit::{SpatialHit, SpatialRay};
use super::metrics::SpatialMetricBus;

pub trait SpatialScene {
    fn build(&self, frame: &SpatialFrame, metrics: &SpatialMetricBus) -> SpatialDrawList;

    fn hit_test(&self, list: &SpatialDrawList, ray: SpatialRay) -> Option<SpatialHit> {
        super::hit::pick(list, ray)
    }
}
