//! BusChain Control library — UI, session, and audio worker shared with the daemon.

pub mod app_state;
pub mod audio;
pub mod brand;
pub mod daemon;
pub mod design;
pub mod ipc;
pub mod mixer_api;
pub mod popup_launch;
pub mod scroll_strip;
pub mod session;
pub mod tray;
pub mod ui;
pub mod withdraw;
pub mod hyprland_float;

pub use app_state::AppState;
pub use ipc::ensure_daemon;
pub use session::Session;
