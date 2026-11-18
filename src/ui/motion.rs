//! The application's motion policy: one persisted preference mapped onto the
//! framework's reduced-motion channel.
//!
//! Standard `with_animation` wrappers and Base transitions consult
//! `App::reduce_motion`, but GPUI never reads the platform into that flag. The
//! Base layer owns the platform read (NSWorkspace, `SPI_GETCLIENTAREAANIMATION`,
//! the XDG portal) and the override bookkeeping; this module only translates
//! Lyrel's config value and decides when the system is re-read.

use crate::core::config::{Config, MotionPreference};
use gpui_kit::App;
use gpui_kit::base::{
    ReduceMotionPreference, apply_system_reduce_motion, set_reduce_motion_preference,
};

fn framework_preference(preference: MotionPreference) -> ReduceMotionPreference {
    match preference {
        MotionPreference::System => ReduceMotionPreference::System,
        MotionPreference::Reduce => ReduceMotionPreference::Reduce,
        MotionPreference::Allow => ReduceMotionPreference::Allow,
    }
}

/// Applies the preference currently held in the global [`Config`].
///
/// `System` releases any application override and re-reads the platform
/// preference. `Reduce` and `Allow` take ownership so later system readings
/// cannot silently change the choice.
pub(crate) fn apply(cx: &mut App) {
    let preference = cx.global::<Config>().motion_preference;
    set_reduce_motion_preference(framework_preference(preference), cx);
}

/// Re-reads the platform preference when the user follows the system.
///
/// Windows and macOS report the value only when asked, so a window becoming
/// active is the event that notices a change made in the OS. An explicit
/// override deliberately does nothing here: activation must not erase it.
pub(crate) fn refresh_system(cx: &mut App) {
    if cx.global::<Config>().motion_preference == MotionPreference::System {
        apply_system_reduce_motion(cx);
    }
}

#[cfg(test)]
#[path = "motion_tests.rs"]
mod tests;
