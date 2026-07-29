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
    "buschain_softclip",
    "buschain_overdrive",
    "buschain_limiter",
    "buschain_compressor",
    "buschain_eq8",
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
    let label = normalize_label(&id.id);
    let id = PluginId {
        format: id.format,
        id: label.to_string(),
    };
    let params = default_params(label);
    let mix = params
        .iter()
        .find(|(k, _)| k == "Mix")
        .map(|(_, v)| *v)
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    PluginRef {
        id,
        bypass: false,
        mix,
        slot_id: uuid::Uuid::new_v4(),
        params,
    }
}

impl PluginRef {
    /// Ensure every catalog param exists (keeps user values); drop obsolete keys.
    pub fn ensure_params(&mut self) {
        let Some(spec) = ui_spec_for_ref(self) else {
            return;
        };
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
        max: 24.0,
        default: 12.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Range Band 2 (dB)",
        label: "R2",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 24.0,
        default: 11.5,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Range Band 3 (dB)",
        label: "R3",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 24.0,
        default: 8.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Range Band 4 (dB)",
        label: "R4",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 24.0,
        default: 14.0,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Range Band 5 (dB)",
        label: "R5",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 24.0,
        default: 12.5115,
        modes: None,
        logarithmic: false,
    },
    ParamDef {
        key: "Range Band 6 (dB)",
        label: "R6",
        kind: ParamKind::Slider,
        min: 0.0,
        max: 24.0,
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
        label: "buschain_eq8",
        title: "Parametric EQ",
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
