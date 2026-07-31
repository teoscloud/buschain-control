mod app_icons;
mod config;
mod devices;
mod midi;
mod mixer;
mod mixer_popup;
mod playback;
mod plugin_windows;
mod recording;
pub mod runtime;

pub use config::{draw_config, draw_session};
pub use devices::{draw_input_devices, draw_output_devices};
pub use midi::draw_midi;
pub use mixer_popup::{draw_embedded_popup, draw_popup};
pub use playback::draw_playback;
pub use recording::draw_recording;
pub use runtime::{UiRuntime, VizMode, IDLE_REPAINT_MS, LIVE_REPAINT_MS};

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
                        state.tab = 6;
                    }
                    if design::button(ui, &theme, "Save as…", false).clicked() {
                        state.tab = 6;
                        state.session_save_as_open = true;
                        state.session_save_as_name = state.session.name.clone();
                    }
                    if design::button(ui, &theme, "Save", true).clicked() {
                        state.save_session();
                    }
                    if design::button(ui, &theme, "New", false).clicked() {
                        state.new_session();
                        state.tab = 6;
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
                    "MIDI",
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

    // Bottom panels: status is outermost; analyzer sits above it on Mixer.
    if state.tab == 0 {
        mixer::draw_mixer_analyzer_panel(ctx, state);
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
            5 => draw_midi(ui, state),
            6 => draw_session(ui, state),
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
    // Dimmer on Foreground — message must sit *above* this (Tooltip), or the
    // prompt itself gets washed out under the same veil as the mixer.
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("graph_loading_dim"),
    ));
    painter.rect_filled(
        screen,
        0.0,
        egui::Color32::from_rgba_unmultiplied(8, 10, 14, 160),
    );
    // Don't full-screen-allocate an Area here — that grew the viewport on
    // startup. Dimmer is paint-only; prompt lives on a higher Order.

    let msg = if state.status.starts_with("Loading audio graph") {
        state.status.as_str()
    } else {
        "Loading audio graph — starting FX racks…"
    };
    let card_bg = egui::Color32::from_rgb(28, 32, 40);
    let title = egui::Color32::from_rgb(236, 240, 248);
    let subtitle = egui::Color32::from_rgb(168, 176, 192);
    egui::Window::new("graph_loading_msg")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(card_bg)
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(24, 18))
                .stroke(egui::Stroke::new(1.0_f32, theme.border_soft())),
        )
        .show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.label(egui::RichText::new(msg).size(16.0).strong().color(title));
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("App is awake — PipeWire helpers can take a few seconds.")
                    .size(12.5)
                    .color(subtitle),
            );
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
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
