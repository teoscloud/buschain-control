mod app_icons;
mod config;
mod devices;
mod mixer;
mod playback;
mod plugin_windows;
mod recording;

pub use config::{draw_config, draw_session};
pub use devices::{draw_input_devices, draw_output_devices};
pub use playback::draw_playback;
pub use recording::draw_recording;

use crate::app_state::AppState;
use crate::design::{self, Theme};
use egui::Key;

pub fn draw(ctx: &egui::Context, state: &mut AppState) {
    let theme = state.theme;

    egui::TopBottomPanel::top("chrome")
        .exact_height(84.0)
        .frame(
            egui::Frame::NONE
                .fill(theme.bg_panel())
                .stroke(egui::Stroke::new(1.0_f32, theme.border_soft())),
        )
        .show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new("BUSCHAIN CONTROL")
                        .size(15.0)
                        .strong()
                        .color(theme.text()),
                );
                ui.label(
                    egui::RichText::new("Control surface")
                        .size(12.0)
                        .color(theme.text_muted()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(12.0);
                    if design::button(ui, &theme, "Sessions", false).clicked() {
                        state.tab = 5;
                    }
                    if design::button(ui, &theme, "Save as…", false).clicked() {
                        state.tab = 5;
                        state.session_save_as_open = true;
                        state.session_save_as_name = state.session.name.clone();
                    }
                    if design::button(ui, &theme, "Save", true).clicked() {
                        state.save_session();
                    }
                    if design::button(ui, &theme, "New", false).clicked() {
                        state.new_session();
                        state.tab = 5;
                        state.session_save_as_open = true;
                        state.session_save_as_name = "New session".into();
                    }
                });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                let tabs = [
                    "Mixer",
                    "Playback",
                    "Recording",
                    "Output",
                    "Input",
                    "Sessions",
                    "Settings",
                ];
                design::tab_bar(ui, &theme, &tabs, &mut state.tab);
            });
        });

    egui::TopBottomPanel::bottom("status")
        .exact_height(28.0)
        .frame(
            egui::Frame::NONE
                .fill(theme.bg_panel())
                .stroke(egui::Stroke::new(1.0_f32, theme.border_soft())),
        )
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.add_space(12.0);
                let mode = if state.worker.via_daemon {
                    "daemon"
                } else {
                    "in-process"
                };
                design::chip(ui, &theme, mode, state.worker.via_daemon);
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(&state.status)
                        .size(11.0)
                        .color(theme.text_muted()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(
                            "close / Super+Q hides to tray · Quit from tray or Settings",
                        )
                        .size(10.0)
                        .color(theme.text_muted()),
                    );
                });
            });
        });

    handle_plugin_shortcuts(ctx, state);

    if state.tab == 0 {
        mixer::draw_channel_rack_panel(ctx, state);
    }

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(theme.bg_app())
                .inner_margin(egui::Margin::symmetric(10, 8)),
        )
        .show(ctx, |ui| match state.tab {
            0 => mixer::draw_mixer_strips(ui, state),
            1 => draw_playback(ui, state),
            2 => draw_recording(ui, state),
            3 => draw_output_devices(ui, state),
            4 => draw_input_devices(ui, state),
            5 => draw_session(ui, state),
            _ => draw_config(ui, state),
        });

    plugin_windows::prune_plugin_windows(state);
    plugin_windows::draw_plugin_windows(ctx, state);

    if state.graph_loading {
        draw_graph_loading_overlay(ctx, state);
    }
}

fn draw_graph_loading_overlay(ctx: &egui::Context, state: &AppState) {
    let theme = state.theme;
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("graph_loading_overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(screen.size(), egui::Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(
                rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(12, 14, 18, 200),
            );
            let msg = if state.status.starts_with("Loading audio graph") {
                state.status.as_str()
            } else {
                "Loading audio graph — starting FX racks…"
            };
            let galley = painter.layout_no_wrap(
                msg.to_string(),
                egui::FontId::proportional(16.0),
                theme.text(),
            );
            let pos = rect.center() - galley.size() * 0.5;
            painter.galley(pos, galley, theme.text());
            let sub = painter.layout_no_wrap(
                "App is awake — PipeWire helpers can take a few seconds.".to_string(),
                egui::FontId::proportional(12.0),
                theme.text_muted(),
            );
            let sub_pos = egui::pos2(
                rect.center().x - sub.size().x * 0.5,
                pos.y + 28.0,
            );
            painter.galley(sub_pos, sub, theme.text_muted());
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
}

fn popup_favorites_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("buschain-control/mixer-pins.json")
}

