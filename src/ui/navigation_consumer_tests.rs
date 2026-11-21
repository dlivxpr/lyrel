//! Production Root consumers for manual-activation navigation, not private focus helpers.
use crate::ui::settings::SettingsSection;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Focusable as _, TestAppContext, VisualTestContext, px, size};
use lyrel_core::core::config::RightPanelTab;
use lyrel_ui_base::actions::{
    FocusRightPanelNavigation, FocusSettingsNavigation, FocusTerminalTabNavigation,
    ShowRightPanelFiles,
};
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.render_frame(cx);
        });
    }
    cx.run_until_parked();
}

#[gpui_kit::test]
fn panel_navigation_enters_selected_roves_activates_and_returns_to_content(
    cx: &mut TestAppContext,
) {
    for locale in ["en", "zh-CN"] {
        lyrel_ui_base::i18n::set_locale(locale);
        let (app, mut vcx, _stream) = crate::ui::app::test_window::harness_with_pane(cx);
        vcx.simulate_resize(size(px(1000.), px(720.)));
        let content = app.update_in(&mut vcx, |app, window, cx| {
            app.focus_active(window, cx);
            window.focused(cx).expect("terminal content owns focus")
        });
        // Roving needs tabs to rove across: a fresh workspace prepares only
        // Info, so open the other three tools through the same commands
        // Search Everywhere dispatches, then select Info again so entry
        // lands where the old fixed row used to start.
        vcx.dispatch_action(lyrel_ui_base::actions::ShowRightPanelFiles);
        vcx.dispatch_action(lyrel_ui_base::actions::ShowRightPanelChanges);
        vcx.dispatch_action(lyrel_ui_base::actions::ShowRightPanelGitHub);
        vcx.dispatch_action(lyrel_ui_base::actions::ShowRightPanelInfo);
        settle(&mut vcx);
        vcx.dispatch_action(FocusRightPanelNavigation);
        settle(&mut vcx);
        vcx.update(|window, cx| {
            assert_eq!(window.find("tool-tab-info").focused(), Some(true));
            window.press("right", cx);
            assert_eq!(window.find("tool-tab-files").focused(), Some(true));
            assert_eq!(window.find("tool-tab-info").selected(), Some(true));
            window.press("enter", cx);
        });
        settle(&mut vcx);
        assert_eq!(
            app.read_with(&vcx, |app, _| app.right_panel_tab),
            RightPanelTab::Files
        );
        vcx.update(|window, cx| {
            assert_eq!(window.find("tool-tab-files").focused(), Some(true));
            window.press("right", cx);
            assert_eq!(window.find("tool-tab-changes").focused(), Some(true));
            window.press("space", cx);
        });
        settle(&mut vcx);
        assert_eq!(
            app.read_with(&vcx, |app, _| app.right_panel_tab),
            RightPanelTab::Scm
        );
        vcx.update(|window, cx| {
            window.press("end", cx);
            assert_eq!(window.find("tool-tab-github").focused(), Some(true));
            window.press("right", cx);
            assert_eq!(window.find("tool-tab-info").focused(), Some(true));
            window.press("home", cx);
            window.press("tab", cx);
            assert_ne!(window.find("tool-tab-info").focused(), Some(true));
            window.press("shift-tab", cx);
            assert_eq!(window.find("tool-tab-changes").focused(), Some(true));
            window.press("escape", cx);
            assert!(
                content.is_focused(window),
                "Escape returns to the owning terminal"
            );
            window.click("tool-tab-files", cx);
            assert!(
                content.is_focused(window),
                "pointer activation keeps content focus"
            );
        });
    }
    lyrel_ui_base::i18n::set_locale("en");
}

