use crate::app_state::AppState;
use crate::design::{self, Theme};
use egui::RichText;

pub fn draw_recording(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    design::page_header(
        ui,
        &theme,
        "Recording",
        "Apps capturing audio (mics, loopbacks). Assign sources and levels without digging through pw-dump.",
    );
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Capture streams")
                .size(12.0)
                .strong()
                .color(theme.text_dim()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.checkbox(&mut state.show_hidden_recording, "Show internals");
        });
    });
    ui.add_space(6.0);

    egui::ScrollArea::vertical().show(ui, |ui| {
        let outs: Vec<_> = state
            .snapshot
            .source_outputs
            .iter()
            .filter(|s| state.show_hidden_recording || s.is_user_app())
            .cloned()
            .collect();
        if outs.is_empty() {
            ui.label(
                RichText::new(if state.show_hidden_recording {
                    "No active recording streams"
                } else {
                    "No application capture streams (enable Show hidden for internals)"
                })
                .color(theme.text_muted()),
            );
            return;
        }
        for stream in outs {
            design::panel(ui, &theme, |ui| {
                ui.label(
                    RichText::new(&stream.application)
                        .size(13.0)
                        .strong()
                        .color(theme.text()),
                );
                ui.label(
                    RichText::new(format!(
                        "source {} · {}%",
                        stream.sink_or_source, stream.volume_pct
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
                ui.label(
                    RichText::new(if stream.mute { "Muted" } else { "Active" })
                        .size(11.0)
                        .color(if stream.mute {
                            theme.danger()
                        } else {
                            theme.success()
                        }),
                );
            });
            ui.add_space(6.0);
        }
    });
}
