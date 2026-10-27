use super::super::test_support::socket_pair as tcp_pair;
use super::*;

/// What the daemon replays into a pane restored from a stored screen, in
/// the frames and the order it sends them: the dead pane's screen, then the
/// preamble, then the new shell's own first output.
fn replay_restore_into(daemon: &mut std::net::TcpStream, old_screen: &[u8], shell: &[u8]) {
    let size = WinSize {
        cols: 40,
        rows: 10,
        cell_w: 8,
        cell_h: 17,
    };
    DaemonMsg::Size(size).encode(daemon).unwrap();
    DaemonMsg::Snapshot(old_screen.to_vec())
        .encode(daemon)
        .unwrap();
    DaemonMsg::Size(size).encode(daemon).unwrap();
    DaemonMsg::Snapshot(crate::daemon::pane::restore_preamble(Some(
        "the shell below is new",
    )))
    .encode(daemon)
    .unwrap();
    DaemonMsg::Output(shell.to_vec()).encode(daemon).unwrap();
}

fn row(term: &RemoteTerminal, line: i32) -> String {
    use rio_vt::crosswords::pos::{Column, Line, Pos};
    let term = term.term.lock();
    term.bounds_to_string(
        Pos::new(Line(line), Column(0)),
        Pos::new(Line(line), Column(term.columns() - 1)),
    )
    .trim_end()
    .to_string()
}

/// A restored screen must not be sitting in the viewport when the new
/// shell's ConPTY starts drawing on it.
///
/// conhost addresses its own screen buffer absolutely — PSReadLine repaints
/// the line being typed with `ESC[6;20H` and conhost frames it the same way
/// — and that buffer starts blank with the cursor at the top-left. Restored
/// output is output conhost never produced: left on screen it shifts every
/// row conhost names, so the first keystroke repaints the input line on top
/// of the old text instead of at the prompt. The restored screen belongs in
/// scrollback, where it survives without claiming a row.
#[test]
fn a_restored_screen_leaves_the_new_shell_the_viewport_conpty_thinks_it_has() {
    crate::core::config::pin_test_config_dir();
    let (client_side, mut daemon_side) = tcp_pair();
    let term = RemoteTerminal::from_stream(client_side, TermSize::new(40, 10)).unwrap();

    // Five lines of the dead pane, then the shell painting its prompt the
    // way conhost does: at row 1 of a buffer it believes is blank.
    replay_restore_into(
        &mut daemon_side,
        b"line one\r\nline two\r\nline three\r\nline four\r\nline five\r\n",
        b"\x1b[?25l\x1b[1;1HPS C:\\> \x1b[?25h",
    );

    let mut top = String::new();
    for _ in 0..400 {
        top = row(&term, 0);
        if top.starts_with("PS C:") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        top, "PS C:\\>",
        "the shell's prompt paints where conhost put it"
    );

    for line in 1..10 {
        assert_eq!(
            row(&term, line),
            "",
            "row {line} still holds restored output, so conhost and the client \
             disagree about which row is which: the next repaint of the input \
             line lands on the old screen instead of at the prompt"
        );
    }

    // Kept, not erased: `ESC[2J` on the primary screen scrolls the viewport
    // into history, so the screen the daemon restored is one scroll away.
    let depth = {
        use rio_vt::crosswords::grid::Dimensions as _;
        term.term.lock().grid.history_size() as i32
    };
    let history: Vec<String> = (-depth..0).map(|line| row(&term, line)).collect();
    for wanted in [
        "line one",
        "line two",
        "line three",
        "line four",
        "line five",
    ] {
        assert!(
            history.iter().any(|row| row == wanted),
            "{wanted:?} is not in the scrollback; the restored screen was erased \
             rather than scrolled away. History holds {history:?}"
        );
    }
}
