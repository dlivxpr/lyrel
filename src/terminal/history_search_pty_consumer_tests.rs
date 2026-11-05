// Real daemon/ConPTY execution; the Mac key policy is modeled, not native macOS.
#[cfg(windows)]
#[gpui_kit::test]
fn reverse_search_cmd_enter_runs_selected_command_in_real_pty(cx: &mut TestAppContext) {
    use crate::core::config::KeybindingOverride;
    use crate::daemon::protocol::{ShellSpec, WinSize};
    use crate::ui::pane::{Pane, PaneSlot};
    use gpui_kit::{OperatingSystem, PlatformInfo, PlatformTarget};
    use std::time::{Duration, Instant};

    crate::core::config::pin_test_config_dir();
    cx.set_platform_info(PlatformInfo {
        os: OperatingSystem::Mac,
        target: PlatformTarget::Native,
    });
    let daemon = GraphicsDaemonProcess::start();
    let count_path = daemon.dir.path().join("executions.txt");
    let cmd = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
        .join("System32")
        .join("cmd.exe");
    // /D disables the user's AutoRun; cmd.exe has no PSReadLine history file.
    // The prompt itself emits the normal OSC 133 ownership boundaries.
    let shell = ShellSpec {
        program: cmd.to_string_lossy().into_owned(),
        args: vec![
            "/D".into(),
            "/Q".into(),
            "/K".into(),
            "prompt $E]133;A$E\\$E]133;B$E\\".into(),
        ],
        args_are_lyrel_defaults: false,
    };
    let session = daemon
        .panes()
        .spawn(
            Some(daemon.dir.path().to_path_buf()),
            WinSize {
                cols: 80,
                rows: 24,
                cell_w: 10,
                cell_h: 20,
            },
            Some(shell),
            None,
            None,
        )
        .unwrap();
    let id = session.pane_id();
    session.detach().unwrap();
    let terminal = RemoteTerminal::from_stream_with(
        daemon.stream(id),
        TermSize::new(80, 24),
        Vec::new(),
        PtySource::LocalConpty,
    )
    .unwrap();
    let selected = format!(
        "echo 180_SELECTED>>\"{}\" & echo 180_EXECUTED",
        count_path.display()
    );
    let (app, mut vcx, _quiet_stream) = crate::ui::app::test_window::harness_with_pane(cx);
    vcx.update(|_, cx| {
        cx.global_mut::<Config>().keybindings.insert(
            "ToggleFullscreen".into(),
            KeybindingOverride::Exact(vec!["cmd-enter".into()]),
        );
        crate::ui::keymap::rebind(cx);
    });
    let view = app.update_in(&mut vcx, |app, window, cx| {
        let view = cx.new(|cx| TerminalView::with_terminal(terminal, id, window, cx));
        view.update(cx, |view, cx| {
            view.history = TerminalHistory::from_loaded(test_support::history_fixture(vec![
                selected.clone(),
                "echo 180_UNSELECTED".into(),
            ]));
            window.focus(&view.focus_handle, cx);
        });
        app.tabs[0].pane = Pane::leaf(PaneSlot::Ready(view.clone()));
        cx.notify();
        view
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        vcx.run_until_parked();
        if view.read_with(&vcx, |view, _| view.input_active()) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "real cmd prompt never engaged: {}",
            view.read_with(&vcx, |view, _| view.scrollback_text())
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    vcx.simulate_keystrokes("ctrl-r");
    vcx.simulate_input("180_SELECTED");
    vcx.run_until_parked();
    view.read_with(&vcx, |view, _| {
        assert_eq!(
            view.history
                .search()
                .unwrap()
                .selected_line(view.history.entries()),
            Some(selected.as_str())
        );
    });
    vcx.simulate_keystrokes("cmd-enter");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        vcx.run_until_parked();
        let output = view.read_with(&vcx, |view, _| view.scrollback_text());
        let record = std::fs::read_to_string(&count_path).unwrap_or_default();
        if record.trim_end() == "180_SELECTED"
            && record.lines().count() == 1
            && output.lines().any(|line| line.trim() == "180_EXECUTED")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "selected command did not execute exactly once: record={record:?}; output={output}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    vcx.update(|window, _| assert!(!window.is_fullscreen()));
    let record = std::fs::read_to_string(&count_path).unwrap();
    assert_eq!(record.trim_end(), "180_SELECTED");
    assert_eq!(record.lines().count(), 1);
    eprintln!("real ConPTY emitted 180_EXECUTED; execution record={record:?}");
    daemon.panes().kill(id).unwrap();
}
