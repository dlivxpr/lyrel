//! Whole-window consumer tests and native-case anchors for the editor
//! workbench that now lives in `lyrel-workbench-editor`. Keeping this module
//! at `crate::ui::code_editor` preserves every test selector; production code
//! consumes `lyrel_workbench_editor::code_editor` directly.

pub(crate) use lyrel_workbench_editor::code_editor::*;

// Inherited by the retained tests from the production module's scope before
// the move; kept here so their `use super::*` resolves unchanged.
#[cfg(any(test, feature = "native-markdown-tests"))]
use crate::ui::app::LyrelApp;
#[cfg(any(test, feature = "native-markdown-tests"))]
use gpui_kit::component::input::Position;
use gpui_kit::{Entity, EntityInputHandler as _, Focusable as _, SharedString, px};
#[cfg(any(test, feature = "native-markdown-tests"))]
use lyrel_ui_base::host_ops::HostId;
#[cfg(any(test, feature = "native-markdown-tests"))]
use lyrel_ui_base::i18n::L10nKey;
#[cfg(any(test, feature = "native-markdown-tests"))]
use std::path::{Path, PathBuf};

#[cfg(any(test, feature = "native-markdown-tests"))]
pub(crate) use lyrel_workbench_editor::code_editor::test_host;

#[cfg(any(test, feature = "native-markdown-tests"))]
use gpui_kit::MouseButton;

#[cfg(test)]
pub(crate) mod book_view;
#[cfg(test)]
pub(crate) mod buffers;
#[cfg(test)]
pub(crate) mod commands;
#[cfg(test)]
pub(crate) mod csv_document;
pub(crate) mod document_input;
#[cfg(test)]
pub(crate) mod drawio_document;
#[cfg(test)]
pub(crate) mod gutter;
#[cfg(test)]
pub(crate) mod image_document;
#[cfg(test)]
pub(crate) mod json_document;
pub(crate) mod markdown;
pub(crate) mod markdown_formula;
#[cfg(test)]
pub(crate) mod markdown_search;
#[cfg(test)]
pub(crate) mod nav;
#[cfg(test)]
pub(crate) mod pdf_document;
#[cfg(test)]
pub(crate) mod problems;
pub(crate) mod saving;
#[cfg(test)]
pub(crate) mod split;
#[cfg(test)]
pub(crate) mod sqlite_document;
#[cfg(test)]
pub(crate) mod strip;
#[cfg(test)]
pub(crate) mod svg_document;
#[cfg(test)]
pub(crate) mod toml_document;
#[cfg(test)]
pub(crate) mod xml_document;
#[cfg(test)]
pub(crate) mod yaml_document;

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp dir as the editor will see paths under it: through the local
    /// host's `canonicalize`, the call `load_file` makes. That resolves
    /// macOS's `/var` → `/private/var` and Windows' 8.3 short names
    /// (`RUNNER~1`), and never answers in Windows' `\\?\` form.
    pub(crate) fn test_real_dir(dir: &Path) -> PathBuf {
        lyrel_core::host::local::LocalHost::new()
            .canonicalize(dir)
            .unwrap()
    }
}

