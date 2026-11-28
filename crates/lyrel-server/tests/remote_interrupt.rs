//! #433: the same foreground task through a native SSH shell channel and a
//! daemon-owned remote workspace. The observation boundary is the pane wire.
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lyrel_core::client::ControlClient;
use lyrel_core::core::machine::{PaneSeed, TabId};
use lyrel_core::core::session::WorkspaceId;
use lyrel_core::daemon::control::{ControlHello, ControlRequest, ReplyOk};
use lyrel_core::daemon::protocol::{self, ClientMsg, DaemonMsg, ShellSpec};
use lyrel_core::daemon::transport::{self, Stream};
use lyrel_core::host::{Host, remote::RemoteHost};
use lyrel_test_support::native_ssh::{NativeSshFixture, status, win_size};
use serde::Deserialize;
use serde_json::json;

#[path = "remote_interrupt/traffic.rs"]
mod traffic;

const TASK: &[u8] = include_bytes!("fixtures/remote_interrupt.mjs");
const WITHIN: Duration = Duration::from_secs(15);
const INTERRUPT_AFTER: u64 = 3072;
const ROW_BYTES: u64 = 64; // Including N_TTY's CRLF.
const DATA: &str = "LYREL433_DATA ";
const BOOT: &str = "LYREL433_BOOT";
const PROMPT: &str = "LYREL433_PROMPT";
const TOKEN: &str = "lyrel-433-interrupt";
const HOSTNAME: &str = "dedicated SSH interrupt client";
const ACK: &str = "LYREL433_ACK ";
const EXIT: &str = "LYREL433_EXIT ";

#[derive(Clone, Copy, PartialEq)]
enum Source {
    Any,
    Live,
    Replay,
}

struct Pane {
    stream: Stream,
    id: u64,
    buffered: Vec<u8>,
    offset: usize,
    source: Source,
    replay_bytes: usize,
}

impl Pane {
    fn new(stream: Stream, id: u64) -> Self {
        stream.set_read_timeout(Some(WITHIN)).unwrap();
        stream.set_write_timeout(Some(WITHIN)).unwrap();
        Self {
            stream,
            id,
            buffered: Vec::new(),
            offset: 0,
            source: Source::Any,
            replay_bytes: 0,
        }
    }

    fn spawn(mut stream: Stream, request: ClientMsg) -> Self {
        request.encode(&mut stream).unwrap();
        let id = match DaemonMsg::read(&mut stream).unwrap() {
            DaemonMsg::Spawned { pane_id } => pane_id,
            other => panic!("expected real Spawned, got {other:?}"),
        };
        Self::new(stream, id)
    }

    fn send(&mut self, bytes: Vec<u8>) {
        ClientMsg::Input(bytes).encode(&mut self.stream).unwrap();
    }

    fn line(&mut self, source: Source, deadline: Instant) -> &str {
        if source != self.source {
            self.buffered.clear();
            self.offset = 0;
            self.source = source;
        }
        loop {
            assert!(Instant::now() < deadline, "pane {} text deadline", self.id);
            if let Some(length) = self.buffered[self.offset..]
                .iter()
                .position(|b| *b == b'\n')
            {
                let line = &self.buffered[self.offset..self.offset + length];
                self.offset += length + 1;
                // Startup banners need not be UTF-8. Fixture records are ASCII.
                let line = std::str::from_utf8(line).unwrap_or("");
                return if line.starts_with(DATA) {
                    line
                } else {
                    line.trim_end_matches('\r')
                };
            }
            self.buffered.drain(..self.offset);
            self.offset = 0;
            assert!(self.buffered.len() < 64 << 10, "unterminated PTY line");
            let bytes = match DaemonMsg::read(&mut self.stream).expect("bounded real pane stream") {
                DaemonMsg::Output(bytes) if source != Source::Replay => bytes,
                DaemonMsg::Snapshot(bytes) if source != Source::Live => {
                    self.replay_bytes += bytes.len();
                    bytes
                }
                DaemonMsg::Error(reason) => panic!("pane rejected request: {reason}"),
                DaemonMsg::AuthPrompt { .. } => {
                    panic!("dedicated pinned endpoint prompted for auth")
                }
                DaemonMsg::Exited { code } => panic!("shell exited unexpectedly: {code:?}"),
                DaemonMsg::SshStatus {
                    phase: protocol::SshPhase::Failed { reason },
                } => panic!("native SSH failed: {reason}"),
                _ => continue,
            };
            if self.buffered.is_empty() {
                self.buffered = bytes;
            } else {
                self.buffered.extend(bytes);
            }
        }
    }

