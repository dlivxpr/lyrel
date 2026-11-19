//! The settings window's per-section pages. Each module owns exactly one
//! page's rendering plus the section-local `impl SettingsWindow` helpers that
//! page needs; rows, popovers and the frame shared by every page live in
//! [`crate::ui::settings::shell`] and [`crate::ui::settings::kit`], and every
//! side effect that reaches past this window leaves through
//! [`crate::ui::settings_window::SettingsEvent`] instead of a direct call.
use super::*;

mod about;
mod appearance;
mod general;
mod input;
mod terminal;
pub(crate) mod theme_edit;
pub(crate) mod theme_picker;
