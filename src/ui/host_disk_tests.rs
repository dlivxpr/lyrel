//! Consumer regressions for the disk tile through a real window and a real
//! Host server (#376): the tile shows the latest completed scan of the
//! context pane's exact directory, rescans only when that pair changes, and
//! marks failed scans without pretending they were empty.
use super::*;
use lyrel_core::host::Host as _;
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};

use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::host_metrics::tests::HostServer;
use crate::ui::local_link::LocalLink;

fn settle(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.render_frame(cx));
    vcx.run_until_parked();
}

fn open_monitor(app: &gpui_kit::Entity<LyrelApp>, vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.set_right_panel_tab(RightPanelTab::Monitor, window, cx);
        });
    });
    settle(vcx);
}

/// The pane's directory, planted the way a live shell report would arrive.
fn plant_cwd(app: &gpui_kit::Entity<LyrelApp>, vcx: &mut VisualTestContext, cwd: &std::path::Path) {
    vcx.update(|window, cx| {
        let pane = app
            .read(cx)
            .context_pane(window, cx)
            .expect("a context pane");
        pane.update(cx, |view, _| {
            view.set_git_status_cwd_for_test(Some(cwd.to_path_buf()));
            view.terminal.seed_cwd(Some(cwd.to_path_buf()));
        });
    });
    settle(vcx);
}

fn disk_value(vcx: &mut VisualTestContext) -> Option<String> {
    vcx.update(|window, _| {
        window
            .try_find("monitor-tile-disk-value")
            .and_then(|value| value.label().map(str::to_owned))
    })
}

fn disk_dir(vcx: &mut VisualTestContext) -> Option<String> {
    vcx.update(|window, _| {
        window
            .try_find("monitor-tile-disk-dir")
            .and_then(|dir| dir.label().map(str::to_owned))
    })
}

fn disk_detail(vcx: &mut VisualTestContext) -> Option<String> {
    vcx.update(|window, _| {
        window
            .try_find("monitor-tile-disk")
            .and_then(|tile| tile.label().map(str::to_owned))
    })
}

fn wait_value<F>(vcx: &mut VisualTestContext, mut wanted: F, what: &str)
where
    F: FnMut(&Option<String>) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        vcx.executor().advance_clock(Duration::from_secs(1));
        settle(vcx);
        let value = disk_value(vcx);
        if wanted(&value) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "disk tile value did not become {what} (last: {value:?})"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One sparse file of `bytes` apparent size: the walk reads real metadata,
/// so the size is honest without spending disk on the fixture.
fn sized_file(dir: &std::path::Path, name: &str, bytes: u64) {
    let file = std::fs::File::create(dir.join(name)).unwrap();
    file.set_len(bytes).unwrap();
    file.sync_all().unwrap();
}

fn scanned_at(app: &gpui_kit::Entity<LyrelApp>, vcx: &mut VisualTestContext) -> Option<u64> {
    vcx.update(|_, cx| {
        let view = app.read(cx).disk_view.as_ref()?;
        match &view.read(cx).state {
            DiskViewState::Ready(usage) => match usage.state() {
                WireDiskState::Scanned(scan) => Some(scan.scanned_at_millis()),
                WireDiskState::Unavailable(_) => None,
            },
            _ => None,
        }
    })
}

/// The approved reading is `4.6 GB / 512 GB`; without a volume total the
/// unknown side is a bare `—`, never `— GB`. Partial scans keep the `≥`.
#[test]
fn disk_value_puts_a_unit_on_each_side_and_never_on_a_missing_total() {
    let readout = |size: u64, total: Option<u64>, partial: bool| DiskReadout {
        size_bytes: Some(size),
        total_bytes: total,
        partial,
        directory: None,
        marker: None,
        status: None,
        detail: String::new(),
    };
    let gib = 1_073_741_824_u64;
    let size = (4.6_f64 * gib as f64) as u64;
    let total = 512 * gib;
    assert_eq!(
        readout(size, Some(total), false).value_text(),
        "4.6 GB / 512 GB"
    );
    assert_eq!(readout(size, None, false).value_text(), "4.6 GB / —");
    assert_eq!(
        readout(size, Some(total), true).value_text(),
        "≥ 4.6 GB / 512 GB"
    );
}

