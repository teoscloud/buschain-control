//! StatusNotifier tray for hide-on-close.
//!
//! Uses `ksni` over D-Bus. Left-click → mixer popup; menu: Open full app · Hide · Quit.
//!
//! `--hidden` runs headless (no egui window) until [`wait_until_show`].

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use crate::app_state::AppState;
use crate::daemon;
use eframe::egui::Context;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayCmd {
    Show,
    Hide,
    Popup,
    Quit,
}

/// Result of the headless `--hidden` wait loop.
pub enum HeadlessWait {
    /// User asked to open the mixer UI — start egui with this state.
    Show(AppState),
    /// User quit from the tray — process should exit (state already torn down).
    Quit,
}

struct TrayChannels {
    tx: Sender<TrayCmd>,
    rx: Mutex<Receiver<TrayCmd>>,
}

fn channels() -> &'static TrayChannels {
    static C: OnceLock<TrayChannels> = OnceLock::new();
    C.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        TrayChannels {
            tx,
            rx: Mutex::new(rx),
        }
    })
}

struct BusChainTray {
    tx: Sender<TrayCmd>,
}

impl ksni::Tray for BusChainTray {
    fn id(&self) -> String {
        "buschain-control".into()
    }

    fn title(&self) -> String {
        "BusChain Control".into()
    }

    fn icon_theme_path(&self) -> String {
        crate::brand::tray_icon_theme_path()
    }

    fn icon_name(&self) -> String {
        crate::brand::tray_icon_name()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        crate::brand::tray_icon_pixmap()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            icon_name: crate::brand::tray_icon_name(),
            icon_pixmap: crate::brand::tray_icon_pixmap(),
            title: "BusChain Control".into(),
            description: "Mixer — left-click popup · right-click for full app".into(),
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        vec![
            StandardItem {
                label: "Open BusChain Control".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(TrayCmd::Show);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Hide".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(TrayCmd::Hide);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(TrayCmd::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(TrayCmd::Popup);
    }
}

pub fn ensure_started() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let tx = channels().tx.clone();
        thread::Builder::new()
            .name("buschain-tray".into())
            .spawn(move || {
                let service = ksni::TrayService::new(BusChainTray { tx });
                if let Err(e) = service.run() {
                    eprintln!("buschain-control: tray failed: {e:#}");
                }
            })
            .ok();
    });
}

/// No window: worker + IPC + tray only, until Show or Quit.
pub fn wait_until_show() -> HeadlessWait {
    ensure_started();
    let mut state = AppState::new();
    // No window → Idle viz (pause Pulse meters / clear FFT watches).
    state.sleep_visualization();
    eprintln!("buschain-control: headless tray (no window until Show)");

    loop {
        state.tick();

        if daemon::take_embedded_quit() {
            teardown_headless(state);
            return HeadlessWait::Quit;
        }
        if daemon::take_embedded_show() {
            return HeadlessWait::Show(state);
        }

        let Ok(rx) = channels().rx.lock() else {
            thread::sleep(Duration::from_millis(50));
            continue;
        };
        loop {
            match rx.try_recv() {
                Ok(TrayCmd::Show) => return HeadlessWait::Show(state),
                Ok(TrayCmd::Quit) => {
                    drop(rx);
                    teardown_headless(state);
                    return HeadlessWait::Quit;
                }
                // No egui yet — cold-start compact popup process.
                Ok(TrayCmd::Popup) => spawn_popup_playback(),
                Ok(TrayCmd::Hide) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        drop(rx);
        thread::sleep(Duration::from_millis(50));
    }
}

fn teardown_headless(mut state: AppState) {
    // Same as window Quit — keep sticky buschain preferred; live restore is
    // Shutdown → restore_system_audio (HW default + stream move-off).
    let _ = state.session.save();
    state.meters.shutdown();
    state.restore_desktop_and_stop_worker();
    shutdown();
}

pub fn poll_tray_commands(state: &mut AppState, _ctx: &Context) {
    if daemon::take_embedded_show() {
        state.request_show = true;
    }
    if daemon::take_embedded_quit() {
        state.request_quit = true;
    }

    let Ok(rx) = channels().rx.lock() else {
        return;
    };
    loop {
        match rx.try_recv() {
            Ok(TrayCmd::Show) => {
                state.request_show = true;
            }
            Ok(TrayCmd::Hide) => {
                state.request_hide = true;
            }
            Ok(TrayCmd::Popup) => {
                // Same router as ctl/waybar: QS → GTK → egui (not in-process egui).
                spawn_popup_playback();
            }
            Ok(TrayCmd::Quit) => {
                state.request_quit = true;
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => break,
        }
    }
}

pub fn shutdown() {}

/// Spawn the compact mixer popup (ctl / cold headless tray / external callers).
pub fn spawn_popup_playback() {
    crate::popup_launch::spawn_mixer_popup();
}
