//! Settings — categorical panels (Appearance, Audio, Plugins, Session, Advanced).

use crate::app_state::AppState;
use crate::design::{self, SpectrumTheme, Theme};
use egui::{ecolor::Hsva, RichText};

const SETTINGS_CATS: &[&str] = &[
    "Appearance",
    "Audio",
    "Plugins",
    "Session",
    "Advanced",
];

/// Top-level Settings tab (replaces the old single Configuration scroll).
pub fn draw_config(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    design::page_header(
        ui,
        &theme,
        "Settings",
        "Appearance, audio clock, plugin paths, session prefs, and advanced toggles.",
    );

    // Take the full CentralPanel height. A plain horizontal()+ScrollArea only
    // sized itself to the short category rail and left a huge empty gap.
    let avail = ui.available_size();
    ui.allocate_ui_with_layout(
        avail,
        egui::Layout::left_to_right(egui::Align::Min),
        |ui| {
            ui.vertical(|ui| {
                ui.set_width(148.0);
                ui.set_min_height(avail.y);
                design::panel(ui, &theme, |ui| {
                    ui.label(
                        RichText::new("Categories")
                            .size(10.0)
                            .color(theme.text_muted()),
                    );
                    ui.add_space(4.0);
                    for (i, name) in SETTINGS_CATS.iter().enumerate() {
                        let on = state.settings_tab == i;
                        let fill = if on {
                            theme.accent().gamma_multiply(0.22)
                        } else {
                            egui::Color32::TRANSPARENT
                        };
                        let text = if on {
                            theme.text()
                        } else {
                            theme.text_muted()
                        };
                        let resp = ui.add_sized(
                            [ui.available_width(), 28.0],
                            egui::Button::new(
                                RichText::new(*name).size(12.0).strong().color(text),
                            )
                            .fill(fill)
                            .stroke(if on {
                                egui::Stroke::new(1.0, theme.accent().gamma_multiply(0.45))
                            } else {
                                egui::Stroke::NONE
                            }),
                        );
                        if resp.clicked() {
                            state.settings_tab = i;
                        }
                        ui.add_space(2.0);
                    }
                });
            });

            ui.add_space(10.0);

            ui.vertical(|ui| {
                ui.set_min_width((avail.x - 168.0).max(320.0));
                ui.set_min_height(avail.y);
                egui::ScrollArea::vertical()
                    .id_salt("settings_content")
                    .max_height(avail.y)
                    .auto_shrink([false, false])
                    .show(ui, |ui| match state.settings_tab {
                        0 => draw_appearance(ui, state),
                        1 => draw_audio(ui, state),
                        2 => draw_plugins(ui, state),
                        3 => draw_session(ui, state),
                        _ => draw_advanced(ui, state),
                    });
            });
        },
    );
}

