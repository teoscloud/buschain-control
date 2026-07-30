mod catalog;
mod host;
mod ladspa;
mod lv2;
mod clap_stub;

mod vst3_carla;

pub use catalog::{
    apply_denoiser_preset, apply_equalizer_preset, apply_limiter_preset, apply_reverb_preset,
    denoiser_preset_names, dynamic_ui_for_ref, equalizer_preset_names, limiter_preset_names,
    normalize_label, plugin_file_for, plugin_ref_with_defaults, plugin_title_for_ref,
    reverb_preset_names, ui_spec_for_ref, OwnedParamDef, DynamicPluginUiSpec, ParamDef, ParamKind,
    PluginUiSpec,
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
    /// Opaque CLAP/VST3 state chunk for session restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_blob: Option<Vec<u8>>,
    /// Sidechain source bus/track sink name (P7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidechain_from: Option<String>,
}

impl PluginRef {
    /// Control values for a live filter-chain node (graph-free Props).
    ///
    /// Black-box host: write only advertised ports. User Mix updates a Mix port
    /// if present. Insert power sets Bypass/Enable only when those ports exist —
    /// never invent Mix-as-power or other plugin-specific DSP semantics.
    pub fn effective_control_params(&self) -> Vec<(String, f32)> {
        let mut params = self.params.clone();
        if let Some((_, v)) = params.iter_mut().find(|(k, _)| k == "Mix") {
            *v = self.mix.clamp(0.0, 1.0);
        }
        let has_bypass = params.iter().any(|(k, _)| k == "Bypass");
        let has_enable = params.iter().any(|(k, _)| k == "Enable");
        if self.bypass {
            if has_bypass {
                for (k, v) in &mut params {
                    if k == "Bypass" {
                        *v = 1.0;
                    }
                }
            }
            if has_enable {
                for (k, v) in &mut params {
                    if k == "Enable" {
                        *v = 0.0;
                    }
                }
            }
        } else {
            if has_bypass {
                for (k, v) in &mut params {
                    if k == "Bypass" {
                        *v = 0.0;
                    }
                }
            }
            if has_enable {
                for (k, v) in &mut params {
                    if k == "Enable" {
                        *v = 1.0;
                    }
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