#[cfg(test)]
mod close_gpui_tests {
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};

    use super::nav::gpui_tests::open_rs;
    use super::*;
    use crate::core::actions::CloseActiveTab;
    use crate::ui::app::test_window::harness_with_tabs;
    use std::sync::Arc;

    fn panel(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> (bool, usize) {
        app.read_with(vcx, |app, _| {
            let code = app.tab_code().expect("the tab is still there");
            (code.visible, code.files.len())
        })
    }

    /// Runs what is pending and paints a frame: the panel notes whether it
    /// holds the focus as it is drawn.
    fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
        vcx.run_until_parked();
    }

    fn terminal_focused(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> bool {
        app.update_in(vcx, |app, window, cx| {
            app.tabs[app.tabs.active_index()]
                .pane
                .leaves()
                .iter()
                .any(|l| l.contains_focused(window, cx))
        })
    }

    /// ⌘W, ⌘W on an editor with one file closes the file and then the empty
    /// editor. The second one used to close the terminal: closing the last
    /// file left nothing in the panel to hold the focus, so it fell back to
    /// the terminal pane.
    #[gpui_kit::test]
    fn closing_the_last_file_keeps_close_on_the_editor(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/close/a.rs");
        settle(&mut vcx);
        assert_eq!(panel(&app, &mut vcx), (true, 1));

        vcx.dispatch_action(CloseActiveTab);
        settle(&mut vcx);
        assert_eq!(panel(&app, &mut vcx), (true, 0), "the file closes first");
        assert!(!terminal_focused(&app, &mut vcx));

        vcx.dispatch_action(CloseActiveTab);
        settle(&mut vcx);
        assert_eq!(panel(&app, &mut vcx), (false, 0), "then the empty editor");
        assert!(
            terminal_focused(&app, &mut vcx),
            "and the terminal has it back"
        );

        let tab = app.read_with(&vcx, |app, _| app.tabs[0].id());
        vcx.dispatch_action(CloseActiveTab);
        settle(&mut vcx);
        assert!(
            app.read_with(&vcx, |app, _| app.tabs.iter().all(|t| t.id() != tab)),
            "only now is ⌘W the terminal's"
        );
    }

    /// A tab switched to keeps its focus in its terminal, even with an empty
    /// editor showing and the focus in the editor of the tab left behind.
    #[gpui_kit::test]
    fn switching_to_an_empty_editor_leaves_the_focus_alone(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        crate::ui::i18n::set_locale("en");
        app.update_in(&mut vcx, |app, _, _| {
            app.tabs.select(1);
            app.tab_code_mut_or_init().unwrap().visible = true;
            app.tabs.select(0);
        });
        open_rs(&app, &mut vcx, "/close/a.rs");
        settle(&mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.tabs.select(1);
            app.focus_active(window, cx);
        });
        settle(&mut vcx);
        assert_eq!(panel(&app, &mut vcx), (true, 0));
        assert!(terminal_focused(&app, &mut vcx));
    }

    /// With the focus in the terminal beside an empty editor, ⌘W is still
    /// the terminal's.
    #[gpui_kit::test]
    fn close_from_the_terminal_is_still_the_terminals(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        app.update_in(&mut vcx, |app, window, cx| {
            app.tab_code_mut_or_init().unwrap().visible = true;
            app.focus_active(window, cx);
        });
        vcx.run_until_parked();
        assert!(terminal_focused(&app, &mut vcx));
        assert!(!app.update_in(&mut vcx, |app, window, cx| {
            app.editor_close_active_if_focused(window, cx)
        }));
        assert_eq!(panel(&app, &mut vcx), (true, 0));
    }

    #[gpui_kit::test]
    fn document_dock_fill_and_close_keep_the_same_editor_and_terminal(cx: &mut TestAppContext) {
        use crate::core::actions::{
            ActivateTab1, ActivateTab2, EditorNewFile, ToggleCodePanel, ToggleDocumentFill,
        };
        use gpui_kit::size;

        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        vcx.simulate_resize(size(px(1600.), px(900.)));
        for collapsed in [false, true] {
            app.update_in(&mut vcx, |app, window, cx| {
                app.sidebar_collapsed = collapsed;
                app.right_panel_visible = true;
                app.focus_active(window, cx);
                cx.notify();
            });
            settle(&mut vcx);
            let pane = app.read_with(&vcx, |app, _| {
                app.tabs[0].pane.first_leaf().unwrap().entity_id()
            });
            vcx.dispatch_action(EditorNewFile);
            settle(&mut vcx);
            let editor = app.read_with(&vcx, |app, cx| {
                app.active_buffer(cx)
                    .unwrap()
                    .input
                    .code()
                    .expect("ordinary code editor")
                    .clone()
            });
            assert!(app.update_in(&mut vcx, |app, window, cx| {
                app.document_dock_px(window, cx).is_some()
            }));
            for docked in [false, true] {
                vcx.dispatch_action(ToggleDocumentFill);
                settle(&mut vcx);
                assert_eq!(
                    app.update_in(&mut vcx, |app, window, cx| {
                        app.document_dock_px(window, cx).is_some()
                    }),
                    docked,
                );
                app.read_with(&vcx, |app, cx| {
                    assert_eq!(
                        app.active_buffer(cx)
                            .unwrap()
                            .input
                            .code()
                            .expect("ordinary code editor")
                            .entity_id(),
                        editor.entity_id()
                    );
                    assert_eq!(app.tabs[0].pane.first_leaf().unwrap().entity_id(), pane);
                });
                vcx.update(|window, _| {
                    assert!(window.find("editor-panel-close").visible());
                    assert_eq!(window.find_all("editor-panel-close").len(), 1);
                });
            }
            vcx.update(|window, cx| window.click("editor-panel-close", cx));
            settle(&mut vcx);
            assert!(!app.read_with(&vcx, |app, _| app.code_panel_visible()));
            assert!(terminal_focused(&app, &mut vcx));
            vcx.dispatch_action(ToggleCodePanel);
            settle(&mut vcx);
            assert_eq!(
                app.read_with(&vcx, |app, cx| app
                    .active_buffer(cx)
                    .unwrap()
                    .input
                    .code()
                    .expect("ordinary code editor")
                    .entity_id()),
                editor.entity_id()
            );
            vcx.dispatch_action(CloseActiveTab);
            settle(&mut vcx);
            assert!(app.read_with(&vcx, |app, _| app.code_panel_visible()));
            vcx.dispatch_action(CloseActiveTab);
            settle(&mut vcx);
            assert!(!app.read_with(&vcx, |app, _| app.code_panel_visible()));
            assert!(terminal_focused(&app, &mut vcx));
            assert_eq!(app.read_with(&vcx, |app, _| app.tabs.len()), 2);
            vcx.dispatch_action(ActivateTab2);
            settle(&mut vcx);
            assert_eq!(app.read_with(&vcx, |app, _| app.tabs.active_index()), 1);
            vcx.dispatch_action(ActivateTab1);
            settle(&mut vcx);
        }
    }

    #[gpui_kit::test]
    fn docked_status_follows_terminal_focus_and_recovers_document_actions(cx: &mut TestAppContext) {
        use crate::core::actions::EditorNewFile;
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(gpui_kit::size(px(1600.), px(900.)));
        vcx.dispatch_action(EditorNewFile);
        settle(&mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            let handle = app.tabs[0].pane.first_leaf().unwrap().focus_handle(cx);
            window.focus(&handle, cx);
        });
        settle(&mut vcx);
        vcx.update(|window, _| {
            let local = window.find("code-status-bar");
            let global = window.find("workbench-status");
            let cursor = window.find("status-cursor");
            assert!(local.bounds().contains(&cursor.bounds().center()));
            assert!(!global.bounds().contains(&cursor.bounds().center()));
        });
        app.update_in(&mut vcx, |app, window, cx| app.focus_editor(window, cx));
        settle(&mut vcx);
        vcx.update(|window, cx| {
            assert!(window.find("status-cursor").visible());
            window.click("status-problems", cx);
        });
        settle(&mut vcx);
        vcx.update(|window, _| assert!(window.find("editor-problems").visible()));
        vcx.update(|window, cx| window.click("editor-panel-close", cx));
        settle(&mut vcx);
        assert!(terminal_focused(&app, &mut vcx));
        vcx.update(|window, _| assert!(window.try_find("status-cursor").is_none()));
    }

    #[gpui_kit::test]
    fn diff_status_uses_its_host_path_not_the_covered_document(cx: &mut TestAppContext) {
        use crate::core::actions::{EditorNewFile, ToggleCodePanel};
        use crate::ui::scm::diff::{DiffSnapshot, DiffSource};
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(gpui_kit::size(px(1600.), px(900.)));
        vcx.dispatch_action(EditorNewFile);
        settle(&mut vcx);
        let remote = HostId::from_connection_key("status-remote-host");
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_supplied_diff(
                remote,
                Arc::new(DiffSnapshot {
                    root: PathBuf::from("/srv/项目 with spaces/repository"),
                    source: DiffSource::Patch {
                        id: "status-context".into(),
                        label: None,
                    },
                    ..Default::default()
                }),
                None,
                window,
                cx,
            );
        });
        settle(&mut vcx);
        vcx.update(|window, cx| {
            assert!(window.try_find("status-cursor").is_none());
            window.click("status-path", cx);
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some("/srv/项目 with spaces/repository")
            );
        });
        // Raising the still-open editor restores its own position controls.
        vcx.dispatch_action(ToggleCodePanel);
        settle(&mut vcx);
        vcx.update(|window, _| assert!(window.find("status-cursor").visible()));
    }

    #[gpui_kit::test]
    fn settings_escape_restores_the_current_document_or_docked_terminal(cx: &mut TestAppContext) {
        use crate::core::actions::{EditorNewFile, ToggleDocumentFill};
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(gpui_kit::size(px(1600.), px(900.)));
        vcx.dispatch_action(EditorNewFile);
        settle(&mut vcx);
        // Both dock and fill must return typing to the document, not its shell.
        for text in ["docked", "filled"] {
            if text == "filled" {
                vcx.dispatch_action(ToggleDocumentFill);
                settle(&mut vcx);
            }
            vcx.update(|window, cx| window.click("status-settings", cx));
            settle(&mut vcx);
            assert!(app.read_with(&vcx, |app, _| app.settings.is_some()));
            vcx.simulate_keystrokes("escape");
            settle(&mut vcx);
            assert!(app.update_in(&mut vcx, |app, window, cx| app.editor_has_focus(window, cx)));
            vcx.update(|window, cx| window.input(text, cx));
            settle(&mut vcx);
        }
        assert_eq!(
            app.read_with(&vcx, |app, cx| app
                .active_buffer(cx)
                .unwrap()
                .input
                .text()
                .unwrap()
                .read(cx)
                .text()
                .to_string()),
            "dockedfilled"
        );
        vcx.dispatch_action(ToggleDocumentFill);
        app.update_in(&mut vcx, |app, window, cx| {
            let handle = app.tabs[0].pane.first_leaf().unwrap().focus_handle(cx);
            window.focus(&handle, cx);
        });
        settle(&mut vcx);
        vcx.update(|window, cx| window.click("status-settings", cx));
        settle(&mut vcx);
        vcx.simulate_keystrokes("escape");
        settle(&mut vcx);
        assert!(terminal_focused(&app, &mut vcx));
    }
}

