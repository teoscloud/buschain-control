//! Hide / restore the egui window after it has been created.
//!
//! Initial `--hidden` startup does **not** create a window at all (see `main.rs`);
//! this module is only for Hide / close / Super+Q after the UI exists.

use std::process::Command;

use eframe::egui::{self, Context};

const SPECIAL: &str = "special:buschaincontrol";

fn hyprctl(args: &[&str]) -> bool {
    Command::new("hyprctl")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Withdraw the main window without killing the process (tray stays alive).
pub fn withdraw(ctx: &Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    let _ = hyprctl(&[
        "dispatch",
        "movetoworkspacesilent",
        &format!("{SPECIAL},class:^(buschain-control)$"),
    ]);
}

/// Bring the main window back from tray / special workspace.
pub fn restore(ctx: &Context) {
    let _ = hyprctl(&[
        "dispatch",
        "movetoworkspace",
        "e+0,class:^(buschain-control)$",
    ]);
    let _ = hyprctl(&["dispatch", "focuswindow", "class:^(buschain-control)$"]);
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
}
