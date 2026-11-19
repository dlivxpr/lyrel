//! Overlays the shared vertical scrollbar on a scroll area.
//!
//! Lives in `lyrel-ui-base` so the workbench editor's document views and the
//! shell's panels draw the same bar; this facade keeps `crate::ui::scrollbar::`
//! working for the root package.

pub use lyrel_ui_base::scrollbar::{
    over_vertical_scroll, with_inset_vertical_scrollbar, with_vertical_scrollbar,
};
