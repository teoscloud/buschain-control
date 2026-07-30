//! MIDI control-surface tab — devices, route matrix, learn mode.

use crate::app_state::AppState;
use crate::audio::worker::Command;
use crate::design::{self, Theme};
use buschain_engine::{MidiIntent, MidiMapTarget, MidiRoute, MidiRouteTarget};
use egui::RichText;

pub fn draw_midi(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    design::page_header(
        ui,
        &theme,
        "MIDI",
        "Controllers → BusChain. Map CCs to insert params, or learn a knob. WirePlumber still owns the hardware links.",
    );
    ui.horizontal(|ui| {
        if design::button(ui, &theme, "Refresh devices", false).clicked() {
            state.worker.send(Command::RefreshMidi);
        }
        if state.midi_learn.is_some() {
            if design::button(ui, &theme, "Cancel learn", true).clicked() {
                state.midi_learn = None;
                state.worker.send(Command::Midi(MidiIntent::StopLearn));
                state.status = "MIDI learn cancelled".into();
            }
        }
    });
    ui.add_space(8.0);

    ui.columns(2, |cols| {
        draw_device_list(&mut cols[0], state);
        draw_route_matrix(&mut cols[1], state);
    });

    ui.add_space(10.0);
    draw_maps_panel(ui, state);
}

