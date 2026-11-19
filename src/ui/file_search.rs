//! The shared file index lives in `lyrel-ui-base`; this facade keeps
//! `crate::ui::file_search::` working for the workbench while consumers move
//! onto direct `lyrel_ui_base::file_search` imports.

pub use lyrel_ui_base::file_search::*;
