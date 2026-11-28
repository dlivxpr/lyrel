//! The Markdown workbench's retained native-case modules and the case
//! aggregation the root `native_markdown` binary runs on the process main
//! thread. Production rendering lives in
//! `lyrel_workbench_editor::code_editor::markdown`.

#[cfg(any(test, feature = "native-markdown-tests"))]
mod foundation;
#[cfg(feature = "native-markdown-tests")]
mod history_441;
#[cfg(feature = "native-markdown-tests")]
mod mermaid_host_tests;
#[cfg(any(test, feature = "native-markdown-tests"))]
mod resources_tests;
#[cfg(any(test, feature = "native-markdown-tests"))]
mod table_code_tests;
#[cfg(any(test, feature = "native-markdown-tests"))]
pub(super) mod test_support;
#[cfg(any(test, feature = "native-markdown-tests"))]
mod writing_tests;

pub(crate) use lyrel_workbench_editor::code_editor::markdown::*;

/// Every native Markdown case, in the order the pre-extraction suite listed
/// them. Selector strings are unchanged.
#[cfg(feature = "native-markdown-tests")]
pub(crate) fn native_cases() -> impl Iterator<Item = crate::native_markdown::Case> {
    foundation::native_cases()
        .chain(resources_tests::native_cases())
        .chain(writing_tests::native_cases())
        .chain(table_code_tests::native_cases())
        .chain([
            crate::native_markdown::Case::new(
                "ui::code_editor::workbench_views::shared_context_and_ime_handoff",
                super::document_input::workbench_views::shared_context_and_ime_handoff,
            ),
            crate::native_markdown::Case::new(
                "ui::code_editor::workbench_views::focused_consumers_and_layout",
                super::document_input::workbench_views::focused_consumers_and_layout,
            ),
        ])
        .chain([
            crate::native_markdown::Case::new(
                "ui::code_editor::markdown::mermaid_host::native_edit_refresh_history_save_reopen",
                mermaid_host_tests::lifecycle,
            ),
            crate::native_markdown::Case::new(
                "ui::code_editor::markdown::mermaid_host::native_stale_completion",
                mermaid_host_tests::stale_completion,
            ),
            crate::native_markdown::Case::new(
                "ui::code_editor::markdown::mermaid_host::native_theme_zoom_scroll_geometry",
                mermaid_host_tests::theme_zoom_scroll,
            ),
        ])
        .chain(super::saving::native_cases())
        .chain(super::markdown_formula::native_cases())
        .chain([crate::native_markdown::Case::new(
            "ui::code_editor::markdown::history_441::document_history_441",
            history_441::document_history_441,
        )])
}