#[gpui_kit::test]
fn disk_tile_shows_the_completed_scan_of_the_context_directory(cx: &mut TestAppContext) {
    let server = HostServer::new();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|window, cx| {
        crate::ui::windows::WindowRegistry::init(cx);
        crate::ui::windows::WindowRegistry::register(
            cx,
            app.read(cx).workspace,
            window.window_handle(),
            app.downgrade(),
        );
        LocalLink::install_client_for_test(cx, server.client.clone());
    });

    let temp = tempfile::tempdir().unwrap();
    sized_file(temp.path(), "payload.bin", 600_000_000);
    plant_cwd(&app, &mut vcx, temp.path());
    open_monitor(&app, &mut vcx);

    // The tile renders in the approved layout and fills with the host's own
    // completed scan: the real walk saw the 600 MB file, and the volume's
    // real total rides beside it.
    wait_value(
        &mut vcx,
        |v| v.as_deref().is_some_and(|s| s.starts_with("0.6")),
        "0.6 GB",
    );
    let value = disk_value(&mut vcx).unwrap();
    assert!(
        value.starts_with("0.6 GB / ") && value.ends_with(" GB"),
        "each side carries its own unit: directory size then volume total: {value}"
    );
    let shown = disk_dir(&mut vcx).unwrap_or_default();
    let leaf = temp.path().file_name().unwrap().to_string_lossy();
    assert!(
        shown.contains(leaf.as_ref()),
        "the tile names the scanned directory: {shown}"
    );
    let detail = disk_detail(&mut vcx).unwrap_or_default();
    assert!(
        detail.contains(leaf.as_ref()) && detail.contains("0.6"),
        "the hover detail carries the directory and the size: {detail}"
    );
}

#[gpui_kit::test]
fn disk_scan_recovers_for_the_same_directory_after_connection_loss(cx: &mut TestAppContext) {
    let first = HostServer::new();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| LocalLink::install_client_for_test(cx, first.client.clone()));
    let dir = tempfile::tempdir().unwrap();
    plant_cwd(&app, &mut vcx, dir.path());
    open_monitor(&app, &mut vcx);
    wait_value(
        &mut vcx,
        |v| v.as_deref().is_some_and(|s| s.starts_with("0 /")),
        "empty directory",
    );

    drop(first);
    wait_value(&mut vcx, |v| v.as_deref() == Some("—"), "disconnected");
    sized_file(dir.path(), "new.bin", 600_000_000);
    let second = HostServer::new();
    vcx.update(|_, cx| LocalLink::install_client_for_test(cx, second.client.clone()));
    wait_value(
        &mut vcx,
        |v| v.as_deref().is_some_and(|s| s.starts_with("0.6 /")),
        "reconnected directory",
    );
}

