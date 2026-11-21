use crate::ui::app::test_window;
use crate::ui::settings::SettingsSection;
use crate::ui::settings::test_window as settings_window;
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;
use lyrel_core::core::config::MotionPreference;
use lyrel_ui_base::config::Config;
use lyrel_ui_base::i18n::{L10nKey, t};

fn set_motion(vcx: &mut gpui_kit::VisualTestContext, preference: MotionPreference) {
    vcx.update(|_, cx| {
        cx.global_mut::<Config>().motion_preference = preference;
        crate::ui::motion::apply(cx);
    });
    vcx.run_until_parked();
}

/// The first `ClientMsg::Input` the pane sent its daemon, if any.
fn pane_input(daemon: &mut crate::daemon::transport::Stream) -> Option<Vec<u8>> {
    use std::io::ErrorKind;
    daemon
        .set_read_timeout(Some(std::time::Duration::from_millis(250)))
        .unwrap();
    loop {
        match crate::daemon::protocol::ClientMsg::read(daemon) {
            Ok(crate::daemon::protocol::ClientMsg::Input(bytes)) => return Some(bytes),
            Ok(_) => continue,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return None;
            }
            Err(e) => panic!("pane socket failed: {e}"),
        }
    }
}

/// Drops the harness pane's SSH link the way the daemon reports it: a failed
/// phase with a reason, then the child exiting. The pane keeps its spec, so
/// the workbench draws the floating notice pill over it.
fn drop_ssh_link(stream: &mut crate::daemon::transport::Stream) {
    use crate::daemon::protocol::{DaemonMsg, SshPhase};
    DaemonMsg::SshStatus {
        phase: SshPhase::Failed {
            reason: "motion-test link drop".into(),
        },
    }
    .encode(stream)
    .unwrap();
    DaemonMsg::Exited { code: None }.encode(stream).unwrap();
}

fn give_pane_an_ssh_spec(
    app: &gpui_kit::Entity<crate::ui::app::LyrelApp>,
    vcx: &mut gpui_kit::VisualTestContext,
) {
    use crate::daemon::protocol::NativeSshSpec;
    let mut profile = crate::core::ssh_profile::SshProfile::new("motion-test".to_string());
    profile.host = "motion-test".to_string();
    profile.user = "tester".to_string();
    profile.port = 22;
    let spec: NativeSshSpec = crate::ui::ssh_connect::build_native_ssh_spec(
        &profile,
        &[],
        &crate::core::keychain::InMemoryCredentialStore::new(),
        false,
    );
    app.update_in(vcx, |app, window, cx| {
        let leaf = app.tabs[app.tabs.active_index()]
            .detail_pane(window, cx)
            .expect("the harness has one pane");
        leaf.update(cx, |view, _| {
            view.set_restored_ssh_spec(Some(Box::new(spec)));
        });
    });
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn toast_entrance_settles_and_reduce_keeps_the_pill_static(cx: &mut TestAppContext) {
    let (app, mut vcx, mut stream) = test_window::harness_with_pane(cx);
    give_pane_an_ssh_spec(&app, &mut vcx);

    // Motion allowed: the dropped link raises the pill with a short
    // translation-plus-fade entrance that then settles.
    drop_ssh_link(&mut stream);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        vcx.run_until_parked();
        let up = vcx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("ssh-strip-reason").is_some()
        });
        if up {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the toast never appeared after the link dropped"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    vcx.update(|window, cx| {
        assert!(
            window.simulate_next_frame(cx) > 0,
            "the toast entrance runs as an animation while motion is allowed"
        );
    });
    vcx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "the settled toast requests no animation-only frames"
        );
        assert!(
            window.find("ssh-strip-reason").visible(),
            "the settled toast shows its reason at full opacity"
        );
    });

    // Reduce: the same pill stays legible at its final state, statically.
    set_motion(&mut vcx, MotionPreference::Reduce);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        // Drop a callback the preference switch may have queued.
        let _ = window.simulate_next_frame(cx);
        window.render_frame(cx);
        assert!(window.find("ssh-strip-reason").visible());
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "a settled toast must not request animation-only frames under Reduce"
        );
    });
}

