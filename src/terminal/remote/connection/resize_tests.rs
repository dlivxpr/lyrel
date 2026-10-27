use super::*;
use crate::daemon::protocol::DaemonMsg;
use crate::terminal::remote::RemoteTerminal;
use std::io::Write as _;
use std::os::unix::net::UnixStream;

#[test]
fn first_resize_always_syncs_then_dedups() {
    let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
    let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

    term.resize(TermSize::new(80, 24), 8, 17);
    match ClientMsg::read(&mut daemon_side).unwrap() {
        ClientMsg::Resize(ws) => assert_eq!((ws.cols, ws.rows), (80, 24)),
        other => panic!("expected the first Resize to be sent, got {other:?}"),
    }

    term.resize(TermSize::new(80, 24), 8, 17);
    term.write(b"marker".to_vec());
    match ClientMsg::read(&mut daemon_side).unwrap() {
        ClientMsg::Input(bytes) => assert_eq!(bytes, b"marker"),
        other => panic!("expected Input (dup resize sends nothing), got {other:?}"),
    }
}

#[test]
fn layout_resize_reasserts_geometry_after_a_late_size_frame() {
    let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
    let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

    term.resize(TermSize::new(100, 40), 8, 17);
    assert!(matches!(
        ClientMsg::read(&mut daemon_side).unwrap(),
        ClientMsg::Resize(_)
    ));

    DaemonMsg::Size(WinSize {
        cols: 120,
        rows: 30,
        cell_w: 8,
        cell_h: 17,
    })
    .encode(&mut daemon_side)
    .unwrap();
    DaemonMsg::Snapshot(b"old screen".to_vec())
        .encode(&mut daemon_side)
        .unwrap();
    daemon_side.flush().unwrap();
    for _ in 0..200 {
        if term.term.lock().columns() == 120 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(term.term.lock().columns(), 120, "replay geometry applied");

    term.resize(TermSize::new(100, 40), 8, 17);
    assert_eq!(term.term.lock().columns(), 100);
    assert_eq!(term.term.lock().screen_lines(), 40);
    assert!(matches!(
        ClientMsg::read(&mut daemon_side).unwrap(),
        ClientMsg::Resize(ws) if ws.cols == 100 && ws.rows == 40
    ));
}

#[test]
fn resize_updates_size_and_notifies_daemon() {
    let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
    let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

    term.resize(TermSize::new(100, 40), 9, 18);
    assert_eq!(term.size(), TermSize::new(100, 40));
    match ClientMsg::read(&mut daemon_side).unwrap() {
        ClientMsg::Resize(ws) => {
            assert_eq!((ws.cols, ws.rows, ws.cell_w, ws.cell_h), (100, 40, 9, 18));
        }
        other => panic!("expected Resize, got {other:?}"),
    }
}

#[test]
fn echoed_resize_defers_the_grid_reflow_to_the_daemons_size_frame() {
    use rio_vt::crosswords::pos::{Column, Line};

    let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
    let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
    // A remote route whose host advertised the echo. The local daemon's
    // answer rides the same gate through `local_daemon_supports`, so this
    // covers the deferral for both.
    term.connection.route = PaneRoute::Remote {
        header: Box::new(crate::daemon::router::RouteHeader::wsl("Ubuntu-22.04")),
        resize_echo: true,
    };

    term.resize(TermSize::new(120, 30), 8, 17);
    assert!(matches!(
        ClientMsg::read(&mut daemon_side).unwrap(),
        ClientMsg::Resize(ws) if ws.cols == 120 && ws.rows == 30
    ));
    assert_eq!(
        term.term.lock().columns(),
        80,
        "the grid keeps the old geometry until the daemon's echo arrives"
    );

    // Old-width bytes still in flight ahead of the echo: a CUP to column
    // 100 must clamp on the 80-col grid. The same addressing after the
    // echo reaches the real column on the 120-col grid.
    DaemonMsg::Output(b"\x1b[1;100HA".to_vec())
        .encode(&mut daemon_side)
        .unwrap();
    DaemonMsg::Size(WinSize {
        cols: 120,
        rows: 30,
        cell_w: 8,
        cell_h: 17,
    })
    .encode(&mut daemon_side)
    .unwrap();
    DaemonMsg::Output(b"\x1b[2;100HB".to_vec())
        .encode(&mut daemon_side)
        .unwrap();
    daemon_side.flush().unwrap();

    for _ in 0..400 {
        {
            let t = term.term.lock();
            if t.columns() == 120 && t.grid[Line(1)][Column(99)].c() == 'B' {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let t = term.term.lock();
    let grid = &t.grid;
    assert_eq!(
        grid[Line(0)][Column(79)].c(),
        'A',
        "bytes queued before the echo parse at the old width"
    );
    assert_eq!(
        grid[Line(1)][Column(99)].c(),
        'B',
        "bytes after the echo parse at the new width"
    );
}

/// A route whose daemon promised to echo every resize, over a socket pair
/// the test plays the daemon on. The switch tests build the same route
/// against a real pane.
fn echoing_pair() -> (RemoteTerminal, UnixStream) {
    let (client_side, daemon_side) = UnixStream::pair().unwrap();
    let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
    term.connection.route = PaneRoute::Remote {
        header: Box::new(crate::daemon::router::RouteHeader::wsl("Ubuntu-22.04")),
        resize_echo: true,
    };
    daemon_side
        .set_read_timeout(Some(std::time::Duration::from_millis(150)))
        .unwrap();
    (term, daemon_side)
}

/// The geometry of the next frame the pane sent, if it sent one at all.
fn next_resize(daemon_side: &mut UnixStream) -> Option<(u16, u16)> {
    match ClientMsg::read(daemon_side) {
        Ok(ClientMsg::Resize(ws)) => Some((ws.cols, ws.rows)),
        Ok(other) => panic!("expected a Resize, got {other:?}"),
        Err(_) => None,
    }
}

/// Makes the last resize old enough that an echo for it is no longer
/// expected, without the test sitting out the grace for real.
fn outlive_the_echo_grace(term: &mut RemoteTerminal) {
    term.connection.resize_sent_at = std::time::Instant::now()
        .checked_sub(RESIZE_ECHO_GRACE + std::time::Duration::from_millis(1));
}

fn wait_for_columns(term: &RemoteTerminal, cols: usize) {
    for _ in 0..400 {
        if term.term.lock().columns() == cols {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the grid never reached {cols} columns");
}

/// #893: a pane came back from sleep or a workspace switch painting a grid
/// shorter than its bounds, and stayed that way until a divider drag. On an
/// echoing route the grid only moves when the daemon's `Size` comes back,
/// and the early-out trusted the request: once asked for, the same size
/// was never sent again, so a request or echo that went missing stranded
/// the grid — and the child — at the old geometry for good.
#[test]
fn an_echoed_resize_whose_echo_never_came_is_asked_for_again() {
    let (mut term, mut daemon_side) = echoing_pair();
    term.resize(TermSize::new(120, 30), 8, 17);
    assert_eq!(next_resize(&mut daemon_side), Some((120, 30)));

    // Every frame asks again. While the echo can still be on its way that
    // is not a reason to resend.
    term.resize(TermSize::new(120, 30), 8, 17);
    assert_eq!(
        next_resize(&mut daemon_side),
        None,
        "an echo still in flight must not be chased with duplicates"
    );

    // The echo never comes. Past the grace the grid is still 80x24, and
    // the frame that notices must ask for the size again.
    outlive_the_echo_grace(&mut term);
    term.resize(TermSize::new(120, 30), 8, 17);
    assert_eq!(
        next_resize(&mut daemon_side),
        Some((120, 30)),
        "a resize whose echo never arrived was never retried, so the grid \
         stays at the old size until the layout asks for a different one"
    );
    assert_eq!(
        term.term.lock().columns(),
        80,
        "the retry still waits for the echo rather than reflowing early"
    );

    // Once the echo lands the size is settled, however old the request.
    DaemonMsg::Size(WinSize {
        cols: 120,
        rows: 30,
        cell_w: 8,
        cell_h: 17,
    })
    .encode(&mut daemon_side)
    .unwrap();
    daemon_side.flush().unwrap();
    wait_for_columns(&term, 120);
    assert_eq!(term.term.lock().screen_lines(), 30);
    outlive_the_echo_grace(&mut term);
    term.resize(TermSize::new(120, 30), 8, 17);
    assert_eq!(
        next_resize(&mut daemon_side),
        None,
        "a grid that reached the requested size must not be resized again"
    );
}

/// The other way the grid and the request can part on an echoing route: a
/// `Size` frame that lands after the echo moves the grid off the size the
/// layout asked for. The non-echoing path already re-asserts there
/// (`layout_resize_reasserts_geometry_after_a_late_size_frame`); the
/// echoing one skipped it, since the request itself had not changed.
#[test]
fn an_echoed_resize_reasserts_geometry_after_a_late_size_frame() {
    let (mut term, mut daemon_side) = echoing_pair();
    term.resize(TermSize::new(120, 30), 8, 17);
    assert_eq!(next_resize(&mut daemon_side), Some((120, 30)));
    for (cols, rows) in [(120, 30), (100, 12)] {
        DaemonMsg::Size(WinSize {
            cols,
            rows,
            cell_w: 8,
            cell_h: 17,
        })
        .encode(&mut daemon_side)
        .unwrap();
    }
    daemon_side.flush().unwrap();
    wait_for_columns(&term, 100);

    outlive_the_echo_grace(&mut term);
    term.resize(TermSize::new(120, 30), 8, 17);
    assert_eq!(
        next_resize(&mut daemon_side),
        Some((120, 30)),
        "a grid knocked off the requested size was left there"
    );
}

/// A relink re-sends the geometry the pane has now — including a resize
/// made after the old link had already died, which reached no daemon.
#[test]
fn a_relink_delivers_the_resize_made_while_the_link_was_down() {
    crate::core::config::pin_test_config_dir();
    let (old_client, mut old_daemon) = UnixStream::pair().unwrap();
    let mut term = RemoteTerminal::from_stream(old_client, TermSize::new(80, 24)).unwrap();
    term.resize(TermSize::new(100, 40), 8, 17);
    assert!(matches!(
        ClientMsg::read(&mut old_daemon).unwrap(),
        ClientMsg::Resize(ws) if ws.cols == 100 && ws.rows == 40
    ));

    // The link drops; the layout keeps moving.
    drop(old_daemon);
    term.resize(TermSize::new(120, 30), 8, 17);

    let (new_client, mut new_daemon) = UnixStream::pair().unwrap();
    new_daemon
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    // What `relink_plan` hands the relink: the size the view has now.
    let size = term.size();
    term.adopt_relink(new_client, Vec::new(), &PaneRoute::Local, size, 8, 17)
        .unwrap();
    assert!(
        matches!(
            ClientMsg::read(&mut new_daemon).unwrap(),
            ClientMsg::Resize(ws) if ws.cols == 120 && ws.rows == 30
        ),
        "the relinked daemon must be told the size the pane has now"
    );
}
#[test]
fn a_remote_route_without_the_advertised_echo_reflows_at_request_time() {
    crate::core::config::pin_test_config_dir();
    let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
    let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
    // What `for_workspace` builds when the host's hello named no echo —
    // an older server, or a link that was down when the route was made.
    term.connection.route = PaneRoute::Remote {
        header: Box::new(crate::daemon::router::RouteHeader::wsl("Ubuntu-22.04")),
        resize_echo: false,
    };

    term.resize(TermSize::new(120, 30), 8, 17);
    assert_eq!(
        term.term.lock().columns(),
        120,
        "with no promised echo the grid must reflow at request time — \
         deferring would leave it at the old width forever"
    );
    assert!(matches!(
        ClientMsg::read(&mut daemon_side).unwrap(),
        ClientMsg::Resize(ws) if ws.cols == 120 && ws.rows == 30
    ));

    // The other route with no daemon to promise anything.
    term.connection.route = PaneRoute::Unroutable("no ssh details".into());
    assert!(
        !term.connection.resize_echoed(),
        "no route, no echo to wait for"
    );
}
