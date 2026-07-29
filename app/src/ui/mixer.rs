use crate::app_state::AppState;
use crate::audio::plugin::{
    apply_denoiser_preset, denoiser_preset_names, plugin_ref_with_defaults, ui_spec_for_ref,
    ParamKind, PluginFormat, PluginId,
};
use crate::audio::worker::Command;
use crate::design::{self, Theme};
use egui::{Color32, RichText, Sense, Vec2};
use uuid::Uuid;

const STRIP_W: f32 = 90.0;
const STRIP_HEADER_H: f32 = 40.0;
/// dB + M/S + util row — identical on every strip so bottoms line up.
const STRIP_DECK_H: f32 = 86.0;
const STRIP_PAD_Y: f32 = 8.0; // frame vertical margin × 2 roughly

/// Left: channel strips (central panel) — full height, horizontally scrollable.
pub fn draw_mixer_strips(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("MIXER")
                .size(11.0)
                .strong()
                .color(theme.text_dim()),
        );
        ui.label(
            RichText::new("scroll horizontally for more channels")
                .size(10.0)
                .color(theme.text_muted()),
        );
    });
    ui.add_space(6.0);

    let strip_h = (ui.available_height() - 4.0).max(280.0);
    // Tall vertical throw for console-style metallic faders.
    let fader_h = (strip_h - STRIP_HEADER_H - STRIP_DECK_H - STRIP_PAD_Y - 8.0).max(160.0);

    egui::ScrollArea::horizontal()
        .id_salt("mixer_strips")
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .show(ui, |ui| {
            ui.set_height(strip_h);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);
                let order = state.session.tracks_ui_order();
                for &ti in &order {
                    if ti >= state.session.tracks.len() {
                        continue;
                    }
                    draw_strip(ui, state, ti, strip_h, fader_h);
                }
                // Add-track pad — same height as strips for alignment
                ui.allocate_ui_with_layout(
                    Vec2::new(72.0, strip_h),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.add_space(strip_h * 0.35);
                        if design::button(ui, &theme, "+ Track", false).clicked() {
                            let n = state
                                .session
                                .tracks
                                .iter()
                                .filter(|t| !t.kind.is_master())
                                .count()
                                + 1;
                            let id = state.session.add_track(format!("Track {n}"));
                            state.selected_track = Some(id);
                            state.ensure_new_track(id);
                        }
                    },
                );
            });
        });
}

/// Right channel rack — real SidePanel (correct hit-testing; never covers strips).
pub fn draw_channel_rack_panel(ctx: &egui::Context, state: &mut AppState) {
    let theme = state.theme;
    let min_w = 300.0_f32;
    let max_w = 560.0_f32;
    state.rack_width = state.rack_width.clamp(min_w, max_w);

    let resp = egui::SidePanel::right("buschain_channel_rack")
        .resizable(true)
        .default_width(state.rack_width)
        .min_width(min_w)
        .max_width(max_w)
        .frame(
            egui::Frame::NONE
                .fill(theme.bg_panel())
                .stroke(egui::Stroke::new(1.0_f32, theme.border_soft()))
                .inner_margin(egui::Margin::same(10)),
        )
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("channel_rack_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    draw_side_panel(ui, state);
                });
        });

    state.rack_width = resp.response.rect.width().clamp(min_w, max_w);
}

