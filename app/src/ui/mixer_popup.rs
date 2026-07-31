//! QS-inspired compact mixer popup (tray left-click / `buschain-control --popup`).

use crate::app_state::AppState;
use crate::audio::worker::Command;
use crate::design::{self, Theme};
use crate::session::Session;
use egui::{CornerRadius, RichText, Stroke, Vec2, ViewportBuilder, ViewportClass, ViewportId};

fn favorites_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("buschain-control/mixer-pins.json")
}

fn load_favorites() -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(favorites_path()) else {
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

/// Snap track fader UI% to unity (0 dB).
fn snap_unity_ui(v: &mut f32) {
    if (*v - 100.0).abs() < 4.0 {
        *v = 100.0;
    }
}

/// Snap device/app volume to 100%.
fn snap_pct_100(v: &mut f32) {
    if (*v - 100.0).abs() < 3.0 {
        *v = 100.0;
    }
}

fn is_internal_helper(name: &str) -> bool {
    name.starts_with("easyeffects_")
        || name.contains("filter-chain")
        || name == "auto_null"
        || name.starts_with("buschain_hold")
        || name.starts_with("buschain_post_")
        || name.starts_with("buschain_rs_")
        || name.starts_with("buschain_vinf_")
        || name.starts_with("buschain_fx_")
}

fn include_sink(name: &str, session: &Session) -> bool {
    if is_internal_helper(name) {
        return false;
    }
    if name.starts_with("buschain_") {
        return session.tracks.iter().any(|t| {
            t.expected_sink_name() == name && (t.kind.is_master() || t.virtual_output)
        });
    }
    true
}

fn include_source(name: &str, session: &Session) -> bool {
    if is_internal_helper(name) || name.contains(".monitor") {
        return false;
    }
    if name.starts_with("buschain_vin_") {
        return session.tracks.iter().any(|t| {
            !t.kind.is_master() && t.virtual_input && t.expected_virtual_input_name() == name
        });
    }
    if name.starts_with("buschain_") {
        return false;
    }
    true
}

fn sink_title(session: &Session, name: &str, description: &str) -> String {
    if let Some(t) = session.tracks.iter().find(|t| t.expected_sink_name() == name) {
        if t.kind.is_master() {
            return "Master".into();
        }
        return t.name.clone();
    }
    description.to_string()
}

fn source_title(session: &Session, name: &str, description: &str) -> String {
    if let Some(t) = session.tracks.iter().find(|t| {
        !t.kind.is_master() && t.virtual_input && t.expected_virtual_input_name() == name
    }) {
        return t.name.clone();
    }
    description.to_string()
}

fn pill_tab(ui: &mut egui::Ui, theme: &dyn Theme, label: &str, active: bool) -> egui::Response {
    let fill = if active {
        theme.accent()
    } else {
        theme.bg_well()
    };
    let text = if active {
        egui::Color32::WHITE
    } else {
        theme.text_muted()
    };
    ui.add(
        egui::Button::new(RichText::new(label).size(11.0).strong().color(text))
            .fill(fill)
            .stroke(Stroke::new(
                1.0_f32,
                if active {
                    theme.accent_dim()
                } else {
                    theme.border_soft()
                },
            ))
            .corner_radius(CornerRadius::same(12)),
    )
}

fn badge(ui: &mut egui::Ui, theme: &dyn Theme, label: &str, accent: bool) {
    let fill = if accent {
        theme.accent().gamma_multiply(0.35)
    } else {
        theme.bg_well()
    };
    let color = if accent {
        theme.accent()
    } else {
        theme.text_muted()
    };
    egui::Frame::NONE
        .fill(fill)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(9.0).strong().color(color));
        });
}

/// Speaker icon mute toggle (no "Mute" text).
fn mute_speaker(ui: &mut egui::Ui, theme: &dyn Theme, muted: &mut bool) -> egui::Response {
    let icon = if *muted {
        egui_phosphor::regular::SPEAKER_SLASH
    } else {
        egui_phosphor::regular::SPEAKER_HIGH
    };
    let color = if *muted {
        theme.danger()
    } else {
        theme.text_muted()
    };
    let mut resp = ui
        .add(
            egui::Button::new(RichText::new(icon).size(16.0).color(color))
                .fill(egui::Color32::TRANSPARENT)
                .stroke(Stroke::NONE)
                .min_size(Vec2::new(28.0, 28.0)),
        )
        .on_hover_text(if *muted { "Unmute" } else { "Mute" });
    if resp.clicked() {
        *muted = !*muted;
        resp.mark_changed();
    }
    resp
}

