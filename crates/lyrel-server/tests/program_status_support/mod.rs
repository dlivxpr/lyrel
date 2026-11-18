//! Shared plumbing for the public OSC 7501 consumer tests.
//!
//! `program_status.rs` and `handoff.rs` both drive a real daemon, a real PTY
//! and the released `PaneSession`/`ProgramStatusClient`. This module only
//! collects ordered daemon messages into text and frames; it never fabricates
//! a frame, reaches into pane state or parses OSC 7501 itself.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use lyrel_core::client::{PaneClient, PaneSession};
use lyrel_core::core::program_status::{
    ProgramDelta, ProgramRecord, ProgramStatusClient, ProgramStatusFrame,
};
use lyrel_core::daemon::protocol::DaemonMsg;

/// How long one consumer assertion may take before the test gives up. Every
/// read is bounded by [`POLL`]; the deadline bounds the whole wait. CI's
/// capacity-constrained macOS runners stretch a real daemon+PTY round trip
/// well past a local machine's seconds, so the bound is generous — it only
/// has to catch a wedged stream, not measure latency.
pub const STREAM_WITHIN: Duration = Duration::from_secs(120);

const POLL: Duration = Duration::from_millis(500);

/// A shell execution barrier. The input line the tty echoes never contains
/// the joined value: the closing `printf` joins two halves at run time, so
/// only an executed command can put `__lyrel_ps_barrier__` on the output
/// stream. A literal marker cannot distinguish an echo from execution, and a
/// platform shell may echo one input line before it executes anything: macOS
/// `/bin/sh` is bash, and input written before its prompt/readline is ready
/// can be echoed by the tty and redisplayed by the line editor. Requiring the
/// joined value once therefore proves execution and cannot be satisfied by an
/// echo, no matter how many times the platform redraws the pending line.
const BARRIER_VALUE_HEAD: &str = "__lyrel_ps_";
const BARRIER_VALUE_TAIL: &str = "barrier__";

fn barrier_value() -> String {
    format!("{BARRIER_VALUE_HEAD}{BARRIER_VALUE_TAIL}")
}

static SETTLE_SEQ: AtomicU64 = AtomicU64::new(1);

/// Ordered messages read from one consumer while waiting for a condition.
#[derive(Default)]
pub struct Pumped {
    pub text: Vec<u8>,
    pub frames: Vec<ProgramStatusFrame>,
}

impl Pumped {
    /// Keep both the text and the order in which frames arrived.
    pub fn absorb(&mut self, other: Pumped) {
        self.text.extend_from_slice(&other.text);
        self.frames.extend(other.frames);
    }

