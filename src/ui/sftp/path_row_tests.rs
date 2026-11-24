//! The Native SSH Files path row: one row under the tool tabs holds the
//! scrollable breadcrumb and the fixed Refresh/More tiles, and Go to Path
//! swaps that row for a full-width editor in the same slot.

use super::*;
use crate::daemon::protocol::{DaemonMsg, RemoteContext, RemoteKind, SshPhase};
use crate::daemon::transport::Stream;
use crate::ui::app::test_window::harness_with_pane;
use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{Entity, MouseButton, ScrollDelta, TestAppContext, VisualTestContext, point};

fn settle(vcx: &mut VisualTestContext) {
    vcx.executor().advance_clock(Duration::from_millis(100));
    vcx.run_until_parked();
    vcx.update(|window, cx| window.render_frame(cx));
    vcx.run_until_parked();
}

fn click_label(vcx: &mut VisualTestContext, label: &str) {
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let item = window.find_by_label(label);
        let center = item.bounds().center();
        window.pointer_down(center, MouseButton::Left, cx);
        window.pointer_up(center, MouseButton::Left, cx);
    });
    settle(vcx);
}

/// A window browsing the quiet test pane as a connected native SSH Host.
fn sftp_harness(cx: &mut TestAppContext) -> (Entity<LyrelApp>, VisualTestContext, Stream) {
    let (app, mut vcx, mut stream) = harness_with_pane(cx);
    vcx.update(|_, _| lyrel_ui_base::i18n::set_locale("en"));
    DaemonMsg::RemoteContext(Some(RemoteContext {
        kind: RemoteKind::NativeSsh,
        argv: vec![],
        target: "sftp-path-row".into(),
    }))
    .encode(&mut stream)
    .unwrap();
    DaemonMsg::SshStatus {
        phase: SshPhase::Connected,
    }
    .encode(&mut stream)
    .unwrap();
    DaemonMsg::Cwd(std::path::PathBuf::from("/srv/repo"))
        .encode(&mut stream)
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let ready = app.update_in(&mut vcx, |app, window, cx| {
            app.right_panel_visible = true;
            app.right_panel_tab = lyrel_core::core::config::RightPanelTab::Files;
            let Some(pane) = app.tabs[app.tabs.active_index()].detail_pane(window, cx) else {
                return false;
            };
            let view = pane.read(cx);
            view.remote_context()
                .is_some_and(|remote| remote.kind == RemoteKind::NativeSsh)
                && matches!(view.ssh_phase(), Some(SshPhase::Connected))
        });
        if ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pane did not report a connected native SSH context"
        );
        settle(&mut vcx);
    }
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.sftp_sync_pane(Some(1), window, cx));
    });
    settle(&mut vcx);
    settle(&mut vcx);
    (app, vcx, stream)
}

/// Put a path on screen without a daemon round trip and let the lane settle.
fn show_path(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext, path: &str) {
    app.update_in(vcx, |app, _window, cx| {
        app.sftp_panel.cwd = path.to_string();
        cx.notify();
    });
    settle(vcx);
    settle(vcx);
}

/// A path too long for the lane to show whole, ending in a Unicode
/// directory name.
fn long_path(depth: usize) -> String {
    let nested = vec!["a-very-long-directory-name"; depth].join("/");
    format!("/srv/{nested}/终端/工作目录")
}

#[gpui_kit::test]
fn the_first_row_holds_the_scrollable_breadcrumb_and_fixed_actions(cx: &mut TestAppContext) {
    let (_app, mut vcx, _stream) = sftp_harness(cx);
    vcx.update(|window, _| {
        let header = window.find("side-panel-tool-header");
        let row = window.find("sftp-path-row");
        let row_bounds = row.bounds();
        let lane = window.find("sftp-breadcrumb");
        let crumb = window.find("sftp-crumb-/");
        let refresh = window.find("panel-sftp-refresh");
        let more = window.find("panel-sftp-menu");

        assert!(
            row_bounds.top() >= header.bounds().bottom() - gpui_kit::px(0.5),
            "the path row is the first row beneath the tool tabs"
        );
        assert!(
            lane.bounds().left() >= row_bounds.left(),
            "the breadcrumb lane lives inside the row"
        );
        assert!(
            lane.bounds().right() <= refresh.bounds().left(),
            "Refresh stays fixed at the lane's trailing edge"
        );
        assert!(more.bounds().left() >= refresh.bounds().right());
        for element in [crumb.bounds(), refresh.bounds(), more.bounds()] {
            assert!(
                (element.center().y - row_bounds.center().y).abs() <= gpui_kit::px(1.),
                "the path and its actions share one vertical centre"
            );
        }
    });
}

#[gpui_kit::test]
fn a_long_path_scrolls_its_lane_and_reveals_the_current_directory(cx: &mut TestAppContext) {
    let (app, mut vcx, _stream) = sftp_harness(cx);
    let long = long_path(24);
    show_path(&app, &mut vcx, &long);

    vcx.update(|window, _| {
        let lane = window.find("sftp-breadcrumb");
        let lane_right = lane.bounds().right();
        let head = window.find("sftp-crumb-/");
        let tail_id = SharedString::from(format!("sftp-crumb-{long}"));
        let tail = window.find(tail_id);
        assert!(
            head.bounds().left() < lane.bounds().left(),
            "a long path scrolls its ancestors out of the lane instead of wrapping"
        );
        assert!(
            tail.bounds().right() <= lane_right + gpui_kit::px(1.),
            "navigation reveals the current directory"
        );
        // The actions stay fixed and reachable however long the path is.
        let row_right = window.find("sftp-path-row").bounds().right();
        assert!(window.find("panel-sftp-refresh").bounds().right() <= row_right);
        assert!(window.find("panel-sftp-menu").bounds().right() <= row_right);
    });
}

