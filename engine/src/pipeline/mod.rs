//! High-level pipeline steps built on the audio backend.

pub mod arm;
pub mod insert;

pub use arm::{
    arm_master_hw, arm_track_egress, arm_track_egress_soft_cutover, arm_wet_ab_cutover,
    disarm_master_hw, disarm_track_egress, dry_spine_instant_ready, spine_instant_ready,
    track_path_ready, wait_dry_spine_stable, wait_spine_stable, DRY_DWELL, WET_DWELL,
};
pub use insert::{
    ensure_fx_chain, probe_chain_state, push_fx_controls, teardown_fx_chain,
};
