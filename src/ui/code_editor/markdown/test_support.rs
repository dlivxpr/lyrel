//! Shared entity-read helpers for the Markdown workbench tests.
//!
//! The panel state lives on its own entity after the editor inversion, so a
//! borrow cannot leave a read: these helpers take the assertion by closure,
//! the way every other entity read in the workbench tests does.
use super::super::*;
use gpui_kit::App;

/// The front buffer of the active tab's editor; `None` when the active tab
/// shows no file.
pub(super) fn with_active_file<R>(
    app: &LyrelApp,
    cx: &App,
    read: impl FnOnce(&OpenFile, &App) -> R,
) -> Option<R> {
    let code = app.tab_code()?;
    app.editor.read_with(cx, |editor, cx| {
        editor.active_file(code).map(|file| read(file, cx))
    })
}

/// The front input of the active tab's editor — the clone the old shell
/// `active_editor_input` handed out.
pub(super) fn active_editor_input(app: &LyrelApp, cx: &App) -> Option<EditorInput> {
    let tab = app.tabs.get(app.tabs.active_index())?.id();
    let code = app.tab_code()?;
    app.editor
        .read_with(cx, |editor, _| editor.active_editor_input(tab, code))
}
