// Retained whole-window tests for the `svg_document` module, which now lives in
// `lyrel-workbench-editor`. Keeping this anchor at the original module path
// preserves every existing test selector.

#[cfg(test)]
mod tests;

pub(crate) use lyrel_workbench_editor::code_editor::svg_document::*;

use lyrel_ui_base::host_ops::SharedHost;
use lyrel_ui_base::i18n::t;
