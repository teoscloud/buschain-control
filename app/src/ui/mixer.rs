use crate::app_state::AppState;
use crate::audio::plugin::{
    apply_denoiser_preset, denoiser_preset_names, dynamic_ui_for_ref, plugin_ref_with_defaults,
    plugin_title_for_ref, ui_spec_for_ref, OwnedParamDef, ParamKind, PluginFormat, PluginId,
};
use crate::audio::worker::Command;
use crate::design::{self, Theme};
use egui::{Color32, RichText, Sense, Vec2};
use uuid::Uuid;

const STRIP_W: f32 = 90.0;
const STRIP_HEADER_H: f32 = 40.0;
/// dB + ON LED — identical on every strip so bottoms line up.
const STRIP_DECK_H: f32 = 58.0;
const STRIP_PAD_Y: f32 = 8.0; // frame vertical margin × 2 roughly

#[derive(Clone, Copy, Default)]
struct TrackDrag {
    /// Index into [`Session::tracks_ui_order`].
    from_ui: usize,
}

/// Left: channel strips (central panel) — full height, horizontally scrollable.
pub fn draw_mixer_strips(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;

    // Solo UI removed — clear any leftover session solos so buses aren't stuck muted.
    if state.session.tracks.iter().any(|t| t.solo) {
        for t in &mut state.session.tracks {
            t.solo = false;
        }
        state.dirty = true;
        state.schedule_levels();
    }

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("MIXER")
                .size(11.0)
                .strong()
                .color(theme.text_dim()),
        );
        ui.label(
            RichText::new("drag headers to reorder · analyzer below")
                .size(10.0)
                .color(theme.text_muted()),
        );
    });
    ui.add_space(6.0);

    let strip_h = (ui.available_height() - 4.0).max(280.0);
    let fader_h = (strip_h - STRIP_HEADER_H - STRIP_DECK_H - STRIP_PAD_Y - 8.0).max(160.0);

    let drag_key = egui::Id::new("mixer_track_dnd");
    let mut dragging = ui.ctx().data(|d| d.get_temp::<TrackDrag>(drag_key));
    let pointer_x = ui.ctx().pointer_interact_pos().map(|p| p.x);
    let mut drop_before: Option<usize> = None;
    let mut did_reorder = false;

    // Clip so strip Outside-strokes / scroll content never paint into the rack.
    let clip = ui.clip_rect();
    egui::ScrollArea::horizontal()
        .id_salt("mixer_strips")
        .auto_shrink([false, false])
        // Don't steal vertical fader drags for pan-scrolling.
        .drag_to_scroll(false)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .show(ui, |ui| {
            ui.set_clip_rect(clip.intersect(ui.max_rect()));
            ui.set_height(strip_h);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);
                let order = state.session.tracks_ui_order();
                for (ui_i, &ti) in order.iter().enumerate() {
                    if ti >= state.session.tracks.len() {
                        continue;
                    }
                    let is_source = dragging.is_some_and(|d| d.from_ui == ui_i);
                    let outer = draw_strip(ui, state, ti, strip_h, fader_h, is_source, ui_i == 0);

                    // Drop indicator between strips (Master slot 0 is pinned).
                    if let (Some(_), Some(x)) = (dragging, pointer_x) {
                        if outer.x_range().contains(x) {
                            drop_before =
                                Some(if x < outer.center().x { ui_i } else { ui_i + 1 });
                        }
                    }
                    if let Some(db) = drop_before {
                        if db == ui_i && ui_i > 0 {
                            let x = outer.left() - 3.0;
                            ui.painter().vline(
                                x,
                                egui::Rangef::new(outer.top() + 8.0, outer.bottom() - 8.0),
                                egui::Stroke::new(2.0_f32, theme.accent()),
                            );
                        }
                    }

                    // Header: click = select, drag = reorder (buses only — Master pinned).
                    let header = egui::Rect::from_min_size(
                        outer.min,
                        Vec2::new(outer.width(), STRIP_HEADER_H + 8.0),
                    );
                    let sense = ui.interact(
                        header,
                        egui::Id::new(("track_hdr_dnd", ti)),
                        Sense::click_and_drag(),
                    );
                    if sense.clicked() {
                        let id = state.session.tracks[ti].id;
                        state.selected_track = Some(id);
                        state.sync_selected_plugin();
                    }
                    // Right-click → select + delete option (buses only; Master stays).
                    sense.context_menu(|ui| {
                        let id = state.session.tracks[ti].id;
                        let name = state.session.tracks[ti].name.clone();
                        let is_master = state.session.tracks[ti].kind.is_master();
                        state.selected_track = Some(id);
                        state.sync_selected_plugin();
                        if is_master {
                            ui.label(
                                RichText::new("Master can’t be deleted")
                                    .size(11.0)
                                    .color(theme.text_muted()),
                            );
                        } else if ui
                            .button(format!("Delete “{name}”"))
                            .on_hover_text("Remove this track")
                            .clicked()
                        {
                            state.request_remove_track(id);
                            ui.close_menu();
                        }
                    });
                    if ui_i > 0 {
                        if sense.drag_started() {
                            let d = TrackDrag { from_ui: ui_i };
                            ui.ctx().data_mut(|data| data.insert_temp(drag_key, d));
                            dragging = Some(d);
                        }
                        if sense.dragged() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                        } else if sense.hovered() && dragging.is_none() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                        }
                    }
                }

                if let (Some(_), Some(db)) = (dragging, drop_before) {
                    if db == order.len() {
                        // After last strip — paint at cursor edge is fine via last loop.
                    }
                }

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

    let released = ui.input(|i| i.pointer.any_released());
    if released {
        if let Some(drag) = dragging {
            let to = drop_before.unwrap_or(drag.from_ui).max(1);
            if state.session.reorder_tracks_ui(drag.from_ui, to) {
                did_reorder = true;
            }
        }
        ui.ctx()
            .data_mut(|d| d.remove_temp::<TrackDrag>(drag_key));
    }
    if did_reorder {
        state.dirty = true;
        state.status = "Tracks reordered".into();
    }
}

/// Right channel rack — SidePanel with stable width sync (no fight with egui memory).
pub fn draw_channel_rack_panel(ctx: &egui::Context, state: &mut AppState) {
    let theme = state.theme;
    let min_w = 300.0_f32;
    let max_w = 560.0_f32;
    state.rack_width = state.rack_width.clamp(min_w, max_w);

    let resp = egui::SidePanel::right("buschain_channel_rack")
        .resizable(true)
        .default_width(state.rack_width)
        .width_range(min_w..=max_w)
        .frame(
            egui::Frame::NONE
                .fill(theme.bg_panel())
                .stroke(egui::Stroke::new(1.0_f32, theme.border()))
                .inner_margin(egui::Margin::same(10)),
        )
        .show(ctx, |ui| {
            // Opaque fill already on the frame — keep content clipped to panel.
            ui.set_clip_rect(ui.max_rect());
            egui::ScrollArea::vertical()
                .id_salt("channel_rack_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    draw_side_panel(ui, state);
                });
        });

    // Only persist width while the user is dragging the resize handle.
    if resp.response.dragged() || resp.response.drag_stopped() {
        state.rack_width = resp.response.rect.width().clamp(min_w, max_w);
    }
}