    pub fn contains(&self, needle: &[u8]) -> bool {
        contains(&self.text, needle)
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

pub fn send_line(session: &mut PaneSession, line: &str) {
    session
        .input(format!("{line}\r").as_bytes())
        .expect("write to the pty");
}

fn pump(session: &mut PaneSession, mut done: impl FnMut(&Pumped) -> bool) -> Pumped {
    let deadline = Instant::now() + STREAM_WITHIN;
    let mut pumped = Pumped::default();
    session
        .set_recv_timeout(Some(POLL))
        .expect("bound the stream reads");
    while !done(&pumped) {
        assert!(
            Instant::now() < deadline,
            "the pane stream did not reach the expected state within {STREAM_WITHIN:?}; \
             frames {:?} text {:?}",
            pumped.frames,
            String::from_utf8_lossy(&pumped.text)
        );
        match session.recv() {
            Ok(DaemonMsg::Output(bytes)) | Ok(DaemonMsg::Snapshot(bytes)) => {
                pumped.text.extend_from_slice(&bytes)
            }
            Ok(DaemonMsg::ProgramStatus(frame)) => pumped.frames.push(frame),
            Ok(DaemonMsg::Exited { code }) => panic!("the pane exited early: {code:?}"),
            Ok(DaemonMsg::Error(message)) => panic!("the daemon reported: {message}"),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("the pane stream failed: {error}"),
        }
    }
    pumped
}

pub fn pump_until_text(session: &mut PaneSession, needle: &[u8]) -> Pumped {
    pump(session, |pumped| pumped.contains(needle))
}

/// Wait for the value a [`run`] barrier prints. The joined value exists only
/// in an executed command's output, so one occurrence is execution; a
/// controller and every observer see that printed value once.
pub fn pump_past_barrier(session: &mut PaneSession) -> Pumped {
    let value = barrier_value();
    pump_until_text(session, value.as_bytes())
}

fn barrier(session: &mut PaneSession, command: &str) -> Pumped {
    let line = format!("{command}; printf '%s%s\\n' '{BARRIER_VALUE_HEAD}' '{BARRIER_VALUE_TAIL}'");
    send_line(session, &line);
    pump_past_barrier(session)
}

/// The daemon-side settle: a comment line is echoed by the tty as soon as it
/// is written, so seeing its echo proves the reader loop has consumed every
/// earlier byte. The barrier guarantees the shell already executed the command
/// before this line is written, so the echo lands after every byte whose
/// frames the settle must collect. The reader publishes one read's text and
/// then its frames before it reads the next chunk, and both travel on the same
/// ordered connection, so a later echo cannot overtake those frames.
pub fn settle(session: &mut PaneSession) -> Pumped {
    let token = format!(
        "lyrel_ps_settle_{}",
        SETTLE_SEQ.fetch_add(1, Ordering::Relaxed)
    );
    send_line(session, &format!("#{token}"));
    pump_until_text(session, token.as_bytes())
}

/// Run `command` in the pane's shell and collect every frame it produced.
///
/// The barrier proves the shell executed the whole line; the settle proves the
/// daemon published the frames from that read before the window closed.
pub fn run(session: &mut PaneSession, command: &str) -> Pumped {
    let mut pumped = barrier(session, command);
    pumped.absorb(settle(session));
    pumped
}

/// The attach/reconnect baseline, which must arrive before any live update.
pub fn pump_baseline(session: &mut PaneSession) -> (Pumped, ProgramStatusFrame) {
    let deadline = Instant::now() + STREAM_WITHIN;
    let mut pumped = Pumped::default();
    session
        .set_recv_timeout(Some(POLL))
        .expect("bound the stream reads");
    loop {
        assert!(
            Instant::now() < deadline,
            "no program-status baseline arrived; saw {:?}",
            String::from_utf8_lossy(&pumped.text)
        );
        match session.recv() {
            Ok(DaemonMsg::ProgramStatus(frame)) => match frame {
                ProgramStatusFrame::Baseline(snapshot) => {
                    let frame = ProgramStatusFrame::Baseline(snapshot);
                    pumped.frames.push(frame.clone());
                    return (pumped, frame);
                }
                ProgramStatusFrame::Update(_) => {
                    panic!("a live program-status update arrived before the attach baseline")
                }
                _ => {}
            },
            Ok(DaemonMsg::Output(bytes)) | Ok(DaemonMsg::Snapshot(bytes)) => {
                pumped.text.extend_from_slice(&bytes)
            }
            Ok(DaemonMsg::Exited { code }) => {
                panic!("the pane exited before its baseline: {code:?}")
            }
            Ok(DaemonMsg::Error(message)) => panic!("the daemon reported: {message}"),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("the pane stream failed: {error}"),
        }
    }
}

/// Read until the attached process reports its exit, keeping the ordered
/// program frames that precede it.
pub fn wait_exit(session: &mut PaneSession) -> (Pumped, Option<i32>) {
    let deadline = Instant::now() + STREAM_WITHIN;
    let mut pumped = Pumped::default();
    session
        .set_recv_timeout(Some(POLL))
        .expect("bound the stream reads");
    loop {
        assert!(
            Instant::now() < deadline,
            "the pane never reported its exit; saw {:?}",
            String::from_utf8_lossy(&pumped.text)
        );
        match session.recv() {
            Ok(DaemonMsg::Exited { code }) => return (pumped, code),
            Ok(DaemonMsg::Output(bytes)) | Ok(DaemonMsg::Snapshot(bytes)) => {
                pumped.text.extend_from_slice(&bytes)
            }
            Ok(DaemonMsg::ProgramStatus(frame)) => pumped.frames.push(frame),
            Ok(DaemonMsg::Error(message)) => panic!("the daemon reported: {message}"),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("the pane stream failed: {error}"),
        }
    }
}

pub fn apply_frames(client: &mut ProgramStatusClient, frames: &[ProgramStatusFrame]) -> bool {
    let mut changed = false;
    for frame in frames {
        changed |= client.apply(frame);
    }
    changed
}

pub fn record<'a>(client: &'a ProgramStatusClient, id: &str) -> Option<&'a ProgramRecord> {
    client.records().find(|record| record.id() == id)
}