fn draw_appearance(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    ui.label(
        RichText::new("Appearance")
            .size(15.0)
            .strong()
            .color(theme.text()),
    );
    ui.label(
        RichText::new("Platform accent for primary actions, scope signal, and highlights.")
            .size(11.0)
            .color(theme.text_muted()),
    );
    ui.add_space(8.0);

    design::panel(ui, &theme, |ui| {
        let mut rgb = state.session.accent_rgb;
        let mut hsva = Hsva::from_srgb(rgb);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Accent").size(11.0).color(theme.text_dim()));
            if egui::color_picker::color_edit_button_hsva(
                ui,
                &mut hsva,
                egui::color_picker::Alpha::Opaque,
            )
            .changed()
            {
                rgb = hsva.to_srgb();
                state.session.accent_rgb = rgb;
                state.theme = SpectrumTheme::from_session_rgb(rgb);
                state.dirty = true;
            }
            ui.label(
                RichText::new(format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]))
                    .size(11.0)
                    .monospace()
                    .color(theme.text_dim()),
            );
            if design::button(ui, &theme, "Reset", false).clicked() {
                state.session.accent_rgb = [0x6e, 0xaa, 0x96];
                state.theme = SpectrumTheme::from_session_rgb(state.session.accent_rgb);
                state.dirty = true;
            }
        });
        ui.add_space(6.0);
        let mut hsva2 = Hsva::from_srgb(state.session.accent_rgb);
        if egui::color_picker::color_picker_hsva_2d(
            ui,
            &mut hsva2,
            egui::color_picker::Alpha::Opaque,
        ) {
            state.session.accent_rgb = hsva2.to_srgb();
            state.theme = SpectrumTheme::from_session_rgb(state.session.accent_rgb);
            state.dirty = true;
        }
    });

    ui.add_space(14.0);
    ui.label(
        RichText::new("Theme creator")
            .size(14.0)
            .strong()
            .color(theme.text()),
    );
    ui.label(
        RichText::new("Save and load named accents under ~/.config/buschain-control/themes/.")
            .size(11.0)
            .color(theme.text_muted()),
    );
    ui.add_space(6.0);
    design::panel(ui, &theme, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Name").size(11.0).color(theme.text_dim()));
            ui.add(
                egui::TextEdit::singleline(&mut state.theme_draft_name)
                    .desired_width(180.0)
                    .hint_text("Mint console"),
            );
            if design::button(ui, &theme, "Save theme", true).clicked() {
                let name = if state.theme_draft_name.trim().is_empty() {
                    format!(
                        "Accent {:02X}{:02X}{:02X}",
                        state.session.accent_rgb[0],
                        state.session.accent_rgb[1],
                        state.session.accent_rgb[2]
                    )
                } else {
                    state.theme_draft_name.trim().to_string()
                };
                match crate::session::save_theme(&name, state.session.accent_rgb) {
                    Ok(meta) => {
                        state.status = format!("Theme saved · {}", meta.name);
                        state.theme_draft_name = meta.name;
                    }
                    Err(e) => state.status = format!("Theme save failed: {e:#}"),
                }
            }
        });
        ui.add_space(8.0);
        ui.label(
            RichText::new("Saved themes")
                .size(11.0)
                .color(theme.text_muted()),
        );
        ui.add_space(4.0);
        match crate::session::list_themes() {
            Ok(themes) if themes.is_empty() => {
                ui.label(
                    RichText::new("No saved themes yet.")
                        .size(11.0)
                        .color(theme.text_muted()),
                );
            }
            Ok(themes) => {
                for t in themes {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&t.name)
                                .size(12.0)
                                .color(theme.text()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if design::button(ui, &theme, "Delete", false).clicked() {
                                let _ = crate::session::delete_theme(&t.slug);
                            }
                            if design::button(ui, &theme, "Load", false).clicked() {
                                match crate::session::load_theme(&t.slug) {
                                    Ok(preset) => {
                                        state.session.accent_rgb = preset.accent_rgb;
                                        state.theme = SpectrumTheme::from_session_rgb(
                                            state.session.accent_rgb,
                                        );
                                        state.theme_draft_name = preset.name.clone();
                                        state.dirty = true;
                                        state.status = format!("Theme loaded · {}", preset.name);
                                    }
                                    Err(e) => {
                                        state.status = format!("Theme load failed: {e:#}");
                                    }
                                }
                            }
                        });
                    });
                }
            }
            Err(e) => {
                ui.label(
                    RichText::new(format!("Could not list themes: {e:#}"))
                        .size(11.0)
                        .color(theme.danger()),
                );
            }
        }
    });
}

