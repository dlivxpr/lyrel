use super::*;
use crate::daemon::protocol::DaemonMsg;
use crate::daemon::transport::Stream;

use super::super::test_support::socket_pair;
#[test]
fn a_retired_title_still_lets_the_same_title_through_next_run() {
    crate::core::config::pin_test_config_dir();
    let run = b"\x1b]133;C\x07\x1b]2;build\x07\x1b]133;D\x07";
    for synchronized in [false, true] {
        let (client_side, mut daemon) = socket_pair();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        let mut bytes = Vec::new();
        for _ in 0..2 {
            if synchronized {
                bytes.extend_from_slice(b"\x1b[?2026h");
            }
            bytes.extend_from_slice(run);
            if synchronized {
                bytes.extend_from_slice(b"\x1b[?2026l");
            }
        }
        bytes.extend_from_slice(b"\x1b]2;shell\x07");
        // One frame makes coalescing deterministic: both retirements must
        // precede the next title even within a single reader batch.
        DaemonMsg::Output(bytes).encode(&mut daemon).unwrap();

        let mut titles = Vec::new();
        for _ in 0..600 {
            while let Ok(event) = term.events.try_recv() {
                match event {
                    PaneEvent::Title(title) => titles.push(title),
                    PaneEvent::ResetTitle => titles.push(String::new()),
                    _ => {}
                }
            }
            if titles.last().is_some_and(|title| title == "shell") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            titles,
            ["build", "", "build", "", "shell"],
            "command titles retire in stream order; synchronized={synchronized}"
        );
    }
}

fn ws(cols: u16, rows: u16) -> WinSize {
    WinSize {
        cols,
        rows,
        cell_w: 8,
        cell_h: 17,
    }
}

/// Everything the grid holds, scrollback included, one row per line.
fn all_text(term: &RemoteTerminal) -> String {
    use rio_vt::crosswords::grid::Dimensions as _;
    use rio_vt::crosswords::pos::{Column, Line};
    let t = term.term.lock();
    let grid = &t.grid;
    let mut out = String::new();
    let top = -(grid.history_size() as i32);
    for line in top..grid.screen_lines() as i32 {
        let row = &grid[Line(line)];
        let mut text = String::new();
        for col in 0..grid.columns() {
            text.push(row[Column(col)].c());
        }
        let text = text.trim_end();
        if !text.is_empty() {
            out.push_str(text);
            out.push('\n');
        }
    }
    out
}

/// The visible screen alone.
fn screen_text(term: &RemoteTerminal) -> String {
    use rio_vt::crosswords::grid::Dimensions as _;
    use rio_vt::crosswords::pos::{Column, Line};
    let t = term.term.lock();
    let grid = &t.grid;
    let mut out = String::new();
    for line in 0..grid.screen_lines() as i32 {
        let row = &grid[Line(line)];
        let mut text = String::new();
        for col in 0..grid.columns() {
            text.push(row[Column(col)].c());
        }
        let text = text.trim_end();
        if !text.is_empty() {
            out.push_str(text);
            out.push('\n');
        }
    }
    out
}

/// The grid's width. A grid is born at the size its `RemoteTerminal` was
/// built with and only ever leaves it on a `DaemonMsg::Size`, so this is
/// what says which geometry frames were applied.
fn columns(term: &RemoteTerminal) -> usize {
    use rio_vt::crosswords::grid::Dimensions as _;
    term.term.lock().grid.columns()
}