fn draw_device_list(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    design::panel(ui, &theme, |ui| {
        ui.label(
            RichText::new("DEVICES")
                .size(11.0)
                .strong()
                .color(theme.text_muted()),
        );
        ui.add_space(4.0);
        if state.midi_snapshot.devices.is_empty() {
            ui.label(
                RichText::new("No MIDI nodes found — click Refresh")
                    .size(11.0)
                    .color(theme.text_dim()),
            );
            return;
        }
        let devices = state.midi_snapshot.devices.clone();
        egui::ScrollArea::vertical()
            .id_salt("midi_devices")
            .max_height(280.0)
            .show(ui, |ui| {
                for dev in &devices {
                    let mut enabled = state
                        .session
                        .midi_devices
                        .iter()
                        .find(|d| d.id == dev.id)
                        .map(|d| d.enabled)
                        .unwrap_or(true);
                    ui.horizontal(|ui| {
                        draw_activity_led(ui, &theme, dev.activity);
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(&dev.description)
                                    .size(12.0)
                                    .color(theme.text()),
                            );
                            ui.label(
                                RichText::new(&dev.id)
                                    .size(10.0)
                                    .color(theme.text_dim()),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.checkbox(&mut enabled, "Listen").changed() {
                                upsert_device_enabled(state, &dev.id, &dev.description, enabled);
                                state.worker.send(Command::Midi(MidiIntent::SetDeviceEnabled {
                                    device_id: dev.id.clone(),
                                    enabled,
                                }));
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
            });
    });
}

fn draw_activity_led(ui: &mut egui::Ui, theme: &dyn Theme, activity: f32) {
    let size = 10.0;
    let (rect, _resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let alpha = (activity * 255.0) as u8;
    let color = if activity > 0.05 {
        egui::Color32::from_rgba_unmultiplied(
            theme.accent().r(),
            theme.accent().g(),
            theme.accent().b(),
            alpha.max(40),
        )
    } else {
        theme.bg_well()
    };
    ui.painter().circle_filled(rect.center(), size * 0.45, color);
}

fn draw_route_matrix(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    design::panel(ui, &theme, |ui| {
        ui.label(
            RichText::new("ROUTE MATRIX")
                .size(11.0)
                .strong()
                .color(theme.text_muted()),
        );
        ui.label(
            RichText::new("MIDI source → mixer destination")
                .size(10.0)
                .color(theme.text_dim()),
        );
        ui.add_space(4.0);

        let devices: Vec<String> = if state.midi_snapshot.devices.is_empty() {
            state
                .session
                .midi_devices
                .iter()
                .map(|d| d.id.clone())
                .collect()
        } else {
            state
                .midi_snapshot
                .devices
                .iter()
                .map(|d| d.id.clone())
                .collect()
        };

        if devices.is_empty() {
            ui.label(
                RichText::new("Add a MIDI device first")
                    .size(11.0)
                    .color(theme.text_dim()),
            );
            return;
        }

        let mut selected_device = state
            .midi_selected_device
            .clone()
            .or_else(|| devices.first().cloned())
            .unwrap_or_default();
        egui::ComboBox::from_id_salt("midi_route_device")
            .selected_text(
                state
                    .midi_snapshot
                    .devices
                    .iter()
                    .find(|d| d.id == selected_device)
                    .map(|d| d.description.as_str())
                    .unwrap_or(&selected_device),
            )
            .show_ui(ui, |ui| {
                for id in &devices {
                    let label = state
                        .midi_snapshot
                        .devices
                        .iter()
                        .find(|d| d.id == *id)
                        .map(|d| d.description.as_str())
                        .unwrap_or(id.as_str());
                    ui.selectable_value(&mut selected_device, id.clone(), label);
                }
            });
        if state.midi_selected_device.as_deref() != Some(selected_device.as_str()) {
            state.midi_selected_device = Some(selected_device.clone());
        }

        ui.add_space(6.0);
        let mut rows: Vec<(MidiRouteTarget, String)> = Vec::new();
        for &idx in state.session.tracks_ui_order().iter() {
            let track = state.session.tracks[idx].clone();
            if track.kind.is_master() {
                rows.push((MidiRouteTarget::Master, "Master".into()));
            } else {
                rows.push((
                    MidiRouteTarget::Track { track_id: track.id },
                    track.name.clone(),
                ));
                for plug in &track.inserts {
                    let label = format!(
                        "{} › {}",
                        track.name,
                        plugin_label(state, &plug.id)
                    );
                    rows.push((
                        MidiRouteTarget::Insert {
                            track_id: track.id,
                            slot_id: plug.slot_id,
                        },
                        label,
                    ));
                }
            }
        }
        egui::Grid::new("midi_route_grid")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Target").size(10.0).color(theme.text_dim()));
                ui.label(RichText::new("Routed").size(10.0).color(theme.text_dim()));
                ui.end_row();
                for (target, label) in rows {
                    draw_route_row(ui, state, &selected_device, target, &label);
                }
            });
    });
}

fn draw_route_row(
    ui: &mut egui::Ui,
    state: &mut AppState,
    device_id: &str,
    target: MidiRouteTarget,
    label: &str,
) {
    let theme = state.theme;
    ui.label(RichText::new(label).size(11.0).color(theme.text()));
    let key = target.key();
    let mut routed = state.session.midi_routes.iter().any(|r| {
        r.device_id == device_id && r.target.key() == key && r.enabled
    });
    if ui.checkbox(&mut routed, "").changed() {
        if routed {
            let route = MidiRoute {
                device_id: device_id.to_string(),
                target: target.clone(),
                enabled: true,
            };
            state.session.midi_routes.retain(|r| {
                !(r.device_id == route.device_id && r.target.key() == route.target.key())
            });
            state.session.midi_routes.push(route.clone());
            state.worker.send(Command::Midi(MidiIntent::SetRoute { route }));
        } else {
            state.session.midi_routes.retain(|r| {
                !(r.device_id == device_id && r.target.key() == key)
            });
            state.worker.send(Command::Midi(MidiIntent::RemoveRoute {
                device_id: device_id.to_string(),
                target_key: key,
            }));
        }
        state.dirty = true;
    }
    ui.end_row();
}

fn draw_maps_panel(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    design::panel(ui, &theme, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("CC MAPS")
                    .size(11.0)
                    .strong()
                    .color(theme.text_muted()),
            );
            ui.add_space(12.0);
            if design::button(ui, &theme, "Learn selected param", false).clicked() {
                start_learn_for_selection(state);
            }
        });

        if let Some(learn) = &state.midi_learn {
            ui.label(
                RichText::new(format!("Learn armed — move a CC on {}", learn.hint))
                    .size(11.0)
                    .color(theme.accent()),
            );
        }

        ui.add_space(4.0);
        if state.session.midi_maps.is_empty() {
            ui.label(
                RichText::new(
                    "No bindings yet. Select a track + insert in the mixer, open a knob, \
                     then click Learn selected param.",
                )
                .size(11.0)
                .color(theme.text_dim()),
            );
        } else {
            egui::ScrollArea::vertical()
                .id_salt("midi_maps")
                .max_height(160.0)
                .show(ui, |ui| {
                    let mut remove: Option<(String, u8, u8)> = None;
                    for m in &state.session.midi_maps {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "Ch{} CC{} → {}",
                                    if m.channel == 255 {
                                        "*".into()
                                    } else {
                                        (m.channel + 1).to_string()
                                    },
                                    m.controller,
                                    map_target_label(state, &m.target)
                                ))
                                .size(11.0)
                                .color(theme.text()),
                            );
                            ui.label(
                                RichText::new(&m.device_id)
                                    .size(10.0)
                                    .color(theme.text_dim()),
                            );
                            if design::text_tool_button(ui, &theme, "×").clicked() {
                                remove = Some((m.device_id.clone(), m.channel, m.controller));
                            }
                        });
                    }
                    if let Some((device_id, channel, controller)) = remove {
                        state.session.midi_maps.retain(|m| {
                            !(m.device_id == device_id
                                && m.channel == channel
                                && m.controller == controller)
                        });
                        state.worker.send(Command::Midi(MidiIntent::UnmapCc {
                            device_id,
                            channel,
                            controller,
                        }));
                        state.dirty = true;
                    }
                });
        }
    });
}

