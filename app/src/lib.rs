//! BusChain Control library — UI, session, and audio worker shared with the daemon.

pub mod app_state;
pub mod audio;
pub mod daemon;
pub mod design;
pub mod ipc;
pub mod popup_launch;
pub mod session;
pub mod tray;
pub mod ui;
pub mod withdraw;

pub use app_state::AppState;
pub use ipc::ensure_daemon;
pub use session::Session;