fn draw_strip(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    strip_h: f32,
    fader_h: f32,
) {
    let theme = state.theme;
    let track_id = state.session.tracks[track_idx].id;
    let (name, kind) = {
        let t = &state.session.tracks[track_idx];
        (t.name.clone(), t.kind)
    };

    let selected = state.selected_track == Some(track_id);
    let fill = if selected {
        // Soft accent wash — no harsh full-strip outline
        let a = theme.accent();
        Color32::from_rgb(
            ((theme.bg_panel().r() as u16 * 3 + a.r() as u16) / 4) as u8,
            ((theme.bg_panel().g() as u16 * 3 + a.g() as u16) / 4) as u8,
            ((theme.bg_panel().b() as u16 * 3 + a.b() as u16) / 4) as u8,
        )
    } else {
        theme.bg_panel()
    };

    // Hover-only outer rect — click/drag here would steal the fader.
    // Track selection is on the header button (and deck controls).
    let (outer, _strip_resp) =
        ui.allocate_exact_size(Vec2::new(STRIP_W, strip_h), Sense::hover());

    // Paint full-height body first so every strip shares the same silhouette
    ui.painter().rect_filled(outer, theme.rounding(), fill);
    ui.painter().rect_stroke(
        outer,
        theme.rounding(),
        egui::Stroke::new(
            1.0_f32,
            if selected {
                theme.accent().gamma_multiply(0.55)
            } else {
                theme.border_soft()
            },
        ),
        egui::StrokeKind::Outside,
    );
    if selected {
        // Left accent rail
        let rail = egui::Rect::from_min_max(
            outer.min,
            egui::pos2(outer.left() + 3.5, outer.bottom()),
        );
        ui.painter().rect_filled(
            rail,
            egui::CornerRadius {
                nw: 3,
                ne: 0,
                sw: 3,
                se: 0,
            },
            theme.accent(),
        );
        // Header bar underline
        let bar = egui::Rect::from_min_max(
            egui::pos2(outer.left() + 3.5, outer.top()),
            egui::pos2(outer.right(), outer.top() + 2.5),
        );
        ui.painter().rect_filled(bar, egui::CornerRadius::ZERO, theme.accent());
    }

    let inner = outer.shrink2(egui::vec2(5.0, 8.0));
    let header_rect = egui::Rect::from_min_size(
        inner.min,
        Vec2::new(inner.width(), STRIP_HEADER_H),
    );
    let deck_rect = egui::Rect::from_min_max(
        egui::pos2(inner.left(), inner.bottom() - STRIP_DECK_H),
        inner.max,
    );
    let fader_rect = egui::Rect::from_min_max(
        egui::pos2(inner.left(), header_rect.bottom() + 4.0),
        egui::pos2(inner.right(), deck_rect.top() - 4.0),
    );

    // ---- Header (top-locked) ----
    ui.scope_builder(egui::UiBuilder::new().max_rect(header_rect), |ui| {
        ui.set_clip_rect(header_rect);
        let role = if kind.is_master() { "MASTER" } else { "BUS" };
        let header = ui.add_sized(
            [ui.available_width(), STRIP_HEADER_H - 4.0],
            egui::Button::new(
                RichText::new(format!("{}\n{role}", name.to_uppercase()))
                    .size(10.0)
                    .strong()
                    .color(theme.text()),
            )
            .fill(Color32::TRANSPARENT)
            .stroke(egui::Stroke::NONE),
        );
        if header.clicked() {
            state.selected_track = Some(track_id);
            state.sync_selected_plugin();
        }
        ui.painter().hline(
            header_rect.x_range(),
            header_rect.bottom() - 1.0,
            egui::Stroke::new(1.0_f32, theme.border_soft()),
        );
    });

    // ---- Fader (middle band) ----
    let fader_draw_h = fader_h.min(fader_rect.height()).max(80.0);
    ui.scope_builder(egui::UiBuilder::new().max_rect(fader_rect), |ui| {
        ui.set_clip_rect(fader_rect);
        ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
            let top_pad = ((fader_rect.height() - fader_draw_h) * 0.5).max(0.0);
            ui.add_space(top_pad);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let total_w = 26.0 + 6.0 + 11.0;
                let pad = ((ui.available_width() - total_w) * 0.5).max(0.0);
                ui.add_space(pad);
                let mut gain = state.session.tracks[track_idx].gain_db;
                if design::fader_db(
                    ui,
                    &theme,
                    &mut gain,
                    design::STRIP_DB_MIN..=design::STRIP_DB_MAX,
                    Vec2::new(20.0, fader_draw_h),
                )
                .changed()
                {
                    state.session.tracks[track_idx].gain_db = gain;
                    state.selected_track = Some(track_id);
                    state.dirty = true;
                    state.schedule_track_level(track_id);
                }
                let peak_db = state
                    .session
                    .tracks
                    .get(track_idx)
                    .map(|t| {
                        let key = t
                            .sink_name
                            .clone()
                            .unwrap_or_else(|| t.expected_sink_name());
                        state.meters.peak_db(&key)
                    })
                    .unwrap_or(-90.0);
                // Same dB window as the fader — 0 dB rails stay locked together.
                design::meter(ui, &theme, peak_db, Vec2::new(11.0, fader_draw_h), track_id);
            });
        });
    });

    // ---- Control deck (bottom-locked, identical geometry on every strip) ----
    ui.painter().rect_filled(
        deck_rect,
        egui::CornerRadius {
            nw: 0,
            ne: 0,
            sw: 3,
            se: 3,
        },
        theme.bg_well(),
    );
    ui.painter().hline(
        deck_rect.x_range(),
        deck_rect.top(),
        egui::Stroke::new(1.0_f32, theme.border_soft()),
    );

    ui.scope_builder(egui::UiBuilder::new().max_rect(deck_rect), |ui| {
        ui.set_clip_rect(deck_rect);
        ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
            ui.add_space(6.0);
            let gdb = state.session.tracks[track_idx].gain_db;
            let db_col = if gdb > 0.05 {
                theme.warning()
            } else if gdb.abs() < 0.05 {
                theme.accent()
            } else {
                theme.text_dim()
            };
            ui.label(
                RichText::new(format!("{gdb:+.1} dB"))
                    .size(10.0)
                    .monospace()
                    .color(db_col),
            );
            ui.add_space(5.0);

            // M + S always — master S is a dead pad (same size)
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let mut mute = state.session.tracks[track_idx].mute;
                let has_sink = state.session.tracks[track_idx].sink_name.is_some();
                let mute_resp = design::mixer_pad(ui, &theme, "M", &mut mute, theme.danger())
                    .on_hover_text("Mute — live");
                if mute_resp.changed() {
                    state.session.tracks[track_idx].mute = mute;
                    state.selected_track = Some(track_id);
                    state.dirty = true;
                    if has_sink {
                        state.schedule_levels();
                    } else {
                        // Cold bus — bring graph up so mute takes effect on the live sink.
                        state.commit(crate::audio::LiveChange::Reconcile);
                    }
                }

                if kind.is_master() {
                    let _ = design::mixer_pad_inert(ui, &theme, "S")
                        .on_hover_text("Solo is for buses only");
                } else {
                    let mut solo = state.session.tracks[track_idx].solo;
                    if design::mixer_pad(ui, &theme, "S", &mut solo, theme.warning()).changed() {
                        state.session.tracks[track_idx].solo = solo;
                        state.selected_track = Some(track_id);
                        state.dirty = true;
                        state.schedule_levels();
                    }
                }
            });

            ui.add_space(6.0);

            // Util row — same three slots on every strip
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                if kind.is_master() {
                    let _ = design::mixer_util_btn(ui, &theme, egui_phosphor::regular::CIRCLE, false);
                    let _ = design::mixer_util_btn(ui, &theme, egui_phosphor::regular::CIRCLE, false);
                    let _ = design::mixer_util_btn(ui, &theme, egui_phosphor::regular::CIRCLE, false);
                } else {
                    let order = state.session.tracks_ui_order();
                    let pos = order.iter().position(|&i| i == track_idx);
                    let can_left = pos.map(|p| p > 1).unwrap_or(false);
                    let can_right = pos.map(|p| p + 1 < order.len()).unwrap_or(false);
                    if design::mixer_util_btn(
                        ui,
                        &theme,
                        egui_phosphor::regular::CARET_LEFT,
                        can_left,
                    )
                    .on_hover_text("Move left")
                    .clicked()
                        && can_left
                    {
                        if let Some(p) = pos {
                            let a = order[p - 1];
                            let b = order[p];
                            state.session.tracks.swap(a, b);
                            state.dirty = true;
                        }
                    }
                    if design::mixer_util_btn(
                        ui,
                        &theme,
                        egui_phosphor::regular::CARET_RIGHT,
                        can_right,
                    )
                    .on_hover_text("Move right")
                    .clicked()
                        && can_right
                    {
                        if let Some(p) = pos {
                            let a = order[p];
                            let b = order[p + 1];
                            state.session.tracks.swap(a, b);
                            state.dirty = true;
                        }
                    }
                    if design::mixer_util_btn(ui, &theme, egui_phosphor::regular::X, true)
                        .on_hover_text("Remove track")
                        .clicked()
                    {
                        state.request_remove_track(track_id);
                    }
                }
            });
        });
    });

}

fn draw_side_panel(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    ui.set_min_width(ui.available_width());

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("CHANNEL")
                .size(11.0)
                .strong()
                .color(theme.text_dim()),
        );
        ui.label(
            RichText::new("RACK")
                .size(11.0)
                .strong()
                .color(theme.accent()),
        );
    });
    ui.add_space(8.0);

    let Some(sel_id) = state.selected_track else {
        ui.label(
            RichText::new("Select a track on the left")
                .size(13.0)
                .color(theme.text_muted()),
        );
        return;
    };
    let Some(track_idx) = state.session.track_index(sel_id) else {
        ui.label(
            RichText::new("Track gone")
                .size(13.0)
                .color(theme.danger()),
        );
        return;
    };

    let is_master = state.session.tracks[track_idx].kind.is_master();
    let track_name = state.session.tracks[track_idx].name.clone();

    ui.label(
        RichText::new(format!("CHANNEL — {track_name}"))
            .size(14.0)
            .strong()
            .color(theme.text()),
    );
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.label(RichText::new("Name").size(11.0).color(theme.text_dim()));
        let resp = ui.text_edit_singleline(&mut state.session.tracks[track_idx].name);
        if resp.changed() {
            state.dirty = true;
            // Optimistic Output label + push PW device.description (stale Track_N otherwise).
            let bus = state.session.tracks[track_idx].expected_sink_name();
            let desc = if state.session.tracks[track_idx].kind.is_master() {
                "BusChainControl_Master".into()
            } else {
                format!(
                    "BusChainControl_{}",
                    state.session.tracks[track_idx].name.replace(' ', "_")
                )
            };
            if let Some(s) = state.snapshot.sinks.iter_mut().find(|s| s.name == bus) {
                s.description = desc.clone();
            }
            state.worker.send(Command::SetBusDescription {
                name: bus,
                description: desc,
            });
        }
    });

    ui.add_space(10.0);
    design::section_label(ui, &theme, "INPUT SOURCE");
    draw_input_dropdown(ui, state, track_idx);

    if !is_master {
        ui.add_space(4.0);
        let mut listen = state.session.tracks[track_idx].listen;
        if ui.checkbox(&mut listen, "Listen").changed() {
            state.session.tracks[track_idx].listen = listen;
            state.mark_routing_dirty();
        }
        let mut virt = state.session.tracks[track_idx].virtual_output;
        if ui
            .checkbox(&mut virt, "Virtual output device")
            .on_hover_text(
                "When on, this track appears as a system sink (apps / default / Move to). \
                 When off, BusChain hides it from Output / Move to (bus still exists for routing).",
            )
            .changed()
        {
            state.session.tracks[track_idx].virtual_output = virt;
            let bus = state.session.tracks[track_idx].expected_sink_name();
            let desc = format!(
                "BusChainControl_{}",
                state.session.tracks[track_idx].name.replace(' ', "_")
            );
            state.dirty = true;
            let _ = state.session.save();
            // Keep worker last_session aligned so idle reconcile doesn't clobber the flag.
            state
                .worker
                .send(Command::ApplyLevels(state.session.clone()));
            state.worker.send(Command::Refresh);
            // Optimistic Output row — don't wait for the snapshot round-trip.
            if virt {
                if !state.snapshot.sinks.iter().any(|s| s.name == bus) {
                    state.snapshot.sinks.push(crate::audio::graph::DeviceNode {
                        index: 0,
                        name: bus,
                        description: desc,
                        volume_pct: 100,
                        mute: false,
                        sample_rate: None,
                    });
                }
            }
            state.status = if virt {
                "Virtual output enabled for track".into()
            } else {
                "Virtual output hidden — internal bus only".into()
            };
        }
    }

    ui.add_space(10.0);
    design::section_label(ui, &theme, "OUTPUT ROUTING");
    if is_master {
        let hw = state
            .session
            .master_output
            .clone()
            .unwrap_or_else(|| "(auto)".into());
        ui.label(
            RichText::new(format!("HW out: {hw}"))
                .size(11.0)
                .color(theme.text_muted()),
        );
    } else {
        draw_output_routing(ui, state, track_idx, sel_id);
    }

    ui.add_space(10.0);
    design::section_label(ui, &theme, "APPS ON THIS TRACK");
    draw_app_assign(ui, state, track_idx);

    ui.add_space(10.0);
    design::section_label(ui, &theme, "INSERTS");
    draw_insert_rack(ui, state, track_idx);

    ui.add_space(12.0);
    draw_plugin_browser(ui, state, track_idx);
}

