//! Immutable rack generation — ordered slots, fingerprint, process chain.

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use uuid::Uuid;

use crate::domain::{inserts_signature, normalize_ladspa_label, InsertFormat, InsertSlot};

use super::clap::{self, ClapInstance};
use super::control::ControlMsg;
use super::ladspa::load_instance;
use super::lv2::{self, Lv2Instance};
use super::processor::{AudioProcessor, MidiEvent};
use super::remote::RemoteProcessor;
use super::slot::Slot;
use super::vst3::{self, Vst3Instance};

static RACK_GEN: AtomicU64 = AtomicU64::new(1);

/// One published generation of the insert rack.
pub struct Rack {
    pub generation: u64,
    pub fingerprint: String,
    pub slots: Vec<Slot>,
    pub sample_rate: u32,
    pub max_block: u32,
}

impl Rack {
    pub fn build(
        inserts: &[InsertSlot],
        sample_rate: u32,
        max_block: u32,
        ladspa_path: &str,
    ) -> Result<Self> {
        let fingerprint = inserts_signature(inserts);
        let mut slots = Vec::with_capacity(inserts.len());
        for ins in inserts {
            let mut proc: Box<dyn AudioProcessor> = match ins.format {
                InsertFormat::Ladspa => {
                    let label = normalize_ladspa_label(&ins.plugin_key);
                    let mut p = load_instance(&ins.plugin_so, &label, sample_rate, ladspa_path)
                        .with_context(|| format!("load {} ({})", ins.plugin_key, ins.plugin_so))?;
                    for (name, val) in &ins.controls {
                        let lname = name.to_ascii_lowercase();
                        if lname == "bypass" || lname == "enable" {
                            continue;
                        }
                        let _ = p.set_control_by_name(name, *val);
                    }
                    Box::new(p)
                }
                InsertFormat::Clap => {
                    let mut p = ClapInstance::load(
                        &ins.plugin_so,
                        &ins.plugin_key,
                        ins.state_blob.clone(),
                        sample_rate,
                        max_block,
                    )
                    .with_context(|| format!("CLAP {}", ins.plugin_key))?;
                    clap::apply_controls(&mut p, &ins.controls);
                    Box::new(p)
                }
                InsertFormat::Vst3 => {
                    let mut p = Vst3Instance::load(
                        &ins.plugin_so,
                        &ins.plugin_key,
                        ins.state_blob.clone(),
                        sample_rate,
                        max_block,
                    )
                    .with_context(|| format!("VST3 {}", ins.plugin_key))?;
                    vst3::apply_controls(&mut p, &ins.controls);
                    Box::new(p)
                }
                InsertFormat::Lv2 => {
                    let mut p = Lv2Instance::load(
                        &ins.plugin_key,
                        &ins.plugin_so,
                        sample_rate,
                        max_block,
                    )
                    .with_context(|| format!("LV2 {}", ins.plugin_key))?;
                    lv2::apply_controls(&mut p, &ins.controls);
                    Box::new(p)
                }
            };
            // P6 sandbox OR VST3 surface promotion (DSP+GUI in buschain-plugin-surface).
            let want_surface = matches!(ins.format, InsertFormat::Vst3)
                && super::surface::slot_wants_surface(ins.slot_id);
            if want_surface || sandbox_requested(&ins.plugin_key) {
                let fmt = match ins.format {
                    InsertFormat::Ladspa => "ladspa",
                    InsertFormat::Lv2 => "lv2",
                    InsertFormat::Clap => "clap",
                    InsertFormat::Vst3 => "vst3",
                };
                let lat = proc.latency_samples();
                // Short bus token for logging / RemoteProcessor identity — never the
                // full VST3 filesystem path (that blew Unix socket SUN_LEN).
                let bus_token = format!("s{}", &ins.slot_id.as_simple().to_string()[..8]);
                let mut remote = if want_surface {
                    RemoteProcessor::spawn_for_surface(
                        &bus_token,
                        ins.slot_id,
                        &ins.plugin_key,
                        &ins.plugin_so,
                        fmt,
                        ins.state_blob.clone(),
                        sample_rate,
                        max_block,
                    )
                } else {
                    RemoteProcessor::spawn_for_plugin(
                        &bus_token,
                        &ins.plugin_key,
                        &ins.plugin_so,
                        fmt,
                        sample_rate,
                        max_block,
                    )
                };
                remote.set_latency(lat);
                proc = Box::new(remote);
            }

            let key = match ins.format {
                InsertFormat::Ladspa => normalize_ladspa_label(&ins.plugin_key).to_string(),
                _ => ins.plugin_key.clone(),
            };
            let bypassed = insert_bypassed(ins);
            let mut slot = Slot::new(ins.slot_id, key, proc);
            slot.sidechain_from = ins.sidechain_from.clone();
            slot.prepare(sample_rate, max_block);
            slot.set_bypassed(bypassed);
            if bypassed {
                slot.fade.snap(true);
            }
            slots.push(slot);
        }
        Ok(Self {
            generation: RACK_GEN.fetch_add(1, Ordering::Relaxed),
            fingerprint,
            slots,
            sample_rate,
            max_block,
        })
    }

