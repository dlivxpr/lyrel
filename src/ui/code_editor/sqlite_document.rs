// Retained whole-window tests for the `sqlite_document` module, which now lives in
// `lyrel-workbench-editor`. Keeping this anchor at the original module path
// preserves every existing test selector.

#[cfg(test)]
mod tests;

pub(crate) use lyrel_workbench_editor::code_editor::sqlite_document::*;

// Inherited by the retained tests from the production module's scope before
// the move; kept here so their `use super::*` resolves unchanged.
#[cfg(test)]
use lyrel_ui_base::host_ops::SharedHost;