fn draw_input_dropdown(ui: &mut egui::Ui, state: &mut AppState, track_idx: usize) {
    let theme = state.theme;
    let current = state.session.tracks[track_idx]
        .input_source
        .clone()
        .unwrap_or_else(|| "(none)".into());
    let sources: Vec<(String, String)> = std::iter::once(("(none)".into(), "(none)".into()))
        .chain(
            state
                .snapshot
                .sources
                .iter()
                .map(|s| (s.name.clone(), s.description.clone())),
        )
        .collect();

    egui::ComboBox::from_id_salt(format!("input_{track_idx}"))
        .selected_text(RichText::new(&current).size(11.0))
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for (name, desc) in &sources {
                let label = if name == "(none)" {
                    "(none)".to_string()
                } else if desc.is_empty() {
                    name.clone()
                } else {
                    format!("{desc}")
                };
                if ui
                    .selectable_label(current == *name, RichText::new(&label).size(11.0))
                    .clicked()
                {
                    if name == "(none)" {
                        state.session.tracks[track_idx].input_source = None;
                        state.session.tracks[track_idx].input_source_desc = None;
                    } else {
                        state.session.tracks[track_idx].input_source = Some(name.clone());
                        state.session.tracks[track_idx].input_source_desc =
                            Some(desc.clone());
                    }
                    state.mark_routing_dirty();
                }
            }
        });
    let _ = theme;
}

fn draw_output_routing(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    self_id: Uuid,
) {
    let master_id = state.session.master_id();
    let candidates: Vec<(Uuid, String, bool)> = state
        .session
        .tracks
        .iter()
        .filter(|t| t.id != self_id)
        .map(|t| {
            (
                t.id,
                if t.kind.is_master() {
                    "Master".into()
                } else {
                    t.name.clone()
                },
                t.kind.is_master(),
            )
        })
        .collect();

    for (id, name, _is_m) in candidates {
        let mut on = state.session.tracks[track_idx]
            .output_targets
            .contains(&id);
        if ui.checkbox(&mut on, &name).changed() {
            let targets = &mut state.session.tracks[track_idx].output_targets;
            if on {
                if !targets.contains(&id) {
                    targets.push(id);
                }
            } else {
                targets.retain(|x| *x != id);
            }
            // Always keep at least master if everything unchecked
            if targets.is_empty() {
                if let Some(mid) = master_id {
                    targets.push(mid);
                }
            }
            state.mark_routing_dirty();
        }
    }
}

fn draw_app_assign(ui: &mut egui::Ui, state: &mut AppState, track_idx: usize) {
    let theme = state.theme;

    // Unique live *user* apps (skip BusChain / foreign filter-chain helpers)
    // (key, display label, icon_name)
    let mut live: Vec<(String, String, Option<String>)> = Vec::new();
    for s in &state.snapshot.sink_inputs {
        if !s.is_user_app() {
            continue;
        }
        let key = s.app_key();
        if key.starts_with("stream:") {
            continue; // still anonymous — don't offer
        }
        if !live.iter().any(|(k, _, _)| k == &key) {
            live.push((key, s.display_name(), s.icon_name.clone()));
        }
    }
    live.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));

    // Assigned chips (resolve label from live streams when possible)
    let assigned = state.session.tracks[track_idx].assigned_playback.clone();
    let mut remove_key: Option<String> = None;
    if !assigned.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
            for key in &assigned {
                let (label, icon) = live
                    .iter()
                    .find(|(k, _, _)| k == key)
                    .map(|(_, d, ic)| (d.clone(), ic.clone()))
                    .unwrap_or_else(|| (pretty_app_key(key), None));
                egui::Frame::NONE
                    .fill(theme.bg_elevated())
                    .stroke(egui::Stroke::new(1.0_f32, theme.border()))
                    .corner_radius(theme.rounding())
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            crate::ui::app_icons::draw_app_icon(ui, state, icon.as_deref());
                            ui.label(RichText::new(&label).size(11.0).color(theme.text()));
                            if design::icon_button(ui, &theme, egui_phosphor::regular::X, true)
                                .on_hover_text("Remove app from this track")
                                .clicked()
                            {
                                remove_key = Some(key.clone());
                            }
                        });
                    });
            }
        });
        ui.add_space(4.0);
    }
    if let Some(key) = remove_key {
        state.session.tracks[track_idx]
            .assigned_playback
            .retain(|a| a != &key && !keys_same_app(a, &key));
        // Unpin → move off this bus onto preferred default / master (fresh list).
        let fallback = state
            .session
            .preferred_default_sink
            .clone()
            .filter(|s| state.snapshot.sinks.iter().any(|d| d.name == *s))
            .or_else(|| Some("buschain_master".into()));
        if let Some(dest) = fallback {
            state.worker.send(crate::audio::worker::Command::PlaceApp {
                app_key: key,
                sink: dest,
            });
        }
    }

    let assigned_now = state.session.tracks[track_idx].assigned_playback.clone();
    let choices: Vec<(String, String, Option<String>)> = live
        .into_iter()
        .filter(|(k, _, _)| {
            !assigned_now
                .iter()
                .any(|a| a == k || keys_same_app(a, k))
        })
        .collect();

    let mut pick: Option<String> = None;
    egui::ComboBox::from_id_salt(format!("app_assign_{track_idx}"))
        .selected_text(
            RichText::new(if choices.is_empty() {
                if state.snapshot.sink_inputs.is_empty() {
                    "No apps playing right now"
                } else {
                    "All live apps already assigned"
                }
            } else {
                "Add application…"
            })
            .size(11.0),
        )
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            if choices.is_empty() {
                ui.label(
                    RichText::new("No live apps")
                        .size(11.0)
                        .color(theme.text_muted()),
                );
            }
            for (key, label, icon) in &choices {
                ui.horizontal(|ui| {
                    crate::ui::app_icons::draw_app_icon(ui, state, icon.as_deref());
                    if ui
                        .selectable_label(false, RichText::new(label).size(11.0))
                        .on_hover_text(key)
                        .clicked()
                    {
                        pick = Some(key.clone());
                    }
                });
            }
        });

    if let Some(key) = pick {
        let tid = state.session.tracks[track_idx].id;
        let sink = state.session.tracks[track_idx]
            .sink_name
            .clone()
            .unwrap_or_else(|| state.session.tracks[track_idx].expected_sink_name());
        {
            let track = &mut state.session.tracks[track_idx];
            track.assigned_playback.retain(|a| {
                !a.chars().all(|c| c.is_ascii_digit()) && !keys_same_app(a, &key)
            });
            if !track.assigned_playback.contains(&key) {
                track.assigned_playback.push(key.clone());
            }
            track.sink_name = Some(sink.clone());
        }
        // Ensure bus exists, then fresh pactl place (priority over FX rewire in worker).
        if !state.snapshot.sinks.iter().any(|s| s.name == sink) {
            state.ensure_new_track(tid);
        }
        state.worker.send(crate::audio::worker::Command::PlaceApp {
            app_key: key,
            sink,
        });
        state.status = "Placing app streams…".into();
    }
}

