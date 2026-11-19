//! The shared A-style radius scale lives in `lyrel-ui-base`; this facade keeps
//! `crate::ui::rounding::` working for the workbench while consumers move onto
//! direct `lyrel_ui_base::rounding` imports.

pub use lyrel_ui_base::rounding::*;
