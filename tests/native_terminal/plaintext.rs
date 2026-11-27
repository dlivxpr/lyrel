use gpui_kit::{InputHandler as _, TestAppContext, VisualTestContext};
use lyrel_core::daemon::protocol::WinSize;
use lyrel_terminal::lifecycle::declare_displayed;
use lyrel_terminal::remote::PtySource;
use lyrel_terminal::size::TermSize;
use lyrel_terminal::test_support::{
    graphics_assert_production_paint_at, graphics_build_rooted_consumer_at,
    graphics_recovery_destroy, graphics_recovery_wait, reading_painted_rows,
};
use lyrel_terminal::view::input::TerminalInputHandler;
use lyrel_test_support::graphics_fixture::{GraphicsDaemonProcess, graphics_fixture_shell};
use lyrel_test_support::plaintext_pty::verify_output;

pub(super) fn plaintext_echo_and_hidden_return(cx: &mut TestAppContext) {
    plaintext_echo(cx, true);
}

pub(super) fn plaintext_echo_while_visible(cx: &mut TestAppContext) {
    plaintext_echo(cx, false);
}

fn plaintext_echo(cx: &mut TestAppContext, hide: bool) {
    cx.executor().allow_parking();
    cx.update(lyrel::init_native_terminal_test);
    let daemon = GraphicsDaemonProcess::start();
    let mut shell = graphics_fixture_shell("plaintext");
    shell
        .args
        .push(daemon.dir.path().to_string_lossy().into_owned());
    // Faster than the formal 400 lines/s fixture while preserving the corpus.
    shell.args.push("6400".into());
    let size = WinSize {
        cols: 96,
        rows: 24,
        cell_w: 10,
        cell_h: 20,
    };
    let session = daemon
        .panes()
        .spawn(
            Some(daemon.dir.path().to_path_buf()),
            size,
            Some(shell),
            None,
            None,
        )
        .unwrap();
    let pane = session.pane_id();
    session.detach().unwrap();
    let source = if cfg!(windows) {
        PtySource::LocalConpty
    } else {
        PtySource::Raw
    };
    let grid = TermSize::new(96, 24);
    let (view, window) = graphics_build_rooted_consumer_at(
        cx,
        pane,
        daemon.stream_with_capacity_at(pane, 64 << 20, 128 << 20, size),
        Vec::new(),
        grid,
        source,
    );
    graphics_assert_production_paint_at(window, &view, cx, grid);
    graphics_recovery_wait(cx, "visible PTY calibration", |cx| {
        reading_painted_rows(&view, cx)
            .iter()
            .any(|row| row.trim() == "PLAIN_READY")
    });
    window
        .update(cx, |_, window, cx| {
            window.activate_window();
            let focus = view.read(cx).focus_handle.clone();
            focus.focus(window, cx);
        })
        .unwrap();
    VisualTestContext::from_window(window, cx).simulate_keystrokes("a");
    graphics_recovery_wait(cx, "first ordinary key reaches the displayed pane", |cx| {
        reading_painted_rows(&view, cx)
            .iter()
            .any(|row| row.trim() == ">aaab>a>")
    });

    let trigger = daemon.dir.path().join("start");
    if !hide {
        std::fs::write(&trigger, b"start\n").unwrap();
        graphics_recovery_wait(cx, "numbered workload reaches the visible surface", |cx| {
            reading_painted_rows(&view, cx)
                .iter()
                .any(|row| row.starts_with("PLAIN "))
        });
    }

    // Exercise the platform input-handler seam with an uncommitted preedit and
    // a committed Unicode string. The manual run goes through the real OS IME.
    window
        .update(cx, |_, window, cx| {
            let mut ime = TerminalInputHandler::new(view.clone(), None);
            ime.replace_and_mark_text_in_range(None, "ceshi", None, window, cx);
            assert_eq!(ime.marked_text_range(window, cx), Some(0..5));
            ime.unmark_text(window, cx);
            ime.replace_and_mark_text_in_range(None, "zhong", None, window, cx);
            ime.replace_text_in_range(None, "中文", window, cx);
            assert_eq!(ime.marked_text_range(window, cx), None);
        })
        .unwrap();
    VisualTestContext::from_window(window, cx).simulate_keystrokes("b");
    graphics_recovery_wait(cx, "commit and second key are visible exactly once", |cx| {
        let rows = reading_painted_rows(&view, cx);
        ["IME 中", "IME 文", ">aaac>b>"]
            .iter()
            .all(|expected| rows.iter().filter(|row| row.as_str() == *expected).count() == 1)
    });

    if hide {
        // Use the same visibility gate as a workbench tab switch.
        window
            .update(cx, |_, window, cx| {
                declare_displayed(cx, [(view.entity_id(), false)]);
                window.replace_root(cx, |_, _| gpui_kit::EmptyView);
            })
            .unwrap();
        std::fs::write(trigger, b"start\n").unwrap();
    }
    graphics_recovery_wait(cx, "PTY producer finishes its final line", |cx| {
        view.read_with(cx, |view, _| {
            view.scrollback_text()
                .lines()
                .any(|line| line.trim() == "PLAIN_DONE")
        })
    });
    if hide {
        window
            .update(cx, |_, window, cx| {
                declare_displayed(cx, [(view.entity_id(), true)]);
                window.replace_root(cx, |window, cx| {
                    gpui_kit::component::Root::new(view.clone(), window, cx)
                });
                let focus = view.read(cx).focus_handle.clone();
                focus.focus(window, cx);
            })
            .unwrap();
    }
    graphics_assert_production_paint_at(window, &view, cx, grid);
    let rows = reading_painted_rows(&view, cx);
    assert!(rows.iter().any(|row| row.starts_with("PLAIN 9600 ")));
    assert!(rows.iter().any(|row| row.trim() == "PLAIN_DONE"));
    VisualTestContext::from_window(window, cx).simulate_keystrokes("c");
    graphics_recovery_wait(cx, "a key reaches the pane after the workload", |cx| {
        reading_painted_rows(&view, cx)
            .iter()
            .any(|row| row.trim() == ">aaad>c>")
    });
    let output = view.read_with(cx, |view, _| view.scrollback_text());
    verify_output(&output, true, "abc", "中文").unwrap();
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}