fn pretty_app_key(key: &str) -> String {
    if let Some(b) = key.strip_prefix("bin:") {
        return b.to_string();
    }
    if let Some(id) = key.strip_prefix("id:") {
        return id.to_string();
    }
    if let Some(n) = key.strip_prefix("name:") {
        return n.to_string();
    }
    key.to_string()
}

fn keys_same_app(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let strip = |k: &str| {
        k.strip_prefix("bin:")
            .or_else(|| k.strip_prefix("id:"))
            .or_else(|| k.strip_prefix("name:"))
            .unwrap_or(k)
            .to_lowercase()
    };
    strip(a) == strip(b)
}

#[derive(Clone, Copy, Default)]
struct InsertDrag {
    from: usize,
}

/// Move `from` so it lands before `drop_before` (0..=len). No-op if order unchanged.
fn reorder_insert_at<T>(items: &mut Vec<T>, from: usize, drop_before: usize) -> bool {
    let n = items.len();
    if from >= n {
        return false;
    }
    let drop_before = drop_before.min(n);
    if drop_before == from || drop_before == from + 1 {
        return false;
    }
    let item = items.remove(from);
    let insert_at = if drop_before > from {
        drop_before - 1
    } else {
        drop_before
    };
    items.insert(insert_at, item);
    true
}

fn draw_insert_rack(ui: &mut egui::Ui, state: &mut AppState, track_idx: usize) {
    let theme = state.theme;
    // Seed defaults for any inserts missing params (older sessions)
    for plug in &mut state.session.tracks[track_idx].inserts {
        plug.ensure_params();
    }

    if state.session.tracks[track_idx].inserts.is_empty() {
        ui.label(
            RichText::new("No inserts")
                .size(11.0)
                .color(theme.text_muted()),
        );
        return;
    }

    let mut remove = None;
    let mut toggle_slot: Option<Uuid> = None;
    let mut did_reorder = false;
    let n = state.session.tracks[track_idx].inserts.len();
    let tid = state.session.tracks[track_idx].id;
    let drag_key = egui::Id::new(("insert_dnd", tid));

    let mut dragging = ui.ctx().data(|d| d.get_temp::<InsertDrag>(drag_key));
    let pointer_y = ui.ctx().pointer_interact_pos().map(|p| p.y);
    let mut drop_before: Option<usize> = None;
    let mut last_row_bottom = ui.cursor().top();
    let content_w = ui.available_width();
    ui.set_max_width(content_w);

    for i in 0..n {
        let (title, slot_id, bypassed) = {
            let plug = &state.session.tracks[track_idx].inserts[i];
            let title = ui_spec_for_ref(plug)
                .map(|s| s.title.to_string())
                .unwrap_or_else(|| plug.id.id.clone());
            (title, plug.slot_id, plug.bypass)
        };
        let selected = state.selected_plugin_slot == Some(slot_id)
            && state.selected_track == Some(tid);
        let window_open = state
            .plugin_windows
            .iter()
            .any(|w| w.track_id == tid && w.slot_id == slot_id);
        let is_source = dragging.is_some_and(|d| d.from == i);

        let fill = if is_source {
            theme.bg_elevated().gamma_multiply(0.85)
        } else if window_open || selected {
            theme.bg_elevated()
        } else {
            theme.bg_well()
        };
        let stroke = if is_source || window_open || selected {
            theme.accent()
        } else if bypassed {
            theme.border_soft()
        } else {
            theme.border()
        };

        if let Some(db) = drop_before {
            if db == i {
                let y = ui.cursor().top();
                let left = ui.max_rect().left();
                ui.painter().hline(
                    egui::Rangef::new(left, left + content_w),
                    y,
                    egui::Stroke::new(2.0_f32, theme.accent()),
                );
            }
        }

        // Fixed columns — identical geometry on every row, clipped to panel width.
        // [ power 28 | name flex | knob 34 | pct 40 | × 26 ]
        const ROW_H: f32 = 44.0;
        const PAD_X: f32 = 8.0;
        const PAD_Y: f32 = 5.0;
        const GAP: f32 = 6.0;
        const POWER_W: f32 = 28.0;
        const KNOB: f32 = 34.0;
        const PCT_W: f32 = 40.0;
        const BTN_W: f32 = 26.0;

        let (row_rect, row_sense) =
            ui.allocate_exact_size(Vec2::new(content_w, ROW_H), Sense::click_and_drag());
        ui.painter().rect_filled(row_rect, theme.rounding(), fill);
        ui.painter().rect_stroke(
            row_rect,
            theme.rounding(),
            egui::Stroke::new(1.0_f32, stroke),
            egui::StrokeKind::Inside,
        );

        if row_sense.drag_started() {
            let d = InsertDrag { from: i };
            ui.ctx().data_mut(|data| data.insert_temp(drag_key, d));
            dragging = Some(d);
        }
        if row_sense.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if row_sense.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        if let (Some(_), Some(y)) = (dragging, pointer_y) {
            if row_rect.y_range().contains(y) {
                drop_before = Some(if y < row_rect.center().y { i } else { i + 1 });
            }
        }
        last_row_bottom = row_rect.bottom();

        let inner = row_rect.shrink2(Vec2::new(PAD_X, PAD_Y));
        let mid_y = inner.center().y;
        let right = inner.right();

        let btn_rect = egui::Rect::from_min_size(
            egui::pos2(right - BTN_W, mid_y - 12.0),
            Vec2::new(BTN_W, 24.0),
        );
        let pct_rect = egui::Rect::from_min_size(
            egui::pos2(btn_rect.left() - GAP - PCT_W, mid_y - 10.0),
            Vec2::new(PCT_W, 20.0),
        );
        let knob_rect = egui::Rect::from_center_size(
            egui::pos2(pct_rect.left() - GAP - KNOB * 0.5, mid_y),
            Vec2::splat(KNOB),
        );
        let power_rect = egui::Rect::from_min_size(
            egui::pos2(inner.left(), mid_y - 12.0),
            Vec2::new(POWER_W, 24.0),
        );
        let name_left = power_rect.right() + GAP;
        let name_right = knob_rect.left() - GAP;
        let name_rect = egui::Rect::from_min_max(
            egui::pos2(name_left, inner.top()),
            egui::pos2(name_right.max(name_left + 24.0), inner.bottom()),
        );

        // Power
        ui.scope_builder(egui::UiBuilder::new().max_rect(power_rect), |ui| {
            let mut enabled = !state.session.tracks[track_idx].inserts[i].bypass;
            let power = design::power_toggle(ui, &theme, &mut enabled);
            if power.changed() || power.clicked() {
                {
                    let plug = &mut state.session.tracks[track_idx].inserts[i];
                    plug.ensure_params();
                    plug.bypass = !enabled;
                }
                state.dirty = true;
                state.flush_fx_params_now(tid);
                state.status = if enabled {
                    "Plugin on".into()
                } else {
                    "Plugin off".into()
                };
            }
        });

        // Name (flex column — truncated)
        let enabled = !bypassed;
        let name_color = if window_open || selected {
            theme.accent()
        } else if enabled {
            theme.text()
        } else {
            theme.text_muted()
        };
        let name = format!("{}. {title}", i + 1);
        ui.scope_builder(egui::UiBuilder::new().max_rect(name_rect), |ui| {
            ui.with_layout(
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_max_width(name_rect.width());
                    let name_resp = ui.add(
                        egui::Label::new(
                            RichText::new(name).size(13.0).strong().color(name_color),
                        )
                        .truncate()
                        .sense(Sense::click()),
                    );
                    if name_resp.clicked() {
                        toggle_slot = Some(slot_id);
                    }
                    if !enabled {
                        ui.label(
                            RichText::new("OFF")
                                .size(10.0)
                                .strong()
                                .color(theme.text_muted()),
                        );
                    }
                },
            );
        });

        // Mix knob — fixed column, no label
        let mut mix = state.session.tracks[track_idx].inserts[i]
            .mix
            .clamp(0.0, 1.0);
        if let Some(v) = state.session.tracks[track_idx].inserts[i].param("Mix") {
            mix = v.clamp(0.0, 1.0);
        }
        let mix_pct = (mix * 100.0).round().clamp(0.0, 100.0) as i32;
        ui.scope_builder(egui::UiBuilder::new().max_rect(knob_rect), |ui| {
            let mix_resp = design::knob_sized(ui, &theme, &mut mix, 0.0..=1.0, "", KNOB)
                .on_hover_text(format!("Mix {mix_pct}% wet"));
            if mix_resp.changed() {
                let plug = &mut state.session.tracks[track_idx].inserts[i];
                plug.mix = mix;
                if plug.param("Mix").is_some() {
                    plug.set_param("Mix", mix);
                }
                state.dirty = true;
                state.schedule_fx_params(tid);
            }
        });

        // Percent — fixed width, right-aligned digits
        ui.scope_builder(egui::UiBuilder::new().max_rect(pct_rect), |ui| {
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.label(
                        RichText::new(format!("{mix_pct}%"))
                            .size(12.0)
                            .strong()
                            .color(if mix_pct == 0 {
                                theme.text_muted()
                            } else {
                                theme.text()
                            }),
                    );
                },
            );
        });

        // Remove
        ui.scope_builder(egui::UiBuilder::new().max_rect(btn_rect), |ui| {
            if design::text_tool_button(ui, &theme, "×")
                .on_hover_text("Remove")
                .clicked()
            {
                remove = Some(i);
            }
        });

        if row_sense.clicked() {
            toggle_slot = Some(slot_id);
        }

        ui.add_space(3.0);
    }

    if let (Some(_), Some(y)) = (dragging, pointer_y) {
        if drop_before.is_none() && y >= last_row_bottom - 2.0 {
            drop_before = Some(n);
        }
        if drop_before == Some(n) {
            let left = ui.max_rect().left();
            ui.painter().hline(
                egui::Rangef::new(left, left + content_w),
                last_row_bottom,
                egui::Stroke::new(2.0_f32, theme.accent()),
            );
        }
    }

    let released = ui.input(|i| i.pointer.any_released());
    if released {
        if let Some(drag) = dragging {
            let to = drop_before.unwrap_or(drag.from);
            if reorder_insert_at(&mut state.session.tracks[track_idx].inserts, drag.from, to)
            {
                did_reorder = true;
            }
        }
        ui.ctx()
            .data_mut(|d| d.remove_temp::<InsertDrag>(drag_key));
    }

    if let Some(slot_id) = toggle_slot {
        let open = state
            .plugin_windows
            .iter()
            .any(|w| w.track_id == tid && w.slot_id == slot_id);
        if open {
            state.close_plugin_window(tid, slot_id);
        } else {
            state.open_plugin_window(tid, slot_id);
        }
    }
    if let Some(i) = remove {
        let slot_id = state.session.tracks[track_idx].inserts[i].slot_id;
        state.close_plugin_window(tid, slot_id);
        state.session.tracks[track_idx].inserts.remove(i);
        state.sync_selected_plugin();
        state.schedule_fx_rewire(tid);
        state.status = "Plugin removed".into();
    }
    if did_reorder {
        state.dirty = true;
        state.schedule_fx_rewire(tid);
        state.status = "Plugin reordered".into();
    }
}

