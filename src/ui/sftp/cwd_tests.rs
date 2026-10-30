use super::*;
use crate::daemon::protocol::{DaemonMsg, RemoteContext, RemoteKind, SshPhase};
use crate::ui::app::test_window::harness_with_pane;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};

fn report_cwd(
    app: &Entity<LyrelApp>,
    vcx: &mut VisualTestContext,
    stream: &mut crate::daemon::transport::Stream,
    cwd: &str,
) {
    DaemonMsg::Cwd(PathBuf::from(cwd)).encode(stream).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let arrived = app.update_in(vcx, |app, window, cx| {
            app.tabs[app.tabs.active_index()]
                .detail_pane(window, cx)
                .unwrap()
                .read(cx)
                .cwd()
                == Some(PathBuf::from(cwd))
        });
        if arrived {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "cwd frame did not arrive"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui_kit::test]
fn an_open_sftp_panel_follows_cwd_frames_without_a_pane_switch(cx: &mut TestAppContext) {
    let (app, mut vcx, mut stream) = harness_with_pane(cx);
    DaemonMsg::RemoteContext(Some(RemoteContext {
        kind: RemoteKind::NativeSsh,
        argv: vec![],
        target: "cwd-consumer-test".into(),
    }))
    .encode(&mut stream)
    .unwrap();
    DaemonMsg::SshStatus {
        phase: SshPhase::Connected,
    }
    .encode(&mut stream)
    .unwrap();
    report_cwd(&app, &mut vcx, &mut stream, "/srv/repo");
    let first = app.update_in(&mut vcx, |app, window, cx| {
        app.right_panel_visible = true;
        app.right_panel_tab = crate::core::config::RightPanelTab::Files;
        assert!(app.sftp_sync_pane(Some(1), window, cx));
        app.sftp_panel.nav_gen
    });
    let mut previous = first;
    for cwd in ["/srv/repo/src", "/tmp/outside"] {
        report_cwd(&app, &mut vcx, &mut stream, cwd);
        app.update_in(&mut vcx, |app, window, cx| {
            assert_eq!(
                app.sftp_panel.open_pane_id,
                Some(1),
                "the connected pane remains open"
            );
            assert!(app.sftp_sync_pane(Some(1), window, cx));
            assert!(
                app.sftp_panel.nav_gen > previous,
                "same-pane cd must navigate to {cwd}"
            );
            previous = app.sftp_panel.nav_gen;
            assert!(
                !app.sftp_panel.navigation_is_current(1, first),
                "late login-directory results cannot undo cd"
            );
            let followed = app.sftp_panel.nav_gen;
            app.sftp_sync_pane(Some(1), window, cx);
            assert_eq!(
                app.sftp_panel.nav_gen, followed,
                "unchanged cwd must not resubmit"
            );
        });
    }
    app.update_in(&mut vcx, |app, _, cx| {
        assert!(app.sftp_panel.nav_gen > first);
        let pending = app.sftp_panel.nav_gen;
        app.sftp_close_browser(cx);
        assert_ne!(
            app.sftp_panel.nav_gen, pending,
            "closing retires late navigation results"
        );
        app.sftp_panel.open_pane_id = Some(1);
        assert!(
            !app.sftp_panel.navigation_is_current(1, pending),
            "same pane after reconnect rejects the old listing"
        );
        app.sftp_close_browser(cx);
    });
}

#[gpui_kit::test]
fn a_workspace_switch_with_the_same_pane_id_retires_sftp_navigation(cx: &mut TestAppContext) {
    use crate::core::session::{RemoteTarget, WorkspaceId};
    use crate::terminal::PaneWorkspace;

    let (app, mut vcx, _stream) = harness_with_pane(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].detail_pane(window, cx).unwrap();
        let old = PaneWorkspace {
            workspace: WorkspaceId::new(),
            target: RemoteTarget::Alias {
                alias: "sftp-old".into(),
            },
            spec: None,
            label: None,
            resize_echo: false,
        };
        let new = PaneWorkspace {
            workspace: WorkspaceId::new(),
            target: RemoteTarget::Alias {
                alias: "sftp-new".into(),
            },
            ..old.clone()
        };
        app.sftp_panel.open_pane_id = Some(1);
        app.sftp_panel.open_workspace = Some(old.clone());
        app.sftp_panel
            .cwds
            .insert((Some(old.workspace), 1), "/old".into());
        let pending = app.sftp_panel.nav_gen;
        pane.update(cx, |pane, _| pane.set_workspace(Some(new.clone())));
        assert!(app.sftp_sync_pane(Some(1), window, cx));
        assert_eq!(
            app.sftp_panel.open_workspace.as_ref().unwrap().workspace,
            new.workspace
        );
        assert!(!app.sftp_panel.navigation_is_current(1, pending));
        assert!(!app.sftp_panel.cwds.contains_key(&(Some(new.workspace), 1)));
        app.sftp_close_browser(cx);
    });
}