/// Horizontal volume: slider only + speaker mute. No label / no number box.
fn vol_row(
    ui: &mut egui::Ui,
    theme: &dyn Theme,
    vol: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    muted: &mut bool,
    snap_100: bool,
) -> (bool, bool) {
    let mut vol_changed = false;
    let mut mute_changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let mut resp = design::slider_drag(ui, vol, range.clone(), |s| {
            s.show_value(false).trailing_fill(true)
        });
        if resp.changed() {
            if snap_100 {
                snap_pct_100(vol);
            }
            vol_changed = true;
            resp.mark_changed();
        }
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width().max(1.0), 0.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                if mute_speaker(ui, theme, muted).changed() {
                    mute_changed = true;
                }
            },
        );
    });
    (vol_changed, mute_changed)
}

/// Narrow mixer strip — fixed slot so favorites and apps share the same grid.
const STRIP_W: f32 = 52.0;
const STRIP_H: f32 = 220.0;
const STRIP_TITLE_H: f32 = 18.0;
const STRIP_READOUT_H: f32 = 16.0;
const STRIP_MUTE_H: f32 = 28.0;
/// Compact fader hit target (caps auto-shrink when width < 36).
const FADER_SIZE: Vec2 = Vec2::new(32.0, 140.0);

/// Vertical strip fader (Playback apps / Tracks).
fn strip_fader(
    ui: &mut egui::Ui,
    theme: &dyn Theme,
    title: &str,
    ui_vol: &mut f32,
    _range: std::ops::RangeInclusive<f32>,
    muted: &mut bool,
    readout: &str,
    snap_unity: bool,
) -> (bool, bool) {
    let mut vol_changed = false;
    let mut mute_changed = false;
    let inner_w = STRIP_W - 8.0;
    ui.allocate_ui_with_layout(
        Vec2::new(STRIP_W, STRIP_H),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            ui.set_min_size(Vec2::new(STRIP_W, STRIP_H));
            ui.set_max_size(Vec2::new(STRIP_W, STRIP_H));
            egui::Frame::NONE
                .fill(theme.bg_panel())
                .stroke(Stroke::new(1.0_f32, theme.border_soft()))
                .corner_radius(theme.rounding())
                .inner_margin(egui::Margin::symmetric(4, 6))
                .show(ui, |ui| {
                    ui.set_min_size(Vec2::new(inner_w, STRIP_H - 12.0));
                    ui.set_max_size(Vec2::new(inner_w, STRIP_H - 12.0));
                    // Fixed slots → favorite tracks and app streams share one baseline.
                    ui.allocate_ui_with_layout(
                        Vec2::new(inner_w, STRIP_TITLE_H),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(title)
                                        .size(10.0)
                                        .strong()
                                        .color(theme.text()),
                                )
                                .truncate(),
                            );
                        },
                    );
                    let mut db = ui_to_db(*ui_vol);
                    let resp = design::fader_db(
                        ui,
                        theme,
                        &mut db,
                        design::STRIP_DB_MIN..=design::STRIP_DB_MAX,
                        FADER_SIZE,
                    );
                    if resp.changed() {
                        *ui_vol = db_to_ui(db);
                        if snap_unity {
                            snap_unity_ui(ui_vol);
                        } else {
                            snap_pct_100(ui_vol);
                        }
                        vol_changed = true;
                    }
                    ui.allocate_ui_with_layout(
                        Vec2::new(inner_w, STRIP_READOUT_H),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            ui.label(
                                RichText::new(readout)
                                    .size(9.0)
                                    .strong()
                                    .monospace()
                                    .color(if *muted {
                                        theme.danger()
                                    } else {
                                        theme.text()
                                    }),
                            );
                        },
                    );
                    ui.allocate_ui_with_layout(
                        Vec2::new(inner_w, STRIP_MUTE_H),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            if mute_speaker(ui, theme, muted).changed() {
                                mute_changed = true;
                            }
                        },
                    );
                });
        },
    );
    (vol_changed, mute_changed)
}

