use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::{ElementInputHandler, TestAppContext, VisualContext as _, test::{TestEventExt as _, TestInput, TestWindowExt as _}};

#[gpui_kit::test]
fn formula_structural_fraction_cancel_commit_and_shared_document_undo(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("structural-formula.md");
    let source = "前 $\\frac{a}{b}+\\mathrm{keep}$ 后\r\n\r\n$$\nx_i^2\n$$\n";
    std::fs::write(&path, source).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| app.open_file_in_editor(&path, window, cx));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let view = loop {
        context.run_until_parked();
        if let Some(view) = app.read_with(&context, |app, _| app.active_editor_input().and_then(|input| input.markdown().cloned())) { break view; }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    view.update_in(&mut context, |view, window, cx| { if !view.is_live() { view.toggle_mode(window, cx); } });
    let ready = "markdown-math-ready-\\frac{a}{b}+\\mathrm{keep}";
    loop {
        context.run_until_parked();
        context.update(|window, cx| window.render_frame(cx));
        if context.update(|window, _| window.try_find(ready).is_some()) { break; }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    context.update(|window, cx| {
        window.click(ready, cx);
        window.press(if cfg!(target_os = "macos") { "cmd-home" } else { "ctrl-home" }, cx);
        window.press("right", cx); // Numerator, not a source byte inside \\frac.
        let bounds = window.find("markdown-source").bounds();
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        input.compose_text(None, "ni", Some(2..2), cx);
        assert_eq!(input.marked_text_range(cx), Some(0..2));
        input.compose_text(None, "", None, cx);
        assert!(input.marked_text_range(cx).is_none());
        window.input("q", cx);
        assert_eq!(view.read(cx).document().read(cx).encode().unwrap(), source.as_bytes());
        window.press("escape", cx);
    });
    view.read_with(&context, |view, cx| assert_eq!(view.document().read(cx).encode().unwrap(), source.as_bytes()));
    context.run_until_parked();
    context.update(|window, cx| { window.render_frame(cx); window.click(ready, cx); window.press(if cfg!(target_os = "macos") { "cmd-home" } else { "ctrl-home" }, cx); window.press("right", cx); window.input("q", cx); window.press("enter", cx); });
    let expected = source.replacen("{a}", "{qa}", 1);
    view.read_with(&context, |view, cx| assert_eq!(view.document().read(cx).encode().unwrap(), expected.as_bytes()));
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| assert_eq!(view.document().read(cx).encode().unwrap(), source.as_bytes()));
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| assert_eq!(view.document().read(cx).encode().unwrap(), expected.as_bytes()));
}

#[test]
fn formula_lossless_wrappers_scripts_and_matrix_keep_untouched_spelling() {
    use super::lossless::Seed;
    use ratex_gpui::editor::{cursor::{Cursor, Slot, Step}, model::Atom};
    let raw = r"\frac { a }{b}+\mathrm{keep}+x^2_i+\begin{bmatrix} p & q \\ r & s \end{bmatrix}";
    let seed = Seed::parse(raw);
    assert_eq!(seed.serialize(&seed.row), raw);
    let empty = Seed::parse(r"\frac{}{b}");
    assert_eq!(empty.serialize(&empty.row), r"\frac{}{b}");
    assert_eq!(empty.preview(&empty.row), r"\frac{\square}{b}");
    let mut row = seed.row.clone();
    let mut cursor = Cursor { path: vec![Step { atom: 0, slot: Slot::Num }], index: 0 };
    cursor.insert(&mut row, Atom::Sym("z".into()));
    assert_eq!(seed.serialize(&row), raw.replacen(" a ", " za ", 1));
    let mut cursor = Cursor { path: vec![Step { atom: row.atoms.len() - 1, slot: Slot::Cell(0, 0) }], index: 0 };
    cursor.matrix_add_row(&mut row);
    let changed = seed.serialize(&row);
    assert!(changed.contains(r"\begin{bmatrix}"));
    assert!(changed.ends_with(r"\end{bmatrix}"));
    assert!(changed.contains(r"\mathrm{keep}+x^2_i"));
}
