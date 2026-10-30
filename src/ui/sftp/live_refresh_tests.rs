//! Real native SSH/SFTP consumer coverage; no listing or route substitution.
use super::*;
use crate::core::config::RightPanelTab;
use crate::daemon::protocol::{ClientMsg, DaemonMsg, RemoteContext, RemoteKind, SshPhase};
use crate::ui::app::test_window::harness_with_pane;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use lyrel_core::host::Host as _;

// The shared real-endpoint fixture also serves graphics consumers; this test
// intentionally needs only its router, native pane and RemoteHost helpers.
#[allow(dead_code)]
mod native_fixture {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/support/native_ssh_graphics.rs"
    ));
}

struct NativePane(u64);

impl Drop for NativePane {
    fn drop(&mut self) {
        if let Ok(mut stream) = crate::daemon::transport::connect() {
            let _ = ClientMsg::Kill { pane_id: self.0 }.encode(&mut stream);
        }
    }
}

fn native_pane(fixture: &native_fixture::NativeSshFixture) -> NativePane {
    let mut stream = crate::daemon::transport::connect().unwrap();
    stream
        .set_read_timeout(Some(native_fixture::STREAM_WITHIN))
        .unwrap();
    crate::daemon::protocol::negotiate_pane(&mut stream).unwrap();
    ClientMsg::GraphicsCapacity {
        encoded_bytes: 64 << 20,
        decoded_bytes: 128 << 20,
        offers: false,
    }
    .encode(&mut stream)
    .unwrap();
    ClientMsg::SpawnNativeSsh {
        cwd: None,
        size: native_fixture::win_size(),
        spec: Box::new(fixture.spec()),
    }
    .encode(&mut stream)
    .unwrap();
    let pane = match DaemonMsg::read(&mut stream).unwrap() {
        DaemonMsg::Spawned { pane_id } => NativePane(pane_id),
        other => panic!("native SSH pane was not spawned: {other:?}"),
    };
    loop {
        match DaemonMsg::read(&mut stream).unwrap() {
            DaemonMsg::SshStatus {
                phase: SshPhase::Connected,
            } => break,
            DaemonMsg::SshStatus {
                phase: SshPhase::Failed { reason },
            } => panic!("SSH fixture failed: {reason}"),
            DaemonMsg::AuthPrompt { .. } => panic!("explicit key/pin fixture must not prompt"),
            DaemonMsg::Exited { .. } | DaemonMsg::Error(_) => panic!("native SSH fixture exited"),
            _ => {}
        }
    }
    ClientMsg::Detach.encode(&mut stream).unwrap();
    pane
}

fn tick(vcx: &mut VisualTestContext) {
    // Advance the production 500ms timer in steps so repeated timers get armed.
    for _ in 0..5 {
        vcx.executor().advance_clock(Duration::from_millis(500));
        vcx.background_executor.run_until_parked();
    }
}