/// Favorites first (leftmost), then remaining session tracks.
fn tracks_favorites_first(state: &AppState, favorites: &[String]) -> Vec<crate::session::Track> {
    let mut fav = Vec::new();
    let mut rest = Vec::new();
    for t in &state.session.tracks {
        if favorites.iter().any(|id| id == &t.id.to_string()) {
            fav.push(t.clone());
        } else {
            rest.push(t.clone());
        }
    }
    fav.extend(rest);
    fav
}

fn apply_track_level(state: &mut AppState, track_id: uuid::Uuid, gain_db: f32, mute: bool) {
    let sink = state
        .session
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .map(|t| t.expected_sink_name())
        .unwrap_or_default();
    if let Some(t) = state.session.tracks.iter_mut().find(|t| t.id == track_id) {
        t.gain_db = gain_db;
        t.mute = mute;
    }
    crate::daemon::push_track_mixer_to_daemon(track_id, gain_db, mute);
    state.worker.send(Command::SetTrackLevel {
        sink,
        gain_db,
        muted: mute,
    });
}

fn draw_session_track_strip(ui: &mut egui::Ui, state: &mut AppState, track: &crate::session::Track) {
    let theme = state.theme;
    let mut mute = track.mute;
    let mut ui_vol = db_to_ui(track.gain_db);
    let gain = track.gain_db;
    // Same width class as app "100%" so strip columns stay uniform.
    let readout = if gain.abs() < 0.05 {
        "  0dB".into()
    } else {
        format!("{gain:+4.0}")
    };
    let title = if track.kind.is_master() {
        "Master".to_string()
    } else {
        track.name.clone()
    };
    let (vol_ch, mute_ch) = strip_fader(
        ui,
        &theme,
        &title,
        &mut ui_vol,
        0.0..=150.0,
        &mut mute,
        &readout,
        true,
    );
    if vol_ch {
        snap_unity_ui(&mut ui_vol);
        apply_track_level(state, track.id, ui_to_db(ui_vol), mute);
    } else if mute_ch {
        apply_track_level(state, track.id, track.gain_db, mute);
    }
}

/// Standalone `--popup` window (fills the viewport).
pub fn draw_popup(ctx: &egui::Context, state: &mut AppState) {
    let theme = state.theme;
    let mut _keep = true;
    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(theme.bg_app())
                .inner_margin(egui::Margin::symmetric(12, 10)),
        )
        .show(ctx, |ui| {
            draw_popup_body(ui, state, &mut _keep, false);
        });
}

/// In-process always-on-top child viewport (tray left-click while UI exists).
pub fn draw_embedded_popup(ctx: &egui::Context, state: &mut AppState) {
    if !state.mixer_popup_open {
        return;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.mixer_popup_open = false;
        return;
    }

    let mut keep_open = true;
    let theme = state.theme;
    ctx.show_viewport_immediate(
        ViewportId::from_hash_of("buschain_mixer_popup"),
        ViewportBuilder::default()
            .with_title("BusChain Mixer")
            .with_inner_size([480.0, 560.0])
            .with_min_inner_size([360.0, 320.0])
            .with_always_on_top()
            .with_decorations(true)
            .with_app_id("buschain-control-popup"),
        |vp_ctx, class| {
            if class == ViewportClass::Embedded {
                egui::Window::new("BusChain Mixer")
                    .collapsible(false)
                    .resizable(true)
                    .default_size([480.0, 560.0])
                    .frame(
                        egui::Frame::NONE
                            .fill(theme.bg_app())
                            .stroke(Stroke::new(1.0_f32, theme.border()))
                            .corner_radius(theme.rounding())
                            .inner_margin(egui::Margin::symmetric(12, 10)),
                    )
                    .show(vp_ctx, |ui| {
                        draw_popup_body(ui, state, &mut keep_open, true);
                    });
                return;
            }
            if vp_ctx.input(|i| i.viewport().close_requested()) {
                keep_open = false;
            }
            if vp_ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                keep_open = false;
            }
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::NONE
                        .fill(theme.bg_app())
                        .inner_margin(egui::Margin::symmetric(12, 10)),
                )
                .show(vp_ctx, |ui| {
                    draw_popup_body(ui, state, &mut keep_open, true);
                });
        },
    );
    state.mixer_popup_open = keep_open;
}