/// Collapsible bottom analyzer — peak spectrum of the selected track.
/// Height is owned by AppState (`exact_height`); a custom top grab strip resizes it.
/// (egui's built-in panel resize stores content-rect height and fights the drag.)
pub fn draw_mixer_analyzer_panel(ctx: &egui::Context, state: &mut AppState) {
    let theme = state.theme;
    let open = state.mixer_analyzer_open;
    let min_h = 96.0_f32;
    let max_h = 420.0_f32;
    let bar_h = 28.0_f32;
    state.mixer_analyzer_height = state.mixer_analyzer_height.clamp(min_h, max_h);
    let panel_h = if open {
        state.mixer_analyzer_height
    } else {
        bar_h
    };

    let frame = egui::Frame::NONE
        .fill(theme.bg_panel())
        .stroke(egui::Stroke::new(1.0_f32, theme.border_soft()))
        .inner_margin(egui::Margin::symmetric(10, 4));

    egui::TopBottomPanel::bottom("mixer_analyzer_v5")
        .exact_height(panel_h)
        .resizable(false)
        .frame(frame)
        .show(ctx, |ui| {
            if open {
                // Custom resize grab along the top edge (drag up = taller).
                let full = ui.max_rect();
                let grab = egui::Rect::from_x_y_ranges(
                    full.x_range(),
                    egui::Rangef::new(full.top() - 2.0, full.top() + 7.0),
                );
                let grab_resp = ui.interact(
                    grab,
                    egui::Id::new("mixer_analyzer_resize_grab"),
                    Sense::drag(),
                );
                if grab_resp.dragged() {
                    state.mixer_analyzer_height = (state.mixer_analyzer_height
                        - grab_resp.drag_delta().y)
                        .clamp(min_h, max_h);
                }
                if grab_resp.hovered() || grab_resp.dragged() {
                    ui.ctx()
                        .set_cursor_icon(egui::CursorIcon::ResizeVertical);
                    ui.painter().hline(
                        full.x_range(),
                        full.top() + 1.0,
                        egui::Stroke::new(2.0_f32, theme.accent().gamma_multiply(0.85)),
                    );
                } else {
                    ui.painter().hline(
                        full.x_range(),
                        full.top() + 1.0,
                        egui::Stroke::new(1.0_f32, theme.border_soft()),
                    );
                }
            }

            ui.horizontal(|ui| {
                let open = state.mixer_analyzer_open;
                let label = if open { "▾ Analyzer" } else { "▸ Analyzer" };
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(label)
                                .size(11.0)
                                .strong()
                                .color(theme.accent()),
                        )
                        .fill(Color32::TRANSPARENT)
                        .stroke(egui::Stroke::NONE),
                    )
                    .on_hover_text(if open {
                        "Collapse analyzer"
                    } else {
                        "Expand analyzer"
                    })
                    .clicked()
                {
                    state.mixer_analyzer_open = !state.mixer_analyzer_open;
                }
                let (track_name, peak, _, _) = selected_track_analyzer_data(state);
                ui.label(
                    RichText::new(track_name)
                        .size(11.0)
                        .color(theme.text_dim()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let txt = if peak <= -89.0 {
                        "— dB".into()
                    } else {
                        format!("{peak:+.0} dB")
                    };
                    ui.label(
                        RichText::new(txt)
                            .size(10.0)
                            .monospace()
                            .color(theme.text_muted()),
                    );
                });
            });

            if state.mixer_analyzer_open {
                ui.add_space(2.0);
                let (_track_name, peak, bands, out_g) = selected_track_analyzer_data(state);
                let w = ui.available_width();
                let h = (ui.available_height() - 2.0).max(48.0);
                design::track_analyzer(
                    ui,
                    &theme,
                    peak,
                    bands.as_deref(),
                    out_g,
                    Vec2::new(w, h),
                );
            }
        });
}

fn selected_track_analyzer_data(
    state: &AppState,
) -> (String, f32, Option<Vec<design::PeqBand>>, f32) {
    let Some(tid) = state.selected_track else {
        return ("(no track)".into(), -90.0, None, 0.0);
    };
    let Some(track) = state.session.tracks.iter().find(|t| t.id == tid) else {
        return ("(no track)".into(), -90.0, None, 0.0);
    };
    let key = track
        .sink_name
        .clone()
        .unwrap_or_else(|| track.expected_sink_name());
    let peak = if let Some((_pre, post)) = crate::audio::engine_handle::host_meter_peaks(&key) {
        if post > 1e-8 {
            (20.0 * post.log10()).clamp(-90.0, 12.0)
        } else if _pre > 1e-8 {
            (20.0 * _pre.log10()).clamp(-90.0, 12.0)
        } else {
            state.meters.peak_db(&key)
        }
    } else {
        state.meters.peak_db(&key)
    };

    // EQ curve only when the selected insert is Parametric EQ — never from other plugs.
    let mut out_g = 0.0;
    let bands = state
        .selected_plugin_slot
        .and_then(|sid| track.inserts.iter().find(|p| p.slot_id == sid))
        .filter(|p| {
            !p.bypass
                && ui_spec_for_ref(p)
                    .map(|s| s.label == "buschain_eq8")
                    .unwrap_or(false)
        })
        .map(|plug| {
            out_g = plug.param("Output (dB)").unwrap_or(0.0);
            (1..=8)
                .map(|n| design::PeqBand {
                    on: plug.param(&format!("B{n} On")).unwrap_or(1.0) >= 0.5,
                    freq: plug.param(&format!("B{n} Freq")).unwrap_or(1000.0),
                    gain_db: plug.param(&format!("B{n} Gain")).unwrap_or(0.0),
                    q: plug.param(&format!("B{n} Q")).unwrap_or(0.707),
                    mode: plug.param(&format!("B{n} Type")).unwrap_or(0.0).round() as i32,
                })
                .collect::<Vec<_>>()
        });

    (track.name.clone(), peak, bands, out_g)
}

