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

fn registered_tabs(
    cx: &mut TestAppContext,
) -> (
    gpui_kit::Entity<LyrelApp>,
    VisualTestContext,
    Vec<crate::daemon::transport::Stream>,
) {
    let (app, mut vcx, streams) = crate::ui::app::test_window::harness_with_tabs(cx, 2);
    app.update_in(&mut vcx, |app, window, cx| {
        WindowRegistry::init(cx);
        let weak = cx.entity().downgrade();
        WindowRegistry::register(cx, app.workspace, window.window_handle(), weak);
        app.focus_active(window, cx);
    });
    vcx.run_until_parked();
    (app, vcx, streams)
}

#[gpui_kit::test]
fn close_window_shortcut_keeps_the_native_tab_close_distinct(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = registered_tabs(cx);
    let kept = app.read_with(&vcx, |app, _| app.tabs[1].id());

    let tab_chord = if cfg!(target_os = "macos") {
        "secondary-w"
    } else {
        "secondary-shift-w"
    };
    vcx.simulate_keystrokes(tab_chord);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, _| {
        assert_eq!(
            app.tabs.len(),
            1,
            "the native tab chord closes only the active tab"
        );
        assert_eq!(app.tabs[0].id(), kept);
    });
    assert_eq!(vcx.update(|_, cx| WindowRegistry::count(cx)), 1);

    #[cfg(target_os = "macos")]
    {
        vcx.simulate_keystrokes("secondary-shift-w");
        drop(vcx);
        cx.run_until_parked();
        assert!(cx.update(|cx| cx.windows().is_empty()));
        assert!(cx.update(|cx| WindowRegistry::open_windows(cx).is_empty()));
    }
}

#[gpui_kit::test]
fn close_window_override_keeps_unsaved_confirmation_and_cancellation(cx: &mut TestAppContext) {
    use crate::core::actions::EditorNewFile;
    use crate::core::config::KeybindingOverride;
    use crate::ui::i18n::{L10nKey, t};
    use gpui_kit::test::TestWindowExt as _;

    let (app, mut vcx, _streams) = registered_tabs(cx);
    vcx.update(|_, cx| {
        cx.global_mut::<Config>().keybindings.insert(
            "CloseWindow".to_string(),
            KeybindingOverride::Exact(vec!["secondary-alt-f9".to_string()]),
        );
        crate::ui::keymap::rebind(cx);
    });
    vcx.dispatch_action(EditorNewFile);
    vcx.run_until_parked();
    vcx.update(|window, cx| window.input("keep these edits", cx));
    vcx.run_until_parked();
    let edited = app.read_with(&vcx, |app, _| {
        app.tab_code()
            .and_then(|code| code.active_id())
            .expect("the edited document is open")
    });

    #[cfg(target_os = "macos")]
    {
        vcx.simulate_keystrokes("secondary-shift-w");
        vcx.run_until_parked();
        assert!(
            !vcx.has_pending_prompt(),
            "the override retires the default key"
        );
        assert_eq!(vcx.update(|_, cx| WindowRegistry::count(cx)), 1);
    }

    vcx.simulate_keystrokes("secondary-alt-f9");
    vcx.run_until_parked();
    assert!(vcx.has_pending_prompt());
    vcx.simulate_prompt_answer(t(L10nKey::Cancel));
    vcx.run_until_parked();
    assert_eq!(vcx.update(|_, cx| WindowRegistry::count(cx)), 1);
    app.read_with(&vcx, |app, _| {
        assert_eq!(
            app.tab_code().and_then(|code| code.active_id()),
            Some(edited),
            "Cancel keeps the same document open"
        );
        assert_eq!(app.editor_unsaved(), vec![edited]);
        assert_eq!(app.tabs.len(), 2, "closing a window must not close a tab");
    });
    vcx.update(|window, cx| {
        window.press("secondary-a", cx);
        window.press("secondary-c", cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()).as_deref(),
            Some("keep these edits"),
            "Cancel preserves the document text and restores its input focus"
        );
    });

    vcx.simulate_keystrokes("secondary-alt-f9");
    vcx.run_until_parked();
    assert!(vcx.has_pending_prompt());
    vcx.simulate_prompt_answer(t(L10nKey::EditorDiscard));
    drop(vcx);
    cx.run_until_parked();
    assert!(cx.update(|cx| cx.windows().is_empty()));
    assert!(cx.update(|cx| WindowRegistry::open_windows(cx).is_empty()));
}
