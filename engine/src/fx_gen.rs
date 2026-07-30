//! Active FX names per bus — canonical only (in-process host, no A/B gens).

use crate::backend::sink_exists;
use crate::domain::{fx_name_for_bus, post_name_for_bus};
use crate::host::registry;

pub fn canonical_fx(bus: &str) -> String {
    fx_name_for_bus(bus)
}

pub fn canonical_post(bus: &str) -> String {
    post_name_for_bus(bus)
}

/// Alias — staging names are no longer used.
pub fn staging_fx(bus: &str) -> String {
    canonical_fx(bus)
}

/// Alias — staging names are no longer used.
pub fn staging_post(bus: &str) -> String {
    canonical_post(bus)
}

/// No-op — live names are always canonical.
pub fn heal_live_gen(_bus: &str) {}

pub fn live_fx_name(bus: &str) -> String {
    canonical_fx(bus)
}

pub fn live_post_name(bus: &str) -> String {
    canonical_post(bus)
}

pub fn set_live_gen(_bus: &str, _fx: &str, _post: &str) {}

pub fn clear_live_gen(_bus: &str) {}

/// Both slots resolve to canonical (A/B retired).
pub fn staging_pair(bus: &str) -> (String, String) {
    (canonical_fx(bus), canonical_post(bus))
}

pub fn is_staging_name(name: &str) -> bool {
    name.ends_with("__stg")
}

/// True when the in-process host is up, or a legacy Pulse helper remains.
pub fn any_gen_live(bus: &str) -> bool {
    if registry::host_running(bus) {
        return true;
    }
    // Out-of-process leftovers only — duplex PwFxNode is never a Pulse sink.
    sink_exists(&canonical_fx(bus))
}
