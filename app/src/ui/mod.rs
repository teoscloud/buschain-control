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
use crate::design::Theme;
use egui::Key;

pub fn draw(ctx: &egui::Context, state: &mut AppState) {
    let theme = state.theme;
    // Mixer is the only central view — keep tab pinned.
    state.tab = 0;

    egui::TopBottomPanel::top("chrome")
        .exact_height(40.0)
        .frame(
            egui::Frame::NONE
                .fill(theme.bg_panel())
                .stroke(egui::Stroke::new(1.0_f32, theme.border_soft()))
                .inner_margin(egui::Margin::symmetric(8, 2)),
        )
        .show(ctx, |ui| {
            let bar = ui.max_rect();
            ui.horizontal_centered(|ui| {
                // Menus lead from the left: File · System · BusChain
                draw_menu_bar(ui, state);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    draw_window_controls(ui, ctx, state);
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("BUSCHAIN CONTROL")
                            .size(11.0)
                            .strong()
                            .color(theme.text_muted()),
                    );
                });
            });
            // Session title centered in the bar (independent of brand / menus).
            let name = state.session.name.clone();
            let font = egui::FontId::proportional(15.0);
            let name_w = ui.fonts(|f| {
                f.layout_no_wrap(name.clone(), font.clone(), theme.text())
                    .size()
                    .x
            });
            let c = bar.center();
            ui.painter().text(
                c,
                egui::Align2::CENTER_CENTER,
                name,
                font,
                theme.text(),
            );
            if state.dirty {
                ui.painter().text(
                    egui::pos2(c.x + name_w * 0.5 + 8.0, c.y),
                    egui::Align2::LEFT_CENTER,
                    "unsaved",
                    egui::FontId::proportional(11.0),
                    theme.warning(),
                );
            }
        });

    handle_global_shortcuts(ctx, state);
    handle_plugin_shortcuts(ctx, state);

    mixer::draw_mixer_analyzer_panel(ctx, state);
    mixer::draw_channel_rack_panel(ctx, state);

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(theme.bg_app())
                .inner_margin(egui::Margin::ZERO),
        )
        .show(ctx, |ui| {
            mixer::draw_mixer_strips(ui, state);
        });

    draw_options_window(ctx, state);

    plugin_windows::prune_plugin_windows(state);
    plugin_windows::draw_plugin_windows(ctx, state);

    if state.graph_loading {
        draw_graph_loading_overlay(ctx, state);
    }
}

/// Minimal window controls (minimize / fullscreen / close) — quiet suite chrome.
fn draw_window_controls(ui: &mut egui::Ui, ctx: &egui::Context, state: &mut AppState) {
    let theme = state.theme;
    let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
    ui.spacing_mut().item_spacing.x = 2.0;

    let mk = |ui: &mut egui::Ui, glyph: &str, tip: &str, close: bool| {
        let size = egui::vec2(24.0, 22.0);
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
        let hovered = resp.hovered();
        if hovered {
            let fill = if close {
                egui::Color32::from_rgba_unmultiplied(
                    theme.danger().r(),
                    theme.danger().g(),
                    theme.danger().b(),
                    55,
                )
            } else {
                egui::Color32::from_rgba_unmultiplied(
                    theme.text().r(),
                    theme.text().g(),
                    theme.text().b(),
                    28,
                )
            };
            ui.painter()
                .rect_filled(rect, theme.rounding(), fill);
        }
        let glyph_col = if hovered {
            if close {
                theme.danger()
            } else {
                theme.text()
            }
        } else {
            theme.text_muted()
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            glyph,
            egui::FontId::proportional(13.0),
            glyph_col,
        );
        resp.on_hover_text(tip)
    };

    // RTL layout: paint close first so it lands on the far right.
    if mk(ui, "×", "Close (hide to tray)", true).clicked() {
        state.request_hide = true;
    }
    if mk(
        ui,
        if fullscreen { "❐" } else { "□" },
        if fullscreen {
            "Exit fullscreen"
        } else {
            "Fullscreen"
        },
        false,
    )
    .clicked()
    {
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
    }
    if mk(ui, "–", "Minimize", false).clicked() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }
}

