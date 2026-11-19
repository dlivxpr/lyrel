// Retained whole-window tests for the `book_view` module, which now lives in
// `lyrel-workbench-editor`. Keeping this anchor at the original module path
// preserves every existing test selector.

#[cfg(test)]
mod tests;

pub(crate) use lyrel_workbench_editor::code_editor::book_view::*;

use crate::ui::app::LyrelApp;
use gpui_kit::{Entity, px};
use lyrel_ui_base::host_ops::{HostId, SharedHost};
use lyrel_workbench_editor::code_editor::*;
use std::time::Duration;