fn draw_strip(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    strip_h: f32,
    fader_h: f32,
    dragging: bool,
    is_master_slot: bool,
) -> egui::Rect {
    let _ = is_master_slot;
    let theme = state.theme;
    let track_id = state.session.tracks[track_idx].id;
    let (name, kind) = {
        let t = &state.session.tracks[track_idx];
        (t.name.clone(), t.kind)
    };

    let selected = state.selected_track == Some(track_id);
    let fill = if dragging {
        theme.bg_elevated().gamma_multiply(0.9)
    } else if selected {
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
            if dragging || selected {
                theme.accent().gamma_multiply(0.55)
            } else {
                theme.border_soft()
            },
        ),
        egui::StrokeKind::Inside,
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
            ui.add_space(6.0);

            // ON LED only — delete via right-click on the strip header.
            let mut live = !state.session.tracks[track_idx].mute;
            let has_sink = state.session.tracks[track_idx].sink_name.is_some();
            if design::track_on_led(ui, &theme, &mut live).changed() {
                state.session.tracks[track_idx].mute = !live;
                state.selected_track = Some(track_id);
                state.dirty = true;
                if has_sink {
                    state.schedule_levels();
                } else {
                    state.commit(crate::audio::LiveChange::Reconcile);
                }
            }
        });
    });

    outer
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

    let input_salt = state.session.tracks[track_idx].id;
    egui::ComboBox::from_id_salt(format!("input_{input_salt}"))
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
        state.dirty = true;
        // Sync worker last_session so idle enforce doesn't re-pin this app.
        state
            .worker
            .send(crate::audio::worker::Command::ApplyLevels(
                state.session.clone(),
            ));
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
    let track_id_salt = state.session.tracks[track_idx].id;
    let choices: Vec<(String, String, Option<String>)> = live
        .into_iter()
        .filter(|(k, _, _)| {
            !assigned_now
                .iter()
                .any(|a| a == k || keys_same_app(a, k))
        })
        .collect();

    let mut pick: Option<String> = None;
    egui::ComboBox::from_id_salt(format!("app_assign_{track_id_salt}"))
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
        state.dirty = true;
        // Sync pins into worker before place — idle enforce uses last_session.
        state
            .worker
            .send(crate::audio::worker::Command::ApplyLevels(
                state.session.clone(),
            ));
        // Ensure bus exists (coalesce runs Ensure before PlaceApp in-batch).
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
            let title = plugin_title_for_ref(plug);
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

        // Remove (+ native editor on secondary click via hover menu)
        ui.scope_builder(egui::UiBuilder::new().max_rect(btn_rect), |ui| {
            let rm = design::text_tool_button(ui, &theme, "×").on_hover_text("Remove");
            if rm.clicked() {
                remove = Some(i);
            }
            rm.context_menu(|ui| {
                if state.insert_supports_native_editor(tid, slot_id) {
                    if ui.button("Reopen native editor").clicked() {
                        state.open_native_editor(tid, slot_id);
                        ui.close_menu();
                    }
                    if ui.button("Show converted params").clicked() {
                        state.open_plugin_window(tid, slot_id);
                        if let Some(w) = state
                            .plugin_windows
                            .iter_mut()
                            .find(|w| w.track_id == tid && w.slot_id == slot_id)
                        {
                            w.show_converted_ui = true;
                        }
                        ui.close_menu();
                    }
                }
            });
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
    if let Some(spec) = ui_spec_for_ref(plug) {
        draw_static_plugin_params(ui, state, track_idx, insert_idx, spec);
        return;
    }
    if let Some(dyn_spec) = dynamic_ui_for_ref(plug) {
        draw_dynamic_plugin_params(ui, state, track_idx, insert_idx, &dyn_spec);
        return;
    }
    ui.label(
        RichText::new("Unknown plugin — no parameter map")
            .size(11.0)
            .color(theme.danger()),
    );
}

/// Live peak for an insert window — host post when available, else Pulse strip.
fn insert_peak_db(state: &AppState, track_idx: usize, insert_idx: usize) -> f32 {
    let powered_off = state
        .session
        .tracks
        .get(track_idx)
        .and_then(|t| t.inserts.get(insert_idx))
        .map(|p| p.bypass)
        .unwrap_or(true);
    if powered_off {
        return -90.0;
    }
    let Some(track) = state.session.tracks.get(track_idx) else {
        return -90.0;
    };
    let key = track
        .sink_name
        .clone()
        .unwrap_or_else(|| track.expected_sink_name());
    if let Some((_pre, post)) = crate::audio::engine_handle::host_meter_peaks(&key) {
        if post > 1e-8 {
            return (20.0 * post.log10()).clamp(-90.0, 12.0);
        }
        if _pre > 1e-8 {
            return (20.0 * _pre.log10()).clamp(-90.0, 12.0);
        }
    }
    state.meters.peak_db(&key)
}

fn draw_static_plugin_params(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
    spec: &'static crate::audio::plugin::PluginUiSpec,
) {
    let label = spec.label;
    let mut changed = false;
    let track_id = state.session.tracks[track_idx].id;

    // Dry/wet is on the channel-rack entry knob (PluginRef.mix), not here.
    changed |= match label {
        "buschain_softclip" => draw_softclip_panel(ui, state, track_idx, insert_idx),
        "buschain_overdrive" => draw_overdrive_panel(ui, state, track_idx, insert_idx),
        "buschain_eq8" => draw_peq_panel(ui, state, track_idx, insert_idx),
        "buschain_compressor" => draw_compressor_panel(ui, state, track_idx, insert_idx),
        "buschain_limiter" => draw_limiter_panel(ui, state, track_idx, insert_idx),
        "buschain_gate" => draw_gate_panel(ui, state, track_idx, insert_idx),
        "buschain_pitch" => draw_pitch_panel(ui, state, track_idx, insert_idx),
        "buschain_eq" => draw_eq1_panel(ui, state, track_idx, insert_idx),
        "buschain_denoiser" => draw_denoiser_panel(ui, state, track_idx, insert_idx),
        _ => {
            let theme = state.theme;
            let peak = insert_peak_db(state, track_idx, insert_idx);
            let mut row_changed = false;
            design::inhouse_shell(ui, &theme, spec.title, "BusChain", peak, |ui| {
                for def in spec.params.iter().filter(|p| {
                    p.key != "Mix"
                        && p.key != "Bypass"
                        && p.key != "Output (dB)"
                        && p.key != "Post Gain"
                }) {
                    if draw_param_row(ui, state, track_idx, insert_idx, def) {
                        row_changed = true;
                    }
                }
            });
            row_changed
        }
    };

    if changed {
        state.schedule_fx_params(track_id);
    }
}

fn draw_dynamic_plugin_params(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
    spec: &crate::audio::plugin::DynamicPluginUiSpec,
) {
    let track_id = state.session.tracks[track_idx].id;
    let mut changed = false;
    for def in spec
        .params
        .iter()
        .filter(|p| p.key != "Mix" && p.key != "Bypass")
    {
        changed |= draw_owned_param_row(ui, state, track_idx, insert_idx, def);
    }

    // P7 — sidechain source (bus monitor tap); fingerprint invalidates on change.
    draw_sidechain_picker(ui, state, track_idx, insert_idx);

    if changed {
        state.schedule_fx_params(track_id);
    }
}

/// Sidechain source combo (P7). Also used from slim VST3 host chrome.
pub(crate) fn draw_sidechain_picker(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
) {
    let theme = state.theme;
    let track_id = state.session.tracks[track_idx].id;
    ui.add_space(6.0);
    ui.label(
        RichText::new("Sidechain source")
            .size(10.0)
            .strong()
            .color(theme.text_dim()),
    );
    let current = state.session.tracks[track_idx].inserts[insert_idx]
        .sidechain_from
        .clone();
    let mut pick = current.clone().unwrap_or_default();
    egui::ComboBox::from_id_salt(("sidechain", track_id, insert_idx))
        .selected_text(if pick.is_empty() {
            "None (main input)".to_string()
        } else {
            pick.clone()
        })
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(pick.is_empty(), "None (main input)")
                .clicked()
            {
                pick.clear();
            }
            for t in &state.session.tracks {
                let name = t.expected_sink_name();
                if ui.selectable_label(pick == name, &t.name).clicked() {
                    pick = name;
                }
            }
        });
    let new_sc = if pick.is_empty() {
        None
    } else {
        Some(pick)
    };
    if new_sc != current {
        state.session.tracks[track_idx].inserts[insert_idx].sidechain_from = new_sc;
        state.dirty = true;
        state.schedule_fx_rewire(track_id);
        state.status = "Sidechain source updated — rack rewire…".into();
    }
}

fn inhouse_knob_col(
    ui: &mut egui::Ui,
    theme: &dyn Theme,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    size: f32,
) -> bool {
    let mut changed = false;
    ui.allocate_ui_with_layout(
        Vec2::new(size + 12.0, size + 28.0),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            if design::knob_sized(ui, theme, value, range, "", size).changed() {
                changed = true;
            }
            ui.label(
                RichText::new(label)
                    .size(9.0)
                    .color(theme.text_dim()),
            );
        },
    );
    changed
}

