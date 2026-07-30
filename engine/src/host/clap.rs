//! Real CLAP `AudioProcessor` via clack-host + clack-extensions.
//!
//! Off-RT (`Rack::build`): load entry → factory → create → discover params →
//! restore `state_blob` → activate at GraphClock rate/max block → start_processing.
//! RT: planar stereo process + ParamValueEvent inbox; ControlQueue → `set_control`.
//!
//! Crash policy: trusted in-process until Phase P6 sandbox.

use std::ffi::CString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use anyhow::{anyhow, bail, Context, Result};
use clack_extensions::audio_ports::{
    AudioPortInfoBuffer, AudioPortRescanFlags, HostAudioPorts, HostAudioPortsImpl, PluginAudioPorts,
};
use clack_extensions::latency::{HostLatency, HostLatencyImpl, PluginLatency};
use clack_extensions::params::{
    HostParams, HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags, ParamInfoBuffer,
    ParamRescanFlags, PluginParams,
};
use clack_extensions::state::{HostState, HostStateImpl, PluginState};
use clack_host::events::event_types::{MidiEvent as ClapMidiEvent, ParamValueEvent};
use clack_host::events::spaces::CoreEventSpace;
use clack_host::factory::plugin::PluginFactory;
use clack_host::prelude::*;
use clack_host::utils::Cookie;

use super::processor::{AudioProcessor, MidiEvent};

/// One discovered CLAP parameter (plain values).
#[derive(Debug, Clone)]
pub struct ClapParamInfo {
    pub id: u32,
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub value: f32,
    pub cookie: Cookie,
}

struct BusClapHost;

struct BusClapShared {
    restart_requested: AtomicBool,
    process_requested: AtomicBool,
    callback_requested: AtomicBool,
    params_ext: OnceLock<Option<PluginParams>>,
    latency_ext: OnceLock<Option<PluginLatency>>,
    state_ext: OnceLock<Option<PluginState>>,
    audio_ports_ext: OnceLock<Option<PluginAudioPorts>>,
}

impl Default for BusClapShared {
    fn default() -> Self {
        Self {
            restart_requested: AtomicBool::new(false),
            process_requested: AtomicBool::new(false),
            callback_requested: AtomicBool::new(false),
            params_ext: OnceLock::new(),
            latency_ext: OnceLock::new(),
            state_ext: OnceLock::new(),
            audio_ports_ext: OnceLock::new(),
        }
    }
}

impl<'a> SharedHandler<'a> for BusClapShared {
    fn initializing(&self, instance: InitializingPluginHandle<'a>) {
        let _ = self.params_ext.set(instance.get_extension());
        let _ = self.latency_ext.set(instance.get_extension());
        let _ = self.state_ext.set(instance.get_extension());
        let _ = self.audio_ports_ext.set(instance.get_extension());
    }

    fn request_restart(&self) {
        self.restart_requested.store(true, Ordering::Relaxed);
    }

    fn request_process(&self) {
        self.process_requested.store(true, Ordering::Relaxed);
    }

    fn request_callback(&self) {
        self.callback_requested.store(true, Ordering::Relaxed);
    }
}

impl HostParamsImplShared for BusClapShared {
    fn request_flush(&self) {
        // Params are flushed via process() input events while streaming.
    }
}

struct BusClapMainThread<'a> {
    shared: &'a BusClapShared,
    _handle: Option<InitializedPluginHandle<'a>>,
    latency_changed: AtomicBool,
    state_dirty: AtomicBool,
}

impl<'a> BusClapMainThread<'a> {
    fn new(shared: &'a BusClapShared) -> Self {
        Self {
            shared,
            _handle: None,
            latency_changed: AtomicBool::new(false),
            state_dirty: AtomicBool::new(false),
        }
    }
}

impl<'a> MainThreadHandler<'a> for BusClapMainThread<'a> {
    fn initialized(&mut self, instance: InitializedPluginHandle<'a>) {
        self._handle = Some(instance);
    }
}

impl HostLatencyImpl for BusClapMainThread<'_> {
    fn changed(&mut self) {
        self.latency_changed.store(true, Ordering::Relaxed);
    }
}

