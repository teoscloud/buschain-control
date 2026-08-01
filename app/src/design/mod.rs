//! Modular design system — tokens + primitives only. No audio logic.

mod dynamics_xfer_3d;
mod eq_chart;
pub mod spatial_viz;
mod tokens;
mod widgets;

pub use dynamics_xfer_3d::{
    fill_log_bands_linear, fill_log_curve_linear, paint_dynamics_xfer_3d, transfer_viz_mode_toggle,
    PhosphorTrail3d, TransferVizMode, Xfer3dCamera, BAND_N, BAND_PHOS_HIST, EQ_CURVE_N,
};
pub use eq_chart::*;
pub use tokens::*;
pub use widgets::*;