#[gpui_kit::test]
fn picker_entrance_settles_and_fast_dismiss_returns_the_keyboard(cx: &mut TestAppContext) {
    let (app, mut vcx, mut streams) = test_window::harness_with_tabs(cx, 1);
    let daemon = &mut streams[0];
    let chord = vcx
        .update(|_, cx| crate::ui::keymap::effective_key("TogglePalette", cx))
        .unwrap_or_else(|| panic!("TogglePalette is bound"));
    let pane_focus = app.update_in(&mut vcx, |app, window, cx| {
        app.focus_active(window, cx);
        app.tabs[app.tabs.active_index()]
            .detail_pane(window, cx)
            .expect("the harness has one pane")
            .read(cx)
            .focus_handle(cx)
    });
    let showing = |vcx: &mut gpui_kit::VisualTestContext| {
        app.read_with(vcx, |app, cx| app.search.view().map(|s| s.read(cx).tab()))
    };

    vcx.simulate_keystrokes(&chord);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.find("search-card");
        assert!(
            window.simulate_next_frame(cx) > 0,
            "the picker's entrance runs as an animation while motion is allowed"
        );
    });
    vcx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "the settled picker requests no animation-only frames"
        );
        assert!(
            window.find("search-card").visible(),
            "the picker is fully shown once the entrance settles"
        );
        assert_ne!(
            window.focused(cx),
            Some(pane_focus.clone()),
            "opening the picker moves the keyboard to its query field"
        );
    });

    // A fast dismissal does not wait for anything: the card is gone, the
    // keyboard is back on the pane, and the next keystroke reaches it.
    vcx.simulate_keystrokes(&chord);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("search-card").is_none(),
            "the dismissed picker is gone immediately"
        );
        assert_eq!(
            window.focused(cx),
            Some(pane_focus.clone()),
            "dismissing the picker returns the keyboard to the pane"
        );
    });
    assert_eq!(showing(&mut vcx), None, "the palette reports closed");
    vcx.simulate_keystrokes("q");
    vcx.run_until_parked();
    assert_eq!(
        pane_input(daemon).as_deref(),
        Some(&b"q"[..]),
        "after a fast dismissal the keyboard reaches the pane without waiting for a transition"
    );

    // Reduce: the reopened picker is fully usable at its final state, with no
    // animation frames at all.
    set_motion(&mut vcx, MotionPreference::Reduce);
    vcx.simulate_keystrokes(&chord);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let _ = window.simulate_next_frame(cx);
        window.render_frame(cx);
        window.find("search-card");
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "Reduce opens the picker at its final state without animation frames"
        );
    });
}

/// The hint reveal and fade-out timers are real-time (`smol::Timer` on a
/// spawned task): sleeping past them and then running the executor is what
/// lets their tasks land, as in the `ui::hints` tests.
fn sleep_ms(ms: u64) {
    std::thread::sleep(std::time::Duration::from_millis(ms));
}

#[gpui_kit::test]
fn held_hint_badges_fade_in_and_out_without_idle_frames(cx: &mut TestAppContext) {
    let (_app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    let hold = vcx
        .update(|_, cx| crate::ui::keymap::tab_badge_hold(cx))
        .expect("the shipped tab chords are on a modifier");

    // Hold: after the real reveal delay the badges fade in, then settle.
    vcx.simulate_modifiers_change(hold);
    sleep_ms(300);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.find(("tab-badge", 0));
        assert!(
            window.simulate_next_frame(cx) > 0,
            "the badge fade-in runs as an animation while motion is allowed"
        );
    });
    vcx.executor()
        .advance_clock(std::time::Duration::from_millis(600));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.find(("tab-badge", 0)).visible(),
            "the held badges stay shown at their final state"
        );
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "held badges settle and request no idle frames"
        );
    });

    // Release: the badges dismiss instantly as state and fade out visually,
    // then unmount — the final state never waits on the transition.
    vcx.simulate_modifiers_change(gpui_kit::Modifiers::none());
    sleep_ms(80);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find_all(("tab-badge", 0)).len(),
            1,
            "the badges stay mounted while they fade out"
        );
    });
    sleep_ms(300);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.find_all(("tab-badge", 0)).is_empty(),
            "the badges unmount once the fade-out completes"
        );
    });
}

#[gpui_kit::test]
fn reduced_held_hints_appear_static_and_a_fast_tap_raises_nothing(cx: &mut TestAppContext) {
    let (_app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    let hold = vcx
        .update(|_, cx| crate::ui::keymap::tab_badge_hold(cx))
        .expect("the shipped tab chords are on a modifier");
    set_motion(&mut vcx, MotionPreference::Reduce);

    // A tap shorter than the hold delay raises no badges at all.
    vcx.simulate_modifiers_change(hold);
    sleep_ms(120);
    vcx.simulate_modifiers_change(gpui_kit::Modifiers::none());
    sleep_ms(400);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.find_all(("tab-badge", 0)).is_empty(),
            "a tap shorter than the hold delay raises no badges"
        );
    });

    // A real hold under Reduce shows the final state with no animation frames.
    vcx.simulate_modifiers_change(hold);
    sleep_ms(300);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let _ = window.simulate_next_frame(cx);
        window.render_frame(cx);
        window.find(("tab-badge", 0));
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "Reduce shows held hints at their final state without animation frames"
        );
    });

    // Releasing under Reduce dismisses them; nothing keeps requesting frames.
    vcx.simulate_modifiers_change(gpui_kit::Modifiers::none());
    sleep_ms(300);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.find_all(("tab-badge", 0)).is_empty());
        assert_eq!(window.simulate_next_frame(cx), 0);
    });
}

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
    // Every test in this process writes to the same scratch config.json; keep
    // a save→load→assert sequence atomic against a concurrent writer.
    let _config_file = Config::lock_config_file();
    let (settings, mut vcx) = settings_window::harness(cx);
    let dir =
        lyrel_core::core::config::config_dir_path().expect("the harness pins a scratch config");
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
            lyrel_ui_base::config::Config::load_with_outcome(),
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