#[gpui_kit::test]
fn resizing_re_reveals_the_current_directory(cx: &mut TestAppContext) {
    let (app, mut vcx, _stream) = sftp_harness(cx);
    // A known wide window first: the panel sits at its preferred width, so
    // the narrowing below is a real change in the breadcrumb lane.
    vcx.simulate_resize(gpui_kit::size(gpui_kit::px(1400.), gpui_kit::px(900.)));
    settle(&mut vcx);
    settle(&mut vcx);
    let long = long_path(16);
    show_path(&app, &mut vcx, &long);

    // Wheel back to the ancestors: the current directory may leave the
    // viewport when the user goes looking for its parents.
    vcx.update(|window, cx| {
        window.scroll("sftp-breadcrumb", ScrollDelta::Lines(point(500., 0.)), cx);
    });
    settle(&mut vcx);
    let wide_lane = vcx.update(|window, _| {
        let lane = window.find("sftp-breadcrumb");
        let tail_id = SharedString::from(format!("sftp-crumb-{long}"));
        assert!(
            window.find(tail_id).bounds().right() > lane.bounds().right(),
            "the wheel scrolls the lane back toward the ancestors"
        );
        lane.bounds().size.width
    });

    // A width change re-reveals the tail on the frames that follow it.
    vcx.simulate_resize(gpui_kit::size(gpui_kit::px(500.), gpui_kit::px(900.)));
    settle(&mut vcx);
    settle(&mut vcx);
    vcx.update(|window, _| {
        let lane = window.find("sftp-breadcrumb");
        let tail_id = SharedString::from(format!("sftp-crumb-{long}"));
        assert!(
            lane.bounds().size.width < wide_lane,
            "the narrower window must narrow the breadcrumb lane"
        );
        assert!(
            window.find(tail_id).bounds().right() <= lane.bounds().right() + gpui_kit::px(1.),
            "a narrower panel must reveal the current directory again"
        );
        // Narrowing the panel must not push the fixed actions out of line.
        let row = window.find("sftp-path-row");
        assert!(lane.bounds().right() <= window.find("panel-sftp-refresh").bounds().left());
        for action in [
            window.find("panel-sftp-refresh"),
            window.find("panel-sftp-menu"),
        ] {
            assert!(
                action.bounds().left() >= row.bounds().left()
                    && action.bounds().right() <= row.bounds().right(),
                "the actions stay inside the narrow row"
            );
            assert!(
                (action.bounds().center().y - row.bounds().center().y).abs() <= gpui_kit::px(1.),
                "the actions stay vertically aligned in a narrow panel"
            );
        }
    });
}

#[gpui_kit::test]
fn go_to_path_swaps_the_row_for_a_full_width_editor(cx: &mut TestAppContext) {
    let (_app, mut vcx, _stream) = sftp_harness(cx);
    vcx.update(|window, cx| window.click("panel-sftp-menu", cx));
    settle(&mut vcx);
    click_label(&mut vcx, t(L10nKey::SftpMenuGotoPath));
    // The editor opens on the effect cycle after the menu hands focus back.
    settle(&mut vcx);

    let (editor_w, row_w) = vcx.update(|window, _| {
        let row = window.find("sftp-path-row");
        let edit = window.find("sftp-path-edit");
        assert!(
            window.try_find("sftp-breadcrumb").is_none(),
            "the editor takes the path row's slot, not a second row"
        );
        assert!(
            window.try_find("panel-sftp-refresh").is_none()
                && window.try_find("panel-sftp-menu").is_none(),
            "the actions step aside while the editor is up"
        );
        (edit.bounds().size.width, row.bounds().size.width)
    });
    assert!(
        editor_w > row_w - gpui_kit::px(40.),
        "the editor spans the row minus its content insets"
    );

    vcx.simulate_keystrokes("escape");
    settle(&mut vcx);
    vcx.update(|window, _| {
        assert!(
            window.try_find("sftp-path-edit").is_none(),
            "Escape cancels"
        );
        assert!(
            window.try_find("sftp-breadcrumb").is_some(),
            "the breadcrumb returns"
        );
        assert!(
            window.try_find("panel-sftp-refresh").is_some()
                && window.try_find("panel-sftp-menu").is_some(),
            "the actions return when editing ends"
        );
    });
}

#[gpui_kit::test]
fn double_click_opens_the_editor_and_enter_submits(cx: &mut TestAppContext) {
    let (_app, mut vcx, _stream) = sftp_harness(cx);
    vcx.update(|window, cx| window.double_click("sftp-breadcrumb", cx));
    settle(&mut vcx);
    vcx.update(|window, _| {
        assert!(
            window.try_find("sftp-path-edit").is_some(),
            "the existing double-click gesture opens the same editor"
        );
        assert!(window.try_find("panel-sftp-refresh").is_none());
    });

    vcx.simulate_keystrokes("enter");
    settle(&mut vcx);
    vcx.update(|window, _| {
        assert!(
            window.try_find("sftp-path-edit").is_none(),
            "Enter submits the path"
        );
        assert!(
            window.try_find("panel-sftp-refresh").is_some()
                && window.try_find("panel-sftp-menu").is_some(),
            "the actions return when editing ends"
        );
    });
}
