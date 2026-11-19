// Retained whole-window tests for the `nav` module, which now lives in
// `lyrel-workbench-editor`. Keeping this anchor at the original module path
// preserves every existing test selector.

pub(crate) use lyrel_workbench_editor::code_editor::nav::*;

// Narrow imports for the retained whole-window tests, which drive `LyrelApp`.
#[cfg(test)]
use crate::ui::app::LyrelApp;
#[cfg(test)]
use crate::ui::search::{CommandKind, SearchTab};
#[cfg(test)]
use gpui_kit::component::input::Position;
#[cfg(test)]
use gpui_kit::{App, Context, Window};
#[cfg(test)]
use lyrel_workbench_editor::code_editor::*;
#[cfg(test)]
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::time::{Duration, Instant};

#[cfg(test)]
pub(super) mod gpui_tests {
    use super::super::BufferKind;
    use super::*;
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};
    use lyrel_document::text::{EditorConfig, TextFormat};

    /// `alpha` on line 0, `beta` on 30, `gamma` on 50.
    pub(crate) fn source() -> String {
        let mut lines = vec!["fn alpha() {".to_string()];
        lines.extend((1..=20).map(|_| "    let x = 1;".to_string()));
        lines.push("}".into());
        lines.extend((22..30).map(|_| String::new()));
        lines.push("fn beta() {".into());
        lines.push("}".into());
        lines.extend((32..50).map(|_| String::new()));
        lines.push("fn gamma() {".into());
        lines.push("}".into());
        lines.join("\n") + "\n"
    }

    pub(crate) fn open_rs(
        app: &Entity<LyrelApp>,
        vcx: &mut VisualTestContext,
        path: &str,
    ) -> BufferId {
        let path = PathBuf::from(path);
        app.update_in(vcx, |app, window, cx| {
            let host = app.active_host(cx).expect("a host");
            let tab = app.nav_tab().expect("a tab");
            let indent = lyrel_document::text::detect_indent("", "rust");
            let id = app.editor.update(cx, |editor, cx| {
                editor.new_buffer(
                    host,
                    path,
                    None,
                    BufferKind::Text,
                    source(),
                    TextFormat::default(),
                    EditorConfig::default(),
                    indent,
                    None,
                    window,
                    cx,
                )
            });
            app.watch_editor_buffer_focus(id, window, cx);
            app.editor_show_in_tab(tab, id, true, window, cx);
            id
        })
    }

    /// The front buffer's text caret — where navigation history sees it.
    pub(crate) fn caret(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> Position {
        app.read_with(vcx, |app, cx| {
            let code = app.tab_code().expect("a tab with code");
            app.editor
                .read(cx)
                .active_file(code)
                .unwrap()
                .input
                .text()
                .unwrap()
                .read(cx)
                .cursor_position()
        })
    }

    /// What drawing the editor does: sample the caret.
    pub(crate) fn draw(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) {
        app.update(vcx, |app, cx| app.editor_nav_tick(cx));
    }

    pub(crate) fn put_caret(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext, line: u32) {
        app.update_in(vcx, |app, window, cx| {
            let input = {
                let code = app.tab_code().expect("a tab with code");
                app.editor
                    .read(cx)
                    .active_file(code)
                    .unwrap()
                    .input
                    .code()
                    .expect("ordinary code editor")
                    .clone()
            };
            input.update(cx, |s, cx| {
                s.set_cursor_position(Position::new(line, 4), window, cx)
            });
        });
        draw(app, vcx);
    }

    #[gpui_kit::test]
    fn go_to_symbol_previews_and_escape_puts_the_caret_back(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        put_caret(&app, &mut vcx, 5);

        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_go_to_symbol(window, cx)
        });
        vcx.run_until_parked();
        let symbols = app.read_with(&vcx, |app, cx| {
            let view = app.search.view().expect("the search is open");
            assert_eq!(view.read(cx).tab(), SearchTab::Symbols);
            view.read(cx).symbol_count()
        });
        assert_eq!(symbols, 3);
        // Opening on the symbol around the caret does not move it.
        assert_eq!(caret(&app, &mut vcx), Position::new(5, 4));

        vcx.simulate_keystrokes("down");
        vcx.run_until_parked();
        assert_eq!(
            caret(&app, &mut vcx),
            Position::new(30, 3),
            "beta is previewed"
        );

        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, _| !app.search.is_open()));
        assert_eq!(caret(&app, &mut vcx), Position::new(5, 4), "and put back");
        draw(&app, &mut vcx);
        app.read_with(&vcx, |app, cx| {
            let editor = app.editor.read(cx);
            let tab = app.nav_tab().unwrap();
            assert!(
                editor.nav.tabs[&tab].history.back.is_empty(),
                "looking is not jumping"
            );
        });
    }

    /// The live input of the buffer in front — the panel's own answer to
    /// "what would the keyboard act on", resolved through the editor entity.
    fn front_input(app: &LyrelApp, cx: &App) -> EditorInput {
        let code = app.tab_code().expect("a tab with code");
        let tab = app.nav_tab().expect("a tab");
        app.editor
            .read(cx)
            .active_editor_input(tab, code)
            .expect("a file in front")
    }

    /// The ordinary code editor of the buffer in front.
    fn front_code(
        app: &LyrelApp,
        cx: &App,
    ) -> gpui_kit::Entity<gpui_kit::component::input::EditorState> {
        front_input(app, cx)
            .code()
            .expect("ordinary code editor")
            .clone()
    }

    #[gpui_kit::test]
    fn markdown_symbol_preview_cancel_restores_only_its_original_group(cx: &mut TestAppContext) {
        use gpui_kit::test::TestWindowExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("symbols.md");
        std::fs::write(&path, "# One\nfirst\n\n# Two\nsecond\n").unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !app.read_with(&vcx, |app, cx| {
            app.tab_code()
                .is_some_and(|code| app.editor.read(cx).active_file(code).is_some())
        }) {
            vcx.run_until_parked();
            assert!(Instant::now() < deadline, "Markdown open did not settle");
            std::thread::sleep(Duration::from_millis(5));
        }
        vcx.update(|window, cx| window.render_frame(cx));
        let first = app.update_in(&mut vcx, |app, window, cx| {
            let input = front_input(app, cx);
            input
                .text()
                .unwrap()
                .set_cursor_position(Position::new(1, 2), window, cx);
            app.editor_split(window, cx);
            input
        });
        vcx.update(|window, cx| window.render_frame(cx));
        let second = app.update_in(&mut vcx, |app, window, cx| {
            let input = front_input(app, cx);
            input
                .text()
                .unwrap()
                .set_cursor_position(Position::new(4, 2), window, cx);
            app.editor_go_to_symbol(window, cx);
            input
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("up");
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_focus_group(true, window, cx);
            app.close_search(window, cx);
        });
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                first.text().unwrap().read(cx).cursor_position(),
                Position::new(1, 2)
            );
            assert_eq!(
                second.text().unwrap().read(cx).cursor_position(),
                Position::new(4, 2)
            );
            let code = app.tab_code().unwrap();
            assert!(!app.editor.read(cx).active_file(code).unwrap().dirty);
        });
    }

    fn open_md(
        app: &mut LyrelApp,
        path: &Path,
        source: &str,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
    ) -> BufferId {
        let host = app.active_host(cx).expect("a host");
        let tab = app.nav_tab().expect("a tab");
        let id = app.editor.update(cx, |editor, cx| {
            editor.new_buffer(
                host,
                path.to_path_buf(),
                None,
                BufferKind::Text,
                source.into(),
                TextFormat::default(),
                EditorConfig::default(),
                lyrel_document::text::detect_indent(source, "markdown"),
                None,
                window,
                cx,
            )
        });
        app.editor_show_in_tab(tab, id, true, window, cx);
        id
    }

    #[gpui_kit::test]
    fn fresh_markdown_symbols_win_over_a_queued_breadcrumb_snapshot(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let original = "# Old\nbody\n\n# OldTarget\nbody\n";
        let current = "# Current\nbody\n\n\n\n# NewTarget\nbody\n";
        app.update_in(&mut vcx, |app, window, cx| {
            let id = open_md(app, Path::new("/nav-test/race.md"), original, window, cx);
            // Capture the old source before yielding to either executor.
            app.render_editor_breadcrumbs_for(id, 0, true, window, cx);
            let input = front_input(app, cx);
            input.markdown().unwrap().update(cx, |view, cx| {
                view.replace_range(0..original.len(), current, cx);
            });
            input
                .text()
                .unwrap()
                .set_cursor_position(Position::new(1, 2), window, cx);
            app.editor_go_to_symbol(window, cx);
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("n e w t a r g e t");
        assert_eq!(caret(&app, &mut vcx), Position::new(5, 2));
        vcx.simulate_keystrokes("escape");
        assert_eq!(caret(&app, &mut vcx), Position::new(1, 2));
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                front_input(app, cx)
                    .text()
                    .unwrap()
                    .read(cx)
                    .text()
                    .to_string(),
                current
            );
        });
        // The settled background work must not replace the next explicit query.
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_go_to_symbol(window, cx)
        });
        vcx.simulate_keystrokes("n e w t a r g e t enter");
        assert_eq!(caret(&app, &mut vcx), Position::new(5, 2));
    }

    #[gpui_kit::test]
    fn closed_markdown_breadcrumb_work_cannot_retarget_a_same_path_replacement(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let path = Path::new("/nav-test/replaced.md");
        let replacement = "# Replacement\nbody\n\n\n# Reopened\nbody\n";
        app.update_in(&mut vcx, |app, window, cx| {
            let closed = open_md(app, path, "# Departing\nbody\n", window, cx);
            app.render_editor_breadcrumbs_for(closed, 0, true, window, cx);
            app.editor_close_file(0, window, cx);
            open_md(app, path, replacement, window, cx);
            front_input(app, cx).text().unwrap().set_cursor_position(
                Position::new(1, 2),
                window,
                cx,
            );
        });
        vcx.run_until_parked();
        assert_eq!(caret(&app, &mut vcx), Position::new(1, 2));
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_go_to_symbol(window, cx)
        });
        vcx.simulate_keystrokes("r e o p e n e d enter");
        assert_eq!(caret(&app, &mut vcx), Position::new(4, 2));
        app.read_with(&vcx, |app, cx| {
            let code = app.tab_code().unwrap();
            let file = app.editor.read(cx).active_file(code).unwrap();
            assert_eq!(file.path, path);
            assert_eq!(
                file.input.text().unwrap().read(cx).text().to_string(),
                replacement
            );
            assert!(!file.dirty);
        });
    }

    #[gpui_kit::test]
    fn a_symbol_jump_can_be_walked_back_and_forward(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        put_caret(&app, &mut vcx, 5);

        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_go_to_symbol(window, cx)
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("g a m m a");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, _| !app.search.is_open()));
        assert_eq!(caret(&app, &mut vcx), Position::new(50, 3));
        draw(&app, &mut vcx);

        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_navigate(false, window, cx)
        });
        vcx.run_until_parked();
        assert_eq!(
            caret(&app, &mut vcx),
            Position::new(5, 4),
            "back to where it was"
        );
        draw(&app, &mut vcx);

        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_navigate(true, window, cx)
        });
        vcx.run_until_parked();
        assert_eq!(
            caret(&app, &mut vcx),
            Position::new(50, 3),
            "and forward again"
        );
        draw(&app, &mut vcx);

        // Another file coming forward is a jump too, and Back returns to the
        // first one.
        open_rs(&app, &mut vcx, "/nav-test/b.rs");
        draw(&app, &mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_navigate(false, window, cx)
        });
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            let code = app.tab_code().unwrap();
            assert_eq!(
                app.editor.read(cx).active_file(code).unwrap().path,
                PathBuf::from("/nav-test/a.rs")
            );
        });
        assert_eq!(caret(&app, &mut vcx), Position::new(50, 3));
    }

    #[gpui_kit::test]
    fn history_follows_edits_to_the_file(cx: &mut TestAppContext) {
        use gpui_kit::EntityInputHandler as _;
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        put_caret(&app, &mut vcx, 5);
        put_caret(&app, &mut vcx, 50);
        // Three lines typed at the top of the file.
        app.update_in(&mut vcx, |app, window, cx| {
            let input = front_code(app, cx);
            input.update(cx, |s, cx| {
                s.replace_text_in_range(Some(0..0), "//\n//\n//\n", window, cx);
                // Typing took the caret to the top; back to where it was
                // reading, which has moved down with its line.
                s.set_cursor_position(Position::new(53, 4), window, cx);
            });
        });
        vcx.run_until_parked();
        draw(&app, &mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_navigate(false, window, cx)
        });
        vcx.run_until_parked();
        assert_eq!(
            caret(&app, &mut vcx).line,
            8,
            "the place moved down with its code"
        );
    }

    #[gpui_kit::test]
    fn back_and_forward_follow_a_completion_and_import_in_one_replacement(cx: &mut TestAppContext) {
        use gpui_kit::EntityInputHandler as _;
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        put_caret(&app, &mut vcx, 5);
        put_caret(&app, &mut vcx, 30);
        put_caret(&app, &mut vcx, 50);
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_navigate(false, window, cx)
        });
        vcx.run_until_parked();
        assert_eq!(caret(&app, &mut vcx).line, 30);

        // The completion consumer applies its main edit and file-header import
        // as one public input replacement. Keep beta between those edits.
        let before = source();
        let after = format!(
            "use std::fmt;\n{}",
            before.replace("fn gamma()", "fn gamma_completed()")
        );
        app.update_in(&mut vcx, |app, window, cx| {
            let input = front_code(app, cx);
            input.update(cx, |state, cx| {
                state.replace_text_in_range(
                    Some(0..before.encode_utf16().count()),
                    &after,
                    window,
                    cx,
                );
                state.set_cursor_position(Position::new(31, 4), window, cx);
            });
        });
        vcx.run_until_parked();
        draw(&app, &mut vcx);

        for (forward, expected) in [(false, 6), (true, 31), (true, 51)] {
            app.update_in(&mut vcx, |app, window, cx| {
                app.editor_navigate(forward, window, cx)
            });
            vcx.run_until_parked();
            assert_eq!(caret(&app, &mut vcx).line, expected);
            draw(&app, &mut vcx);
        }
    }

    #[gpui_kit::test]
    fn navigation_targets_follow_real_undo_and_redo_of_multiline_edits(cx: &mut TestAppContext) {
        use gpui_kit::EntityInputHandler as _;
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        put_caret(&app, &mut vcx, 30);
        put_caret(&app, &mut vcx, 50);
        let before = source();
        let after = format!(
            "use std::fmt;\n{}",
            before.replace("fn gamma()", "fn gamma_completed()")
        );
        app.update_in(&mut vcx, |app, window, cx| {
            let input = front_code(app, cx);
            input.update(cx, |state, cx| {
                state.replace_text_in_range(
                    Some(0..before.encode_utf16().count()),
                    &after,
                    window,
                    cx,
                );
                state.set_cursor_position(Position::new(51, 4), window, cx);
            });
        });
        vcx.run_until_parked();
        draw(&app, &mut vcx);

        for (undo, expected_text, target_line) in [(true, &before, 30), (false, &after, 31)] {
            if undo {
                vcx.dispatch_action(gpui_kit::component::input::Undo);
            } else {
                vcx.dispatch_action(gpui_kit::component::input::Redo);
            }
            vcx.run_until_parked();
            app.read_with(&vcx, |app, cx| {
                assert_eq!(
                    front_input(app, cx)
                        .text()
                        .unwrap()
                        .read(cx)
                        .text()
                        .to_string(),
                    *expected_text
                );
            });
            draw(&app, &mut vcx);
            let from = caret(&app, &mut vcx);
            app.update_in(&mut vcx, |app, window, cx| {
                app.editor_navigate(false, window, cx)
            });
            vcx.run_until_parked();
            assert_eq!(caret(&app, &mut vcx).line, target_line);
            draw(&app, &mut vcx);
            app.update_in(&mut vcx, |app, window, cx| {
                app.editor_navigate(true, window, cx)
            });
            vcx.run_until_parked();
            assert_eq!(caret(&app, &mut vcx), from);
            draw(&app, &mut vcx);
        }
    }

    #[gpui_kit::test]
    fn the_symbol_chord_puts_go_to_symbol_away(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        put_caret(&app, &mut vcx, 5);
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_go_to_symbol(window, cx)
        });
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, _| app.search.is_open()));
        vcx.simulate_keystrokes("secondary-shift-o");
        vcx.run_until_parked();
        assert!(
            app.read_with(&vcx, |app, _| !app.search.is_open()),
            "the chord that opened it closes it"
        );
        assert_eq!(caret(&app, &mut vcx), Position::new(5, 4));
    }

    /// Go to File and Go to Symbol share the editor's row: Tab walks from one
    /// to the other, each arriving set up the way its chord sets it up.
    #[gpui_kit::test]
    fn tab_walks_the_editors_row(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        // No file in front: Files is all the row would hold.
        assert_eq!(
            app.read_with(&vcx, |app, cx| app.editor_search_tabs(cx)),
            vec![SearchTab::Files]
        );
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        assert_eq!(
            app.read_with(&vcx, |app, cx| app.editor_search_tabs(cx)),
            vec![SearchTab::Files, SearchTab::Symbols]
        );
        app.update_in(&mut vcx, |app, window, cx| app.quick_open_file(window, cx));
        vcx.run_until_parked();
        let tab = |app: &Entity<LyrelApp>, vcx: &mut gpui_kit::VisualTestContext| {
            app.read_with(vcx, |app, cx| app.search.view().map(|v| v.read(cx).tab()))
        };
        assert_eq!(tab(&app, &mut vcx), Some(SearchTab::Files));
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        assert_eq!(tab(&app, &mut vcx), Some(SearchTab::Symbols));
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        assert_eq!(tab(&app, &mut vcx), Some(SearchTab::Files));
    }

    #[gpui_kit::test]
    fn the_palette_offers_the_editor_commands_only_with_a_file_open(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        assert!(app.read_with(&vcx, |app, cx| app.editor_palette_items(cx).is_empty()));
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        let kinds: Vec<CommandKind> = app.read_with(&vcx, |app, cx| {
            app.editor_palette_items(cx)
                .into_iter()
                .map(|i| i.kind)
                .collect()
        });
        assert_eq!(
            kinds,
            [
                CommandKind::EditorGoToSymbol,
                CommandKind::EditorNavigateBack,
                CommandKind::EditorNavigateForward,
                CommandKind::EditorSplitRight,
                CommandKind::EditorFocusLeftGroup,
                CommandKind::EditorFocusRightGroup,
                CommandKind::FocusDocumentTabNavigation,
                CommandKind::EditorDocumentOverflow,
            ]
        );
    }
}
