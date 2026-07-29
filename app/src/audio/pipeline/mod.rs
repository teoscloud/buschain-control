//! Thin re-exports of [`buschain_engine`] clock contracts for older call sites.
//!
//! All PipeWire/Pulse I/O lives in `buschain-engine`. Do not add `pactl`/`pw-link`
//! call sites here — extend the engine instead.

#![allow(unused_imports)]

pub use buschain_engine::{
    probe_master_hw, resolve_profile, AudioPreset, DeviceCaps, PerformanceProfile,
};

pub use crate::audio::engine_handle::{
    ensure_link_raw as ensure_link, link_is_live, teardown_links as teardown_buschain_links,
    unlink_from_source_except,
};

/// Apply performance profile props onto a null-sink create argument bundle.
pub fn null_sink_latency_args(profile: &PerformanceProfile) -> String {
    format!(
        "sink_properties=media.name=buschain-control session.suspend-timeout-seconds={} node.latency={}",
        if profile.force_suspend_timeout_zero {
            "0"
        } else {
            "5"
        },
        profile.node_latency_prop()
    )
}