pub(crate) fn draw_plugin_params(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
) {
    let theme = state.theme;
    let plug = &state.session.tracks[track_idx].inserts[insert_idx];
    let Some(spec) = ui_spec_for_ref(plug) else {
        ui.label(
            RichText::new("Unknown plugin — no parameter map")
                .size(11.0)
                .color(theme.danger()),
        );
        return;
    };

    let label = spec.label;
    let is_denoiser = label == "buschain_denoiser";
    let is_eq8 = label == "buschain_eq8";
    let is_softclip = label == "buschain_softclip";
    let is_overdrive = label == "buschain_overdrive";
    let mut changed = false;

    // Dry/wet is on the channel-rack entry knob (PluginRef.mix), not here.

    let track_id = state.session.tracks[track_idx].id;

    if is_softclip {
        changed |= draw_softclip_panel(ui, state, track_idx, insert_idx);
        if changed {
            state.schedule_fx_params(track_id);
        }
        return;
    }

    if is_overdrive {
        changed |= draw_overdrive_panel(ui, state, track_idx, insert_idx);
        if changed {
            state.schedule_fx_params(track_id);
        }
        return;
    }

    if is_eq8 {
        changed |= draw_peq_panel(ui, state, track_idx, insert_idx);
        if changed {
            state.schedule_fx_params(track_id);
        }
        return;
    }

    if is_denoiser {
        ui.label(RichText::new("DENOISER PRESETS").size(10.0).color(theme.text_dim()));
        let mut apply_name: Option<String> = None;
        ui.horizontal_wrapped(|ui| {
            for name in denoiser_preset_names() {
                if design::button(ui, &theme, name, *name == "default").clicked() {
                    apply_name = Some((*name).to_string());
                }
            }
        });
        if let Some(name) = apply_name {
            if apply_denoiser_preset(
                &mut state.session.tracks[track_idx].inserts[insert_idx],
                &name,
            ) {
                changed = true;
                state.status = format!("Denoiser preset “{name}”");
            }
        }
        ui.add_space(4.0);
    }

    // Main controls: skip per-band for denoiser; hide Output/Post Gain makeup (use Mix)
    let main_params: Vec<_> = spec
        .params
        .iter()
        .filter(|p| {
            if is_denoiser {
                return !p.key.starts_with("Range Band") && !p.key.starts_with("Band ");
            }
            // Prefer Mix slider / power toggle over duplicate ports in the generic UI
            if p.key == "Output (dB)"
                || p.key == "Post Gain"
                || p.key == "Mix"
                || p.key == "Bypass"
            {
                return false;
            }
            true
        })
        .collect();

    for def in &main_params {
        changed |= draw_param_row(ui, state, track_idx, insert_idx, def);
    }

    if is_denoiser {
        egui::CollapsingHeader::new(
            RichText::new("Band ranges & freqs")
                .size(11.0)
                .color(theme.text_dim()),
        )
        .default_open(false)
        .show(ui, |ui| {
            for def in spec.params.iter().filter(|p| {
                p.key.starts_with("Range Band") || p.key.starts_with("Band ")
            }) {
                if draw_param_row(ui, state, track_idx, insert_idx, def) {
                    changed = true;
                }
            }
        });
    }

    if changed {
        state.schedule_fx_params(track_id);
    }
}