    fn expect(&mut self, source: Source, deadline: Instant, expected: &str) {
        loop {
            let line = self.line(source, deadline);
            if line == expected {
                break;
            }
            assert!(
                ![DATA, ACK, EXIT]
                    .iter()
                    .any(|&prefix| line.starts_with(prefix))
                    && !(expected.starts_with(EXIT) && line == PROMPT),
                "expected {expected}, got out-of-order PTY record {line:?}"
            );
        }
    }

    fn setup(&mut self, home: &str) -> u32 {
        let deadline = Instant::now() + WITHIN;
        self.expect(Source::Any, deadline, BOOT);
        self.send(
            b"unset HISTFILE; stty -echo isig intr '^C' noflsh opost onlcr || exit; \
              lyrel433_shell=$$; PS1='\\nLYREL433_PROMPT\\n'; \
              printf '\\nLYREL433_%s %s %s\\n' SHELL \"$lyrel433_shell\" \"$HOME\"\n"
                .to_vec(),
        );
        let pid = loop {
            if let Some(fields) = self
                .line(Source::Live, deadline)
                .strip_prefix("LYREL433_SHELL ")
            {
                let (pid, observed_home) = fields.split_once(' ').unwrap();
                assert_eq!(observed_home, home);
                break pid.parse().unwrap();
            }
        };
        self.expect(Source::Live, deadline, PROMPT);
        pid
    }

    fn probe(&mut self, deadline: Instant, serial: &str, shell: u32) {
        // The echoed command cannot produce a matching whole-line result.
        self.send(
            format!("printf '\\nLYREL433_%s %s %s\\n' PROBE {serial} \"$lyrel433_shell\"\n")
                .into_bytes(),
        );
        self.expect(
            Source::Live,
            deadline,
            &format!("LYREL433_PROBE {serial} {shell}"),
        );
        self.expect(Source::Live, deadline, PROMPT);
    }

    fn attach(mut stream: Stream, id: u64) -> Self {
        ClientMsg::Attach {
            pane_id: id,
            size: win_size(),
            allow_remote_clipboard_write: false,
        }
        .encode(&mut stream)
        .unwrap();
        Self::new(stream, id)
    }

    fn assert_disconnected(&mut self) {
        let deadline = Instant::now() + WITHIN;
        let lost = loop {
            assert!(Instant::now() < deadline, "old pane stream did not close");
            if let Err(err) = DaemonMsg::read(&mut self.stream) {
                break err;
            }
        };
        assert!(
            matches!(
                lost.kind(),
                io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::BrokenPipe
            ),
            "expected old pane stream to close: {lost}"
        );
    }
}

fn endpoint(fixture: &NativeSshFixture, name: &str) -> PathBuf {
    let suffix = if cfg!(windows) { "port" } else { "sock" };
    fixture.local_config_dir().join(format!("{name}.{suffix}"))
}

