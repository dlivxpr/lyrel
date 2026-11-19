// Retained whole-window tests for the `yaml_document` module, which now lives in
// `lyrel-workbench-editor`. Keeping this anchor at the original module path
// preserves every existing test selector.

#[cfg(test)]
mod tests;

pub(crate) use lyrel_workbench_editor::code_editor::yaml_document::*;

// Inherited by the retained tests from the production module's scope before
// the move; kept here so their `use super::*` resolves unchanged.
#[cfg(test)]
use crate::ui::app::LyrelApp;
#[cfg(test)]
use gpui_kit::component::input::Position;
#[cfg(test)]
use gpui_kit::prelude::*;
#[cfg(test)]
use gpui_kit::{
    App, Context, Entity, EntityInputHandler as _, Focusable as _, SharedString, Window, px,
};
#[cfg(test)]
use lyrel_ui_base::host_ops::{HostId, HostOps, SharedHost};
#[cfg(test)]
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};
#[cfg(test)]
use lyrel_workbench_editor::code_editor::*;
#[cfg(test)]
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::time::{Duration, Instant};