/// Soft Clipper — knobs · meters · transfer curve (BusChain console).
fn draw_softclip_panel(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
) -> bool {
    let theme = state.theme;
    let peak_db = insert_peak_db(state, track_idx, insert_idx);
    let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
    plug.ensure_params();
    let mut thres = plug.param("Threshold").unwrap_or(0.5);
    let mut post = plug.param("Post").unwrap_or(1.0);
    let mut changed = false;

    // Stacked knobs on the left · meters · tall transfer plot filling the rest.
    let panel_h = 200.0_f32;
    design::inhouse_shell_ex(
        ui,
        &theme,
        "Soft Clipper",
        "tanh knee",
        peak_db,
        false, // meters live in the transfer-curve row
        |ui| {
            ui.set_max_width(420.0);
            ui.horizontal(|ui| {
                ui.set_height(panel_h);
                ui.spacing_mut().item_spacing.x = 8.0;

                // Stacked THRES / GAIN column
                ui.allocate_ui_with_layout(
                    Vec2::new(72.0, panel_h),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.add_space(4.0);
                        if inhouse_knob_col(ui, &theme, "THRES", &mut thres, 0.05..=1.0, 48.0) {
                            changed = true;
                        }
                        ui.add_space(10.0);
                        if design::knob_sized(ui, &theme, &mut post, 0.0..=4.0, "", 48.0)
                            .changed()
                        {
                            changed = true;
                        }
                        ui.label(
                            RichText::new("GAIN")
                                .size(9.0)
                                .color(theme.text_dim()),
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
                                    theme.meter_orange()
                                } else {
                                    theme.text_dim()
                                }),
                        );
                        ui.add_space(8.0);
                        // Quiet mode hint — not an FL-style split badge.
                        ui.label(
                            RichText::new("knee")
                                .size(9.0)
                                .color(theme.text_muted()),
                        );
                    },
                );

                design::softclip_meters(
                    ui,
                    &theme,
                    peak_db,
                    Vec2::new(22.0, panel_h - 4.0),
                );

                // Plot takes remaining width and full column height.
                let plot_w = (ui.available_width() - 4.0).clamp(200.0, 300.0);
                if design::softclip_transfer_plot(
                    ui,
                    &theme,
                    &mut thres,
                    post,
                    peak_db,
                    Vec2::new(plot_w, panel_h - 4.0),
                ) {
                    changed = true;
                }
            });
        },
    );

    if changed {
        let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
        plug.set_param("Threshold", thres);
        plug.set_param("Post", post);
    }
    changed
}

/// Theatre Drive — cinema overdrive + waveshaper viz (BusChain console).
fn draw_overdrive_panel(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
) -> bool {
    let theme = state.theme;
    let peak_db = insert_peak_db(state, track_idx, insert_idx);
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

    let panel_h = 228.0_f32;
    design::inhouse_shell(
        ui,
        &theme,
        "Theatre Drive",
        "drive bass @ 100 Hz",
        peak_db,
        |ui| {
            ui.set_max_width(620.0);
            ui.horizontal(|ui| {
                ui.set_min_height(panel_h);
                ui.spacing_mut().item_spacing.x = 10.0;

                // Controls column
                ui.vertical(|ui| {
                    ui.set_max_width(348.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        for (label, val, range) in [
                            ("PRE", &mut preband, 0.0..=1.0_f32),
                            ("DRIVE", &mut drive, 0.0..=1.0),
                            ("BIAS", &mut bias, -1.0..=1.0),
                            ("LEVEL", &mut postg, 0.0..=1.0),
                        ] {
                            if inhouse_knob_col(ui, &theme, label, val, range, 46.0) {
                                changed = true;
                            }
                        }
                    });

                    ui.add_space(4.0);
                    for (lab, val, range) in [
                        ("Color", &mut color, 40.0..=8000.0_f32),
                        ("Post cut", &mut postf, 200.0..=20000.0),
                        ("Split", &mut split, 20.0..=500.0),
                    ] {
                        ui.horizontal(|ui| {
                            ui.set_max_width(348.0);
                            ui.label(RichText::new(lab).size(10.0).color(theme.text_dim()));
                            ui.scope(|ui| {
                                ui.set_max_width(220.0);
                                if design::slider_drag(ui, val, range.clone(), |s| {
                                    s.logarithmic(true).suffix(" Hz")
                                })
                                .changed()
                                {
                                    changed = true;
                                }
                            });
                        });
                    }

                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.set_max_width(348.0);
                        ui.label(RichText::new("Focus").size(10.0).color(theme.text_dim()));
                        let before = focus;
                        egui::ComboBox::from_id_salt(format!("od_focus_{track_idx}_{insert_idx}"))
                            .width(110.0)
                            .selected_text(
                                ["Full", "Drive Bass", "Protect Bass"]
                                    .get(focus as usize)
                                    .copied()
                                    .unwrap_or("?"),
                            )
                            .show_ui(ui, |ui| {
                                for (i, name) in
                                    ["Full", "Drive Bass", "Protect Bass"].iter().enumerate()
                                {
                                    ui.selectable_value(&mut focus, i as i32, *name);
                                }
                            });
                        if focus != before {
                            changed = true;
                        }

                        ui.label(RichText::new("Char").size(10.0).color(theme.text_dim()));
                        let before = character;
                        egui::ComboBox::from_id_salt(format!("od_char_{track_idx}_{insert_idx}"))
                            .width(72.0)
                            .selected_text(
                                ["Tube", "Soft", "Hard", "Diode"]
                                    .get(character as usize)
                                    .copied()
                                    .unwrap_or("?"),
                            )
                            .show_ui(ui, |ui| {
                                for (i, name) in ["Tube", "Soft", "Hard", "Diode"].iter().enumerate()
                                {
                                    ui.selectable_value(&mut character, i as i32, *name);
                                }
                            });
                        if character != before {
                            changed = true;
                        }

                        ui.label(RichText::new("Pre").size(10.0).color(theme.text_dim()));
                        let before = preshape;
                        egui::ComboBox::from_id_salt(format!("od_pre_{track_idx}_{insert_idx}"))
                            .width(88.0)
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

                        if design::toggle_chip(ui, &theme, "×10", &mut boost, theme.accent())
                            .changed()
                        {
                            changed = true;
                        }
                    });
                });

                // Waveshaper viz — same rice as Soft Clipper.
                let plot_w = (ui.available_width() - 4.0).clamp(200.0, 260.0);
                design::theatre_drive_plot(
                    ui,
                    &theme,
                    drive,
                    boost,
                    bias,
                    character,
                    postg,
                    peak_db,
                    Vec2::new(plot_w, panel_h - 4.0),
                );
            });
        },
    );

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

