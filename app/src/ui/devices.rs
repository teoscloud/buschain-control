use crate::app_state::AppState;
use crate::audio::graph::DeviceNode;
use crate::audio::worker::Command;
use crate::design::{self, Theme};
use crate::session::DeviceClockConfig;
use egui::RichText;
use buschain_engine::{resolve_profile, AudioPreset};

/// Per-device rate/quantum editor (Output / Input / Settings Master HW).
pub fn draw_device_clock_panel(
    ui: &mut egui::Ui,
    state: &mut AppState,
    device: &DeviceNode,
    is_source: bool,
) {
    let theme = state.theme;
    let is_master_hw = !is_source
        && state.session.master_output.as_deref() == Some(device.name.as_str());

    // Caps are TTL-cached on AppState — never pw-dump/pactl every frame.
    let caps = state.caps_for_device(&device.name, &device.description, is_source);
    let live = device
        .sample_rate
        .or(caps.preferred_rate.nonzero())
        .unwrap_or(48_000);

    // Display-only defaults — do not write device_clocks until the user edits
    // or clicks Apply (avoids saving every sink ever opened on Output).
    let stored = state.session.device_clocks.get(&device.name).cloned();
    let soft_pref = stored.as_ref().map(|c| c.soft_quantum).unwrap_or(true);
    let resolved_pref = resolve_profile(
        AudioPreset::Custom,
        &caps,
        Some(stored.as_ref().map(|c| c.sample_rate).unwrap_or(live)),
        Some(
            stored
                .as_ref()
                .map(|c| c.quantum)
                .unwrap_or_else(|| state.session.performance.quantum.max(64)),
        ),
        soft_pref,
    );

    ui.add_space(4.0);
    ui.separator();
    ui.label(
        RichText::new(if is_master_hw {
            "Clock (Master HW out — Apply also binds BusChain graph)"
        } else {
            "Clock (PipeWire graph force-rate; shared across the card)"
        })
        .size(11.0)
        .color(theme.text_dim()),
    );
    ui.label(
        RichText::new(format!(
            "Live: {} Hz · Device rates: {}",
            live,
            if caps.rates.is_empty() {
                "—".into()
            } else {
                caps.rates
                    .iter()
                    .map(|r| r.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ))
        .size(10.0)
        .color(if device.sample_rate == Some(resolved_pref.sample_rate) {
            theme.accent()
        } else {
            theme.warning()
        }),
    );

    ui.horizontal(|ui| {
        ui.label(RichText::new("Rate").size(11.0).color(theme.text_dim()));
        let rates = if caps.rates.is_empty() {
            vec![live]
        } else {
            caps.rates.clone()
        };
        let mut rate = resolved_pref.sample_rate;
        if !rates.contains(&rate) {
            rate = rates[0];
        }
        let rate_before = rate;
        egui::ComboBox::from_id_salt(format!("dev_rate_{}", device.name))
            .selected_text(format!("{rate}"))
            .show_ui(ui, |ui| {
                for r in &rates {
                    ui.selectable_value(&mut rate, *r, format!("{r}"));
                }
            });

        ui.label(RichText::new("Quantum").size(11.0).color(theme.text_dim()));
        let mut q = resolved_pref.quantum;
        let q_before = q;
        egui::ComboBox::from_id_salt(format!("dev_q_{}", device.name))
            .selected_text(format!("{q}"))
            .show_ui(ui, |ui| {
                for qq in [64, 128, 256, 512, 1024, 2048] {
                    if caps.allows_quantum(qq) {
                        ui.selectable_value(&mut q, qq, format!("{qq}"));
                    }
                }
            });

        if rate != rate_before || q != q_before {
            let entry = state
                .session
                .device_clocks
                .entry(device.name.clone())
                .or_insert_with(|| DeviceClockConfig {
                    sample_rate: rate,
                    quantum: q,
                    soft_quantum: soft_pref,
                });
            entry.sample_rate = rate;
            entry.quantum = q;
            state.dirty = true;
        }
    });

    let mut soft = soft_pref;
    if ui
        .checkbox(&mut soft, "Soft quantum (PipeWire may raise period under load)")
        .changed()
    {
        let entry = state
            .session
            .device_clocks
            .entry(device.name.clone())
            .or_insert_with(|| DeviceClockConfig {
                sample_rate: resolved_pref.sample_rate,
                quantum: resolved_pref.quantum,
                soft_quantum: soft,
            });
        entry.soft_quantum = soft;
        state.dirty = true;
    }

    ui.horizontal(|ui| {
        let label = if is_master_hw {
            "Apply clock + BusChain"
        } else {
            "Apply device clock"
        };
        if design::button(ui, &theme, label, true).clicked() {
            // Ensure a prefs row exists before Apply (may be first touch).
            state
                .session
                .device_clocks
                .entry(device.name.clone())
                .or_insert_with(|| DeviceClockConfig {
                    sample_rate: resolved_pref.sample_rate,
                    quantum: resolved_pref.quantum,
                    soft_quantum: soft_pref,
                });
            state.apply_device_clock(&device.name, is_source);
        }
        let period = {
            let sr = resolved_pref.sample_rate;
            if sr > 0 {
                (resolved_pref.quantum as f32 * 1000.0) / sr as f32
            } else {
                0.0
            }
        };
        ui.label(
            RichText::new(format!("period {period:.2} ms"))
                .size(10.0)
                .color(theme.text_muted()),
        );
    });
}

trait NonZeroRate {
    fn nonzero(self) -> Option<u32>;
}
impl NonZeroRate for u32 {
    fn nonzero(self) -> Option<u32> {
        if self > 0 {
            Some(self)
        } else {
            None
        }
    }
}

pub fn draw_output_devices(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    design::page_header(
        ui,
        &theme,
        "Output",
        "Pick Master hardware (speakers/headphones) and the system default sink. BusChain tracks are not hardware — they feed into Master.",
    );

    // Header uses the same resolver truth as the audio path.
    let hw_label = state
        .session
        .master_output
        .as_ref()
        .map(|name| {
            let desc = state
                .session
                .master_output_desc
                .as_deref()
                .filter(|d| !d.is_empty())
                .unwrap_or(name.as_str());
            format!("Master HW → {desc}")
        })
        .unwrap_or_else(|| "Master HW → (none)".into());
    ui.label(
        RichText::new(hw_label)
            .size(11.0)
            .color(theme.accent()),
    );
    if let Some(name) = &state.session.master_output {
        ui.label(
            RichText::new(name)
                .size(10.0)
                .monospace()
                .color(theme.text_muted()),
        );
    }
    let pref = state.session.preferred_default_sink.clone();
    let live_def = state.snapshot.default_sink.clone();
    let shown_def = pref.clone().or_else(|| live_def.clone());
    if let Some(def) = &shown_def {
        let stick = match (&pref, &live_def) {
            (Some(p), Some(l)) if p == l => "ok",
            (Some(_), Some(_)) => "applying…",
            (Some(_), None) => "applying…",
            _ => "",
        };
        ui.label(
            RichText::new(format!("System default: {def} {stick}"))
                .size(11.0)
                .color(theme.text_dim()),
        );
    }
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.checkbox(&mut state.show_hidden_output, "Show hidden");
        ui.label(
            RichText::new("helpers: buschain_fx / post / hold")
                .size(10.0)
                .color(theme.text_muted()),
        );
    });
    ui.add_space(4.0);

    let active_hw = state.session.master_output.clone();
    // Prefer session preference (set on click) so highlight updates this frame;
    // live snapshot can lag a few seconds behind pactl.
    let active_default = state
        .session
        .preferred_default_sink
        .clone()
        .or_else(|| state.snapshot.default_sink.clone());

    egui::ScrollArea::vertical().show(ui, |ui| {
        let mut sinks: Vec<_> = state
            .snapshot
            .sinks
            .iter()
            .filter(|s| {
                if state.show_hidden_output {
                    return true;
                }
                // Always hide helpers + pre-rebrand Shadow Audio leftovers.
                if s.name.starts_with("buschain_fx_")
                    || s.name.starts_with("buschain_post_")
                    || s.name.starts_with("buschain_mid_")
                    || s.name.starts_with("buschain_rs_")
                    || s.name.starts_with("buschain_vinf_")
                    || s.name == "buschain_hold"
                    || s.name.starts_with("shadow_")
                    || s.description.starts_with("ShadowAudio_")
                {
                    return false;
                }
                if s.name.starts_with("buschain_track_") {
                    return state.session.tracks.iter().any(|t| {
                        t.expected_sink_name() == s.name && t.virtual_output
                    });
                }
                true
            })
            .cloned()
            .collect();
        // Session-owned virtual buses may exist in PW before the next snapshot
        // lands — synthesize Output rows so toggles / +Track feel instant.
        for t in &state.session.tracks {
            let show = t.kind.is_master() || t.virtual_output;
            if !show {
                continue;
            }
            let name = t.expected_sink_name();
            if sinks.iter().any(|s| s.name == name) {
                continue;
            }
            sinks.push(DeviceNode {
                index: 0,
                name,
                description: format!("BusChainControl_{}", t.name.replace(' ', "_")),
                volume_pct: 100,
                mute: false,
                sample_rate: None,
            });
        }
        for sink in sinks {
            let is_app_bus = sink.name == "buschain_master"
                || sink.name.starts_with("buschain_track_");
            let is_shadow = sink.name.starts_with("buschain_")
                || sink.name.starts_with("shadow_");
            let is_active_hw = active_hw.as_deref() == Some(sink.name.as_str());
            let is_active_def = active_default.as_deref() == Some(sink.name.as_str());
            // Prefer session track name — PW keeps the create-time description
            // (e.g. Track_1) until a rename push lands.
            let label = state
                .session
                .tracks
                .iter()
                .find(|t| t.expected_sink_name() == sink.name)
                .map(|t| {
                    if t.kind.is_master() {
                        "BusChainControl_Master".into()
                    } else {
                        format!("BusChainControl_{}", t.name.replace(' ', "_"))
                    }
                })
                .unwrap_or_else(|| sink.description.clone());

            design::panel(ui, &theme, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(&label)
                                .size(13.0)
                                .strong()
                                .color(theme.text()),
                        );
                        ui.label(
                            RichText::new(&sink.name)
                                .size(10.0)
                                .monospace()
                                .color(theme.text_muted()),
                        );
                        if is_app_bus {
                            ui.label(
                                RichText::new("Mixer-owned bus — use strip fader / mute")
                                    .size(10.0)
                                    .color(theme.warning()),
                            );
                        } else if is_shadow {
                            ui.label(
                                RichText::new("BusChain helper (not a hardware device)")
                                    .size(10.0)
                                    .color(theme.warning()),
                            );
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !is_shadow {
                            let hw_btn = design::button(
                                ui,
                                &theme,
                                "Master HW out",
                                is_active_hw,
                            )
                            .on_hover_text(
                                "Master bus plays to this device — applied live",
                            );
                            if hw_btn.clicked() {
                                state.session.master_output = Some(sink.name.clone());
                                state.session.master_output_desc =
                                    Some(sink.description.clone());
                                let _ = state.session.save();
                                // Light relink only — clock bind is explicit Apply.
                                state.worker.send(Command::SetMasterHw {
                                    name: sink.name.clone(),
                                    desc: Some(sink.description.clone()),
                                });
                                state.status = format!("Master HW out → {}", sink.description);
                            }
                        }
                        let is_fx = sink.name.starts_with("buschain_fx_")
                            || sink.name.starts_with("buschain_post_")
                            || sink.name.starts_with("buschain_rs_")
                            || sink.name == "buschain_hold"
                            || sink.name.starts_with("shadow_");
                        let is_hidden_track = sink.name.starts_with("buschain_track_")
                            && !state.session.tracks.iter().any(|t| {
                                t.expected_sink_name() == sink.name && t.virtual_output
                            });
                        let can_default = !is_fx && !is_hidden_track;
                        if can_default {
                            let def_btn = design::button(
                                ui,
                                &theme,
                                "System default",
                                is_active_def,
                            )
                            .on_hover_text(
                                "Apps open onto this sink. Prefer a BusChain track bus \
                                 or Master (not raw HW). Watchdog reasserts if WirePlumber fights.",
                            );
                            if def_btn.clicked() {
                                if sink.name.starts_with("buschain_track_") {
                                    if let Some(t) = state
                                        .session
                                        .tracks
                                        .iter_mut()
                                        .find(|t| t.expected_sink_name() == sink.name)
                                    {
                                        t.virtual_output = true;
                                    }
                                }
                                state.session.preferred_default_sink = Some(sink.name.clone());
                                // Optimistic UI — don't wait for the slow snapshot poll.
                                state.snapshot.default_sink = Some(sink.name.clone());
                                state
                                    .worker
                                    .send(Command::SetDefaultSink(sink.name.clone()));
                                let _ = state.session.save();
                                state.status =
                                    format!("System default → {}", sink.description);
                            }
                        }
                        if !is_app_bus {
                            let sink_name = sink.name.clone();
                            let mut mute = sink.mute;
                            if design::toggle_chip(ui, &theme, "Mute", &mut mute, theme.danger())
                                .changed()
                            {
                                if is_active_hw {
                                    let _ = crate::ipc::Client::call_fast(
                                        &crate::ipc::Request::SetHwMute { mute },
                                    )
                                    .or_else(|_| {
                                        crate::ipc::Client::call(&crate::ipc::Request::SetHwMute {
                                            mute,
                                        })
                                    });
                                } else {
                                    state.worker.send(Command::SetSinkMute {
                                        name: sink_name.clone(),
                                        mute,
                                    });
                                }
                                if let Some(s) = state
                                    .snapshot
                                    .sinks
                                    .iter_mut()
                                    .find(|s| s.name == sink_name)
                                {
                                    s.mute = mute;
                                }
                            }
                        }
                    });
                });
                if is_app_bus {
                    ui.label(
                        RichText::new(format!(
                            "Level from mixer · sink {}%{}",
                            sink.volume_pct,
                            if sink.mute { " (opening…)" } else { "" }
                        ))
                        .size(11.0)
                        .color(theme.text_dim()),
                    );
                } else {
                    // Hardware sinks: hard-cap at 100% (apps/tracks may boost).
                    // Master HW shares waybar/GTK `hw-vol` — never a second writer.
                    let sink_name = sink.name.clone();
                    let mut vol = (sink.volume_pct as f32).min(100.0);
                    if design::h_slider(ui, &theme, &mut vol, 0.0..=100.0, "Volume")
                        .changed()
                    {
                        let pct = vol.clamp(0.0, 100.0) as u32;
                        if is_active_hw {
                            let _ = crate::ipc::Client::call_fast(
                                &crate::ipc::Request::SetHwVolume { pct },
                            )
                            .or_else(|_| {
                                crate::ipc::Client::call(&crate::ipc::Request::SetHwVolume { pct })
                            });
                        } else {
                            state.worker.send(Command::SetSinkVolume {
                                name: sink_name.clone(),
                                pct,
                            });
                        }
                        if let Some(s) = state
                            .snapshot
                            .sinks
                            .iter_mut()
                            .find(|s| s.name == sink_name)
                        {
                            s.volume_pct = pct;
                        }
                    }
                }
                if !is_shadow {
                    draw_device_clock_panel(ui, state, &sink, false);
                }
            });
            ui.add_space(6.0);
        }
    });
}

