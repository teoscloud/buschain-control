//! Active FX generation per bus — canonical vs staging (`__stg`) for A/B cutover.
//!
//! Props and spine probes must target the *live* generation, not always
//! `fx_name_for_bus` (canonical). After a warm A/B flip, live may be `__stg`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::backend::sink_exists;
use crate::domain::{fx_name_for_bus, post_name_for_bus};

#[derive(Clone)]
struct Gen {
    fx: String,
    post: String,
}

fn map() -> &'static Mutex<HashMap<String, Gen>> {
    static M: OnceLock<Mutex<HashMap<String, Gen>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn canonical_fx(bus: &str) -> String {
    fx_name_for_bus(bus)
}

pub fn canonical_post(bus: &str) -> String {
    post_name_for_bus(bus)
}

pub fn staging_fx(bus: &str) -> String {
    format!("{}__stg", fx_name_for_bus(bus))
}

pub fn staging_post(bus: &str) -> String {
    format!("{}__stg", post_name_for_bus(bus))
}

fn claimed_fx(bus: &str) -> String {
    map()
        .lock()
        .ok()
        .and_then(|g| g.get(bus).map(|x| x.fx.clone()))
        .unwrap_or_else(|| canonical_fx(bus))
}

fn claimed_post(bus: &str) -> String {
    map()
        .lock()
        .ok()
        .and_then(|g| g.get(bus).map(|x| x.post.clone()))
        .unwrap_or_else(|| canonical_post(bus))
}

/// Heal pointer if claimed generation is gone but the other slot is alive.
pub fn heal_live_gen(bus: &str) {
    let claimed = claimed_fx(bus);
    if sink_exists(&claimed) {
        return;
    }
    let (alt_fx, alt_post) = if is_staging_name(&claimed) {
        (canonical_fx(bus), canonical_post(bus))
    } else {
        (staging_fx(bus), staging_post(bus))
    };
    if sink_exists(&alt_fx) {
        set_live_gen(bus, &alt_fx, &alt_post);
        crate::fx_trace::log("AbHeal", &alt_fx, 0);
    }
}

pub fn live_fx_name(bus: &str) -> String {
    heal_live_gen(bus);
    claimed_fx(bus)
}

pub fn live_post_name(bus: &str) -> String {
    heal_live_gen(bus);
    claimed_post(bus)
}

pub fn set_live_gen(bus: &str, fx: &str, post: &str) {
    if let Ok(mut g) = map().lock() {
        g.insert(
            bus.to_string(),
            Gen {
                fx: fx.to_string(),
                post: post.to_string(),
            },
        );
    }
}

pub fn clear_live_gen(bus: &str) {
    if let Ok(mut g) = map().lock() {
        g.remove(bus);
    }
}

/// Opposite generation from whatever is currently live (for warm A/B spawn).
pub fn staging_pair(bus: &str) -> (String, String) {
    let live_fx = live_fx_name(bus);
    let can_fx = canonical_fx(bus);
    if live_fx == can_fx {
        (staging_fx(bus), staging_post(bus))
    } else {
        (can_fx, canonical_post(bus))
    }
}

pub fn is_staging_name(name: &str) -> bool {
    name.ends_with("__stg")
}

/// True when either generation helper is present (warm A/B eligible).
pub fn any_gen_live(bus: &str) -> bool {
    sink_exists(&canonical_fx(bus)) || sink_exists(&staging_fx(bus))
}