/// Parametric EQ — spectrum backdrop + interactive graph (BusChain console).
fn draw_peq_panel(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
) -> bool {
    let theme = state.theme;
    let peak_db = insert_peak_db(state, track_idx, insert_idx);
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

    design::inhouse_shell(
        ui,
        &theme,
        "Parametric EQ",
        "drag nodes · snap @ 0 dB",
        peak_db,
        |ui| {
            let graph_w = ui.available_width().clamp(240.0, 560.0);
            if design::peq_graph(
                ui,
                &theme,
                &mut bands,
                out_gain,
                &mut selected,
                Vec2::new(graph_w, 168.0),
                peak_db,
            ) {
                changed = true;
            }

            ui.add_space(6.0);

            // Output fader LEFT of band gain faders (taller throws).
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.vertical(|ui| {
                    ui.set_width(40.0);
                    ui.label(
                        RichText::new("OUT")
                            .size(9.0)
                            .strong()
                            .color(theme.accent()),
                    );
                    if design::fader_db(
                        ui,
                        &theme,
                        &mut out_gain,
                        -24.0..=24.0,
                        Vec2::new(30.0, 140.0),
                    )
                    .changed()
                    {
                        changed = true;
                    }
                    ui.label(
                        RichText::new(format!("{out_gain:+.1}"))
                            .size(9.0)
                            .monospace()
                            .color(theme.accent()),
                    );
                    if ui
                        .add(egui::Button::new("0").small())
                        .on_hover_text("Reset output")
                        .clicked()
                    {
                        out_gain = 0.0;
                        changed = true;
                    }
                });
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
                        if design::fader_db(
                            ui,
                            &theme,
                            &mut g,
                            -24.0..=24.0,
                            Vec2::new(28.0, 140.0),
                        )
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

            ui.add_space(4.0);
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
                    if design::slider_drag(ui, &mut freq, 20.0..=20000.0, |s| {
                        s.logarithmic(true).show_value(true)
                    })
                    .changed()
                    {
                        b.freq = freq;
                        changed = true;
                    }
                    ui.label(RichText::new("Q").size(10.0).color(theme.text_dim()));
                    if design::slider_drag(ui, &mut q, 0.1..=10.0, |s| s.show_value(true))
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
        },
    );

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

fn draw_compressor_panel(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
) -> bool {
    let theme = state.theme;
    let peak_db = insert_peak_db(state, track_idx, insert_idx);
    let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
    plug.ensure_params();
    let mut thresh = plug.param("Threshold (dB)").unwrap_or(-18.0);
    let mut ratio = plug.param("Ratio").unwrap_or(4.0);
    let mut attack = plug.param("Attack (ms)").unwrap_or(10.0);
    let mut release = plug.param("Release (ms)").unwrap_or(100.0);
    let mut makeup = plug.param("Makeup (dB)").unwrap_or(0.0);
    let mut changed = false;

    design::inhouse_shell(
        ui,
        &theme,
        "Compressor",
        "",
        peak_db,
        |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                for (label, val, range) in [
                    ("THRESH", &mut thresh, -60.0..=0.0_f32),
                    ("RATIO", &mut ratio, 1.0..=20.0),
                    ("ATTACK", &mut attack, 0.1..=100.0),
                    ("RELEASE", &mut release, 1.0..=1000.0),
                    ("MAKEUP", &mut makeup, -24.0..=24.0),
                ] {
                    if inhouse_knob_col(ui, &theme, label, val, range, 50.0) {
                        changed = true;
                    }
                }
                ui.add_space(6.0);
                design::plugin_stereo_meters(
                    ui,
                    &theme,
                    peak_db,
                    Vec2::new(24.0, 88.0),
                    ("comp_meters", track_idx, insert_idx),
                );
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{thresh:+.1} dB  ·  {ratio:.1}:1  ·  {attack:.1} ms / {release:.0} ms  ·  makeup {makeup:+.1}"
                    ))
                    .size(10.0)
                    .monospace()
                    .color(theme.text_dim()),
                );
            });
        },
    );

    if changed {
        let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
        plug.set_param("Threshold (dB)", thresh);
        plug.set_param("Ratio", ratio);
        plug.set_param("Attack (ms)", attack);
        plug.set_param("Release (ms)", release);
        plug.set_param("Makeup (dB)", makeup);
    }
    changed
}

fn draw_limiter_panel(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
) -> bool {
    let theme = state.theme;
    let peak_db = insert_peak_db(state, track_idx, insert_idx);
    let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
    plug.ensure_params();
    let mut ceiling = plug.param("Ceiling (dB)").unwrap_or(-0.1);
    let mut release = plug.param("Release (ms)").unwrap_or(50.0);
    let mut changed = false;

    design::inhouse_shell(
        ui,
        &theme,
        "Limiter",
        "",
        peak_db,
        |ui| {
            ui.set_max_width(268.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                if inhouse_knob_col(ui, &theme, "CEILING", &mut ceiling, -24.0..=0.0, 56.0) {
                    changed = true;
                }
                if inhouse_knob_col(ui, &theme, "RELEASE", &mut release, 1.0..=500.0, 56.0) {
                    changed = true;
                }
                design::plugin_stereo_meters(
                    ui,
                    &theme,
                    peak_db,
                    Vec2::new(24.0, 88.0),
                    ("lim_meters", track_idx, insert_idx),
                );
                ui.vertical(|ui| {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!("{ceiling:+.2} dB"))
                            .size(13.0)
                            .monospace()
                            .strong()
                            .color(theme.accent()),
                    );
                    ui.label(
                        RichText::new(format!("{release:.0} ms"))
                            .size(10.0)
                            .color(theme.text_muted()),
                    );
                });
            });
        },
    );

    if changed {
        let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
        plug.set_param("Ceiling (dB)", ceiling);
        plug.set_param("Release (ms)", release);
    }
    changed
}

fn draw_gate_panel(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
) -> bool {
    let theme = state.theme;
    let peak_db = insert_peak_db(state, track_idx, insert_idx);
    let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
    plug.ensure_params();
    // Rack power owns on/off — keep DSP Enable latched on (no in-panel Enable chip).
    if plug.param("Enable").unwrap_or(1.0) < 0.5 {
        plug.set_param("Enable", 1.0);
    }
    let mut thresh = plug.param("Threshold (dB)").unwrap_or(-78.0);
    let mut hyst = plug.param("Hysteresis (dB)").unwrap_or(3.0);
    let mut attack = plug.param("Attack (ms)").unwrap_or(2.0);
    let mut hold = plug.param("Hold (ms)").unwrap_or(80.0);
    let mut release = plug.param("Release (ms)").unwrap_or(120.0);
    let mut range = plug.param("Range (dB)").unwrap_or(100.0);
    let mut mix = plug.param("Mix").unwrap_or(1.0);
    let mut changed = false;

    design::inhouse_shell(
        ui,
        &theme,
        "Gate",
        "dry/wet blends closed dig",
        peak_db,
        |ui| {
            ui.set_max_width(400.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for (label, val, range) in [
                    ("THRESH", &mut thresh, -140.0..=0.0_f32),
                    ("HYST", &mut hyst, 0.0..=24.0),
                    ("ATTACK", &mut attack, 0.1..=50.0),
                    ("HOLD", &mut hold, 0.0..=500.0),
                    ("RELEASE", &mut release, 1.0..=1000.0),
                    ("RANGE", &mut range, 0.0..=140.0),
                    ("DRY/WET", &mut mix, 0.0..=1.0),
                ] {
                    if inhouse_knob_col(ui, &theme, label, val, range, 40.0) {
                        changed = true;
                    }
                }
                design::plugin_stereo_meters(
                    ui,
                    &theme,
                    peak_db,
                    Vec2::new(20.0, 72.0),
                    ("gate_meters", track_idx, insert_idx),
                );
            });
        },
    );

    if changed {
        let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
        plug.set_param("Enable", 1.0);
        plug.set_param("Threshold (dB)", thresh);
        plug.set_param("Hysteresis (dB)", hyst);
        plug.set_param("Attack (ms)", attack);
        plug.set_param("Hold (ms)", hold);
        plug.set_param("Release (ms)", release);
        plug.set_param("Range (dB)", range);
        plug.set_param("Mix", mix);
    }
    changed
}

