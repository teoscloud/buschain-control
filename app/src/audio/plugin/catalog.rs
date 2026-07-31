//! Built-in BusChain LADSPA control-port catalogs (order matches C sources).
//! `module-ladspa-sink` `control=` is a comma-separated list in this order.

use super::{PluginFormat, PluginId, PluginRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    Slider,
    /// 0/1 toggle
    Toggle,
    /// Discrete integer modes shown as combo
    Mode,
}

#[derive(Debug, Clone)]
pub struct ParamDef {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: ParamKind,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// Optional mode names when kind == Mode (index = value).
    pub modes: Option<&'static [&'static str]>,
    pub logarithmic: bool,
}

/// Owned counterpart for discovered CLAP/VST3/LV2 params (non-catalog plugins).
#[derive(Debug, Clone)]
pub struct OwnedParamDef {
    pub key: String,
    pub label: String,
    pub kind: ParamKind,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub modes: Option<Vec<String>>,
    pub logarithmic: bool,
}

#[derive(Debug, Clone)]
pub struct DynamicPluginUiSpec {
    pub title: String,
    pub params: Vec<OwnedParamDef>,
}

#[derive(Debug, Clone)]
pub struct PluginUiSpec {
    pub label: &'static str,
    pub title: &'static str,
    /// Pulse `plugin=` stem (without .so). Builtins share one .so.
    pub plugin_file: &'static str,
    pub params: &'static [ParamDef],
}

pub fn normalize_label(id: &str) -> &str {
    let id = id.trim();
    // listplugins lines like "buschain_compressor (392012/0)"
    let head = id.split_whitespace().next().unwrap_or(id);
    let head = head.strip_prefix("ladspa:").unwrap_or(head);
    // Pre-rebrand sessions stored shadow_* LADSPA labels.
    let head = head.strip_prefix("shadow_").map(|rest| {
        for known in KNOWN_LABELS {
            if known.strip_prefix("buschain_") == Some(rest) {
                return *known;
            }
        }
        head
    }).unwrap_or(head);
    // Forgotten Parametric / eq8 identity → Equalizer.
    if head == "buschain_eq8" || head.contains("buschain_eq8") || id.contains("buschain_eq8")
    {
        return "buschain_equalizer";
    }
    for known in KNOWN_LABELS {
        if head == *known || head.contains(known) || id.contains(known) {
            return known;
        }
    }
    head
}

const KNOWN_LABELS: &[&str] = &[
    "buschain_denoiser",
    "buschain_gate",
    "buschain_reverb",
    "buschain_softclip",
    "buschain_overdrive",
    "buschain_limiter",
    "buschain_compressor",
    // Before buschain_eq — "buschain_equalizer".contains("buschain_eq") would false-match.
    "buschain_equalizer",
    "buschain_eq",
    "buschain_pitch",
];

/// Denoiser tuner presets (denoise controls only — gate is a separate insert).
pub fn denoiser_preset_names() -> &'static [&'static str] {
    &[
        "default",
        "UserTuned",
        "PCM2902 Measured",
        "Mac Classic",
        "Gentle (generic)",
    ]
}

pub fn apply_denoiser_preset(plug: &mut PluginRef, name: &str) -> bool {
    let Some(values) = denoiser_preset_values(name) else {
        return false;
    };
    plug.ensure_params();
    for (k, v) in values {
        plug.set_param(k, *v);
    }
    true
}

fn denoiser_preset_values(name: &str) -> Option<&'static [(&'static str, f32)]> {
    match name {
        "default" => Some(PRESET_DEFAULT),
        "UserTuned" => Some(PRESET_USER_TUNED),
        "PCM2902 Measured" => Some(PRESET_PCM2902),
        "Mac Classic" | "Mac Bertom" => Some(PRESET_MAC), // legacy alias
        "Gentle (generic)" => Some(PRESET_GENTLE),
        _ => None,
    }
}

// Ship-as-default — matches pcm2902 mic defaults
static PRESET_DEFAULT: &[(&str, f32)] = &[
    ("Threshold (dB)", -71.4583),
    ("Range Band 1 (dB)", 12.0),
    ("Range Band 2 (dB)", 11.5),
    ("Range Band 3 (dB)", 8.0),
    ("Range Band 4 (dB)", 14.0),
    ("Range Band 5 (dB)", 12.5115),
    ("Range Band 6 (dB)", 8.8918),
    ("Band 1 Freq (Hz)", 120.0),
    ("Band 2 Freq (Hz)", 240.0),
    ("Band 3 Freq (Hz)", 600.0),
    ("Band 4 Freq (Hz)", 1580.0),
    ("Band 5 Freq (Hz)", 3000.0),
    ("Band 6 Freq (Hz)", 12000.0),
    ("HF Bias", 0.40),
    ("Stereo Link", 0.90),
    ("Stability", 0.45),
        ("NR1 On", 1.0),
    ("NR1 Freq (Hz)", 120.0),
    ("NR1 Depth (dB)", 12.0),
    ("NR1 Q", 1.5),
    ("NR2 On", 1.0),
    ("NR2 Freq (Hz)", 240.0),
    ("NR2 Depth (dB)", 11.5),
    ("NR2 Q", 1.5),
    ("NR3 On", 1.0),
    ("NR3 Freq (Hz)", 600.0),
    ("NR3 Depth (dB)", 8.0),
    ("NR3 Q", 1.5),
    ("NR4 On", 1.0),
    ("NR4 Freq (Hz)", 1580.0),
    ("NR4 Depth (dB)", 14.0),
    ("NR4 Q", 1.5),
    ("NR5 On", 1.0),
    ("NR5 Freq (Hz)", 3000.0),
    ("NR5 Depth (dB)", 12.5115),
    ("NR5 Q", 1.5),
    ("NR6 On", 1.0),
    ("NR6 Freq (Hz)", 12000.0),
    ("NR6 Depth (dB)", 8.8918),
    ("NR6 Q", 1.5),
    ("NR7 On", 0.0),
    ("NR7 Freq (Hz)", 1000.0),
    ("NR7 Depth (dB)", 0.0),
    ("NR7 Q", 1.5),
    ("NR8 On", 0.0),
    ("NR8 Freq (Hz)", 1000.0),
    ("NR8 Depth (dB)", 0.0),
    ("NR8 Q", 1.5),
    ("Gate Enable", 0.0),
    ("Gate Knee (dB)", 4.0),
    ("Gate Ratio", 12.0),
    ("Gate Attack (ms)", 1.2),
    ("Gate Release (ms)", 90.0),
    ("Gate Mix", 1.0),
("Bypass", 0.0),
];

static PRESET_USER_TUNED: &[(&str, f32)] = &[
    ("Threshold (dB)", -44.1146),
    ("Range Band 1 (dB)", 12.0),
    ("Range Band 2 (dB)", 11.5),
    ("Range Band 3 (dB)", 8.0),
    ("Range Band 4 (dB)", 14.0),
    ("Range Band 5 (dB)", 12.5115),
    ("Range Band 6 (dB)", 15.0295),
    ("Band 1 Freq (Hz)", 120.0),
    ("Band 2 Freq (Hz)", 240.0),
    ("Band 3 Freq (Hz)", 600.0),
    ("Band 4 Freq (Hz)", 1580.0),
    ("Band 5 Freq (Hz)", 3000.0),
    ("Band 6 Freq (Hz)", 12000.0),
    ("HF Bias", 0.40),
    ("Stereo Link", 0.90),
    ("Stability", 0.45),
        ("NR1 On", 1.0),
    ("NR1 Freq (Hz)", 120.0),
    ("NR1 Depth (dB)", 12.0),
    ("NR1 Q", 1.5),
    ("NR2 On", 1.0),
    ("NR2 Freq (Hz)", 240.0),
    ("NR2 Depth (dB)", 11.5),
    ("NR2 Q", 1.5),
    ("NR3 On", 1.0),
    ("NR3 Freq (Hz)", 600.0),
    ("NR3 Depth (dB)", 8.0),
    ("NR3 Q", 1.5),
    ("NR4 On", 1.0),
    ("NR4 Freq (Hz)", 1580.0),
    ("NR4 Depth (dB)", 14.0),
    ("NR4 Q", 1.5),
    ("NR5 On", 1.0),
    ("NR5 Freq (Hz)", 3000.0),
    ("NR5 Depth (dB)", 12.5115),
    ("NR5 Q", 1.5),
    ("NR6 On", 1.0),
    ("NR6 Freq (Hz)", 12000.0),
    ("NR6 Depth (dB)", 15.0295),
    ("NR6 Q", 1.5),
    ("NR7 On", 0.0),
    ("NR7 Freq (Hz)", 1000.0),
    ("NR7 Depth (dB)", 0.0),
    ("NR7 Q", 1.5),
    ("NR8 On", 0.0),
    ("NR8 Freq (Hz)", 1000.0),
    ("NR8 Depth (dB)", 0.0),
    ("NR8 Q", 1.5),
    ("Gate Enable", 0.0),
    ("Gate Knee (dB)", 4.0),
    ("Gate Ratio", 12.0),
    ("Gate Attack (ms)", 1.2),
    ("Gate Release (ms)", 90.0),
    ("Gate Mix", 1.0),
("Bypass", 0.0),
];