impl HostStateImpl for BusClapMainThread<'_> {
    fn mark_dirty(&mut self) {
        self.state_dirty.store(true, Ordering::Relaxed);
    }
}

impl HostParamsImplMainThread for BusClapMainThread<'_> {
    fn rescan(&mut self, _flags: ParamRescanFlags) {}
    fn clear(&mut self, _param_id: ClapId, _flags: ParamClearFlags) {}
}

impl HostAudioPortsImpl for BusClapMainThread<'_> {
    fn is_rescan_flag_supported(&self, _flag: AudioPortRescanFlags) -> bool {
        false
    }

    fn rescan(&mut self, _flags: AudioPortRescanFlags) {}
}

impl HostHandlers for BusClapHost {
    type Shared<'a> = BusClapShared;
    type MainThread<'a> = BusClapMainThread<'a>;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder
            .register::<HostParams>()
            .register::<HostLatency>()
            .register::<HostState>()
            .register::<HostAudioPorts>();
    }
}

/// `PluginInstance` is !Send by design; BusChain only touches main-thread CLAP
/// APIs on the ensure/gen-swap worker, never concurrently with RT `process`.
struct SendInstance(PluginInstance<BusClapHost>);
// SAFETY: see comment on struct.
unsafe impl Send for SendInstance {}

struct LiveClap {
    instance: SendInstance,
    processor: StartedPluginAudioProcessor<BusClapHost>,
    input_ports: AudioPorts,
    output_ports: AudioPorts,
    /// Scratch planar buffers (size = max_block).
    in_l: Vec<f32>,
    in_r: Vec<f32>,
    out_l: Vec<f32>,
    out_r: Vec<f32>,
    event_buf: EventBuffer,
    out_events: EventBuffer,
    steady_time: u64,
}

/// Activated CLAP insert.
pub struct ClapInstance {
    plugin_id: String,
    params: Vec<ClapParamInfo>,
    /// Pending (param_index, value) from ControlQueue — drained into process events.
    pending: Vec<(usize, f32)>,
    /// Pending MIDI bytes (status, d1, d2) — drained into process events (M2).
    pending_midi: Vec<[u8; 3]>,
    latency: u32,
    state_blob: Option<Vec<u8>>,
    sample_rate: u32,
    max_block: u32,
    live: Option<LiveClap>,
}