fn draw_pitch_panel(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
) -> bool {
    let theme = state.theme;
    let peak_db = insert_peak_db(state, track_idx, insert_idx);
    let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
    plug.ensure_params();
    let mut semi = plug.param("Semitones").unwrap_or(0.0);
    let mut cents = plug.param("Cents").unwrap_or(0.0);
    let mut smooth = plug.param("Smooth").unwrap_or(0.65);
    let mut changed = false;

    design::inhouse_shell(
        ui,
        &theme,
        "Pitch",
        "",
        peak_db,
        |ui| {
            ui.set_max_width(268.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                if inhouse_knob_col(ui, &theme, "SEMI", &mut semi, -12.0..=12.0, 52.0) {
                    changed = true;
                }
                if inhouse_knob_col(ui, &theme, "CENTS", &mut cents, -100.0..=100.0, 52.0) {
                    changed = true;
                }
                if inhouse_knob_col(ui, &theme, "SMOOTH", &mut smooth, 0.0..=1.0, 52.0) {
                    changed = true;
                }
                design::plugin_stereo_meters(
                    ui,
                    &theme,
                    peak_db,
                    Vec2::new(20.0, 80.0),
                    ("pitch_meters", track_idx, insert_idx),
                );
            });
            ui.label(
                RichText::new(format!(
                    "{semi:+.0} st  ·  {cents:+.0} ¢  ·  smooth {smooth:.2}"
                ))
                .size(11.0)
                .monospace()
                .color(theme.text_dim()),
            );
        },
    );

    if changed {
        let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
        plug.set_param("Semitones", semi);
        plug.set_param("Cents", cents);
        plug.set_param("Smooth", smooth);
    }
    changed
}

fn draw_eq1_panel(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
) -> bool {
    let theme = state.theme;
    let peak_db = insert_peak_db(state, track_idx, insert_idx);
    let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
    plug.ensure_params();
    let mut freq = plug.param("Freq (Hz)").unwrap_or(1000.0);
    let mut gain = plug.param("Gain (dB)").unwrap_or(0.0);
    let mut q = plug.param("Q").unwrap_or(0.707);
    let mut mode = plug.param("Mode").unwrap_or(0.0).round() as i32;
    let mut changed = false;

    design::inhouse_shell(
        ui,
        &theme,
        "EQ 1-Band",
        "",
        peak_db,
        |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                if inhouse_knob_col(ui, &theme, "FREQ", &mut freq, 20.0..=20000.0, 54.0) {
                    changed = true;
                }
                if inhouse_knob_col(ui, &theme, "GAIN", &mut gain, -24.0..=24.0, 54.0) {
                    changed = true;
                }
                if inhouse_knob_col(ui, &theme, "Q", &mut q, 0.1..=10.0, 54.0) {
                    changed = true;
                }
                design::plugin_stereo_meters(
                    ui,
                    &theme,
                    peak_db,
                    Vec2::new(24.0, 86.0),
                    ("eq1_meters", track_idx, insert_idx),
                );
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Mode").size(10.0).color(theme.text_dim()));
                let before = mode;
                let modes = ["Peak", "Low shelf", "High shelf", "High-pass", "Low-pass"];
                egui::ComboBox::from_id_salt(format!("eq1_mode_{track_idx}_{insert_idx}"))
                    .selected_text(modes.get(mode as usize).copied().unwrap_or("Peak"))
                    .show_ui(ui, |ui| {
                        for (i, name) in modes.iter().enumerate() {
                            ui.selectable_value(&mut mode, i as i32, *name);
                        }
                    });
                if mode != before {
                    changed = true;
                }
                ui.label(
                    RichText::new(format!("{freq:.0} Hz  ·  {gain:+.1} dB  ·  Q {q:.2}"))
                        .size(10.0)
                        .monospace()
                        .color(theme.text_dim()),
                );
            });
        },
    );

    if changed {
        let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
        plug.set_param("Freq (Hz)", freq);
        plug.set_param("Gain (dB)", gain);
        plug.set_param("Q", q);
        plug.set_param("Mode", mode as f32);
    }
    changed
}

fn insert_pre_post_peak_db(state: &AppState, track_idx: usize, insert_idx: usize) -> (f32, f32) {
    let powered_off = state
        .session
        .tracks
        .get(track_idx)
        .and_then(|t| t.inserts.get(insert_idx))
        .map(|p| p.bypass)
        .unwrap_or(true);
    if powered_off {
        return (-90.0, -90.0);
    }
    let Some(track) = state.session.tracks.get(track_idx) else {
        return (-90.0, -90.0);
    };
    let key = track
        .sink_name
        .clone()
        .unwrap_or_else(|| track.expected_sink_name());
    if let Some((pre, post)) = crate::audio::engine_handle::host_meter_peaks(&key) {
        let pre_db = if pre > 1e-8 {
            (20.0 * pre.log10()).clamp(-90.0, 12.0)
        } else {
            -90.0
        };
        let post_db = if post > 1e-8 {
            (20.0 * post.log10()).clamp(-90.0, 12.0)
        } else {
            -90.0
        };
        return (pre_db, post_db);
    }
    let p = state.meters.peak_db(&key);
    (p, p)
}

fn load_nr_nodes(plug: &crate::audio::plugin::PluginRef) -> Vec<design::NrNode> {
    let mut nodes = Vec::new();
    for i in 1..=design::DN_NR_MAX_NODES {
        let on = plug.param(&format!("NR{i} On")).unwrap_or(0.0) >= 0.5;
        let freq = plug.param(&format!("NR{i} Freq (Hz)")).unwrap_or(1000.0);
        let depth = plug.param(&format!("NR{i} Depth (dB)")).unwrap_or(0.0);
        let q = plug.param(&format!("NR{i} Q")).unwrap_or(1.5);
        if on || depth > 0.05 {
            nodes.push(design::NrNode {
                on,
                freq,
                depth_db: depth,
                q,
            });
        }
    }
    if nodes.is_empty() {
        // Migrate legacy band ranges → one bell per band.
        for i in 1..=6 {
            let depth = plug
                .param(&format!("Range Band {i} (dB)"))
                .unwrap_or(0.0);
            let freq = plug.param(&format!("Band {i} Freq (Hz)")).unwrap_or(1000.0);
            if depth > 0.05 {
                nodes.push(design::NrNode {
                    on: true,
                    freq,
                    depth_db: depth,
                    q: 1.5,
                });
            }
        }
    }
    nodes
}

fn store_nr_nodes(plug: &mut crate::audio::plugin::PluginRef, nodes: &[design::NrNode]) {
    for i in 1..=design::DN_NR_MAX_NODES {
        let n = nodes.get(i - 1);
        plug.set_param(&format!("NR{i} On"), if n.map(|x| x.on).unwrap_or(false) { 1.0 } else { 0.0 });
        plug.set_param(
            &format!("NR{i} Freq (Hz)"),
            n.map(|x| x.freq).unwrap_or(1000.0),
        );
        plug.set_param(
            &format!("NR{i} Depth (dB)"),
            n.map(|x| x.depth_db).unwrap_or(0.0),
        );
        plug.set_param(&format!("NR{i} Q"), n.map(|x| x.q).unwrap_or(1.5));
    }
    let (ranges, freqs) = design::nr_nodes_to_bands(nodes);
    for i in 0..6 {
        plug.set_param(&format!("Range Band {} (dB)", i + 1), ranges[i]);
        plug.set_param(&format!("Band {} Freq (Hz)", i + 1), freqs[i]);
    }
}