/// Waits for the reader thread to have applied everything named, then
/// returns the whole grid either way so a failure can print what landed.
fn settled(term: &RemoteTerminal, needles: &[&str]) -> String {
    for _ in 0..600 {
        let text = all_text(term);
        if needles.iter().all(|n| text.contains(n)) {
            return text;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    all_text(term)
}

/// The shape a re-attach takes: the client's grid is born at the hardcoded
/// attach size, and the daemon replays every ring segment at the geometry
/// it was recorded at.
#[test]
fn every_replayed_segment_reaches_the_grid() {
    crate::core::config::pin_test_config_dir();
    let (client_side, mut daemon) = socket_pair();
    let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

    DaemonMsg::Size(ws(80, 24)).encode(&mut daemon).unwrap();
    DaemonMsg::Snapshot(b"BIRTH-BANNER\r\n".to_vec())
        .encode(&mut daemon)
        .unwrap();
    DaemonMsg::Size(ws(120, 40)).encode(&mut daemon).unwrap();
    DaemonMsg::Snapshot(b"SECOND-SEGMENT\r\n".to_vec())
        .encode(&mut daemon)
        .unwrap();
    DaemonMsg::Size(ws(120, 40)).encode(&mut daemon).unwrap();
    DaemonMsg::Snapshot(b"THIRD-SEGMENT\r\n".to_vec())
        .encode(&mut daemon)
        .unwrap();

    let text = settled(&term, &["BIRTH-BANNER", "SECOND-SEGMENT", "THIRD-SEGMENT"]);
    assert!(text.contains("BIRTH-BANNER"), "grid held:\n{text}");
    assert!(text.contains("SECOND-SEGMENT"), "grid held:\n{text}");
    assert!(text.contains("THIRD-SEGMENT"), "grid held:\n{text}");
    // Each segment's geometry too, or this would pass on a replay that
    // dropped every `Size`. The unix-gated
    // `segmented_ring_replay_reproduces_live_rendering` asserts that half
    // already; nothing did on the platforms this module exists for.
    assert_eq!(
        columns(&term),
        120,
        "the geometry each segment was recorded at never reached the grid, \
         so it is still at the attach size"
    );
    drop(daemon);
}

/// A real pane's ring is megabytes; one `Snapshot` frame is far larger than
/// the reader's 256 KiB read buffer, so the frame is assembled across many
/// reads — several of which time out at `QUIT_POLL` while the sender is
/// still pushing.
#[test]
fn a_snapshot_larger_than_one_read_still_lands_whole() {
    crate::core::config::pin_test_config_dir();
    let (client_side, daemon) = socket_pair();
    let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

    let mut bulk = Vec::new();
    bulk.extend_from_slice(b"HEAD-OF-THE-RING\r\n");
    for i in 0..40_000 {
        bulk.extend_from_slice(format!("line {i} of the pane's history\r\n").as_bytes());
    }
    bulk.extend_from_slice(b"TAIL-OF-THE-RING\r\n");

    let feeder = std::thread::spawn(move || {
        let mut daemon = daemon;
        DaemonMsg::Size(ws(120, 40)).encode(&mut daemon).unwrap();
        DaemonMsg::Snapshot(bulk).encode(&mut daemon).unwrap();
        DaemonMsg::Size(ws(120, 40)).encode(&mut daemon).unwrap();
        DaemonMsg::Snapshot(b"AFTER-THE-BULK\r\n".to_vec())
            .encode(&mut daemon)
            .unwrap();
        daemon
    });

    let text = settled(&term, &["AFTER-THE-BULK"]);
    assert!(
        text.contains("AFTER-THE-BULK"),
        "the frame after a multi-megabyte snapshot never arrived; grid tail:\n{}",
        screen_text(&term)
    );
    assert!(
        text.contains("TAIL-OF-THE-RING"),
        "the snapshot itself was truncated"
    );
    drop(feeder.join().unwrap());
}

/// The view resizes to its real geometry the first time it paints, which
/// happens while the replay is still arriving. Against a resize-echoing
/// daemon the grid must not reflow when the request goes out — only when
/// the daemon echoes the `Size` back, which is the stream position where
/// the bytes stop being old-width — and neither step may cost the
/// replayed screen.
///
/// The two geometries are deliberately different: at identical dimensions
/// a reflow and a deferred reflow look the same, so the invariant would be
/// unobservable.
#[test]
fn a_resize_racing_the_replay_keeps_the_replayed_screen() {
    crate::core::config::pin_test_config_dir();
    let (client_side, mut daemon) = socket_pair();
    let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
    term.route = echoing_route();

    DaemonMsg::Size(ws(120, 40)).encode(&mut daemon).unwrap();
    DaemonMsg::Snapshot(b"REPLAYED-SCREEN\r\n".to_vec())
        .encode(&mut daemon)
        .unwrap();
    let text = settled(&term, &["REPLAYED-SCREEN"]);
    assert!(text.contains("REPLAYED-SCREEN"), "grid held:\n{text}");

    assert_eq!(columns(&term), 120, "the replay set the grid's geometry");

    // First paint: the pane is 100x30 on screen, not the 120x40 the ring
    // was recorded at. The request goes down the link and the grid stays
    // where the replay left it.
    term.resize(TermSize::new(100, 30), 8, 17);
    assert_eq!(
        columns(&term),
        120,
        "the grid reflowed at request time instead of waiting for the echo"
    );
    let _ = daemon.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    match ClientMsg::read(&mut daemon) {
        Ok(ClientMsg::Resize(size)) => assert_eq!((size.cols, size.rows), (100, 30)),
        other => panic!("the resize never reached the daemon: {other:?}"),
    }

    // The echo is the stream position the reflow belongs at.
    DaemonMsg::Size(ws(100, 30)).encode(&mut daemon).unwrap();
    for _ in 0..600 {
        if columns(&term) == 100 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let text = all_text(&term);
    assert_eq!(columns(&term), 100, "the echo never reflowed the grid");
    assert!(
        text.contains("REPLAYED-SCREEN"),
        "the first paint's resize cost the replay; grid held:\n{text}"
    );
    drop(daemon);
}

/// The attach handshake reads off the same socket the reader will, far
/// enough to tell an `Error` frame from a replay, and hands what it read
/// on as the reader's starting buffer. A whole replay can already be
/// sitting in the socket when it looks, so the prefix it takes is several
/// frames wide and the split lands mid-frame — every byte of it has to
/// reach the grid.
#[test]
fn the_attach_handshakes_prefix_carries_the_replay_it_swallowed() {
    crate::core::config::pin_test_config_dir();
    let (mut client_side, mut daemon) = socket_pair();

    // The whole replay before the client looks: this is the daemon that
    // answered instantly, which is the daemon a local attach meets.
    DaemonMsg::Size(ws(80, 24)).encode(&mut daemon).unwrap();
    DaemonMsg::Snapshot(b"BIRTH-BANNER\r\n".to_vec())
        .encode(&mut daemon)
        .unwrap();
    DaemonMsg::Size(ws(120, 40)).encode(&mut daemon).unwrap();
    let mut bulk = Vec::new();
    for i in 0..400 {
        bulk.extend_from_slice(format!("scrollback row {i}\r\n").as_bytes());
    }
    bulk.extend_from_slice(b"LAST-ROW-OF-THE-RING\r\n");
    DaemonMsg::Snapshot(bulk).encode(&mut daemon).unwrap();

    let buffered =
        attach_reply_prefix(&mut client_side, 1, std::time::Duration::from_secs(2)).unwrap();
    assert!(
        !buffered.is_empty(),
        "the handshake read nothing, so it proves nothing"
    );
    let term = RemoteTerminal::from_stream_with(
        client_side,
        TermSize::new(80, 24),
        buffered,
        PtySource::Raw,
    )
    .unwrap();

    let text = settled(&term, &["BIRTH-BANNER", "LAST-ROW-OF-THE-RING"]);
    assert!(text.contains("BIRTH-BANNER"), "grid held:\n{text}");
    assert!(
        text.contains("LAST-ROW-OF-THE-RING"),
        "the replay the handshake swallowed never reached the grid"
    );
    drop(daemon);
}

/// Issue #711: what a prompt report costs a replay that ended inside the
/// alternate screen.
///
/// A re-attach replays the pane's ring and then the pane's *state*, and
/// the state ends with the shell's prompt status. `active && at_prompt`
/// makes the reader scrub the TUI modes it finds in the grid, alternate
/// screen first — see `prompt_report_scrubs_stale_tui_modes`, which is the
/// case that is meant to reach it: a program that died without its
/// `?1049l` leaves the grid stranded on a screen nothing owns, and the
/// shell's next prompt is what proves it stale.
///
/// On a replay the alternate screen the scrub finds is the one the ring
/// just rebuilt, and `?1049l` does not undo it — it swaps it away. The
/// grid is left holding the primary screen underneath: the banner and the
/// command line the pane was born with, which is exactly what #711's
/// reporter photographed. The client cannot tell the two apart from the
/// frame, so the daemon decides — it declines to claim a prompt while a
/// foreground command owns the pty (`daemon::pane::replayed_at_prompt`).
/// Both halves of that contract are pinned here, from the side that pays
/// for it.
#[test]
fn a_replayed_prompt_report_decides_whether_the_alternate_screen_survives() {
    crate::core::config::pin_test_config_dir();

    // A re-attach, as the daemon writes one: the shell's banner on the
    // primary screen, the agent's alternate screen drawn over it, then the
    // pane's stored state. The trailing `Output` is a barrier — frames are
    // applied in order, so a grid that holds it has already been through
    // the prompt report, and neither half can pass on a race.
    let replayed_pane = |at_prompt: bool| {
        let (client_side, mut daemon) = socket_pair();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        DaemonMsg::Size(ws(80, 24)).encode(&mut daemon).unwrap();
        DaemonMsg::Snapshot(
            b"BIRTH-BANNER\r\nMac:Accounts joe$ omp --resume --fork-session\r\n".to_vec(),
        )
        .encode(&mut daemon)
        .unwrap();
        DaemonMsg::Snapshot(b"\x1b[?1049h\x1b[2J\x1b[HAGENT-SCREEN\r\n".to_vec())
            .encode(&mut daemon)
            .unwrap();
        DaemonMsg::Prompt {
            active: true,
            at_prompt,
            last_exit: Some(0),
        }
        .encode(&mut daemon)
        .unwrap();
        DaemonMsg::Output(b"REPORT-APPLIED\r\n".to_vec())
            .encode(&mut daemon)
            .unwrap();
        let text = settled(&term, &["REPORT-APPLIED"]);
        drop(daemon);
        text
    };

    // A program owns the pane, so the replay reports no prompt and the
    // screen the ring rebuilt is the screen the pane shows.
    let text = replayed_pane(false);
    assert!(
        text.contains("REPORT-APPLIED"),
        "the barrier never arrived, so nothing below is tested; grid held:\n{text}"
    );
    assert!(
        text.contains("AGENT-SCREEN"),
        "a replay that does not claim a prompt must leave the alternate screen it \
         rebuilt alone; grid held:\n{text}"
    );

    // The same replay under the stale claim: the scrub swaps the agent's
    // screen away, and what the pane comes back showing is the banner it
    // was born with. That is #711.
    let text = replayed_pane(true);
    assert!(
        !text.contains("AGENT-SCREEN"),
        "a prompt report still has to scrub a stranded alternate screen, or the daemon's \
         gate is load-bearing for nothing; grid held:\n{text}"
    );
    assert!(
        text.contains("BIRTH-BANNER"),
        "what the scrub leaves is the primary screen underneath; grid held:\n{text}"
    );
}

// ---- The switch, end to end, with a real daemon pane ----------------
//
// Everything above drives the client half against a scripted daemon. This
// drives the real one: a live `DaemonPane` over a real pty, in this
// process, with its `DaemonMsg` stream forwarded onto a socket pair the
// way `spawn_writer` forwards it, so an attach here is the same attach a
// re-attach makes. It is the switch (#711) minus gpui: attach, resize to
// the geometry the window actually has, produce output, drop the client,
// produce more, attach again — and read the grid the second client ends up
// with.

/// A client hung off `pane`, the way `stream_pane_with_attach` hangs one
/// off it: subscribe, forward every queued message onto the wire, and read
/// the client's own frames back the way `run_stream` does — epoch guard
/// included, so a displaced client's resize is dropped here exactly as the
/// daemon drops it.
///
/// The route is a resize-echoing one, which is what the local daemon is:
/// the grid must not reflow until the `Size` the daemon echoes back.
fn attach_client(
    pane: &std::sync::Arc<lyrel_core::daemon::pane::DaemonPane>,
) -> (u64, RemoteTerminal, std::thread::JoinHandle<()>) {
    let (client_side, daemon_side) = socket_pair();
    let mut daemon_write = daemon_side.try_clone().expect("clone the daemon half");
    let (tx, rx) = std::sync::mpsc::channel::<DaemonMsg>();
    let epoch = pane.attach(tx);
    let gate = pane.gate();
    let forward = std::thread::spawn(move || {
        while let Ok(msg) = rx.recv() {
            let drained = match &msg {
                DaemonMsg::Output(b) => b.len(),
                DaemonMsg::Graphics(b) => b.len(),
                _ => 0,
            };
            let ok = msg.encode(&mut daemon_write).is_ok();
            if drained > 0 {
                gate.sub(drained);
            }
            if !ok {
                break;
            }
        }
    });
    {
        let pane = pane.clone();
        let mut daemon_read = daemon_side;
        std::thread::spawn(move || {
            use std::io::Read as _;
            let mut pending: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 65536];
            loop {
                while let Ok(Some((kind, payload))) =
                    crate::daemon::protocol::take_frame(&mut pending)
                {
                    match ClientMsg::from_frame(kind, payload) {
                        Ok(ClientMsg::Input(bytes)) if pane.controls(epoch) => {
                            pane.write_input(&bytes)
                        }
                        Ok(ClientMsg::Resize(size)) if pane.controls(epoch) => {
                            pane.resize(size)
                        }
                        Ok(_) => {}
                        Err(_) => return,
                    }
                }
                match daemon_read.read(&mut chunk) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => pending.extend_from_slice(&chunk[..n]),
                }
            }
        });
    }
    let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24))
        .expect("a client over the pair");
    term.route = echoing_route();
    (epoch, term, forward)
}

