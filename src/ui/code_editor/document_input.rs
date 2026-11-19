// Retains the `document_input::workbench_views` native consumer module; the
// production `document_input` lives in `lyrel-workbench-editor`.

#[cfg(any(test, feature = "native-markdown-tests"))]
#[path = "workbench_views.rs"]
pub(super) mod workbench_views;
