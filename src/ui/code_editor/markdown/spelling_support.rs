use super::SourcePresentation;
use crate::ui::code_editor::markdown::projection::Kind;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, VisualTestContext};
pub(super) fn settle_geometry(view: &Entity<SourcePresentation>, context: &mut VisualTestContext) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        context.update(|window, cx| window.render_frame(cx));
        if view.read_with(context, |view, cx| {
            view.live
                && view.geometry_is_current(cx)
                && !view
                    .projection
                    .leaves
                    .iter()
                    .any(|leaf| matches!(leaf.kind, Kind::PendingSource))
        }) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "spelling geometry did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

macro_rules! spelling_fixture {
    ($cx:expr, $original:expr) => {{
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("waves.md");
        std::fs::write(&path, $original).unwrap();
        let (app, mut context, streams) = harness_with_tabs($cx, 1);
        app.update_in(&mut context, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let view = loop {
            context.run_until_parked();
            if let Some(view) = app.read_with(&context, |app, _| {
                app.active_editor_input()
                    .and_then(|input| input.markdown().cloned())
            }) {
                break view;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        view.update_in(&mut context, |view, window, cx| {
            if !view.live {
                view.toggle_mode(window, cx);
            }
            view.set_soft_wrap(true, cx);
            let end = view.document.read(cx).source().len();
            view.select_range(end..end, cx);
            view.focus.focus(window, cx);
            view.spelling.invalidate();
            view.set_spelling_diagnostics(
                vec![Diagnostic {
                    ranges: vec![0..end],
                    word: "deterministic OS diagnostic".into(),
                }],
                cx,
            );
        });
        (directory, path, app, context, streams, view)
    }};
}
pub(super) use spelling_fixture;
