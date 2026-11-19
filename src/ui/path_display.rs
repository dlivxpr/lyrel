//! Path display helpers live in `lyrel-ui-base`; this facade keeps
//! `crate::ui::path_display::` working.

pub use lyrel_ui_base::path_display::{
    abbreviate_home, native_separators, readable_path, split_path_leaf,
};
