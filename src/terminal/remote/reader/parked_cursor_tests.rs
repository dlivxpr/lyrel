use super::super::test_support::socket_pair;
use super::*;
use std::io::Write as _;

fn terminal_on(pty: PtySource, size: TermSize) -> (RemoteTerminal, Stream) {
    crate::core::config::pin_test_config_dir();
    let (client_side, daemon_side) = socket_pair();
    let term = RemoteTerminal::from_stream_with(client_side, size, Vec::new(), pty)
        .expect("a terminal over a socket pair");
    assert_eq!(term.is_local_conpty(), pty == PtySource::LocalConpty);
    (term, daemon_side)
}

/// Feeds one conhost-shaped repaint and reports the cell the cursor ends on,
/// waiting for the `X` the frame paints so the reader is known to be done.
fn cursor_after_conpty_frame(pty: PtySource, frame: &[u8]) -> (i32, usize) {
    let (term, mut daemon_side) = terminal_on(pty, TermSize::new(80, 24));

    // Where the TUI put the cursor before conhost repainted over it.
    DaemonMsg::Output(b"\x1b[6;4H".to_vec())
        .encode(&mut daemon_side)
        .unwrap();
    DaemonMsg::Output(frame.to_vec())
        .encode(&mut daemon_side)
        .unwrap();
    daemon_side.flush().unwrap();

    for _ in 0..600 {
        {
            let t = term.term.lock();
            let painted = t.grid[rio_vt::crosswords::pos::Line(19)]
                [rio_vt::crosswords::pos::Column(1)]
            .c();
            if painted == 'X' {
                let point = term.paint_cursor_position().unwrap_or(t.grid.cursor.pos);
                return (point.row.0, point.col.0);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the reader never applied the frame");
}

#[test]
fn streaming_lines_keep_the_previous_frame_before_the_final_repaint() {
    use rio_vt::crosswords::pos::{Column, Line};

    for pty in [PtySource::Raw, PtySource::LocalConpty] {
        let (term, mut daemon) = terminal_on(pty, TermSize::new(40, 10));
        for frame in [
            b"\x1b[2;1H\x1b[?25l\x1b[2;1Hthinking first\r\nthinking second\x1b[?25h".as_slice(),
            b"\r\nthinking third".as_slice(),
        ] {
            DaemonMsg::Output(frame.to_vec())
                .encode(&mut daemon)
                .unwrap();
            daemon.flush().unwrap();
            let expected = if frame.ends_with(b"third") { 't' } else { 's' };
            let row = if expected == 't' { 3 } else { 2 };
            for _ in 0..200 {
                if term.term.lock().grid[Line(row)][Column(9)].c() == expected {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        let grid = term.term.lock();
        let row = |line| {
            (0..40)
                .map(|col| match grid.grid[Line(line)][Column(col)].c() {
                    '\0' => ' ',
                    ch => ch,
                })
                .collect::<String>()
                .trim_end()
                .to_string()
        };
        assert_eq!(row(1), "thinking first", "{pty:?}");
        assert_eq!(
            row(2),
            "thinking second",
            "streaming overwrote a previous row: {pty:?}"
        );
        assert_eq!(row(3), "thinking third", "{pty:?}");
    }
}

#[test]
fn a_local_route_is_a_conpty_only_where_conhost_exists() {
    assert_eq!(
        PtySource::for_route(&PaneRoute::Local),
        if cfg!(windows) {
            PtySource::LocalConpty
        } else {
            PtySource::Raw
        },
    );
}

#[test]
fn a_routed_pane_reads_a_raw_pty_whatever_the_client_was_built_for() {
    let spec: NativeSshSpec = serde_json::from_str(
        r#"{"host":"linux-box","port":22,"user":"dev","auth_mode":"auto"}"#,
    )
    .unwrap();
    // A remote workspace only ever installs a `lyrel-server` on Linux or
    // macOS (`install::asset::asset_for_uname`), and a WSL workspace is a
    // Linux server too, so a routed pane's pty is a raw one — the far end
    // is never a conhost, whoever is dialling it.
    for header in [
        crate::daemon::router::RouteHeader::ssh(spec),
        crate::daemon::router::RouteHeader::wsl("Ubuntu-22.04"),
    ] {
        let route = PaneRoute::Remote {
            header: Box::new(header),
            resize_echo: false,
        };
        assert_eq!(PtySource::for_route(&route), PtySource::Raw);
    }
}

#[test]
fn a_conpty_frame_that_shows_the_cursor_over_an_erase_keeps_the_cell_it_hid_on() {
    assert_eq!(
        cursor_after_conpty_frame(
            PtySource::LocalConpty,
            b"\x1b[?25l\x1b[20;2HX\x1b[K\x1b[m\x1b[22;42H\x1b[K\x1b[?25h",
        ),
        (5, 3),
        "conhost parked the cursor on the cell it erased last; the cursor \
         belongs where it was when the repaint hid it"
    );
}

#[test]
fn the_same_frame_off_a_raw_pty_leaves_the_cursor_where_the_frame_left_it() {
    assert_eq!(
        cursor_after_conpty_frame(
            PtySource::Raw,
            b"\x1b[?25l\x1b[20;2HX\x1b[K\x1b[m\x1b[22;42H\x1b[K\x1b[?25h",
        ),
        (21, 41),
        "with no conhost in between the stream is the application's own, \
         and the cell it left the cursor on is the cell it meant"
    );
}

#[test]
fn a_conpty_frame_that_moves_the_cursor_before_showing_it_is_obeyed() {
    assert_eq!(
        cursor_after_conpty_frame(
            PtySource::LocalConpty,
            b"\x1b[?25l\x1b[20;2HX\x1b[K\x1b[m\x1b[22;42H\x1b[K\x1b[9;9H\x1b[?25h"
        ),
        (8, 8),
        "the frame painted the cursor somewhere on purpose"
    );
}

/// Vim opens its command line with exactly the shape the parked-cursor
/// scanner calls parked — hide, move around to paint, end on the `:` it
/// wrote — and then echoes every following keystroke as a bare byte at
/// wherever that left the cursor. Putting the cursor back on a raw pty
/// therefore does not straighten out a stray caret, it drops `wq!` onto the
/// row vim was editing. Bytes below are a capture of vim 9 on a 20x11 pty.
#[test]
fn a_raw_pty_repaint_keeps_the_cursor_the_frame_left_so_the_echo_lands_on_it() {
    let (term, mut daemon_side) = terminal_on(PtySource::Raw, TermSize::new(20, 11));

    let mut stream: Vec<u8> = Vec::new();
    // `vim test.md`: the alternate screen, the file, cursor home.
    stream.extend_from_slice(b"\x1b[?1049h\x1b[H\x1b[2J\x1b[1;1H123456789\x1b[1;1H");
    // Esc, then `:` — two bracketed repaints, the second ending on the `:`
    // vim wrote at the head of the command line.
    stream.extend_from_slice(b"\x1b[?25l\x1b[m\x1b[11;10H^[\x1b[1;1H\x1b[?25h");
    stream.extend_from_slice(b"\x1b[?25l\x1b[11;10H  \x1b[1;1H\x07\x1b[?25h");
    stream.extend_from_slice(
        b"\x1b[?25l\x1b[11;10H:\x1b[1;1H\x1b[11;1H\x1b[K\x1b[11;1H:\x1b[?25h",
    );
    // `w`, `q`, `!`: vim echoes them with no positioning of their own.
    stream.extend_from_slice(b"wq!");
    DaemonMsg::Output(stream).encode(&mut daemon_side).unwrap();
    daemon_side.flush().unwrap();

    let row = |t: &Crosswords<EventProxy>, line: i32| -> String {
        (0..20)
            .flat_map(|col| {
                t.grid.cell_text(rio_vt::crosswords::pos::Pos::new(
                    rio_vt::crosswords::pos::Line(line),
                    rio_vt::crosswords::pos::Column(col),
                ))
            })
            // A cell nothing was written into reads NUL out of the packed
            // cell; for the text a row *shows* that is a blank.
            .map(|ch| if ch == '\0' { ' ' } else { ch })
            .collect::<String>()
            .trim_end()
            .to_string()
    };

    // The whole batch is applied under one lock, so the `:` landing on the
    // command line means every byte after it landed too.
    let mut command_line = String::new();
    let mut edited = String::new();
    for _ in 0..600 {
        {
            let t = term.term.lock();
            command_line = row(&t, 10);
            edited = row(&t, 0);
        }
        if command_line.starts_with(':') {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        command_line, ":wq!",
        "the keystrokes belong after the `:` the repaint ended on"
    );
    assert_eq!(
        edited, "123456789",
        "and nothing of them belongs on the row vim was editing"
    );
}

/// The route cannot tell a native-SSH pane from a local shell: both are
/// spawned through this machine's daemon. What tells them apart is the
/// `RemoteContext` the daemon sends — and it has to, because a window
/// reopening onto an existing native-SSH pane attaches by id and has
/// nothing else to go on.
#[test]
fn a_native_ssh_context_turns_the_repair_off_mid_stream() {
    let (term, mut daemon_side) = terminal_on(PtySource::LocalConpty, TermSize::new(80, 24));

    DaemonMsg::RemoteContext(Some(RemoteContext {
        kind: RemoteKind::NativeSsh,
        argv: Vec::new(),
        target: "dev@linux-box".into(),
    }))
    .encode(&mut daemon_side)
    .unwrap();
    daemon_side.flush().unwrap();
    for _ in 0..600 {
        if term.remote_context().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        term.remote_context().is_some(),
        "the reader never applied the context"
    );
    assert!(!term.is_local_conpty());

    DaemonMsg::Output(b"\x1b[6;4H".to_vec())
        .encode(&mut daemon_side)
        .unwrap();
    DaemonMsg::Output(b"\x1b[?25l\x1b[20;2HX\x1b[K\x1b[m\x1b[22;42H\x1b[K\x1b[?25h".to_vec())
        .encode(&mut daemon_side)
        .unwrap();
    daemon_side.flush().unwrap();

    let mut cursor = (0, 0);
    for _ in 0..600 {
        {
            let t = term.term.lock();
            let painted = t.grid[rio_vt::crosswords::pos::Line(19)]
                [rio_vt::crosswords::pos::Column(1)]
            .c();
            if painted == 'X' {
                let point = t.grid.cursor.pos;
                cursor = (point.row.0, point.col.0);
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        cursor,
        (21, 41),
        "an ssh channel's bytes are the far host's own; nothing parked that cursor"
    );
}