impl ClapInstance {
    pub fn load(
        path: &str,
        plugin_id: &str,
        state_blob: Option<Vec<u8>>,
        sample_rate: u32,
        max_block: u32,
    ) -> Result<Self> {
        if !Path::new(path).exists() {
            bail!("CLAP file not found: {path}");
        }
        if sample_rate == 0 || max_block == 0 {
            bail!("invalid audio config sr={sample_rate} max_block={max_block}");
        }

        let host_info = HostInfo::new(
            "BusChain Control",
            "BusChain",
            "https://github.com/buschain/buschain-control",
            env!("CARGO_PKG_VERSION"),
        )
        .map_err(|e| anyhow!("HostInfo: {e}"))?;

        // SAFETY: path points at a user/system CLAP bundle; clack validates entry.
        let entry = unsafe { PluginEntry::load(path) }
            .with_context(|| format!("CLAP entry load {path}"))?;

        let factory = entry
            .get_plugin_factory()
            .ok_or_else(|| anyhow!("no plugin factory in {path}"))?;

        let id_c = resolve_plugin_id(&factory, plugin_id, path)?;

        let mut instance = PluginInstance::<BusClapHost>::new(
            |_| BusClapShared::default(),
            |shared| BusClapMainThread::new(shared),
            &entry,
            &id_c,
            &host_info,
        )
        .map_err(|e| anyhow!("instantiate {plugin_id}: {e}"))?;

        let params = discover_params(&mut instance);
        if let Some(ref blob) = state_blob {
            let _ = load_state(&mut instance, blob);
        }

        // Session controls override defaults / state for named params.
        // (Applied again via apply_controls from Rack::build.)

        let max_block = max_block.max(1);
        let config = PluginAudioConfiguration {
            sample_rate: sample_rate as f64,
            min_frames_count: 1,
            max_frames_count: max_block as u32,
        };

        let stopped = instance
            .activate(|_, _| (), config)
            .map_err(|e| anyhow!("activate {plugin_id}: {e}"))?;
        let processor = stopped
            .start_processing()
            .map_err(|e| anyhow!("start_processing {plugin_id}: {e}"))?;

        let latency = query_latency(&mut instance);

        let mut live = LiveClap {
            instance: SendInstance(instance),
            processor,
            input_ports: AudioPorts::with_capacity(2, 1),
            output_ports: AudioPorts::with_capacity(2, 1),
            in_l: vec![0.0; max_block as usize],
            in_r: vec![0.0; max_block as usize],
            out_l: vec![0.0; max_block as usize],
            out_r: vec![0.0; max_block as usize],
            event_buf: EventBuffer::new(),
            out_events: EventBuffer::new(),
            steady_time: 0,
        };

        // Touch audio-ports ext so we fail soft if layout is exotic (still stereo planar).
        let _ = query_stereo_ok(&mut live.instance.0);

        Ok(Self {
            plugin_id: plugin_id.to_string(),
            params,
            pending: Vec::new(),
            pending_midi: Vec::new(),
            latency,
            state_blob,
            sample_rate,
            max_block,
            live: Some(live),
        })
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn state_blob(&self) -> Option<&[u8]> {
        self.state_blob.as_deref()
    }

    pub fn set_state_blob(&mut self, blob: Option<Vec<u8>>) {
        self.state_blob = blob;
    }

    /// Snapshot current plugin state into `state_blob` (off-RT).
    pub fn capture_state(&mut self) -> Option<&[u8]> {
        let Some(live) = self.live.as_mut() else {
            return self.state_blob.as_deref();
        };
        let mut buf = Vec::new();
        let ok = live.instance.0.access_handler_mut(|_mt| {
            // State save needs PluginMainThreadHandle — via plugin_handle on instance.
            true
        });
        let _ = ok;
        if let Ok(()) = save_state(&mut live.instance.0, &mut buf) {
            if !buf.is_empty() {
                self.state_blob = Some(buf);
            }
        }
        self.state_blob.as_deref()
    }

    pub fn discovered_params(&self) -> &[ClapParamInfo] {
        &self.params
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn max_block(&self) -> u32 {
        self.max_block
    }
}

impl Drop for ClapInstance {
    fn drop(&mut self) {
        if let Some(mut live) = self.live.take() {
            // Best-effort state capture before teardown.
            let mut buf = Vec::new();
            if save_state(&mut live.instance.0, &mut buf).is_ok() && !buf.is_empty() {
                self.state_blob = Some(buf);
            }
            let stopped = live.processor.stop_processing();
            live.instance.0.deactivate(stopped);
        }
    }
}

impl AudioProcessor for ClapInstance {
    fn prepare(&mut self, _sample_rate: u32, _max_block: u32) {
        // Activated at load with GraphClock rate / max block.
    }

    fn latency_samples(&self) -> u32 {
        self.latency
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        if n == 0 {
            return;
        }
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if n > live.in_l.len() {
            // Block larger than activate max — skip (should not happen under GraphClock).
            return;
        }

        live.in_l[..n].copy_from_slice(&left[..n]);
        live.in_r[..n].copy_from_slice(&right[..n]);

        live.event_buf.clear();
        live.out_events.clear();
        for (idx, value) in self.pending.drain(..) {
            if let Some(p) = self.params.get(idx) {
                let ev = ParamValueEvent::new(
                    0,
                    ClapId::from_raw(p.id).unwrap_or(ClapId::from_raw(0).unwrap()),
                    Pckn::match_all(),
                    value as f64,
                    p.cookie,
                );
                live.event_buf.push(&ev);
                if let Some(param) = self.params.get_mut(idx) {
                    param.value = value;
                }
            }
        }
        for data in self.pending_midi.drain(..) {
            live.event_buf.push(&ClapMidiEvent::new(0, 0, data));
        }

        let input_events = InputEvents::from_buffer(&live.event_buf);
        let mut output_events = OutputEvents::from_buffer(&mut live.out_events);

        let input_audio = live.input_ports.with_input_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_input_only(
                [&mut live.in_l[..n], &mut live.in_r[..n]]
                    .into_iter()
                    .map(InputChannel::variable),
            ),
        }]);
        let mut output_audio = live.output_ports.with_output_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only(
                [&mut live.out_l[..n], &mut live.out_r[..n]].into_iter(),
            ),
        }]);

        let _ = live.processor.process(
            &input_audio,
            &mut output_audio,
            &input_events,
            &mut output_events,
            Some(live.steady_time),
            None,
        );
        live.steady_time = live.steady_time.saturating_add(n as u64);

        left[..n].copy_from_slice(&live.out_l[..n]);
        right[..n].copy_from_slice(&live.out_r[..n]);

        // Absorb plugin→host param feedback (UI sync later).
        for i in 0..live.out_events.len() {
            if let Some(unknown) = live.out_events.get(i) {
                if let Some(CoreEventSpace::ParamValue(ev)) = unknown.as_core_event() {
                    if let Some(pid) = ev.param_id() {
                        let id = pid.get();
                        let val = ev.value() as f32;
                        if let Some(p) = self.params.iter_mut().find(|p| p.id == id) {
                            p.value = val;
                        }
                    }
                }
            }
        }
    }

    fn set_control(&mut self, index: usize, value: f32) {
        if index >= self.params.len() {
            return;
        }
        let min = self.params[index].min;
        let max = self.params[index].max;
        let v = value.clamp(min, max);
        self.pending.push((index, v));
    }

    fn control_count(&self) -> usize {
        self.params.len()
    }

    fn control_name(&self, index: usize) -> Option<&str> {
        self.params.get(index).map(|p| p.name.as_str())
    }

    fn control_value(&self, index: usize) -> Option<f32> {
        // Session / apply_controls use raw CLAP units (min..max), not 0..1.
        self.params.get(index).map(|p| p.value)
    }

    fn capture_state_blob(&mut self) -> Option<Vec<u8>> {
        self.capture_state().map(|b| b.to_vec())
    }

    fn feed_midi(&mut self, events: &[MidiEvent]) {
        for ev in events {
            let bytes = match *ev {
                MidiEvent::NoteOn {
                    channel,
                    note,
                    velocity,
                } => [0x90 | (channel & 0x0f), note & 0x7f, velocity & 0x7f],
                MidiEvent::NoteOff {
                    channel,
                    note,
                    velocity,
                } => [0x80 | (channel & 0x0f), note & 0x7f, velocity & 0x7f],
                MidiEvent::Cc {
                    channel,
                    controller,
                    value,
                } => [0xb0 | (channel & 0x0f), controller & 0x7f, value & 0x7f],
            };
            self.pending_midi.push(bytes);
        }
    }
}

