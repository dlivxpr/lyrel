use super::*;
use crate::terminal::remote::test_support::ssh_workspace;
use crate::daemon::protocol::WinSize;
use std::io::Write as _;
use std::os::unix::net::UnixStream;

#[test]
fn a_local_spawn_carries_its_workspace_so_the_shell_gets_lyrel_ws() {
    let ws = "0d4e1a54-0000-4000-8000-000000000001";

    assert_eq!(
        spawn_workspace(Some(ws), &PaneRoute::Local).as_deref(),
        Some(ws),
        "without this the pane's shell has no $LYREL_WS and every \
         workspace-scoped CLI verb in it needs an explicit address"
    );

    assert_eq!(
        spawn_workspace(None, &PaneRoute::Local),
        None,
        "a pane outside any workspace must not claim one"
    );

    assert_eq!(
        spawn_workspace(Some(ws), &PaneRoute::for_workspace(Some(&ssh_workspace()))),
        None,
        "a remote server has its own machine tree; this id names a workspace in ours"
    );
}

#[test]
fn spawn_retry_only_for_daemon_disconnects() {
    let eof: anyhow::Error =
        std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "closed").into();
    assert!(daemon_disconnected_before_spawn_reply(&eof));

    let refused = anyhow::anyhow!("daemon refused Spawn: configured shell missing");
    assert!(!daemon_disconnected_before_spawn_reply(&refused));
}

#[test]
fn only_a_dead_daemon_is_worth_starting_one_for() {
    let connect_failed = |kind| -> anyhow::Error {
        anyhow::Error::new(std::io::Error::new(kind, "no listener"))
            .context("connect to daemon at /tmp/lyrel.sock")
    };
    assert!(daemon_not_listening(&connect_failed(
        std::io::ErrorKind::ConnectionRefused
    )));
    assert!(daemon_not_listening(&connect_failed(
        std::io::ErrorKind::NotFound
    )));

    let refused = anyhow::anyhow!("daemon refused Spawn: configured shell missing");
    assert!(!daemon_not_listening(&refused));
    let eof: anyhow::Error =
        std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "closed").into();
    assert!(!daemon_not_listening(&eof));
}

#[test]
fn an_attach_to_a_missing_pane_is_an_error_not_a_disconnect() {
    let (mut client_side, mut daemon_side) = UnixStream::pair().unwrap();
    DaemonMsg::Error("no such pane 7".to_string())
        .encode(&mut daemon_side)
        .unwrap();
    daemon_side.flush().unwrap();

    let err = attach_reply_prefix(&mut client_side, 7, attach_reply_wait(&PaneRoute::Local))
        .expect_err("a missing pane must fail");
    assert!(
        format!("{err:#}").contains("no such pane 7"),
        "the daemon's own wording is what names which pane went: {err:#}"
    );
}

#[test]
fn an_attach_the_daemon_hangs_up_on_is_an_error() {
    let (mut client_side, daemon_side) = UnixStream::pair().unwrap();
    drop(daemon_side);
    assert!(
        attach_reply_prefix(&mut client_side, 7, attach_reply_wait(&PaneRoute::Local)).is_err()
    );
}

/// #673: a daemon that attached replays the pane's ring at once, and the
/// ring always holds a segment, so even a pane that has printed nothing
/// answers with a `Size`. A connection that stays silent for the whole wait
/// is therefore one nobody is serving — read as success, it became a pane
/// the window drew as restored while every keystroke, Ctrl-C included,
/// went into a socket nothing drained.
#[test]
fn an_attach_nobody_answers_is_not_an_attach() {
    let (mut client_side, _daemon_side) = UnixStream::pair().unwrap();
    let wait = std::time::Duration::from_millis(200);
    let err = attach_reply_prefix(&mut client_side, 7, wait)
        .expect_err("silence for the whole wait must not pass for an attached pane");
    assert!(
        format!("{err:#}").contains("did not answer"),
        "the failure has to say the daemon never answered, not that the pane is gone: {err:#}"
    );
    assert!(
        attach_unanswered(&err),
        "silence is its own verdict, told apart from a refusal or a hangup"
    );
    // A refusal is not it: the pane really is gone then, and the caller
    // may say so.
    let (mut client_side, mut daemon_side) = UnixStream::pair().unwrap();
    DaemonMsg::Error("no such pane 7".into())
        .encode(&mut daemon_side)
        .unwrap();
    daemon_side.flush().unwrap();
    let refused = attach_reply_prefix(&mut client_side, 7, wait).expect_err("refused");
    assert!(!attach_unanswered(&refused));
}