fn router_pid(fixture: &NativeSshFixture) -> u32 {
    let local = ControlClient::connect_at(
        &endpoint(fixture, "control"),
        &ControlHello::host_rpc(TOKEN, "local isolated router"),
    )
    .unwrap();
    match local.request(ControlRequest::Status).unwrap() {
        ReplyOk::Status(status) => status.pid,
        other => panic!("expected local router status, got {other:?}"),
    }
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn local_stream(fixture: &NativeSshFixture) -> Stream {
    let stream = transport::connect_endpoint_at(&endpoint(fixture, "daemon")).unwrap();
    stream.set_read_timeout(Some(WITHIN)).unwrap();
    stream.set_write_timeout(Some(WITHIN)).unwrap();
    stream
}

fn native_pane_stream(fixture: &NativeSshFixture) -> Stream {
    let mut stream = local_stream(fixture);
    protocol::negotiate_pane(&mut stream).unwrap();
    ClientMsg::GraphicsCapacity {
        encoded_bytes: 0,
        decoded_bytes: 0,
        offers: false,
    }
    .encode(&mut stream)
    .unwrap();
    stream
}

fn native_pane(fixture: &NativeSshFixture) -> Pane {
    let stream = native_pane_stream(fixture);
    let mut spec = fixture.spec();
    spec.login_script = vec![format!(
        "exec /usr/bin/env PS1='\\n{BOOT}\\n' PROMPT_COMMAND= /bin/bash --noprofile --norc -i"
    )];
    Pane::spawn(
        stream,
        ClientMsg::SpawnNativeSsh {
            cwd: Some(fixture.remote_home().into()),
            size: win_size(),
            spec: Box::new(spec),
        },
    )
}

struct OwnedWorkspace {
    host: Arc<RemoteHost>,
    id: WorkspaceId,
    tab: TabId,
}

impl OwnedWorkspace {
    fn create(host: Arc<RemoteHost>, name: &str) -> Self {
        let workspace = Self {
            host,
            id: WorkspaceId::new(),
            tab: TabId::new(),
        };
        let reply = workspace
            .host
            .client()
            .call(ControlRequest::WorkspaceCreate {
                name: Some(name.into()),
                workspace: Some(workspace.id),
            })
            .unwrap();
        assert!(matches!(reply, ReplyOk::WorkspaceTree(_)));
        workspace.claim();
        workspace
    }

    fn claim(&self) {
        let reply = self
            .host
            .client()
            .call(ControlRequest::WorkspaceAttach {
                id: self.id.to_string(),
            })
            .unwrap();
        assert!(matches!(
            reply,
            ReplyOk::Attached {
                took_over_from: None
            }
        ));
    }

    fn assert_tree(&self, pane: u64) {
        let reply = self
            .host
            .client()
            .call(ControlRequest::WorkspaceTree { workspace: self.id })
            .unwrap();
        let ReplyOk::WorkspaceTree(tree) = reply else {
            panic!("missing remote workspace tree");
        };
        assert_eq!(tree.id, self.id);
        let [tab] = tree.tabs.as_slice() else {
            panic!("expected one owned tab");
        };
        assert_eq!(tab.id, self.tab);
        assert!(tab.root.contains(pane));
        assert_eq!(tree.attachment.as_ref().unwrap().hostname, HOSTNAME);
    }
}

impl Drop for OwnedWorkspace {
    fn drop(&mut self) {
        let _ = self
            .host
            .client()
            .call(ControlRequest::WorkspaceRemove { workspace: self.id });
    }
}

fn workspace_pane(fixture: &NativeSshFixture, workspace: &OwnedWorkspace) -> (Pane, String) {
    let (stream, version) = fixture.connect_pane();
    assert!(!version.instance.is_empty());
    let pane = Pane::spawn(
        stream,
        ClientMsg::Spawn {
            cwd: Some(fixture.remote_home().into()),
            size: win_size(),
            shell: Some(ShellSpec {
                program: "/usr/bin/env".into(),
                args: vec![
                    format!("PS1=\\n{BOOT}\\n"),
                    "PROMPT_COMMAND=".into(),
                    "/bin/bash".into(),
                    "--noprofile".into(),
                    "--norc".into(),
                    "-i".into(),
                ],
                args_are_lyrel_defaults: false,
            }),
            owner: None,
            workspace: Some(workspace.id.to_string()),
            restore: None,
            allow_remote_clipboard_write: false,
            await_claim: false,
        },
    );
    fixture.track_pane(pane.id);
    let reply = workspace
        .host
        .client()
        .call(ControlRequest::TabCreate {
            workspace: workspace.id,
            at: None,
            pane: PaneSeed::bare(pane.id),
            tab: Some(workspace.tab),
        })
        .unwrap();
    assert!(matches!(reply, ReplyOk::TabTree(_)));
    workspace.assert_tree(pane.id);
    (pane, version.instance)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Receipt {
    sample: String,
    signal: String,
    pid: u32,
    pgrp: u32,
    foreground: u32,
    lines: u64,
    started_ns: String,
    receipt_ns: String,
    resource_usage: serde_json::Value,
}

fn check_row(row: &str, serial: &str, expected: u64) {
    let (sample, rest) = row.split_once(' ').expect("numbered sample");
    let (number, payload) = rest.split_once(' ').expect("numbered data row");
    assert_eq!(sample, serial);
    assert_eq!(number.len(), 6);
    assert_eq!(
        number.parse::<u64>().unwrap(),
        expected,
        "#433 task row lost, duplicated or reordered"
    );
    assert_eq!(payload, "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\r");
    assert_eq!((DATA.len() + row.len() + 1) as u64, ROW_BYTES);
}

fn wait_begin(pane: &mut Pane, source: Source, deadline: Instant, serial: &str) -> u32 {
    let prefix = format!("LYREL433_BEGIN {serial} ");
    loop {
        if let Some(pid) = pane.line(source, deadline).strip_prefix(&prefix) {
            return pid.parse().unwrap();
        }
    }
}

struct LastSample {
    serial: String,
    pid: u32,
    rows: u64,
}

fn replay_sample(pane: &mut Pane, last: &LastSample, shell: u32, deadline: Instant) {
    let source = Source::Replay;
    assert_eq!(wait_begin(pane, source, deadline, &last.serial), last.pid);
    for expected in 1..=last.rows {
        let line = pane.line(source, deadline);
        check_row(
            line.strip_prefix(DATA).expect("retained numbered row"),
            &last.serial,
            expected,
        );
    }
    pane.expect(
        source,
        deadline,
        &format!("{ACK}{} {}", last.serial, last.rows),
    );
    pane.expect(source, deadline, &format!("{EXIT}{} 130", last.serial));
    pane.expect(source, deadline, PROMPT);
    pane.expect(
        source,
        deadline,
        &format!("LYREL433_PROBE {} {shell}", last.serial),
    );
    pane.expect(source, deadline, PROMPT);
}

fn observe_tasks(
    fixture: &NativeSshFixture,
    host: &RemoteHost,
    pane: &mut Pane,
    task: &ShellSpec,
    route: &str,
    samples: usize,
    shell: u32,
) -> LastSample {
    let command = format!("{} {}", quote(&task.program), quote(&task.args[0]));
    let mut acks = Vec::with_capacity(samples);
    let mut prompts = Vec::with_capacity(samples);
    let mut last = None;
    for sample in 0..=samples {
        let serial = format!("{sample:03}");
        let trace = fixture.remote_path(&format!("{route}-{serial}.json"));
        pane.send(
            format!(
                "{command} {serial} {}; printf '\\n{EXIT}%s %s\\n' {serial} \"$?\"\n",
                quote(trace.to_str().unwrap())
            )
            .into_bytes(),
        );
        let deadline = Instant::now() + WITHIN;
        let pid = wait_begin(pane, Source::Live, deadline, &serial);
        let ack = format!("{ACK}{serial} ");
        let mut lines = 0;
        let mut interrupt = None;
        let input_to_ack = loop {
            let line = pane.line(Source::Live, deadline);
            if let Some(row) = line.strip_prefix(DATA) {
                lines += 1;
                check_row(row, &serial, lines);
                if lines == INTERRUPT_AFTER {
                    interrupt = Some(Instant::now());
                    pane.send(vec![0x03]);
                }
            } else if let Some(count) = line.strip_prefix(&ack) {
                let received = Instant::now();
                assert_eq!(count.parse::<u64>().unwrap(), lines);
                break received - interrupt.expect("task ACK before Ctrl-C");
            } else {
                panic!("unexpected task output before ACK: {line:?}");
            }
        };
        pane.expect(Source::Live, deadline, &format!("{EXIT}{serial} 130"));
        let input_to_exit = interrupt.unwrap().elapsed();
        pane.expect(Source::Live, deadline, PROMPT);
        let input_to_prompt_output = interrupt.unwrap().elapsed();
        pane.probe(deadline, &serial, shell);
        let input_to_prompt = interrupt.unwrap().elapsed();

        // Read the independent host file only after local timing. The SIGINT
        // handler recorded its *own* monotonic time before writing the ACK.
        let evidence = host
            .read_file(&trace, 4096)
            .expect("remote SIGINT receipt file");
        let receipt: Receipt = serde_json::from_slice(&evidence).unwrap();
        assert_eq!(receipt.sample, serial);
        assert_eq!(receipt.signal, "SIGINT");
        assert_eq!(receipt.pid, pid);
        assert_eq!(receipt.pgrp, receipt.foreground);
        assert!(receipt.pgrp > 0);
        assert_eq!(receipt.lines, lines);
        let started = receipt.started_ns.parse::<u64>().unwrap();
        let received = receipt.receipt_ns.parse::<u64>().unwrap();
        let remote_ns = received.checked_sub(started).expect("remote clock order");
        assert!(remote_ns > 0);
        let bytes = ROW_BYTES * lines;
        println!(
            "SSH433 {}",
            json!({
                "event": "sample", "route": route, "sample": serial, "warmup": sample == 0,
                "pane": pane.id, "shell_pid": shell,
                "input_to_ack_ms": milliseconds(input_to_ack),
                "input_to_exit_status_ms": milliseconds(input_to_exit),
                "input_to_prompt_output_ms": milliseconds(input_to_prompt_output),
                "input_to_usable_prompt_ms": milliseconds(input_to_prompt),
                "ordered_rows": lines, "pty_bytes": bytes,
                "remote_task_avg_pty_bytes_per_sec": bytes as f64 * 1e9 / remote_ns as f64,
                "remote_receipt": {
                    "signal": receipt.signal, "task_pid": pid, "foreground_pgrp": receipt.foreground,
                    "task_started_ns": receipt.started_ns, "sigint_handler_ns": receipt.receipt_ns,
                    "task_resource_usage": receipt.resource_usage,
                },
            })
        );
        if sample > 0 {
            acks.push(input_to_ack);
            prompts.push(input_to_prompt);
        }
        last = Some(LastSample {
            serial,
            pid,
            rows: lines,
        });
    }
    println!(
        "SSH433 {}",
        json!({
            "event": "summary", "route": route, "samples": samples,
            "input_to_ack_p50_ms": percentile(&mut acks, 50),
            "input_to_ack_p95_ms": percentile(&mut acks, 95),
            "input_to_usable_prompt_p95_ms": percentile(&mut prompts, 95),
        })
    );
    last.unwrap()
}

fn milliseconds(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn percentile(samples: &mut [Duration], p: usize) -> f64 {
    samples.sort_unstable();
    milliseconds(samples[(samples.len() * p).div_ceil(100) - 1])
}

fn control(fixture: &NativeSshFixture) -> Arc<RemoteHost> {
    fixture.connect_control_as("fixture:ssh433", TOKEN, HOSTNAME)
}

fn local_binary() -> PathBuf {
    std::env::var_os("LYREL_TEST_SSH_INTERRUPT_LOCAL_SERVER").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_lyrel-server")),
        PathBuf::from,
    )
}

#[test]
#[ignore = "requires an explicitly authorized dedicated SSH endpoint and preconfigured pinned fixture"]
fn ssh_interrupt_and_workspace_reconnect() {
    let samples: usize = std::env::var("LYREL_TEST_SSH_INTERRUPT_SAMPLES")
        .map_or(1, |raw| raw.parse().expect("numeric sample count"));
    assert!((1..=100).contains(&samples));
    let ip_rtt = std::env::var("LYREL_TEST_SSH_INTERRUPT_NETWORK_RTT_MS")
        .ok()
        .map(|raw| {
            let rtt: f64 = raw.parse().expect("recorded network RTT in ms");
            assert!(rtt.is_finite() && rtt >= 0.0);
            rtt
        });
    let local_binary = local_binary();
    let mut fixture = NativeSshFixture::start(&local_binary);
    let host = control(&fixture);
    let remote_status = status(&host);
    let remote_instance = host.peer().instance.clone();
    let local_pid = router_pid(&fixture);
    assert_eq!(
        host.client().call(ControlRequest::Ping).unwrap(),
        ReplyOk::Pong
    );
    let mut rtt = [Duration::ZERO; 12];
    for elapsed in &mut rtt {
        let start = Instant::now();
        assert_eq!(
            host.client().call(ControlRequest::Ping).unwrap(),
            ReplyOk::Pong
        );
        *elapsed = start.elapsed();
    }
    println!(
        "SSH433 {}",
        json!({
            "event": "run", "local_router_pid": local_pid, "remote_daemon_pid": remote_status.pid,
            "remote_instance": remote_instance, "remote_build": host.peer().build,
            "local_server": local_binary, "debug_assertions": cfg!(debug_assertions),
            "control_version": remote_status.control_version, "pane_version": remote_status.protocol_version,
            "grid": [win_size().cols, win_size().rows], "samples_per_route": samples,
            "warmups_per_route": 1, "interrupt_at_row": INTERRUPT_AFTER,
            "target_pty_bytes_per_sec": 512_000, "network_rtt_ms": ip_rtt,
            "ssh_control_rtt_p50_ms": percentile(&mut rtt, 50),
            "ssh_control_rtt_p95_ms": percentile(&mut rtt, 95),
        })
    );
    let task = fixture.install_script(&host, "remote_interrupt.mjs", TASK, Vec::new());

    let mut native = native_pane(&fixture);
    let shell = native.setup(fixture.remote_home());
    let last = observe_tasks(
        &fixture,
        &host,
        &mut native,
        &task,
        "native-shell-channel",
        samples,
        shell,
    );
    let reattach_started = Instant::now();
    ClientMsg::Detach.encode(&mut native.stream).unwrap();
    native.assert_disconnected();
    let mut fresh_native = Pane::attach(native_pane_stream(&fixture), native.id);
    let deadline = Instant::now() + WITHIN;
    replay_sample(&mut fresh_native, &last, shell, deadline);
    fresh_native.probe(deadline, "reattach", shell);
    println!(
        "SSH433 {}",
        json!({
            "event": "reattach", "route": "native-shell-channel", "pane": native.id,
            "shell_pid": shell, "verified_replayed_rows": last.rows,
            "restore_to_usable_prompt_ms": milliseconds(reattach_started.elapsed()),
        })
    );
    let mut local = local_stream(&fixture);
    protocol::negotiate_pane(&mut local).unwrap();
    ClientMsg::Kill { pane_id: native.id }
        .encode(&mut local)
        .unwrap();
    drop(fresh_native);

    let mut workspace = OwnedWorkspace::create(host, "Dedicated SSH interrupt workload");
    let (mut remote, pane_instance) = workspace_pane(&fixture, &workspace);
    let shell = remote.setup(fixture.remote_home());
    let last = observe_tasks(
        &fixture,
        &workspace.host,
        &mut remote,
        &task,
        "server-workspace",
        samples,
        shell,
    );

    let reconnect_started = Instant::now();
    fixture.restart_router();
    remote.assert_disconnected();
    workspace.host = control(&fixture);
    assert_eq!(workspace.host.peer().instance, remote_instance);
    assert_eq!(status(&workspace.host).pid, remote_status.pid);
    workspace.claim();
    let (stream, version) = fixture.connect_pane();
    assert_eq!(version.instance, pane_instance);
    let mut reattached = Pane::attach(stream, remote.id);
    let deadline = Instant::now() + WITHIN;
    replay_sample(&mut reattached, &last, shell, deadline);
    reattached.probe(deadline, "reconnect", shell);
    let reconnect_to_prompt = reconnect_started.elapsed();
    workspace.assert_tree(reattached.id);
    println!(
        "SSH433 {}",
        json!({
            "event": "reconnect", "route": "server-workspace", "workspace": workspace.id, "tab": workspace.tab,
            "pane": reattached.id, "shell_pid": shell, "remote_instance": remote_instance,
            "remote_daemon_pid": remote_status.pid, "new_router_pid": router_pid(&fixture),
            "restore_to_usable_prompt_ms": milliseconds(reconnect_to_prompt),
            "replay_bytes_read": reattached.replay_bytes, "verified_replayed_rows": last.rows,
        })
    );
}