/// Fruity Soft Clipper layout: THRES|POST knobs · stereo meters · first-quadrant curve.
fn draw_softclip_panel(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
) -> bool {
    let theme = state.theme;
    // OFF = not in the live chain — don't show bus peaks as if this plug is hearing them.
    let powered_off = state
        .session
        .tracks
        .get(track_idx)
        .and_then(|t| t.inserts.get(insert_idx))
        .map(|p| p.bypass)
        .unwrap_or(true);
    let peak_db = if powered_off {
        -90.0
    } else {
        state
            .session
            .tracks
            .get(track_idx)
            .map(|t| {
                let key = t
                    .sink_name
                    .clone()
                    .unwrap_or_else(|| t.expected_sink_name());
                state.meters.peak_db(&key)
            })
            .unwrap_or(-90.0)
    };

    let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
    plug.ensure_params();
    let mut thres = plug.param("Threshold").unwrap_or(0.5);
    let mut post = plug.param("Post").unwrap_or(1.0);
    let mut changed = false;

    let panel_h = 132.0_f32;
    egui::Frame::NONE
        .fill(Color32::from_rgb(0x2b, 0x30, 0x35))
        .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(0x3a, 0x40, 0x46)))
        .corner_radius(egui::CornerRadius::same(3))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.set_height(panel_h);

                // ---- Knobs (side by side, FL style) ----
                ui.allocate_ui_with_layout(
                    Vec2::new(128.0, panel_h),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 14.0;
                            ui.vertical(|ui| {
                                ui.with_layout(
                                    egui::Layout::top_down(egui::Align::Center),
                                    |ui| {
                                        if design::knob_sized(
                                            ui, &theme, &mut thres, 0.05..=1.0, "", 54.0,
                                        )
                                        .changed()
                                        {
                                            changed = true;
                                        }
                                        ui.label(
                                            RichText::new("THRES")
                                                .size(10.0)
                                                .color(Color32::from_rgb(0xb0, 0xb6, 0xbc)),
                                        );
                                    },
                                );
                            });
                            ui.vertical(|ui| {
                                ui.with_layout(
                                    egui::Layout::top_down(egui::Align::Center),
                                    |ui| {
                                        if design::knob_sized(
                                            ui, &theme, &mut post, 0.0..=4.0, "", 54.0,
                                        )
                                        .changed()
                                        {
                                            changed = true;
                                        }
                                        ui.label(
                                            RichText::new("GAIN")
                                                .size(10.0)
                                                .color(Color32::from_rgb(0xb0, 0xb6, 0xbc)),
                                        );
                                        let post_db = if post <= 1e-6 {
                                            "-∞".into()
                                        } else {
                                            format!("{:+.1}", 20.0 * post.log10())
                                        };
                                        ui.label(
                                            RichText::new(format!("{post_db} dB"))
                                                .size(9.0)
                                                .monospace()
                                                .color(if post > 1.001 {
                                                    Color32::from_rgb(0xf0, 0x8a, 0x6a)
                                                } else {
                                                    Color32::from_rgb(0xb0, 0xb6, 0xbc)
                                                }),
                                        );
                                    },
                                );
                            });
                        });
                        ui.add_space(4.0);
                        // SOFT | CLIPPER badge
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            egui::Frame::NONE
                                .fill(Color32::from_rgb(0xc8, 0xcc, 0xd0))
                                .inner_margin(egui::Margin::symmetric(6, 2))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new("SOFT")
                                            .size(10.0)
                                            .strong()
                                            .color(Color32::from_rgb(0x2a, 0x2e, 0x32)),
                                    );
                                });
                            egui::Frame::NONE
                                .stroke(egui::Stroke::new(
                                    1.0_f32,
                                    Color32::from_rgb(0xc8, 0xcc, 0xd0),
                                ))
                                .inner_margin(egui::Margin::symmetric(6, 2))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new("CLIPPER")
                                            .size(10.0)
                                            .strong()
                                            .color(Color32::from_rgb(0xc8, 0xcc, 0xd0)),
                                    );
                                });
                        });
                    },
                );

                ui.add_space(6.0);
                design::softclip_meters(ui, peak_db, Vec2::new(22.0, panel_h - 4.0));
                ui.add_space(6.0);

                let plot_w = (ui.available_width() - 4.0).clamp(160.0, 260.0);
                if design::softclip_transfer_plot(
                    ui,
                    &theme,
                    &mut thres,
                    post,
                    Vec2::new(plot_w, panel_h - 4.0),
                ) {
                    changed = true;
                }
            });
        });

    if changed {
        let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
        plug.set_param("Threshold", thres);
        plug.set_param("Post", post);
    }
    changed
}

/// Blood Overdrive–class panel. Mix lives on the channel-rack MIX knob.
fn draw_overdrive_panel(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
) -> bool {
    let theme = state.theme;
    let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
    plug.ensure_params();

    let mut preband = plug.param("Pre Band").unwrap_or(0.55);
    let mut color = plug.param("Color (Hz)").unwrap_or(180.0);
    let mut preshape = plug.param("Pre Shape").unwrap_or(0.0).round() as i32;
    let mut drive = plug.param("Drive").unwrap_or(0.34);
    let mut boost = plug.param("Boost").unwrap_or(0.0) >= 0.5;
    let mut character = plug.param("Character").unwrap_or(0.0).round() as i32;
    let mut bias = plug.param("Bias").unwrap_or(0.10);
    let mut postf = plug.param("Post Filter (Hz)").unwrap_or(3200.0);
    let mut postg = plug.param("Post Gain").unwrap_or(0.42);
    let mut split = plug.param("Split (Hz)").unwrap_or(100.0);
    let mut focus = plug.param("Focus").unwrap_or(1.0).round() as i32;
    let mut changed = false;

    egui::Frame::NONE
        .fill(Color32::from_rgb(0x1e, 0x22, 0x28))
        .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(0x3d, 0x34, 0x2e)))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("THEATRE DRIVE")
                        .size(11.0)
                        .strong()
                        .color(Color32::from_rgb(0xe8, 0xc9, 0x8a)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new("cinema · Drive Bass @ 100 Hz")
                            .size(9.0)
                            .color(theme.text_muted()),
                    );
                });
            });
            ui.add_space(8.0);

            // Compact knob row — fixed widths so nothing clips
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                for (label, val, range) in [
                    ("PRE", &mut preband, 0.0..=1.0_f32),
                    ("DRIVE", &mut drive, 0.0..=1.0),
                    ("BIAS", &mut bias, -1.0..=1.0),
                    ("LEVEL", &mut postg, 0.0..=1.0),
                ] {
                    ui.allocate_ui_with_layout(
                        Vec2::new(56.0, 72.0),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            if design::knob_sized(ui, &theme, val, range, "", 48.0).changed() {
                                changed = true;
                            }
                            ui.label(
                                RichText::new(label)
                                    .size(9.0)
                                    .color(Color32::from_rgb(0xa8, 0xae, 0xb4)),
                            );
                        },
                    );
                }
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Color").size(10.0).color(theme.text_dim()));
                if design::slider_drag(
                    ui,
                    egui::Slider::new(&mut color, 40.0..=8000.0)
                        .logarithmic(true)
                        .suffix(" Hz"),
                )
                .changed()
                {
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("Post cut").size(10.0).color(theme.text_dim()));
                if design::slider_drag(
                    ui,
                    egui::Slider::new(&mut postf, 200.0..=20000.0)
                        .logarithmic(true)
                        .suffix(" Hz"),
                )
                .changed()
                {
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("Split").size(10.0).color(theme.text_dim()));
                if design::slider_drag(
                    ui,
                    egui::Slider::new(&mut split, 20.0..=500.0)
                        .logarithmic(true)
                        .suffix(" Hz"),
                )
                .changed()
                {
                    changed = true;
                }
            });

            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Focus").size(10.0).color(theme.text_dim()));
                let before = focus;
                egui::ComboBox::from_id_salt(format!("od_focus_{track_idx}_{insert_idx}"))
                    .selected_text(
                        ["Full", "Drive Bass", "Protect Bass"]
                            .get(focus as usize)
                            .copied()
                            .unwrap_or("?"),
                    )
                    .show_ui(ui, |ui| {
                        for (i, name) in ["Full", "Drive Bass", "Protect Bass"].iter().enumerate() {
                            ui.selectable_value(&mut focus, i as i32, *name);
                        }
                    });
                if focus != before {
                    changed = true;
                }

                ui.label(RichText::new("Char").size(10.0).color(theme.text_dim()));
                let before = character;
                egui::ComboBox::from_id_salt(format!("od_char_{track_idx}_{insert_idx}"))
                    .selected_text(
                        ["Tube", "Soft", "Hard", "Diode"]
                            .get(character as usize)
                            .copied()
                            .unwrap_or("?"),
                    )
                    .show_ui(ui, |ui| {
                        for (i, name) in ["Tube", "Soft", "Hard", "Diode"].iter().enumerate() {
                            ui.selectable_value(&mut character, i as i32, *name);
                        }
                    });
                if character != before {
                    changed = true;
                }

                ui.label(RichText::new("Pre").size(10.0).color(theme.text_dim()));
                let before = preshape;
                egui::ComboBox::from_id_salt(format!("od_pre_{track_idx}_{insert_idx}"))
                    .selected_text(
                        ["Low-pass", "Band-pass"]
                            .get(preshape as usize)
                            .copied()
                            .unwrap_or("?"),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut preshape, 0, "Low-pass");
                        ui.selectable_value(&mut preshape, 1, "Band-pass");
                    });
                if preshape != before {
                    changed = true;
                }

                if design::toggle_chip(ui, &theme, "×10", &mut boost, theme.accent()).changed() {
                    changed = true;
                }
            });
        });

    if changed {
        let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
        plug.set_param("Pre Band", preband);
        plug.set_param("Color (Hz)", color);
        plug.set_param("Pre Shape", preshape as f32);
        plug.set_param("Drive", drive);
        plug.set_param("Boost", if boost { 1.0 } else { 0.0 });
        plug.set_param("Character", character as f32);
        plug.set_param("Bias", bias);
        plug.set_param("Post Filter (Hz)", postf);
        plug.set_param("Post Gain", postg);
        plug.set_param("Split (Hz)", split);
        plug.set_param("Focus", focus as f32);
    }
    changed
}

