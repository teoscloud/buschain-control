//! Rack latency bookkeeping + filter-thread ProcessLatency publish hook.

use super::pdc;
use super::registry;

/// Record rack latency, refresh host peer pads when Master Direct (GLC off), and
/// refresh Master-edge GLC δ from the cached egress topology when GLC is on.
pub fn publish_bus_latency(bus: &str) {
    let lat = registry::host_latency(bus);
    pdc::set_bus_latency(bus, lat);
    apply_all_pdc_delays();
    if let Ok(plan) = crate::pipeline::glc::recompute_from_cached_topo() {
        crate::pipeline::glc::apply_delays_to_live_nodes(&plan);
    }
}

/// Push host peer pads when GLC is disabled; keep host pads at 0 when GLC is on.
pub fn apply_all_pdc_delays() {
    let glc_off = crate::pipeline::glc::is_disabled();
    for (bus, delay) in pdc::all_compensation_targets() {
        let d = if glc_off { delay } else { 0 };
        registry::set_host_pdc_delay(&bus, d);
    }
}

/// UI helper: reported rack latency in samples.
pub fn reported_latency(bus: &str) -> u32 {
    pdc::reported_latency(bus)
}

/// UI helper: compensation delay — GLC Master pad when enabled, else host peer pad.
pub fn compensation_latency(bus: &str) -> u32 {
    if crate::pipeline::glc::is_disabled() {
        pdc::compensation_samples(bus)
    } else {
        crate::pipeline::glc::master_pad_samples(bus)
    }
}