/// The relink retry loop keys off this split: a refusal means the pane is
/// gone on its machine and asking again can only repeat the answer, while
/// silence and a hangup are transport trouble worth another try.
#[test]
fn a_refused_attach_is_final_and_every_other_failure_is_not() {
    let wait = std::time::Duration::from_millis(200);

    let (mut client_side, mut daemon_side) = UnixStream::pair().unwrap();
    DaemonMsg::Error("no such pane 7".into())
        .encode(&mut daemon_side)
        .unwrap();
    daemon_side.flush().unwrap();
    let refused = attach_reply_prefix(&mut client_side, 7, wait).expect_err("refused");
    assert!(attach_refused(&refused));
    assert!(
        format!("{refused:#}").contains("no such pane 7"),
        "the daemon's own words survive: {refused:#}"
    );

    let (mut client_side, _held_open) = UnixStream::pair().unwrap();
    let silent = attach_reply_prefix(&mut client_side, 7, wait).expect_err("silent");
    assert!(
        !attach_refused(&silent),
        "silence is not a verdict on the pane"
    );

    let (mut client_side, daemon_side) = UnixStream::pair().unwrap();
    drop(daemon_side);
    let hungup = attach_reply_prefix(&mut client_side, 7, wait).expect_err("hung up");
    assert!(
        !attach_refused(&hungup),
        "a hangup is not a verdict on the pane"
    );
}

#[test]
fn graphics_budget_refusal_is_not_evidence_that_the_pane_is_gone() {
    for message in [
        "Desktop graphics budget cannot admit retained session content",
        "Graphics replay connection closed during admission",
        "Graphics replay failed",
    ] {
        let (mut client_side, mut daemon_side) = UnixStream::pair().unwrap();
        DaemonMsg::Error(message.into())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        let error =
            attach_reply_prefix(&mut client_side, 7, std::time::Duration::from_millis(200))
                .expect_err("admission refused");
        assert!(
            !attach_refused(&error),
            "only an explicit missing-pane verdict can abandon the original session: {error:#}"
        );
        assert!(format!("{error:#}").contains(message));
    }
}

/// The other side of the rule above: the frame a quiet pane does send is
/// enough. Nothing else is required of the daemon for the attach to stand.
#[test]
fn a_size_frame_alone_is_a_live_attach() {
    let (mut client_side, mut daemon_side) = UnixStream::pair().unwrap();
    DaemonMsg::Size(WinSize {
        cols: 80,
        rows: 24,
        cell_w: 8,
        cell_h: 17,
    })
    .encode(&mut daemon_side)
    .unwrap();
    daemon_side.flush().unwrap();

    let buffered =
        attach_reply_prefix(&mut client_side, 7, std::time::Duration::from_millis(200))
            .expect("the daemon's first frame is the attach ack");
    assert!(
        !buffered.is_empty(),
        "the Size frame was read to classify the reply; it must reach the reader"
    );
}

#[test]
fn a_local_attach_does_not_wait_as_long_as_a_remote_one() {
    let local = attach_reply_wait(&PaneRoute::Local);
    let remote = attach_reply_wait(&PaneRoute::for_workspace(Some(&ssh_workspace())));
    assert!(local < remote, "{local:?} must be the shorter wait");
    assert!(
        local <= std::time::Duration::from_secs(2),
        "the UI thread is holding still for this"
    );
}

#[test]
fn a_live_attach_hands_its_replay_bytes_to_the_reader() {
    crate::core::config::pin_test_config_dir();
    let (mut client_side, mut daemon_side) = UnixStream::pair().unwrap();
    DaemonMsg::Snapshot(b"hello".to_vec())
        .encode(&mut daemon_side)
        .unwrap();
    daemon_side.flush().unwrap();

    let buffered =
        attach_reply_prefix(&mut client_side, 7, attach_reply_wait(&PaneRoute::Local))
            .expect("a live pane attaches");
    assert!(
        !buffered.is_empty(),
        "the classification read the Snapshot frame; it must come back"
    );
    let term = RemoteTerminal::from_stream_with(
        client_side,
        TermSize::new(80, 24),
        buffered,
        PtySource::Raw,
    )
    .unwrap();

    let mut got = String::new();
    for _ in 0..200 {
        {
            let t = term.term.lock();
            let grid = &t.grid;
            got.clear();
            for col in 0..5usize {
                got.push(
                    grid[rio_vt::crosswords::pos::Line(0)]
                        [rio_vt::crosswords::pos::Column(col)]
                    .c(),
                );
            }
        }
        if got == "hello" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        got, "hello",
        "the pre-read replay must still reach the grid"
    );
}
