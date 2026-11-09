use super::SourcePresentation;
use gpui_kit::test::TestWindowExt as _;
pub(super) fn start_in_source(
    view: &gpui_kit::Entity<SourcePresentation>,
    cx: &mut gpui_kit::VisualTestContext,
) {
    view.update_in(cx, |view, window, cx| {
        if view.is_live() {
            view.toggle_mode(window, cx);
        }
        assert!(
            !view.is_live(),
            "this scenario starts in source before exercising live mode"
        );
    });
}
pub(super) fn settle_live_geometry(
    view: &gpui_kit::Entity<SourcePresentation>,
    cx: &mut gpui_kit::VisualTestContext,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(100));
        cx.run_until_parked();
        cx.update(|window, cx| window.render_frame(cx));
        if view.read_with(cx, |view, cx| {
            view.is_live() && view.range_to_bounds(&(0..0), cx).is_some()
        }) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "live document geometry did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
