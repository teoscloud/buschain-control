//! In-app plugin editor windows (never separate OS/viewport instances).
//! Multiple editors may be open; order is oldest → newest. Esc closes newest.
//!
//! VST3: native OOP editor opens by default; converted egui params are opt-in.

use egui::{Align2, Key, RichText, Vec2};
use uuid::Uuid;

use crate::app_state::AppState;
use crate::audio::plugin::{plugin_title_for_ref, ui_spec_for_ref};
use crate::design::Theme;
use crate::ui::mixer::{draw_plugin_params, draw_sidechain_picker};

/// Preferred *width* — tight to content (no empty right pad).
fn preferred_width(label: &str) -> f32 {
    match label {
        "buschain_equalizer" | "buschain_eq8" => 680.0,
        "buschain_softclip" => 400.0,
        "buschain_overdrive" => 620.0,
        "buschain_denoiser" => 700.0,
        "buschain_pitch" => 280.0,
        "buschain_gate" => 360.0,
        "buschain_reverb" => 780.0,
        "buschain_eq" => 300.0,
        "buschain_compressor" => 360.0,
        "buschain_limiter" => 540.0,
        _ => 340.0,
    }
}

fn chrome_size() -> Vec2 {
    Vec2::new(340.0, 140.0)
}

/// Draw every open plugin editor as an egui `Window` inside the main app.
pub fn draw_plugin_windows(ctx: &egui::Context, state: &mut AppState) {
    let keys: Vec<(Uuid, Uuid)> = state
        .plugin_windows
        .iter()
        .map(|w| (w.track_id, w.slot_id))
        .collect();

    let mut close: Vec<(Uuid, Uuid)> = Vec::new();
    let mut toggle_fs: Option<(Uuid, Uuid)> = None;
    let mut focus: Option<(Uuid, Uuid)> = None;

    let esc = !keys.is_empty()
        && !ctx.wants_keyboard_input()
        && ctx.input(|i| i.key_pressed(Key::Escape));
    if esc {
        if let Some(&(tid, sid)) = keys.last() {
            close.push((tid, sid));
        }
    }

    for (stack_idx, (track_id, slot_id)) in keys.into_iter().enumerate() {
        if close.iter().any(|(t, s)| *t == track_id && *s == slot_id) {
            continue;
        }
        let Some(win_idx) = state
            .plugin_windows
            .iter()
            .position(|w| w.track_id == track_id && w.slot_id == slot_id)
        else {
            continue;
        };
        let fullscreen = state.plugin_windows[win_idx].fullscreen;
        let show_converted = state.plugin_windows[win_idx].show_converted_ui;
        let supports_native = state.insert_supports_native_editor(track_id, slot_id);
        let surface = buschain_engine::slot_wants_surface(slot_id);

        let Some((track_idx, insert_idx)) = state.find_insert(track_id, slot_id) else {
            close.push((track_id, slot_id));
            continue;
        };

        let (title, label) = {
            let plug = &state.session.tracks[track_idx].inserts[insert_idx];
            let name = plugin_title_for_ref(plug);
            let label = ui_spec_for_ref(plug)
                .map(|s| s.label.to_string())
                .unwrap_or_else(|| plug.id.id.clone());
            let track_name = state.session.tracks[track_idx].name.clone();
            (format!("{name} — {track_name}"), label)
        };

        let want_w = if show_converted || !supports_native {
            preferred_width(&label)
        } else {
            chrome_size().x
        };
        let open_gen = state.plugin_windows[win_idx].open_gen;
        // "fit3" salt invalidates old oversized egui window memory.
        let id = egui::Id::new((
            "buschain_plugin_win",
            "fit3",
            track_id,
            slot_id,
            open_gen,
            show_converted,
        ));
        let mut open = true;
        let cascade = stack_idx as f32 * 28.0;

        let mut win = egui::Window::new(&title)
            .id(id)
            .open(&mut open)
            .collapsible(false)
            .resizable(!fullscreen)
            .default_width(want_w)
            .min_width((want_w * 0.85).min(want_w))
            .max_width(want_w)
            // Height follows content — never force a tall empty frame.
            .auto_sized()
            .default_pos([72.0 + cascade, 72.0 + cascade])
            .hscroll(false)
            .vscroll(false);

        if fullscreen {
            let screen = ctx.screen_rect();
            win = win
                .anchor(Align2::LEFT_TOP, [0.0, 0.0])
                .fixed_size(screen.size())
                .constrain(true);
        } else {
            win = win.constrain(true);
        }

        let mut interacted = false;
        let mut reopen_native = false;
        let mut set_converted: Option<bool> = None;
        let fs_clicked = win
            .show(ctx, |ui| {
                ui.set_max_width(want_w);
                let mut fs = false;
                if ui.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::Enter)) {
                    fs = true;
                }
                if ui.input(|i| i.pointer.any_pressed() || i.pointer.any_down())
                    && ui.ui_contains_pointer()
                {
                    interacted = true;
                }

                if supports_native {
                    let theme = state.theme;
                    ui.horizontal(|ui| {
                        let mut converted = show_converted;
                        if ui
                            .checkbox(&mut converted, "Converted params")
                            .on_hover_text(
                                "Generic egui parameter list (optional when a native GUI exists)",
                            )
                            .changed()
                        {
                            set_converted = Some(converted);
                        }
                        if ui
                            .small_button("Native editor")
                            .on_hover_text("Reopen out-of-process VST3 GUI")
                            .clicked()
                        {
                            reopen_native = true;
                        }
                    });
                    if surface {
                        ui.label(
                            RichText::new("Native editor floats separately · enable Converted params for egui.")
                                .size(10.0)
                                .color(theme.text_muted()),
                        );
                        state.try_attach_surface_editor(slot_id);
                    } else if !show_converted {
                        ui.label(
                            RichText::new("Native GUI floats separately · enable Converted params for egui.")
                                .size(10.0)
                                .color(theme.text_muted()),
                        );
                    }
                    if !show_converted {
                        if let Some((ti, ii)) = state.find_insert(track_id, slot_id) {
                            draw_sidechain_picker(ui, state, ti, ii);
                        }
                    }
                    ui.add_space(4.0);
                }

                if show_converted || !supports_native {
                    if let Some((ti, ii)) = state.find_insert(track_id, slot_id) {
                        draw_plugin_params(ui, state, ti, ii);
                    }
                }

                fs
            })
            .map(|r| {
                if r.response.clicked()
                    || r.response.drag_started()
                    || r.response.has_focus()
                    || r.response.gained_focus()
                {
                    interacted = true;
                }
                r.inner.unwrap_or(false)
            })
            .unwrap_or(false);

        if let Some(v) = set_converted {
            if let Some(w) = state
                .plugin_windows
                .iter_mut()
                .find(|w| w.track_id == track_id && w.slot_id == slot_id)
            {
                w.show_converted_ui = v;
            }
        }
        if reopen_native {
            state.open_native_editor(track_id, slot_id);
        }
        if interacted {
            focus = Some((track_id, slot_id));
        }
        if fs_clicked {
            toggle_fs = Some((track_id, slot_id));
        }
        if !open {
            close.push((track_id, slot_id));
        }
    }

    for (tid, sid) in close {
        state.close_plugin_window(tid, sid);
    }
    if let Some((tid, sid)) = focus {
        if state
            .plugin_windows
            .iter()
            .any(|w| w.track_id == tid && w.slot_id == sid)
        {
            state.focus_plugin_window(tid, sid);
        }
    }
    if let Some((tid, sid)) = toggle_fs {
        state.toggle_plugin_fullscreen(tid, sid);
    }
}

/// Drop windows whose inserts/tracks no longer exist.
pub fn prune_plugin_windows(state: &mut AppState) {
    let stale: Vec<(Uuid, Uuid)> = state
        .plugin_windows
        .iter()
        .filter(|w| state.find_insert(w.track_id, w.slot_id).is_none())
        .map(|w| (w.track_id, w.slot_id))
        .collect();
    for (tid, sid) in stale {
        state.close_plugin_window(tid, sid);
    }
}