fn draw_popup_body(
    ui: &mut egui::Ui,
    state: &mut AppState,
    keep_open: &mut bool,
    show_close: bool,
) {
    let theme = state.theme;

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("BUSCHAIN MIXER")
                .size(13.0)
                .strong()
                .color(theme.text()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if show_close {
                if ui
                    .add(
                        egui::Button::new(RichText::new("×").size(14.0).color(theme.text_muted()))
                            .fill(egui::Color32::TRANSPARENT)
                            .frame(false),
                    )
                    .on_hover_text("Close")
                    .clicked()
                {
                    *keep_open = false;
                }
            }
        });
    });
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let tabs = ["Playback", "Tracks", "Output", "Input"];
        for (i, label) in tabs.iter().enumerate() {
            let active = state.mixer_popup_tab == i as u8;
            if pill_tab(ui, &theme, label, active).clicked() {
                state.mixer_popup_tab = i as u8;
            }
        }
    });
    ui.add_space(10.0);

    egui::ScrollArea::vertical()
        .id_salt("mixer_popup_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| match state.mixer_popup_tab {
            0 => draw_tab_playback(ui, state),
            1 => draw_tab_tracks(ui, state),
            2 => draw_tab_output(ui, state),
            _ => draw_tab_input(ui, state),
        });
}

fn draw_tab_playback(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    let hw_name = state
        .session
        .master_output
        .clone()
        .unwrap_or_else(|| "(no Master HW)".into());
    let hw_desc = state
        .session
        .master_output_desc
        .clone()
        .unwrap_or_else(|| "Master HW".into());

    design::panel(ui, &theme, |ui| {
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(&hw_desc)
                        .size(12.0)
                        .strong()
                        .color(theme.accent()),
                )
                .truncate(),
            );
            badge(ui, &theme, "Master HW", true);
        });
        let hw_snap = state
            .snapshot
            .sinks
            .iter()
            .find(|s| s.name == hw_name)
            .map(|s| (s.mute, s.volume_pct));
        if let Some((hw_mute, hw_pct)) = hw_snap {
            let mut mute = hw_mute;
            let mut vol = (hw_pct as f32).min(100.0);
            let (vol_ch, mute_ch) = vol_row(ui, &theme, &mut vol, 0.0..=100.0, &mut mute, true);
            if vol_ch {
                let pct = vol.clamp(0.0, 100.0) as u32;
                let _ = crate::ipc::Client::call_fast(&crate::ipc::Request::SetHwVolume { pct })
                    .or_else(|_| {
                        crate::ipc::Client::call(&crate::ipc::Request::SetHwVolume { pct })
                    });
                if let Some(s) = state.snapshot.sinks.iter_mut().find(|s| s.name == hw_name) {
                    s.volume_pct = pct;
                }
            }
            if mute_ch {
                let _ = crate::ipc::Client::call_fast(&crate::ipc::Request::SetHwMute { mute })
                    .or_else(|_| {
                        crate::ipc::Client::call(&crate::ipc::Request::SetHwMute { mute })
                    });
                if let Some(s) = state.snapshot.sinks.iter_mut().find(|s| s.name == hw_name) {
                    s.mute = mute;
                }
            }
        } else {
            ui.label(
                RichText::new("Hardware sink not ready")
                    .size(11.0)
                    .color(theme.warning()),
            );
        }
    });

    ui.add_space(10.0);
    ui.label(
        RichText::new("STRIPS")
            .size(10.0)
            .strong()
            .color(theme.text_muted()),
    );
    ui.add_space(4.0);

    let favorites = load_favorites();
    let fav_tracks: Vec<_> = state
        .session
        .tracks
        .iter()
        .filter(|t| favorites.iter().any(|id| id == &t.id.to_string()))
        .cloned()
        .collect();
    let streams: Vec<_> = state
        .snapshot
        .sink_inputs
        .iter()
        .filter(|s| s.is_user_app())
        .cloned()
        .collect();

    if fav_tracks.is_empty() && streams.is_empty() {
        ui.label(
            RichText::new("No favorite tracks or app streams")
                .size(11.0)
                .color(theme.text_muted()),
        );
        return;
    }

    egui::ScrollArea::horizontal()
        .id_salt("popup_playback_strips")
        .show(ui, |ui| {
            // Left-aligned equal gaps — matches STRIPS header, keeps fav + apps in one grid.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for track in &fav_tracks {
                    draw_session_track_strip(ui, state, track);
                }
                for stream in streams {
                    let mut mute = stream.mute;
                    let mut vol = stream.volume_pct as f32;
                    let readout = format!("{:>3}%", stream.volume_pct.min(150));
                    let (vol_ch, mute_ch) = strip_fader(
                        ui,
                        &theme,
                        &stream.display_name(),
                        &mut vol,
                        0.0..=150.0,
                        &mut mute,
                        &readout,
                        false,
                    );
                    if vol_ch {
                        snap_pct_100(&mut vol);
                        state.worker.send(Command::SetSinkInputVolume {
                            index: stream.index,
                            pct: vol.round() as u32,
                        });
                    }
                    if mute_ch {
                        state.worker.send(Command::SetSinkInputMute {
                            index: stream.index,
                            mute,
                        });
                    }
                }
            });
        });
}