static PRESET_PCM2902: &[(&str, f32)] = &[
    ("Threshold (dB)", -69.0),
    ("Range Band 1 (dB)", 12.0),
    ("Range Band 2 (dB)", 11.5),
    ("Range Band 3 (dB)", 8.0),
    ("Range Band 4 (dB)", 14.0),
    ("Range Band 5 (dB)", 12.5),
    ("Range Band 6 (dB)", 10.0),
    ("Band 1 Freq (Hz)", 120.0),
    ("Band 2 Freq (Hz)", 240.0),
    ("Band 3 Freq (Hz)", 600.0),
    ("Band 4 Freq (Hz)", 1580.0),
    ("Band 5 Freq (Hz)", 3000.0),
    ("Band 6 Freq (Hz)", 12000.0),
    ("HF Bias", 0.40),
    ("Stereo Link", 0.90),
    ("Stability", 0.45),
        ("NR1 On", 1.0),
    ("NR1 Freq (Hz)", 120.0),
    ("NR1 Depth (dB)", 12.0),
    ("NR1 Q", 1.5),
    ("NR2 On", 1.0),
    ("NR2 Freq (Hz)", 240.0),
    ("NR2 Depth (dB)", 11.5),
    ("NR2 Q", 1.5),
    ("NR3 On", 1.0),
    ("NR3 Freq (Hz)", 600.0),
    ("NR3 Depth (dB)", 8.0),
    ("NR3 Q", 1.5),
    ("NR4 On", 1.0),
    ("NR4 Freq (Hz)", 1580.0),
    ("NR4 Depth (dB)", 14.0),
    ("NR4 Q", 1.5),
    ("NR5 On", 1.0),
    ("NR5 Freq (Hz)", 3000.0),
    ("NR5 Depth (dB)", 12.5),
    ("NR5 Q", 1.5),
    ("NR6 On", 1.0),
    ("NR6 Freq (Hz)", 12000.0),
    ("NR6 Depth (dB)", 10.0),
    ("NR6 Q", 1.5),
    ("NR7 On", 0.0),
    ("NR7 Freq (Hz)", 1000.0),
    ("NR7 Depth (dB)", 0.0),
    ("NR7 Q", 1.5),
    ("NR8 On", 0.0),
    ("NR8 Freq (Hz)", 1000.0),
    ("NR8 Depth (dB)", 0.0),
    ("NR8 Q", 1.5),
    ("Gate Enable", 0.0),
    ("Gate Knee (dB)", 4.0),
    ("Gate Ratio", 12.0),
    ("Gate Attack (ms)", 1.2),
    ("Gate Release (ms)", 90.0),
    ("Gate Mix", 1.0),
("Bypass", 0.0),
];

static PRESET_MAC: &[(&str, f32)] = &[
    ("Threshold (dB)", -88.2),
    ("Range Band 1 (dB)", 0.8),
    ("Range Band 2 (dB)", 0.8),
    ("Range Band 3 (dB)", 6.1),
    ("Range Band 4 (dB)", 11.4),
    ("Range Band 5 (dB)", 5.5),
    ("Range Band 6 (dB)", 5.5),
    ("Band 1 Freq (Hz)", 250.0),
    ("Band 2 Freq (Hz)", 574.0),
    ("Band 3 Freq (Hz)", 1300.0),
    ("Band 4 Freq (Hz)", 3000.0),
    ("Band 5 Freq (Hz)", 7000.0),
    ("Band 6 Freq (Hz)", 16000.0),
    ("HF Bias", 0.35),
    ("Stereo Link", 0.85),
    ("Stability", 0.45),
        ("NR1 On", 1.0),
    ("NR1 Freq (Hz)", 250.0),
    ("NR1 Depth (dB)", 0.8),
    ("NR1 Q", 1.5),
    ("NR2 On", 1.0),
    ("NR2 Freq (Hz)", 574.0),
    ("NR2 Depth (dB)", 0.8),
    ("NR2 Q", 1.5),
    ("NR3 On", 1.0),
    ("NR3 Freq (Hz)", 1300.0),
    ("NR3 Depth (dB)", 6.1),
    ("NR3 Q", 1.5),
    ("NR4 On", 1.0),
    ("NR4 Freq (Hz)", 3000.0),
    ("NR4 Depth (dB)", 11.4),
    ("NR4 Q", 1.5),
    ("NR5 On", 1.0),
    ("NR5 Freq (Hz)", 7000.0),
    ("NR5 Depth (dB)", 5.5),
    ("NR5 Q", 1.5),
    ("NR6 On", 1.0),
    ("NR6 Freq (Hz)", 16000.0),
    ("NR6 Depth (dB)", 5.5),
    ("NR6 Q", 1.5),
    ("NR7 On", 0.0),
    ("NR7 Freq (Hz)", 1000.0),
    ("NR7 Depth (dB)", 0.0),
    ("NR7 Q", 1.5),
    ("NR8 On", 0.0),
    ("NR8 Freq (Hz)", 1000.0),
    ("NR8 Depth (dB)", 0.0),
    ("NR8 Q", 1.5),
    ("Gate Enable", 0.0),
    ("Gate Knee (dB)", 4.0),
    ("Gate Ratio", 12.0),
    ("Gate Attack (ms)", 1.2),
    ("Gate Release (ms)", 90.0),
    ("Gate Mix", 1.0),
("Bypass", 0.0),
];

static PRESET_GENTLE: &[(&str, f32)] = &[
    ("Threshold (dB)", -48.0),
    ("Range Band 1 (dB)", 8.0),
    ("Range Band 2 (dB)", 9.0),
    ("Range Band 3 (dB)", 7.0),
    ("Range Band 4 (dB)", 8.0),
    ("Range Band 5 (dB)", 12.0),
    ("Range Band 6 (dB)", 14.0),
    ("Band 1 Freq (Hz)", 60.0),
    ("Band 2 Freq (Hz)", 180.0),
    ("Band 3 Freq (Hz)", 500.0),
    ("Band 4 Freq (Hz)", 1500.0),
    ("Band 5 Freq (Hz)", 4500.0),
    ("Band 6 Freq (Hz)", 12000.0),
    ("HF Bias", 0.35),
    ("Stereo Link", 0.85),
    ("Stability", 0.45),
        ("NR1 On", 1.0),
    ("NR1 Freq (Hz)", 60.0),
    ("NR1 Depth (dB)", 8.0),
    ("NR1 Q", 1.5),
    ("NR2 On", 1.0),
    ("NR2 Freq (Hz)", 180.0),
    ("NR2 Depth (dB)", 9.0),
    ("NR2 Q", 1.5),
    ("NR3 On", 1.0),
    ("NR3 Freq (Hz)", 500.0),
    ("NR3 Depth (dB)", 7.0),
    ("NR3 Q", 1.5),
    ("NR4 On", 1.0),
    ("NR4 Freq (Hz)", 1500.0),
    ("NR4 Depth (dB)", 8.0),
    ("NR4 Q", 1.5),
    ("NR5 On", 1.0),
    ("NR5 Freq (Hz)", 4500.0),
    ("NR5 Depth (dB)", 12.0),
    ("NR5 Q", 1.8),
    ("NR6 On", 1.0),
    ("NR6 Freq (Hz)", 12000.0),
    ("NR6 Depth (dB)", 14.0),
    ("NR6 Q", 2.0),
    ("NR7 On", 0.0),
    ("NR7 Freq (Hz)", 1000.0),
    ("NR7 Depth (dB)", 0.0),
    ("NR7 Q", 1.5),
    ("NR8 On", 0.0),
    ("NR8 Freq (Hz)", 1000.0),
    ("NR8 Depth (dB)", 0.0),
    ("NR8 Q", 1.5),
    ("Gate Enable", 0.0),
    ("Gate Knee (dB)", 4.0),
    ("Gate Ratio", 12.0),
    ("Gate Attack (ms)", 1.2),
    ("Gate Release (ms)", 90.0),
    ("Gate Mix", 1.0),
("Bypass", 0.0),
];

