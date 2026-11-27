//! #441 的工作台正确性回放。实窗显示延迟、进程内存和系统 IME 由产品回放采集。

#[path = "../../../../scripts/bench/document_history_441.rs"]
mod fixture;

use super::super::{Entity, LyrelApp, px};
use super::foundation::{save, settle, until};
use super::test_support::{active_editor_input, with_active_file};
use crate::ui::app::test_window::harness_with_tabs;
use fixture::{Fixture, IME_COMMIT, INPUT, STEPS};
use gpui_kit::component::input::{Redo, Undo};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{EntityInputHandler as _, Focusable as _, TestAppContext, VisualTestContext};
use lyrel_markdown_editor::EditorState;
use std::ops::Range;
use std::path::Path;

type View = Entity<EditorState>;

fn open(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, path: &Path) -> View {
    app.update_in(cx, |app, window, cx| {
        app.open_file_in_editor(path, window, cx)
    });
    until(cx, |cx| {
        app.read_with(cx, |app, cx| with_active_file(app, cx, |_, _| ()).is_some())
    });
    let view = app.read_with(cx, |app, cx| {
        active_editor_input(app, cx)
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    view.update_in(cx, |view, window, cx| {
        assert!(view.is_live());
        view.focus_handle(cx).focus(window, cx);
    });
    settle(cx);
    view
}

fn close(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, view: View) {
    let document = view.read_with(cx, |view, _| view.document().downgrade());
    app.update_in(cx, |app, window, cx| app.editor_close_file(0, window, cx));
    drop(view);
    until(cx, |cx| {
        document.upgrade().is_none()
            && app.read_with(cx, |app, cx| with_active_file(app, cx, |_, _| ()).is_none())
    });
}

fn save_bytes(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, path: &Path, expected: &[u8]) {
    save(app, cx);
    let actual = std::fs::read(path).unwrap();
    assert_eq!(actual.len(), expected.len(), "{}", path.display());
    let mismatch = actual
        .iter()
        .zip(expected)
        .position(|(got, want)| got != want);
    assert_eq!(mismatch, None, "first differing byte in {}", path.display());
}

fn check(
    view: &View,
    cx: &mut VisualTestContext,
    selection: Range<usize>,
    revision: u64,
    at: usize,
    expected: &str,
) {
    view.read_with(cx, |view, cx| {
        assert_eq!(view.document().read(cx).revision(), revision);
        assert_eq!(view.cursor(), selection.end);
        assert_eq!(view.selected_range(), selection);
        assert_eq!(view.text().get(at..at + expected.len()), Some(expected));
    });
}

fn replacements(
    app: &Entity<LyrelApp>,
    cx: &mut VisualTestContext,
    path: &Path,
    fixture: &Fixture,
) {
    let view = open(app, cx, path);
    let mut revision = view.read_with(cx, |view, cx| {
        assert_eq!(
            view.document().read(cx).format().encoding_label(),
            "UTF-8 BOM"
        );
        view.document().read(cx).revision()
    });
    save_bytes(app, cx, path, &fixture.original);

    for site in fixture.sites {
        let at = site.source;
        view.update_in(cx, |view, _, cx| view.select_range(at..at + 1, cx));
        cx.update(|window, cx| window.input(INPUT, cx));
        revision += 1;
        check(&view, cx, at + 1..at + 1, revision, at, INPUT);
    }
    let replaced = fixture.replacements(STEPS);
    let halfway = fixture.replacements(STEPS / 2);
    save_bytes(app, cx, path, &replaced);

    for (step, site) in fixture.sites.iter().enumerate().rev() {
        let at = site.source;
        cx.dispatch_action(Undo);
        revision += 1;
        check(&view, cx, at..at + 1, revision, at, "a");
        if step == STEPS / 2 {
            save_bytes(app, cx, path, &halfway);
        }
    }
    let first = fixture.sites[0].source;
    cx.dispatch_action(Undo);
    check(&view, cx, first..first + 1, revision, first, "a");
    save_bytes(app, cx, path, &fixture.original);

    for (step, site) in fixture.sites.iter().enumerate() {
        let at = site.source;
        cx.dispatch_action(Redo);
        revision += 1;
        check(&view, cx, at + 1..at + 1, revision, at, INPUT);
        if step + 1 == STEPS / 2 {
            save_bytes(app, cx, path, &halfway);
        }
    }
    let last = fixture.sites[STEPS - 1].source;
    cx.dispatch_action(Redo);
    check(&view, cx, last + 1..last + 1, revision, last, INPUT);
    save_bytes(app, cx, path, &replaced);

    // 按平台输入处理器合同发送 UTF-16 选区及 marked text，覆盖非 BMP 字符。
    let ime_start = 1;
    let before_caret = ime_start + "🎉".len();
    let after_caret = ime_start + IME_COMMIT.len();
    view.update_in(cx, |view, window, cx| {
        view.select_range(ime_start..before_caret, cx);
        view.replace_and_mark_text_in_range(None, "中😀", Some(1..3), window, cx);
        assert_eq!(view.marked_text_range(window, cx), Some(1..4));
        let selection = view.selected_text_range(false, window, cx).unwrap();
        assert_eq!(selection.range, 2..4);
        assert_eq!(view.selected_range(), 4..8);
        view.replace_text_in_range(None, IME_COMMIT, window, cx);
        assert_eq!(view.marked_text_range(window, cx), None);
    });
    revision += 2;
    check(
        &view,
        cx,
        after_caret..after_caret,
        revision,
        ime_start,
        IME_COMMIT,
    );
    let committed = fixture.ime();
    save_bytes(app, cx, path, &committed);

    cx.dispatch_action(Undo);
    revision += 1;
    check(
        &view,
        cx,
        before_caret..before_caret,
        revision,
        ime_start,
        "🎉",
    );
    save_bytes(app, cx, path, &replaced);
    view.update_in(cx, |view, window, cx| {
        view.select_range(ime_start..before_caret, cx);
        view.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
        view.replace_and_mark_text_in_range(None, "", Some(0..0), window, cx);
        assert_eq!(view.marked_text_range(window, cx), None);
        assert!(!view.document().read(cx).has_composition());
    });
    revision += 2;
    check(&view, cx, ime_start..ime_start, revision, ime_start, "🎉");
    save_bytes(app, cx, path, &replaced);
    // 取消预编辑后，仍可重做之前提交的事务。
    cx.dispatch_action(Redo);
    revision += 1;
    check(
        &view,
        cx,
        after_caret..after_caret,
        revision,
        ime_start,
        IME_COMMIT,
    );
    cx.dispatch_action(Undo);
    revision += 1;
    check(
        &view,
        cx,
        before_caret..before_caret,
        revision,
        ime_start,
        "🎉",
    );
    cx.dispatch_action(Undo);
    revision += 1;
    check(&view, cx, last..last + 1, revision, last, "a");
    cx.dispatch_action(Redo);
    cx.dispatch_action(Redo);
    save_bytes(app, cx, path, &committed);
    close(app, cx, view);

    let reopened = open(app, cx, path);
    let revision = reopened.read_with(cx, |view, cx| view.document().read(cx).revision());
    cx.dispatch_action(Undo);
    reopened.read_with(cx, |view, cx| {
        assert_eq!(view.document().read(cx).revision(), revision)
    });
    save_bytes(app, cx, path, &committed);
    close(app, cx, reopened);
}

fn typing(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, path: &Path, fixture: &Fixture) {
    let view = open(app, cx, path);
    let revision = view.read_with(cx, |view, cx| view.document().read(cx).revision());
    let at = fixture.sites[0].source;
    view.update_in(cx, |view, _, cx| view.select_range(at..at, cx));
    for count in 1..=STEPS {
        cx.update(|window, cx| window.input(INPUT, cx));
        let caret = at + count;
        check(
            &view,
            cx,
            caret..caret,
            revision + count as u64,
            caret - 1,
            INPUT,
        );
    }
    let typed = fixture.typing();
    save_bytes(app, cx, path, &typed);

    let revision = revision + STEPS as u64;
    cx.dispatch_action(Undo);
    check(&view, cx, at..at, revision + 1, at, "a");
    cx.dispatch_action(Undo);
    check(&view, cx, at..at, revision + 1, at, "a");
    save_bytes(app, cx, path, &fixture.original);
    cx.dispatch_action(Redo);
    let caret = at + STEPS;
    check(&view, cx, caret..caret, revision + 2, caret, "a");
    cx.dispatch_action(Redo);
    check(&view, cx, caret..caret, revision + 2, caret, "a");
    save_bytes(app, cx, path, &typed);
    close(app, cx, view);
}

pub(super) fn document_history_441(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let fixture = Fixture::new();
    assert_eq!(fixture.original.len(), 1024 * 1024);
    let replace_path = directory.path().join("replace.md");
    let typing_path = directory.path().join("typing.md");
    for path in [&replace_path, &typing_path] {
        std::fs::write(path, &fixture.original).unwrap();
    }
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.simulate_resize(gpui_kit::size(px(1600.), px(1000.)));
    replacements(&app, &mut context, &replace_path, &fixture);
    typing(&app, &mut context, &typing_path, &fixture);
}