fn draw_tab_tracks(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    let favorites = load_favorites();
    let tracks = tracks_favorites_first(state, &favorites);

    if tracks.is_empty() {
        ui.label(
            RichText::new("No session tracks")
                .size(11.0)
                .color(theme.text_muted()),
        );
        return;
    }

    ui.label(
        RichText::new("TRACKS")
            .size(10.0)
            .strong()
            .color(theme.text_muted()),
    );
    ui.add_space(6.0);

    egui::ScrollArea::horizontal()
        .id_salt("popup_track_strips")
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for track in &tracks {
                    draw_session_track_strip(ui, state, track);
                }
            });
        });
}

fn draw_tab_output(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    let master = state.session.master_output.clone();
    let default_sink = state.snapshot.default_sink.clone();
    let sinks: Vec<_> = state
        .snapshot
        .sinks
        .iter()
        .filter(|s| include_sink(&s.name, &state.session))
        .cloned()
        .collect();

    if sinks.is_empty() {
        ui.label(
            RichText::new("No output devices")
                .size(11.0)
                .color(theme.text_muted()),
        );
        return;
    }

    for sink in sinks {
        let is_master = master.as_deref() == Some(sink.name.as_str());
        let is_default = default_sink.as_deref() == Some(sink.name.as_str());
        let is_virtual = state.session.tracks.iter().any(|t| {
            t.expected_sink_name() == sink.name && (t.kind.is_master() || t.virtual_output)
        });
        let is_app_bus =
            sink.name == "buschain_master" || sink.name.starts_with("buschain_track_");
        let title = sink_title(&state.session, &sink.name, &sink.description);

        design::panel(ui, &theme, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(&title)
                            .size(12.0)
                            .strong()
                            .color(theme.text()),
                    )
                    .truncate(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if is_virtual {
                        badge(ui, &theme, "Virtual", false);
                    }
                    if is_master {
                        badge(ui, &theme, "Master HW", true);
                    }
                    if is_default {
                        badge(ui, &theme, "Default", true);
                    }
                });
            });
            ui.horizontal(|ui| {
                if !is_app_bus && design::button(ui, &theme, "Set HW", is_master).clicked() {
                    state.session.master_output = Some(sink.name.clone());
                    state.session.master_output_desc = Some(sink.description.clone());
                    let _ = state.session.save();
                    state.worker.send(Command::SetMasterHw {
                        name: sink.name.clone(),
                        desc: Some(sink.description.clone()),
                    });
                }
                if design::button(ui, &theme, "Set default", is_default).clicked() {
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
                    state.snapshot.default_sink = Some(sink.name.clone());
                    state
                        .worker
                        .send(Command::SetDefaultSink(sink.name.clone()));
                    let _ = state.session.save();
                }
            });
            if !is_app_bus {
                let mut mute = sink.mute;
                let mut vol = (sink.volume_pct as f32).min(100.0);
                let (vol_ch, mute_ch) =
                    vol_row(ui, &theme, &mut vol, 0.0..=100.0, &mut mute, true);
                if vol_ch {
                    let pct = vol.clamp(0.0, 100.0) as u32;
                    if is_master {
                        let _ =
                            crate::ipc::Client::call_fast(&crate::ipc::Request::SetHwVolume { pct })
                                .or_else(|_| {
                                    crate::ipc::Client::call(&crate::ipc::Request::SetHwVolume {
                                        pct,
                                    })
                                });
                    } else {
                        state.worker.send(Command::SetSinkVolume {
                            name: sink.name.clone(),
                            pct,
                        });
                    }
                    if let Some(s) = state.snapshot.sinks.iter_mut().find(|s| s.name == sink.name)
                    {
                        s.volume_pct = pct;
                    }
                }
                if mute_ch {
                    if is_master {
                        let _ =
                            crate::ipc::Client::call_fast(&crate::ipc::Request::SetHwMute { mute })
                                .or_else(|_| {
                                    crate::ipc::Client::call(&crate::ipc::Request::SetHwMute {
                                        mute,
                                    })
                                });
                    } else {
                        state.worker.send(Command::SetSinkMute {
                            name: sink.name.clone(),
                            mute,
                        });
                    }
                    if let Some(s) = state.snapshot.sinks.iter_mut().find(|s| s.name == sink.name)
                    {
                        s.mute = mute;
                    }
                }
            } else {
                ui.label(
                    RichText::new("Level from track fader")
                        .size(10.0)
                        .color(theme.text_dim()),
                );
            }
        });
        ui.add_space(4.0);
    }
}

