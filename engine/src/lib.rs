//! BusChain Control engine — contracts over PipeWire/Pulse.
//!
//! Application code must not shell out to pactl/pw-link/pw-cli. Use [`Engine`]
//! and [`contract::Intent`] instead.

pub mod backend;
pub mod clock;
pub mod contract;
pub mod domain;
pub mod fx_busy;
pub mod fx_gen;
pub mod fx_trace;
pub mod host;
pub mod midi;
pub mod pipeline;
pub mod plan;
pub mod runtime;

pub use fx_busy::{is_rebuilding as fx_is_rebuilding, RebuildGuard as FxRebuildGuard};
pub use fx_gen::{any_gen_live, heal_live_gen, live_fx_name, live_post_name};

pub use clock::{
    invalidate_clock_probe_caches, probe_endpoint_caps, probe_master_hw_from_sinks, probe_rates_pw,
    probe_sink_running_rate, probe_source_running_rate, resolve_profile, set_graph_force_clock,
    wait_hw_running_rate, AudioPreset, DeviceCaps, EndpointCaps, GraphClock, PerformanceProfile,
};
pub use contract::{ApplyReport, ClockProps, Intent};
pub use domain::{
    bus_suffix, fx_name_for_bus, normalize_ladspa_label, post_name_for_bus, ChainEnsureMode,
    ChainSpec, ChainState, DeviceNode, GraphSnapshot, InsertFormat, InsertSlot, LinkSpec, NodeName,
    NodeRole, NodeSpec, Props, WirePlan,
};
pub use plan::{BusLevel, DesiredState};
pub use pipeline::{
    dry_spine_instant_ready, spine_instant_ready, track_path_ready, DRY_DWELL, WET_DWELL,
};
pub use midi::{
    enumerate_devices, MidiAction, MidiCcMap, MidiDeviceInfo, MidiDeviceLive, MidiIntent,
    MidiMapTarget, MidiRoute, MidiRouteTarget, MidiSnapshot,
};
pub use host::{
    clear_surface_slot, close_editor, drain_editor_closed_events, drain_editor_param_events,
    ensure_editor_ipc, harvest_host_slot_states, mark_surface_slot, probe_clap_params,
    probe_lv2_params, probe_vst3_params, request_open_editor, slot_wants_surface, surface_ctrl_ready,
    surface_enabled, surface_forget_ctrl, surface_send_ctrl, AudioProcessor, ClapInstance,
    ClapParamInfo, EditorParamEvent, Lv2ParamInfo,
    MidiEvent, OpenEditorRequest, SlotStateSnapshot, Vst3Instance, Vst3ParamInfo,
};
pub use runtime::{plan_capture_delta, Engine};

/// Convenience probe used by the app (lists sinks via CLI backend).
pub fn probe_master_hw(master_output: Option<&str>) -> DeviceCaps {
    // Prefer a named sink without spinning a full Engine snapshot when possible.
    if let Some(name) = master_output.filter(|s| !s.is_empty()) {
        return probe_named_device_caps(name, name, false);
    }
    let mut eng = Engine::new();
    eng.probe_master_hw(master_output)
}

/// Caps for one known sink/source name (no Engine snapshot). Safe for rare UI cache fills.
pub fn probe_named_device_caps(name: &str, description: &str, is_source: bool) -> DeviceCaps {
    let mut rates = probe_rates_pw(name);
    let running = if is_source {
        probe_source_running_rate(name)
    } else {
        probe_sink_running_rate(name)
    };
    let preferred = running
        .or_else(|| rates.first().copied())
        .unwrap_or(48_000);
    if rates.is_empty() {
        rates.push(preferred);
    }
    if let Some(r) = running {
        rates.retain(|x| *x != r);
        rates.insert(0, r);
    }
    DeviceCaps {
        sink_name: name.to_string(),
        description: description.to_string(),
        rates,
        preferred_rate: preferred,
        min_quantum: 64,
        max_quantum: 2048,
        preferred_quantum: 256,
    }
}