fn load_popup_favorites() -> Vec<String> {
    let path = popup_favorites_path();
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn db_to_ui(db: f32) -> f32 {
    let lin = 10f32.powf(db / 20.0);
    (lin * 100.0).clamp(0.0, 150.0)
}

fn ui_to_db(ui: f32) -> f32 {
    let lin = (ui / 100.0).max(1e-4);
    (20.0 * lin.log10()).clamp(-48.0, 12.0)
}

/// Compact overlay — Master HW + favorited tracks + app streams (egui fallback).
pub fn draw_popup(ctx: &egui::Context, state: &mut AppState) {
    use crate::audio::worker::Command;
    use egui::RichText;

    let theme = state.theme;
    let favorites = load_popup_favorites();

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(theme.bg_app())
                .inner_margin(egui::Margin::symmetric(12, 10)),
        )
        .show(ctx, |ui| {
            ui.label(
                RichText::new("BUSCHAIN MIXER")
                    .size(14.0)
                    .strong()
                    .color(theme.text()),
            );
            ui.label(
                RichText::new("Esc to close · portable egui fallback")
                    .size(10.0)
                    .color(theme.text_muted()),
            );
            ui.add_space(8.0);

            design::panel(ui, &theme, |ui| {
                let hw_name = state
                    .session
                    .master_output
                    .clone()
                    .unwrap_or_else(|| "(no Master HW)".into());
                let hw_desc = state
                    .session
                    .master_output_desc
                    .clone()
                    .unwrap_or_else(|| hw_name.clone());
                ui.label(
                    RichText::new(format!("Master HW — {hw_desc}"))
                        .size(12.0)
                        .strong()
                        .color(theme.accent()),
                );
                if let Some(sink) = state.snapshot.sinks.iter().find(|s| s.name == hw_name) {
                    let mut mute = sink.mute;
                    ui.horizontal(|ui| {
                        if design::toggle_chip(ui, &theme, "Mute", &mut mute, theme.danger())
                            .changed()
                        {
                            state.worker.send(Command::SetSinkMute {
                                name: hw_name.clone(),
                                mute,
                            });
                        }
                    });
                    let mut vol = (sink.volume_pct as f32).min(100.0);
                    if design::h_slider(ui, &theme, &mut vol, 0.0..=100.0, "HW volume")
                        .changed()
                    {
                        state.worker.send(Command::SetSinkVolume {
                            name: hw_name.clone(),
                            pct: vol.clamp(0.0, 100.0) as u32,
                        });
                    }
                } else {
                    ui.label(
                        RichText::new("Hardware sink not in snapshot yet")
                            .size(11.0)
                            .color(theme.warning()),
                    );
                }
            });

            // Favorited BusChain tracks (same pins as QS / GTK mixer-pins.json)
            let fav_tracks: Vec<_> = state
                .session
                .tracks
                .iter()
                .filter(|t| favorites.iter().any(|id| id == &t.id.to_string()))
                .cloned()
                .collect();
            if !fav_tracks.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("FAVORITES")
                        .size(11.0)
                        .strong()
                        .color(theme.text_muted()),
                );
                ui.add_space(4.0);
                egui::ScrollArea::horizontal()
                    .id_salt("popup_fav_tracks")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for track in fav_tracks {
                                design::panel(ui, &theme, |ui| {
                                    ui.set_min_width(72.0);
                                    ui.vertical_centered(|ui| {
                                        ui.label(
                                            RichText::new("★")
                                                .size(12.0)
                                                .color(theme.accent()),
                                        );
                                        ui.label(
                                            RichText::new(track.name.clone())
                                                .size(10.0)
                                                .color(theme.text()),
                                        );
                                        let mut ui_vol = db_to_ui(track.gain_db);
                                        if ui
                                            .add(
                                                egui::Slider::new(&mut ui_vol, 0.0..=150.0)
                                                    .vertical()
                                                    .show_value(false),
                                            )
                                            .changed()
                                        {
                                            let gain_db = ui_to_db(ui_vol);
                                            if let Some(t) = state
                                                .session
                                                .tracks
                                                .iter_mut()
                                                .find(|t| t.id == track.id)
                                            {
                                                t.gain_db = gain_db;
                                            }
                                            state.worker.send(Command::SetTrackLevel {
                                                sink: track.expected_sink_name(),
                                                gain_db,
                                                muted: track.mute,
                                            });
                                        }
                                        ui.label(
                                            RichText::new(format!("{:.0}%", db_to_ui(track.gain_db)))
                                                .size(10.0)
                                                .color(theme.text_muted()),
                                        );
                                    });
                                });
                            }
                        });
                    });
            }

            ui.add_space(10.0);
            draw_playback(ui, state);
        });
}

/// Ctrl+1‥9 → open (or focus) floating plugin window for selected track insert N.
fn handle_plugin_shortcuts(ctx: &egui::Context, state: &mut AppState) {
    if state.tab != 0 {
        return;
    }
    let pressed: Option<usize> = ctx.input(|i| {
        if !i.modifiers.ctrl || i.modifiers.shift || i.modifiers.alt {
            return None;
        }
        let keys = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        for (idx, key) in keys.iter().enumerate() {
            if i.key_pressed(*key) {
                return Some(idx);
            }
        }
        None
    });
    if let Some(idx) = pressed {
        if state.select_plugin_by_index(idx) {
            state.status = format!("Plugin {}", idx + 1);
        }
    }
}