pub fn delta(frame: &ProgramStatusFrame) -> Option<&ProgramDelta> {
    match frame {
        ProgramStatusFrame::Update(delta) => Some(delta),
        _ => None,
    }
}

pub fn updates(frames: &[ProgramStatusFrame]) -> Vec<&ProgramDelta> {
    frames.iter().filter_map(delta).collect()
}

pub fn changed_ids(delta: &ProgramDelta) -> Vec<&str> {
    delta.changed().iter().map(ProgramRecord::id).collect()
}

/// A watcher has to stop when the pane leaves the registry; an exited pane is
/// never addressable again.
pub fn wait_pane_gone(panes: &PaneClient, pane_id: u64) {
    let deadline = Instant::now() + STREAM_WITHIN;
    loop {
        let listed = panes.list().expect("list the daemon's panes");
        if !listed.iter().any(|pane| pane.pane_id == pane_id) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "pane {pane_id} stayed in the registry after it exited"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Standard base64, matching what OSC 7501 carries for `title`/`msg`.
pub fn b64(text: &str) -> String {
    b64_bytes(text.as_bytes())
}

pub fn b64_bytes(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
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

/// One OSC 7501 report as a producer would emit it. `title`/`message` are
/// base64-encoded here because that is what the released protocol carries.
#[derive(Clone, Debug)]
pub struct Status {
    body: String,
}

impl Status {
    pub fn new(state: &str) -> Status {
        Status {
            body: format!("state={state}"),
        }
    }

    pub fn pair(mut self, key: &str, value: &str) -> Status {
        self.body.push(':');
        self.body.push_str(key);
        self.body.push('=');
        self.body.push_str(value);
        self
    }

    pub fn id(self, id: &str) -> Status {
        self.pair("id", id)
    }

    pub fn app(self, app: &str) -> Status {
        self.pair("app", app)
    }

    pub fn kind(self, kind: &str) -> Status {
        self.pair("kind", kind)
    }

    pub fn progress(self, progress: u8) -> Status {
        self.pair("progress", &progress.to_string())
    }

    pub fn title(self, text: &str) -> Status {
        self.pair("title", &b64(text))
    }

    pub fn title_encoded(self, encoded: &str) -> Status {
        self.pair("title", encoded)
    }

    pub fn message(self, text: &str) -> Status {
        self.pair("msg", &b64(text))
    }

    pub fn message_encoded(self, encoded: &str) -> Status {
        self.pair("msg", encoded)
    }

    /// The exact `printf` a producer would run, terminated by BEL.
    pub fn command(&self) -> String {
        format!("printf '\\033]7501;{}\\007'", self.body)
    }
}

/// A raw OSC 7501 body, for the cases the released parser must reject whole.
pub fn raw_command(body: &str) -> String {
    format!("printf '\\033]7501;{body}\\007'")
}

/// A command line that emits every status and then runs `tail`.
pub fn reports(statuses: &[Status], tail: &str) -> String {
    let mut command = String::new();
    for status in statuses {
        command.push_str(&status.command());
        command.push_str("; ");
    }
    command.push_str(tail);
    command
}