pub fn ui_spec_for(label: &str) -> Option<&'static PluginUiSpec> {
    let label = normalize_label(label);
    SPECS.iter().find(|s| s.label == label)
}

pub fn ui_spec_for_ref(plug: &PluginRef) -> Option<&'static PluginUiSpec> {
    if plug.id.format != PluginFormat::Ladspa {
        return None;
    }
    ui_spec_for(&plug.id.id)
}

fn infer_param_kind(name: &str, min: f32, max: f32, toggled: bool) -> ParamKind {
    if toggled || (min <= 0.0 && max <= 1.0 && name.to_ascii_lowercase().contains("bypass")) {
        return ParamKind::Toggle;
    }
    if toggled || (max - min <= 1.0 && min >= 0.0 && max <= 1.0 && name.len() <= 12) {
        let l = name.to_ascii_lowercase();
        if l.contains("enable") || l.contains("bypass") || l.ends_with(" on") {
            return ParamKind::Toggle;
        }
    }
    ParamKind::Slider
}

fn owned_defs_from_param_pairs(pairs: &[(String, f32)]) -> Vec<OwnedParamDef> {
    pairs
        .iter()
        .filter(|(k, _)| k != "Mix")
        .map(|(k, v)| OwnedParamDef {
            key: k.clone(),
            label: k.clone(),
            kind: infer_param_kind(k, 0.0, 1.0, false),
            min: 0.0,
            max: 1.0,
            default: *v,
            modes: None,
            logarithmic: false,
        })
        .collect()
}

fn probe_owned_defs(plug: &PluginRef) -> Vec<OwnedParamDef> {
    match plug.id.format {
        PluginFormat::Clap => {
            if let Ok(infos) =
                buschain_engine::probe_clap_params(&plug.id.id, &plug.id.id)
            {
                return infos
                    .into_iter()
                    .map(|p| OwnedParamDef {
                        key: p.name.clone(),
                        label: p.name.clone(),
                        kind: infer_param_kind(&p.name, p.min, p.max, false),
                        min: p.min,
                        max: p.max,
                        default: p.default,
                        modes: None,
                        logarithmic: p.max > p.min * 10.0 && p.min > 0.0,
                    })
                    .collect();
            }
        }
        PluginFormat::Lv2 => {
            let (uri, bundle) = parse_lv2_id(&plug.id.id);
            if let Ok(infos) =
                buschain_engine::probe_lv2_params(&uri, bundle.as_deref())
            {
                return infos
                    .into_iter()
                    .map(|p| OwnedParamDef {
                        key: p.name.clone(),
                        label: p.name.clone(),
                        kind: infer_param_kind(&p.name, p.min, p.max, p.toggled),
                        min: p.min,
                        max: p.max,
                        default: p.default,
                        modes: None,
                        logarithmic: false,
                    })
                    .collect();
            }
        }
        PluginFormat::Vst3 => {}
        PluginFormat::Ladspa => {}
    }
    Vec::new()
}

fn parse_lv2_id(id: &str) -> (String, Option<String>) {
    if let Some(path) = id.strip_prefix("bundle:") {
        return (String::new(), Some(path.to_string()));
    }
    (id.to_string(), None)
}

/// Build a dynamic egui param map when the static LADSPA catalog has no entry.
pub fn dynamic_ui_for_ref(plug: &PluginRef) -> Option<DynamicPluginUiSpec> {
    if ui_spec_for_ref(plug).is_some() {
        return None;
    }
    let defs = if plug.params.is_empty() {
        probe_owned_defs(plug)
    } else {
        let probed = probe_owned_defs(plug);
        if probed.is_empty() {
            owned_defs_from_param_pairs(&plug.params)
        } else {
            probed
        }
    };
    if defs.is_empty() {
        return None;
    }
    let title = plug
        .id
        .id
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&plug.id.id)
        .trim_end_matches(".clap")
        .to_string();
    Some(DynamicPluginUiSpec { title, params: defs })
}

fn populate_probed_params(plug: &mut PluginRef) {
    if !plug.params.is_empty() {
        return;
    }
    let defs = probe_owned_defs(plug);
    if !defs.is_empty() {
        plug.params = defs
            .iter()
            .map(|d| (d.key.clone(), d.default))
            .collect();
        return;
    }
    match plug.id.format {
        PluginFormat::Vst3 => {
            let path = plug
                .id
                .id
                .clone();
            let probed = buschain_engine::probe_vst3_params(&path, &plug.id.id);
            if probed.is_empty() {
                plug.params.push(("Gain".into(), 1.0));
            } else {
                plug.params = probed
                    .into_iter()
                    .map(|(name, _min, _max, def)| (name, def))
                    .collect();
            }
        }
        _ => {}
    }
}

pub fn plugin_title_for_ref(plug: &PluginRef) -> String {
    ui_spec_for_ref(plug)
        .map(|s| s.title.to_string())
        .or_else(|| dynamic_ui_for_ref(plug).map(|d| d.title))
        .unwrap_or_else(|| plug.id.id.clone())
}

/// Pulse module-ladspa-sink plugin= file stem for a label.
pub fn plugin_file_for(label: &str) -> &str {
    ui_spec_for(label)
        .map(|s| s.plugin_file)
        .unwrap_or_else(|| normalize_label(label))
}

pub fn default_params(label: &str) -> Vec<(String, f32)> {
    match ui_spec_for(label) {
        Some(spec) => spec
            .params
            .iter()
            .map(|p| (p.key.to_string(), p.default))
            .collect(),
        None => Vec::new(),
    }
}

pub fn plugin_ref_with_defaults(id: PluginId) -> PluginRef {
    let (resolved, params, mix) = match id.format {
        PluginFormat::Ladspa => {
            let label = normalize_label(&id.id);
            let params = default_params(label);
            let mix = params
                .iter()
                .find(|(k, _)| k == "Mix")
                .map(|(_, v)| *v)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            (
                PluginId {
                    format: id.format,
                    id: label.to_string(),
                },
                params,
                mix,
            )
        }
        _ => {
            let mix = 1.0_f32;
            (id, Vec::new(), mix)
        }
    };
    let mut plug = PluginRef {
        id: resolved,
        bypass: false,
        mix,
        slot_id: uuid::Uuid::new_v4(),
        params,
        state_blob: None,
        sidechain_from: None,
    };
    populate_probed_params(&mut plug);
    plug
}

impl PluginRef {
    /// Ensure every catalog param exists (keeps user values); drop obsolete keys.
    pub fn ensure_params(&mut self) {
        if let Some(spec) = ui_spec_for_ref(self) {
            self.ensure_static_params(spec);
            return;
        }
        if let Some(dyn_spec) = dynamic_ui_for_ref(self) {
            for def in &dyn_spec.params {
                if !self.params.iter().any(|(k, _)| k == &def.key) {
                    self.params.push((def.key.clone(), def.default));
                }
            }
            self.params
                .retain(|(k, _)| dyn_spec.params.iter().any(|d| &d.key == k));
            return;
        }
        populate_probed_params(self);
    }