/// Apply session control map onto a freshly loaded instance.
pub fn apply_controls(inst: &mut ClapInstance, controls: &[(String, f32)]) {
    for (name, val) in controls {
        let lname = name.to_ascii_lowercase();
        if lname == "bypass" || lname == "enable" {
            continue;
        }
        if let Some(idx) = inst
            .params
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name))
        {
            inst.set_control(idx, *val);
        }
    }
}

/// Off-RT param discovery for mixer UI (loads + activates briefly).
pub fn probe_clap_params(path: &str, plugin_id: &str) -> Result<Vec<ClapParamInfo>> {
    let inst = ClapInstance::load(path, plugin_id, None, 48_000, 256)?;
    Ok(inst.discovered_params().to_vec())
}

/// Best-effort: verify the file exports `clap_entry` (scan/validate).
pub fn has_clap_entry(path: &str) -> bool {
    // SAFETY: discovery-only open.
    unsafe { PluginEntry::load(path).is_ok() }
}

fn host_info_static() -> HostInfo {
    HostInfo::new(
        "BusChain Control",
        "BusChain",
        "https://github.com/buschain/buschain-control",
        env!("CARGO_PKG_VERSION"),
    )
    .expect("HostInfo")
}

fn resolve_plugin_id(
    factory: &PluginFactory<'_>,
    plugin_id: &str,
    path: &str,
) -> Result<CString> {
    if let Ok(c) = CString::new(plugin_id) {
        if factory
            .plugin_descriptors()
            .any(|d| d.id().map(|id| id.to_bytes() == c.as_bytes()).unwrap_or(false))
        {
            return Ok(c);
        }
    }
    // Fall back to first descriptor in the bundle.
    let first = factory
        .plugin_descriptors()
        .next()
        .and_then(|d| d.id().map(|id| CString::new(id.to_bytes()).ok()).flatten())
        .ok_or_else(|| anyhow!("no plugins in {path}"))?;
    Ok(first)
}

