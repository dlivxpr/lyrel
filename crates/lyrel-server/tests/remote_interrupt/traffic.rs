//! #437: real Host control, native SSH SFTP/forward jobs, and foreground PTY input.
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpStream};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use lyrel_core::daemon::protocol::{
    ForwardStatus, ManagedForward, SftpEntryKind, SftpJobProgress, SftpJobState, SftpTransferKind,
    SftpTransferSpec, SshForwardKind, SshForwardRule, WorkspaceOp, WorkspaceRequest,
};
use serde_json::Value;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use super::{
    ClientMsg, ControlClient, ControlHello, ControlRequest, DaemonMsg, Host, NativeSshFixture,
    OwnedWorkspace, PROMPT, Pane, RemoteHost, ReplyOk, Source, TOKEN, WITHIN, WorkspaceId, control,
    endpoint, json, local_binary, local_stream, milliseconds, native_pane, percentile, protocol,
    status, win_size, workspace_pane,
};

const HOST_ROUNDS: usize = 24;
const TRANSFER_BYTES: usize = 32 << 20;
const FORWARD_BYTES: usize = 1 << 20;
const FORWARD_CHUNK: usize = 16 << 10;
const FORWARD_PAUSE: Duration = Duration::from_millis(4);
const WATCH_BODY: &[u8] = b"lyrel-437 named watch fixture\n";
const ECHO_SCRIPT: &[u8] = br#"
import { createServer } from 'node:net';
const server = createServer(socket => {
  socket.on('error', () => socket.destroy());
  socket.pipe(socket);
});
server.listen(0, '127.0.0.1', () => {
  console.log(`LYREL437_LISTENER ${server.address().port} ${process.pid}`);
});
"#;

struct Route<'a> {
    fixture: &'a NativeSshFixture,
    workspace: Option<WorkspaceId>,
    pane: u64,
}

impl Route<'_> {
    fn name(&self) -> &'static str {
        if self.workspace.is_some() {
            "server-workspace"
        } else {
            "native-shell-channel"
        }
    }

    fn call(&self, standalone: ClientMsg, workspace_op: impl FnOnce() -> WorkspaceOp) -> DaemonMsg {
        // The product's services run on the desktop router in both cases.
        // Only the workspace PTY and RemoteHost control use the remote daemon.
        let request = match self.workspace {
            Some(workspace) => ClientMsg::OnWorkspace(Box::new(WorkspaceRequest {
                workspace,
                spec: Box::new(self.fixture.spec()),
                view_pane: self.pane,
                op: workspace_op(),
            })),
            None => standalone,
        };
        let mut stream = local_stream(self.fixture);
        protocol::negotiate_pane(&mut stream).unwrap();
        request.encode(&mut stream).unwrap();
        DaemonMsg::read(&mut stream).expect("traffic RPC returned a live pane response")
    }

    fn job(&self, id: u64) -> SftpJobProgress {
        let reply = self.call(ClientMsg::SftpTransferList { pane_id: self.pane }, || {
            WorkspaceOp::SftpTransferList
        });
        let DaemonMsg::SftpTransferProgress(jobs) = reply else {
            panic!("{} SFTP job list: {reply:?}", self.name());
        };
        jobs.into_iter()
            .find(|job| job.job_id == id)
            .expect("started SFTP job is present in its own route")
    }

    fn verify_sftp_listing(&self, dir: &Path) {
        let path = dir.to_str().unwrap().to_owned();
        let reply = self.call(
            ClientMsg::SftpList {
                pane_id: self.pane,
                path: path.clone(),
            },
            || WorkspaceOp::SftpList { path },
        );
        let DaemonMsg::SftpEntries(entries) = reply else {
            panic!("{} SFTP directory list: {reply:?}", self.name());
        };
        let [file] = entries.as_slice() else {
            panic!("expected the uploaded fixture file, got {entries:?}");
        };
        assert_eq!(file.name, "payload.bin");
        assert_eq!(file.kind, SftpEntryKind::File);
        assert_eq!(file.size, TRANSFER_BYTES as u64);
    }

    fn add_forward(&self, target_port: u16) -> ManagedForward {
        let rule = SshForwardRule {
            kind: SshForwardKind::Local,
            bind_host: Ipv4Addr::LOCALHOST.to_string(),
            bind_port: 0,
            target_host: Ipv4Addr::LOCALHOST.to_string(),
            target_port,
            description: Some("#437 disposable loopback echo".into()),
            enabled: true,
        };
        let reply = self.call(
            ClientMsg::AddForward {
                pane_id: self.pane,
                rule: rule.clone(),
            },
            || WorkspaceOp::AddForward { rule },
        );
        let DaemonMsg::ForwardList(forwards) = reply else {
            panic!("{} managed forward add: {reply:?}", self.name());
        };
        let [forward] = forwards.as_slice() else {
            panic!("expected one fixture-owned forward, got {forwards:?}");
        };
        assert_eq!(forward.pane_id, self.pane);
        assert_eq!(forward.kind, SshForwardKind::Local);
        assert_eq!(forward.bind_host, "127.0.0.1");
        assert_eq!(forward.target_host, "127.0.0.1");
        assert_eq!(forward.target_port, target_port);
        assert!(forward.enabled);
        assert_eq!(forward.status, ForwardStatus::Listening);
        assert_ne!(forward.bind_port, 0);
        forward.clone()
    }

    fn remove_forward(&self, forward_id: u64) {
        let reply = self.call(
            ClientMsg::RemoveForward {
                pane_id: self.pane,
                forward_id,
            },
            || WorkspaceOp::RemoveForward { forward_id },
        );
        assert!(matches!(reply, DaemonMsg::ForwardList(list) if list.is_empty()));
    }
}