fn draw_audio(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    ui.label(
        RichText::new("Audio engine")
            .size(15.0)
            .strong()
            .color(theme.text()),
    );
    ui.label(
        RichText::new(
            "BusChain engine clock (tracks, FX, Master bus) is independent of Master HW speakers. \
             Engine may run at 96/192 kHz (or DXD/384) while HW stays at 48 kHz — mismatch uses an egress converter. \
             Foreign mic inputs still use inbound rate-bridges.",
        )
        .size(11.0)
        .color(theme.text_muted()),
    );
    ui.add_space(8.0);

    design::panel(ui, &theme, |ui| {
        ui.label(
            RichText::new("Master HW out")
                .size(13.0)
                .strong()
                .color(theme.text()),
        );
        ui.label(
            RichText::new(
                "Speaker/headphone device rate & quantum. Apply force-rates this sink only — does not change the BusChain engine clock.",
            )
            .size(11.0)
            .color(theme.text_muted()),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Sink name").size(11.0).color(theme.text_dim()));
            let mut out = state
                .session
                .master_output
                .clone()
                .unwrap_or_default();
            if ui.text_edit_singleline(&mut out).changed() {
                state.session.master_output = if out.is_empty() { None } else { Some(out) };
                state.dirty = true;
            }
        });
        if let Some(desc) = &state.session.master_output_desc {
            ui.label(
                RichText::new(desc)
                    .size(11.0)
                    .color(theme.text_dim()),
            );
        }
        // Full per-device clock editor for the critical Master HW out.
        if let Some(hw) = state.session.master_output.clone() {
            if let Some(node) = state
                .snapshot
                .sinks
                .iter()
                .find(|s| s.name == hw)
                .cloned()
            {
                crate::ui::devices::draw_device_clock_panel(ui, state, &node, false);
            } else {
                ui.label(
                    RichText::new(
                        "Master HW sink not in snapshot yet — open Output devices or wait for refresh.",
                    )
                    .size(10.0)
                    .color(theme.warning()),
                );
            }
        }
    });

    ui.add_space(8.0);
    draw_performance_panel(ui, state);
}

fn draw_plugins(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    ui.label(
        RichText::new("Plugins")
            .size(15.0)
            .strong()
            .color(theme.text()),
    );
    ui.label(
        RichText::new(
            "Scan paths and discovered inserts. Mixer Add lists LADSPA, CLAP, LV2, and VST3 from this scan.",
        )
        .size(11.0)
        .color(theme.text_muted()),
    );
    ui.add_space(8.0);

    design::panel(ui, &theme, |ui| {
        ui.label(
            RichText::new("Search paths")
                .size(13.0)
                .strong()
                .color(theme.text()),
        );
        ui.label(
            RichText::new("LADSPA_PATH / LV2_PATH / CLAP_PATH env vars are also scanned.")
                .size(11.0)
                .color(theme.text_muted()),
        );
        path_list(ui, &theme, "LADSPA", &mut state.session.ladspa_paths);
        path_list(ui, &theme, "LV2", &mut state.session.lv2_paths);
        path_list(ui, &theme, "CLAP", &mut state.session.clap_paths);

        ui.add_space(6.0);
        let mut vst3 = state.session.vst3_enabled;
        if ui
            .checkbox(&mut vst3, "Scan VST3 plugins (Carla discovery)")
            .changed()
        {
            state.session.vst3_enabled = vst3;
            state.rebuild_plugins();
        }
        let mut sandbox = state.session.sandbox_untrusted;
        if ui
            .checkbox(
                &mut sandbox,
                "Sandbox untrusted plugins (per-track SHM — crash isolates FX)",
            )
            .changed()
        {
            state.session.sandbox_untrusted = sandbox;
            if sandbox {
                std::env::set_var("BUSCHAIN_SANDBOX_ALL", "1");
            } else {
                std::env::remove_var("BUSCHAIN_SANDBOX_ALL");
            }
            state.dirty = true;
            state.status = if sandbox {
                "Plugin sandbox on — next FX rewire uses buschain-plugin-dsp".into()
            } else {
                "Plugin sandbox off — trusted in-process".into()
            };
        }

        if design::button(ui, &theme, "Rescan plugins", true).clicked() {
            state.rebuild_plugins();
            state.status = format!("Plugins: {}", state.plugins.plugins().len());
        }
    });

    ui.add_space(8.0);
    design::panel(ui, &theme, |ui| {
        ui.label(
            RichText::new(format!(
                "Discovered ({})",
                state.plugins.plugins().len()
            ))
            .size(13.0)
            .strong()
            .color(theme.text()),
        );
        egui::ScrollArea::vertical()
            .id_salt("discovered_plugins")
            .max_height(320.0)
            .show(ui, |ui| {
                for p in state.plugins.plugins() {
                    ui.label(
                        RichText::new(format!(
                            "[{:?}] {} — {}",
                            p.id.format, p.name, p.maker
                        ))
                        .size(11.0)
                        .color(theme.text_dim()),
                    );
                }
            });
    });
}

