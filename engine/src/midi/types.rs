//! Session-persisted MIDI device / route / map types.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

fn default_true() -> bool {
    true
}

/// Known MIDI hardware or PipeWire node (persisted enable flag).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MidiDeviceInfo {
    pub id: String,
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Route a MIDI source to a mixer destination (logical — PipeWire links externally).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MidiRoute {
    pub device_id: String,
    pub target: MidiRouteTarget,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum MidiRouteTarget {
    Track { track_id: Uuid },
    Insert { track_id: Uuid, slot_id: Uuid },
    Master,
}

impl MidiRouteTarget {
    pub fn key(&self) -> String {
        match self {
            Self::Track { track_id } => format!("track:{track_id}"),
            Self::Insert { track_id, slot_id } => format!("insert:{track_id}:{slot_id}"),
            Self::Master => "master".into(),
        }
    }
}

/// CC → parameter binding (learn mode writes these).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MidiCcMap {
    pub device_id: String,
    /// 0–15, or 255 = match any channel.
    pub channel: u8,
    pub controller: u8,
    pub target: MidiMapTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MidiMapTarget {
    TrackGain { track_id: Uuid },
    InsertParam {
        track_id: Uuid,
        slot_id: Uuid,
        param: String,
    },
    MasterGain,
}

/// Live device row for the UI (non-persisted extras merged at refresh).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MidiDeviceLive {
    pub id: String,
    pub description: String,
    pub enabled: bool,
    /// 0..1 activity meter (decays in UI).
    pub activity: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MidiSnapshot {
    pub devices: Vec<MidiDeviceLive>,
    pub status: String,
}
