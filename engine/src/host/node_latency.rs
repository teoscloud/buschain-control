//! Rack latency bookkeeping + filter-thread ProcessLatency publish hook.

use super::pdc;
use super::registry;

/// Record latency, update Master-bus PDC targets for **all** peers, and note that SPA
/// `ProcessLatency` must be applied on the filter mainloop thread (never from
/// the worker — wrong-context). Filter thread observes `latency_samples` atomic.
pub fn publish_bus_latency(bus: &str) {
    let lat = registry::host_latency(bus);
    pdc::set_bus_latency(bus, lat);
    apply_all_pdc_delays();
}

/// Push every peer's compensation delay into its live host (after map recompute).
pub fn apply_all_pdc_delays() {
    for (bus, delay) in pdc::all_compensation_targets() {
        registry::set_host_pdc_delay(&bus, delay);
    }
}

/// UI helper: reported rack latency in samples.
pub fn reported_latency(bus: &str) -> u32 {
    pdc::reported_latency(bus)
}

/// UI helper: compensation delay applied for Master-bus align.
pub fn compensation_latency(bus: &str) -> u32 {
    pdc::compensation_samples(bus)
}