pub fn draw_input_devices(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    design::page_header(
        ui,
        &theme,
        "Input",
        "Microphones and capture devices. Set levels, mute, and the system default source.",
    );
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Sources")
                .size(12.0)
                .strong()
                .color(theme.text_dim()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.checkbox(&mut state.show_hidden_recording, "Show internals");
        });
    });
    ui.add_space(4.0);

    egui::ScrollArea::vertical().show(ui, |ui| {
        let sources: Vec<_> = state
            .snapshot
            .sources
            .iter()
            .filter(|s| {
                if state.show_hidden_recording {
                    return true;
                }
                // Show app-facing virtual mics; hide feed/helpers.
                if s.name.starts_with("buschain_vin_") {
                    return state.session.tracks.iter().any(|t| {
                        t.virtual_input && t.expected_virtual_input_name() == s.name
                    });
                }
                !s.name.starts_with("buschain_")
            })
            .cloned()
            .collect();
        for src in sources {
            design::panel(ui, &theme, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(&src.description)
                                .size(13.0)
                                .strong()
                                .color(theme.text()),
                        );
                        ui.label(
                            RichText::new(&src.name)
                                .size(10.0)
                                .monospace()
                                .color(theme.text_muted()),
                        );
                    });
                    let is_def_src = state.snapshot.default_source.as_deref()
                        == Some(src.name.as_str());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if design::button(ui, &theme, "System default", is_def_src).clicked() {
                            state.snapshot.default_source = Some(src.name.clone());
                            state
                                .worker
                                .send(Command::SetDefaultSource(src.name.clone()));
                            state.status =
                                format!("System default source → {}", src.description);
                        }
                        let mut mute = src.mute;
                        if design::toggle_chip(ui, &theme, "Mute", &mut mute, theme.danger())
                            .changed()
                        {
                            state.worker.send(Command::SetSourceMute {
                                name: src.name.clone(),
                                mute,
                            });
                        }
                    });
                });
                let mut vol = (src.volume_pct as f32).min(100.0);
                if design::h_slider(ui, &theme, &mut vol, 0.0..=100.0, "Volume")
                    .changed()
                {
                    state.worker.send(Command::SetSourceVolume {
                        name: src.name.clone(),
                        pct: vol.clamp(0.0, 100.0) as u32,
                    });
                }
                if !src.name.starts_with("buschain_") {
                    draw_device_clock_panel(ui, state, &src, true);
                }
            });
            ui.add_space(6.0);
        }
    });
}