/// Parametric EQ — interactive graph + band controls (BusChain console theme).
fn draw_peq_panel(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
) -> bool {
    let theme = state.theme;
    let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
    plug.ensure_params();

    let mut bands: Vec<design::PeqBand> = (1..=8)
        .map(|n| design::PeqBand {
            on: plug.param(&format!("B{n} On")).unwrap_or(1.0) >= 0.5,
            freq: plug.param(&format!("B{n} Freq")).unwrap_or(1000.0),
            gain_db: plug.param(&format!("B{n} Gain")).unwrap_or(0.0),
            q: plug.param(&format!("B{n} Q")).unwrap_or(0.707),
            mode: plug.param(&format!("B{n} Type")).unwrap_or(0.0).round() as i32,
        })
        .collect();
    let mut out_gain = plug.param("Output (dB)").unwrap_or(0.0);
    let mut changed = false;

    let sel_key = egui::Id::new(("peq_sel", track_idx, insert_idx));
    let mut selected: usize = ui.ctx().data(|d| d.get_temp(sel_key)).unwrap_or(0);
    selected = selected.min(bands.len().saturating_sub(1));

    egui::Frame::NONE
        .fill(theme.bg_elevated())
        .stroke(egui::Stroke::new(1.0_f32, theme.border_soft()))
        .corner_radius(theme.rounding())
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("PARAMETRIC EQ")
                        .size(12.0)
                        .strong()
                        .color(theme.accent()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new("drag nodes · snap @ 0 dB")
                            .size(10.0)
                            .color(theme.text_muted()),
                    );
                });
            });
            ui.add_space(4.0);

            let graph_w = ui.available_width().clamp(240.0, 560.0);
            if design::peq_graph(
                ui,
                &theme,
                &mut bands,
                out_gain,
                &mut selected,
                Vec2::new(graph_w, 180.0),
            ) {
                changed = true;
            }

            ui.add_space(8.0);

            // Per-band gain faders
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (i, b) in bands.iter_mut().enumerate() {
                    let col = design::PEQ_BAND_COLORS[i % design::PEQ_BAND_COLORS.len()];
                    ui.vertical(|ui| {
                        ui.set_width(36.0);
                        let on_lbl = format!("{}", i + 1);
                        let mut on = b.on;
                        if design::toggle_chip(ui, &theme, &on_lbl, &mut on, col).changed() {
                            b.on = on;
                            changed = true;
                            selected = i;
                        }
                        let mut g = b.gain_db;
                        if design::fader_db(ui, &theme, &mut g, -24.0..=24.0, Vec2::new(28.0, 90.0))
                            .changed()
                        {
                            b.gain_db = g;
                            changed = true;
                            selected = i;
                        }
                        ui.label(
                            RichText::new(format!("{:+.1}", b.gain_db))
                                .size(9.0)
                                .monospace()
                                .color(col),
                        );
                    });
                }
            });

            ui.add_space(6.0);
            let bi = selected;
            let col = design::PEQ_BAND_COLORS[bi % design::PEQ_BAND_COLORS.len()];
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("Band {}", bi + 1))
                        .size(11.0)
                        .strong()
                        .color(col),
                );
                if let Some(b) = bands.get_mut(bi) {
                    let mut freq = b.freq;
                    let mut q = b.q;
                    let mut mode = b.mode;
                    ui.label(RichText::new("Hz").size(10.0).color(theme.text_dim()));
                    if design::slider_drag(
                        ui,
                        egui::Slider::new(&mut freq, 20.0..=20000.0)
                            .logarithmic(true)
                            .show_value(true),
                    )
                    .changed()
                    {
                        b.freq = freq;
                        changed = true;
                    }
                    ui.label(RichText::new("Q").size(10.0).color(theme.text_dim()));
                    if design::slider_drag(
                        ui,
                        egui::Slider::new(&mut q, 0.1..=10.0).show_value(true),
                    )
                    .changed()
                    {
                        b.q = q;
                        changed = true;
                    }
                    let modes = ["Peak", "LowShelf", "HighShelf", "HP", "LP"];
                    egui::ComboBox::from_id_salt(format!("peq_mode_{track_idx}_{insert_idx}"))
                        .selected_text(modes.get(mode as usize).copied().unwrap_or("Peak"))
                        .show_ui(ui, |ui| {
                            for (i, name) in modes.iter().enumerate() {
                                if ui.selectable_value(&mut mode, i as i32, *name).changed() {
                                    changed = true;
                                }
                            }
                        });
                    if mode != b.mode {
                        b.mode = mode;
                        changed = true;
                    }
                }
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("OUTPUT")
                        .size(11.0)
                        .strong()
                        .color(theme.accent()),
                );
                ui.label(
                    RichText::new("overall trim")
                        .size(10.0)
                        .color(theme.text_muted()),
                );
            });
            ui.horizontal(|ui| {
                if design::fader_db(
                    ui,
                    &theme,
                    &mut out_gain,
                    -24.0..=24.0,
                    Vec2::new(28.0, 72.0),
                )
                .changed()
                {
                    changed = true;
                }
                ui.vertical(|ui| {
                    ui.add_space(8.0);
                    if design::slider_drag(
                        ui,
                        egui::Slider::new(&mut out_gain, -24.0..=24.0)
                            .suffix(" dB")
                            .show_value(true),
                    )
                    .changed()
                    {
                        changed = true;
                    }
                    if ui
                        .add(egui::Button::new("0 dB").small())
                        .on_hover_text("Reset overall gain")
                        .clicked()
                    {
                        out_gain = 0.0;
                        changed = true;
                    }
                    ui.label(
                        RichText::new(format!("{out_gain:+.1} dB"))
                            .size(11.0)
                            .monospace()
                            .color(theme.text()),
                    );
                });
            });
            ui.label(
                RichText::new("Use MIX above for dry/wet · Output is post-EQ makeup (±24 dB)")
                    .size(9.0)
                    .color(theme.text_muted()),
            );
        });

    ui.ctx().data_mut(|d| d.insert_temp(sel_key, selected));

    if changed {
        let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
        for (i, b) in bands.iter().enumerate() {
            let n = i + 1;
            plug.set_param(&format!("B{n} On"), if b.on { 1.0 } else { 0.0 });
            plug.set_param(&format!("B{n} Freq"), b.freq);
            plug.set_param(&format!("B{n} Gain"), b.gain_db);
            plug.set_param(&format!("B{n} Q"), b.q);
            plug.set_param(&format!("B{n} Type"), b.mode as f32);
        }
        plug.set_param("Output (dB)", out_gain.clamp(-24.0, 24.0));
    }
    changed
}

