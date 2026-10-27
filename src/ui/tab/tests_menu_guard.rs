use super::*;
use gpui_kit::TestAppContext;
use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};

#[gpui_kit::test]
fn close_confirmation_keeps_its_original_pane_when_focus_changes(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 2);
    let (left, right) = app.update_in(&mut vcx, |app, window, cx| {
        cx.global_mut::<Config>().confirm_close = crate::core::config::ConfirmClose::Always;
        let left = app.tabs[0].pane.first_leaf().unwrap();
        let right = app.tabs.remove(1).pane.first_leaf().unwrap();
        app.tabs[0]
            .pane
            .split_leaf(left.entity_id(), Axis::Horizontal, false, right.clone());
        window.focus(&right.focus_handle(cx), cx);
        cx.notify();
        (left, right)
    });
    vcx.dispatch_action(CloseActiveTab);
    vcx.run_until_parked();
    assert!(vcx.has_pending_prompt());
    app.update_in(&mut vcx, |_, window, cx| {
        window.focus(&left.focus_handle(cx), cx)
    });
    vcx.simulate_prompt_answer(t(L10nKey::Close));
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, _, _| {
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].pane.leaves().len(), 1);
        assert_eq!(
            app.tabs[0].pane.first_leaf().unwrap().entity_id(),
            left.entity_id()
        );
        assert_ne!(left.entity_id(), right.entity_id());
    });
}

#[gpui_kit::test]
fn pane_handle_remains_usable_after_interface_zoom(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    let pane = app.update_in(&mut vcx, |app, _, _| app.tabs[0].pane.first_leaf().unwrap());
    for size in [14., 20.] {
        app.update_in(&mut vcx, |_, _, cx| {
            cx.global_mut::<Config>().ui_font_size = size;
            gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(size));
            cx.notify();
        });
        vcx.update(|window, cx| {
            window.render_frame(cx);
            let id = ("pane-drag-handle", pane.entity_id().as_u64() as usize);
            window.pointer_move(window.find(id).bounds().center(), None, cx);
            assert!(window.find(id).visible());
            window.click(("pane-drag-handle", pane.entity_id().as_u64() as usize), cx);
            window.press("escape", cx);
            assert!(pane.contains_focused(window, cx));
        });
    }
}