fn draw_menu_bar(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    egui::menu::bar(ui, |ui| {
        ui.menu_button(
            egui::RichText::new("File").size(12.0).color(theme.text_dim()),
            |ui| {
                if ui.button("New session").clicked() {
                    state.new_session();
                    state.open_options_buschain(1);
                    state.session_save_as_open = true;
                    state.session_save_as_name = "New session".into();
                    ui.close_menu();
                }
                if ui
                    .button("Save session")
                    .on_hover_text("Ctrl+S")
                    .clicked()
                {
                    state.save_session();
                    ui.close_menu();
                }
                if ui.button("Save session as…").clicked() {
                    state.open_options_buschain(1);
                    state.session_save_as_open = true;
                    state.session_save_as_name = state.session.name.clone();
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Sessions…").clicked() {
                    state.open_options_buschain(1);
                    ui.close_menu();
                }
                if ui.button("Options…").clicked() {
                    state.open_options_system(0);
                    ui.close_menu();
                }
            },
        );
        ui.menu_button(
            egui::RichText::new("System")
                .size(12.0)
                .color(theme.text_dim()),
            |ui| {
                ui.label(
                    egui::RichText::new("System audio")
                        .size(10.0)
                        .color(theme.text_muted()),
                );
                ui.separator();
                if ui.button("Playback").clicked() {
                    state.open_options_system(0);
                    ui.close_menu();
                }
                if ui.button("Recording").clicked() {
                    state.open_options_system(1);
                    ui.close_menu();
                }
                if ui.button("Output devices").clicked() {
                    state.open_options_system(2);
                    ui.close_menu();
                }
                if ui.button("Input devices").clicked() {
                    state.open_options_system(3);
                    ui.close_menu();
                }
            },
        );
        ui.menu_button(
            egui::RichText::new("BusChain")
                .size(12.0)
                .color(theme.text_dim()),
            |ui| {
                ui.label(
                    egui::RichText::new("BusChain")
                        .size(10.0)
                        .color(theme.text_muted()),
                );
                ui.separator();
                if ui.button("MIDI").clicked() {
                    state.open_options_buschain(0);
                    ui.close_menu();
                }
                if ui.button("Sessions").clicked() {
                    state.open_options_buschain(1);
                    ui.close_menu();
                }
                if ui.button("Settings…").clicked() {
                    state.open_options_buschain(2);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    state.request_quit = true;
                    ui.close_menu();
                }
            },
        );
    });
}

fn options_frame(theme: &dyn Theme) -> egui::Frame {
    egui::Frame::NONE
        .fill(theme.bg_panel())
        .stroke(egui::Stroke::new(1.0_f32, theme.border_soft()))
        .corner_radius(theme.rounding())
        .inner_margin(egui::Margin::symmetric(10, 8))
}

fn options_tab_chip(
    ui: &mut egui::Ui,
    theme: &dyn Theme,
    label: &str,
    on: bool,
) -> egui::Response {
    let fill = if on {
        egui::Color32::from_rgba_unmultiplied(
            theme.accent().r(),
            theme.accent().g(),
            theme.accent().b(),
            36,
        )
    } else {
        egui::Color32::TRANSPARENT
    };
    let text = if on {
        theme.text()
    } else {
        theme.text_muted()
    };
    let stroke = if on {
        egui::Stroke::new(1.0_f32, theme.accent_dim())
    } else {
        egui::Stroke::NONE
    };
    ui.add(
        egui::Button::new(egui::RichText::new(label).size(12.0).strong().color(text))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(theme.rounding())
            .min_size(egui::vec2(0.0, 24.0)),
    )
}