/// A route whose daemon echoes `Size` when it applies a resize — which is
/// what `PaneRoute::Local` is against a current daemon, and what the
/// harness above implements.
///
/// Built the way the neighbouring resize tests build one, rather than
/// through a `PaneWorkspace`: routing a workspace is not what any of these
/// cover, and going that way would have a change to `NativeSshSpec`'s
/// serde shape fail them on an `unwrap` that has nothing to do with
/// replay.
fn echoing_route() -> PaneRoute {
    PaneRoute::Remote {
        header: Box::new(crate::daemon::router::RouteHeader::wsl("Ubuntu-22.04")),
        resize_echo: true,
    }
}

fn wait_for(term: &RemoteTerminal, needle: &str) -> bool {
    for _ in 0..600 {
        if all_text(term).contains(needle) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    false
}

/// Direct PTY input does not order against the client's asynchronous resize.
/// Wait for its Size echo before recording the markers replayed by a new view.
fn wait_for_columns(term: &RemoteTerminal, expected: usize) -> bool {
    for _ in 0..600 {
        if columns(term) == expected {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    false
}

/// Types `line` at the pane until the grid echoes `needle` back, and
/// answers whether it ever did.
///
/// One write is not enough for the *first* command a fresh pane is given.
/// `spawn` returning means the pty exists, not that the shell behind it
/// has started, printed a prompt, or begun reading — and on Windows the
/// ConPTY has not necessarily connected the child to its input pipe yet,
/// so bytes typed into that window reach nobody at all. On a loaded runner
/// the window is wide: both cases showed up on Windows CI as a grid that
/// was still completely empty after 15s, so not even a prompt had been
/// printed, let alone an echo.
///
/// Retyping is safe for everything asserted here: a line that did land and
/// was merely slow simply runs twice, and every assertion is a `contains`.
/// The later commands in these tests keep their single `write_input` —
/// by then the shell has echoed once, which is proof it is reading.
fn type_until_echoed(
    pane: &lyrel_core::daemon::pane::DaemonPane,
    line: &[u8],
    term: &RemoteTerminal,
    needle: &str,
) -> bool {
    for _ in 0..15 {
        pane.write_input(line);
        for _ in 0..80 {
            if all_text(term).contains(needle) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
    false
}

/// Switching workspaces drops every pane and attaches to the same daemon
/// panes again. Whatever the pane put on screen while the window was
/// elsewhere — and whatever it had already — has to come back with it.
#[test]
fn a_pane_re_attached_after_a_switch_gets_its_screen_back() {
    crate::core::config::pin_test_config_dir();
    let pane = lyrel_core::daemon::pane::DaemonPane::spawn(
        7711,
        std::env::current_dir().ok(),
        ws(80, 24),
        None,
        None,
        None,
        None,
        false,
        || {},
    )
    .expect("a pty-backed pane");

    // The window this pane is shown in, attaching for the first time.
    let (epoch, mut first, forward) = attach_client(&pane);
    // What the first paint does, from the same side it does it on: the
    // pane is not 80x24 on screen, so the real geometry goes down the link
    // and the grid waits for the daemon to echo it back.
    first.resize(TermSize::new(120, 40), 8, 17);
    assert!(
        wait_for_columns(&first, 120),
        "the initial resize was not acknowledged before preparing the replay"
    );
    assert!(
        type_until_echoed(
            &pane,
            b"echo BEFORE-THE-SWITCH\r",
            &first,
            "BEFORE-THE-SWITCH"
        ),
        "the pane never echoed the first command; grid held:\n{}",
        all_text(&first)
    );

    // Switching away: the view is dropped, which closes the link, and the
    // daemon gives up the seat.
    drop(first);
    pane.detach(epoch);
    drop(forward);

    // The pane keeps working while the window is showing another workspace.
    pane.write_input(b"echo DURING-THE-SWITCH\r");
    std::thread::sleep(std::time::Duration::from_millis(600));

    // Switching back: a brand new view, attaching at the hardcoded size.
    //
    // It deliberately never resizes. Resizing is the workaround #711's
    // reporter found — it makes the daemon `TIOCSWINSZ` the pty and the
    // *child* repaint, which would put the screen back whether or not the
    // replay ever arrived, and would also feed the grid a `Size` echo that
    // `resize_state` sends unconditionally. Everything asserted below has
    // to come from the replay itself.
    let (_epoch, second, forward) = attach_client(&pane);
    let landed = wait_for(&second, "DURING-THE-SWITCH");
    let text = all_text(&second);
    let width = columns(&second);
    pane.kill();
    drop(forward);

    assert!(
        landed,
        "the re-attached pane never got the output produced while it was hidden; \
         grid held:\n{text}"
    );
    assert!(
        text.contains("BEFORE-THE-SWITCH"),
        "the re-attached pane lost the screen it had before the switch; grid held:\n{text}"
    );
    // Nothing resized this client, so 120 can only have come from the
    // `Size` frame the replay sends ahead of the segment recorded at it.
    assert_eq!(
        width, 120,
        "the replay's geometry never reached the grid, so it is still at the attach size"
    );
}

/// A switch can rebuild a window twice — a second hydration lands while the
/// first one's panes are still draining their replay — so the same pane is
/// attached to twice in quick succession and the window keeps the later
/// view. That view must hold the screen, and it must still be the one the
/// daemon obeys: the displaced attach's teardown must not take the seat
/// with it.
///
/// The needle is weaker here than in the switch test above, and knowingly
/// so: the pane is attached to throughout, and a ConPTY that owns the whole
/// viewport reprints what is on screen as ordinary live output, so
/// `RACED-OUTPUT` can reach the second view without the replay. What only
/// the replay can supply is the geometry — nothing resizes this client —
/// and what only the seat can supply is `AFTER-THE-RACE`. Those two are the
/// assertions that discriminate.
#[test]
fn the_later_of_two_racing_attaches_keeps_the_screen_and_the_seat() {
    crate::core::config::pin_test_config_dir();
    let pane = lyrel_core::daemon::pane::DaemonPane::spawn(
        7712,
        std::env::current_dir().ok(),
        ws(80, 24),
        None,
        None,
        None,
        None,
        false,
        || {},
    )
    .expect("a pty-backed pane");

    let (first_epoch, mut first, first_forward) = attach_client(&pane);
    first.resize(TermSize::new(120, 40), 8, 17);
    assert!(
        wait_for_columns(&first, 120),
        "the initial resize was not acknowledged before preparing the replay"
    );
    assert!(
        type_until_echoed(&pane, b"echo RACED-OUTPUT\r", &first, "RACED-OUTPUT"),
        "the pane never echoed; grid held:\n{}",
        all_text(&first)
    );

    // The second rebuild attaches before the first one's view is dropped.
    let (second_epoch, second, second_forward) = attach_client(&pane);
    drop(first);
    drop(first_forward);
    // The displaced connection detaches on its way out, epoch-guarded.
    pane.detach(first_epoch);

    // Never resized, for the reason the switch test is not: a resize would
    // repaint the pane from the child and echo a `Size` back, and both are
    // exactly what must not be allowed to stand in for the replay.
    let landed = wait_for(&second, "RACED-OUTPUT");
    let width = columns(&second);
    // Two halves of "the seat survived", and only one of them can catch a
    // detach that took it. `controls` answers from `subscriber_epoch`
    // alone, and `DaemonPane::detach` clears `subscriber` without ever
    // touching that counter — so it says the pane would obey this view's
    // input and resizes, but it would keep saying so with the epoch guard
    // stripped out of `detach`. `live` is what notices that: a detach that
    // dropped the wrong subscriber silences the surviving view.
    let controls = pane.controls(second_epoch);
    pane.write_input(b"echo AFTER-THE-RACE\r");
    let live = wait_for(&second, "AFTER-THE-RACE");
    let text = all_text(&second);
    pane.kill();
    drop(second_forward);

    assert!(
        landed,
        "the second attach's replay was lost; grid held:\n{text}"
    );
    assert_eq!(
        width, 120,
        "the second attach's replay carried no geometry, so the grid is still at the \
         attach size"
    );
    assert!(
        controls,
        "the pane no longer answers to the surviving view, so nothing it \
         types or resizes would reach the pty"
    );
    assert!(
        live,
        "the displaced attach's detach took the live seat: the surviving \
         view stopped receiving output"
    );
}