fn names(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> Vec<String> {
    app.update_in(vcx, |app, _, _| {
        app.sftp_panel
            .entries
            .iter()
            .map(|entry| entry.name.clone())
            .collect()
    })
}

#[gpui_kit::test]
#[ignore = "requires explicit isolated native SSH-to-Linux fixture and same-commit local server; run alone"]
fn visible_sftp_polling_observes_external_create_rename_and_delete(cx: &mut TestAppContext) {
    let binary = PathBuf::from(
        std::env::var_os("LYREL_TEST_SSH_LOCAL_SERVER").expect("same-commit lyrel-server required"),
    );
    let fixture = native_fixture::NativeSshFixture::start(&binary);
    crate::core::config::set_config_dir(fixture.local_config_dir());
    let host = fixture.connect_control();
    // Registers the unique remote root with the fixture's existing cleanup.
    fixture.install_sender(&host);
    let directory = fixture.remote_path("live-refresh");
    host.create_dir(&directory, false).unwrap();
    let cwd = directory.to_string_lossy().into_owned();
    let pane = native_pane(&fixture);
    let (app, mut vcx, mut stream) = harness_with_pane(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.tabs[0]
            .detail_pane(window, cx)
            .unwrap()
            .update(cx, |view, _| view.pane_id = pane.0);
    });
    DaemonMsg::RemoteContext(Some(RemoteContext {
        kind: RemoteKind::NativeSsh,
        argv: vec![],
        target: "sftp-live-refresh".into(),
    }))
    .encode(&mut stream)
    .unwrap();
    DaemonMsg::SshStatus {
        phase: SshPhase::Connected,
    }
    .encode(&mut stream)
    .unwrap();
    DaemonMsg::Cwd(directory.clone())
        .encode(&mut stream)
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let ready = app.update_in(&mut vcx, |app, window, cx| {
            let view = app.tabs[0].detail_pane(window, cx).unwrap();
            let view = view.read(cx);
            view.cwd() == Some(directory.clone()) && view.ssh_phase() == Some(SshPhase::Connected)
        });
        if ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "SSH consumer frames did not arrive"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    app.update_in(&mut vcx, |app, window, cx| {
        app.right_panel_visible = true;
        app.right_panel_tab = RightPanelTab::Files;
        assert!(app.sftp_sync_pane(Some(pane.0), window, cx));
    });
    vcx.background_executor.run_until_parked();
    app.update_in(&mut vcx, |app, _, _| {
        assert!(!app.sftp_panel.loading);
        assert_eq!(app.sftp_panel.cwd, cwd);
        assert!(app.sftp_panel.entries.is_empty());
        assert!(
            app.sftp_panel.error.is_none(),
            "real initial SFTP listing must succeed"
        );
    });

    // A separate production RemoteHost changes Linux files; no GUI refresh call.
    let created = PathBuf::from(remote_join(&cwd, "agent-created.txt"));
    let renamed = PathBuf::from(remote_join(&cwd, "agent-renamed.txt"));
    host.write_file(&created, b"external write").unwrap();
    tick(&mut vcx);
    assert_eq!(
        names(&app, &mut vcx),
        ["agent-created.txt"],
        "the visible SFTP consumer must discover an external file without manual refresh"
    );

    let moved = fixture.remote_path("live-refresh-moved");
    host.rename(&directory, &moved).unwrap();
    tick(&mut vcx);
    assert_eq!(
        names(&app, &mut vcx),
        ["agent-created.txt"],
        "a failed background listing must keep the usable rows"
    );
    app.update_in(&mut vcx, |app, _, _| {
        assert!(!app.sftp_panel.loading);
        assert!(
            app.sftp_panel.error.is_none(),
            "background errors must not blank the browser"
        );
    });
    host.rename(&moved, &directory).unwrap();

    let draft = app.update_in(&mut vcx, |app, window, cx| {
        app.sftp_begin_new_file(window, cx);
        let Some(SftpEdit::NewFile(input)) = &app.sftp_panel.editing else {
            panic!("new-file editor must be open")
        };
        let input = input.clone();
        input.update(cx, |input, cx| {
            input.set_value("unfinished-name", window, cx)
        });
        input
    });
    host.write_file(&created, b"updated while inline editing")
        .unwrap();
    tick(&mut vcx);
    app.update_in(&mut vcx, |app, _, cx| {
        assert!(
            matches!(&app.sftp_panel.editing, Some(SftpEdit::NewFile(input)) if input == &draft)
        );
        assert_eq!(draft.read(cx).value().as_ref(), "unfinished-name");
        assert!(
            !app.sftp_panel.loading,
            "quiet refresh must never flash navigation loading"
        );
    });
    app.update_in(&mut vcx, |app, window, cx| app.sftp_cancel_edit(window, cx));
    tick(&mut vcx);
    app.update_in(&mut vcx, |app, _, _| {
        assert_eq!(app.sftp_panel.entries[0].size, 28)
    });

    let path_input = app.update_in(&mut vcx, |app, window, cx| {
        app.sftp_begin_edit_path(window, cx);
        let input = app.sftp_panel.editing_path.clone().unwrap();
        input.update(cx, |input, cx| {
            input.set_value("/unfinished/path", window, cx)
        });
        input
    });
    tick(&mut vcx);
    app.update_in(&mut vcx, |app, _, cx| {
        assert_eq!(app.sftp_panel.editing_path.as_ref(), Some(&path_input));
        assert_eq!(path_input.read(cx).value().as_ref(), "/unfinished/path");
        app.sftp_cancel_edit_path(cx);
    });
    host.rename(&created, &renamed).unwrap();
    tick(&mut vcx);
    assert_eq!(names(&app, &mut vcx), ["agent-renamed.txt"]);
    host.remove(&renamed, false).unwrap();
    let child = PathBuf::from(remote_join(&cwd, "agent-directory"));
    host.create_dir(&child, false).unwrap();
    tick(&mut vcx);
    assert_eq!(names(&app, &mut vcx), ["agent-directory"]);
    host.remove(&child, false).unwrap();
    tick(&mut vcx);
    assert!(names(&app, &mut vcx).is_empty());
    app.update_in(&mut vcx, |app, _, cx| app.sftp_close_browser(cx));
    drop(pane);
    drop(fixture);
}
