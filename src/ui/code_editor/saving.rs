//! Retained whole-window save-lifecycle tests and native cases for the saving
//! module, which now lives in `lyrel-workbench-editor`.

#[cfg(any(test, feature = "native-markdown-tests"))]
mod fidelity_tests;
#[cfg(test)]
mod tests;

use crate::ui::app::LyrelApp;
use gpui_kit::{Entity, Focusable as _};
use lyrel_ui_base::host_ops::HostId;
use lyrel_ui_base::i18n::{L10nKey, t};
#[cfg(test)]
use lyrel_workbench_editor::code_editor::BufferId;
use lyrel_workbench_editor::code_editor::BufferKind;
use std::path::{Path, PathBuf};

#[cfg(feature = "native-markdown-tests")]
pub(crate) fn native_cases() -> impl Iterator<Item = crate::native_markdown::Case> {
    fidelity_tests::native_cases()
}
