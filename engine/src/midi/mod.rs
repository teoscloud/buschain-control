//! Control-surface MIDI — enumerate PipeWire nodes, map CC → mixer targets.
//!
//! Hot path: MIDI worker reads events → pushes [`crate::host::control::ControlMsg`]
//! (insert params) or emits track-level actions for the app worker.
//! Never shells out to `aconnect`.

mod enumerate;
mod event;
mod intent;
mod map;
mod queue;
mod runtime;
mod types;

pub use enumerate::{enumerate_devices, enumerate_pw, enumerate_pw_cli};
pub use event::MidiEvent;
pub use intent::MidiIntent;
pub use map::{apply_insert_cc, cc_to_gain_db, cc_to_norm, control_msg_for_param, MidiAction};
pub use queue::MidiEventQueue;
pub use runtime::{
    apply_intent_global, ensure_runtime, poll_actions_global, snapshot_global, MidiRuntime,
};
pub use types::{
    MidiCcMap, MidiDeviceInfo, MidiDeviceLive, MidiMapTarget, MidiRoute, MidiRouteTarget,
    MidiSnapshot,
};
