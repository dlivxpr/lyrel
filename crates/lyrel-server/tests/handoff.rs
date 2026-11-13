//! The daemon replaces its own binary and the shells never notice.
//!
//! Everything else about a handoff can be checked in a unit test — the blob
//! round-trips, the flags parse, the ids keep climbing. None of that answers
//! the only question that matters, which is whether the process on the other
//! end of the pty is still the same process afterwards. So this runs a real
//! daemon, puts a real shell in a real pty, and asks the shell.
//!
//! The proof is a variable. `lyrel_kept=…` lives in that shell's memory and
//! nowhere else: no file has it, the daemon has never seen it, and a shell
//! started after the handoff would answer with an empty string. Getting the
//! value back is something only the original process can do.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use lyrel_core::client::PaneClient;
use lyrel_core::daemon::protocol::{DaemonMsg, PaneProgress, ProgressKind, ShellSpec, WinSize};

const READY_WITHIN: Duration = Duration::from_secs(30);
const STREAM_WITHIN: Duration = Duration::from_secs(30);

struct Daemon {
    child: Child,
    dir: tempfile::TempDir,
}

impl Daemon {
    fn start() -> Daemon {
        let dir = tempfile::TempDir::new().unwrap();
        let child = Command::new(Self::binary())
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

    fn binary() -> PathBuf {
        PathBuf::from(env!("CARGO_BIN_EXE_lyrel-server"))
    }

    fn panes(&self) -> PaneClient {
        PaneClient::at(self.dir.path().join("daemon.sock"))
    }

    /// The pid the daemon records for itself. An `exec` keeps it; stopping and
    /// starting cannot.
    fn recorded_pid(&self) -> Option<u32> {
        std::fs::read_to_string(self.dir.path().join("daemon.pid"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    /// A fresh uuid per program image: it is minted on first use and lives in
    /// memory, so it survives anything except being replaced. Together with the
    /// pid it pins down exactly what happened — same pid and a new instance is
    /// an `exec` and nothing else.
    fn instance(&self) -> String {
        self.panes()
            .version()
            .expect("the daemon answers its version")
            .instance
    }

    fn await_ready(&self) {
        let deadline = Instant::now() + READY_WITHIN;
        loop {
            if self.panes().version().is_ok() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the daemon did not open its pane endpoint within {READY_WITHIN:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
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

fn interactive_shell() -> ShellSpec {
    ShellSpec {
        program: "/bin/sh".into(),
        args: Vec::new(),
        args_are_lyrel_defaults: false,
    }
}

fn windows_contain(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn collect_until(session: &mut lyrel_core::client::PaneSession, marker: &[u8]) -> Vec<u8> {
    let mut seen: Vec<u8> = Vec::new();
    loop {
        match session.recv() {
            Ok(DaemonMsg::Output(bytes)) | Ok(DaemonMsg::Snapshot(bytes)) => {
                seen.extend_from_slice(&bytes);
                if windows_contain(&seen, marker) {
                    return seen;
                }
            }
            Ok(DaemonMsg::Exited { code }) => panic!(
                "the pane exited ({code:?}) before {:?} appeared; saw {:?}",
                String::from_utf8_lossy(marker),
                String::from_utf8_lossy(&seen)
            ),
            Ok(_) => {}
            Err(e) => panic!(
                "the pane stream ended early: {e}; saw {:?}",
                String::from_utf8_lossy(&seen)
            ),
        }
    }
}

/// A small valid `glyf` outline — one closed quad — the only payload format
/// this build accepts. The pane stores these exact bytes, so a fresh
/// consumer can be held to them rather than to anything the sender said.
fn glyph_outline(width: i16, height: i16) -> Vec<u8> {
    let mut bytes = Vec::new();
    for word in [1, 0, 0, width, height, 3, 0] {
        bytes.extend_from_slice(&word.to_be_bytes());
    }
    bytes.extend_from_slice(&[1; 4]);
    for delta in [0, width, 0, -width, 0, 0, height, 0] {
        bytes.extend_from_slice(&delta.to_be_bytes());
    }
    bytes
}

fn base64(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();
    for chunk in bytes.chunks(3) {
        let bits = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        encoded.push(DIGITS[((bits >> 18) & 63) as usize] as char);
        encoded.push(DIGITS[((bits >> 12) & 63) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            DIGITS[((bits >> 6) & 63) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            DIGITS[(bits & 63) as usize] as char
        } else {
            '='
        });
    }
    encoded
}

/// One complete `r` request — `ESC _ 25a1 ; r ; … ESC \` — as a producer
/// would emit it. `layout` is the §8.5 sizing tail when the registration
/// declares one; absent keys take the protocol's defaults.
fn glyph_registration(
    cp: u32,
    glyf: &[u8],
    width: u8,
    layout: Option<&str>,
    reply: u8,
) -> Vec<u8> {
    let layout = layout.map_or_else(String::new, |params| format!(";{params}"));
    format!(
        "\x1b_25a1;r;cp={cp:x};fmt=glyf;upm=1000;width={width};reply={reply}{layout};{}\x1b\\",
        base64(glyf)
    )
    .into_bytes()
}

#[test]
fn a_handoff_keeps_the_process_the_pty_and_the_shell_that_is_on_it() {
    let daemon = Daemon::start();
    let panes = daemon.panes();
    let before_pid = daemon.recorded_pid().expect("the daemon records its pid");
    let before_instance = daemon.instance();

    let mut session = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .expect("spawn an interactive pane");
    let pane_id = session.pane_id();
    session
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");

    // Put something in this shell's memory that exists nowhere else, and print
    // a marker so we know the shell has read its input before we hand over.
    session
        .input(b"lyrel_kept=survivor; echo lyrel_before_$lyrel_kept\r")
        .expect("the shell takes input");
    collect_until(&mut session, b"lyrel_before_survivor");

    // One shell command emits progress once, then forces those bytes (and a
    // one-shot notification) out of the production 8 MiB ring. Error survives
    // a reliable command-end mark too; no later command starts before handoff.
    let expected_progress = Some(PaneProgress::new(ProgressKind::Error, Some(47)));
    session
        .input(b"printf '\\033]9;4;2;47\\007\\033]9;lyrel_handoff_old_notice\\007'; i=0; while [ \"$i\" -lt 8300 ]; do printf '%01024d\\n' \"$i\"; i=$((i + 1)); done; printf 'lyrel_trimmed_%s\\n' \"$lyrel_kept\"\r")
        .expect("the original shell produces one progress update and a finite flood");
    loop {
        match session.recv().expect("bound the live progress observation") {
            DaemonMsg::Progress(state) => {
                assert_eq!(
                    state, expected_progress,
                    "observe live authority, not OSC text"
                );
                break;
            }
            DaemonMsg::Exited { code } => panic!("the producer exited before progress: {code:?}"),
            _ => {}
        }
    }
    // This new live barrier follows the flood; the old marker is deliberately
    // no longer in the ring. The shell now waits for input without resending.
    collect_until(&mut session, b"lyrel_trimmed_survivor");
    drop(session);

    panes
        .hand_off(&Daemon::binary())
        .expect("the daemon hands over");
    daemon.await_ready();

    assert_eq!(
        daemon.recorded_pid(),
        Some(before_pid),
        "an exec keeps the process; a different pid here would mean the daemon stopped and \
         started, which is the thing this is supposed to avoid"
    );
    assert_ne!(
        daemon.instance(),
        before_instance,
        "and a *new image* has to be what is answering — without this the test would pass just \
         as well if the handoff had quietly done nothing at all"
    );

    let mut session = panes
        .attach(pane_id, size())
        .expect("the pane is still there under the same id");
    session
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");

    // Fresh attachment must restore authority after the real exec even though
    // the progress sequence itself is no longer available for text replay.
    let mut replayed = Vec::new();
    loop {
        match session
            .recv()
            .expect("bound the post-exec progress baseline")
        {
            DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => replayed.extend(bytes),
            DaemonMsg::Progress(state) => {
                assert_eq!(
                    state, expected_progress,
                    "the adopted live pane keeps current progress"
                );
                break;
            }
            DaemonMsg::Exited { code } => {
                panic!("the adopted pane exited before its baseline: {code:?}")
            }
            _ => {}
        }
    }
    assert!(
        replayed.len() > 7 * 1024 * 1024,
        "the actual production ring must have filled"
    );
    assert!(
        windows_contain(&replayed, b"lyrel_trimmed_survivor"),
        "the pane came back without the live barrier it printed after eviction"
    );
    assert!(
        !windows_contain(&replayed, b"lyrel_before_survivor"),
        "the original marker must really have left the ring"
    );
    assert!(
        !windows_contain(&replayed, b"\x1b]9;4;2;47\x07"),
        "progress restoration must not depend on historical OSC bytes"
    );
    assert!(
        !windows_contain(&replayed, b"lyrel_handoff_old_notice"),
        "old notifications must not be replayed to a fresh consumer"
    );

    // And the variable. Only the shell that ran the first line can answer this;
    // the command line echoes back unexpanded, so the expansion in the output
    // is unambiguous.
    session
        .input(b"echo lyrel_after_$lyrel_kept\r")
        .expect("the shell still takes input");
    collect_until(&mut session, b"lyrel_after_survivor");

    session
        .input(b"printf 'lyrel_adopted_final_%s\\n' \"$lyrel_kept\"; exit 9\r")
        .expect("the inherited child can finish normally");
    let mut final_output = Vec::new();
    loop {
        match session.recv().expect("inherited PTY drains before exit") {
            DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => final_output.extend(bytes),
            DaemonMsg::Exited { code } => {
                assert_eq!(
                    code,
                    Some(9),
                    "waitpid must preserve the inherited child's status"
                );
                assert!(
                    windows_contain(&final_output, b"lyrel_adopted_final_survivor"),
                    "the inherited PTY must deliver its final output before Exited"
                );
                break;
            }
            _ => {}
        }
    }

    session.kill().expect("kill the pane");
}

#[test]
fn a_handoff_to_something_that_will_not_exec_leaves_the_daemon_serving() {
    let daemon = Daemon::start();
    let panes = daemon.panes();
    let before_pid = daemon.recorded_pid().expect("the daemon records its pid");
    let before_instance = daemon.instance();

    let mut session = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .expect("spawn an interactive pane");
    let pane_id = session.pane_id();
    session
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");
    session
        .input(b"echo lyrel_still_here\r")
        .expect("the shell takes input");
    collect_until(&mut session, b"lyrel_still_here");

    let err = panes
        .hand_off(Path::new("/nonexistent/lyrel-that-is-not-there"))
        .expect_err("a binary that cannot be executed must be reported, not assumed");
    assert!(
        err.to_string().contains("cannot become"),
        "a missing binary is refused before anything is given up; the refusal was {err}"
    );

    // The state is staged before the exec and the exec is the last step, so a
    // failure at that step has to cost nothing at all.
    assert_eq!(daemon.recorded_pid(), Some(before_pid));
    assert_eq!(
        daemon.instance(),
        before_instance,
        "nothing was replaced, so the same image has to still be answering"
    );
    session
        .input(b"echo lyrel_unharmed\r")
        .expect("the pane still has its shell");
    collect_until(&mut session, b"lyrel_unharmed");
    assert_eq!(
        session.pane_id(),
        pane_id,
        "the pane the caller was holding is the pane it still holds"
    );

    session.kill().expect("kill the pane");
}

#[test]
fn a_handoff_keeps_the_pane_colour_overrides() {
    use lyrel_core::core::pane_colors::{ColorFrame, PaneColors};

    let daemon = Daemon::start();
    let panes = daemon.panes();
    let mut session = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .expect("spawn an interactive pane");
    let pane_id = session.pane_id();
    session
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");
    session
        .input(b"printf '\\033]4;1;#112233\\007\\033]11;#101112\\007'; echo lyrel_colors_set\r")
        .expect("the shell takes input");
    collect_until(&mut session, b"lyrel_colors_set");
    drop(session);

    panes
        .hand_off(&Daemon::binary())
        .expect("the daemon hands over");
    daemon.await_ready();

    let mut session = panes
        .attach(pane_id, size())
        .expect("the pane is still there under the same id");
    session
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");

    let mut colors = None;
    let mut saw_ring = false;
    let deadline = Instant::now() + STREAM_WITHIN;
    while colors.is_none() || !saw_ring {
        assert!(
            Instant::now() < deadline,
            "the handoff replay never delivered the colour projection"
        );
        match session.recv().expect("the adopted pane answers") {
            DaemonMsg::Colors(frame) => {
                let frame = ColorFrame::decode(&frame).expect("decodable colour snapshot");
                assert!(frame.is_snapshot(), "an attach carries a full snapshot");
                let mut state = PaneColors::default();
                frame.apply(&mut state);
                colors = Some(state);
            }
            DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => {
                saw_ring |= windows_contain(&bytes, b"lyrel_colors_set");
            }
            DaemonMsg::Exited { code } => panic!("pane exited after handoff: {code:?}"),
            _ => {}
        }
    }
    let colors = colors.expect("colour snapshot");
    assert_eq!(colors.get(1), Some([0x11, 0x22, 0x33]));
    assert_eq!(
        colors.get(lyrel_core::core::pane_colors::DEFAULT_BG),
        Some([0x10, 0x11, 0x12]),
        "the same live shell keeps its decoded overrides across the exec"
    );
    session.kill().expect("kill the pane");
}

/// The glossary has to cross the exec with the pane, like the shell on it.
/// Both are proven the same way: only the original shell knows `lyrel_kept`,
/// and only a carried glossary can hand a fresh consumer the replacement
/// definition after the registration bytes were forced out of the ring.
#[test]
fn a_handoff_keeps_the_pane_glyph_glossary() {
    use lyrel_core::core::glyph_protocol::GlyphFrame;

    let daemon = Daemon::start();
    let panes = daemon.panes();
    let before_pid = daemon.recorded_pid().expect("the daemon records its pid");
    let before_instance = daemon.instance();

    let mut session = panes
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .expect("spawn an interactive pane");
    let pane_id = session.pane_id();
    session
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");

    session
        .input(b"lyrel_kept=survivor; echo lyrel_before_$lyrel_kept\r")
        .expect("the shell takes input");
    collect_until(&mut session, b"lyrel_before_survivor");

    // A real producer — `cat` on bytes it never saw before — registers one
    // definition, then replaces it with a declared §8.5 layout, then exits.
    // reply=1 asks the daemon for both ACKs: the replies go to the pane's
    // tty, and the shell echo of `status=0` is the live acceptance evidence.
    const CP: u32 = 0x100000;
    let original = glyph_outline(1000, 1000);
    let replacement = glyph_outline(2000, 500);
    let mut produced = glyph_registration(CP, &original, 1, None, 1);
    produced.extend(glyph_registration(
        CP,
        &replacement,
        2,
        Some("aw=1800;lh=1400;ymin=-100;size=cover;align=start,baseline;pad=0.05,0.1,0.15,0.2"),
        1,
    ));
    let glyphs_file = daemon.dir.path().join("glyphs.bin");
    std::fs::write(&glyphs_file, &produced).unwrap();
    let command = format!(
        "cat '{}'; echo lyrel_glyph_sender_exit_$?\r",
        glyphs_file.display()
    );
    session
        .input(command.as_bytes())
        .expect("the registration producer runs");
    // The ACK echo and the exit marker share no ordering — the tty echoes a
    // reply the moment the daemon writes it, before `cat` has finished — so
    // the drain has to accept either first.
    let mut live = Vec::new();
    let live_deadline = Instant::now() + STREAM_WITHIN;
    while !(windows_contain(&live, b"lyrel_glyph_sender_exit_0")
        && windows_contain(&live, b"cp=100000;status=0"))
    {
        assert!(
            Instant::now() < live_deadline,
            "the producer's output or the daemon's ACK never arrived"
        );
        match session.recv().expect("the pane answers") {
            DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => live.extend(bytes),
            DaemonMsg::Exited { code } => panic!("the pane exited during registration: {code:?}"),
            _ => {}
        }
    }
    assert!(
        windows_contain(&live, b"\x1b_25a1;r;cp=100000"),
        "an accepted body is re-emitted at its stream position, not silently kept"
    );

    // The echo of the answers sits in the shell's line buffer; kill the
    // pending line (^U) before the next command or it would run junk.
    // Then force every registration byte — and those echoes — out of the
    // 8 MiB ring with output that carries no protocol content of its own.
    session
        .input(b"\x15i=0; while [ \"$i\" -lt 8400 ]; do printf \"\\033[3$((i % 8))m%01024d\\033[0m\\n\" \"$i\"; i=$((i + 1)); done; printf 'lyrel_trimmed_%s\\n' \"$lyrel_kept\"\r")
        .expect("the flood runs on the same shell");
    collect_until(&mut session, b"lyrel_trimmed_survivor");
    drop(session);

    panes
        .hand_off(&Daemon::binary())
        .expect("the daemon hands over");
    daemon.await_ready();

    assert_eq!(
        daemon.recorded_pid(),
        Some(before_pid),
        "an exec keeps the process; a different pid means a stop and start"
    );
    assert_ne!(
        daemon.instance(),
        before_instance,
        "a new image has to answer, or the exec never happened"
    );

    // A fresh negotiated attach opts in to the glossary projection
    // (FEATURE_GLYPH_STATE), so it receives exactly one authoritative frame
    // after the replayed ring.
    let mut session = panes
        .attach(pane_id, size())
        .expect("the pane is still there under the same id");
    session
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");

    let mut replayed = Vec::new();
    let mut glyph_frames: Vec<GlyphFrame> = Vec::new();
    let deadline = Instant::now() + STREAM_WITHIN;
    while glyph_frames.is_empty() || !windows_contain(&replayed, b"lyrel_trimmed_survivor") {
        assert!(
            Instant::now() < deadline,
            "the handoff replay never delivered the glossary projection"
        );
        match session.recv().expect("the adopted pane answers") {
            DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => replayed.extend(bytes),
            DaemonMsg::GlyphState(frame) => {
                glyph_frames
                    .push(GlyphFrame::decode(&frame).expect("decodable glossary snapshot"));
            }
            DaemonMsg::Exited { code } => panic!("pane exited after handoff: {code:?}"),
            _ => {}
        }
    }
    assert!(
        replayed.len() > 7 * 1024 * 1024,
        "the actual production ring must have filled"
    );
    assert!(
        !windows_contain(&replayed, b"\x1b_25a1"),
        "every registration APC and echoed reply must really have left the ring"
    );
    assert!(
        !windows_contain(&replayed, b"cp=100000;status=0"),
        "the historical register ACK is ring text, not restored state"
    );
    assert_eq!(
        glyph_frames.len(),
        1,
        "one full snapshot, not a resend of the producer's bytes"
    );
    let frame = &glyph_frames[0];
    assert_eq!(
        frame.generation(),
        2,
        "two accepted mutations carried, not replayed"
    );
    assert_eq!(
        frame.glyphs().len(),
        1,
        "the retained glossary must hold exactly the replacement: {frame:?}"
    );
    let glyph = &frame.glyphs()[0];
    assert_eq!(
        (glyph.cp, glyph.upm, glyph.width),
        (CP, 1000, 2),
        "the projected definition is the replacement, not the original"
    );
    assert_eq!(glyph.glyf, replacement);
    assert_eq!(
        format!("{:?}", glyph.layout),
        "GlyphLayout { aw: Some(1800), lh: Some(1400), ymin: Some(-100), size: Cover, align_h: Start, align_v: Baseline, pad: [0.05, 0.1, 0.15, 0.2] }",
        "the declared extent, alignment, baseline and padding survive verbatim"
    );

    // The exec must not re-answer old requests: the continuing shell sees
    // neither a re-emitted registration nor a second ACK after replay.
    session
        .input(b"\x15echo lyrel_after_$lyrel_kept\r")
        .expect("the shell still takes input");
    let after = collect_until(&mut session, b"lyrel_after_survivor");
    assert!(
        !windows_contain(&after, b"\x1b_25a1"),
        "restored state is projected to consumers, never re-emitted as protocol"
    );
    assert!(
        !windows_contain(&after, b"status=0"),
        "the historical register ACK is not re-sent to the surviving shell"
    );

    session.kill().expect("kill the pane");
}
