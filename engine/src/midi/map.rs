//! CC → ControlMsg / track-level actions.

use uuid::Uuid;

use crate::host::ControlMsg;
use crate::host::registry;

use super::event::MidiEvent;
use super::types::{MidiCcMap, MidiMapTarget, MidiRoute, MidiRouteTarget};

/// Non-RT action emitted when a CC maps to something outside the insert host.
#[derive(Debug, Clone)]
pub enum MidiAction {
    SetTrackLevel {
        sink: String,
        gain_db: f32,
    },
    MapLearned {
        map: MidiCcMap,
    },
}

/// Resolve CC value (0..127) to linear 0..1.
pub fn cc_to_norm(value: u8) -> f32 {
    (value as f32 / 127.0).clamp(0.0, 1.0)
}

/// Map CC 0..127 to fader dB (−48..+12).
pub fn cc_to_gain_db(value: u8) -> f32 {
    let t = cc_to_norm(value);
    -48.0 + t * 60.0
}

pub struct MapContext<'a> {
    pub maps: &'a [MidiCcMap],
    pub routes: &'a [MidiRoute],
    pub track_sink: &'a dyn Fn(Uuid) -> Option<String>,
    pub track_bus: &'a dyn Fn(Uuid) -> Option<String>,
}

impl MapContext<'_> {
    /// Apply one MIDI event; returns optional UI/learn side effects.
    pub fn dispatch(
        &self,
        device_id: &str,
        event: &MidiEvent,
        learn: Option<&MidiMapTarget>,
    ) -> Option<MidiAction> {
        let MidiEvent::Cc {
            channel,
            controller,
            value,
        } = event
        else {
            return None;
        };

        if !self.route_allows(device_id, *channel) {
            return None;
        }

        if let Some(pending) = learn {
            let map = MidiCcMap {
                device_id: device_id.to_string(),
                channel: *channel,
                controller: *controller,
                target: pending.clone(),
            };
            self.apply_map(&map, *value);
            return Some(MidiAction::MapLearned { map });
        }

        let map = self.find_map(device_id, *channel, *controller)?;
        self.apply_map(map, *value);
        None
    }

    fn route_allows(&self, device_id: &str, _channel: u8) -> bool {
        self.routes
            .iter()
            .any(|r| r.enabled && r.device_id == device_id)
            || self.maps.iter().any(|m| m.device_id == device_id)
    }

    fn find_map(&self, device_id: &str, channel: u8, controller: u8) -> Option<&MidiCcMap> {
        self.maps.iter().find(|m| {
            m.device_id == device_id
                && m.controller == controller
                && (m.channel == 255 || m.channel == channel)
        })
    }

    fn apply_map(&self, map: &MidiCcMap, value: u8) {
        match &map.target {
            MidiMapTarget::TrackGain { track_id } => {
                if let Some(sink) = (self.track_sink)(*track_id) {
                    let gain_db = cc_to_gain_db(value);
                    // Track gain is applied by the app worker (SetTrackLevel).
                    let _ = (sink, gain_db);
                }
            }
            MidiMapTarget::MasterGain => {
                let _ = cc_to_gain_db(value);
            }
            MidiMapTarget::InsertParam {
                track_id,
                slot_id,
                param,
            } => {
                if let Some(bus) = (self.track_bus)(*track_id) {
                    if let Some(idx) = registry::control_index_for(&bus, *slot_id, param) {
                        let norm = cc_to_norm(value);
                        registry::push_host_param(&bus, *slot_id, idx, norm);
                    }
                }
            }
        }
    }
}

/// Push mapped insert param directly (used from runtime after lookup).
pub fn apply_insert_cc(
    bus: &str,
    slot_id: Uuid,
    param: &str,
    value: u8,
) -> bool {
    if let Some(idx) = registry::control_index_for(bus, slot_id, param) {
        registry::push_host_param(bus, slot_id, idx, cc_to_norm(value));
        true
    } else {
        false
    }
}

/// Apply a map with external track sink/bus resolution; returns track-level action if needed.
pub fn apply_map_with(
    map: &MidiCcMap,
    value: u8,
    track_sink: &dyn Fn(Uuid) -> Option<String>,
    track_bus: &dyn Fn(Uuid) -> Option<String>,
) -> Option<MidiAction> {
    match &map.target {
        MidiMapTarget::TrackGain { track_id } => {
            let sink = track_sink(*track_id)?;
            Some(MidiAction::SetTrackLevel {
                sink,
                gain_db: cc_to_gain_db(value),
            })
        }
        MidiMapTarget::MasterGain => track_sink(Uuid::nil()).map(|sink| MidiAction::SetTrackLevel {
            sink,
            gain_db: cc_to_gain_db(value),
        }),
        MidiMapTarget::InsertParam {
            track_id,
            slot_id,
            param,
        } => {
            if let Some(bus) = track_bus(*track_id) {
                apply_insert_cc(&bus, *slot_id, param, value);
            }
            None
        }
    }
}

/// Route matrix: is this device routed to the given target?
pub fn route_enabled(routes: &[MidiRoute], device_id: &str, target: &MidiRouteTarget) -> bool {
    routes.iter().any(|r| {
        r.enabled && r.device_id == device_id && &r.target == target
    })
}

/// Hot path: CC already resolved → ControlMsg on the host queue.
pub fn control_msg_for_param(
    slot_id: Uuid,
    control_index: u32,
    value: u8,
) -> ControlMsg {
    ControlMsg::param(slot_id, control_index, cc_to_norm(value))
}
