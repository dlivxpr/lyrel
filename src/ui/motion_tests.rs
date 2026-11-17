use crate::core::config::{Config, MotionPreference};
use crate::ui::app::test_window;
use crate::ui::i18n::{L10nKey, t};
use crate::ui::settings::SettingsSection;
use crate::ui::settings::test_window as settings_window;
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;

fn choose_motion(vcx: &mut gpui_kit::VisualTestContext, index: usize) {
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("motion-preference-trigger", cx);
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click(format!("motion-preference-item-{index}"), cx);
    });
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn motion_preference_round_trips_through_settings_and_reload(cx: &mut TestAppContext) {
    let (settings, mut vcx) = settings_window::harness(cx);
    let dir = crate::core::config::config_dir_path().expect("the harness pins a scratch config");
    settings.update_in(&mut vcx, |settings, window, cx| {
        cx.global_mut::<Config>().font_size = 17.0;
        settings.navigate_settings(
            SettingsSection::Appearance,
            Some(L10nKey::SettingsMotionPreference),
            window,
            cx,
        );
        let search = settings
            .active_settings()
            .unwrap()
            .navigation
            .search()
            .clone();
        search.update(cx, |search, cx| search.set_value("motion", window, cx));
        settings.autoselect_settings_search(cx);
    });
    vcx.run_until_parked();

    choose_motion(&mut vcx, 1);
    vcx.update(|_, cx| {
        assert_eq!(
            cx.global::<Config>().motion_preference,
            MotionPreference::Reduce
        );
        assert!(cx.reduce_motion(), "Reduce reaches the framework flag");
    });
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap()).unwrap();
    assert_eq!(saved["motion_preference"], "reduce");
    assert_eq!(
        saved["font_size"], 17.0,
        "an unrelated preference survives the write"
    );
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("motion-preference-trigger").label(),
            Some(t(L10nKey::SettingsMotionReduce))
        );
    });

    choose_motion(&mut vcx, 2);
    vcx.update(|_, cx| {
        assert_eq!(
            cx.global::<Config>().motion_preference,
            MotionPreference::Allow
        );
        assert!(!cx.reduce_motion(), "Allow releases reduced motion");
    });
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap()).unwrap();
    assert_eq!(saved["motion_preference"], "allow");

    // Save Reduce, then let a different in-memory state stand in for the next
    // launch: reloading the file is the same path a hand edit and a restart
    // take, and it must restore both the policy and unrelated settings.
    choose_motion(&mut vcx, 1);
    vcx.update(|_, cx| {
        let mut cfg = cx.global::<Config>().clone();
        cfg.motion_preference = MotionPreference::Allow;
        cx.set_global(cfg);
        crate::ui::motion::apply(cx);
        assert!(!cx.reduce_motion());
    });
    vcx.update(|_, cx| {
        let mut announced = false;
        crate::apply_reloaded_config(
            cx,
            crate::core::config::Config::load_with_outcome(),
            &mut announced,
        );
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        assert_eq!(
            cx.global::<Config>().motion_preference,
            MotionPreference::Reduce
        );
        assert!(cx.reduce_motion(), "reload restores the saved policy");
        assert_eq!(cx.global::<Config>().font_size, 17.0);
        window.render_frame(cx);
        assert_eq!(
            window.find("motion-preference-trigger").label(),
            Some(t(L10nKey::SettingsMotionReduce))
        );
    });

    choose_motion(&mut vcx, 0);
    vcx.update(|_, cx| {
        assert_eq!(
            cx.global::<Config>().motion_preference,
            MotionPreference::System
        );
    });
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap()).unwrap();
    assert_eq!(saved["motion_preference"], "system");
}

#[gpui_kit::test]
fn window_activation_keeps_an_explicit_override(cx: &mut TestAppContext) {
    let (settings, mut vcx) = settings_window::harness(cx);
    settings.update_in(&mut vcx, |settings, _, cx| {
        settings.set_motion_preference(MotionPreference::Reduce, cx);
    });
    vcx.update(|_, cx| assert!(cx.reduce_motion()));

    vcx.deactivate_window();
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        assert!(
            cx.reduce_motion(),
            "reactivation must not erase an explicit Reduce"
        );
    });

    settings.update_in(&mut vcx, |settings, _, cx| {
        settings.set_motion_preference(MotionPreference::Allow, cx);
    });
    vcx.deactivate_window();
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        assert!(
            !cx.reduce_motion(),
            "reactivation must not erase an explicit Allow"
        );
    });

    settings.update_in(&mut vcx, |settings, _, cx| {
        settings.set_motion_preference(MotionPreference::System, cx);
    });
    vcx.deactivate_window();
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        assert_eq!(
            cx.global::<Config>().motion_preference,
            MotionPreference::System
        );
        // Under the test scheduler the platform is never consulted, so the
        // last value is retained by design. A real Windows/macOS activation
        // re-reads the OS through `apply_system_reduce_motion`.
    });
}

#[gpui_kit::test]
fn one_policy_settles_every_open_workbench_window(cx: &mut TestAppContext) {
    let (_, mut first) = test_window::harness(cx);
    let (_, mut second) = test_window::harness(cx);
    first.update(|_, cx| {
        cx.global_mut::<Config>().motion_preference = MotionPreference::Reduce;
        crate::ui::motion::apply(cx);
    });
    for window in [&mut first, &mut second] {
        window.run_until_parked();
        window.update(|window, cx| {
            assert!(cx.reduce_motion());
            window.render_frame(cx);
            let _ = window.simulate_next_frame(cx);
            window.render_frame(cx);
            assert!(window.find("home-page").visible());
            assert_eq!(
                window.simulate_next_frame(cx),
                0,
                "every open window sees the same settled policy"
            );
        });
    }
}

#[gpui_kit::test]
fn reduced_home_is_visible_and_settles_without_animation_frames(cx: &mut TestAppContext) {
    let (_, mut vcx) = test_window::harness(cx);
    vcx.update(|_, cx| {
        cx.global_mut::<Config>().motion_preference = MotionPreference::Reduce;
        crate::ui::motion::apply(cx);
    });
    vcx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        // Drop a callback the initial default-policy frame may have queued
        // before the reduction took effect.
        let _ = window.simulate_next_frame(cx);
        window.render_frame(cx);
        assert!(window.find("home-page").visible());
        assert!(cx.reduce_motion());
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "a settled home page must not request animation-only frames"
        );
    });
}

#[gpui_kit::test]
fn reduced_terminal_input_still_reaches_the_real_pane(cx: &mut TestAppContext) {
    use crate::daemon::protocol::ClientMsg;
    use std::io::ErrorKind;
    use std::time::Duration;

    let (app, mut vcx, mut stream) = test_window::harness_with_pane(cx);
    vcx.update(|_, cx| {
        cx.global_mut::<Config>().motion_preference = MotionPreference::Reduce;
        crate::ui::motion::apply(cx);
        assert!(cx.reduce_motion());
    });
    app.update_in(&mut vcx, |app, window, cx| app.focus_active(window, cx));
    vcx.run_until_parked();
    vcx.simulate_keystrokes("ctrl-u");
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    loop {
        match ClientMsg::read(&mut stream) {
            Ok(ClientMsg::Input(bytes)) => {
                assert_eq!(bytes, b"\x15");
                break;
            }
            Ok(_) => {}
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                panic!("terminal input never reached the pane under reduced motion");
            }
            Err(error) => panic!("pane socket failed: {error}"),
        }
    }
}
