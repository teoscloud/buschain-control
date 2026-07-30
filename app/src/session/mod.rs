use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audio::plugin::PluginRef;
use buschain_engine::PerformanceProfile;

pub mod store;
pub use store::{list_sessions, load_active, resolve_devices, ResolveReport, SessionMeta};

/// Per-hardware-device clock preference (Output / Input device panels).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceClockConfig {
    pub sample_rate: u32,
    pub quantum: u32,
    #[serde(default = "default_true")]
    pub soft_quantum: bool,
}

impl Default for DeviceClockConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            quantum: 256,
            soft_quantum: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TrackKind {
    Bus,
    Master,
    /// Legacy sessions may still carry this; `normalize()` coerces to Bus.
    Input,
}

impl TrackKind {
    pub fn is_master(self) -> bool {
        matches!(self, TrackKind::Master)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: Uuid,
    pub name: String,
    pub kind: TrackKind,
    pub gain_db: f32,
    pub mute: bool,
    pub solo: bool,
    /// Monitor this track's input source into the master bus (when an input is set).
    #[serde(default)]
    pub listen: bool,
    pub inserts: Vec<PluginRef>,
    /// Apps pinned to this track. Prefer stable keys: `bin:firefox`, `id:…`, `name:Firefox`.
    /// Legacy bare names / stream indices still match at apply time.
    #[serde(default)]
    pub assigned_playback: Vec<String>,
    /// PipeWire source name feeding this track (dropdown).
    #[serde(default)]
    pub input_source: Option<String>,
    /// Human description of input (survives PipeWire renaming the node).
    #[serde(default)]
    pub input_source_desc: Option<String>,
    /// Destinations: other track IDs and/or master. Empty ⇒ master only.
    #[serde(default)]
    pub output_targets: Vec<Uuid>,
    /// Expose this track as a system virtual output (apps / default sink / Move to).
    /// Master is always exposed. When off, the bus still exists for internal routing.
    /// Missing field in old JSON ⇒ false (opt-in), matching new-track defaults.
    #[serde(default)]
    pub virtual_output: bool,
    /// Runtime: null-sink / filter-chain sink name for this track
    #[serde(skip)]
    pub sink_name: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Track {
    /// Deterministic PipeWire sink name (stable across restarts).
    pub fn expected_sink_name(&self) -> String {
        if self.kind.is_master() {
            "buschain_master".into()
        } else {
            format!("buschain_track_{}", self.id.simple())
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Display name in the session library.
    #[serde(default)]
    pub name: String,
    /// Filesystem slug (`sessions/<slug>.json`).
    #[serde(default)]
    pub slug: String,
    /// ISO-ish timestamp of last save (informational).
    #[serde(default)]
    pub saved_at: String,
    pub tracks: Vec<Track>,
    pub master_output: Option<String>,
    /// Description of Master HW out — used when the sink name changes after reboot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_output_desc: Option<String>,
    /// Preferred system default sink (usually a BusChain track bus). Re-applied on graph apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_default_sink: Option<String>,
    /// Legacy: forced false on launch; kept for Settings checkbox / older tools.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub autostart_graph: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ladspa_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lv2_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clap_paths: Vec<String>,
    /// VST3 scan on by default (Carla discovery). Toggle in Config if unwanted.
    #[serde(default = "default_true")]
    pub vst3_enabled: bool,
    /// When true, non-trusted plugins run in a per-track SHM sandbox (P6).
    /// Default false = trusted in-process hosting.
    #[serde(default)]
    pub sandbox_untrusted: bool,
    /// Mixer selection / accent highlight RGB (Settings → Appearance).
    #[serde(default = "default_accent_rgb")]
    pub accent_rgb: [u8; 3],
    /// Device-aware audio performance profile (rate / quantum / preset).
    #[serde(default)]
    pub performance: PerformanceProfile,
    /// Per HW sink/source clock prefs (keyed by PipeWire node name).
    /// Only written when the user edits/Applies a device clock — not on mere view.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub device_clocks: HashMap<String, DeviceClockConfig>,
    /// MIDI hardware / PipeWire nodes (enable flags + labels).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub midi_devices: Vec<buschain_engine::MidiDeviceInfo>,
    /// Logical routes: MIDI source → track / insert / master.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub midi_routes: Vec<buschain_engine::MidiRoute>,
    /// CC → parameter maps (learn mode writes here).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub midi_maps: Vec<buschain_engine::MidiCcMap>,
}

fn default_accent_rgb() -> [u8; 3] {
    [0xc9, 0xa2, 0x6b]
}

impl Default for Session {
    fn default() -> Self {
        let master_id = Uuid::new_v4();
        let track1_id = Uuid::new_v4();
        Self {
            name: "Default".into(),
            slug: "default".into(),
            saved_at: String::new(),
            tracks: vec![
                Track {
                    id: master_id,
                    name: "Master".into(),
                    kind: TrackKind::Master,
                    gain_db: 0.0,
                    mute: false,
                    solo: false,
                    listen: false,
                    inserts: vec![],
                    assigned_playback: vec![],
                    input_source: None,
                    input_source_desc: None,
                    output_targets: vec![],
                    virtual_output: true,
                    sink_name: None,
                },
                Track {
                    id: track1_id,
                    name: "Track 1".into(),
                    kind: TrackKind::Bus,
                    gain_db: 0.0,
                    mute: false,
                    solo: false,
                    listen: false,
                    inserts: vec![],
                    assigned_playback: vec![],
                    input_source: None,
                    input_source_desc: None,
                    output_targets: vec![master_id],
                    virtual_output: false,
                    sink_name: None,
                },
            ],
            master_output: None,
            master_output_desc: None,
            preferred_default_sink: None,
            autostart_graph: false,
            ladspa_paths: vec![],
            lv2_paths: vec![],
            clap_paths: vec![],
            vst3_enabled: true,
            sandbox_untrusted: false,
            accent_rgb: default_accent_rgb(),
            performance: PerformanceProfile::default(),
            device_clocks: HashMap::new(),
            midi_devices: vec![],
            midi_routes: vec![],
            midi_maps: vec![],
        }
    }
}

impl Session {
    pub fn path() -> std::path::PathBuf {
        store::session_path(&store::read_active_slug())
    }

    pub fn load() -> Self {
        store::load_active()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let mut s = self.clone();
        s.prune_for_persist();
        store::save_active(&mut s)
    }

    pub fn touch_saved_at(&mut self) {
        self.saved_at = chrono_like_now();
    }

    /// Drop runtime / irrelevant noise before writing JSON.
    pub fn prune_for_persist(&mut self) {
        // Mixer-owned buses never carry meaningful device clocks.
        self.device_clocks
            .retain(|k, _| !k.starts_with("buschain_") && !k.starts_with("shadow_"));
        for t in &mut self.tracks {
            // Empty optionals stay null; clear blank descs.
            if t.input_source_desc
                .as_ref()
                .is_some_and(|d| d.trim().is_empty())
            {
                t.input_source_desc = None;
            }
            if t.input_source.is_none() {
                t.input_source_desc = None;
            }
        }
        if self
            .master_output_desc
            .as_ref()
            .is_some_and(|d| d.trim().is_empty())
        {
            self.master_output_desc = None;
        }
    }
}

fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

impl Session {

    /// Coerce legacy Input → Bus and ensure bus tracks default to master outs.
    /// Also rewrites pre-rebrand `shadow_*` insert labels → `buschain_*`.
    /// Returns true when the session was mutated (caller should persist).
    pub fn normalize(&mut self) -> bool {
        let mut dirty = false;
        let master_id = self
            .tracks
            .iter()
            .find(|t| t.kind.is_master())
            .map(|t| t.id);
        for t in &mut self.tracks {
            if matches!(t.kind, TrackKind::Input) {
                t.kind = TrackKind::Bus;
                dirty = true;
            }
            if !t.kind.is_master() && t.output_targets.is_empty() {
                if let Some(mid) = master_id {
                    t.output_targets.push(mid);
                    dirty = true;
                }
            }
            for plug in &mut t.inserts {
                let migrated = crate::audio::plugin::normalize_label(&plug.id.id);
                if migrated != plug.id.id.as_str() {
                    plug.id.id = migrated.to_string();
                    dirty = true;
                }
            }
        }
        dirty
    }

    /// Snap Custom rate/quantum to true Master HW caps (drops phantom 192k etc.).
    pub fn clamp_performance_to_device(&mut self) {
        use buschain_engine::{probe_master_hw, resolve_profile, AudioPreset};
        let caps = probe_master_hw(self.master_output.as_deref());
        let soft = self.performance.soft_quantum;
        let preset = self.performance.preset;
        self.performance = match preset {
            AudioPreset::Custom => resolve_profile(
                AudioPreset::Custom,
                &caps,
                Some(self.performance.sample_rate),
                Some(self.performance.quantum),
                soft,
            ),
            other => resolve_profile(other, &caps, None, None, soft),
        };
    }

    pub fn master_id(&self) -> Option<Uuid> {
        self.tracks
            .iter()
            .find(|t| t.kind.is_master())
            .map(|t| t.id)
    }

    pub fn track_index(&self, id: Uuid) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }

    pub fn add_track(&mut self, name: impl Into<String>) -> Uuid {
        let id = Uuid::new_v4();
        let master = self.master_id();
        self.tracks.push(Track {
            id,
            name: name.into(),
            kind: TrackKind::Bus,
            gain_db: 0.0,
            mute: false,
            solo: false,
            listen: false,
            inserts: vec![],
            assigned_playback: vec![],
            input_source: None,
            input_source_desc: None,
            output_targets: master.into_iter().collect(),
            // Opt-in: new buses are not system virtual devices until enabled.
            virtual_output: false,
            sink_name: None,
        });
        id
    }

    /// Whether this track should appear as an app-facing virtual sink.
    pub fn is_virtual_output(&self, track_id: Uuid) -> bool {
        self.tracks
            .iter()
            .find(|t| t.id == track_id)
            .map(|t| t.kind.is_master() || t.virtual_output)
            .unwrap_or(false)
    }

    /// Master first (leftmost), then others in stable order.
    pub fn tracks_ui_order(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.tracks.len()).collect();
        idx.sort_by_key(|&i| {
            let master = !self.tracks[i].kind.is_master();
            (master, i)
        });
        idx
    }

    /// Reorder tracks by UI order indices (Master stays first — cannot move).
    /// `from_ui` / `drop_before_ui` are indices into [`Self::tracks_ui_order`].
    pub fn reorder_tracks_ui(&mut self, from_ui: usize, drop_before_ui: usize) -> bool {
        let order = self.tracks_ui_order();
        let n = order.len();
        if from_ui == 0 || from_ui >= n {
            return false;
        }
        if order
            .get(from_ui)
            .map(|&i| self.tracks[i].kind.is_master())
            .unwrap_or(true)
        {
            return false;
        }
        // Keep Master pinned at UI slot 0.
        let drop_before_ui = drop_before_ui.clamp(1, n);
        if drop_before_ui == from_ui || drop_before_ui == from_ui + 1 {
            return false;
        }

        let mut ids: Vec<Uuid> = order.iter().map(|&i| self.tracks[i].id).collect();
        let item = ids.remove(from_ui);
        let insert_at = if drop_before_ui > from_ui {
            drop_before_ui - 1
        } else {
            drop_before_ui
        };
        ids.insert(insert_at, item);

        if let Some(mid) = self.master_id() {
            ids.retain(|id| *id != mid);
            ids.insert(0, mid);
        }

        let old = std::mem::take(&mut self.tracks);
        let mut map: std::collections::HashMap<Uuid, Track> =
            old.into_iter().map(|t| (t.id, t)).collect();
        self.tracks = ids.into_iter().filter_map(|id| map.remove(&id)).collect();
        true
    }
}