pub fn draw_session(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    design::page_header(
        ui,
        &theme,
        "Sessions",
        "Save and load named mixer layouts (~/.config/buschain-control/sessions/). Missing hardware rebinds softly on load.",
    );
    ui.add_space(8.0);

    design::panel(ui, &theme, |ui| {
        ui.label(
            RichText::new(format!(
                "Active: «{}» ({})",
                state.session.name, state.session.slug
            ))
            .size(12.0)
            .color(theme.text()),
        );
        ui.label(
            RichText::new(format!("{}", crate::session::Session::path().display()))
                .size(10.0)
                .monospace()
                .color(theme.text_muted()),
        );
        ui.add_space(6.0);
        ui.checkbox(
            &mut state.session.autostart_graph,
            "Autostart mixer graph on launch (OFF recommended)",
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if design::button(ui, &theme, "Save", true).clicked() {
                state.save_session();
            }
            if design::button(ui, &theme, "Save as…", false).clicked() {
                state.session_save_as_open = true;
                state.session_save_as_name = state.session.name.clone();
            }
            if design::button(ui, &theme, "New…", false).clicked() {
                state.new_session();
                state.session_save_as_open = true;
                state.session_save_as_name = "New session".into();
            }
            if design::button(ui, &theme, "Clear Master FX", false).clicked() {
                state.clear_master_fx();
            }
        });
        let master_n = state
            .session
            .tracks
            .iter()
            .find(|t| t.kind.is_master())
            .map(|t| t.inserts.len())
            .unwrap_or(0);
        ui.label(
            RichText::new(format!("Master inserts: {master_n}"))
                .size(11.0)
                .color(theme.text_muted()),
        );
    });

    if state.session_save_as_open {
        ui.add_space(6.0);
        design::panel(ui, &theme, |ui| {
            ui.label(RichText::new("Save session as").size(12.0).color(theme.text()));
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut state.session_save_as_name);
            });
            ui.horizontal(|ui| {
                if design::button(ui, &theme, "Save", true).clicked() {
                    let name = state.session_save_as_name.trim().to_string();
                    if !name.is_empty() {
                        state.save_session_as(&name);
                        state.session_save_as_open = false;
                    }
                }
                if design::button(ui, &theme, "Cancel", false).clicked() {
                    state.session_save_as_open = false;
                }
            });
        });
    }

    ui.add_space(10.0);
    design::panel(ui, &theme, |ui| {
        ui.label(
            RichText::new("Library")
                .size(13.0)
                .strong()
                .color(theme.text()),
        );
        ui.add_space(4.0);
        let list = crate::session::list_sessions().unwrap_or_default();
        if list.is_empty() {
            ui.label(
                RichText::new("No saved sessions yet.")
                    .size(11.0)
                    .color(theme.text_muted()),
            );
        }
        let mut load_slug: Option<String> = None;
        let mut delete_slug: Option<String> = None;
        for meta in &list {
            let insert_n = if meta.slug == state.session.slug {
                state
                    .session
                    .tracks
                    .iter()
                    .find(|t| t.kind.is_master())
                    .map(|t| t.inserts.len())
                    .unwrap_or(0)
            } else {
                crate::session::store::load_slug(&meta.slug)
                    .ok()
                    .and_then(|s| {
                        s.tracks
                            .iter()
                            .find(|t| t.kind.is_master())
                            .map(|t| t.inserts.len())
                    })
                    .unwrap_or(0)
            };
            ui.horizontal(|ui| {
                let active = meta.slug == state.session.slug;
                ui.label(
                    RichText::new(format!(
                        "{}{}{}",
                        meta.name,
                        if active { "  (active)" } else { "" },
                        if insert_n > 0 {
                            format!(" — {insert_n} Master FX")
                        } else {
                            " — clean".into()
                        }
                    ))
                    .size(12.0)
                    .color(if active { theme.accent() } else { theme.text() }),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if design::button(ui, &theme, "Delete", false).clicked() {
                        delete_slug = Some(meta.slug.clone());
                    }
                    if !active && design::button(ui, &theme, "Load", true).clicked() {
                        load_slug = Some(meta.slug.clone());
                    }
                });
            });
            ui.label(
                RichText::new(&meta.slug)
                    .size(10.0)
                    .monospace()
                    .color(theme.text_muted()),
            );
            ui.add_space(4.0);
        }
        if let Some(s) = load_slug {
            state.load_session_slug(&s);
        }
        if let Some(s) = delete_slug {
            state.delete_session_slug(&s);
        }
    });
}

