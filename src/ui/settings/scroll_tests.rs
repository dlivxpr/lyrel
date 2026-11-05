use super::*;
use crate::ui::settings::test_window::harness;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{ScrollDelta, ScrollEvent, TestAppContext, VisualTestContext, point, size};

fn wheel(
    cx: &mut VisualTestContext,
    outer: &gpui_kit::ScrollHandle,
    list: &'static str,
    delta: ScrollDelta,
) {
    let position = cx.update(|window, cx| {
        window.render_frame(cx);
        let bounds = window.find(list).bounds().intersect(&outer.bounds());
        assert!(
            bounds.size.height > px(0.),
            "list must be under the pointer"
        );
        bounds.center()
    });
    cx.simulate_mouse_move(position, None, Default::default());
    cx.simulate_event(ScrollEvent {
        position,
        delta,
        ..Default::default()
    });
    cx.run_until_parked();
}

fn row_position(cx: &mut VisualTestContext, list: &'static str, row: &str) -> gpui_kit::Pixels {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.find(row.to_owned()).bounds().top() - window.find(list).bounds().top()
    })
}

fn assert_boundary_handoff(
    cx: &mut VisualTestContext,
    outer: &gpui_kit::ScrollHandle,
    list: &'static str,
    first: &str,
    last: &str,
    top: gpui_kit::Pixels,
) {
    let outer_start = outer.offset().y;
    // Overshooting the inner bottom consumes the entire event, not its
    // remainder. Only a subsequent event that starts at the edge chains out.
    wheel(
        cx,
        outer,
        list,
        ScrollDelta::Pixels(point(px(0.), px(-10000.))),
    );
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            (window.find(last.to_owned()).bounds().bottom() - window.find(list).bounds().bottom())
                .abs()
                < px(1.)
        );
    });
    assert_eq!(outer.offset().y, outer_start);
    let bottom = row_position(cx, list, first);
    wheel(
        cx,
        outer,
        list,
        ScrollDelta::Pixels(point(px(0.), px(-30.))),
    );
    assert_eq!(row_position(cx, list, first), bottom);
    assert_eq!(outer.offset().y, outer_start - px(30.));

    wheel(cx, outer, list, ScrollDelta::Pixels(point(px(0.), px(30.))));
    assert_eq!(row_position(cx, list, first), bottom + px(30.));
    assert_eq!(outer.offset().y, outer_start - px(30.));
    wheel(
        cx,
        outer,
        list,
        ScrollDelta::Pixels(point(px(0.), px(10000.))),
    );
    assert_eq!(row_position(cx, list, first), top);
    assert_eq!(outer.offset().y, outer_start - px(30.));
    wheel(cx, outer, list, ScrollDelta::Pixels(point(px(0.), px(20.))));
    assert_eq!(row_position(cx, list, first), top);
    assert_eq!(outer.offset().y, outer_start - px(10.));

    let outer_start = outer.offset().y;
    wheel(cx, outer, list, ScrollDelta::Lines(point(0., -1.)));
    assert!(row_position(cx, list, first) < top);
    assert_eq!(
        outer.offset().y,
        outer_start,
        "wheel detents stay inside too"
    );
    wheel(cx, outer, list, ScrollDelta::Lines(point(0., 1.)));
    assert_eq!(row_position(cx, list, first), top);
    assert_eq!(outer.offset().y, outer_start);
}

fn assert_unbounded_navigation(
    cx: &mut VisualTestContext,
    outer: &gpui_kit::ScrollHandle,
    region: &'static str,
) {
    assert!(outer.max_offset().y > px(50.));
    outer.set_offset(point(px(0.), px(-30.)));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    wheel(
        cx,
        outer,
        region,
        ScrollDelta::Pixels(point(px(0.), px(-20.))),
    );
    assert_eq!(
        outer.offset().y,
        px(-50.),
        "short/empty lists must not trap downward input"
    );
    wheel(
        cx,
        outer,
        region,
        ScrollDelta::Pixels(point(px(0.), px(20.))),
    );
    assert_eq!(
        outer.offset().y,
        px(-30.),
        "short/empty lists must not trap upward input"
    );
}