#[gpui_kit::test]
fn a_directory_change_rescans_and_never_polls_on_the_metrics_cadence(cx: &mut TestAppContext) {
    let server = HostServer::new();
    // Two tabs, one pane each: switching the active tab moves the context
    // directory the way a real `cd` would. (`seed_cwd` only seeds an empty
    // terminal, so one pane cannot be re-seeded; two panes need no re-seed.)
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    vcx.update(|window, cx| {
        crate::ui::windows::WindowRegistry::init(cx);
        crate::ui::windows::WindowRegistry::register(
            cx,
            app.read(cx).workspace,
            window.window_handle(),
            app.downgrade(),
        );
        LocalLink::install_client_for_test(cx, server.client.clone());
    });

    let select_tab =
        |app: &gpui_kit::Entity<LyrelApp>, vcx: &mut VisualTestContext, index: usize| {
            vcx.update(|_, cx| app.update(cx, |app, _| app.tabs.select(index)));
            settle(vcx);
        };

    let first = tempfile::tempdir().unwrap();
    sized_file(first.path(), "one.bin", 600_000_000);
    let second = tempfile::tempdir().unwrap();
    sized_file(second.path(), "two.bin", 800_000_000);

    select_tab(&app, &mut vcx, 0);
    plant_cwd(&app, &mut vcx, first.path());
    select_tab(&app, &mut vcx, 1);
    plant_cwd(&app, &mut vcx, second.path());
    select_tab(&app, &mut vcx, 0);
    open_monitor(&app, &mut vcx);
    wait_value(
        &mut vcx,
        |v| v.as_deref().is_some_and(|s| s.starts_with("0.6")),
        "first dir",
    );
    let first_stamp = scanned_at(&app, &mut vcx);
    assert!(first_stamp.is_some(), "the first scan completed");

    // Only one scan view exists for the one pair on screen.
    assert_eq!(
        vcx.update(|_, cx| cx.default_global::<DiskViews>().0.len()),
        1,
        "one (host, directory) pair, one scan view"
    );

    // The metrics cadence must not re-scan: several seconds of clock advance
    // with the directory unchanged leaves the completed stamp untouched.
    for _ in 0..5 {
        vcx.executor().advance_clock(Duration::from_secs(1));
        settle(&mut vcx);
    }
    assert_eq!(
        scanned_at(&app, &mut vcx),
        first_stamp,
        "sampling cadence must not re-scan an unchanged directory"
    );

    // A directory change starts exactly one fresh scan for the new pair, and
    // the stale pair's answer is never shown against the new directory.
    select_tab(&app, &mut vcx, 1);
    wait_value(
        &mut vcx,
        |v| v.as_deref().is_some_and(|s| s.starts_with("0.7")),
        "second dir",
    );
    let second_stamp = scanned_at(&app, &mut vcx);
    assert!(
        second_stamp.is_some_and(|at| at >= first_stamp.unwrap_or(0)),
        "the new scan is a fresh completion"
    );
    let shown = disk_dir(&mut vcx).unwrap_or_default();
    let leaf = second.path().file_name().unwrap().to_string_lossy();
    assert!(
        shown.contains(leaf.as_ref()),
        "the tile now names the new directory: {shown}"
    );
}

#[gpui_kit::test]
fn a_failed_scan_is_marked_and_a_missing_directory_never_reads_empty(cx: &mut TestAppContext) {
    let server = HostServer::new();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|window, cx| {
        crate::ui::windows::WindowRegistry::init(cx);
        crate::ui::windows::WindowRegistry::register(
            cx,
            app.read(cx).workspace,
            window.window_handle(),
            app.downgrade(),
        );
        LocalLink::install_client_for_test(cx, server.client.clone());
    });

    // A directory that does not exist: the host's scan fails honestly.
    let gone = std::env::temp_dir().join("lyrel-disk-test-definitely-gone");
    let _ = std::fs::remove_dir_all(&gone);
    plant_cwd(&app, &mut vcx, &gone);
    open_monitor(&app, &mut vcx);

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        vcx.executor().advance_clock(Duration::from_secs(1));
        settle(&mut vcx);
        if disk_value(&mut vcx).as_deref() == Some("—") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the failed scan never surfaced its marker"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let detail = disk_detail(&mut vcx).unwrap_or_default();
    assert!(
        detail.contains("Disk scan failed"),
        "the hover detail names the failure: {detail}"
    );
    assert!(
        disk_dir(&mut vcx).is_none(),
        "a failed scan must not present a directory as if one was scanned"
    );
}