struct Probe {
    received: Instant,
    elapsed: std::time::Duration,
    input_bytes: usize,
    output_bytes: usize,
}

fn probe(pane: &mut Pane, serial: &str) -> Probe {
    let expected = format!("LYREL437_ECHO {serial}");
    // The PTY echo is off, and the sent command cannot contain the full response.
    let input = format!("printf '\\nLYREL437_%s %s\\n' ECHO {serial}\n").into_bytes();
    let input_bytes = input.len();
    let start = Instant::now();
    pane.send(input);
    let deadline = start + WITHIN;
    let received = loop {
        let line = pane.line(Source::Live, deadline);
        if line.starts_with("LYREL437_ECHO ") {
            assert_eq!(line, expected);
            break Instant::now();
        }
    };
    loop {
        let line = pane.line(Source::Live, deadline);
        assert!(
            !line.starts_with("LYREL437_ECHO "),
            "duplicate echo: {line}"
        );
        if line == PROMPT {
            break;
        }
    }
    Probe {
        received,
        elapsed: received - start,
        input_bytes,
        output_bytes: expected.len() + 2, // Linux PTY CRLF, excluding the leading blank line.
    }
}

fn host_load(host: &RemoteHost, dir: &Path, pane: &mut Pane, serial: &str) -> (Probe, Value) {
    host.create_dir(dir, false).unwrap();
    let watch = host.watch(&[dir.to_owned()]).unwrap();
    let names: Vec<_> = (0..HOST_ROUNDS)
        .map(|i| format!("watched-{i:02}.txt"))
        .collect();
    let watched = host.join(dir, names.last().unwrap());
    let (first, ready) = mpsc::sync_channel(1);
    let (probe, started, last_write, last_list, finished, list_time) = thread::scope(|scope| {
        let work = scope.spawn(|| {
            let started = Instant::now();
            let mut last_write = started;
            let mut last_list = started;
            let mut list_time = Duration::ZERO;
            for (i, name) in names.iter().enumerate() {
                last_write = Instant::now();
                host.write_file(&host.join(dir, name), WATCH_BODY).unwrap();
                last_list = Instant::now();
                let entries = host.read_dir(dir, None).unwrap();
                list_time += last_list.elapsed();
                assert_eq!(entries.len(), i + 1);
                for (entry, name) in entries.iter().zip(&names) {
                    assert_eq!(&entry.name, name);
                    assert!(!entry.is_dir && !entry.is_symlink);
                }
                if i == 0 {
                    first.send(()).unwrap();
                }
            }
            (started, last_write, last_list, Instant::now(), list_time)
        });
        ready
            .recv_timeout(WITHIN)
            .expect("first Host write and listing");
        let probe = probe(pane, serial);
        let (started, last_write, last_list, finished, list_time) = work.join().unwrap();
        (probe, started, last_write, last_list, finished, list_time)
    });
    assert!(
        probe.received < last_list,
        "Host listing workload ended before PTY echo"
    );
    let remaining = last_list - probe.received;
    let watch_deadline = last_write + WITHIN;
    let watch_received = loop {
        assert!(
            Instant::now() < watch_deadline,
            "no named watch event for {watched:?}"
        );
        match watch.events().try_recv() {
            Ok(paths) if paths.contains(&watched) => break Instant::now(),
            Ok(_) => continue,
            Err(_) => {
                assert!(!watch.events().is_closed(), "Host watch disconnected");
                thread::sleep(Duration::from_millis(2));
            }
        }
    };
    assert_eq!(host.read_file(&watched, 1024).unwrap(), WATCH_BODY);
    drop(watch);
    host.remove(dir, true).unwrap();
    (
        probe,
        json!({
            "file_host_route": "remote-host-control",
            "directory_listings": HOST_ROUNDS,
            "listed_entries": HOST_ROUNDS * (HOST_ROUNDS + 1) / 2,
            "list_call_total_ms": milliseconds(list_time),
            "acknowledged_write_bytes": HOST_ROUNDS * WATCH_BODY.len(),
            "verified_watch_file_bytes": WATCH_BODY.len(),
            "write_and_list_elapsed_ms": milliseconds(finished - started),
            "last_list_start_after_pty_response_ms": milliseconds(remaining),
            "named_watch_event": names.last().unwrap(),
            "final_write_to_watch_observation_ms": milliseconds(watch_received - last_write),
        }),
    )
}