#[gpui_kit::test]
fn panel_navigation_retry_yields_to_a_real_external_focus_change(cx: &mut TestAppContext) {
    let (app, mut vcx, _stream) = crate::ui::app::test_window::harness_with_pane(cx);
    vcx.simulate_resize(size(px(1000.), px(720.)));
    app.update_in(&mut vcx, |app, window, cx| app.focus_active(window, cx));
    vcx.dispatch_action(FocusRightPanelNavigation);

    // A real Root navigation action takes focus before the pending entry's
    // first frame. The old panel retry must not steal it back on a later render.
    vcx.dispatch_action(FocusTerminalTabNavigation);
    settle(&mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.sidebar_nav_focus.is_focused(window));
        assert!(!app.right_panel_nav_focus.contains_focused(window, cx));
    });
}

#[gpui_kit::test]
fn panel_navigation_new_request_replaces_the_pending_origin(cx: &mut TestAppContext) {
    let (app, mut vcx, _stream) = crate::ui::app::test_window::harness_with_pane(cx);
    vcx.simulate_resize(size(px(1000.), px(720.)));
    app.update_in(&mut vcx, |app, window, cx| app.focus_active(window, cx));
    vcx.dispatch_action(FocusRightPanelNavigation);
    vcx.dispatch_action(FocusTerminalTabNavigation);
    vcx.dispatch_action(FocusRightPanelNavigation);
    settle(&mut vcx);
    vcx.update(|window, cx| {
        assert_eq!(window.find("tool-tab-info").focused(), Some(true));
        window.press("escape", cx);
    });
    app.update_in(&mut vcx, |app, window, _| {
        assert!(
            app.sidebar_nav_focus.is_focused(window),
            "Escape uses the new request's owning content, not the old terminal origin"
        );
    });
}

#[gpui_kit::test]
fn panel_navigation_owner_replacement_cancels_pending_entry(cx: &mut TestAppContext) {
    let (app, mut vcx, _stream) = crate::ui::app::test_window::harness_with_pane(cx);
    vcx.simulate_resize(size(px(1000.), px(720.)));
    settle(&mut vcx);
    let content = app.update_in(&mut vcx, |app, window, cx| {
        assert!(!app.right_panel_open(cx));
        app.focus_active(window, cx);
        window.focused(cx).expect("terminal content owns focus")
    });
    // Each VisualTestContext::dispatch_action drains work and may draw enough
    // frames to complete entry. Queue both actions before GPUI draws the panel.
    vcx.update(|window, cx| {
        window.dispatch_action(Box::new(FocusRightPanelNavigation), cx);
        window.defer(cx, {
            let app = app.clone();
            let content = content.clone();
            move |window, cx| {
                assert!(app.read(cx).right_panel_open(cx));
                assert!(content.is_focused(window), "entry is still pending");
            }
        });
        window.dispatch_action(Box::new(ShowRightPanelFiles), cx);
    });
    settle(&mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.right_panel_tab, RightPanelTab::Files);
        assert!(content.is_focused(window));
        assert!(!app.right_panel_nav_focus.contains_focused(window, cx));
    });
}

#[gpui_kit::test]
fn settings_navigation_retry_yields_to_external_input_focus(cx: &mut TestAppContext) {
    let (settings, mut vcx) = crate::ui::settings::test_window::harness(cx);
    vcx.simulate_resize(size(px(960.), px(700.)));
    vcx.dispatch_action(FocusSettingsNavigation);
    let search = settings.update_in(&mut vcx, |settings, window, cx| {
        let search = settings
            .active_settings()
            .unwrap()
            .navigation
            .search()
            .clone();
        search.update(cx, |input, cx| input.focus(window, cx));
        search.read(cx).focus_handle(cx)
    });
    settle(&mut vcx);
    settings.update_in(&mut vcx, |settings, window, _| {
        assert!(search.is_focused(window));
        assert!(!settings.navigation_focus_handle().is_focused(window));
    });
}