fn map_target_label(state: &AppState, target: &MidiMapTarget) -> String {
    match target {
        MidiMapTarget::MasterGain => "Master gain".into(),
        MidiMapTarget::TrackGain { track_id } => state
            .session
            .tracks
            .iter()
            .find(|t| t.id == *track_id)
            .map(|t| format!("{} gain", t.name))
            .unwrap_or_else(|| "Track gain".into()),
        MidiMapTarget::InsertParam {
            track_id,
            slot_id,
            param,
        } => {
            let track = state
                .session
                .tracks
                .iter()
                .find(|t| t.id == *track_id)
                .map(|t| t.name.as_str())
                .unwrap_or("Track");
            let plug = state
                .session
                .tracks
                .iter()
                .find(|t| t.id == *track_id)
                .and_then(|t| t.inserts.iter().find(|p| p.slot_id == *slot_id));
            let ins = plug
                .map(|p| plugin_label(state, &p.id))
                .unwrap_or_else(|| "Insert".into());
            format!("{track} › {ins} › {param}")
        }
    }
}

fn plugin_label(state: &AppState, id: &crate::audio::plugin::PluginId) -> String {
    state
        .plugins
        .plugins()
        .iter()
        .find(|d| d.id == *id)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| id.id.clone())
}

fn upsert_device_enabled(state: &mut AppState, id: &str, description: &str, enabled: bool) {
    if let Some(d) = state.session.midi_devices.iter_mut().find(|d| d.id == id) {
        d.enabled = enabled;
    } else {
        state.session.midi_devices.push(buschain_engine::MidiDeviceInfo {
            id: id.to_string(),
            description: description.to_string(),
            enabled,
        });
    }
    state.dirty = true;
}

fn start_learn_for_selection(state: &mut AppState) {
    let device_id = state.midi_selected_device.clone();
    let pending = if let Some(slot) = state.selected_plugin_slot {
        if let Some(track_id) = state.selected_track {
            if let Some(track) = state.session.tracks.iter().find(|t| t.id == track_id) {
                if let Some(plug) = track.inserts.iter().find(|p| p.slot_id == slot) {
                    let param = plug
                        .params
                        .first()
                        .map(|(k, _)| k.clone())
                        .unwrap_or_else(|| "Mix".into());
                    MidiMapTarget::InsertParam {
                        track_id,
                        slot_id: slot,
                        param,
                    }
                } else {
                    MidiMapTarget::TrackGain { track_id }
                }
            } else {
                MidiMapTarget::MasterGain
            }
        } else {
            MidiMapTarget::MasterGain
        }
    } else if let Some(track_id) = state.selected_track {
        if state
            .session
            .tracks
            .iter()
            .find(|t| t.id == track_id)
            .is_some_and(|t| t.kind.is_master())
        {
            MidiMapTarget::MasterGain
        } else {
            MidiMapTarget::TrackGain { track_id }
        }
    } else {
        MidiMapTarget::MasterGain
    };

    let hint = device_id.clone().unwrap_or_else(|| "any device".into());
    state.midi_learn = Some(crate::app_state::MidiLearnState { hint });
    state.worker.send(Command::Midi(MidiIntent::StartLearn {
        device_id,
        pending,
    }));
    state.status = "MIDI learn — move a control on your surface".into();
}
