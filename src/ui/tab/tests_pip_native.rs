use super::*;
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;

#[gpui_kit::test]
fn native_ssh_on_the_local_workspace_uses_the_same_live_view(cx: &mut TestAppContext) {
    let (app, mut vcx) = test_window::harness(cx);
    let (pane, _stream) = app.update_in(&mut vcx, |app, window, cx| {
        let (pane, stream) = crate::terminal::view::quiet_test_ssh_pane(41, window, cx);
        app.tabs
            .push(Tab::new(Pane::Leaf(PaneSlot::Ready(pane.clone()))));
        app.tabs.select(0);
        let focus = pane.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        cx.notify();
        (pane, stream)
    });
    vcx.dispatch_action(TogglePanePip);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find(("pane-pip", pane.entity_id().as_u64() as usize))
                .visible()
        );
    });
    vcx.dispatch_action(RestorePanePip);
    app.update_in(&mut vcx, |app, _, _| {
        assert_eq!(
            app.tabs[0].pane.first_leaf().unwrap().entity_id(),
            pane.entity_id()
        );
    });
}