fn draw_param_row(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
    def: &crate::audio::plugin::ParamDef,
) -> bool {
    let theme = state.theme;
    let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
    let mut value = plug.param(def.key).unwrap_or(def.default);
    let mut changed = false;

    match def.kind {
        ParamKind::Toggle => {
            let mut on = value >= 0.5;
            ui.horizontal(|ui| {
                if design::toggle_chip(ui, &theme, def.label, &mut on, theme.accent()).changed() {
                    value = if on { 1.0 } else { 0.0 };
                    changed = true;
                }
            });
        }
        ParamKind::Mode => {
            let modes = def.modes.unwrap_or(&[]);
            let mut idx = value.round().clamp(def.min, def.max) as usize;
            let before = idx;
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(def.label)
                        .size(10.0)
                        .color(theme.text_dim()),
                );
                egui::ComboBox::from_id_salt(format!(
                    "mode_{track_idx}_{insert_idx}_{}",
                    def.key
                ))
                .selected_text(modes.get(idx).copied().unwrap_or("?"))
                .show_ui(ui, |ui| {
                    for (i, name) in modes.iter().enumerate() {
                        ui.selectable_value(&mut idx, i, *name);
                    }
                });
            });
            if idx != before {
                changed = true;
            }
            value = idx as f32;
        }
        ParamKind::Slider => {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(def.label)
                        .size(10.0)
                        .color(theme.text_dim())
                        .strong(),
                );
                let mut slider = egui::Slider::new(&mut value, def.min..=def.max)
                    .show_value(true)
                    .min_decimals(0)
                    .max_decimals(2);
                if def.logarithmic {
                    slider = slider.logarithmic(true);
                }
                if design::slider_drag(ui, slider).changed() {
                    changed = true;
                }
            });
        }
    }

    if changed {
        plug.set_param(def.key, value.clamp(def.min, def.max));
    }
    changed
}

fn plugin_menu_group(id: &str, fmt: PluginFormat) -> &'static str {
    match fmt {
        PluginFormat::Ladspa => {
            let id = id.to_lowercase();
            if id.contains("compressor")
                || id.contains("limiter")
                || id.contains("gate")
                || id.contains("softclip")
            {
                "Dynamics"
            } else if id.contains("eq") || id.contains("overdrive") {
                "EQ & Tone"
            } else if id.contains("pitch") {
                "Pitch"
            } else if id.contains("denoiser") || id.contains("noise") {
                "Cleanup"
            } else if id.contains("buschain_") {
                "BusChain"
            } else {
                "LADSPA"
            }
        }
        PluginFormat::Lv2 => "LV2",
        PluginFormat::Clap => "CLAP",
        PluginFormat::Vst3 => "VST3",
    }
}

/// Group order for the add-plugin menu.
const PLUGIN_GROUP_ORDER: &[&str] = &[
    "Dynamics",
    "EQ & Tone",
    "Pitch",
    "Cleanup",
    "BusChain",
    "LADSPA",
    "LV2",
    "CLAP",
    "VST3",
];

fn draw_plugin_browser(ui: &mut egui::Ui, state: &mut AppState, track_idx: usize) {
    let theme = state.theme;
    // Prefer concrete LADSPA labels; skip aggregate .so names / vague bundles.
    let plugins: Vec<(PluginId, String, PluginFormat)> = state
        .plugins
        .plugins()
        .iter()
        .filter(|p| p.id.format == PluginFormat::Ladspa)
        .filter(|p| {
            let id = p.id.id.to_lowercase();
            let name = p.name.to_lowercase();
            if id.contains("buschain_builtins") || name == "shadow builtins" {
                return false;
            }
            true
        })
        .map(|p| (p.id.clone(), p.name.clone(), p.id.format))
        .collect();

    if plugins.is_empty() {
        ui.label(
            RichText::new("No plugins scanned — Settings → Plugins / make plugins")
                .size(11.0)
                .color(theme.text_muted()),
        );
        return;
    }

    let mut groups: Vec<(&str, Vec<(PluginId, String)>)> = PLUGIN_GROUP_ORDER
        .iter()
        .map(|g| (*g, Vec::new()))
        .collect();
    for (id, name, fmt) in plugins {
        let g = plugin_menu_group(&id.id, fmt);
        if let Some((_, bucket)) = groups.iter_mut().find(|(n, _)| *n == g) {
            bucket.push((id, name));
        }
    }
    for (_, bucket) in &mut groups {
        bucket.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    }

    let add_label = format!("{}  Add plugin", egui_phosphor::regular::PLUS);
    let mut picked: Option<(PluginId, String)> = None;

    let add_w = ui.available_width();
    egui::Frame::NONE
        .fill(theme.accent())
        .stroke(egui::Stroke::new(1.0_f32, theme.accent_dim()))
        .corner_radius(theme.rounding())
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_max_width(add_w);
            ui.set_min_width(add_w - 4.0);
            ui.spacing_mut().button_padding = egui::vec2(6.0, 4.0);
            ui.menu_button(
                RichText::new(&add_label)
                    .size(13.0)
                    .strong()
                    .color(Color32::WHITE),
                |ui| {
                    ui.set_min_width(240.0);
                    ui.label(
                        RichText::new("Choose an insert")
                            .size(11.0)
                            .color(theme.text_muted()),
                    );
                    ui.label(
                        RichText::new("v1: LADSPA inserts only")
                            .size(10.0)
                            .color(theme.text_muted()),
                    );
                    ui.separator();
                    for (group, items) in &groups {
                        if items.is_empty() {
                            continue;
                        }
                        ui.label(
                            RichText::new(*group)
                                .size(10.0)
                                .strong()
                                .color(theme.accent()),
                        );
                        for (id, name) in items {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(name).size(12.0).color(theme.text()),
                                    )
                                    .fill(Color32::TRANSPARENT)
                                    .min_size(Vec2::new(220.0, 22.0)),
                                )
                                .clicked()
                            {
                                picked = Some((id.clone(), name.clone()));
                                ui.close_menu();
                            }
                        }
                        ui.add_space(4.0);
                    }
                },
            );
        });

    if let Some((id, name)) = picked {
        state.session.tracks[track_idx]
            .inserts
            .push(plugin_ref_with_defaults(id));
        let tid = state.session.tracks[track_idx].id;
        state.schedule_fx_rewire(tid);
        if !state.status.starts_with("Live graph bring-up") {
            state.status = format!("Added {name} — live slot rewire…");
        }
    }
}