fn transfer(
    route: &Route<'_>,
    pane: &mut Pane,
    serial: &str,
    kind: SftpTransferKind,
    local: &Path,
    remote: &Path,
) -> (Probe, Value) {
    let spec = SftpTransferSpec {
        pane_id: pane.id,
        kind,
        local: local.to_owned(),
        remote: remote.to_str().unwrap().to_owned(),
        recursive: false,
    };
    let start = Instant::now();
    let reply = route.call(ClientMsg::SftpTransferStart(spec.clone()), || {
        WorkspaceOp::SftpTransferStart { spec }
    });
    let DaemonMsg::SftpTransferStarted { job_id } = reply else {
        panic!("{} SFTP {kind:?} did not start: {reply:?}", route.name());
    };
    let deadline = start + WITHIN;
    let before = loop {
        assert!(
            Instant::now() < deadline,
            "SFTP {kind:?} never made progress"
        );
        let job = route.job(job_id);
        assert_eq!(
            job.state,
            SftpJobState::Running,
            "no in-flight SFTP {kind:?}: {job:?}"
        );
        if job.bytes_done > 0 && job.bytes_done < job.bytes_total {
            break job;
        }
        thread::yield_now();
    };
    let probe = probe(pane, serial);
    let after = route.job(job_id);
    assert_eq!(
        after.state,
        SftpJobState::Running,
        "SFTP {kind:?} finished before the post-echo observation: {after:?}"
    );
    assert!(after.bytes_done >= before.bytes_done);
    let (completed, elapsed) = loop {
        assert!(
            Instant::now() < deadline,
            "SFTP {kind:?} did not complete: {after:?}"
        );
        let job = route.job(job_id);
        match job.state {
            SftpJobState::Running => thread::sleep(Duration::from_millis(2)),
            SftpJobState::Done => break (job, start.elapsed()),
            _ => panic!("SFTP {kind:?} failed: {job:?}"),
        }
    };
    assert_eq!(completed.kind, kind);
    assert_eq!(completed.bytes_total, TRANSFER_BYTES as u64);
    assert_eq!(completed.bytes_done, TRANSFER_BYTES as u64);
    assert!(completed.error.is_none());
    (
        probe,
        json!({
            "job": job_id,
            "bytes": completed.bytes_done,
            "bytes_done_before_input": before.bytes_done,
            "bytes_done_after_response": after.bytes_done,
            "request_to_done_observation_ms": milliseconds(elapsed),
            "verified_payload_bytes_per_second": completed.bytes_done as f64 / elapsed.as_secs_f64(),
        }),
    )
}

