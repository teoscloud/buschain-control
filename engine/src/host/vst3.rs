//! Native VST3 `AudioProcessor` via `vst3-host` (in-process; Carla remains discovery-only).
//!
//! Off-RT: Module → Factory → IComponent / IAudioProcessor activate at GraphClock
//! rate/max block; restore `state_blob`; discover parameters.
//! RT: planar stereo via reused `AudioBuffers`; ControlQueue → normalized params.
//! MIDI notes/CC feed through `feed_midi` (M2).

use anyhow::{anyhow, bail, Context, Result};
use vst3_host::audio::AudioBuffers;
use vst3_host::host::Vst3Host;
use vst3_host::midi::MidiChannel;
use vst3_host::plugin::Plugin;
use vst3_host::prelude::Parameter;

use super::processor::{AudioProcessor, MidiEvent};

/// Discovered VST3 parameter (normalized 0..1).
#[derive(Debug, Clone)]
pub struct Vst3ParamInfo {
    pub id: u32,
    pub name: String,
    pub value: f32,
    pub default: f32,
}

/// Activated VST3 insert.
pub struct Vst3Instance {
    plugin_id: String,
    plugin: Plugin,
    buffers: AudioBuffers,
    params: Vec<Vst3ParamInfo>,
    latency: u32,
    state_blob: Option<Vec<u8>>,
    sample_rate: u32,
    max_block: u32,
}