#[gpui_kit::test]
fn without_a_directory_the_tile_waits_instead_of_guessing(cx: &mut TestAppContext) {
    let server = HostServer::new();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|window, cx| {
        crate::ui::windows::WindowRegistry::init(cx);
        crate::ui::windows::WindowRegistry::register(
            cx,
            app.read(cx).workspace,
            window.window_handle(),
            app.downgrade(),
        );
        LocalLink::install_client_for_test(cx, server.client.clone());
    });
    open_monitor(&app, &mut vcx);

    // No cwd is planted: the tile shows the pending marker, never a number.
    for _ in 0..3 {
        vcx.executor().advance_clock(Duration::from_secs(1));
        settle(&mut vcx);
    }
    vcx.update(|window, _| {
        assert!(window.find("monitor-tile-disk").visible());
    });
    assert_eq!(
        disk_value(&mut vcx).as_deref(),
        Some("…"),
        "with no directory there is nothing to scan yet"
    );
    assert_eq!(
        vcx.update(|_, cx| cx.default_global::<DiskViews>().0.len()),
        0,
        "no scan view is held without a directory"
    );
}

/// Managed-remote disk scan through the real transport: the remote Host
/// answers with the far machine's own walk — the seeded file's real byte
/// count, the remote volume's real total — and a dropped link reads as a
/// failure, never as an empty scan. Seeding runs against the fixture's
/// disposable remote home through the production SFTP Host.
#[gpui_kit::test]
#[ignore = "requires isolated pinned native SSH/SFTP, same-commit server, and explicit LYREL_TEST_SSH_* endpoint; run alone"]
fn native_ssh_disk_scan_reports_the_remote_trees_real_numbers(cx: &mut gpui_kit::TestAppContext) {
    let binary = std::path::PathBuf::from(
        std::env::var_os("LYREL_TEST_SSH_LOCAL_SERVER").expect("same-commit server"),
    );
    let fixture = crate::native_ssh_graphics_fixture::NativeSshFixture::start(&binary);
    lyrel_core::core::config::set_config_dir(fixture.local_config_dir());
    let host = fixture.connect_control();
    assert!(host.is_connected());
    let remote_home = std::path::PathBuf::from(fixture.remote_home());

    // One real file on the far machine, seeded through the production SFTP Host.
    let probe = host.join(&remote_home, "lyrel-disk-probe");
    host.create_dir(&probe, true).unwrap();
    host.write_file(&host.join(&probe, "payload.bin"), &[7u8; 4096])
        .unwrap();

    let usage = host.disk_usage(&probe).expect("the remote scan answers");
    assert_eq!(
        usage.instance(),
        host.peer().instance,
        "the answer is fenced by the answering daemon's instance"
    );
    assert_eq!(usage.directory(), probe.to_string_lossy());
    let scan = match usage.state() {
        WireDiskState::Scanned(scan) => scan.clone(),
        other => panic!("expected a completed remote scan, got {other:?}"),
    };
    assert_eq!(scan.directory(), probe.to_string_lossy());
    assert_eq!(
        scan.size_bytes(),
        4096,
        "the far machine's walk counted the seeded file's real bytes"
    );
    assert_eq!(scan.files_counted(), 1);
    assert!(
        scan.volume_total_bytes().is_some_and(|total| total > 0),
        "the remote volume's real total rides with the scan"
    );
    assert!(!scan.is_partial());

    // A missing directory fails honestly instead of reading as an empty scan.
    let missing = host.join(&remote_home, "lyrel-disk-probe-gone");
    let err = host.disk_usage(&missing).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);

    // A dropped link reads as a failure, never as an empty answer.
    let mut fixture = fixture;
    fixture.restart_router();
    let err = host.disk_usage(&probe).unwrap_err();
    assert!(
        matches!(
            err.kind(),
            std::io::ErrorKind::NotConnected
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::TimedOut
        ),
        "a dead link must not present a stale scan: {err}"
    );
    let _ = cx;
}