/// Denoiser — parametric NR bells + dual in/out peak-norm spectra.
fn draw_denoiser_panel(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
) -> bool {
    let theme = state.theme;
    let (in_peak_db, out_peak_db) = insert_pre_post_peak_db(state, track_idx, insert_idx);
    let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
    plug.ensure_params();

    let mut thresh = plug.param("Threshold (dB)").unwrap_or(-71.4583);
    let mut hf = plug.param("HF Bias").unwrap_or(0.40);
    let mut link = plug.param("Stereo Link").unwrap_or(0.90);
    let mut stability = plug.param("Stability").unwrap_or(0.45);
    let mut gate_on = plug.param("Gate Enable").unwrap_or(0.0) >= 0.5;
    let mut gate_knee = plug.param("Gate Knee (dB)").unwrap_or(4.0);
    let mut gate_ratio = plug.param("Gate Ratio").unwrap_or(12.0);
    let mut gate_att = plug.param("Gate Attack (ms)").unwrap_or(1.2);
    let mut gate_rel = plug.param("Gate Release (ms)").unwrap_or(90.0);
    let mut gate_mix = plug.param("Gate Mix").unwrap_or(1.0);
    let mut nodes = load_nr_nodes(plug);
    let mut changed = false;
    let mut apply_name: Option<String> = None;

    let preset_id = egui::Id::new(("dn_preset", track_idx, insert_idx));
    let sel_id = egui::Id::new(("dn_nr_sel", track_idx, insert_idx));
    let mut active_preset: String = ui
        .ctx()
        .data(|d| d.get_temp::<String>(preset_id))
        .unwrap_or_else(|| "default".into());
    let mut selected: usize = ui.ctx().data(|d| d.get_temp(sel_id)).unwrap_or(0);
    if !nodes.is_empty() {
        selected = selected.min(nodes.len() - 1);
    } else {
        selected = 0;
    }

    design::inhouse_shell(
        ui,
        &theme,
        "Denoiser",
        "parametric NR · drag nodes to pull noise",
        out_peak_db,
        |ui| {
            ui.set_max_width(740.0);

            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
                for name in denoiser_preset_names() {
                    let active = active_preset == *name;
                    if design::button(ui, &theme, name, active).clicked() {
                        apply_name = Some((*name).to_string());
                    }
                }
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let map_w = (ui.available_width() - 112.0).clamp(400.0, 620.0);
                if design::denoiser_param_graph(
                    ui,
                    &theme,
                    &mut thresh,
                    &mut nodes,
                    &mut selected,
                    in_peak_db,
                    out_peak_db,
                    Vec2::new(map_w, 260.0),
                ) {
                    changed = true;
                    active_preset.clear();
                }

                // Side column: Thresh (NR floor) + HF / Link / Stability.
                ui.allocate_ui_with_layout(
                    Vec2::new(100.0, 260.0),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.label(
                            RichText::new("THRESH")
                                .size(9.0)
                                .color(theme.text_muted()),
                        );
                        if design::knob_sized(ui, &theme, &mut thresh, -140.0..=0.0, "", 46.0)
                            .changed()
                        {
                            changed = true;
                            active_preset.clear();
                        }
                        ui.label(
                            RichText::new(format!("{thresh:+.0} dB"))
                                .size(10.0)
                                .monospace()
                                .color(theme.text()),
                        );
                        ui.label(
                            RichText::new("dig below")
                                .size(8.0)
                                .color(theme.text_dim()),
                        );
                        ui.add_space(8.0);

                        for (title, hint, val) in [
                            ("HF BIAS", "ease HF pass", &mut hf),
                            ("LINK", "L↔R envelopes", &mut link),
                            ("STABILITY", "tames NR flicker", &mut stability),
                        ] {
                            ui.label(
                                RichText::new(title)
                                    .size(9.0)
                                    .color(theme.text_muted()),
                            );
                            if design::knob_sized(ui, &theme, val, 0.0..=1.0, "", 34.0).changed() {
                                changed = true;
                                active_preset.clear();
                            }
                            ui.label(
                                RichText::new(format!("{:.2}", *val))
                                    .size(9.0)
                                    .monospace()
                                    .color(theme.text()),
                            );
                            ui.label(
                                RichText::new(hint)
                                    .size(8.0)
                                    .color(theme.text_dim()),
                            );
                            ui.add_space(4.0);
                        }
                    },
                );
            });

            ui.add_space(6.0);
            // Selected node editor.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(
                    RichText::new("NODE")
                        .size(9.0)
                        .color(theme.text_muted()),
                );
                if design::button(ui, &theme, "+", false).clicked()
                    && nodes.len() < design::DN_NR_MAX_NODES
                {
                    let f = if let Some(n) = nodes.get(selected) {
                        (n.freq * 1.6).clamp(40.0, 16_000.0)
                    } else {
                        4000.0
                    };
                    nodes.push(design::NrNode {
                        on: true,
                        freq: f,
                        depth_db: 10.0,
                        q: 2.4,
                    });
                    selected = nodes.len() - 1;
                    changed = true;
                    active_preset.clear();
                }
                if design::button(ui, &theme, "−", false).clicked() && !nodes.is_empty() {
                    let i = selected.min(nodes.len() - 1);
                    nodes.remove(i);
                    if !nodes.is_empty() {
                        selected = i.min(nodes.len() - 1);
                    } else {
                        selected = 0;
                    }
                    changed = true;
                    active_preset.clear();
                }

                if let Some(n) = nodes.get_mut(selected) {
                    ui.label(
                        RichText::new(format!("N{}", selected + 1))
                            .size(11.0)
                            .strong()
                            .color(theme.accent()),
                    );
                    let mut f = n.freq;
                    let mut d = n.depth_db;
                    let mut q = n.q;
                    ui.label(RichText::new("Hz").size(9.0).color(theme.text_dim()));
                    let mut rf = ui.add(
                        egui::DragValue::new(&mut f)
                            .range(20.0..=20_000.0)
                            .speed(2.0)
                            .suffix(" Hz"),
                    );
                    if design::apply_wheel_to_value(ui, &mut rf, &mut f, 20.0..=20_000.0)
                        || rf.changed()
                    {
                        n.freq = f;
                        changed = true;
                        active_preset.clear();
                    }
                    ui.label(RichText::new("depth").size(9.0).color(theme.text_dim()));
                    let mut rd = ui.add(
                        egui::DragValue::new(&mut d)
                            .range(0.0..=48.0)
                            .speed(0.1)
                            .prefix("−")
                            .suffix(" dB"),
                    );
                    if design::apply_wheel_to_value(ui, &mut rd, &mut d, 0.0..=48.0) || rd.changed()
                    {
                        n.depth_db = d;
                        n.on = d > 0.05;
                        changed = true;
                        active_preset.clear();
                    }
                    ui.label(RichText::new("Q").size(9.0).color(theme.text_dim()));
                    let mut rq =
                        ui.add(egui::DragValue::new(&mut q).range(0.3..=12.0).speed(0.05));
                    if design::apply_wheel_to_value(ui, &mut rq, &mut q, 0.3..=12.0) || rq.changed()
                    {
                        n.q = q;
                        changed = true;
                        active_preset.clear();
                    }
                    let mut on = n.on;
                    if design::toggle_chip(ui, &theme, "ON", &mut on, theme.accent()).changed() {
                        n.on = on;
                        changed = true;
                        active_preset.clear();
                    }
                } else {
                    ui.label(
                        RichText::new("double-click graph to add a pull node")
                            .size(10.0)
                            .color(theme.text_muted()),
                    );
                }
            });

            ui.add_space(4.0);
            // Optional gate — clearly separate from spectral NR.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(
                    RichText::new("GATE")
                        .size(9.0)
                        .color(theme.text_muted()),
                );
                if design::toggle_chip(ui, &theme, if gate_on { "ON" } else { "OFF" }, &mut gate_on, theme.accent())
                    .changed()
                {
                    changed = true;
                    active_preset.clear();
                }
                if !gate_on {
                    ui.label(
                        RichText::new("optional level dig — leave off for nature / leaves")
                            .size(9.0)
                            .color(theme.text_dim()),
                    );
                }
            });
            if gate_on {
                ui.label(
                    RichText::new("uses master THRESH · deeper dig under the floor")
                        .size(9.0)
                        .color(theme.text_dim()),
                );
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if inhouse_knob_col(ui, &theme, "KNEE", &mut gate_knee, 0.0..=24.0, 34.0) {
                        changed = true;
                        active_preset.clear();
                    }
                    if inhouse_knob_col(ui, &theme, "RATIO", &mut gate_ratio, 1.0..=100.0, 34.0) {
                        changed = true;
                        active_preset.clear();
                    }
                    if inhouse_knob_col(ui, &theme, "ATT ms", &mut gate_att, 0.1..=50.0, 34.0) {
                        changed = true;
                        active_preset.clear();
                    }
                    if inhouse_knob_col(ui, &theme, "REL ms", &mut gate_rel, 5.0..=1000.0, 34.0) {
                        changed = true;
                        active_preset.clear();
                    }
                    if inhouse_knob_col(ui, &theme, "DRY/WET", &mut gate_mix, 0.0..=1.0, 34.0) {
                        changed = true;
                        active_preset.clear();
                    }
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(format!("mix {:.0}%", gate_mix * 100.0))
                                .size(10.0)
                                .monospace()
                                .color(theme.text()),
                        );
                        ui.label(
                            RichText::new(format!("{gate_ratio:.0}:1 · {gate_att:.1}/{gate_rel:.0} ms"))
                                .size(9.0)
                                .color(theme.text_dim()),
                        );
                    });
                });
            }
        },
    );

    ui.ctx().data_mut(|d| {
        d.insert_temp(sel_id, selected);
        if !active_preset.is_empty() {
            d.insert_temp(preset_id, active_preset.clone());
        } else {
            d.remove_temp::<String>(preset_id);
        }
    });

    let applied_preset = if let Some(ref name) = apply_name {
        if apply_denoiser_preset(
            &mut state.session.tracks[track_idx].inserts[insert_idx],
            name,
        ) {
            // Rebuild NR nodes from the preset’s band table.
            let plug = &state.session.tracks[track_idx].inserts[insert_idx];
            let mut rebuilt = Vec::new();
            for i in 1..=6 {
                let depth = plug.param(&format!("Range Band {i} (dB)")).unwrap_or(0.0);
                let freq = plug.param(&format!("Band {i} Freq (Hz)")).unwrap_or(1000.0);
                if depth > 0.05 {
                    rebuilt.push(design::NrNode {
                        on: true,
                        freq,
                        depth_db: depth,
                        q: 1.5,
                    });
                }
            }
            let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
            store_nr_nodes(plug, &rebuilt);
            state.status = format!("Denoiser preset “{name}”");
            ui.ctx()
                .data_mut(|d| d.insert_temp(preset_id, name.clone()));
            true
        } else {
            false
        }
    } else {
        false
    };

    if changed && !applied_preset {
        let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
        plug.set_param("Threshold (dB)", thresh);
        plug.set_param("HF Bias", hf);
        plug.set_param("Stereo Link", link);
        plug.set_param("Stability", stability);
        plug.set_param("Gate Enable", if gate_on { 1.0 } else { 0.0 });
        plug.set_param("Gate Knee (dB)", gate_knee);
        plug.set_param("Gate Ratio", gate_ratio);
        plug.set_param("Gate Attack (ms)", gate_att);
        plug.set_param("Gate Release (ms)", gate_rel);
        plug.set_param("Gate Mix", gate_mix);
        store_nr_nodes(plug, &nodes);
    }
    changed || applied_preset
}

