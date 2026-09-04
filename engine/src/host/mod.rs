//! In-process insert DSP host — RT-safe rack, LADSPA/LV2/CLAP/VST3, PipeWire filter node.

pub mod arch;
mod clap;
mod control;
mod denormal;
mod fade;
mod freeze;
mod ladspa;
mod lv2;
mod pdc;
mod processor;
mod rack;
mod remote;
mod slot;
mod spectrum;
mod surface;
mod ui_bridge;
mod vst3;

pub mod dry_meter;
pub mod node;
pub mod node_latency;
pub mod orphan;
pub mod registry;

pub use spectrum::{SpectrumFrame, SpectrumBus, ANALYSIS_N, FFT_N, HOP, MAG_N};

pub use arch::{
    elf_machine, elf_matches_host, host_arch_label, host_elf_machine, host_vst3_contents_subdir,
    vst3_bundle_has_host_binary,
};
pub use clap::{has_clap_entry, probe_clap_params, ClapInstance, ClapParamInfo};
pub use control::{ControlMsg, ControlQueue};
pub use denormal::DenormalGuard;
pub use fade::{mix_sample, BypassFade, FADE_LEN};
pub use freeze::{bounce_rack, FreezeBuffer};
pub use ladspa::{load_instance, resolve_plugin_so, LadspaInstance, LadspaLibrary};
pub use lv2::{probe_lv2_params, Lv2Instance, Lv2ParamInfo};
pub use pdc::{compensation_samples, refresh_peer_pads, reported_latency};
pub use processor::{AudioProcessor, MidiEvent, Passthrough};
pub use rack::Rack;
pub use remote::{surface_ctrl_ready, surface_forget_ctrl, surface_send_ctrl, RemoteProcessor};
pub use slot::{Slot, SlotId};
pub use surface::{
    clear_surface_slot, mark_surface_slot, slot_wants_surface, surface_enabled,
};
pub use ui_bridge::{
    close_editor, drain_editor_closed_events, drain_editor_param_events, editor_event_to_control,
    ensure_editor_ipc,
    request_open_editor, EditorParamEvent, OpenEditorRequest,
};
pub use registry::{harvest_host_slot_states, SlotStateSnapshot};
pub use vst3::{probe_vst3_params, Vst3Instance, Vst3ParamInfo};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::InsertSlot;
    use std::sync::Arc;
    use std::thread;
    use uuid::Uuid;

    struct Gain {
        g: f32,
    }

    impl AudioProcessor for Gain {
        fn prepare(&mut self, _: u32, _: u32) {}
        fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                *l *= self.g;
                *r *= self.g;
            }
        }
        fn set_control(&mut self, index: usize, value: f32) {
            if index == 0 {
                self.g = value;
            }
        }
        fn control_count(&self) -> usize {
            1
        }
        fn control_name(&self, index: usize) -> Option<&str> {
            if index == 0 {
                Some("Gain")
            } else {
                None
            }
        }
    }

    #[test]
    fn control_queue_spsc() {
        let q = ControlQueue::new();
        let id = Uuid::new_v4();
        q.push(ControlMsg::param(id, 0, 0.5));
        q.push(ControlMsg::bypass(id, true));
        let mut out = Vec::new();
        q.drain_into(&mut out);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn bypass_fade_ramps() {
        let mut f = BypassFade::default();
        assert!(f.is_idle());
        f.set_bypassed(true);
        assert!(!f.is_idle());
        let mut last = 1.0;
        for _ in 0..FADE_LEN + 4 {
            last = f.next();
        }
        assert!(last < 1.0e-5);
        assert!(f.is_idle());
    }

    #[test]
    fn rack_process_gain_and_param() {
        let id = Uuid::new_v4();
        let mut slot = Slot::new(id, "gain".into(), Box::new(Gain { g: 0.5 }));
        slot.prepare(48_000, 64);
        let mut l = vec![1.0f32; 32];
        let mut r = vec![1.0f32; 32];
        slot.process(&mut l, &mut r);
        assert!((l[0] - 0.5).abs() < 1e-6);

        let mut rack = Rack {
            generation: 1,
            fingerprint: "t".into(),
            slots: vec![slot],
            sample_rate: 48_000,
            max_block: 64,
        };
        rack.apply_controls(&[ControlMsg::param(id, 0, 2.0)]);
        l.fill(1.0);
        r.fill(1.0);
        rack.process(&mut l, &mut r);
        assert!((l[0] - 2.0).abs() < 1e-6);
    }

    /// Plugin Bypass port must track ControlMsg::bypass (Pitch-style power).
    #[test]
    fn bypass_msg_syncs_plugin_power_port() {
        struct WithBypass {
            bypass: f32,
        }
        impl AudioProcessor for WithBypass {
            fn prepare(&mut self, _: u32, _: u32) {}
            fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
                if self.bypass >= 0.5 {
                    return;
                }
                for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                    *l *= 2.0;
                    *r *= 2.0;
                }
            }
            fn set_control(&mut self, index: usize, value: f32) {
                if index == 0 {
                    self.bypass = value;
                }
            }
            fn control_count(&self) -> usize {
                1
            }
            fn control_name(&self, index: usize) -> Option<&str> {
                if index == 0 {
                    Some("Bypass")
                } else {
                    None
                }
            }
            fn control_value(&self, index: usize) -> Option<f32> {
                if index == 0 {
                    Some(self.bypass)
                } else {
                    None
                }
            }
        }

        let id = Uuid::new_v4();
        let mut slot = Slot::new(id, "pitch".into(), Box::new(WithBypass { bypass: 1.0 }));
        slot.prepare(48_000, 64);
        // Simulate rebuild-while-off: host wet, plugin port still bypassed.
        slot.set_bypassed(false);
        slot.fade.snap(false);
        let mut rack = Rack {
            generation: 1,
            fingerprint: "t".into(),
            slots: vec![slot],
            sample_rate: 48_000,
            max_block: 64,
        };
        let mut l = vec![1.0f32; 8];
        let mut r = vec![1.0f32; 8];
        rack.process(&mut l, &mut r);
        assert!((l[0] - 1.0).abs() < 1e-6, "stuck Bypass=1 keeps dry");

        rack.apply_controls(&[ControlMsg::bypass(id, false)]);
        l.fill(1.0);
        r.fill(1.0);
        rack.process(&mut l, &mut r);
        assert!((l[0] - 2.0).abs() < 1e-6, "power-on must clear plugin Bypass");
        assert!((rack.slots[0].processor.control_value(0).unwrap_or(1.0) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn gen_swap_under_mock_rt() {
        let q = Arc::new(ControlQueue::new());
        let id = Uuid::new_v4();
        let rack_a = Arc::new(std::sync::Mutex::new(Rack {
            generation: 1,
            fingerprint: "a".into(),
            slots: vec![Slot::new(id, "g".into(), Box::new(Gain { g: 1.0 }))],
            sample_rate: 48_000,
            max_block: 64,
        }));
        {
            let mut g = rack_a.lock().unwrap();
            g.slots[0].prepare(48_000, 64);
        }
        let current: Arc<std::sync::Mutex<Option<Arc<std::sync::Mutex<Rack>>>>> =
            Arc::new(std::sync::Mutex::new(Some(rack_a.clone())));

        let cur_rt = current.clone();
        let q_rt = q.clone();
        let rt = thread::spawn(move || {
            let mut drain = Vec::new();
            for _ in 0..20 {
                drain.clear();
                q_rt.drain_into(&mut drain);
                let guard = cur_rt.lock().unwrap();
                if let Some(r) = guard.as_ref() {
                    let mut rack = r.lock().unwrap();
                    rack.apply_controls(&drain);
                    let mut l = [0.1f32; 16];
                    let mut rbuf = [0.1f32; 16];
                    let _d = DenormalGuard::enter();
                    rack.process(&mut l, &mut rbuf);
                }
            }
        });

        // Publish new generation off-thread.
        let rack_b = Arc::new(std::sync::Mutex::new(Rack {
            generation: 2,
            fingerprint: "b".into(),
            slots: vec![Slot::new(id, "g".into(), Box::new(Gain { g: 0.25 }))],
            sample_rate: 48_000,
            max_block: 64,
        }));
        {
            rack_b.lock().unwrap().slots[0].prepare(48_000, 64);
        }
        *current.lock().unwrap() = Some(rack_b);
        q.push(ControlMsg::param(id, 0, 0.5));
        rt.join().unwrap();
    }

    #[test]
    fn ladspa_builtins_offline_if_present() {
        let path = crate::backend::ladspa_search_path();
        let so = match resolve_plugin_so("buschain_builtins", &path) {
            Some(p) => p,
            None => return, // plugins not built in this env
        };
        let lib = LadspaLibrary::open(&so).expect("open builtins");
        let inst = LadspaInstance::instantiate(lib, "buschain_gain", 48_000)
            .or_else(|_| {
                // Try common labels from catalog
                for label in [
                    "buschain_gain",
                    "bc_gain",
                    "shadow_gain",
                    "buschain_equalizer",
                    "buschain_eq8",
                    "buschain_eq",
                ] {
                    if let Ok(i) = LadspaInstance::instantiate(
                        LadspaLibrary::open(&so).unwrap(),
                        label,
                        48_000,
                    ) {
                        return Ok(i);
                    }
                }
                Err(anyhow::anyhow!("no known builtin label"))
            });
        let Ok(mut inst) = inst else {
            return;
        };
        inst.prepare(48_000, 128);
        let mut l = vec![0.25f32; 64];
        let mut r = vec![0.25f32; 64];
        let _d = DenormalGuard::enter();
        inst.process(&mut l, &mut r);
        // Should not explode / NaN
        assert!(l.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn build_rack_from_empty_inserts() {
        let inserts: Vec<InsertSlot> = vec![];
        let rack = Rack::build(&inserts, 48_000, 256, "").unwrap();
        assert!(rack.slots.is_empty());
        assert_eq!(rack.total_latency_samples(), 0);
    }

    #[test]
    fn any_gen_live_prefers_host_not_pulse_fx_sink() {
        // PwFxNode names look like Pulse sinks but are duplex filters — wetness
        // must come from the host registry, not sink_exists alone.
        let bus = "buschain_track_1";
        assert!(!crate::fx_gen::any_gen_live(bus));
        assert_eq!(crate::fx_gen::live_fx_name(bus), "buschain_fx_1");
    }
}
