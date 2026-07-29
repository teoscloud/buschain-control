use crate::app_state::AppState;
use crate::audio::worker::Command;
use crate::design::{self, Theme};
use egui::RichText;

pub fn draw_playback(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    ui.horizontal(|ui| {
        design::section_label(ui, &theme, "PLAYBACK — application streams (sink inputs)");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.checkbox(&mut state.show_hidden_playback, "Show hidden");
        });
    });
    ui.add_space(6.0);

    egui::ScrollArea::vertical().show(ui, |ui| {
        let inputs: Vec<_> = state
            .snapshot
            .sink_inputs
            .iter()
            .filter(|s| state.show_hidden_playback || s.is_user_app())
            .cloned()
            .collect();
        if inputs.is_empty() {
            ui.label(
                RichText::new(if state.show_hidden_playback {
                    "No active playback streams"
                } else {
                    "No application streams (enable Show hidden for internals)"
                })
                .color(theme.text_muted()),
            );
            return;
        }
        for stream in inputs {
            design::panel(ui, &theme, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(stream.display_name())
                                .size(13.0)
                                .strong()
                                .color(theme.text()),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{} · {} · sink {}",
                                stream.name,
                                stream.app_key(),
                                stream.sink_or_source
                            ))
                            .size(11.0)
                            .color(theme.text_muted()),
                        );
                        if !stream.is_user_app() {
                            ui.label(
                                RichText::new("internal")
                                    .size(10.0)
                                    .color(theme.warning()),
                            );
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut mute = stream.mute;
                        if design::toggle_chip(ui, &theme, "Mute", &mut mute, theme.danger())
                            .changed()
                        {
                            state.worker.send(Command::SetSinkInputMute {
                                index: stream.index,
                                mute,
                            });
                        }
                    });
                });
                let mut vol = stream.volume_pct as f32;
                if design::h_slider(ui, &theme, &mut vol, 0.0..=150.0, "Volume").changed() {
                    state.worker.send(Command::SetSinkInputVolume {
                        index: stream.index,
                        pct: vol as u32,
                    });
                }

                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Move to:").size(11.0).color(theme.text_dim()));
                    let track_sinks: Vec<(String, String)> = state
                        .session
                        .tracks
                        .iter()
                        .filter(|t| t.kind.is_master() || t.virtual_output)
                        .filter_map(|t| {
                            t.sink_name
                                .as_ref()
                                .map(|s| (t.name.clone(), s.clone()))
                        })
                        .collect();
                    for (name, sink) in &track_sinks {
                        if design::button(ui, &theme, name, false).clicked() {
                            state.worker.send(Command::MoveSinkInput {
                                index: stream.index,
                                sink: sink.clone(),
                            });
                        }
                    }
                    let track_sink_set: std::collections::HashSet<&str> =
                        track_sinks.iter().map(|(_, s)| s.as_str()).collect();
                    let sinks: Vec<String> = state
                        .snapshot
                        .sinks
                        .iter()
                        .filter(|s| {
                            !s.name.starts_with("buschain_fx_")
                                && !s.name.starts_with("buschain_post_")
                                && !s.name.starts_with("buschain_mid_")
                                && !s.name.starts_with("buschain_rs_")
                                && s.name != "buschain_hold"
                                && !track_sink_set.contains(s.name.as_str())
                        })
                        .map(|s| s.name.clone())
                        .collect();
                    for sink in sinks {
                        if design::button(ui, &theme, &sink, false).clicked() {
                            state.worker.send(Command::MoveSinkInput {
                                index: stream.index,
                                sink,
                            });
                        }
                    }
                });
            });
            ui.add_space(6.0);
        }
    });
}
