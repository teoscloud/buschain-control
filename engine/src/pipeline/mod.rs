//! High-level pipeline steps built on the audio backend.

pub mod arm;
pub mod glc;
pub mod insert;
mod insert_host;

pub use arm::{
    arm_master_hw, arm_track_egress, arm_track_egress_soft_cutover, disarm_master_hw,
    disarm_track_egress, disarm_track_egress_ex, dry_spine_instant_ready, spine_instant_ready,
    track_path_ready,
    wait_dry_spine_stable, wait_spine_stable, DRY_DWELL, WET_DWELL,
};
pub use glc::{
    glc_name_for_bus, is_disabled as glc_is_disabled, is_glc_node, l_star_samples,
    master_pad_samples, path_latency_samples, reconcile as reconcile_glc,
};
pub use insert::{
    ensure_fx_chain, probe_chain_state, push_fx_controls, teardown_fx_chain,
};