fn draw_advanced(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    ui.label(
        RichText::new("Advanced")
            .size(15.0)
            .strong()
            .color(theme.text()),
    );
    ui.label(
        RichText::new("Recovery tools. Normal use never needs these.")
            .size(11.0)
            .color(theme.text_muted()),
    );
    ui.add_space(8.0);

    design::panel(ui, &theme, |ui| {
        ui.label(
            RichText::new("Graph reset")
                .size(13.0)
                .strong()
                .color(theme.text()),
        );
        ui.label(
            RichText::new(
                "The supervisor reconciles automatically. Reset only if modules stacked \
                 or audio is irreparably crackling.",
            )
            .size(11.0)
            .color(theme.text_muted()),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if design::button(ui, &theme, "Reset BusChain graph", false)
                .on_hover_text(
                    "EMERGENCY — unload all BusChain PipeWire modules. \
                     Next edit auto bring-up. Apps may briefly hiccup.",
                )
                .clicked()
            {
                state.teardown_graph();
            }
            if design::button(ui, &theme, "Reconcile now", false)
                .on_hover_text("Force DesiredState↔live reconcile.")
                .clicked()
            {
                state.apply_graph();
            }
        });
    });

    ui.add_space(10.0);
    design::panel(ui, &theme, |ui| {
        ui.label(
            RichText::new("Application")
                .size(13.0)
                .strong()
                .color(theme.text()),
        );
        ui.label(
            RichText::new(
                "Close hides to the tray (graph keeps running). \
                 Quit from here or the tray menu exits fully.",
            )
            .size(11.0)
            .color(theme.text_muted()),
        );
        ui.add_space(6.0);
        if design::button(ui, &theme, "Quit BusChain Control", false).clicked() {
            state.request_quit = true;
        }
    });
}

