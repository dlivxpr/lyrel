// Retained whole-window tests for the `markdown_search` module, which now lives in
// `lyrel-workbench-editor`. Keeping this anchor at the original module path
// preserves every existing test selector.

pub(crate) use lyrel_workbench_editor::code_editor::markdown_search::*;

// Inherited by the retained tests from the production module's scope before
// the move; kept here so their `use super::*` resolves unchanged.
#[cfg(any(test, feature = "native-markdown-tests"))]
use crate::ui::app::LyrelApp;
#[cfg(any(test, feature = "native-markdown-tests"))]
use gpui_kit::component::input::Position;
#[cfg(any(test, feature = "native-markdown-tests"))]
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, Entity, EntityInputHandler as _, Focusable as _, SharedString, Window, px,
};
#[cfg(any(test, feature = "native-markdown-tests"))]
use lyrel_ui_base::host_ops::{HostId, HostOps, SharedHost};
#[cfg(any(test, feature = "native-markdown-tests"))]
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};
#[cfg(any(test, feature = "native-markdown-tests"))]
use lyrel_workbench_editor::code_editor::*;
#[cfg(any(test, feature = "native-markdown-tests"))]
use std::path::{Path, PathBuf};
#[cfg(any(test, feature = "native-markdown-tests"))]
use std::sync::Arc;
#[cfg(any(test, feature = "native-markdown-tests"))]
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, VisualTestContext};

    fn settle(vcx: &mut VisualTestContext) {
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(100));
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
        vcx.run_until_parked();
    }

    #[gpui_kit::test]
    fn live_search_uses_upstream_exact_source_spelling_and_undo(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("visible-find.md");
        let original = "alpha **beta** gamma\n";
        std::fs::write(&path, original).unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.read_with(&vcx, |app, cx| app.active_buffer(cx).is_some()) {
            settle(&mut vcx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if !app.read_with(&vcx, |app, cx| {
            app.active_editor_input(cx)
                .unwrap()
                .markdown()
                .unwrap()
                .read(cx)
                .is_live()
        }) {
            vcx.dispatch_action(crate::core::actions::ToggleDocumentPreview);
        }
        settle(&mut vcx);
        vcx.dispatch_action(gpui_kit::component::input::Replace);
        settle(&mut vcx);
        vcx.update(|window, _| {
            assert_eq!(window.find("markdown-query").focused(), Some(true));
        });
        vcx.update(|window, cx| window.input("alpha **beta**", cx));
        settle(&mut vcx);
        while !vcx.update(|window, _| window.find("markdown-match-count").label() == Some("1/1")) {
            settle(&mut vcx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        vcx.update(|window, cx| {
            assert_eq!(window.find("markdown-match-count").label(), Some("1/1"));
            window.click("markdown-replacement", cx);
            window.input("new", cx);
            window.click("markdown-replace-one", cx);
        });
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer(cx)
                    .unwrap()
                    .input
                    .text()
                    .unwrap()
                    .read(cx)
                    .text()
                    .to_string(),
                "new gamma\n"
            )
        });
        vcx.update(|window, cx| window.press("escape", cx));
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer(cx)
                    .unwrap()
                    .input
                    .text()
                    .unwrap()
                    .read(cx)
                    .text()
                    .to_string(),
                original
            )
        });
        vcx.dispatch_action(gpui_kit::component::input::Search);
        vcx.update(|window, cx| window.input("**", cx));
        settle(&mut vcx);
        vcx.update(|window, _| {
            assert_eq!(window.find("markdown-match-count").label(), Some("2/2"))
        });
    }

    #[gpui_kit::test]
    fn markdown_find_replace_all_is_undoable_and_escape_restores_source_focus(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("find.md");
        std::fs::write(&path, "中🎉 alpha\nAlpha omega alpha").unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(gpui_kit::size(px(1600.), px(900.)));
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.read_with(&vcx, |app, cx| app.active_buffer(cx).is_some()) {
            settle(&mut vcx);
            assert!(
                std::time::Instant::now() < deadline,
                "Markdown open did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.update_in(&mut vcx, |app, window, cx| {
            app.toggle_document_preview(window, cx)
        });
        vcx.dispatch_action(gpui_kit::component::input::Replace);
        settle(&mut vcx);
        vcx.update(|window, cx| window.input("alpha", cx));
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_editor_input(cx)
                    .unwrap()
                    .text()
                    .unwrap()
                    .read(cx)
                    .selected_range(),
                8..13
            )
        });
        vcx.update(|window, cx| {
            assert_eq!(window.find("markdown-match-count").label(), Some("1/3"));
            window.dispatch_action(Box::new(crate::core::actions::FindNext), cx);
        });
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_editor_input(cx)
                    .unwrap()
                    .text()
                    .unwrap()
                    .read(cx)
                    .selected_range(),
                14..19
            )
        });
        vcx.update(|window, cx| {
            window.dispatch_action(Box::new(crate::core::actions::FindPrevious), cx);
            window.click("markdown-match-case", cx);
        });
        settle(&mut vcx);
        vcx.update(|window, cx| {
            assert_eq!(window.find("markdown-match-count").label(), Some("1/2"));
            window.click("markdown-match-case", cx);
        });
        settle(&mut vcx);
        vcx.update(|window, cx| {
            window.click("markdown-replacement", cx);
            window.input("新", cx);
            window.click("markdown-replace-one", cx);
        });
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer(cx)
                    .unwrap()
                    .input
                    .text()
                    .unwrap()
                    .read(cx)
                    .text()
                    .to_string(),
                "中🎉 新\nAlpha omega alpha"
            )
        });
        vcx.update(|window, cx| window.press("escape", cx));
        settle(&mut vcx);
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer(cx)
                    .unwrap()
                    .input
                    .text()
                    .unwrap()
                    .read(cx)
                    .text()
                    .to_string(),
                "中🎉 alpha\nAlpha omega alpha"
            )
        });
        vcx.dispatch_action(gpui_kit::component::input::Replace);
        settle(&mut vcx);
        vcx.update(|window, cx| {
            window.click("markdown-replace-all", cx);
        });
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer(cx)
                    .unwrap()
                    .input
                    .text()
                    .unwrap()
                    .read(cx)
                    .text()
                    .to_string(),
                "中🎉 新\n新 omega 新"
            );
            assert!(app.active_buffer(cx).unwrap().dirty);
        });
        vcx.update(|window, cx| window.press("escape", cx));
        settle(&mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            assert!(app.editor_has_focus(window, cx))
        });
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer(cx)
                    .unwrap()
                    .input
                    .text()
                    .unwrap()
                    .read(cx)
                    .text()
                    .to_string(),
                "中🎉 alpha\nAlpha omega alpha"
            );
            assert!(!app.active_buffer(cx).unwrap().dirty);
        });
    }

    #[gpui_kit::test]
    fn ctrl_f_opens_the_find_bar_in_a_live_markdown_document(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys.md");
        std::fs::write(&path, "alpha beta\n").unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.read_with(&vcx, |app, cx| app.active_buffer(cx).is_some()) {
            settle(&mut vcx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.update_in(&mut vcx, |app, window, cx| {
            let input = app.active_editor_input(cx).expect("markdown presentation");
            input.focus(window, cx);
        });
        settle(&mut vcx);
        vcx.simulate_keystrokes("secondary-f");
        settle(&mut vcx);
        vcx.update(|window, _| {
            assert_eq!(
                window.find("markdown-query").focused(),
                Some(true),
                "Ctrl/Cmd+F must open the Markdown find bar, not do nothing"
            );
        });
    }

    #[gpui_kit::test]
    fn unwrapped_long_lines_gain_a_horizontal_scroll_extent(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.md");
        let long = format!("start {} end\n", "wide ".repeat(600));
        std::fs::write(&path, &long).unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(gpui_kit::size(px(900.), px(700.)));
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            settle(&mut vcx);
            let ready = app.read_with(&vcx, |app, cx| {
                app.active_editor_input(cx)
                    .and_then(|input| input.markdown().cloned())
                    .is_some_and(|view| view.read(cx).scroll_handle().max_offset().x > px(0.))
            });
            if ready {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "nowrap Markdown must expose a horizontal scroll extent"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