fn verify_upload(host: &RemoteHost, remote: &Path, expected: &[u8]) {
    let mut source = host.open_read(remote).unwrap();
    assert_eq!(source.meta().len, expected.len() as u64);
    let mut offset = 0;
    while offset < expected.len() {
        let bytes = source.read_segment(1 << 20).unwrap();
        assert!(!bytes.is_empty());
        let end = offset + bytes.len();
        assert!(
            expected.get(offset..end) == Some(bytes.as_slice()),
            "uploaded bytes differ at segment offset {offset}"
        );
        offset = end;
    }
    assert!(source.read_segment(1).unwrap().is_empty());
}

fn verify_download(local: &Path, expected: &[u8]) {
    let mut source = fs::File::open(local).unwrap();
    assert_eq!(source.metadata().unwrap().len(), expected.len() as u64);
    let mut buffer = vec![0; 256 << 10];
    for (index, chunk) in expected.chunks(buffer.len()).enumerate() {
        let bytes = &mut buffer[..chunk.len()];
        source.read_exact(bytes).unwrap();
        assert!(
            &*bytes == chunk,
            "downloaded bytes differ in segment {index}"
        );
    }
}

fn forward_load(port: u16, pane: &mut Pane, serial: &str, payload: &[u8]) -> (Probe, Value) {
    let (first, ready) = mpsc::sync_channel(1);
    let (probe, last_write, elapsed) = thread::scope(|scope| {
        let work = scope.spawn(|| {
            let address = (Ipv4Addr::LOCALHOST, port).into();
            let mut socket = TcpStream::connect_timeout(&address, WITHIN).unwrap();
            socket.set_nodelay(true).unwrap();
            socket.set_read_timeout(Some(WITHIN)).unwrap();
            socket.set_write_timeout(Some(WITHIN)).unwrap();
            let started = Instant::now();
            let deadline = started + WITHIN;
            let mut received = [0; FORWARD_CHUNK];
            let mut last_write = started;
            for (index, chunk) in payload.chunks_exact(FORWARD_CHUNK).enumerate() {
                assert!(
                    Instant::now() < deadline,
                    "managed forward payload deadline"
                );
                last_write = Instant::now();
                socket.write_all(chunk).unwrap();
                socket.read_exact(&mut received).unwrap();
                assert!(
                    received == chunk,
                    "forward payload differs in chunk {index}"
                );
                if index == 0 {
                    first.send(()).unwrap();
                }
                if index + 1 < payload.len() / FORWARD_CHUNK {
                    thread::sleep(FORWARD_PAUSE);
                }
            }
            let elapsed = started.elapsed();
            socket.shutdown(Shutdown::Write).unwrap();
            assert_eq!(
                socket.read(&mut received[..1]).unwrap(),
                0,
                "extra forwarded bytes"
            );
            (last_write, elapsed)
        });
        ready
            .recv_timeout(WITHIN)
            .expect("first exact forwarded chunk");
        let probe = probe(pane, serial);
        let (last_write, elapsed) = work.join().unwrap();
        (probe, last_write, elapsed)
    });
    assert!(
        probe.received < last_write,
        "forward workload ended before PTY echo"
    );
    let remaining = last_write - probe.received;
    (
        probe,
        json!({
            "bytes_sent": payload.len(),
            "exact_echo_bytes": payload.len(),
            "payload_elapsed_ms": milliseconds(elapsed),
            "last_chunk_start_after_pty_response_ms": milliseconds(remaining),
            "paced_round_trip_payload_bytes_per_second": payload.len() as f64 / elapsed.as_secs_f64(),
        }),
    )
}

