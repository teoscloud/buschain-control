//! VST3 surface promotion — slots that host DSP+GUI in `buschain-plugin-surface`.

use std::collections::HashSet;
use std::sync::Mutex;

use once_cell::sync::Lazy;
use uuid::Uuid;

static SURFACE_SLOTS: Lazy<Mutex<HashSet<Uuid>>> = Lazy::new(|| Mutex::new(HashSet::new()));

/// Kill-switch: `BUSCHAIN_VST3_SURFACE=0` disables promote-to-surface.
pub fn surface_enabled() -> bool {
    match std::env::var("BUSCHAIN_VST3_SURFACE") {
        Ok(v) => {
            let v = v.trim();
            !(v == "0" || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("off"))
        }
        Err(_) => true,
    }
}

pub fn mark_surface_slot(slot_id: Uuid) {
    if let Ok(mut g) = SURFACE_SLOTS.lock() {
        g.insert(slot_id);
    }
}

pub fn clear_surface_slot(slot_id: Uuid) {
    if let Ok(mut g) = SURFACE_SLOTS.lock() {
        g.remove(&slot_id);
    }
}

pub fn slot_wants_surface(slot_id: Uuid) -> bool {
    surface_enabled()
        && SURFACE_SLOTS
            .lock()
            .map(|g| g.contains(&slot_id))
            .unwrap_or(false)
}