/// Single Options window — System / BusChain sections with category tabs.
fn draw_options_window(ctx: &egui::Context, state: &mut AppState) {
    let theme = state.theme;
    let mut open = state.win_options;
    if !open {
        return;
    }

    egui::Window::new("buschain_options")
        .title_bar(false)
        .collapsible(false)
        .resizable(true)
        .constrain(true)
        .default_size([960.0, 680.0])
        .min_width(880.0)
        .min_height(420.0)
        .frame(options_frame(&theme))
        .show(ctx, |ui| {
            // Compact custom title strip (no collapse chevron).
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Options")
                        .size(13.0)
                        .strong()
                        .color(theme.text()),
                );
                ui.label(
                    egui::RichText::new("Esc to close")
                        .size(10.0)
                        .color(theme.text_muted()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("×")
                                    .size(14.0)
                                    .color(theme.text_muted()),
                            )
                            .fill(egui::Color32::TRANSPARENT)
                            .stroke(egui::Stroke::NONE)
                            .min_size(egui::vec2(22.0, 20.0)),
                        )
                        .on_hover_text("Close")
                        .clicked()
                    {
                        open = false;
                    }
                });
            });
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(6.0);

            // Root + subcategory tabs — tight vertical group with a hairline between.
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if options_tab_chip(ui, &theme, "System", state.options_section == 0)
                        .clicked()
                    {
                        state.options_section = 0;
                    }
                    if options_tab_chip(ui, &theme, "BusChain", state.options_section == 1)
                        .clicked()
                    {
                        state.options_section = 1;
                    }
                });
                ui.add_space(3.0);
                let sep_y = ui.cursor().top();
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    sep_y,
                    egui::Stroke::new(1.0_f32, theme.border_soft()),
                );
                ui.add_space(4.0);
                // Slight indent so subcategory row reads as children of the root.
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if state.options_section == 0 {
                        let tabs = ["Playback", "Recording", "Output", "Input"];
                        for (i, name) in tabs.iter().enumerate() {
                            if options_tab_chip(
                                ui,
                                &theme,
                                name,
                                state.options_system_tab == i as u8,
                            )
                            .clicked()
                            {
                                state.options_system_tab = i as u8;
                            }
                        }
                    } else {
                        let tabs = ["MIDI", "Sessions", "Settings"];
                        for (i, name) in tabs.iter().enumerate() {
                            if options_tab_chip(
                                ui,
                                &theme,
                                name,
                                state.options_buschain_tab == i as u8,
                            )
                            .clicked()
                            {
                                state.options_buschain_tab = i as u8;
                            }
                        }
                    }
                });
            });
            ui.add_space(8.0);

            let avail = ui.available_size();
            egui::ScrollArea::vertical()
                .id_salt("options_content_scroll")
                .auto_shrink([false, false])
                .max_height(avail.y)
                .show(ui, |ui| {
                    ui.set_min_width(avail.x.max(840.0));
                    if state.options_section == 0 {
                        match state.options_system_tab {
                            0 => draw_playback(ui, state),
                            1 => draw_recording(ui, state),
                            2 => draw_output_devices(ui, state),
                            _ => draw_input_devices(ui, state),
                        }
                    } else {
                        match state.options_buschain_tab {
                            0 => draw_midi(ui, state),
                            1 => draw_session(ui, state),
                            _ => draw_config(ui, state),
                        }
                    }
                });
        });

    state.win_options = open;
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
                .corner_radius(theme.rounding())
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

fn handle_global_shortcuts(ctx: &egui::Context, state: &mut AppState) {
    let save = ctx.input(|i| {
        i.modifiers.ctrl && !i.modifiers.shift && !i.modifiers.alt && i.key_pressed(Key::S)
    });
    if save {
        state.save_session();
    }

    let esc = !ctx.wants_keyboard_input()
        && ctx.input(|i| i.key_pressed(Key::Escape));
    if esc && state.win_options {
        state.win_options = false;
    }
}

/// Ctrl+1‥9 → open (or focus) floating plugin window for selected track insert N.
fn handle_plugin_shortcuts(ctx: &egui::Context, state: &mut AppState) {
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