fn listener(fixture: &NativeSshFixture, host: &RemoteHost) -> (Pane, u16, u32) {
    let shell = fixture.install_script(host, "traffic-echo.mjs", ECHO_SCRIPT, Vec::new());
    let (stream, _) = fixture.connect_pane();
    let mut pane = Pane::spawn(
        stream,
        ClientMsg::Spawn {
            cwd: Some(fixture.remote_home().into()),
            size: win_size(),
            shell: Some(shell),
            owner: None,
            workspace: None,
            restore: None,
            allow_remote_clipboard_write: false,
            await_claim: false,
        },
    );
    fixture.track_pane(pane.id);
    let deadline = Instant::now() + WITHIN;
    loop {
        if let Some(fields) = pane
            .line(Source::Live, deadline)
            .strip_prefix("LYREL437_LISTENER ")
        {
            let (port, pid) = fields.split_once(' ').unwrap();
            let (port, pid) = (port.parse().unwrap(), pid.parse().unwrap());
            return (pane, port, pid);
        }
    }
}

fn linux_proc(host: &RemoteHost, pid: u32, file: &str) -> String {
    let path = format!("/proc/{pid}/{file}");
    let bytes = host
        .read_file(Path::new(&path), 64 << 10)
        .unwrap_or_else(|err| panic!("{path}: {err}"));
    String::from_utf8(bytes).unwrap()
}

fn linux_stat(stat: &str) -> Vec<&str> {
    // comm can contain spaces and parentheses. Index 0 after it is field 3.
    stat.rsplit_once(") ")
        .unwrap()
        .1
        .split_whitespace()
        .collect()
}

fn linux_usage(host: &RemoteHost, pid: u32) -> Value {
    let stat = linux_proc(host, pid, "stat");
    let fields = linux_stat(&stat);
    let status = linux_proc(host, pid, "status");
    let kib = |field: &str| {
        status.lines().find_map(|line| {
            let value = line.strip_prefix(field)?.split_whitespace().next()?;
            Some(value.parse::<u64>().unwrap() * 1024)
        })
    };
    json!({
        "pid": pid, "comm": stat.split_once('(').unwrap().1.rsplit_once(')').unwrap().0,
        "start_ticks": fields[19].parse::<u64>().unwrap(),
        "cpu_ticks_total": fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap(),
        "rss_bytes": kib("VmRSS:"), "lifetime_peak_rss_bytes": kib("VmHWM:"),
    })
}

struct Resources<'a> {
    system: System,
    local_router: Pid,
    host: &'a RemoteHost,
    linux_daemon: u32,
    linux_sshd_session: u32,
    linux_echo: u32,
}

impl Resources<'_> {
    fn sample(&mut self) -> Value {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.local_router]),
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        let router = self
            .system
            .process(self.local_router)
            .expect("isolated router process");
        json!({
            "local_router": {
                "pid": self.local_router.as_u32(), "start_secs": router.start_time(),
                "cpu_milliseconds_total": router.accumulated_cpu_time(), "rss_bytes": router.memory(),
            },
            "linux_daemon": linux_usage(self.host, self.linux_daemon),
            "linux_ssh_shell_parent": linux_usage(self.host, self.linux_sshd_session),
            "linux_forward_echo": linux_usage(self.host, self.linux_echo),
        })
    }
}

fn emit(record: Value) {
    println!("{record}");
}

fn sample_count() -> usize {
    let samples = std::env::var("LYREL_TEST_SSH_TRAFFIC_SAMPLES")
        .map_or(1, |raw| raw.parse().expect("numeric #437 sample count"));
    assert!((1..=25).contains(&samples));
    samples
}