#[gpui_kit::test]
fn settings_navigation_is_vertical_manual_and_restores_page_focus(cx: &mut TestAppContext) {
    for locale in ["en", "zh-CN"] {
        lyrel_ui_base::i18n::set_locale(locale);
        let (settings, mut vcx) = crate::ui::settings::test_window::harness(cx);
        vcx.simulate_resize(size(px(960.), px(700.)));
        vcx.dispatch_action(FocusSettingsNavigation);
        settle(&mut vcx);
        vcx.update(|window, cx| {
            assert_eq!(
                window.find("settings-nav-settings:general").focused(),
                Some(true)
            );
            window.press("down", cx);
            assert_eq!(
                window.find("settings-nav-settings:appearance").focused(),
                Some(true)
            );
            assert_eq!(
                window.find("settings-nav-settings:general").selected(),
                Some(true)
            );
            window.press("enter", cx);
        });
        settle(&mut vcx);
        assert!(settings.read_with(&vcx, |settings, _| {
            settings.active_settings().unwrap().navigation.section() == SettingsSection::Appearance
        }));
        vcx.update(|window, cx| {
            assert_eq!(
                window.find("settings-nav-settings:appearance").focused(),
                Some(true)
            );
            window.press("down", cx);
            window.press("space", cx);
        });
        settle(&mut vcx);
        assert!(settings.read_with(&vcx, |settings, _| {
            settings.active_settings().unwrap().navigation.section() == SettingsSection::Terminal
        }));
        vcx.update(|window, cx| {
            window.press("end", cx);
            assert_eq!(
                window.find("settings-nav-settings:about").focused(),
                Some(true)
            );
            window.press("down", cx);
            assert_eq!(
                window.find("settings-nav-settings:general").focused(),
                Some(true)
            );
            window.press("home", cx);
            window.press("tab", cx);
            assert_ne!(
                window.find("settings-nav-settings:general").focused(),
                Some(true)
            );
            window.press("shift-tab", cx);
            assert_eq!(
                window.find("settings-nav-settings:terminal").focused(),
                Some(true)
            );
            window.press("escape", cx);
        });
        settings.read_with(&vcx, |settings, _| {
            assert!(
                settings.has_settings(),
                "leaving navigation must not close settings"
            );
        });
        settings.update_in(&mut vcx, |settings, window, _| {
            assert!(
                settings
                    .active_settings()
                    .unwrap()
                    .focus_handle
                    .is_focused(window)
            );
        });
    }
    lyrel_ui_base::i18n::set_locale("en");
}

// ---- Panel boundary motion (#356): the workbench window is the seam; the
// assertions run through production Root consumers — the visible boundary, the
// pane's real PTY size notifications, and the motion preference — never
// private slide state fields.

use std::time::Duration;

use lyrel_core::core::config::MotionPreference;
use lyrel_ui_base::config::Config;
use std::io::ErrorKind;

use crate::daemon::protocol::ClientMsg;

/// Every `Resize` frame the pane connection has emitted, in order. Other
/// frames are skipped; the read stops at the first timeout, so a quiet pane
/// drains clean.
fn resize_frames(stream: &mut crate::daemon::transport::Stream) -> Vec<(u16, u16)> {
    stream
        .set_read_timeout(Some(Duration::from_millis(150)))
        .expect("resize probe timeout");
    let mut frames = Vec::new();
    loop {
        match ClientMsg::read(stream) {
            Ok(ClientMsg::Resize(win)) => frames.push((win.cols, win.rows)),
            Ok(_) => {}
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                break;
            }
            Err(error) => panic!("pane connection failed: {error}"),
        }
    }
    frames
}

fn allow_motion(vcx: &mut VisualTestContext, preference: MotionPreference) {
    vcx.update(|_, cx| {
        cx.global_mut::<Config>().motion_preference = preference;
        crate::ui::motion::apply(cx);
    });
}

