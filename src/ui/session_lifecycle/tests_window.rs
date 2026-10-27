use crate::core::actions::CloseWindow;
use crate::core::config::Config;
use crate::core::session::Session;
use crate::ui::app::LyrelApp;
use crate::ui::windows::WindowRegistry;
use gpui_kit::{AppContext as _, TestAppContext, VisualTestContext};

/// `CloseWindow` has to close the window, and close it the way the red
/// button does.
///
/// The action is otherwise all table entries — the `actions!` row, the
/// keymap slot, the palette command, the Keybindings label — and every one
/// of those can be in place while the action reaches nothing at all. So
/// this drives the real dispatch path and then asks two separate
/// questions: the window is gone from gpui, *and* it left the
/// `WindowRegistry` on the way out. The second is what makes it the same
/// close as the native one — `detach_workspace` is where the session is
/// saved and the workspace is retired, and a `remove_window` that skipped
/// it would still pass the first assertion while quietly dropping a
/// window's tabs on the floor.
#[gpui_kit::test]
fn dispatching_close_window_takes_the_window_down_with_its_registration(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
        crate::ui::keymap::init(cx);
        WindowRegistry::init(cx);
    });
    let window = cx.add_window(|window, cx| {
        let app = cx.new(|cx| LyrelApp::with_session(None, Some(Session::default()), window, cx));
        gpui_kit::component::Root::new(app, window, cx)
    });
    let app = window
        .update(cx, |root, _, _| {
            root.view()
                .clone()
                .downcast::<LyrelApp>()
                .ok()
                .expect("window root wraps a LyrelApp")
        })
        .unwrap();
    // Registered the way an opened window registers itself; the registry
    // is where the close has to show up, so an unregistered window would
    // make the assertion below pass for the wrong reason.
    let handle = window.into();
    let weak = app.downgrade();
    app.update(cx, |app, cx| {
        WindowRegistry::register(cx, app.workspace, handle, weak);
    });

    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.background_executor.run_until_parked();
    assert_eq!(
        vcx.update(|_, cx| WindowRegistry::count(cx)),
        1,
        "the harness starts with exactly the one window"
    );

    vcx.dispatch_action(CloseWindow);
    drop(vcx);

    assert!(
        cx.update(|cx| cx.windows().is_empty()),
        "CloseWindow has to reach `remove_window`; the window is still open"
    );
    assert!(
        cx.update(|cx| WindowRegistry::open_windows(cx).is_empty()),
        "the close has to run the same cleanup the red button runs, \
             which is what takes the window out of the registry"
    );
}
