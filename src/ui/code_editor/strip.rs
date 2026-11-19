// Retained whole-window tests for the `strip` module, which now lives in
// `lyrel-workbench-editor`. Keeping this anchor at the original module path
// preserves every existing test selector.

// Narrow imports for the retained whole-window tests, which drive `LyrelApp`.
#[cfg(test)]
use crate::ui::app::LyrelApp;
#[cfg(test)]
use lyrel_ui_base::i18n::L10nKey;
#[cfg(test)]
use lyrel_workbench_editor::code_editor::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u64) -> Vec<BufferId> {
        (0..n).map(|_| BufferId::new()).collect()
    }

    fn code(files: &[BufferId], active: usize) -> TabCode {
        let mut code = TabCode::new();
        code.files = files.to_vec();
        code.active = active;
        code
    }

    #[test]
    fn the_strip_keeps_the_files_last_in_front_in_their_strip_order() {
        let ids = ids(5);
        let mut code = code(&ids, 0);
        for front in [3, 1, 4] {
            code.active = front;
            code.note_front();
        }
        let (shown, hidden) = code.strip_split(3);
        // 4, 1 and 3 were in front last; they keep their strip positions.
        assert_eq!(shown, vec![(1, ids[1]), (3, ids[3]), (4, ids[4])]);
        // The rest by recency: 0 was in front before any of them, 2 never.
        assert_eq!(hidden, vec![ids[0], ids[2]]);
    }

    #[test]
    fn the_file_in_front_always_has_a_place() {
        let ids = ids(4);
        let mut code = code(&ids, 0);
        code.note_front();
        // Brought forward from outside the strip's view (a link, the list)
        // without a frame drawn in between.
        code.active = 3;
        let (shown, _) = code.strip_split(1);
        assert_eq!(shown, vec![(3, ids[3])]);
    }

    #[test]
    fn a_split_keeps_the_other_groups_order() {
        let ids = ids(4);
        let mut code = code(&ids[..2], 1);
        code.split = Some(super::super::split::OtherGroup {
            id: super::super::split::GroupId::default(),
            files: ids[2..].to_vec(),
            active: 0,
            focus_left: true,
        });
        code.note_front();
        code.swap_focus();
        code.note_front();
        code.swap_focus();
        code.note_front();
        assert_eq!(code.recent, vec![ids[1], ids[0], ids[2], ids[3]]);
    }

    #[test]
    fn closed_files_leave_the_order() {
        let ids = ids(3);
        let mut code = code(&ids, 2);
        code.note_front();
        code.files.remove(1);
        code.active = 1;
        code.note_front();
        assert_eq!(code.recent, vec![ids[2], ids[0]]);
    }
}
#[cfg(test)]
mod gpui_tests {
    use std::path::PathBuf;

    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};
    use lyrel_document::text::{EditorConfig, TextFormat};

    use super::super::nav::gpui_tests::open_rs;
    use super::super::{BufferKind, test_host};
    use super::*;
    use crate::core::actions::{
        FocusDocumentTabNavigation, FocusRightPanelNavigation, FocusTerminalTabNavigation,
    };
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::host_ops::HostId;

    fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
        vcx.run_until_parked();
    }

    fn wait_until(
        vcx: &mut VisualTestContext,
        mut ready: impl FnMut(&mut VisualTestContext) -> bool,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            settle(vcx);
            if ready(vcx) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the editor never settled"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// The document in front, by its path — the identity a user sees.
    fn front(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> PathBuf {
        app.read_with(vcx, |app, cx| {
            let code = app.tab_code().unwrap();
            app.editor.read(cx).active_file(code).unwrap().path.clone()
        })
    }

    fn strip_has_keys(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> bool {
        vcx.update(|window, cx| {
            app.read(cx)
                .editor
                .read(cx)
                .strip_keyboard
                .is_focused(window)
        })
    }

    fn content_has_keys(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> bool {
        vcx.update(|window, cx| {
            let app = app.read(cx);
            let Some(code) = app.tab_code() else {
                return false;
            };
            let Some(tab) = app.tabs.get(app.tabs.active_index()) else {
                return false;
            };
            app.editor
                .read(cx)
                .active_editor_input(tab.id(), code)
                .is_some_and(|input| input.read(cx).focus_handle(cx).contains_focused(window, cx))
        })
    }

    fn picker_has_keys(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> bool {
        vcx.update(|window, cx| {
            app.read(cx)
                .editor
                .read(cx)
                .strip_picker_has_focus(window, cx)
        })
    }

    fn sidebar_has_keys(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> bool {
        vcx.update(|window, cx| app.read(cx).sidebar_nav_focus.is_focused(window))
    }

    fn dirty(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext, id: BufferId) -> bool {
        app.read_with(vcx, |app, cx| {
            app.editor.read(cx).buffers().get(id).unwrap().dirty
        })
    }

    #[gpui_kit::test]
    fn the_strip_is_one_tab_stop_and_activation_leaves_the_keys_in_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/strip/a.rs");
        open_rs(&app, &mut vcx, "/strip/b.rs");
        open_rs(&app, &mut vcx, "/strip/c.rs");
        settle(&mut vcx);
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/c.rs"));
        assert!(
            content_has_keys(&app, &mut vcx),
            "the text starts with the keys"
        );
        assert!(!strip_has_keys(&app, &mut vcx));

        // Entering the focused group's strip takes the keyboard and brings
        // nothing else forward.
        vcx.simulate_keystrokes("secondary-alt-t");
        assert!(strip_has_keys(&app, &mut vcx));
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/c.rs"));

        // Tab leaves the one-stop group and Shift+Tab comes back to it.
        vcx.simulate_keystrokes("tab");
        assert!(!strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("shift-tab");
        assert!(strip_has_keys(&app, &mut vcx));

        // Moving the focused cell does not activate.
        vcx.simulate_keystrokes("left");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/c.rs"));

        // Enter activates the focused cell and keeps the strip navigable.
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/b.rs"));
        assert!(strip_has_keys(&app, &mut vcx));

        // Home and End reach the ends.
        vcx.simulate_keystrokes("home");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/a.rs"));
        vcx.simulate_keystrokes("end");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/c.rs"));

        // Right wraps from the last cell to the first; Left wraps back.
        vcx.simulate_keystrokes("right");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/a.rs"));
        vcx.simulate_keystrokes("left");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/c.rs"));

        // Space activates as Enter does.
        vcx.simulate_keystrokes("left");
        vcx.simulate_keystrokes("space");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/b.rs"));
        assert!(strip_has_keys(&app, &mut vcx));

        // Escape hands the keyboard back to the owning editor.
        vcx.simulate_keystrokes("escape");
        assert!(!strip_has_keys(&app, &mut vcx));
        assert!(content_has_keys(&app, &mut vcx));
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/b.rs"));

        // Keys typed in the text never move the strip's cell: the remembered
        // cell is still b, so re-entering and activating stays on b.
        vcx.simulate_keystrokes("left");
        vcx.simulate_keystrokes("secondary-alt-t");
        assert!(strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/b.rs"));
    }

    #[gpui_kit::test]
    fn the_shared_strip_keeps_each_groups_cell_across_a_switch(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/strip/left.rs");
        app.update_in(&mut vcx, |app, window, cx| {
            assert!(app.editor_split(window, cx), "the editor splits");
        });
        open_rs(&app, &mut vcx, "/strip/right.rs");
        open_rs(&app, &mut vcx, "/strip/right2.rs");
        settle(&mut vcx);
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/right2.rs"));
        assert!(!app.read_with(&vcx, |app, _| app.tab_code().unwrap().focus_is_left()));

        // The right group's strip, with its front file as the first cell.
        vcx.simulate_keystrokes("secondary-alt-t");
        assert!(strip_has_keys(&app, &mut vcx));
        // Move to the other cell without activating it.
        vcx.simulate_keystrokes("left");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/right2.rs"));

        // Choosing the left group while the strip holds the keyboard keeps
        // the keyboard in the shared strip, which now shows that group.
        vcx.simulate_keystrokes("secondary-alt-left");
        assert!(
            strip_has_keys(&app, &mut vcx),
            "the strip follows the group"
        );
        assert!(app.read_with(&vcx, |app, _| app.tab_code().unwrap().focus_is_left()));
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/left.rs"));

        // Back in the right group, Enter still activates the cell it was on —
        // the group's own remembered document, not the one in front.
        vcx.simulate_keystrokes("secondary-alt-right");
        assert!(strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/right.rs"));

        // The left group kept its own front, and Escape returns to its text.
        vcx.simulate_keystrokes("secondary-alt-left");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/left.rs"));
        assert!(strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("escape");
        assert!(content_has_keys(&app, &mut vcx));
        assert!(!strip_has_keys(&app, &mut vcx));
    }

    #[gpui_kit::test]
    fn the_hidden_document_list_opens_from_the_keyboard_and_restores_focus(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        for name in ["alpha", "beta", "gamma", "delta", "epsilon"] {
            open_rs(&app, &mut vcx, &format!("/strip/{name}.rs"));
        }
        settle(&mut vcx);
        // A docked, unsplit editor keeps three names; beta and alpha are the
        // hidden pair.
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/epsilon.rs"));

        // Open the list from the strip's own keyboard and filter it down to a
        // document the strip does not show.
        vcx.simulate_keystrokes("secondary-alt-t");
        assert!(strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("secondary-alt-shift-t");
        assert!(picker_has_keys(&app, &mut vcx));

        // The picker is above both navigation surfaces. Their entry actions
        // cannot pull the keyboard out of it or expose the strip behind it.
        vcx.dispatch_action(FocusTerminalTabNavigation);
        vcx.dispatch_action(FocusRightPanelNavigation);
        vcx.dispatch_action(FocusDocumentTabNavigation);
        assert!(picker_has_keys(&app, &mut vcx));
        assert!(!sidebar_has_keys(&app, &mut vcx));
        assert!(!strip_has_keys(&app, &mut vcx));

        vcx.simulate_keystrokes("a l p h a");
        vcx.simulate_keystrokes("enter");
        settle(&mut vcx);
        assert_eq!(
            front(&app, &mut vcx),
            PathBuf::from("/strip/alpha.rs"),
            "the hidden document came forward"
        );
        assert!(
            strip_has_keys(&app, &mut vcx),
            "confirming a keyboard-opened picker restores the strip"
        );

        // The newly-fronted hidden document is the strip's valid roving cell:
        // Right and Enter activate its visible neighbour, then Home returns.
        vcx.simulate_keystrokes("right");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/alpha.rs"));
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/delta.rs"));
        assert!(strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("home");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/alpha.rs"));
        assert!(strip_has_keys(&app, &mut vcx));

        // Cancelling restores the keyboard to the strip it was opened from.
        vcx.simulate_keystrokes("secondary-alt-t");
        vcx.simulate_keystrokes("secondary-alt-shift-t");
        vcx.simulate_keystrokes("b e t a");
        vcx.simulate_keystrokes("escape");
        assert!(
            strip_has_keys(&app, &mut vcx),
            "cancelling returns to the strip"
        );
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/alpha.rs"));

        // Opened from the text, cancelling returns to the text.
        vcx.simulate_keystrokes("escape");
        assert!(content_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("secondary-alt-shift-t");
        vcx.simulate_keystrokes("escape");
        assert!(content_has_keys(&app, &mut vcx));
        assert!(!strip_has_keys(&app, &mut vcx));
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/alpha.rs"));
    }

    #[gpui_kit::test]
    fn terminal_content_enters_the_document_strip_through_the_root_keymap(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        for name in ["a", "b", "c"] {
            open_rs(&app, &mut vcx, &format!("/strip/{name}.rs"));
        }
        app.update_in(&mut vcx, |app, window, cx| {
            app.set_document_layout(crate::core::config::DocumentLayout::Dock, cx);
            app.focus_active(window, cx);
        });
        settle(&mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            let tab = &app.tabs[app.tabs.active_index()];
            assert!(
                tab.pane
                    .leaves()
                    .iter()
                    .any(|leaf| leaf.contains_focused(window, cx))
            );
        });

        // This is the real terminal key context and Root action consumer, not
        // a direct call to the strip's focus method.
        vcx.simulate_keystrokes("secondary-alt-t");
        assert!(strip_has_keys(&app, &mut vcx));
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/c.rs"));

        vcx.simulate_keystrokes("right");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/c.rs"));
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/a.rs"));
        assert!(strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("tab");
        assert!(!strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("shift-tab");
        assert!(strip_has_keys(&app, &mut vcx));

        // Document navigation's explicit return route is the owning editor.
        vcx.simulate_keystrokes("escape");
        assert!(content_has_keys(&app, &mut vcx));
        assert!(!strip_has_keys(&app, &mut vcx));
    }

    #[gpui_kit::test]
    fn editor_content_enters_sidebar_navigation_and_returns_to_the_editor(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/strip/editor.rs");
        settle(&mut vcx);
        assert!(content_has_keys(&app, &mut vcx));

        vcx.dispatch_action(FocusTerminalTabNavigation);
        assert!(sidebar_has_keys(&app, &mut vcx));
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/editor.rs"));
        vcx.simulate_keystrokes("enter");
        assert!(sidebar_has_keys(&app, &mut vcx));
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/editor.rs"));

        vcx.simulate_keystrokes("tab");
        assert!(!sidebar_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("shift-tab");
        assert!(sidebar_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("escape");
        assert!(content_has_keys(&app, &mut vcx));
        assert!(!sidebar_has_keys(&app, &mut vcx));
    }

    #[gpui_kit::test]
    fn pointer_opened_hidden_document_list_keeps_content_focus(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        for name in ["alpha", "beta", "gamma", "delta", "epsilon"] {
            open_rs(&app, &mut vcx, &format!("/strip/{name}.rs"));
        }
        settle(&mut vcx);
        vcx.simulate_keystrokes("secondary-alt-t");
        assert!(strip_has_keys(&app, &mut vcx));

        // Even when the strip had focus before the pointer press, opening the
        // picker is a pointer interaction and selection keeps pointer focus.
        vcx.update(|window, cx| window.click("editor-strip-more", cx));
        settle(&mut vcx);
        assert!(picker_has_keys(&app, &mut vcx));
        vcx.update(|window, cx| window.click("editor-strip-row-0", cx));
        settle(&mut vcx);
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/beta.rs"));
        assert!(content_has_keys(&app, &mut vcx));
        assert!(!strip_has_keys(&app, &mut vcx));
    }

    #[gpui_kit::test]
    fn closing_a_document_leaves_a_live_strip_cell(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/strip/a.rs");
        let b = open_rs(&app, &mut vcx, "/strip/b.rs");
        open_rs(&app, &mut vcx, "/strip/c.rs");
        settle(&mut vcx);

        vcx.simulate_keystrokes("secondary-alt-t");
        vcx.simulate_keystrokes("left");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/b.rs"));
        assert!(strip_has_keys(&app, &mut vcx));

        // The × the strip offers on the cell.
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_close_file(1, window, cx);
        });
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            let code = app.tab_code().unwrap();
            assert!(!code.files.contains(&b), "the closed document is gone");
            assert_eq!(
                app.editor.read(cx).active_file(code).unwrap().path,
                PathBuf::from("/strip/c.rs"),
                "the neighbour in front took over"
            );
        });

        // The remembered cell went with it; the strip lands on a live cell and
        // still navigates and activates.
        vcx.simulate_keystrokes("secondary-alt-t");
        assert!(strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("home");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/a.rs"));
        vcx.simulate_keystrokes("right");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/c.rs"));
    }

    #[gpui_kit::test]
    fn unsaved_work_survives_strip_navigation_and_still_asks_before_closing(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/strip/clean.rs");
        let edited = open_rs(&app, &mut vcx, "/strip/edited.rs");
        settle(&mut vcx);
        vcx.update(|window, cx| window.input("edited", cx));
        settle(&mut vcx);
        assert!(dirty(&app, &mut vcx, edited));

        // The strip brings the clean document forward without touching the
        // unsaved one.
        vcx.simulate_keystrokes("secondary-alt-t");
        vcx.simulate_keystrokes("home");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/clean.rs"));
        assert!(strip_has_keys(&app, &mut vcx));
        assert!(
            dirty(&app, &mut vcx, edited),
            "the unsaved edits are still there"
        );

        // Closing it still asks first, and the document stays open.
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_close_file(1, window, cx);
        });
        settle(&mut vcx);
        assert!(
            vcx.has_pending_prompt(),
            "the unsaved-change question is still asked"
        );
        app.read_with(&vcx, |app, _| {
            assert!(
                app.tab_code().unwrap().files.contains(&edited),
                "a cancelled close keeps the document"
            );
        });
        vcx.simulate_prompt_answer(crate::ui::i18n::t(L10nKey::Cancel));
        settle(&mut vcx);
        assert!(dirty(&app, &mut vcx, edited));
    }

    #[gpui_kit::test]
    fn the_document_in_front_still_saves_with_the_strip_holding_the_keyboard(
        cx: &mut TestAppContext,
    ) {
        let dir = test_host::canonical_tempdir();
        let host = test_host::FileHost::new(HostId(737));
        let path = dir.path().join("saved.rs");
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        app.update_in(&mut vcx, |app, window, cx| {
            let tab = app.tabs[0].id();
            let id = app.editor.update(cx, |editor, cx| {
                editor.new_buffer(
                    host.clone(),
                    path.clone(),
                    None,
                    BufferKind::Text,
                    "fn main() {}\n".into(),
                    TextFormat::default(),
                    EditorConfig::default(),
                    lyrel_document::text::detect_indent("", "rust"),
                    None,
                    window,
                    cx,
                )
            });
            app.editor_show_in_tab(tab, id, true, window, cx);
        });
        settle(&mut vcx);
        vcx.update(|window, cx| window.input("// edit", cx));
        settle(&mut vcx);
        assert!(app.read_with(&vcx, |app, cx| {
            let code = app.tab_code().unwrap();
            app.editor.read(cx).active_file(code).unwrap().dirty
        }));

        // The strip has the keyboard, and the document commands still reach
        // the document in front.
        vcx.simulate_keystrokes("secondary-alt-t");
        assert!(strip_has_keys(&app, &mut vcx));
        vcx.update(|window, cx| window.press("secondary-s", cx));
        wait_until(&mut vcx, |vcx| {
            app.read_with(vcx, |app, cx| {
                let code = app.tab_code().unwrap();
                !app.editor.read(cx).active_file(code).unwrap().dirty
            })
        });
        assert_eq!(host.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