#[gpui_kit::test]
fn right_panel_toggle_slides_the_boundary_and_resizes_the_pty_once(cx: &mut TestAppContext) {
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    vcx.simulate_resize(size(px(1000.), px(720.)));
    settle(&mut vcx);
    resize_frames(&mut stream);
    allow_motion(&mut vcx, MotionPreference::Allow);
    vcx.update(|_, cx| assert!(!cx.reduce_motion()));

    // Reduced motion is the independent source of the final geometry: the same
    // toggle with no slide has to resize the PTY exactly once, straight to the
    // grid the animated toggle must reach without passing through any
    // visual-frame sizes.
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    app.update_in(&mut vcx, |app, _, cx| app.toggle_right_panel(cx));
    settle(&mut vcx);
    let reduced_open = resize_frames(&mut stream);
    assert_eq!(reduced_open.len(), 1, "a reduced open resizes the PTY once");
    let panel_left = vcx.update(|window, _| window.find("right-panel-resize").bounds().center().x);
    app.update_in(&mut vcx, |app, _, cx| app.toggle_right_panel(cx));
    settle(&mut vcx);
    assert_eq!(
        resize_frames(&mut stream).len(),
        1,
        "a reduced close resizes the PTY once"
    );
    vcx.update(|_, cx| cx.set_reduce_motion(false));

    // The animated open: the seam starts shut, and the terminal is told its
    // final grid on the toggle frame — one resize for the whole slide.
    app.update_in(&mut vcx, |app, _, cx| app.toggle_right_panel(cx));
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let seam = window.find("right-panel-slide").bounds();
        assert_eq!(seam.size.width, px(0.), "the seam starts at the window edge");
    });
    let opened = resize_frames(&mut stream);
    assert_eq!(
        opened.len(),
        1,
        "the whole slide costs the PTY exactly one resize"
    );
    assert_eq!(
        opened[0], reduced_open[0],
        "and it lands on the final grid immediately, not on a visual-frame size"
    );

    // Halfway through the 260 ms slide the non-overshooting curve has covered
    // most of the distance but not arrived; the seam never passes its target.
    vcx.executor().advance_clock(Duration::from_millis(130));
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let seam = window.find("right-panel-slide").bounds();
        let full = window.viewport_size().width - panel_left;
        assert!(
            seam.size.width > full * 0.8 && seam.size.width < full,
            "mid-slide seam {seam:?} should be past 80% of {full} but not settled"
        );
    });

    // Past the duration the slide hands back to the settled mount at exactly
    // the requested layout, and the visual frames drove no further resizes.
    vcx.executor().advance_clock(Duration::from_millis(200));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("right-panel-slide").is_none(),
            "the slide hands back to the settled mount"
        );
        assert_eq!(
            window.find("right-panel-resize").bounds().center().x,
            panel_left,
            "the boundary settles at the requested final layout"
        );
    });
    assert!(
        resize_frames(&mut stream).is_empty(),
        "no transient resizes were driven by the slide's visual frames"
    );
}

#[gpui_kit::test]
fn repeated_right_panel_toggles_during_a_slide_are_ignored(cx: &mut TestAppContext) {
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    vcx.simulate_resize(size(px(1000.), px(720.)));
    settle(&mut vcx);
    resize_frames(&mut stream);
    allow_motion(&mut vcx, MotionPreference::Allow);

    app.update_in(&mut vcx, |app, _, cx| app.toggle_right_panel(cx));
    vcx.update(|window, cx| window.render_frame(cx));
    // Mid-slide toggles — same-frame repeats and a held key — change nothing:
    // no queueing, no reversal under the travelling seam.
    for _ in 0..3 {
        app.update_in(&mut vcx, |app, _, cx| app.toggle_right_panel(cx));
    }
    vcx.update(|window, cx| window.render_frame(cx));
    assert!(
        app.read_with(&vcx, |app, _| app.right_panel_visible),
        "the panel stays on its way to open"
    );
    assert!(
        app.read_with(&vcx, |app, _| {
            app.panel_slide_active(crate::ui::panel_slide::PanelBoundary::Right)
        }),
        "the original slide is still in flight"
    );

    vcx.executor().advance_clock(Duration::from_millis(100));
    app.update_in(&mut vcx, |app, _, cx| app.toggle_right_panel(cx));
    assert!(
        app.read_with(&vcx, |app, _| app.right_panel_visible),
        "a toggle inside the 260 ms window is dropped, not queued"
    );
    vcx.executor().advance_clock(Duration::from_millis(200));
    vcx.run_until_parked();
    vcx.update(|window, cx| window.render_frame(cx));
    assert!(
        app.read_with(&vcx, |app, _| app.right_panel_visible),
        "the boundary settled at the requested final layout"
    );

    // The panel still closes on the next toggle once the seam has settled.
    app.update_in(&mut vcx, |app, _, cx| app.toggle_right_panel(cx));
    vcx.executor().advance_clock(Duration::from_millis(300));
    vcx.run_until_parked();
    vcx.update(|window, cx| window.render_frame(cx));
    assert!(
        !app.read_with(&vcx, |app, _| app.right_panel_visible),
        "after the slide, the next toggle closes the panel"
    );
    resize_frames(&mut stream);
}