#[cfg(test)]
mod pane_scrollback_tests {
    use gpui_kit::TestAppContext;

    #[gpui_kit::test]
    fn scrollback_action_opens_a_saveable_unicode_draft_without_changing_the_pane(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
        let pane = app.update_in(&mut vcx, |app, window, cx| {
            let pane = app.tabs[0].pane.terminals()[0].clone();
            let terminal = pane.read(cx).terminal.term.clone();
            let output = format!(
                "\x1b[31m历史 e\u{301} 🎉\x1b[0m\r\nsecond line\r\n{}",
                "retained history\r\n".repeat(40),
            );
            rio_vt::performer::handler::Processor::default()
                .advance(&mut *terminal.lock(), output.as_bytes());
            let focus = pane.read(cx).focus_handle.clone();
            window.focus(&focus, cx);
            pane
        });
        vcx.dispatch_action(crate::core::actions::OpenScrollbackInEditor);
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, cx| {
            let file = app
                .active_buffer(cx)
                .expect("scrollback opens a real editor buffer");
            let text = file.input.text().unwrap().read(cx).text().to_string();
            assert!(
                text.starts_with("历史 e\u{301} 🎉\nsecond line"),
                "{text:?}"
            );
            assert!(!text.contains('\x1b'));
            assert!(file.untitled.is_some());
            assert!(
                file.dirty,
                "closing the draft must offer Save As instead of losing it"
            );
            assert!(app.editor_has_focus(window, cx));
            assert_eq!(app.tabs[0].pane.terminals()[0], pane);
        });
    }
}