fn draw_tab_input(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    let default_source = state.snapshot.default_source.clone();
    let sources: Vec<_> = state
        .snapshot
        .sources
        .iter()
        .filter(|s| include_source(&s.name, &state.session))
        .cloned()
        .collect();

    if sources.is_empty() {
        ui.label(
            RichText::new("No input devices")
                .size(11.0)
                .color(theme.text_muted()),
        );
        return;
    }

    for src in sources {
        let is_default = default_source.as_deref() == Some(src.name.as_str());
        let is_virtual = state.session.tracks.iter().any(|t| {
            !t.kind.is_master() && t.virtual_input && t.expected_virtual_input_name() == src.name
        });
        let title = source_title(&state.session, &src.name, &src.description);

        design::panel(ui, &theme, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(&title)
                            .size(12.0)
                            .strong()
                            .color(theme.text()),
                    )
                    .truncate(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if is_virtual {
                        badge(ui, &theme, "Virtual", false);
                    }
                    if is_default {
                        badge(ui, &theme, "Default", true);
                    }
                });
            });
            if design::button(ui, &theme, "Set default", is_default).clicked() {
                state.snapshot.default_source = Some(src.name.clone());
                state
                    .worker
                    .send(Command::SetDefaultSource(src.name.clone()));
            }
            let mut mute = src.mute;
            let mut vol = (src.volume_pct as f32).min(100.0);
            let (vol_ch, mute_ch) = vol_row(ui, &theme, &mut vol, 0.0..=100.0, &mut mute, true);
            if vol_ch {
                let pct = vol.clamp(0.0, 100.0) as u32;
                state.worker.send(Command::SetSourceVolume {
                    name: src.name.clone(),
                    pct,
                });
                if let Some(s) = state.snapshot.sources.iter_mut().find(|s| s.name == src.name) {
                    s.volume_pct = pct;
                }
            }
            if mute_ch {
                state.worker.send(Command::SetSourceMute {
                    name: src.name.clone(),
                    mute,
                });
                if let Some(s) = state.snapshot.sources.iter_mut().find(|s| s.name == src.name) {
                    s.mute = mute;
                }
            }
        });
        ui.add_space(4.0);
    }
}
