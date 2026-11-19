//! Semantic theme roles, color presets, blending helpers and iTerm2/YAML
//! theme loading live in `lyrel-ui-base`; this facade keeps
//! `crate::ui::presets::` working.

pub use lyrel_ui_base::presets::{
    ActiveAccent, ActiveBackground, ActiveLanes, Fill, Image, Interactions, LANE_SLOTS, Lanes,
    Parking, Surface, Surfaces, Theme, Themes, all, by_id, caret_ink, fork_to_file, is_dark,
    is_lighter, legible_dim, load_registry, match_wash_targets, mix, needs_edge, rejected,
    resting_ink, scrim_fill, surface_is_dark, themes_dir, to_yaml, wash, write_theme_file,
};

#[cfg(test)]
pub use lyrel_ui_base::presets::{builtins, contrast};
