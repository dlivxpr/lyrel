//! Program-status authority through the public daemon consumers.
//!
//! A real daemon and a real PTY run an interactive `/bin/sh` that prints OSC
//! 7501 sequences. Assertions read ordered `DaemonMsg::ProgramStatus` frames
//! and apply them to the public `ProgramStatusClient`; nothing here reaches
//! into pane state or fabricates a frame. The transparent per-line barriers in
//! [`support`] make the frame window deterministic without sleeping.

#![cfg(unix)]

#[path = "program_status_support/mod.rs"]
mod support;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use lyrel_core::client::{PaneClient, PaneSession};
use lyrel_core::core::program_status::{
    BlockedKind, ProgramRecord, ProgramState, ProgramStatusClient,
};
use lyrel_core::daemon::protocol::{
    ClientMsg, DaemonMsg, DaemonVersion, PROTOCOL_VERSION, ShellSpec, WinSize,
};
use lyrel_core::daemon::transport;
use support::{STREAM_WITHIN, Status};

struct Daemon {
    child: Child,
    dir: tempfile::TempDir,
}

impl Daemon {
    fn start() -> Daemon {
        let dir = tempfile::TempDir::new().unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_lyrel-server"))
            .arg("--daemon")
            .arg("--config-dir")
            .arg(dir.path())
            .env("LYREL_DATA_DIR", dir.path())
            .env("LYREL_CONTROL_SOCK", dir.path().join("control.sock"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start lyrel-server --daemon");
        let daemon = Daemon { child, dir };
        daemon.await_ready();
        daemon
    }

    fn dir_path(&self) -> &Path {
        self.dir.path()
    }

    fn pane_endpoint(&self) -> PathBuf {
        self.dir.path().join("daemon.sock")
    }

    fn panes(&self) -> PaneClient {
        PaneClient::at(self.pane_endpoint())
    }

    fn await_ready(&self) {
        let deadline = Instant::now() + STREAM_WITHIN;
        while self.panes().version().is_err() {
            assert!(
                Instant::now() < deadline,
                "the daemon did not open its pane endpoint"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn spawn_shell(&self) -> PaneSession {
        self.panes()
            .spawn(
                None,
                size(),
                Some(ShellSpec {
                    program: "/bin/sh".into(),
                    args: Vec::new(),
                    args_are_lyrel_defaults: false,
                }),
                None,
                None,
            )
            .expect("spawn /bin/sh in a real pty")
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn size() -> WinSize {
    WinSize {
        cols: 100,
        rows: 30,
        cell_w: 8,
        cell_h: 16,
    }
}

/// A daemon, one live shell and its authoritative consumer projection.
fn fresh() -> (Daemon, PaneSession, ProgramStatusClient) {
    let daemon = Daemon::start();
    let mut session = daemon.spawn_shell();
    let (_, baseline) = support::pump_baseline(&mut session);
    let mut client = ProgramStatusClient::default();
    assert!(
        client.apply(&baseline),
        "a fresh spawn must hand the consumer a session baseline"
    );
    (daemon, session, client)
}

fn collection(ids: &[&str]) -> Vec<String> {
    let mut ids: Vec<String> = ids.iter().map(|id| (*id).to_owned()).collect();
    ids.sort();
    ids
}

fn update(frame: &support::Pumped) -> &lyrel_core::core::program_status::ProgramDelta {
    assert_eq!(frame.frames.len(), 1, "expected exactly one live update");
    support::delta(&frame.frames[0]).expect("one live update")
}

#[test]
fn baseline_live_prompt_exit_and_resets_follow_the_ordered_stream() {
    let (daemon, mut session, mut client) = fresh();

    let pumped = support::run(
        &mut session,
        &support::reports(&[Status::new("working").id("task").title("Demo")], "true"),
    );
    support::apply_frames(&mut client, &pumped.frames);
    let task = support::record(&client, "task").expect("the report arrived live");
    assert_eq!(task.state(), ProgramState::Working);
    assert_eq!(task.title(), Some("Demo"));
    let before_attach = ProgramRecord::clone(task);

    // A fresh attach must receive one baseline, not a replay of reports.
    let pane = session.pane_id();
    session.detach().unwrap();
    let mut session = daemon.panes().attach(pane, size()).unwrap();
    let (_, baseline) = support::pump_baseline(&mut session);
    let session_id = client
        .session_id()
        .expect("the baseline carries a lifetime")
        .to_string();
    // Re-applying an unchanged same-session baseline is a legal no-op; the
    // assertion is the projection it restores, not the return value.
    client.apply(&baseline);
    assert_eq!(client.session_id(), Some(session_id.as_str()));
    assert_eq!(
        support::record(&client, "task"),
        Some(&before_attach),
        "the baseline is the current record, including its recency and versions"
    );

    // Live blocked kind is a new result, not a routine text refresh.
    let pumped = support::run(
        &mut session,
        &support::reports(
            &[Status::new("blocked")
                .id("task")
                .kind("permission")
                .message("need approval")],
            "true",
        ),
    );
    let blocked = update(&pumped);
    assert_eq!(blocked.result_version(), 1, "entering blocked is a result");
    support::apply_frames(&mut client, &pumped.frames);
    let task = support::record(&client, "task").unwrap();
    assert_eq!(task.state(), ProgramState::Blocked);
    assert_eq!(task.kind(), Some(BlockedKind::Permission));
    assert_eq!(task.message(), Some("need approval"));

    // Alternate screen and DECSTR are not resets; a following report must not
    // replace the earlier record's identity.
    let pumped = support::run(
        &mut session,
        &support::reports(&[Status::new("done").id("task").title("First")], "true"),
    );
    support::apply_frames(&mut client, &pumped.frames);
    assert_eq!(
        support::record(&client, "task").unwrap().title(),
        Some("First")
    );
    let other = Status::new("done").id("keep").title("Other");
    let pumped = support::run(
        &mut session,
        &format!(
            "printf '\\033[?1049h\\033[?1049l\\033[!p'; {}",
            other.command()
        ),
    );
    support::apply_frames(&mut client, &pumped.frames);
    assert!(
        support::record(&client, "task").is_some(),
        "alternate screen and DECSTR keep records"
    );
    assert!(support::record(&client, "keep").is_some());

    // RIS clears every record without resetting the session lifetime.
    let pumped = support::run(&mut session, "printf '\\033c'");
    let reset = update(&pumped);
    assert!(reset.semantic_version() > client.semantic_version());
    support::apply_frames(&mut client, &pumped.frames);
    assert!(client.records().next().is_none(), "RIS clears every record");
    assert_eq!(
        client.session_id(),
        Some(session_id.as_str()),
        "clear keeps the same session lifetime"
    );

    // Shell death drops an idle record before Exited reaches the consumer.
    let pumped = support::run(
        &mut session,
        &support::reports(&[Status::new("idle").id("left")], "true"),
    );
    support::apply_frames(&mut client, &pumped.frames);
    assert!(
        support::record(&client, "left").is_some(),
        "idle is retained while live"
    );
    support::send_line(&mut session, "exit");
    let (pumped, code) = support::wait_exit(&mut session);
    assert_eq!(code, Some(0));
    support::apply_frames(&mut client, &pumped.frames);
    assert!(
        support::record(&client, "left").is_none(),
        "exit clears transient state"
    );
    assert_eq!(
        client.session_id(),
        Some(session_id.as_str()),
        "lifetime survives the shell death it clears"
    );
}

#[test]
fn a_peer_without_the_opt_in_never_sees_program_status_frames() {
    let (daemon, mut controller, mut client) = fresh();
    let pane = controller.pane_id();

    // An old peer: it negotiates and observes, but never sends the new opt-in.
    let mut old = transport::connect_endpoint_at(&daemon.pane_endpoint()).unwrap();
    old.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    ClientMsg::Negotiate {
        protocol: PROTOCOL_VERSION,
    }
    .encode(&mut old)
    .unwrap();
    old.flush().unwrap();
    match DaemonMsg::read(&mut old).unwrap() {
        DaemonMsg::Version(DaemonVersion { .. }) => {}
        other => panic!("expected version, got {other:?}"),
    }
    ClientMsg::GraphicsCapacity {
        encoded_bytes: 64 << 20,
        decoded_bytes: 128 << 20,
        offers: false,
    }
    .encode(&mut old)
    .unwrap();
    ClientMsg::Observe {
        pane_id: pane,
        size: size(),
    }
    .encode(&mut old)
    .unwrap();
    old.flush().unwrap();

    // The current peer receives this exact valid report, so the negative gate
    // assertion below cannot pass just because the parser dropped the input.
    let pumped = support::run(
        &mut controller,
        &support::reports(
            &[Status::new("working").id("gated").title("Visible")],
            "true",
        ),
    );
    let delta = update(&pumped);
    assert_eq!(
        support::changed_ids(delta),
        vec!["gated"],
        "the current consumer must see the reported record"
    );
    support::apply_frames(&mut client, &pumped.frames);
    let gated = support::record(&client, "gated").expect("current peer applied the frame");
    assert_eq!(gated.title(), Some("Visible"));

    // Drain the old peer: it sees the same output bytes, because it observes
    // the same pane write, but never the frame kind it did not negotiate.
    let mut old_text = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    old.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    while Instant::now() < deadline {
        match DaemonMsg::read(&mut old) {
            Ok(DaemonMsg::ProgramStatus(_)) => {
                panic!("a peer that never opted in received a program-status frame")
            }
            Ok(DaemonMsg::Output(bytes)) | Ok(DaemonMsg::Snapshot(bytes)) => {
                old_text.extend_from_slice(&bytes)
            }
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("old pane stream failed: {error}"),
        }
    }
    assert!(
        !old_text.is_empty(),
        "the old peer observed the same live pane the controller did"
    );
}

#[test]
fn two_observers_share_exactly_one_daemon_query_reply() {
    let (daemon, mut controller, _) = fresh();
    let pane = controller.pane_id();
    let mut first = daemon.panes().observe(pane, size()).unwrap();
    let mut second = daemon.panes().observe(pane, size()).unwrap();

    let reply =
        "\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x07";
    let reply_hex = reply
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    // Read exactly one reply and its hex, then prove no second reply is still
    // queued for the application. The bounded POSIX tty read returns zero
    // after 200 ms without bytes; raw mode also keeps a reply without a newline
    // readable. Only the od dump identifies the actual reply bytes.
    let command = format!(
        "LYREL_PS_TTY=$(stty -g); stty -echo -icanon min 1 time 0; printf '\\033]7501;?\\007'; dd bs=1 count={} 2>/dev/null | od -An -tx1 | tr -d ' \\n'; printf '\\nREMAINDER:'; stty min 0 time 2; dd bs=1 count=65536 2>/dev/null | wc -c; stty \"$LYREL_PS_TTY\"",
        reply.len()
    );
    let controller_seen = support::run(&mut controller, &command);

    fn reply_dumps(text: &[u8]) -> Vec<String> {
        text.split(|byte| *byte == b'\n')
            .filter(|line| line.windows(b"1b5d37".len()).any(|part| part == b"1b5d37"))
            .map(|line| String::from_utf8_lossy(line).into_owned())
            .collect()
    }

    let dumps = reply_dumps(&controller_seen.text);
    assert_eq!(dumps.len(), 1, "one od dump of one reply");
    assert!(
        dumps[0]
            .as_bytes()
            .windows(reply_hex.len())
            .any(|window| window == reply_hex.as_bytes()),
        "the od dump is the daemon's reply: {}",
        dumps[0]
    );
    assert!(
        controller_seen.contains(b"REMAINDER:0"),
        "the controller's application received no second reply: {:?}",
        String::from_utf8_lossy(&controller_seen.text)
    );

    // Each read-only observer sees the same pty output, so each must also hold
    // exactly one daemon answer, not one per attached consumer.
    for (index, observer) in [&mut first, &mut second].into_iter().enumerate() {
        let seen = support::pump_past_barrier(observer);
        let dumps = reply_dumps(&seen.text);
        assert_eq!(
            dumps.len(),
            1,
            "observer {index} saw the daemon answer more than once: {:?}",
            String::from_utf8_lossy(&seen.text)
        );
        assert!(
            dumps[0]
                .as_bytes()
                .windows(reply_hex.len())
                .any(|window| window == reply_hex.as_bytes()),
            "observer {index} never observed the answered query: {:?}",
            String::from_utf8_lossy(&seen.text)
        );
        assert!(
            seen.contains(b"REMAINDER:0"),
            "observer {index} left a second reply in the input queue: {:?}",
            String::from_utf8_lossy(&seen.text)
        );
    }
}

#[test]
fn whole_reports_replace_records_and_clear_absent_fields() {
    let (_daemon, mut session, mut client) = fresh();
    let pumped = support::run(
        &mut session,
        &support::reports(
            &[Status::new("working")
                .id("job")
                .app("agent")
                .title("Build")
                .message("Compiling")
                .progress(42)],
            "true",
        ),
    );
    support::apply_frames(&mut client, &pumped.frames);
    let before = ProgramRecord::clone(
        support::record(&client, "job").expect("the first report inserted the record"),
    );
    assert_eq!(before.progress(), Some(42));
    assert_eq!(before.app(), Some("agent"));
    assert_eq!(before.title(), Some("Build"));
    assert_eq!(before.message(), Some("Compiling"));

    // A whole replacement: absent keys clear, they are not "unchanged".
    let pumped = support::run(
        &mut session,
        &support::reports(&[Status::new("done").id("job")], "true"),
    );
    let delta = update(&pumped);
    assert!(
        delta.removed().is_empty(),
        "whole replacement does not remove the record"
    );
    assert_eq!(support::changed_ids(delta), vec!["job"]);
    assert_eq!(delta.semantic_version(), before.semantic_version() + 1);
    assert_eq!(
        delta.result_version(),
        before.result_version() + 1,
        "entering done is a result"
    );
    support::apply_frames(&mut client, &pumped.frames);

    let after = support::record(&client, "job").expect("the same id is still there");
    assert_eq!(after.state(), ProgramState::Done);
    assert_eq!(after.kind(), None);
    assert_eq!(after.app(), None);
    assert_eq!(after.title(), None);
    assert_eq!(after.message(), None);
    assert_eq!(after.progress(), None);
    assert!(after.updated() > before.updated());
    assert_eq!(after.semantic_version(), delta.semantic_version());
    assert_eq!(after.result_version(), delta.result_version());
}

#[test]
fn hierarchy_inherits_the_nearest_current_ancestor_app_and_republishes_it() {
    let (_daemon, mut session, mut client) = fresh();
    let pumped = support::run(
        &mut session,
        &support::reports(
            &[
                Status::new("working").app("rootapp"),
                Status::new("working").id("task"),
                Status::new("blocked").id("task/sub").kind("question"),
            ],
            "true",
        ),
    );
    support::apply_frames(&mut client, &pumped.frames);
    assert_eq!(
        support::record(&client, "").unwrap().app(),
        Some("rootapp"),
        "the root record keeps its own explicit app"
    );
    assert_eq!(
        support::record(&client, "task").unwrap().app(),
        Some("rootapp"),
        "the child inherits the root's app"
    );
    let sub = support::record(&client, "task/sub").unwrap();
    assert_eq!(sub.app(), Some("rootapp"));
    assert_eq!(sub.state(), ProgramState::Blocked);
    assert_eq!(sub.kind(), Some(BlockedKind::Question));

    // Replacing an ancestor's app changes every descendant whose effective app
    // comes from it; the same delta must carry them and the same new tick.
    let pumped = support::run(
        &mut session,
        &support::reports(&[Status::new("working").app("nextapp")], "true"),
    );
    let delta = update(&pumped);
    assert_eq!(
        support::changed_ids(delta),
        vec!["", "task", "task/sub"],
        "an ancestor app change republishes every effective descendant"
    );
    for record in delta.changed() {
        assert_eq!(record.app(), Some("nextapp"));
        assert_eq!(record.semantic_version(), delta.semantic_version());
    }
    support::apply_frames(&mut client, &pumped.frames);
    assert_eq!(
        support::record(&client, "task/sub").unwrap().app(),
        Some("nextapp")
    );

    // A nearer explicit ancestor wins over the root.
    let pumped = support::run(
        &mut session,
        &support::reports(&[Status::new("working").id("task").app("tool")], "true"),
    );
    let delta = update(&pumped);
    assert_eq!(support::changed_ids(delta), vec!["task", "task/sub"]);
    for record in delta.changed() {
        assert_eq!(record.app(), Some("tool"));
        assert_eq!(record.semantic_version(), delta.semantic_version());
    }
    support::apply_frames(&mut client, &pumped.frames);
    assert_eq!(
        support::record(&client, "").unwrap().app(),
        Some("nextapp"),
        "the root is not republished for a descendant's app"
    );
    assert_eq!(
        support::record(&client, "task/sub").unwrap().app(),
        Some("tool")
    );

    // The record's own app is nearer than every ancestor.
    let pumped = support::run(
        &mut session,
        &support::reports(&[Status::new("working").id("task/sub").app("own")], "true"),
    );
    support::apply_frames(&mut client, &pumped.frames);
    assert_eq!(
        support::record(&client, "task/sub").unwrap().app(),
        Some("own")
    );

    // Subtree clear removes the addressed path and everything under it.
    let pumped = support::run(
        &mut session,
        &support::reports(&[Status::new("clear").id("task")], "true"),
    );
    let delta = update(&pumped);
    assert_eq!(delta.removed().to_vec(), collection(&["task", "task/sub"]));
    support::apply_frames(&mut client, &pumped.frames);
    assert!(support::record(&client, "task").is_none());
    assert!(support::record(&client, "task/sub").is_none());
    assert!(support::record(&client, "").is_some(), "the root remains");

    // A clear with no id is the root clear.
    let pumped = support::run(
        &mut session,
        &support::reports(&[Status::new("clear")], "true"),
    );
    let delta = update(&pumped);
    assert_eq!(delta.removed().to_vec(), collection(&[""]));
    assert!(
        delta.semantic_version() > client.semantic_version(),
        "removing current records advances semantic authority"
    );
    support::apply_frames(&mut client, &pumped.frames);
    assert!(client.records().next().is_none());
}

#[test]
fn malformed_reports_and_field_limits_preserve_the_prior_record_and_stream() {
    let (_daemon, mut session, mut client) = fresh();
    let pumped = support::run(
        &mut session,
        &support::reports(
            &[
                Status::new("idle").app("rootapp"),
                Status::new("working").id("keeper").title("Keep"),
            ],
            "true",
        ),
    );
    support::apply_frames(&mut client, &pumped.frames);
    let keeper = ProgramRecord::clone(
        support::record(&client, "keeper")
            .expect("the valid record is the baseline for the rejects"),
    );
    let root = ProgramRecord::clone(support::record(&client, "").unwrap());
    let (semantic, result) = (client.semantic_version(), client.result_version());

    let overlong_id = "a".repeat(33);
    let too_deep = "a/b/c/d/e/f/g/h/i";
    let overlong_app = "a".repeat(33);
    let overlong_title = "A".repeat(300);
    let rejects = [
        support::raw_command("state=bogus:id=keeper"),
        support::raw_command("state=idle:id=bad//id"),
        support::raw_command(&format!("state=idle:id={overlong_id}")),
        support::raw_command(&format!("state=idle:id={too_deep}")),
        support::raw_command(&format!("state=idle:id=keeper:app={overlong_app}")),
        support::raw_command(&format!("state=idle:id=keeper:title={overlong_title}")),
        support::raw_command("state=idle:id=keeper:msg=AQ=="),
        support::raw_command("id=keeper"),
        support::raw_command("state=idle:id=keeper:this_key_is_way_too_long=1"),
        support::raw_command("state=clear:id=bad//id"),
    ];
    let pumped = support::run(&mut session, &rejects.join("; "));
    assert!(
        pumped.frames.is_empty(),
        "rejected reports must not publish anything: {:?}",
        support::updates(&pumped.frames)
    );

    // An identical refresh is the authoritative recency delta: it has to show
    // the untouched projection with the old versions, not one the rejects
    // could have changed.
    let pumped = support::run(
        &mut session,
        &support::reports(&[Status::new("working").id("keeper").title("Keep")], "true"),
    );
    let delta = update(&pumped);
    assert_eq!(support::changed_ids(delta), vec!["keeper"]);
    assert!(delta.removed().is_empty());
    assert_eq!(delta.semantic_version(), semantic);
    assert_eq!(delta.result_version(), result);
    support::apply_frames(&mut client, &pumped.frames);
    let refreshed = support::record(&client, "keeper").unwrap();
    assert_eq!(refreshed.state(), keeper.state());
    assert_eq!(refreshed.kind(), keeper.kind());
    assert_eq!(refreshed.app(), keeper.app());
    assert_eq!(refreshed.title(), keeper.title());
    assert_eq!(refreshed.message(), keeper.message());
    assert_eq!(refreshed.progress(), keeper.progress());
    assert_eq!(refreshed.semantic_version(), keeper.semantic_version());
    assert_eq!(refreshed.result_version(), keeper.result_version());
    assert!(refreshed.updated() > keeper.updated());
    assert_eq!(support::record(&client, ""), Some(&root));
    assert_eq!(client.semantic_version(), semantic);
    assert_eq!(client.result_version(), result);
    let keeper_after_refresh = ProgramRecord::clone(refreshed);

    // A later valid report still advances the stream.
    let pumped = support::run(
        &mut session,
        &support::reports(&[Status::new("done").id("after").title("Resumed")], "true"),
    );
    let delta = update(&pumped);
    assert!(delta.semantic_version() > semantic);
    support::apply_frames(&mut client, &pumped.frames);
    assert!(support::record(&client, "after").is_some());
    assert_eq!(
        support::record(&client, "keeper"),
        Some(&keeper_after_refresh),
        "an unrelated later report leaves the prior record untouched"
    );
}

#[test]
fn a_full_table_evicts_the_least_recently_updated_record_and_reinherits_its_child() {
    let (daemon, mut session, mut client) = fresh();
    let pumped = support::run(
        &mut session,
        &support::reports(
            &[
                Status::new("idle").app("rootapp"),
                Status::new("done").id("a").app("tool").title("Release"),
                Status::new("working").id("a/b"),
            ],
            "true",
        ),
    );
    support::apply_frames(&mut client, &pumped.frames);
    let old_a = ProgramRecord::clone(support::record(&client, "a").unwrap());
    assert_eq!(support::record(&client, "a/b").unwrap().app(), Some("tool"));

    // 252 fillers take the table to 255 records through real reports.
    let mut fill = String::new();
    for index in 0..252 {
        fill.push_str(&format!(
            "printf '\\033]7501;state=idle:id=r{index:03}\\007'\n"
        ));
    }
    let script = daemon.dir_path().join("fill.sh");
    std::fs::write(&script, fill).unwrap();
    let pumped = support::run(&mut session, &format!(". '{}'", script.display()));
    support::apply_frames(&mut client, &pumped.frames);
    assert_eq!(client.records().count(), 255, "the cap is not reached yet");

    // Refreshing the oldest record is a recency-only publish; it protects the
    // root from eviction and must not advance either counter.
    let semantic = client.semantic_version();
    let result = client.result_version();
    let pumped = support::run(
        &mut session,
        &support::reports(&[Status::new("idle").app("rootapp")], "true"),
    );
    let delta = update(&pumped);
    assert_eq!(support::changed_ids(delta), vec![""]);
    assert_eq!(delta.semantic_version(), semantic);
    assert_eq!(delta.result_version(), result);
    support::apply_frames(&mut client, &pumped.frames);
    assert!(
        support::record(&client, "").unwrap().updated()
            > support::record(&client, "r251").unwrap().updated()
    );

    let pumped = support::run(
        &mut session,
        &support::reports(&[Status::new("idle").id("r252")], "true"),
    );
    support::apply_frames(&mut client, &pumped.frames);
    assert_eq!(client.records().count(), 256);

    // The next insertion evicts `a`: the oldest update, not the refreshed
    // root. Its child survives and falls back to the next current ancestor.
    let pumped = support::run(
        &mut session,
        &support::reports(&[Status::new("idle").id("r253")], "true"),
    );
    let delta = update(&pumped);
    assert_eq!(delta.removed().to_vec(), collection(&["a"]));
    assert_eq!(
        support::changed_ids(delta),
        vec!["a/b", "r253"],
        "the surviving child's inherited app change is republished"
    );
    let child = delta
        .changed()
        .iter()
        .find(|record| record.id() == "a/b")
        .expect("the child is in the eviction delta");
    assert_eq!(child.app(), Some("rootapp"));
    assert_eq!(child.semantic_version(), delta.semantic_version());
    support::apply_frames(&mut client, &pumped.frames);
    assert!(support::record(&client, "a").is_none());
    assert_eq!(
        support::record(&client, "a/b").unwrap().app(),
        Some("rootapp")
    );
    assert_eq!(client.records().count(), 256);

    // Keep the child alive while recreating the evicted result record; the
    // recency refresh of `a/b` is published as its own single-record delta.
    let pumped = support::run(
        &mut session,
        &support::reports(&[Status::new("working").id("a/b")], "true"),
    );
    let delta = update(&pumped);
    assert_eq!(support::changed_ids(delta), vec!["a/b"]);
    assert_eq!(delta.semantic_version(), client.semantic_version());
    support::apply_frames(&mut client, &pumped.frames);

    let pumped = support::run(
        &mut session,
        &support::reports(
            &[Status::new("done").id("a").app("tool").title("Release")],
            "true",
        ),
    );
    let delta = update(&pumped);
    assert_eq!(
        delta.removed().to_vec(),
        collection(&["r000"]),
        "the recreated record still evicts the least recently updated filler"
    );
    assert_eq!(support::changed_ids(delta), vec!["a", "a/b"]);
    support::apply_frames(&mut client, &pumped.frames);
    let recreated = support::record(&client, "a").unwrap();
    assert!(recreated.result_version() > old_a.result_version());
    assert_eq!(recreated.result_version(), delta.result_version());
    assert_eq!(recreated.app(), Some("tool"));
    assert_eq!(
        support::record(&client, "a/b").unwrap().app(),
        Some("tool"),
        "recreating the ancestor republishes the child's inherited app"
    );
    assert_eq!(client.records().count(), 256);
}

#[test]
fn identical_refresh_publishes_recency_without_new_semantic_or_result() {
    let (_daemon, mut session, mut client) = fresh();
    let pumped = support::run(
        &mut session,
        &support::reports(
            &[
                Status::new("idle").id("a"),
                Status::new("idle").id("b"),
                Status::new("idle").id("c"),
            ],
            "true",
        ),
    );
    support::apply_frames(&mut client, &pumped.frames);
    let semantic = client.semantic_version();
    let result = client.result_version();
    assert_eq!(
        client.representative().map(ProgramRecord::id),
        Some("c"),
        "the latest update wins the same-state tie"
    );
    let before = ProgramRecord::clone(support::record(&client, "a").unwrap());

    // Every identical report is a recency-only delta: one changed record, the
    // same semantic/result counters, and a strictly newer authoritative
    // update timestamp. Even a refresh of the current representative must be
    // visible, so a later removal cannot fall back to a stale tie winner.
    for id in ["a", "b", "c", "c"] {
        let prior = ProgramRecord::clone(support::record(&client, id).unwrap());
        let pumped = support::run(
            &mut session,
            &support::reports(&[Status::new("idle").id(id)], "true"),
        );
        let delta = update(&pumped);
        assert_eq!(support::changed_ids(delta), vec![id]);
        assert!(delta.removed().is_empty());
        assert_eq!(delta.semantic_version(), semantic);
        assert_eq!(delta.result_version(), result);
        support::apply_frames(&mut client, &pumped.frames);
        let refreshed = support::record(&client, id).unwrap();
        assert_eq!(
            refreshed.semantic_version(),
            prior.semantic_version(),
            "a recency refresh never invents a semantic change"
        );
        assert_eq!(
            refreshed.result_version(),
            prior.result_version(),
            "a recency refresh never invents a result"
        );
        assert!(refreshed.updated() > prior.updated());
        assert_eq!(client.semantic_version(), semantic);
        assert_eq!(client.result_version(), result);
        assert_eq!(
            client.representative().map(ProgramRecord::id),
            Some(id),
            "the effective tie winner follows the authoritative recency"
        );
    }
    assert_eq!(client.representative().map(ProgramRecord::id), Some("c"));
    assert!(support::record(&client, "c").unwrap().updated() > before.updated());
}

#[test]
fn a_disconnected_done_working_done_cycle_is_visible_in_a_fresh_baseline() {
    let (daemon, mut session, mut client) = fresh();
    let pumped = support::run(
        &mut session,
        &support::reports(
            &[Status::new("done")
                .id("cyc")
                .app("agent")
                .title("Cycle")
                .message("finished")],
            "true",
        ),
    );
    support::apply_frames(&mut client, &pumped.frames);
    let before =
        ProgramRecord::clone(support::record(&client, "cyc").expect("the first done cycle landed"));
    assert!(before.result_version() > 0);
    let session_id = client.session_id().unwrap().to_string();
    let pane = session.pane_id();
    session.detach().unwrap();

    // No consumer is attached while the shell runs done -> working -> done.
    let cycle = format!(
        "{}; {}; printf 'CYCLE%s\\n' _DONE",
        Status::new("working")
            .id("cyc")
            .app("agent")
            .title("Cycle")
            .message("finished")
            .progress(80)
            .command(),
        Status::new("done")
            .id("cyc")
            .app("agent")
            .title("Cycle")
            .message("finished")
            .command()
    );
    daemon
        .panes()
        .send_input(pane, format!("{cycle}\r").as_bytes())
        .expect("drive the detached shell");

    let mut session = daemon.panes().attach(pane, size()).unwrap();
    let (mut pumped, baseline) = support::pump_baseline(&mut session);
    client.apply(&baseline);
    if !pumped.contains(b"CYCLE_DONE") {
        let completion = support::pump_until_text(&mut session, b"CYCLE_DONE");
        support::apply_frames(&mut client, &completion.frames);
        pumped.absorb(completion);
    }
    let tail = support::settle(&mut session);
    support::apply_frames(&mut client, &tail.frames);

    assert_eq!(client.session_id(), Some(session_id.as_str()));
    let after = support::record(&client, "cyc").expect("the record survived the cycle");
    assert_eq!(after.state(), ProgramState::Done);
    assert_eq!(after.app(), before.app());
    assert_eq!(after.title(), before.title());
    assert_eq!(after.message(), before.message());
    assert_eq!(after.progress(), before.progress());
    assert!(
        after.semantic_version() > before.semantic_version(),
        "the disconnected cycle changed the effective projection"
    );
    assert!(
        after.result_version() > before.result_version(),
        "a re-entered done is a new result even when the payload is identical"
    );
    assert!(after.updated() > before.updated());
    assert!(client.result_version() > before.result_version());
}

#[test]
fn clear_and_reset_recreation_never_reuse_a_result_version() {
    let (_daemon, mut session, mut client) = fresh();
    let generate = || support::reports(&[Status::new("done").id("x").title("Gen")], "true");

    let pumped = support::run(&mut session, &generate());
    support::apply_frames(&mut client, &pumped.frames);
    let first =
        ProgramRecord::clone(support::record(&client, "x").expect("the first done record exists"));
    assert!(first.result_version() > 0);
    let session_id = client.session_id().unwrap().to_string();

    // Clear the id: the version high-water survives even though the record does
    // not, so recreating the identical payload is a new result.
    let pumped = support::run(
        &mut session,
        &support::reports(&[Status::new("clear").id("x")], "true"),
    );
    let delta = update(&pumped);
    assert_eq!(delta.removed().to_vec(), collection(&["x"]));
    assert_eq!(delta.result_version(), first.result_version());
    support::apply_frames(&mut client, &pumped.frames);
    assert!(support::record(&client, "x").is_none());

    let pumped = support::run(&mut session, &generate());
    support::apply_frames(&mut client, &pumped.frames);
    let second = ProgramRecord::clone(support::record(&client, "x").unwrap());
    assert!(second.result_version() > first.result_version());

    // RIS clears all records but keeps the counters; recreating is a third
    // distinct result, never an alias of the first.
    let pumped = support::run(&mut session, "printf '\\033c'");
    let delta = update(&pumped);
    assert_eq!(delta.removed().to_vec(), collection(&["x"]));
    assert_eq!(delta.result_version(), second.result_version());
    support::apply_frames(&mut client, &pumped.frames);
    assert!(client.records().next().is_none());

    let pumped = support::run(&mut session, &generate());
    support::apply_frames(&mut client, &pumped.frames);
    let third = ProgramRecord::clone(support::record(&client, "x").unwrap());
    assert!(third.result_version() > second.result_version());
    assert_eq!(third.result_version(), client.result_version());
    assert_eq!(client.session_id(), Some(session_id.as_str()));
}

#[test]
fn a_local_prompt_clears_transient_records_but_a_relayed_prompt_does_not() {
    let (daemon, mut session, mut client) = fresh();
    let pumped = support::run(
        &mut session,
        &support::reports(
            &[
                Status::new("idle").id("idle"),
                Status::new("working").id("relay"),
                Status::new("blocked").id("block").kind("permission"),
                Status::new("done").id("done"),
                Status::new("error").id("err"),
            ],
            "true",
        ),
    );
    support::apply_frames(&mut client, &pumped.frames);
    assert_eq!(client.records().count(), 5);
    let semantic = client.semantic_version();
    let result = client.result_version();

    // A foreground child owns the pty while it emits an OSC 133 prompt mark.
    // The mark is relayed, so the pane must not clean anything. The child holds
    // the foreground until this test releases it, so the attribution is not a
    // race with the child's exit.
    let hold = daemon.dir_path().join("relay.go");
    std::fs::write(&hold, b"hold").unwrap();
    let relayed = daemon.dir_path().join("relay.bin");
    std::fs::write(&relayed, b"\x1b]133;A\x07RELAY_ARMED\n").unwrap();
    let inner = format!(
        "cat '{}'; while [ -e '{}' ]; do sleep 0.01; done; printf 'RELAY_DONE\\n'",
        relayed.display(),
        hold.display()
    );
    support::send_line(&mut session, &format!("/bin/sh -c \"{inner}\""));
    let armed = support::pump_until_text(&mut session, b"RELAY_ARMED");
    assert!(
        armed.frames.is_empty(),
        "no frame can precede the daemon's settle"
    );
    let settled = support::settle(&mut session);
    assert!(
        settled.frames.is_empty(),
        "a relayed prompt mark must not clean records: {:?}",
        support::updates(&settled.frames)
    );
    assert_eq!(client.records().count(), 5);

    std::fs::remove_file(&hold).unwrap();
    let finished = support::pump_until_text(&mut session, b"RELAY_DONE");
    assert!(finished.frames.is_empty());
    let settled = support::settle(&mut session);
    assert!(settled.frames.is_empty());
    assert_eq!(client.records().count(), 5);

    // A builtin in the local shell keeps the shell as the foreground group, so
    // this mark is a real local prompt: transient records go, done/error stay.
    let pumped = support::run(&mut session, "printf '\\033]133;A\\007'");
    let delta = update(&pumped);
    assert_eq!(
        delta.removed().to_vec(),
        collection(&["block", "idle", "relay"]),
        "a local prompt drops idle/working/blocked"
    );
    assert!(delta.changed().is_empty(), "no inherited app changes here");
    assert!(delta.semantic_version() > semantic);
    assert_eq!(delta.result_version(), result);
    support::apply_frames(&mut client, &pumped.frames);
    assert!(support::record(&client, "idle").is_none());
    assert!(support::record(&client, "relay").is_none());
    assert!(support::record(&client, "block").is_none());
    assert!(support::record(&client, "done").is_some());
    assert!(support::record(&client, "err").is_some());
}

#[test]
fn attached_exit_clears_transient_records_and_a_new_shell_starts_a_new_lifetime() {
    let (daemon, mut session, mut client) = fresh();
    let pumped = support::run(
        &mut session,
        &support::reports(
            &[
                Status::new("idle").id("idle"),
                Status::new("working").id("relay"),
                Status::new("blocked").id("block").kind("auth"),
                Status::new("done").id("done"),
                Status::new("error").id("err"),
            ],
            "true",
        ),
    );
    support::apply_frames(&mut client, &pumped.frames);
    let old_session = client.session_id().unwrap().to_string();

    support::send_line(&mut session, "exit");
    let (pumped, code) = support::wait_exit(&mut session);
    assert_eq!(code, Some(0));
    let delta = update(&pumped);
    assert_eq!(
        delta.removed().to_vec(),
        collection(&["block", "idle", "relay"]),
        "the attached exit drops only the transient states"
    );
    support::apply_frames(&mut client, &pumped.frames);
    assert!(support::record(&client, "done").is_some());
    assert!(support::record(&client, "err").is_some());

    let mut next = daemon.spawn_shell();
    let (_, baseline) = support::pump_baseline(&mut next);
    let mut next_client = ProgramStatusClient::default();
    assert!(next_client.apply(&baseline));
    assert_ne!(
        next_client.session_id(),
        Some(old_session.as_str()),
        "a new shell is a new session lifetime"
    );
    assert!(
        next_client.records().next().is_none(),
        "a new lifetime starts without the old records"
    );

    // Reusing the same id and payload cannot alias the old result: the
    // lifetime identity differs, and the new counter starts on its own.
    let pumped = support::run(
        &mut next,
        &support::reports(&[Status::new("done").id("done").title("Again")], "true"),
    );
    support::apply_frames(&mut next_client, &pumped.frames);
    let record = support::record(&next_client, "done").expect("the new shell owns a new record");
    assert_eq!(record.result_version(), 1);
    assert_ne!(next_client.session_id(), Some(old_session.as_str()));
}