fn discover_params(instance: &mut PluginInstance<BusClapHost>) -> Vec<ClapParamInfo> {
    let mut out = Vec::new();
    let ext = instance.access_shared_handler(|s| s.params_ext.get().cloned().flatten());
    let Some(params) = ext else {
        return out;
    };
    instance.access_handler_mut(|_mt| {});
    let mut handle = instance.plugin_handle();
    let count = params.count(&mut handle);
    let mut info_buf = ParamInfoBuffer::new();
    for i in 0..count {
        let Some(info) = params.get_info(&mut handle, i, &mut info_buf) else {
            continue;
        };
        let name = String::from_utf8_lossy(info.name)
            .trim_end_matches('\0')
            .to_string();
        if name.is_empty() {
            continue;
        }
        let value = params
            .get_value(&mut handle, info.id)
            .unwrap_or(info.default_value) as f32;
        out.push(ClapParamInfo {
            id: info.id.get(),
            name,
            min: info.min_value as f32,
            max: info.max_value as f32,
            default: info.default_value as f32,
            value,
            cookie: info.cookie,
        });
    }
    out
}

fn load_state(instance: &mut PluginInstance<BusClapHost>, blob: &[u8]) -> Result<()> {
    let ext = instance
        .access_shared_handler(|s| s.state_ext.get().cloned().flatten())
        .ok_or_else(|| anyhow!("no state extension"))?;
    let mut handle = instance.plugin_handle();
    let mut cursor = std::io::Cursor::new(blob);
    ext.load(&mut handle, &mut cursor)
        .map_err(|_| anyhow!("CLAP state load failed"))
}

fn save_state(instance: &mut PluginInstance<BusClapHost>, buf: &mut Vec<u8>) -> Result<()> {
    let ext = instance
        .access_shared_handler(|s| s.state_ext.get().cloned().flatten())
        .ok_or_else(|| anyhow!("no state extension"))?;
    let mut handle = instance.plugin_handle();
    buf.clear();
    ext.save(&mut handle, buf)
        .map_err(|_| anyhow!("CLAP state save failed"))
}

fn query_latency(instance: &mut PluginInstance<BusClapHost>) -> u32 {
    let ext = instance.access_shared_handler(|s| s.latency_ext.get().cloned().flatten());
    let Some(lat) = ext else {
        return 0;
    };
    let mut handle = instance.plugin_handle();
    lat.get(&mut handle)
}

fn query_stereo_ok(instance: &mut PluginInstance<BusClapHost>) -> bool {
    let ext = instance.access_shared_handler(|s| s.audio_ports_ext.get().cloned().flatten());
    let Some(ports) = ext else {
        return true; // no ext → assume default stereo
    };
    let mut handle = instance.plugin_handle();
    let mut buf = AudioPortInfoBuffer::new();
    let n_in = ports.count(&mut handle, true);
    let n_out = ports.count(&mut handle, false);
    if n_in == 0 || n_out == 0 {
        return false;
    }
    let _ = ports.get(&mut handle, 0, false, &mut buf);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_errors() {
        assert!(ClapInstance::load("/no/such/plugin.clap", "x", None, 48000, 256).is_err());
    }

    #[test]
    fn host_info_builds() {
        let _ = host_info_static();
    }
}
