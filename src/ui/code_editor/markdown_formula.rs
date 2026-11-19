//! Retained native-case tests for the Markdown formula editor; production
//! lives in `lyrel_workbench_editor::code_editor::markdown_formula`.

#[cfg(any(test, feature = "native-markdown-tests"))]
mod tests;

#[cfg(feature = "native-markdown-tests")]
pub(crate) fn native_cases() -> impl Iterator<Item = crate::native_markdown::Case> {
    tests::native_cases()
}
