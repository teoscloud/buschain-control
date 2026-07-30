//! Typed MIDI mutations — mirror [`crate::contract::Intent`] for control surfaces.

use super::types::{MidiCcMap, MidiDeviceInfo, MidiRoute};

/// Engine-facing MIDI intents (worker applies; never shell out to `aconnect`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum MidiIntent {
    /// Re-scan PipeWire / ALSA for MIDI endpoints.
    RefreshDevices,
    /// Enable or disable listening on one device.
    SetDeviceEnabled {
        device_id: String,
        enabled: bool,
    },
    /// Upsert a logical route (source → track / insert / master).
    SetRoute { route: MidiRoute },
    /// Drop one route by device + target key.
    RemoveRoute {
        device_id: String,
        target_key: String,
    },
    /// Bind CC → mixer target.
    MapCc { map: MidiCcMap },
    /// Remove one CC map.
    UnmapCc {
        device_id: String,
        channel: u8,
        controller: u8,
    },
    /// Push full session MIDI config to the runtime (maps + routes + devices).
    ApplyConfig {
        devices: Vec<MidiDeviceInfo>,
        routes: Vec<MidiRoute>,
        maps: Vec<MidiCcMap>,
    },
    /// Enter learn mode — next CC on `device_id` binds to `pending` target.
    StartLearn {
        device_id: Option<String>,
        pending: super::types::MidiMapTarget,
    },
    StopLearn,
}
