//! The process-wide pane-activation channel lives in `lyrel-ui-base` as the
//! typed `TerminalHostEvent` seam; this facade keeps
//! `crate::ui::notification_bridge::` working for the workbench while
//! consumers move onto direct `lyrel_ui_base::notification_bridge` imports.

pub use lyrel_ui_base::notification_bridge::*;
