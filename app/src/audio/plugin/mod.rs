mod catalog;
mod host;
mod ladspa;
mod lv2;
mod clap_stub;

#[cfg(feature = "vst3-carla")]
mod vst3_carla;

pub use catalog::{
    apply_denoiser_preset, denoiser_preset_names, normalize_label, plugin_file_for,
    plugin_ref_with_defaults, ui_spec_for_ref, ParamDef, ParamKind,
};
pub use host::*;
#[allow(unused_imports)]
pub use ladspa::LadspaBackend;
#[allow(unused_imports)]
pub use lv2::Lv2Backend;
#[allow(unused_imports)]
pub use clap_stub::ClapBackend;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

fn new_slot_id() -> Uuid {
    Uuid::new_v4()
}

fn default_mix() -> f32 {
    1.0
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum PluginFormat {
    Ladspa,
    Lv2,
    Clap,
    Vst3,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PluginId {
    pub format: PluginFormat,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDescriptor {
    pub id: PluginId,
    pub name: String,
    pub maker: String,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRef {
    pub id: PluginId,
    pub bypass: bool,
    /// Wet/dry mix: 0.0 = fully dry (bypass FX), 1.0 = fully wet.
    #[serde(default = "default_mix")]
    pub mix: f32,
    /// Stable per-insert identity for per-slot FX processes / mids.
    #[serde(default = "new_slot_id")]
    pub slot_id: Uuid,
    pub params: Vec<(String, f32)>,
}

impl PluginRef {
    /// Control values pushed to a live filter-chain node (graph-free).
    /// `PluginRef.mix` owns Mix ports; insert power folds into Bypass/Mix/Enable —
    /// no module unload, no topology change.
    pub fn effective_control_params(&self) -> Vec<(String, f32)> {
        let mut params = self.params.clone();
        if let Some((_, v)) = params.iter_mut().find(|(k, _)| k == "Mix") {
            *v = self.mix.clamp(0.0, 1.0);
        }
        if self.bypass {
            for (k, v) in &mut params {
                match k.as_str() {
                    "Mix" | "Wet" | "Dry/Wet" | "Blend" => *v = 0.0,
                    "Bypass" => *v = 1.0,
                    "Enable" => *v = 0.0,
                    _ => {}
                }
            }
        } else {
            // Powered on: clear any stale Bypass/Enable left in session params.
            for (k, v) in &mut params {
                match k.as_str() {
                    "Bypass" => *v = 0.0,
                    "Enable" => *v = 1.0,
                    _ => {}
                }
            }
        }
        params
    }
}

pub trait PluginBackend: Send {
    fn format(&self) -> PluginFormat;
    fn scan(&self) -> Vec<PluginDescriptor>;
}

pub trait PluginInstance: Send {
    fn set_param(&mut self, key: &str, value: f32);
    fn as_ladspa_label(&self) -> Option<&str> {
        None
    }
}