    fn ensure_static_params(&mut self, spec: &'static PluginUiSpec) {
        // Migrate legacy softclip Drive → Threshold/Post once.
        // Do NOT treat Mix as legacy — Mix is still a live wet/dry port.
        if spec.label == "buschain_softclip" {
            let had_drive = self.params.iter().any(|(k, _)| k == "Drive");
            let has_thres = self.params.iter().any(|(k, _)| k == "Threshold");
            if had_drive && !has_thres {
                let drive = self.param("Drive").unwrap_or(2.0);
                // Higher drive ≈ lower threshold (more clipping)
                let thres = (1.0 / drive.max(0.1)).clamp(0.05, 1.0);
                self.params.retain(|(k, _)| k != "Drive");
                self.params.push(("Threshold".into(), thres));
                if !self.params.iter().any(|(k, _)| k == "Post") {
                    self.params.push(("Post".into(), 1.0));
                }
            }
            // Never leave Post stuck at 0 from a bad migrate/session — that muted the track.
            if let Some((_, post)) = self.params.iter_mut().find(|(k, _)| k == "Post") {
                if *post < 1.0e-5 {
                    *post = 1.0;
                }
            }
        }
        for def in spec.params {
            if !self.params.iter().any(|(k, _)| k == def.key) {
                self.params.push((def.key.to_string(), def.default));
            }
        }
        self.params
            .retain(|(k, _)| spec.params.iter().any(|d| d.key == k));
    }

    pub fn param(&self, key: &str) -> Option<f32> {
        self.params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| *v)
    }

    pub fn set_param(&mut self, key: &str, value: f32) {
        if let Some((_, v)) = self.params.iter_mut().find(|(k, _)| k == key) {
            *v = value;
        } else {
            self.params.push((key.to_string(), value));
        }
    }

    /// Comma-separated control= values in LADSPA port order.
    pub fn control_csv(&self) -> Option<String> {
        let spec = ui_spec_for_ref(self)?;
        let mut parts = Vec::with_capacity(spec.params.len());
        for def in spec.params {
            let v = self.param(def.key).unwrap_or(def.default);
            let v = v.clamp(def.min, def.max);
            // Compact formatting — avoid scientific notation surprises
            if (v.fract()).abs() < 1e-6 {
                parts.push(format!("{}", v as i32));
            } else {
                parts.push(format!("{v:.4}"));
            }
        }
        Some(parts.join(","))
    }
}

/* ---- catalogs (port order = C enum after audio I/O) ---- */

const fn p_slider(key: &'static str, label: &'static str, min: f32, max: f32, default: f32) -> ParamDef {
    ParamDef {
        key,
        label,
        kind: ParamKind::Slider,
        min,
        max,
        default,
        modes: None,
        logarithmic: false,
    }
}
const fn p_toggle(key: &'static str, label: &'static str, default: f32) -> ParamDef {
    ParamDef {
        key,
        label,
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default,
        modes: None,
        logarithmic: false,
    }
}

static ROOM_TYPES: &[&str] = &[
    "Shoebox",
    "Cylinder",
    "Barrel vault",
    "Cone roof",
    "Pyramid",
    "Dome",
    "Tunnel",
];

static EAR_PRESETS: &[&str] = &["Flat 180", "Human"];

/// BusChain Room — input controls only (meters are LADSPA output ports).
static REVERB: &[ParamDef] = &[
    p_toggle("Bypass", "Bypass", 0.0),
    p_slider("Mix", "Mix", 0.0, 1.0, 0.25),
    p_slider("Predelay (ms)", "Predelay", 0.0, 200.0, 20.0),
    p_slider("Size", "Size", 0.1, 4.0, 1.0),
    p_slider("Shape", "Shape", 0.5, 2.0, 1.0),
    p_slider("RT60 (s)", "RT60", 0.1, 12.0, 1.8),
    p_slider("Character", "Character", 0.0, 1.0, 0.55),
    p_slider("ER Level", "ER", 0.0, 1.0, 0.55),
    p_slider("ER Spread", "ER Spread", 0.0, 1.0, 0.5),
    p_slider("Diffusion", "Diffusion", 0.0, 1.0, 0.65),
    p_slider("Density", "Density", 0.0, 1.0, 0.7),
    p_slider("Modulation", "Mod", 0.0, 1.0, 0.15),
    p_slider("Decay Lo", "Decay Lo", 0.25, 2.0, 1.0),
    p_slider("Decay Hi", "Decay Hi", 0.25, 2.0, 0.7),
    p_slider("Wet HP (Hz)", "Wet HP", 20.0, 500.0, 80.0),
    p_slider("Wet LP (Hz)", "Wet LP", 2000.0, 20000.0, 12000.0),
    p_slider("Width", "Width", 0.0, 1.0, 0.85),
    p_slider("Duck Amount", "Duck", 0.0, 1.0, 0.0),
    p_slider("Duck Release (ms)", "Duck Rel", 10.0, 1000.0, 200.0),
    p_toggle("Freeze", "Freeze", 0.0),
    p_slider("Gate Time (ms)", "Gate", 0.0, 500.0, 0.0),
    ParamDef {
        key: "Room Type",
        label: "Room",
        kind: ParamKind::Mode,
        min: 0.0,
        max: 6.0,
        default: 0.0,
        modes: Some(ROOM_TYPES),
        logarithmic: false,
    },
    p_slider("Source X", "Src X", 0.0, 1.0, 0.28),
    p_slider("Source Y", "Src Y", 0.0, 1.0, 0.55),
    p_slider("Source Z", "Src Z", 0.0, 1.0, 0.30),
    p_slider("Listener X", "Ear X", 0.0, 1.0, 0.72),
    p_slider("Listener Y", "Ear Y", 0.0, 1.0, 0.50),
    p_slider("Listener Z", "Ear Z", 0.0, 1.0, 0.70),
    p_slider("Source Spacing", "Space", 0.0, 1.0, 0.35),
    p_slider("Source Yaw", "Yaw", -180.0, 180.0, 0.0),
    p_toggle("Face Lock", "Face", 1.0),
    p_slider("Listener Spacing", "Ears", 0.0, 1.0, 0.35),
    p_slider("Ear Angle", "Ear∠", 90.0, 180.0, 180.0),
    ParamDef {
        key: "Ear Preset",
        label: "Ears",
        kind: ParamKind::Mode,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        modes: Some(EAR_PRESETS),
        logarithmic: false,
    },
];

pub fn reverb_preset_names() -> &'static [&'static str] {
    &[
        "Booth",
        "Room",
        "Chamber",
        "Hall",
        "Plate",
        "Cathedral",
        "Tunnel",
        "Infinite",
    ]
}

pub fn apply_reverb_preset(plug: &mut PluginRef, name: &str) -> bool {
    let Some(values) = reverb_preset_values(name) else {
        return false;
    };
    plug.ensure_params();
    for (k, v) in values {
        plug.set_param(k, *v);
    }
    true
}