fn draw_performance_panel(ui: &mut egui::Ui, state: &mut AppState) {
    use buschain_engine::{
        engine_rate_is_extreme, resolve_engine_profile, AudioPreset, ENGINE_QUANTUMS, ENGINE_RATES,
    };
    let theme = state.theme;

    // One-shot when Master HW caps are missing — never re-probe every frame.
    if state.device_caps.sink_name.is_empty() && !state.snapshot.sinks.is_empty() {
        state.refresh_performance_from_device();
    }

    design::panel(ui, &theme, |ui| {
        ui.label(
            RichText::new("BusChain engine clock")
                .size(13.0)
                .strong()
                .color(theme.text()),
        );
        ui.label(
            RichText::new(
                "Internal GraphClock for tracks, FX, and Master bus. Not limited to Master HW caps — \
                 96/192 kHz for quality; 352.8 (DXD) / 384 kHz for archival apex. Speakers stay on the HW clock above.",
            )
            .size(11.0)
            .color(theme.text_muted()),
        );
        ui.add_space(4.0);

        let hw_rate = state.device_caps.preferred_rate;
        let eng_rate = state.session.performance.sample_rate;
        if hw_rate > 0 && eng_rate != hw_rate {
            ui.label(
                RichText::new(format!(
                    "Engine {eng_rate} Hz ≠ Master HW {hw_rate} Hz — egress converts on Apply"
                ))
                .size(10.0)
                .color(theme.warning()),
            );
        }
        if engine_rate_is_extreme(eng_rate) {
            ui.label(
                RichText::new(
                    "Above 192 kHz: heavy CPU, some plugins misbehave, little benefit once egress hits Master HW. DXD/384 is for archival / apex workflows.",
                )
                .size(10.0)
                .color(theme.warning()),
            );
        }

        let mut preset = state.session.performance.preset;
        ui.horizontal(|ui| {
            for p in [
                AudioPreset::Balanced,
                AudioPreset::LowLatency,
                AudioPreset::Stable,
                AudioPreset::Custom,
            ] {
                if ui
                    .selectable_label(preset == p, RichText::new(p.label()).size(11.0))
                    .clicked()
                {
                    preset = p;
                }
            }
        });
        if preset != state.session.performance.preset {
            state.session.performance.preset = preset;
            state.refresh_performance_from_device();
        }

        let perf = &state.session.performance;
        ui.label(
            RichText::new(format!(
                "{} Hz · quantum {} · period {:.2} ms",
                perf.sample_rate,
                perf.quantum,
                perf.period_ms(),
            ))
            .size(11.0)
            .color(theme.accent()),
        );

        if matches!(state.session.performance.preset, AudioPreset::Custom) {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Rate").size(11.0).color(theme.text_dim()));
                let mut rate = state.session.performance.sample_rate;
                if !ENGINE_RATES.contains(&rate) {
                    rate = 48_000;
                }
                egui::ComboBox::from_id_salt("perf_rate")
                    .selected_text(format!("{rate}"))
                    .show_ui(ui, |ui| {
                        for r in ENGINE_RATES {
                            let label = if *r == 352_800 {
                                format!("{r} (DXD)")
                            } else if *r == 384_000 {
                                format!("{r} (apex)")
                            } else {
                                format!("{r}")
                            };
                            ui.selectable_value(&mut rate, *r, label);
                        }
                    });
                ui.label(RichText::new("Quantum").size(11.0).color(theme.text_dim()));
                let mut q = state.session.performance.quantum;
                egui::ComboBox::from_id_salt("perf_q")
                    .selected_text(format!("{q}"))
                    .show_ui(ui, |ui| {
                        for qq in ENGINE_QUANTUMS {
                            ui.selectable_value(&mut q, *qq, format!("{qq}"));
                        }
                    });
                if rate != state.session.performance.sample_rate
                    || q != state.session.performance.quantum
                {
                    let soft = state.session.performance.soft_quantum;
                    state.session.performance = resolve_engine_profile(
                        AudioPreset::Custom,
                        Some(rate),
                        Some(q),
                        soft,
                    );
                    state.dirty = true;
                }
            });
        }

        let mut soft = state.session.performance.soft_quantum;
        if ui
            .checkbox(&mut soft, "Soft quantum (PipeWire may raise period under load)")
            .changed()
        {
            state.session.performance.soft_quantum = soft;
            state.dirty = true;
        }

        ui.horizontal(|ui| {
            if design::button(ui, &theme, "Apply engine clock", true).clicked() {
                state.apply_audio_clock();
            }
            if design::button(ui, &theme, "Reset engine Balanced", false).clicked() {
                state.session.performance.preset = AudioPreset::Balanced;
                state.apply_audio_clock();
            }
            if design::button(ui, &theme, "Match Master HW rate", false)
                .on_hover_text(
                    "Copy Master HW sample rate / quantum into the engine profile (still Apply to bind).",
                )
                .clicked()
            {
                state.match_engine_clock_to_master_hw();
            }
        });
    });
}

fn path_list(ui: &mut egui::Ui, theme: &dyn Theme, label: &str, paths: &mut Vec<String>) {
    ui.label(RichText::new(label).size(11.0).color(theme.text_dim()));
    let mut remove = None;
    for (i, p) in paths.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.text_edit_singleline(p);
            if design::icon_button(ui, theme, egui_phosphor::regular::X, true).clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        paths.remove(i);
    }
    if design::button(ui, theme, &format!("+ {label} path"), false).clicked() {
        paths.push(String::new());
    }
}