#[gpui_kit::test]
fn ssh_host_wheel_moves_the_list_without_moving_the_settings_page(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    let (outer, first) = app.update_in(&mut vcx, |app, window, cx| {
        cx.global_mut::<Config>().ssh_profiles = (0..40)
            .map(|i| {
                let mut profile =
                    crate::core::ssh_profile::SshProfile::new(&format!("host-{i:02}"));
                profile.host = format!("h{i}.example.invalid");
                profile
            })
            .collect();
        let first = cx.global::<Config>().ssh_profiles[0].id;
        app.open_settings_section(SettingsSection::Ssh, window, cx);
        (
            app.active_settings().unwrap().navigation.scroll().clone(),
            first,
        )
    });
    vcx.simulate_resize(size(px(1100.), px(600.)));
    vcx.run_until_parked();
    vcx.update(|window, cx| window.click("ssh-show-all", cx));
    vcx.run_until_parked();
    assert!(outer.max_offset().y > px(60.));
    outer.set_offset(point(px(0.), px(-30.)));
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    let row = format!("ssh-host-{}", first.as_u128());
    let before = row_position(&mut vcx, "ssh-host-list", &row);
    wheel(
        &mut vcx,
        &outer,
        "ssh-host-list",
        ScrollDelta::Pixels(point(px(0.), px(-30.))),
    );
    assert_eq!(
        row_position(&mut vcx, "ssh-host-list", &row),
        before - px(30.)
    );
    assert_eq!(
        outer.offset().y,
        px(-30.),
        "scrolling inside hosts must not move the page"
    );
    let last = app.read_with(&vcx, |_, cx| {
        format!(
            "ssh-host-{}",
            cx.global::<Config>().ssh_profiles[39].id.as_u128()
        )
    });
    assert_boundary_handoff(&mut vcx, &outer, "ssh-host-list", &row, &last, before);

    let (filter, selected) = app.update_in(&mut vcx, |app, window, cx| {
        let filter = app.active_settings().unwrap().ssh.filter().clone();
        window.focus(&filter.read(cx).focus_handle(cx), cx);
        (filter, cx.global::<Config>().ssh_profiles[1].id)
    });
    vcx.simulate_keystrokes("down down");
    let selected = format!("ssh-host-{}", selected.as_u128());
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(selected.clone()).expanded(), Some(true));
        window.input("host-00", cx);
    });
    vcx.simulate_resize(size(px(720.), px(480.)));
    vcx.run_until_parked();
    assert_unbounded_navigation(&mut vcx, &outer, "ssh-host-list");
    vcx.update(|window, cx| {
        assert!(filter.read(cx).focus_handle(cx).is_focused(window));
        window.press("secondary-a", cx);
        window.input("no-host-matches-this", cx);
    });
    vcx.run_until_parked();
    assert_unbounded_navigation(&mut vcx, &outer, "ssh-host-list-empty");
    vcx.update(|window, cx| {
        window.press("secondary-a", cx);
        window.press("backspace", cx);
        assert_eq!(
            window.find(selected).expanded(),
            Some(true),
            "filtering preserves the opened host"
        );
        assert!(filter.read(cx).focus_handle(cx).is_focused(window));
    });
    app.update_in(&mut vcx, |_, _, cx| {
        cx.global_mut::<Config>().ssh_profiles.clear();
        cx.notify();
    });
    vcx.run_until_parked();
    assert_unbounded_navigation(&mut vcx, &outer, "ssh-host-list-empty");
}

#[gpui_kit::test]
fn agent_diagnostic_wheel_moves_the_list_without_moving_the_settings_page(cx: &mut TestAppContext) {
    use crate::core::agent_hooks::{HookAgent, HooksDiagnostic, HooksState};

    let (app, mut vcx) = harness(cx);
    let outer = app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Agents, window, cx);
        let state = app.active_settings_mut().unwrap();
        // Hook discovery is not under test. Keep its asynchronous result from
        // replacing the five real agent rows and their normal diagnostics.
        state.agent_hooks_seq += 1;
        state.agent_show_all = true;
        state.agent_hooks_states = AgentHooksView::Ready(
            HookAgent::ALL
                .iter()
                .map(|&agent| AgentHookRow {
                    agent,
                    state: HooksState::NotInstalled,
                    target: String::new(),
                    diagnostic: match agent {
                        HookAgent::OpenCode => None,
                        HookAgent::Pi => Some(HooksDiagnostic::PiExtension),
                        HookAgent::OhMyPi => Some(HooksDiagnostic::OmpExtension),
                        HookAgent::Kimi => Some(HooksDiagnostic::KimiHistory),
                        HookAgent::Hermes => Some(HooksDiagnostic::HermesObserver {
                            home: std::path::PathBuf::from("/controlled/hermes"),
                        }),
                    },
                })
                .collect(),
        );
        let outer = state.navigation.scroll().clone();
        cx.notify();
        outer
    });
    vcx.simulate_resize(size(px(1100.), px(600.)));
    vcx.run_until_parked();
    assert!(outer.max_offset().y > px(60.));
    outer.set_offset(point(px(0.), px(-30.)));
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    let before = row_position(&mut vcx, "agent-hook-list", "agent-row-0");
    wheel(
        &mut vcx,
        &outer,
        "agent-hook-list",
        ScrollDelta::Pixels(point(px(0.), px(-30.))),
    );
    assert_eq!(
        row_position(&mut vcx, "agent-hook-list", "agent-row-0"),
        before - px(30.)
    );
    assert_eq!(
        outer.offset().y,
        px(-30.),
        "scrolling a long agent diagnostic must not move the page"
    );
    assert_boundary_handoff(
        &mut vcx,
        &outer,
        "agent-hook-list",
        "agent-row-0",
        "agent-row-4",
        before,
    );
    let filter = app.update_in(&mut vcx, |app, window, cx| {
        let filter = app.active_settings().unwrap().agent_query.clone();
        window.focus(&filter.read(cx).focus_handle(cx), cx);
        filter
    });
    vcx.update(|window, cx| window.input("OpenCode", cx));
    vcx.simulate_resize(size(px(720.), px(480.)));
    vcx.run_until_parked();
    assert_unbounded_navigation(&mut vcx, &outer, "agent-hook-list");
    vcx.update(|window, cx| {
        assert!(filter.read(cx).focus_handle(cx).is_focused(window));
        window.press("secondary-a", cx);
        window.input("no-agent-matches-this", cx);
    });
    vcx.run_until_parked();
    assert_unbounded_navigation(&mut vcx, &outer, "agent-hook-list-empty");
    vcx.update(|window, cx| {
        assert!(filter.read(cx).focus_handle(cx).is_focused(window));
    });
}