fn reverb_preset_values(name: &str) -> Option<&'static [(&'static str, f32)]> {
    match name {
        "Booth" => Some(&[
            ("Room Type", 0.0),
            ("Size", 0.35),
            ("Shape", 1.1),
            ("RT60 (s)", 0.35),
            ("Predelay (ms)", 5.0),
            ("Character", 0.4),
            ("ER Level", 0.7),
            ("Diffusion", 0.45),
            ("Density", 0.55),
            ("Decay Hi", 0.55),
            ("Mix", 0.2),
            ("Source X", 0.32),
            ("Source Z", 0.35),
            ("Listener X", 0.68),
            ("Listener Z", 0.65),
            ("Source Spacing", 0.22),
            ("Face Lock", 1.0),
        ]),
        "Room" => Some(&[
            ("Room Type", 0.0),
            ("Size", 1.0),
            ("Shape", 1.0),
            ("RT60 (s)", 1.2),
            ("Predelay (ms)", 18.0),
            ("Character", 0.55),
            ("ER Level", 0.55),
            ("Diffusion", 0.65),
            ("Density", 0.7),
            ("Mix", 0.25),
            ("Source X", 0.28),
            ("Source Z", 0.30),
            ("Listener X", 0.72),
            ("Listener Z", 0.70),
            ("Source Spacing", 0.35),
            ("Face Lock", 1.0),
        ]),
        "Chamber" => Some(&[
            ("Room Type", 2.0),
            ("Size", 1.4),
            ("Shape", 0.85),
            ("RT60 (s)", 2.2),
            ("Predelay (ms)", 28.0),
            ("Character", 0.6),
            ("ER Level", 0.45),
            ("Diffusion", 0.75),
            ("Density", 0.8),
            ("Mix", 0.3),
            ("Source X", 0.30),
            ("Source Z", 0.28),
            ("Listener X", 0.70),
            ("Listener Z", 0.72),
            ("Source Spacing", 0.40),
            ("Face Lock", 1.0),
        ]),
        "Hall" => Some(&[
            ("Room Type", 5.0),
            ("Size", 2.2),
            ("Shape", 1.2),
            ("RT60 (s)", 3.5),
            ("Predelay (ms)", 35.0),
            ("Character", 0.5),
            ("ER Level", 0.4),
            ("Diffusion", 0.8),
            ("Density", 0.85),
            ("Decay Lo", 1.15),
            ("Mix", 0.32),
            ("Source X", 0.25),
            ("Source Z", 0.25),
            ("Listener X", 0.75),
            ("Listener Z", 0.75),
            ("Source Spacing", 0.55),
            ("Face Lock", 1.0),
        ]),
        "Plate" => Some(&[
            ("Room Type", 0.0),
            ("Size", 0.9),
            ("Shape", 1.6),
            ("RT60 (s)", 2.0),
            ("Predelay (ms)", 8.0),
            ("Character", 0.75),
            ("ER Level", 0.25),
            ("Diffusion", 0.9),
            ("Density", 0.95),
            ("Modulation", 0.25),
            ("Decay Hi", 0.9),
            ("Mix", 0.28),
            ("Source X", 0.35),
            ("Source Z", 0.40),
            ("Listener X", 0.65),
            ("Listener Z", 0.60),
            ("Source Spacing", 0.50),
            ("Face Lock", 1.0),
        ]),
        "Cathedral" => Some(&[
            ("Room Type", 2.0),
            ("Size", 3.2),
            ("Shape", 0.75),
            ("RT60 (s)", 6.5),
            ("Predelay (ms)", 55.0),
            ("Character", 0.45),
            ("ER Level", 0.35),
            ("Diffusion", 0.85),
            ("Density", 0.9),
            ("Decay Lo", 1.3),
            ("Decay Hi", 0.55),
            ("Mix", 0.35),
            ("Source X", 0.22),
            ("Source Z", 0.20),
            ("Listener X", 0.78),
            ("Listener Z", 0.80),
            ("Source Spacing", 0.60),
            ("Face Lock", 1.0),
        ]),
        "Tunnel" => Some(&[
            ("Room Type", 6.0),
            ("Size", 2.8),
            ("Shape", 0.55),
            ("RT60 (s)", 4.0),
            ("Predelay (ms)", 40.0),
            ("Character", 0.35),
            ("ER Level", 0.5),
            ("Diffusion", 0.55),
            ("Density", 0.6),
            ("Decay Hi", 0.45),
            ("Width", 0.95),
            ("Mix", 0.3),
            ("Source X", 0.50),
            ("Source Z", 0.18),
            ("Listener X", 0.50),
            ("Listener Z", 0.82),
            ("Source Spacing", 0.18),
            ("Face Lock", 1.0),
        ]),
        "Infinite" => Some(&[
            ("Room Type", 1.0),
            ("Size", 3.5),
            ("Shape", 1.0),
            ("RT60 (s)", 11.0),
            ("Predelay (ms)", 30.0),
            ("Character", 0.5),
            ("ER Level", 0.2),
            ("Diffusion", 0.9),
            ("Density", 0.95),
            ("Modulation", 0.2),
            ("Mix", 0.4),
            ("Source X", 0.28),
            ("Source Z", 0.30),
            ("Listener X", 0.72),
            ("Listener Z", 0.70),
            ("Source Spacing", 0.45),
            ("Face Lock", 1.0),
        ]),
        _ => None,
    }
}

/// Fruity Soft Clipper–style: Threshold + Post (makeup) + Mix (dry/wet).
static SOFTCLIP: &[ParamDef] = &[
    ParamDef {
        key: "Threshold",
        label: "THRES",
        kind: ParamKind::Slider,
        min: 0.05,
        max: 1.0,
        default: 0.5,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Post",
        label: "POST",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 4.0,
        default: 1.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Mix",
        label: "MIX",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 1.0,
        default: 1.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Bypass",
        label: "Bypass",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
];

static LIMITER: &[ParamDef] = &[
    ParamDef {
        key: "Ceiling (dB)",
        label: "Ceiling",
        kind: ParamKind::Slider,
        min: -24.0,
        max: 0.0,
        default: -0.1,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Attack (ms)",
        label: "Attack",
        kind: ParamKind::Slider,
        min: 0.01,
        max: 50.0,
        default: 0.1,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "Release (ms)",
        label: "Release",
        kind: ParamKind::Slider,
        min: 1.0,
        max: 500.0,
        default: 50.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Lookahead (ms)",
        label: "Lookahead",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 10.0,
        default: 1.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Soft Knee (dB)",
        label: "Knee",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 12.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Input (dB)",
        label: "Input",
        kind: ParamKind::Slider,
        min: -24.0,
        max: 24.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Makeup (dB)",
        label: "Makeup",
        kind: ParamKind::Slider,
        min: -24.0,
        max: 24.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Bypass",
        label: "Bypass",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
];

/// Limiter console presets.
pub fn limiter_preset_names() -> &'static [&'static str] {
    &["Transparent", "Mastering", "Aggressive"]
}

pub fn apply_limiter_preset(plug: &mut PluginRef, name: &str) -> bool {
    let Some(values) = limiter_preset_values(name) else {
        return false;
    };
    plug.ensure_params();
    for (k, v) in values {
        plug.set_param(k, *v);
    }
    true
}

fn limiter_preset_values(name: &str) -> Option<&'static [(&'static str, f32)]> {
    match name {
        "Transparent" => Some(&[
            ("Ceiling (dB)", -0.1),
            ("Attack (ms)", 0.1),
            ("Release (ms)", 80.0),
            ("Lookahead (ms)", 1.0),
            ("Soft Knee (dB)", 0.0),
            ("Input (dB)", 0.0),
            ("Makeup (dB)", 0.0),
        ]),
        "Mastering" => Some(&[
            ("Ceiling (dB)", -0.3),
            ("Attack (ms)", 0.05),
            ("Release (ms)", 120.0),
            ("Lookahead (ms)", 3.0),
            ("Soft Knee (dB)", 2.0),
            ("Input (dB)", 0.0),
            ("Makeup (dB)", 0.0),
        ]),
        "Aggressive" => Some(&[
            ("Ceiling (dB)", -1.0),
            ("Attack (ms)", 0.01),
            ("Release (ms)", 30.0),
            ("Lookahead (ms)", 5.0),
            ("Soft Knee (dB)", 0.0),
            ("Input (dB)", 3.0),
            ("Makeup (dB)", 0.0),
        ]),
        _ => None,
    }
}