fn draw_owned_param_row(
    ui: &mut egui::Ui,
    state: &mut AppState,
    track_idx: usize,
    insert_idx: usize,
    def: &OwnedParamDef,
) -> bool {
    let theme = state.theme;
    let plug = &mut state.session.tracks[track_idx].inserts[insert_idx];
    let mut value = plug.param(&def.key).unwrap_or(def.default);
    let mut changed = false;

    match def.kind {
        ParamKind::Toggle => {
            let mut on = value >= 0.5;
            ui.horizontal(|ui| {
                if design::toggle_chip(ui, &theme, &def.label, &mut on, theme.accent()).changed() {
                    value = if on { 1.0 } else { 0.0 };
                    changed = true;
                }
            });
        }
        ParamKind::Mode => {
            let modes: Vec<&str> = def.modes.as_ref().map(|m| m.iter().map(|s| s.as_str()).collect()).unwrap_or_default();
            let mut idx = value.round().clamp(def.min, def.max) as usize;
            let before = idx;
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&def.label)
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
                    RichText::new(&def.label)
                        .size(10.0)
                        .color(theme.text_dim())
                        .strong(),
                );
                let log = def.logarithmic;
                if design::slider_drag(ui, &mut value, def.min..=def.max, |s| {
                    let s = s.show_value(true).min_decimals(0).max_decimals(2);
                    if log {
                        s.logarithmic(true)
                    } else {
                        s
                    }
                })
                .changed()
                {
                    changed = true;
                }
            });
        }
    }

    if changed {
        plug.set_param(&def.key, value.clamp(def.min, def.max));
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
                let log = def.logarithmic;
                if design::slider_drag(ui, &mut value, def.min..=def.max, |s| {
                    let s = s.show_value(true).min_decimals(0).max_decimals(2);
                    if log {
                        s.logarithmic(true)
                    } else {
                        s
                    }
                })
                .changed()
                {
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
    let vst3_on = state.session.vst3_enabled;
    let plugins: Vec<(PluginId, String, PluginFormat)> = state
        .plugins
        .plugins_for_mixer_add(vst3_on)
        .into_iter()
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

    let mut groups: Vec<(&str, Vec<(PluginId, String, PluginFormat)>)> = PLUGIN_GROUP_ORDER
        .iter()
        .map(|g| (*g, Vec::new()))
        .collect();
    for (id, name, fmt) in plugins {
        let g = plugin_menu_group(&id.id, fmt);
        if let Some((_, bucket)) = groups.iter_mut().find(|(n, _)| *n == g) {
            bucket.push((id, name, fmt));
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
                    ui.set_min_width(260.0);
                    ui.label(
                        RichText::new("Choose an insert")
                            .size(11.0)
                            .color(theme.text_muted()),
                    );
                    ui.label(
                        RichText::new("LADSPA · CLAP · LV2 · VST3 from scan")
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
                        for (id, name, fmt) in items {
                            let label = match fmt {
                                PluginFormat::Ladspa => name.clone(),
                                _ => format!("[{}] {name}", format_short_name(*fmt)),
                            };
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(label).size(12.0).color(theme.text()),
                                    )
                                    .fill(Color32::TRANSPARENT)
                                    .min_size(Vec2::new(240.0, 22.0)),
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
        if !state.status.starts_with("Loading audio graph")
            && !state.status.starts_with("Live graph bring-up")
        {
            state.status = format!("Added {name} — live slot rewire…");
        }
    }
}

fn format_short_name(fmt: PluginFormat) -> &'static str {
    match fmt {
        PluginFormat::Ladspa => "LADSPA",
        PluginFormat::Lv2 => "LV2",
        PluginFormat::Clap => "CLAP",
        PluginFormat::Vst3 => "VST3",
    }
}