#[test]
#[ignore = "requires the dedicated loopback Docker Linux SSH fixture with a pinned host key"]
fn ssh_workspace_traffic_during_pty_input() {
    let samples = sample_count();
    assert_eq!(
        std::env::var("LYREL_TEST_SSH_HOST").as_deref(),
        Ok("127.0.0.1"),
        "use the dedicated Docker loopback endpoint",
    );
    let local_binary = local_binary();
    let fixture = NativeSshFixture::start(&local_binary);
    let ssh_port = fixture.spec().port;
    // The fixture already serves its Release daemon. If production falls back
    // to a release download, point even that path at the non-HTTP fixture
    // listener so a missing server can never contact a public release host.
    fs::write(
        fixture.local_config_dir().join("config.json"),
        json!({ "http_proxy": format!("http://127.0.0.1:{ssh_port}") }).to_string(),
    )
    .unwrap();
    let host = control(&fixture);
    let remote = status(&host);
    let local = ControlClient::connect_at(
        &endpoint(&fixture, "control"),
        &ControlHello::host_rpc(TOKEN, "local isolated router"),
    )
    .unwrap();
    let ReplyOk::Status(router) = local.request(ControlRequest::Status).unwrap() else {
        panic!("expected local router status");
    };
    let mut rtt = [Duration::ZERO; 12];
    for elapsed in &mut rtt {
        let start = Instant::now();
        assert_eq!(
            host.client().call(ControlRequest::Ping).unwrap(),
            ReplyOk::Pong
        );
        *elapsed = start.elapsed();
    }
    let mut native = native_pane(&fixture);
    let shell_pid = native.setup(fixture.remote_home());
    let shell_stat = linux_proc(&host, shell_pid, "stat");
    let sshd_pid = linux_stat(&shell_stat)[1].parse::<u32>().unwrap();
    native.send(b"printf '\\nLYREL437_%s %s\\n' CLK_TCK \"$(getconf CLK_TCK)\"\n".to_vec());
    let deadline = Instant::now() + WITHIN;
    let ticks_per_second = loop {
        if let Some(raw) = native
            .line(Source::Live, deadline)
            .strip_prefix("LYREL437_CLK_TCK ")
        {
            break raw.parse::<u64>().unwrap();
        }
    };
    native.expect(Source::Live, deadline, PROMPT);
    assert!(ticks_per_second > 0);
    let workspace = OwnedWorkspace::create(host.clone(), "Dedicated SSH traffic workload");
    let (mut remote_pane, _) = workspace_pane(&fixture, &workspace);
    remote_pane.setup(fixture.remote_home());
    let (_listener, target_port, echo_pid) = listener(&fixture, &host);
    let mut resources = Resources {
        system: System::new().expect("initialize isolated router resource sampler"),
        local_router: Pid::from_u32(router.pid),
        host: &host,
        linux_daemon: remote.pid,
        linux_sshd_session: sshd_pid,
        linux_echo: echo_pid,
    };
    let distro = host.read_file(Path::new("/etc/os-release"), 4096).unwrap();
    let distro = std::str::from_utf8(&distro).unwrap();
    // libtest may leave its `test ...` prefix unterminated with --nocapture.
    println!();
    emit(json!({
        "suite": "ssh437", "event": "run",
        "boundary": "production Host control and pane wire",
        "topology": "local router to Linux Docker/OpenSSH via loopback port mapping; forward binds loopback at both ends",
        "local_os": std::env::consts::OS, "local_arch": std::env::consts::ARCH,
        "docker_os": distro.lines().find_map(|line| line.strip_prefix("PRETTY_NAME=")),
        "ssh_port": ssh_port,
        "local_router_pid": router.pid, "local_build": router.build, "local_server": local_binary,
        "remote_daemon_pid": remote.pid, "remote_build": host.peer().build,
        "remote_instance": host.peer().instance,
        "revision": std::env::var("LYREL_TEST_SSH_TRAFFIC_REVISION").ok(),
        "debug_assertions": cfg!(debug_assertions), "gpu": "not applicable to headless wire workload",
        "linux_cpu_ticks_per_second": ticks_per_second,
        "ssh_control_rtt_p50_ms": percentile(&mut rtt, 50), "ssh_control_rtt_p95_ms": percentile(&mut rtt, 95),
        "grid": [win_size().cols, win_size().rows], "samples_per_route": samples, "warmups_per_route": 1,
        "transfer_bytes_each_direction": TRANSFER_BYTES, "host_rounds": HOST_ROUNDS,
        "forward_bytes_each_direction": FORWARD_BYTES, "forward_chunk_bytes": FORWARD_CHUNK,
        "forward_inter_chunk_pause_ms": FORWARD_PAUSE.as_millis(),
    }));

    let temp = tempfile::tempdir_in(fixture.local_config_dir()).unwrap();
    let source = temp.path().join("upload.bin");
    // A fixed binary fixture with varying chunks, independent of routes/jobs.
    let mut state = 0x243f_6a88_85a3_08d3_u64;
    let mut payload = Vec::with_capacity(TRANSFER_BYTES);
    for _ in 0..TRANSFER_BYTES / 8 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        payload.extend_from_slice(&state.to_le_bytes());
    }
    fs::write(&source, &payload).unwrap();
    let stages = [
        "idle",
        "host-list-watch",
        "sftp-upload",
        "sftp-download",
        "managed-forward",
    ];
    for (pane, workspace_id) in [(&mut native, None), (&mut remote_pane, Some(workspace.id))] {
        let route = Route {
            fixture: &fixture,
            workspace: workspace_id,
            pane: pane.id,
        };
        let forward = route.add_forward(target_port);
        let mut times: [_; 5] = std::array::from_fn(|_| Vec::with_capacity(samples));
        for sample in 0..=samples {
            let serial = format!("{}-{sample:02}", route.name());
            let before = resources.sample();
            let sample_start = Instant::now();
            let idle = probe(pane, &format!("{serial}-idle"));
            let remote_dir = fixture.remote_path(&format!("{serial}-watch"));
            let (listed, watch) = host_load(&host, &remote_dir, pane, &format!("{serial}-host"));
            let transfer_dir = fixture.remote_path(&format!("{serial}-files"));
            host.create_dir(&transfer_dir, false).unwrap();
            let remote_file = host.join(&transfer_dir, "payload.bin");
            let dest = temp.path().join(format!("{serial}.bin"));
            let (upload, upload_job) = transfer(
                &route,
                pane,
                &format!("{serial}-upload"),
                SftpTransferKind::Upload,
                &source,
                &remote_file,
            );
            verify_upload(&host, &remote_file, &payload);
            route.verify_sftp_listing(&transfer_dir);
            let (download, download_job) = transfer(
                &route,
                pane,
                &format!("{serial}-download"),
                SftpTransferKind::Download,
                &dest,
                &remote_file,
            );
            verify_download(&dest, &payload);
            let (forwarded, forward_data) = forward_load(
                forward.bind_port,
                pane,
                &format!("{serial}-forward"),
                &payload[..FORWARD_BYTES],
            );
            let sample_elapsed = sample_start.elapsed();
            let after = resources.sample();
            host.remove(&transfer_dir, true).unwrap();
            fs::remove_file(&dest).unwrap();
            for (index, (probe, load)) in [
                (idle, Value::Null),
                (listed, watch),
                (upload, upload_job),
                (download, download_job),
                (forwarded, forward_data),
            ]
            .into_iter()
            .enumerate()
            {
                if sample > 0 {
                    times[index].push(probe.elapsed);
                }
                emit(json!({
                    "suite": "ssh437", "event": "sample", "route": route.name(),
                    "load": stages[index], "sample": sample, "warmup": sample == 0, "pane": pane.id,
                    "input_to_exact_pty_response_ms": milliseconds(probe.elapsed),
                    "input_pty_bytes": probe.input_bytes, "response_pty_bytes": probe.output_bytes,
                    "verified_load": load,
                }));
            }
            emit(json!({
                "suite": "ssh437", "event": "resource_window", "route": route.name(), "sample": sample,
                "warmup": sample == 0, "scope": "whole sample, including file byte verification",
                "workload_elapsed_ms": milliseconds(sample_elapsed),
                "before": before, "after": after,
            }));
        }
        route.remove_forward(forward.id);
        for (stage, times) in stages.into_iter().zip(&mut times) {
            emit(json!({
                "suite": "ssh437", "event": "summary", "route": route.name(), "load": stage, "samples": samples,
                "input_to_exact_pty_response_p50_ms": percentile(times, 50),
                "input_to_exact_pty_response_p95_ms": percentile(times, 95),
                "input_to_exact_pty_response_min_ms": milliseconds(times[0]),
                "input_to_exact_pty_response_max_ms": milliseconds(times[times.len() - 1]),
            }));
        }
    }
}