static COMPRESSOR: &[ParamDef] = &[
    ParamDef {
        key: "Threshold (dB)",
        label: "Thresh",
        kind: ParamKind::Slider,
        min: -60.0,
        max: 0.0,
        default: -18.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Ratio",
        label: "Ratio",
        kind: ParamKind::Slider,
        min: 1.0,
        max: 20.0,
        default: 4.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Attack (ms)",
        label: "Attack",
        kind: ParamKind::Slider,
        min: 0.1,
        max: 100.0,
        default: 10.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Release (ms)",
        label: "Release",
        kind: ParamKind::Slider,
        min: 1.0,
        max: 1000.0,
        default: 100.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Makeup (dB)",
        label: "Makeup",
        kind: ParamKind::Slider,
        min: -24.0,
        max: 24.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Bypass",
        label: "Bypass",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
];

static EQ_MODES: &[&str] = &["Peak", "Low shelf", "High shelf", "High-pass", "Low-pass"];

static EQ: &[ParamDef] = &[
    ParamDef {
        key: "Freq (Hz)",
        label: "Freq",
        kind: ParamKind::Slider,
        min: 20.0,
        max: 20000.0,
        default: 1000.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "Gain (dB)",
        label: "Gain",
        kind: ParamKind::Slider,
        min: -24.0,
        max: 24.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Q",
        label: "Q",
        kind: ParamKind::Slider,
        min: 0.1,
        max: 10.0,
        default: 0.707,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Mode",
        label: "Mode",
        kind: ParamKind::Mode,
        min: 0.0,
        max: 4.0,
        default: 0.0,
        modes: Some(EQ_MODES),
        logarithmic: false,
    },
    ParamDef {
        key: "Bypass",
        label: "Bypass",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
];

static PITCH: &[ParamDef] = &[
    ParamDef {
        key: "Semitones",
        label: "Semi",
        kind: ParamKind::Slider,
        min: -12.0,
        max: 12.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Cents",
        label: "Cents",
        kind: ParamKind::Slider,
        min: -100.0,
        max: 100.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Smooth",
        label: "Smooth",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 1.0,
        default: 0.65,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Bypass",
        label: "Bypass",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
];

/// Theatre Drive — Blood Overdrive–class; cinema bass defaults (Drive Bass @ 100 Hz).
static OD_PRE_SHAPES: &[&str] = &["Low-pass", "Band-pass"];
static OD_CHARS: &[&str] = &["Tube", "Soft", "Hard", "Diode"];
static OD_FOCUS: &[&str] = &["Full", "Drive Bass", "Protect Bass"];

static OVERDRIVE: &[ParamDef] = &[
    ParamDef {
        key: "Pre Band",
        label: "Pre Band",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 1.0,
        default: 0.55,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Color (Hz)",
        label: "Color",
        kind: ParamKind::Slider,
        min: 40.0,
        max: 8000.0,
        default: 180.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "Pre Shape",
        label: "Pre",
        kind: ParamKind::Mode,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        modes: Some(OD_PRE_SHAPES),
        logarithmic: false,
    },
    ParamDef {
        key: "Drive",
        label: "Drive",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 1.0,
        default: 0.34,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Boost",
        label: "×10",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Character",
        label: "Character",
        kind: ParamKind::Mode,
        min: 0.0,
        max: 3.0,
        default: 0.0,
        modes: Some(OD_CHARS),
        logarithmic: false,
    },
    ParamDef {
        key: "Bias",
        label: "Bias",
        kind: ParamKind::Slider,
        min: -1.0,
        max: 1.0,
        default: 0.10,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Post Filter (Hz)",
        label: "Post Filter",
        kind: ParamKind::Slider,
        min: 200.0,
        max: 20000.0,
        default: 3200.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "Post Gain",
        label: "Post Gain",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 1.0,
        default: 0.42,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Mix",
        label: "Mix",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 1.0,
        default: 0.68,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Split (Hz)",
        label: "Split",
        kind: ParamKind::Slider,
        min: 20.0,
        max: 500.0,
        default: 100.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "Focus",
        label: "Focus",
        kind: ParamKind::Mode,
        min: 0.0,
        max: 2.0,
        default: 1.0, // Drive Bass — theatre / 808 thicken
        modes: Some(OD_FOCUS),
        logarithmic: false,
    },
    ParamDef {
        key: "Bypass",
        label: "Bypass",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
];

// Defaults = "default" preset (live PCM2902)
static DENOISER: &[ParamDef] = &[
    ParamDef {
        key: "Threshold (dB)",
        label: "Thresh",
        kind: ParamKind::Slider,
        min: -140.0,
        max: 0.0,
        default: -71.4583,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Range Band 1 (dB)",
        label: "R1",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 48.0,
        default: 12.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Range Band 2 (dB)",
        label: "R2",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 48.0,
        default: 11.5,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Range Band 3 (dB)",
        label: "R3",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 48.0,
        default: 8.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Range Band 4 (dB)",
        label: "R4",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 48.0,
        default: 14.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Range Band 5 (dB)",
        label: "R5",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 48.0,
        default: 12.5115,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Range Band 6 (dB)",
        label: "R6",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 48.0,
        default: 8.8918,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Band 1 Freq (Hz)",
        label: "F1",
        kind: ParamKind::Slider,
        min: 20.0,
        max: 20000.0,
        default: 120.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "Band 2 Freq (Hz)",
        label: "F2",
        kind: ParamKind::Slider,
        min: 20.0,
        max: 20000.0,
        default: 240.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "Band 3 Freq (Hz)",
        label: "F3",
        kind: ParamKind::Slider,
        min: 20.0,
        max: 20000.0,
        default: 600.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "Band 4 Freq (Hz)",
        label: "F4",
        kind: ParamKind::Slider,
        min: 20.0,
        max: 20000.0,
        default: 1580.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "Band 5 Freq (Hz)",
        label: "F5",
        kind: ParamKind::Slider,
        min: 20.0,
        max: 20000.0,
        default: 3000.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "Band 6 Freq (Hz)",
        label: "F6",
        kind: ParamKind::Slider,
        min: 20.0,
        max: 20000.0,
        default: 12000.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "HF Bias",
        label: "HF Bias",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 1.0,
        default: 0.40,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Stereo Link",
        label: "Link",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 1.0,
        default: 0.90,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Stability",
        label: "Stability",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 1.0,
        default: 0.45,
        modes: None,
        logarithmic: false,
    },
    // Parametric NR nodes (UI) — mapped onto the 6 filterbank bands each frame.
    ParamDef {
        key: "NR1 On",
        label: "NR1",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 1.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR1 Freq (Hz)",
        label: "NR1 Hz",
        kind: ParamKind::Slider,
        min: 20.0,
        max: 20000.0,
        default: 120.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "NR1 Depth (dB)",
        label: "NR1 dB",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 48.0,
        default: 12.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR1 Q",
        label: "NR1 Q",
        kind: ParamKind::Slider,
        min: 0.3,
        max: 12.0,
        default: 1.4,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR2 On",
        label: "NR2",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 1.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR2 Freq (Hz)",
        label: "NR2 Hz",
        kind: ParamKind::Slider,
        min: 20.0,
        max: 20000.0,
        default: 240.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "NR2 Depth (dB)",
        label: "NR2 dB",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 48.0,
        default: 11.5,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR2 Q",
        label: "NR2 Q",
        kind: ParamKind::Slider,
        min: 0.3,
        max: 12.0,
        default: 1.4,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR3 On",
        label: "NR3",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 1.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR3 Freq (Hz)",
        label: "NR3 Hz",
        kind: ParamKind::Slider,
        min: 20.0,
        max: 20000.0,
        default: 600.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "NR3 Depth (dB)",
        label: "NR3 dB",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 48.0,
        default: 8.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR3 Q",
        label: "NR3 Q",
        kind: ParamKind::Slider,
        min: 0.3,
        max: 12.0,
        default: 1.6,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR4 On",
        label: "NR4",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 1.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR4 Freq (Hz)",
        label: "NR4 Hz",
        kind: ParamKind::Slider,
        min: 20.0,
        max: 20000.0,
        default: 1580.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "NR4 Depth (dB)",
        label: "NR4 dB",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 48.0,
        default: 14.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR4 Q",
        label: "NR4 Q",
        kind: ParamKind::Slider,
        min: 0.3,
        max: 12.0,
        default: 1.5,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR5 On",
        label: "NR5",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 1.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR5 Freq (Hz)",
        label: "NR5 Hz",
        kind: ParamKind::Slider,
        min: 20.0,
        max: 20000.0,
        default: 3000.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "NR5 Depth (dB)",
        label: "NR5 dB",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 48.0,
        default: 12.5115,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR5 Q",
        label: "NR5 Q",
        kind: ParamKind::Slider,
        min: 0.3,
        max: 12.0,
        default: 1.8,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR6 On",
        label: "NR6",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 1.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR6 Freq (Hz)",
        label: "NR6 Hz",
        kind: ParamKind::Slider,
        min: 20.0,
        max: 20000.0,
        default: 12000.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "NR6 Depth (dB)",
        label: "NR6 dB",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 48.0,
        default: 8.8918,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR6 Q",
        label: "NR6 Q",
        kind: ParamKind::Slider,
        min: 0.3,
        max: 12.0,
        default: 1.6,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR7 On",
        label: "NR7",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR7 Freq (Hz)",
        label: "NR7 Hz",
        kind: ParamKind::Slider,
        min: 20.0,
        max: 20000.0,
        default: 6000.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "NR7 Depth (dB)",
        label: "NR7 dB",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 48.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR7 Q",
        label: "NR7 Q",
        kind: ParamKind::Slider,
        min: 0.3,
        max: 12.0,
        default: 2.5,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR8 On",
        label: "NR8",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR8 Freq (Hz)",
        label: "NR8 Hz",
        kind: ParamKind::Slider,
        min: 20.0,
        max: 20000.0,
        default: 8000.0,
        modes: None,
        logarithmic: true,
    },
    ParamDef {
        key: "NR8 Depth (dB)",
        label: "NR8 dB",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 48.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "NR8 Q",
        label: "NR8 Q",
        kind: ParamKind::Slider,
        min: 0.3,
        max: 12.0,
        default: 3.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Gate Enable",
        label: "Gate",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Gate Knee (dB)",
        label: "Knee",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 24.0,
        default: 4.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Gate Ratio",
        label: "Ratio",
        kind: ParamKind::Slider,
        min: 1.0,
        max: 100.0,
        default: 12.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Gate Attack (ms)",
        label: "Attack",
        kind: ParamKind::Slider,
        min: 0.1,
        max: 50.0,
        default: 1.2,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Gate Release (ms)",
        label: "Release",
        kind: ParamKind::Slider,
        min: 5.0,
        max: 1000.0,
        default: 90.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Gate Mix",
        label: "Gate Mix",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 1.0,
        default: 1.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Bypass",
        label: "DSP Bypass",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
];

/* 8-band parametric EQ — in-house (not LSP). Port names match C. */
macro_rules! eq8_bands {
    ($(($n:tt, $freq:expr)),* $(,)?) => {
        &[
            $(
                ParamDef {
                    key: concat!("B", stringify!($n), " On"),
                    label: concat!("B", stringify!($n)),
                    kind: ParamKind::Toggle,
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                    modes: None,
                    logarithmic: false,
                },
                ParamDef {
                    key: concat!("B", stringify!($n), " Freq"),
                    label: "Hz",
                    kind: ParamKind::Slider,
                    min: 20.0,
                    max: 20000.0,
                    default: $freq,
                    modes: None,
                    logarithmic: true,
                },
                ParamDef {
                    key: concat!("B", stringify!($n), " Gain"),
                    label: "dB",
                    kind: ParamKind::Slider,
                    min: -24.0,
                    max: 24.0,
                    default: 0.0,
                    modes: None,
                    logarithmic: false,
                },
                ParamDef {
                    key: concat!("B", stringify!($n), " Q"),
                    label: "Q",
                    kind: ParamKind::Slider,
                    min: 0.1,
                    max: 10.0,
                    default: 0.707,
                    modes: None,
                    logarithmic: false,
                },
                ParamDef {
                    key: concat!("B", stringify!($n), " Type"),
                    label: "Type",
                    kind: ParamKind::Mode,
                    min: 0.0,
                    max: 4.0,
                    default: 0.0,
                    modes: Some(EQ_MODES),
                    logarithmic: false,
                },
            )*
            ParamDef {
                key: "Output (dB)",
                label: "Trim",
                kind: ParamKind::Slider,
                min: -24.0,
                max: 24.0,
                default: 0.0,
                modes: None,
                logarithmic: false,
            },
            ParamDef {
                key: "Mix",
                label: "MIX",
                kind: ParamKind::Slider,
                min: 0.0,
                max: 1.0,
                default: 1.0,
                modes: None,
                logarithmic: false,
            },
            ParamDef {
                key: "Bypass",
                label: "Bypass",
                kind: ParamKind::Toggle,
                min: 0.0,
                max: 1.0,
                default: 0.0,
                modes: None,
                logarithmic: false,
            },
        ]
    };
}

static EQ8: &[ParamDef] = eq8_bands!(
    (1, 60.0),
    (2, 150.0),
    (3, 400.0),
    (4, 1000.0),
    (5, 2500.0),
    (6, 5000.0),
    (7, 8000.0),
    (8, 12000.0),
);

pub fn equalizer_preset_names() -> &'static [&'static str] {
    &["Flat", "Voice Presence", "Mud Cut", "Air", "Bass Shelf"]
}

pub fn apply_equalizer_preset(plug: &mut PluginRef, name: &str) -> bool {
    let Some(values) = equalizer_preset_values(name) else {
        return false;
    };
    plug.ensure_params();
    for (k, v) in values {
        plug.set_param(k, *v);
    }
    true
}

fn equalizer_preset_values(name: &str) -> Option<&'static [(&'static str, f32)]> {
    match name {
        "Flat" => Some(EQ_PRESET_FLAT),
        "Voice Presence" => Some(EQ_PRESET_VOICE),
        "Mud Cut" => Some(EQ_PRESET_MUD),
        "Air" => Some(EQ_PRESET_AIR),
        "Bass Shelf" => Some(EQ_PRESET_BASS),
        _ => None,
    }
}

/// All bands on, flat gains, default freqs.
static EQ_PRESET_FLAT: &[(&str, f32)] = &[
    ("B1 On", 1.0), ("B1 Freq", 60.0), ("B1 Gain", 0.0), ("B1 Q", 0.707), ("B1 Type", 0.0),
    ("B2 On", 1.0), ("B2 Freq", 150.0), ("B2 Gain", 0.0), ("B2 Q", 0.707), ("B2 Type", 0.0),
    ("B3 On", 1.0), ("B3 Freq", 400.0), ("B3 Gain", 0.0), ("B3 Q", 0.707), ("B3 Type", 0.0),
    ("B4 On", 1.0), ("B4 Freq", 1000.0), ("B4 Gain", 0.0), ("B4 Q", 0.707), ("B4 Type", 0.0),
    ("B5 On", 1.0), ("B5 Freq", 2500.0), ("B5 Gain", 0.0), ("B5 Q", 0.707), ("B5 Type", 0.0),
    ("B6 On", 1.0), ("B6 Freq", 5000.0), ("B6 Gain", 0.0), ("B6 Q", 0.707), ("B6 Type", 0.0),
    ("B7 On", 1.0), ("B7 Freq", 8000.0), ("B7 Gain", 0.0), ("B7 Q", 0.707), ("B7 Type", 0.0),
    ("B8 On", 1.0), ("B8 Freq", 12000.0), ("B8 Gain", 0.0), ("B8 Q", 0.707), ("B8 Type", 0.0),
    ("Output (dB)", 0.0),
];

static EQ_PRESET_VOICE: &[(&str, f32)] = &[
    ("B1 On", 1.0), ("B1 Freq", 80.0), ("B1 Gain", -2.0), ("B1 Q", 0.7), ("B1 Type", 1.0),
    ("B2 On", 1.0), ("B2 Freq", 220.0), ("B2 Gain", -1.5), ("B2 Q", 1.0), ("B2 Type", 0.0),
    ("B3 On", 0.0), ("B3 Freq", 400.0), ("B3 Gain", 0.0), ("B3 Q", 0.707), ("B3 Type", 0.0),
    ("B4 On", 1.0), ("B4 Freq", 1200.0), ("B4 Gain", 1.5), ("B4 Q", 1.2), ("B4 Type", 0.0),
    ("B5 On", 1.0), ("B5 Freq", 3200.0), ("B5 Gain", 2.5), ("B5 Q", 1.0), ("B5 Type", 0.0),
    ("B6 On", 1.0), ("B6 Freq", 5500.0), ("B6 Gain", 1.0), ("B6 Q", 0.9), ("B6 Type", 0.0),
    ("B7 On", 0.0), ("B7 Freq", 8000.0), ("B7 Gain", 0.0), ("B7 Q", 0.707), ("B7 Type", 0.0),
    ("B8 On", 1.0), ("B8 Freq", 12000.0), ("B8 Gain", 1.5), ("B8 Q", 0.7), ("B8 Type", 2.0),
    ("Output (dB)", 0.0),
];

static EQ_PRESET_MUD: &[(&str, f32)] = &[
    ("B1 On", 1.0), ("B1 Freq", 40.0), ("B1 Gain", 0.0), ("B1 Q", 0.707), ("B1 Type", 3.0),
    ("B2 On", 1.0), ("B2 Freq", 180.0), ("B2 Gain", -3.5), ("B2 Q", 1.4), ("B2 Type", 0.0),
    ("B3 On", 1.0), ("B3 Freq", 320.0), ("B3 Gain", -2.0), ("B3 Q", 1.1), ("B3 Type", 0.0),
    ("B4 On", 0.0), ("B4 Freq", 1000.0), ("B4 Gain", 0.0), ("B4 Q", 0.707), ("B4 Type", 0.0),
    ("B5 On", 0.0), ("B5 Freq", 2500.0), ("B5 Gain", 0.0), ("B5 Q", 0.707), ("B5 Type", 0.0),
    ("B6 On", 0.0), ("B6 Freq", 5000.0), ("B6 Gain", 0.0), ("B6 Q", 0.707), ("B6 Type", 0.0),
    ("B7 On", 0.0), ("B7 Freq", 8000.0), ("B7 Gain", 0.0), ("B7 Q", 0.707), ("B7 Type", 0.0),
    ("B8 On", 0.0), ("B8 Freq", 12000.0), ("B8 Gain", 0.0), ("B8 Q", 0.707), ("B8 Type", 0.0),
    ("Output (dB)", 0.5),
];

static EQ_PRESET_AIR: &[(&str, f32)] = &[
    ("B1 On", 0.0), ("B1 Freq", 60.0), ("B1 Gain", 0.0), ("B1 Q", 0.707), ("B1 Type", 0.0),
    ("B2 On", 0.0), ("B2 Freq", 150.0), ("B2 Gain", 0.0), ("B2 Q", 0.707), ("B2 Type", 0.0),
    ("B3 On", 0.0), ("B3 Freq", 400.0), ("B3 Gain", 0.0), ("B3 Q", 0.707), ("B3 Type", 0.0),
    ("B4 On", 0.0), ("B4 Freq", 1000.0), ("B4 Gain", 0.0), ("B4 Q", 0.707), ("B4 Type", 0.0),
    ("B5 On", 1.0), ("B5 Freq", 4000.0), ("B5 Gain", 1.0), ("B5 Q", 0.8), ("B5 Type", 0.0),
    ("B6 On", 1.0), ("B6 Freq", 8000.0), ("B6 Gain", 2.0), ("B6 Q", 0.7), ("B6 Type", 0.0),
    ("B7 On", 1.0), ("B7 Freq", 12000.0), ("B7 Gain", 2.5), ("B7 Q", 0.7), ("B7 Type", 2.0),
    ("B8 On", 1.0), ("B8 Freq", 16000.0), ("B8 Gain", 1.5), ("B8 Q", 0.7), ("B8 Type", 2.0),
    ("Output (dB)", 0.0),
];

static EQ_PRESET_BASS: &[(&str, f32)] = &[
    ("B1 On", 1.0), ("B1 Freq", 80.0), ("B1 Gain", 3.5), ("B1 Q", 0.7), ("B1 Type", 1.0),
    ("B2 On", 1.0), ("B2 Freq", 200.0), ("B2 Gain", 1.0), ("B2 Q", 0.9), ("B2 Type", 0.0),
    ("B3 On", 1.0), ("B3 Freq", 450.0), ("B3 Gain", -1.0), ("B3 Q", 1.0), ("B3 Type", 0.0),
    ("B4 On", 0.0), ("B4 Freq", 1000.0), ("B4 Gain", 0.0), ("B4 Q", 0.707), ("B4 Type", 0.0),
    ("B5 On", 0.0), ("B5 Freq", 2500.0), ("B5 Gain", 0.0), ("B5 Q", 0.707), ("B5 Type", 0.0),
    ("B6 On", 0.0), ("B6 Freq", 5000.0), ("B6 Gain", 0.0), ("B6 Q", 0.707), ("B6 Type", 0.0),
    ("B7 On", 0.0), ("B7 Freq", 8000.0), ("B7 Gain", 0.0), ("B7 Q", 0.707), ("B7 Type", 0.0),
    ("B8 On", 0.0), ("B8 Freq", 12000.0), ("B8 Gain", 0.0), ("B8 Q", 0.707), ("B8 Type", 0.0),
    ("Output (dB)", -1.0),
];

static GATE: &[ParamDef] = &[
    ParamDef {
        key: "Enable",
        label: "Enable",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 1.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Threshold (dB)",
        label: "Thresh",
        kind: ParamKind::Slider,
        min: -140.0,
        max: 0.0,
        default: -78.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Hysteresis (dB)",
        label: "Hyst",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 24.0,
        default: 3.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Attack (ms)",
        label: "Attack",
        kind: ParamKind::Slider,
        min: 0.1,
        max: 50.0,
        default: 2.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Hold (ms)",
        label: "Hold",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 500.0,
        default: 80.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Release (ms)",
        label: "Release",
        kind: ParamKind::Slider,
        min: 1.0,
        max: 1000.0,
        default: 120.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Range (dB)",
        label: "Range",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 140.0,
        default: 100.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Mix",
        label: "Mix",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 1.0,
        default: 1.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Bypass",
        label: "DSP Bypass",
        kind: ParamKind::Toggle,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        modes: None,
        logarithmic: false,
    },
];

static SPECS: &[PluginUiSpec] = &[
    PluginUiSpec {
        label: "buschain_reverb",
        title: "Room",
        plugin_file: "buschain_reverb",
        params: REVERB,
    },
    PluginUiSpec {
        label: "buschain_softclip",
        title: "Soft Clipper",
        plugin_file: "buschain_builtins",
        params: SOFTCLIP,
    }, // Fruity Soft Clipper–style (Threshold + Post + curve UI)
    PluginUiSpec {
        label: "buschain_overdrive",
        title: "Theatre Drive",
        plugin_file: "buschain_builtins",
        params: OVERDRIVE,
    },
    PluginUiSpec {
        label: "buschain_limiter",
        title: "Limiter",
        plugin_file: "buschain_builtins",
        params: LIMITER,
    },
    PluginUiSpec {
        label: "buschain_compressor",
        title: "Compressor",
        plugin_file: "buschain_builtins",
        params: COMPRESSOR,
    },
    PluginUiSpec {
        label: "buschain_equalizer",
        title: "Equalizer",
        plugin_file: "buschain_builtins",
        params: EQ8,
    },
    PluginUiSpec {
        label: "buschain_eq",
        title: "EQ 1-Band",
        plugin_file: "buschain_builtins",
        params: EQ,
    },
    PluginUiSpec {
        label: "buschain_pitch",
        title: "Pitch",
        plugin_file: "buschain_builtins",
        params: PITCH,
    },
    PluginUiSpec {
        label: "buschain_denoiser",
        title: "Denoiser",
        plugin_file: "buschain_denoiser",
        params: DENOISER,
    },
    PluginUiSpec {
        label: "buschain_gate",
        title: "Gate",
        plugin_file: "buschain_gate",
        params: GATE,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_plugin_has_power_port() {
        // Insert power only writes Bypass/Enable — missing ports = silent no-op
        // (the Equalizer On/Enable regression). Keep both accepted (gate uses Enable).
        for spec in SPECS {
            let has_power = spec
                .params
                .iter()
                .any(|p| p.key == "Bypass" || p.key == "Enable");
            assert!(
                has_power,
                "{} ({}) has no Bypass/Enable port — power toggle will no-op",
                spec.title, spec.label
            );
        }
    }

    #[test]
    fn known_labels_match_specs() {
        for label in KNOWN_LABELS {
            assert!(
                ui_spec_for(label).is_some(),
                "KNOWN_LABELS entry `{label}` missing from SPECS"
            );
        }
    }
}
