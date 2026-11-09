use super::SourcePresentation;
use crate::ui::code_editor::markdown::projection::Kind;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, Focusable as _, VisualTestContext};
use std::time::{Duration, Instant};
#[track_caller]
pub(super) fn until(
    context: &mut VisualTestContext,
    mut ready: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        context.executor().advance_clock(Duration::from_millis(100));
        context.run_until_parked();
        context.update(|window, cx| window.render_frame(cx));
        if ready(context) {
            return;
        }
        assert!(Instant::now() < deadline, "consumer did not settle");
        std::thread::sleep(Duration::from_millis(5));
    }
}
pub(super) fn settle(view: &Entity<SourcePresentation>, context: &mut VisualTestContext) {
    until(context, |context| {
        view.read_with(context, |view, cx| {
            view.is_live()
                && view.projection.revision == view.document().read(cx).revision()
                && !view
                    .projection
                    .leaves
                    .iter()
                    .any(|leaf| matches!(leaf.kind, Kind::PendingSource))
                && view.geometry_is_current(cx)
        })
    });
}
pub(super) fn bytes(view: &Entity<SourcePresentation>, context: &VisualTestContext) -> Vec<u8> {
    view.read_with(context, |view, cx| {
        view.document().read(cx).encode().unwrap()
    })
}
pub(super) fn select(
    view: &Entity<SourcePresentation>,
    context: &mut VisualTestContext,
    start: usize,
    end: usize,
) {
    view.update_in(context, |view, window, cx| {
        view.select_range(start..end, cx);
        view.focus_handle(cx).focus(window, cx);
    });
}
macro_rules! fixture {
    ($cx:expr, $source:expr) => {
        fixture!($cx, $source, false)
    };
    ($cx:expr, $source:expr, $local:expr) => {
        fixture!($cx, $source, $local, "review.md")
    };
    ($cx:expr, $source:expr, $local:expr, $name:expr) => {{
        let directory = crate::ui::code_editor::test_host::canonical_tempdir();
        let path = directory.path().join($name);
        std::fs::write(&path, $source).unwrap();
        let (app, mut context, streams) = harness_with_tabs($cx, 1);
        let host = crate::ui::code_editor::test_host::FileHost::new(lyrel_core::host::HostId(2041));
        app.update_in(&mut context, |app, window, cx| {
            if $local {
                app.open_file_in_editor(&path, window, cx);
            } else {
                app.editor_open_on_host(host, &path, window, cx);
            }
        });
        let mut opened = None;
        until(&mut context, |context| {
            opened = app.read_with(context, |app, _| {
                app.active_editor_input()
                    .and_then(|input| input.markdown().cloned())
            });
            opened.is_some()
        });
        let view = opened.unwrap();
        view.update_in(&mut context, |view, window, cx| {
            if !view.is_live() {
                view.toggle_mode(window, cx);
            }
            view.focus_handle(cx).focus(window, cx);
        });
        settle(&view, &mut context);
        (directory, path, app, context, streams, view)
    }};
}
pub(super) use fixture;