impl Vst3Instance {
    pub fn load(
        path: &str,
        plugin_id: &str,
        state_blob: Option<Vec<u8>>,
        sample_rate: u32,
        max_block: u32,
    ) -> Result<Self> {
        if !std::path::Path::new(path).exists() {
            bail!("VST3 module not found: {path}");
        }
        if sample_rate == 0 || max_block == 0 {
            bail!("invalid audio config sr={sample_rate} max_block={max_block}");
        }

        let p = std::path::Path::new(path);
        if p.is_dir() && !crate::host::arch::vst3_bundle_has_host_binary(p) {
            bail!(
                "VST3 has no {} binary (wrong arch): {path}",
                crate::host::arch::host_arch_label()
            );
        }
        if p.is_file() && !crate::host::arch::elf_matches_host(p) {
            bail!(
                "VST3 ELF arch mismatch (need {}): {path}",
                crate::host::arch::host_arch_label()
            );
        }

        let mut host = Vst3Host::builder()
            .sample_rate(sample_rate as f64)
            .block_size(max_block as usize)
            .input_channels(2)
            .output_channels(2)
            .build()
            .map_err(|e| anyhow!("VST3 host: {e}"))?;

        let load_path = resolve_module_path(path);
        let mut plugin = if plugin_id.is_empty()
            || plugin_id == path
            || plugin_id == load_path
            || std::path::Path::new(plugin_id).exists()
        {
            host.load_plugin(&load_path)
                .with_context(|| format!("VST3 load {load_path}"))?
        } else {
            host.load_plugin_class(&load_path, plugin_id)
                .or_else(|_| host.load_plugin(&load_path))
                .with_context(|| format!("VST3 load {load_path} class={plugin_id}"))?
        };

        // Prefer stereo main bus when the plugin negotiates arrangements.
        use vst3_host::audio::SpeakerArrangement;
        let _ = plugin.set_bus_arrangements(
            &[SpeakerArrangement::STEREO],
            &[SpeakerArrangement::STEREO],
        );

        if let Some(ref blob) = state_blob {
            let _ = plugin.load_state(blob);
        }

        plugin
            .start_processing()
            .map_err(|e| anyhow!("VST3 start_processing: {e}"))?;

        let params = discover_params(&plugin);
        let latency = plugin.latency_samples();
        let buffers = AudioBuffers::new(2, 2, max_block as usize, sample_rate as f64);

        Ok(Self {
            plugin_id: plugin_id.to_string(),
            plugin,
            buffers,
            params,
            latency,
            state_blob,
            sample_rate,
            max_block,
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

    pub fn capture_state(&mut self) -> Option<&[u8]> {
        if let Ok(blob) = self.plugin.save_state() {
            if !blob.is_empty() {
                self.state_blob = Some(blob);
            }
        }
        self.state_blob.as_deref()
    }

    pub fn discovered_params(&self) -> &[Vst3ParamInfo] {
        &self.params
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn max_block(&self) -> u32 {
        self.max_block
    }
}

impl Drop for Vst3Instance {
    fn drop(&mut self) {
        let _ = self.capture_state();
        let _ = self.plugin.stop_processing();
    }
}

impl AudioProcessor for Vst3Instance {
    fn prepare(&mut self, _sample_rate: u32, _max_block: u32) {}

    fn latency_samples(&self) -> u32 {
        self.latency
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len()).min(self.buffers.block_size);
        if n == 0 {
            return;
        }

        self.buffers.inputs[0][..n].copy_from_slice(&left[..n]);
        self.buffers.inputs[1][..n].copy_from_slice(&right[..n]);
        // Silence remainder so leftover frames never leak.
        if n < self.buffers.block_size {
            self.buffers.inputs[0][n..].fill(0.0);
            self.buffers.inputs[1][n..].fill(0.0);
        }
        for ch in &mut self.buffers.outputs {
            ch.fill(0.0);
        }

        // vst3-host process splits/uses numSamples from buffer lengths; temporarily
        // shrink channel vec lens via block_size field + truncating copy path.
        let saved_bs = self.buffers.block_size;
        self.buffers.block_size = n;
        // Truncate channel lengths for this call without realloc if possible.
        for ch in &mut self.buffers.inputs {
            ch.truncate(n);
        }
        for ch in &mut self.buffers.outputs {
            ch.truncate(n);
        }

        let _ = self.plugin.process_audio(&mut self.buffers);

        if self.buffers.outputs.len() >= 2 {
            let out_n = self.buffers.outputs[0].len().min(n);
            left[..out_n].copy_from_slice(&self.buffers.outputs[0][..out_n]);
            right[..out_n].copy_from_slice(&self.buffers.outputs[1][..out_n]);
        }

        // Restore buffer capacity for next block.
        for ch in &mut self.buffers.inputs {
            ch.resize(saved_bs, 0.0);
        }
        for ch in &mut self.buffers.outputs {
            ch.resize(saved_bs, 0.0);
        }
        self.buffers.block_size = saved_bs;
    }

    fn set_control(&mut self, index: usize, value: f32) {
        let Some(p) = self.params.get(index) else {
            return;
        };
        let id = p.id;
        let v = value.clamp(0.0, 1.0) as f64;
        if self.plugin.set_parameter(id, v).is_ok() {
            if let Some(p) = self.params.get_mut(index) {
                p.value = v as f32;
            }
        }
    }

    fn control_count(&self) -> usize {
        self.params.len()
    }

    fn control_name(&self, index: usize) -> Option<&str> {
        self.params.get(index).map(|p| p.name.as_str())
    }

    fn control_value(&self, index: usize) -> Option<f32> {
        self.params.get(index).map(|p| p.value)
    }

    fn capture_state_blob(&mut self) -> Option<Vec<u8>> {
        self.capture_state().map(|b| b.to_vec())
    }

    fn feed_midi(&mut self, events: &[MidiEvent]) {
        for ev in events {
            match *ev {
                MidiEvent::NoteOn {
                    channel,
                    note,
                    velocity,
                } => {
                    let ch = midi_channel(channel);
                    let _ = self.plugin.send_midi_note(note, velocity, ch);
                }
                MidiEvent::NoteOff {
                    channel,
                    note,
                    velocity: _,
                } => {
                    let ch = midi_channel(channel);
                    let _ = self.plugin.send_midi_note_off(note, ch);
                }
                MidiEvent::Cc {
                    channel,
                    controller,
                    value,
                } => {
                    let ch = midi_channel(channel);
                    let _ = self.plugin.send_midi_cc(controller, value, ch);
                }
            }
        }
    }
}

/// Apply session control map onto a freshly loaded instance.
pub fn apply_controls(inst: &mut Vst3Instance, controls: &[(String, f32)]) {
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

/// Off-RT param probe for Mixer dynamic UI (no audio activate required when possible).
pub fn probe_vst3_params(path: &str, plugin_id: &str) -> Vec<(String, f32, f32, f32)> {
    let Ok(inst) = Vst3Instance::load(path, plugin_id, None, 48_000, 256) else {
        return Vec::new();
    };
    inst.params
        .iter()
        .map(|p| (p.name.clone(), 0.0, 1.0, p.default))
        .collect()
}

fn discover_params(plugin: &Plugin) -> Vec<Vst3ParamInfo> {
    let Ok(list) = plugin.get_parameters() else {
        return vec![Vst3ParamInfo {
            id: 0,
            name: "Gain".into(),
            value: 1.0,
            default: 1.0,
        }];
    };
    let mut out: Vec<Vst3ParamInfo> = list
        .into_iter()
        .filter(|p: &Parameter| !p.is_read_only)
        .map(|p| Vst3ParamInfo {
            id: p.id,
            name: if p.name.is_empty() {
                format!("Param {}", p.id)
            } else {
                p.name
            },
            value: p.value as f32,
            default: p.default as f32,
        })
        .collect();
    if out.is_empty() {
        out.push(Vst3ParamInfo {
            id: 0,
            name: "Gain".into(),
            value: 1.0,
            default: 1.0,
        });
    }
    out
}

/// Accept a `.vst3` bundle directory/file, or a direct `.so` under Contents/.
fn resolve_module_path(path: &str) -> String {
    let p = std::path::Path::new(path);
    if p.is_file() {
        return path.to_string();
    }
    if p.is_dir() {
        // Prefer host-arch Contents/ first so multi-arch bundles load natively.
        let host_sub = crate::host::arch::host_vst3_contents_subdir();
        let mut subs: Vec<&str> = vec![host_sub];
        for sub in ["Contents/x86_64-linux", "Contents/aarch64-linux", "Contents/i386-linux"] {
            if sub != host_sub {
                subs.push(sub);
            }
        }
        for sub in subs {
            let dir = p.join(sub);
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let cand = e.path();
                    if cand.extension().and_then(|x| x.to_str()) == Some("so")
                        && crate::host::arch::elf_matches_host(&cand)
                    {
                        // vst3-host wants the bundle path, not the .so — keep bundle.
                        return path.to_string();
                    }
                }
            }
        }
    }
    path.to_string()
}

fn midi_channel(ch: u8) -> MidiChannel {
    match ch & 0x0f {
        0 => MidiChannel::Ch1,
        1 => MidiChannel::Ch2,
        2 => MidiChannel::Ch3,
        3 => MidiChannel::Ch4,
        4 => MidiChannel::Ch5,
        5 => MidiChannel::Ch6,
        6 => MidiChannel::Ch7,
        7 => MidiChannel::Ch8,
        8 => MidiChannel::Ch9,
        9 => MidiChannel::Ch10,
        10 => MidiChannel::Ch11,
        11 => MidiChannel::Ch12,
        12 => MidiChannel::Ch13,
        13 => MidiChannel::Ch14,
        14 => MidiChannel::Ch15,
        _ => MidiChannel::Ch16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_module_errors() {
        assert!(Vst3Instance::load("/no/such.vst3", "x", None, 48000, 256).is_err());
    }
}