#[gpui_kit::test]
fn left_panel_toggle_slides_the_sidebar_boundary_and_resizes_the_pty_once(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    vcx.simulate_resize(size(px(1000.), px(720.)));
    settle(&mut vcx);
    resize_frames(&mut stream);
    // System preference: under the test scheduler the platform is not
    // consulted, so the reduced-motion flag stays released and the slide runs.
    allow_motion(&mut vcx, MotionPreference::System);
    vcx.update(|_, cx| assert!(!cx.reduce_motion()));
    let seam_rest = vcx.update(|window, _| window.find("sidebar-resize").bounds().center().x);
    assert!(seam_rest > px(0.));

    app.update_in(&mut vcx, |app, _, cx| app.toggle_left_panel(cx));
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let seam = window.find("left-panel-slide").bounds();
        assert_eq!(
            seam.size.width, seam_rest,
            "the closing seam starts at the sidebar's settled width"
        );
        assert_eq!(seam.origin.x, px(0.));
    });
    assert_eq!(
        resize_frames(&mut stream).len(),
        1,
        "closing the sidebar resizes the PTY once, straight to the final grid"
    );

    vcx.executor().advance_clock(Duration::from_millis(130));
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let seam = window.find("left-panel-slide").bounds();
        assert!(
            seam.size.width < seam_rest * 0.2 && seam.size.width > px(0.),
            "mid-slide the closing seam has travelled most of the way without overshooting: {seam:?}"
        );
    });

    vcx.executor().advance_clock(Duration::from_millis(200));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("left-panel-slide").is_none());
        assert!(
            window.try_find("sidebar-resize").is_none(),
            "the settled closed sidebar is not mounted"
        );
    });
    assert!(resize_frames(&mut stream).is_empty());
}

#[gpui_kit::test]
fn reduced_motion_renders_the_final_geometry_without_animation_frames(cx: &mut TestAppContext) {
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    vcx.simulate_resize(size(px(1000.), px(720.)));
    settle(&mut vcx);
    resize_frames(&mut stream);
    allow_motion(&mut vcx, MotionPreference::Reduce);
    vcx.update(|_, cx| assert!(cx.reduce_motion()));

    app.update_in(&mut vcx, |app, _, cx| app.toggle_right_panel(cx));
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(
            window.try_find("right-panel-slide").is_none(),
            "reduced motion never mounts the travelling seam"
        );
        window.find("right-panel-resize");
    });
    assert_eq!(
        resize_frames(&mut stream).len(),
        1,
        "the reduced open still resizes the PTY exactly once"
    );
    vcx.update(|window, cx| {
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "no animation frames are scheduled for the boundary"
        );
    });

    // The next toggle closes immediately, with no slide window to wait out.
    app.update_in(&mut vcx, |app, _, cx| app.toggle_right_panel(cx));
    vcx.update(|window, cx| window.render_frame(cx));
    assert!(!app.read_with(&vcx, |app, _| app.right_panel_visible));
    assert_eq!(resize_frames(&mut stream).len(), 1);
}
