use std::time::Duration;

use eframe::egui;
use buschain_control::app_state::AppState;
use buschain_control::audio;
use buschain_control::design::Theme;
use buschain_control::tray::{self, HeadlessWait};
use buschain_control::ui;
use buschain_control::withdraw;

mod dev_bootstrap;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--popup" || a == "popup") {
        // Do not bootstrap — would steal the tray app's IPC socket.
        return run_popup(&args);
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!(
            "buschain-control — tray-resident system mixer\n\n\
               (default)        in-process graph + embedded IPC for ctl/waybar\n\
               --hidden         tray only — no window until Show (autostart)\n\
               --daemon-client  debug: attach to external buschain-daemon\n\
               --popup [playback]  compact QS-like mixer popup\n\n\
               Dev: `cargo run` / `cargo run -- --hidden` bootstraps plugins + buschain-ctl.\n"
        );
        std::process::exit(0);
    }

    // Plain `cargo run` = full local workflow (plugins, ctl for waybar, clean socket).
    dev_bootstrap::run();

    // Native Wayland host by default. VST3 editors float on XWayland separately
    // (no in-rect X11 embed). Override with WINIT_UNIX_BACKEND=x11 if needed.
    if std::env::var_os("WINIT_UNIX_BACKEND").is_none() {
        std::env::set_var("WINIT_UNIX_BACKEND", "wayland");
    }
    eprintln!(
        "buschain-control: window backend → {} (WINIT_UNIX_BACKEND)",
        std::env::var("WINIT_UNIX_BACKEND").unwrap_or_else(|_| "?".into())
    );

    let start_hidden = args.iter().any(|a| a == "--hidden");

    // Do not create a GL/window until the user asks.
    // Hiding after map always races Wayland — never create the surface instead.
    let boot_state = if start_hidden {
        match tray::wait_until_show() {
            HeadlessWait::Quit => return Ok(()),
            HeadlessWait::Show(state) => Some(state),
        }
    } else {
        None
    };

    let icon = window_icon();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1100.0, 720.0])
        .with_min_inner_size([800.0, 480.0])
        .with_title("BusChain Control")
        .with_decorations(true)
        .with_app_id("buschain-control");
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        vsync: false,
        ..Default::default()
    };

    eframe::run_native(
        "BusChain Control",
        options,
        Box::new(move |cc| {
            let mut fonts = egui::FontDefinitions::default();
            egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
            cc.egui_ctx.set_fonts(fonts);

            tray::ensure_started();

            let mut state = boot_state.unwrap_or_else(AppState::new);
            // Headless Show leaves visualization Idle — wake for the new window.
            let mut runtime = if state.viz_live {
                ui::UiRuntime::new_live()
            } else {
                ui::UiRuntime::new_idle()
            };
            if !runtime.is_live() {
                runtime.enter_live(&mut state);
            }
            let app = BusChainApp {
                state,
                hide_on_close: true,
                withdrawn: false,
                runtime,
            };
            app.state.theme.apply_egui(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}

fn run_popup(args: &[String]) -> eframe::Result<()> {
    // Direct egui popup only. Tray / ctl / waybar use
    // `popup_launch::spawn_mixer_popup` (QS → GTK → egui).
    let _tab = args
        .windows(2)
        .find(|w| w[0] == "--popup" || w[0] == "popup")
        .map(|w| w[1].as_str())
        .unwrap_or("playback");

    let icon = window_icon();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([480.0, 560.0])
        .with_min_inner_size([360.0, 320.0])
        .with_title("BusChain Mixer")
        .with_decorations(true)
        .with_always_on_top()
        .with_app_id("buschain-control-popup");
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        vsync: false,
        ..Default::default()
    };

    eframe::run_native(
        "BusChain Control Popup",
        options,
        Box::new(move |cc| {
            let mut fonts = egui::FontDefinitions::default();
            egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
            cc.egui_ctx.set_fonts(fonts);
            std::env::set_var("BUSCHAIN_CONTROL_USE_DAEMON", "1");
            let mut state = AppState::new();
            state.tab = 1;
            state.popup_mode = true;
            let app = PopupApp { state };
            app.state.theme.apply_egui(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}

fn window_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../../assets/icons/buschain-control.png");
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width: w,
        height: h,
    })
}

struct BusChainApp {
    state: AppState,
    hide_on_close: bool,
    /// Window close was intercepted — stay alive withdrawn (Hyprland-safe).
    withdrawn: bool,
    /// Live vs Idle visualization pipeline (meters / spectrum / paint).
    runtime: ui::UiRuntime,
}

impl eframe::App for BusChainApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        tray::poll_tray_commands(&mut self.state, ctx);

        if self.state.request_show {
            self.state.request_show = false;
            self.state.request_hide = false;
            self.withdrawn = false;
            self.runtime.enter_live(&mut self.state);
            withdraw::restore(ctx);
        }
        if self.state.request_hide {
            self.state.request_hide = false;
            self.withdrawn = true;
            self.runtime.enter_idle(&mut self.state);
            withdraw::withdraw(ctx);
        }

        let quitting = self.state.request_quit;
        if quitting {
            self.hide_on_close = false;
            self.withdrawn = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // Window close button AND Hyprland killactive (Super+Q) both raise this.
        let close_req = ctx.input(|i| i.viewport().close_requested());
        if close_req && self.hide_on_close && !quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.withdrawn = true;
            self.runtime.enter_idle(&mut self.state);
            withdraw::withdraw(ctx);
        }

        // Always tick (levels / worker) — keeps the graph responsive while hidden.
        self.state.tick();

        // Tray left-click mixer popup (works while withdrawn).
        if self.state.mixer_popup_open {
            self.state.theme.apply_egui(ctx);
            ui::draw_embedded_popup(ctx, &mut self.state);
        }

        if self.withdrawn {
            if self.runtime.is_live() {
                self.runtime.enter_idle(&mut self.state);
            }
            ctx.request_repaint_after(Duration::from_millis(ui::IDLE_REPAINT_MS));
            return;
        }

        if !self.runtime.is_live() {
            self.runtime.enter_live(&mut self.state);
        }

        self.state.theme.apply_egui(ctx);
        ui::draw(ctx, &mut self.state);
        let ms = buschain_control::audio::adaptive::AdaptivePolicy::global()
            .repaint_ms_for_activity(true);
        ctx.request_repaint_after(Duration::from_millis(ms));
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = self.state.theme.bg_app();
        [
            c.r() as f32 / 255.0,
            c.g() as f32 / 255.0,
            c.b() as f32 / 255.0,
            1.0,
        ]
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.state.session.save();
        self.state.meters.shutdown();
        self.state.worker.send(audio::worker::Command::Shutdown);
        tray::shutdown();
    }
}

struct PopupApp {
    state: AppState,
}

impl eframe::App for PopupApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.state.tick();
        self.state.theme.apply_egui(ctx);
        ui::draw_popup(ctx, &mut self.state);
        ctx.request_repaint_after(Duration::from_millis(33));
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = self.state.theme.bg_app();
        [
            c.r() as f32 / 255.0,
            c.g() as f32 / 255.0,
            c.b() as f32 / 255.0,
            1.0,
        ]
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.state.meters.shutdown();
    }
}