    pub fn total_latency_samples(&self) -> u32 {
        self.slots.iter().map(|s| s.latency_samples()).sum()
    }

    pub fn slot_index(&self, slot_id: Uuid) -> Option<usize> {
        self.slots.iter().position(|s| s.id.0 == slot_id)
    }

    /// Apply drained control messages (call at block start, RT).
    pub fn apply_controls(&mut self, msgs: &[ControlMsg]) {
        for msg in msgs {
            match *msg {
                ControlMsg::Param {
                    slot_id,
                    control_index,
                    value,
                    ..
                } => {
                    if let Some(idx) = self.slot_index(slot_id) {
                        self.slots[idx]
                            .processor
                            .set_control(control_index as usize, value);
                    }
                }
                ControlMsg::Bypass {
                    slot_id, bypassed, ..
                } => {
                    if let Some(idx) = self.slot_index(slot_id) {
                        self.slots[idx].set_bypassed(bypassed);
                        // Host fade alone is not enough for plugins with their own
                        // Bypass/Enable ports (e.g. Pitch). Props historically skipped
                        // named bypass pushes; after rebuild-while-off the port stayed
                        // at 1.0 and power-on sounded like a multi-second no-op.
                        sync_power_ports(&mut self.slots[idx], bypassed);
                    }
                }
            }
        }
    }

    /// M2: forward MIDI events to each slot processor (RT).
    pub fn feed_midi(&mut self, events: &[MidiEvent]) {
        if events.is_empty() {
            return;
        }
        for slot in &mut self.slots {
            slot.processor.feed_midi(events);
        }
    }

    /// RT process — planar stereo, equal lengths.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for slot in &mut self.slots {
            slot.process(left, right);
        }
    }

    /// Offline render helper (freeze/bounce) — not RT.
    pub fn render_offline(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.process(left, right);
    }
}

fn sync_power_ports(slot: &mut Slot, bypassed: bool) {
    let n = slot.processor.control_count();
    for ci in 0..n {
        let Some(name) = slot.processor.control_name(ci) else {
            continue;
        };
        if name.eq_ignore_ascii_case("bypass") {
            slot.processor
                .set_control(ci, if bypassed { 1.0 } else { 0.0 });
        } else if name.eq_ignore_ascii_case("enable") {
            slot.processor
                .set_control(ci, if bypassed { 0.0 } else { 1.0 });
        }
    }
}

fn insert_bypassed(ins: &InsertSlot) -> bool {
    for (name, val) in &ins.controls {
        let n = name.to_ascii_lowercase();
        if n == "bypass" {
            return *val >= 0.5;
        }
        if n == "enable" {
            return *val < 0.5;
        }
    }
    false
}

fn sandbox_requested(key: &str) -> bool {
    if std::env::var_os("BUSCHAIN_SANDBOX_ALL").is_some() {
        return true;
    }
    let Ok(list) = std::env::var("BUSCHAIN_SANDBOX_PLUGINS") else {
        return false;
    };
    list.split(',').any(|s| s.trim() == key)
}
