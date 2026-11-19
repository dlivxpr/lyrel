//! The shared file-open seam lives in `lyrel-ui-base`; this facade keeps
//! `crate::ui::desktop_file::` working for the workbench while consumers move
//! onto direct `lyrel_ui_base::desktop_file` imports.

pub use lyrel_ui_base::desktop_file::*;
