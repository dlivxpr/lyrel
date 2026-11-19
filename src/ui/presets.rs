//! Semantic theme roles, color presets, blending helpers and iTerm2/YAML
//! theme loading live in `lyrel-ui-base`; this facade keeps
//! `crate::ui::presets::` working.

pub use lyrel_ui_base::presets::{
    ActiveBackground, ActiveLanes, Fill, Image, Interactions, LANE_SLOTS, Lanes, Parking, Surface,
    Surfaces, Theme, Themes, all, by_id, fork_to_file, load_registry, mix, needs_edge, rejected,
    resting_ink, scrim_fill, surface_is_dark, themes_dir, to_yaml, write_theme_file,
};

#[cfg(test)]
pub use lyrel_ui_base::presets::{builtins, contrast};
