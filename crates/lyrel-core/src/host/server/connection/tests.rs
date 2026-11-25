use super::*;
use crate::core::machine::{self, MachineStore};
use crate::daemon::control::{
    CONTROL_VERSION, ControlClientMsg, ControlEvent, ControlHello, ControlReply, ControlRequest,
    ControlServerMsg, ReplyOk, WireErrorKind,
};
use crate::host::local::LocalHost;
use crate::host::{
    read::read_file_segmented,
    transfer::{CancelToken, SEGMENT_BYTES},
};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

fn tcp_peer(services: Services) -> (TcpStream, std::thread::JoinHandle<io::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let served = std::thread::spawn(move || {
        let (server, _) = listener.accept().unwrap();
        serve_with(server, LocalHost::new(), services)
    });
    let peer = TcpStream::connect(address).unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    (peer, served)
}

fn hello(peer: &mut TcpStream, hello: ControlHello) {
    ControlClientMsg::Hello(hello).encode(peer).unwrap();
    peer.flush().unwrap();
    let ControlServerMsg::HelloOk(reply) = ControlServerMsg::read(peer).unwrap() else {
        panic!("expected negotiated HELLO_OK");
    };
    assert_eq!(reply.control_version, CONTROL_VERSION);
}

fn call(peer: &mut TcpStream, req_id: u64, req: ControlRequest) -> ControlReply {
    ControlClientMsg::Request { req_id, req }
        .encode(peer)
        .unwrap();
    peer.flush().unwrap();
    loop {
        match ControlServerMsg::read(peer).unwrap() {
            ControlServerMsg::Response { req_id: got, reply } if got == req_id => return reply,
            ControlServerMsg::Event(_) => continue,
            other => panic!("unexpected frame {other:?}"),
        }
    }
}

/// [`call`] for replies that carry a blob (a read segment).
fn call_blob(peer: &mut TcpStream, req_id: u64, req: ControlRequest) -> (ControlReply, Vec<u8>) {
    ControlClientMsg::Request { req_id, req }
        .encode(peer)
        .unwrap();
    peer.flush().unwrap();
    loop {
        match ControlServerMsg::read(peer).unwrap() {
            ControlServerMsg::ResponseBlob {
                req_id: got,
                reply,
                blob,
            } if got == req_id => {
                return (reply, blob);
            }
            ControlServerMsg::Response { req_id: got, reply } if got == req_id => {
                return (reply, Vec::new());
            }
            ControlServerMsg::Event(_) => continue,
            other => panic!("unexpected frame {other:?}"),
        }
    }
}

fn body(len: usize) -> Vec<u8> {
    (0..len as u32).map(|i| (i % 251) as u8).collect()
}

#[test]
fn a_read_session_streams_one_version_over_the_real_link() {
    let dir = tempfile::TempDir::new().unwrap();
    let bytes = body(1024 * 1024 + 7);
    let path = dir.path().join("streamed.bin");
    std::fs::write(&path, &bytes).unwrap();
    let wire = path.to_string_lossy().into_owned();

    let (mut peer, served) = tcp_peer(Services::none());
    hello(&mut peer, ControlHello::host_rpc("read", "laptop"));
    let (reply, _) = call_blob(&mut peer, 1, ControlRequest::OpenRead { path: wire });
    let ControlReply::Ok(ReplyOk::ReadOpened { session, meta }) = reply else {
        panic!("expected an opened read session, got {reply:?}");
    };
    assert_eq!(meta.len, bytes.len() as u64);

    let mut got = Vec::new();
    let mut req_id = 2;
    loop {
        let (reply, blob) = call_blob(
            &mut peer,
            req_id,
            ControlRequest::ReadChunk {
                session,
                max_bytes: SEGMENT_BYTES,
            },
        );
        req_id += 1;
        match reply {
            ControlReply::Ok(ReplyOk::FileMeta { meta }) => {
                assert_eq!(meta.len, bytes.len() as u64);
            }
            other => panic!("expected file metadata, got {other:?}"),
        }
        if blob.is_empty() {
            break;
        }
        assert!(blob.len() as u64 <= SEGMENT_BYTES);
        got.extend_from_slice(&blob);
    }
    assert!(
        got == bytes,
        "the whole file arrived from one bound version"
    );

    assert_eq!(
        call(&mut peer, req_id, ControlRequest::CloseRead { session }),
        ControlReply::Ok(ReplyOk::Unit)
    );
    let (reply, _) = call_blob(
        &mut peer,
        req_id + 1,
        ControlRequest::ReadChunk {
            session,
            max_bytes: 16,
        },
    );
    match reply {
        ControlReply::Err(error) => assert_eq!(error.kind, WireErrorKind::TimedOut),
        other => panic!("a closed session still answered: {other:?}"),
    }
    drop(peer);
    served.join().unwrap().unwrap();
}

#[test]
fn a_segment_reports_a_change_over_the_real_link() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("changed.bin");
    std::fs::write(&path, vec![b'a'; 8192]).unwrap();
    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();

    let (mut peer, served) = tcp_peer(Services::none());
    hello(&mut peer, ControlHello::host_rpc("read", "laptop"));
    let (reply, _) = call_blob(
        &mut peer,
        1,
        ControlRequest::OpenRead {
            path: path.to_string_lossy().into_owned(),
        },
    );
    let ControlReply::Ok(ReplyOk::ReadOpened { session, .. }) = reply else {
        panic!("expected an opened read session, got {reply:?}");
    };
    let (reply, first) = call_blob(
        &mut peer,
        2,
        ControlRequest::ReadChunk {
            session,
            max_bytes: 4096,
        },
    );
    assert!(matches!(reply, ControlReply::Ok(ReplyOk::FileMeta { .. })));
    assert_eq!(first, vec![b'a'; 4096]);

    // Same size, restored mtime: only the server's bound change metadata can
    // see this, and the next segment must say so instead of mixing versions.
    std::thread::sleep(Duration::from_millis(10));
    std::fs::write(&path, vec![b'b'; 8192]).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), mtime);

    let (reply, _) = call_blob(
        &mut peer,
        3,
        ControlRequest::ReadChunk {
            session,
            max_bytes: 4096,
        },
    );
    match reply {
        ControlReply::Err(error) => assert_eq!(error.kind, WireErrorKind::FileChanged),
        other => panic!("a changed file answered with {other:?}"),
    }
    let (reply, _) = call_blob(
        &mut peer,
        4,
        ControlRequest::ReadChunk {
            session,
            max_bytes: 16,
        },
    );
    match reply {
        ControlReply::Err(error) => assert_eq!(error.kind, WireErrorKind::TimedOut),
        other => panic!("an invalidated session still answered: {other:?}"),
    }
    drop(peer);
    served.join().unwrap().unwrap();
}

#[test]
fn a_large_file_crosses_the_real_link_in_bounded_segments() {
    use crate::host::remote::RemoteHost;

    let dir = tempfile::TempDir::new().unwrap();
    let bytes = body(5 * 1024 * 1024 + 123);
    let path = dir.path().join("large.bin");
    std::fs::write(&path, &bytes).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let served = std::thread::spawn(move || {
        let (server, _) = listener.accept().unwrap();
        serve_with(server, LocalHost::new(), Services::none())
    });
    let sock = TcpStream::connect(address).unwrap();
    let host = RemoteHost::over_tcp(
        sock,
        "tcp:segmented-test",
        &ControlHello::host_rpc("tok", "laptop"),
    )
    .unwrap();

    let mut progress = Vec::new();
    let outcome = read_file_segmented(
        &*host,
        &path,
        8 * 1024 * 1024,
        &CancelToken::default(),
        |update| progress.push(update),
    )
    .unwrap();
    assert!(outcome.bytes == bytes, "5 MiB arrived byte for byte");
    assert_eq!(outcome.meta.len, bytes.len() as u64);

    let mut previous = 0;
    for update in &progress {
        assert!(
            update.read - previous <= SEGMENT_BYTES,
            "one segment carried {} bytes",
            update.read - previous
        );
        previous = update.read;
    }
    assert!(
        progress.len() > 20,
        "5 MiB must take many bounded segments, got {}",
        progress.len()
    );
    assert_eq!(progress.last().map(|p| p.read), Some(bytes.len() as u64));

    drop(host);
    served.join().unwrap().unwrap();
}

fn expect_no_preview(peer: &mut TcpStream) {
    peer.set_read_timeout(Some(Duration::from_millis(250)))
        .unwrap();
    for _ in 0..16 {
        match ControlServerMsg::read(peer) {
            Ok(ControlServerMsg::Event(ControlEvent::PreviewOpen { .. })) => {
                panic!("a preview reached a link that does not hold the workspace")
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
}

fn expect_preview(peer: &mut TcpStream) -> (String, u64, String, Option<String>, String) {
    loop {
        match ControlServerMsg::read(peer).unwrap() {
            ControlServerMsg::Event(ControlEvent::PreviewOpen {
                workspace,
                pane,
                url,
                scratch,
                file,
            }) => return (workspace, pane, url, file, scratch),
            ControlServerMsg::Event(_) => continue,
            other => panic!("expected a preview event, got {other:?}"),
        }
    }
}

fn attach_workspace(peer: &mut TcpStream, req_id: u64, workspace: &str) {
    assert!(matches!(
        call(
            peer,
            req_id,
            ControlRequest::WorkspaceAttach {
                id: workspace.to_string(),
            }
        ),
        ControlReply::Ok(ReplyOk::Attached { .. })
    ));
}

#[test]
fn an_agent_preview_targets_only_the_consumer_holding_its_workspace() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
    let workspace = store.workspace_create(None, None, None).unwrap().id;
    let other = store.workspace_create(None, None, None).unwrap().id;
    store
        .tab_create(workspace, None, machine::PaneSeed::bare(7), None, None)
        .unwrap();
    let services = Services::with_machine(Arc::clone(&store));

    let (mut preview, preview_served) = tcp_peer(services.clone());
    hello(
        &mut preview,
        ControlHello::host_rpc("preview", "desktop").with_preview(),
    );
    let (mut bystander, bystander_served) = tcp_peer(services.clone());
    hello(
        &mut bystander,
        ControlHello::host_rpc("bystander", "desktop").with_preview(),
    );
    attach_workspace(&mut bystander, 1, &other.to_string());

    let (mut cli, cli_served) = tcp_peer(services.clone());
    hello(&mut cli, ControlHello::host_rpc("cli", "laptop"));
    let ask = || ControlRequest::PreviewOpen {
        pane: 7,
        url: "http://localhost:5173/app".into(),
        scratch: String::new(),
        file: None,
    };

    // The preview consumer is online, but no link holds the workspace.
    assert_eq!(
        call(&mut cli, 1, ask()),
        ControlReply::Ok(ReplyOk::Bool(false)),
        "an unclaimed workspace has no preview target"
    );
    expect_no_preview(&mut preview);
    expect_no_preview(&mut bystander);

    // The CLI holds the workspace; it did not negotiate preview, so it is
    // not the target and the unrelated consumer still receives nothing.
    attach_workspace(&mut cli, 2, &workspace.to_string());
    assert_eq!(
        call(&mut cli, 3, ask()),
        ControlReply::Ok(ReplyOk::Bool(false)),
        "held by a non-consumer"
    );
    expect_no_preview(&mut preview);
    expect_no_preview(&mut bystander);

    // The preview consumer claims the workspace: delivery is exactly here.
    attach_workspace(&mut preview, 1, &workspace.to_string());
    assert_eq!(
        call(&mut cli, 4, ask()),
        ControlReply::Ok(ReplyOk::Bool(true)),
        "the consuming holder is the target"
    );
    let (got, pane, url, file, scratch) = expect_preview(&mut preview);
    assert_eq!(got, workspace.to_string());
    assert_eq!(pane, 7);
    assert_eq!(url, "http://localhost:5173/app");
    assert_eq!(file, None);
    assert!(
        std::path::Path::new(&scratch).is_absolute(),
        "the daemon resolves the screenshot directory on its own machine: {scratch}"
    );
    expect_no_preview(&mut bystander);

    // The pane is the authority, never a client-supplied workspace.
    assert!(matches!(
        call(
            &mut cli,
            5,
            ControlRequest::PreviewOpen {
                pane: 99,
                url: "http://localhost:5173".into(),
                scratch: String::new(),
                file: None,
            }
        ),
        ControlReply::Err(_)
    ));
    // The URL policy is applied before anything reaches a GUI.
    assert!(matches!(
        call(
            &mut cli,
            6,
            ControlRequest::PreviewOpen {
                pane: 7,
                url: "file:///etc/passwd".into(),
                scratch: String::new(),
                file: None,
            }
        ),
        ControlReply::Err(_)
    ));
    expect_no_preview(&mut preview);

    drop(cli);
    cli_served.join().unwrap().unwrap();
    drop(preview);
    preview_served.join().unwrap().unwrap();
    drop(bystander);
    bystander_served.join().unwrap().unwrap();
}

#[test]
fn a_tcp_protocol_error_releases_attachment_and_unregisters_the_gui() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
    let workspace = store.workspace_create(None, None, None).unwrap().id;
    let services = Services::with_machine(Arc::clone(&store));
    let (mut gui, served) = tcp_peer(services.clone());
    hello(
        &mut gui,
        ControlHello {
            workspace: Some(workspace.to_string()),
            ..ControlHello::gui("gui-token", "desktop")
        },
    );
    assert_eq!(
        call(&mut gui, 1, ControlRequest::Ping),
        ControlReply::Ok(ReplyOk::Pong)
    );
    assert_eq!(store.attachment(workspace).unwrap().token, "gui-token");
    assert_eq!(
        call(
            &mut gui,
            2,
            ControlRequest::GuiOpen {
                path: None,
                workspace: None
            }
        ),
        ControlReply::Ok(ReplyOk::Bool(true)),
    );

    // A protocol error, not only clean EOF, must converge on explicit teardown.
    ControlClientMsg::Hello(ControlHello::host_rpc("second", "invalid"))
        .encode(&mut gui)
        .unwrap();
    gui.flush().unwrap();
    let error = served.join().unwrap().unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    loop {
        if let Err(error) = ControlServerMsg::read(&mut gui) {
            assert!(is_disconnect(&error), "expected disconnect, got {error}");
            break;
        }
    }
    assert!(services.attachments.is_empty());
    assert!(store.attachment(workspace).is_none());

    // Ask through another real control connection, without registry test access.
    let (mut probe, probe_served) = tcp_peer(services);
    hello(&mut probe, ControlHello::host_rpc("probe", "probe"));
    assert_eq!(
        call(
            &mut probe,
            1,
            ControlRequest::GuiOpen {
                path: None,
                workspace: None
            }
        ),
        ControlReply::Ok(ReplyOk::Bool(false)),
    );
    drop(probe);
    probe_served.join().unwrap().unwrap();
}

struct TestPaneDir {
    panes: Vec<crate::daemon::protocol::PaneInfo>,
}

impl crate::host::server::PaneDirectory for TestPaneDir {
    fn pane_count(&self) -> u64 {
        self.panes.len() as u64
    }

    fn panes(&self) -> Vec<crate::daemon::protocol::PaneInfo> {
        self.panes.clone()
    }

    fn agent_states(&self) -> Vec<crate::daemon::control::PaneAgentState> {
        Vec::new()
    }

    fn pane_procs(&self, _pane_id: u64) -> crate::daemon::protocol::PaneProcs {
        crate::daemon::protocol::PaneProcs::default()
    }

    fn hibernate_pane(&self, _pane_id: u64) {}

    fn close_pane(&self, _pane_id: u64) {}
}

struct NonHtmlCanonicalHost {
    inner: crate::host::SharedHost,
}

impl crate::host::Host for NonHtmlCanonicalHost {
    fn id(&self) -> crate::host::HostId {
        self.inner.id()
    }
    fn separator(&self) -> char {
        self.inner.separator()
    }
    fn is_absolute(&self, p: &std::path::Path) -> bool {
        self.inner.is_absolute(p)
    }
    fn read_dir(
        &self,
        dir: &std::path::Path,
        root: Option<&std::path::Path>,
    ) -> io::Result<Vec<crate::host::Entry>> {
        self.inner.read_dir(dir, root)
    }
    fn stat(&self, p: &std::path::Path) -> io::Result<crate::host::Meta> {
        self.inner.stat(p)
    }
    fn read_file(&self, p: &std::path::Path, max_bytes: u64) -> io::Result<Vec<u8>> {
        self.inner.read_file(p, max_bytes)
    }
    fn canonicalize(&self, p: &std::path::Path) -> io::Result<std::path::PathBuf> {
        Ok(p.with_extension("env"))
    }
    fn search(
        &self,
        roots: &[std::path::PathBuf],
        query: &str,
        limit: usize,
        max_dirs: usize,
        show_hidden: bool,
    ) -> io::Result<Vec<crate::host::SearchHit>> {
        self.inner
            .search(roots, query, limit, max_dirs, show_hidden)
    }
    fn search_content(
        &self,
        roots: &[std::path::PathBuf],
        query: &crate::host::ContentQuery,
        limits: &crate::host::ContentLimits,
    ) -> io::Result<crate::host::ContentResults> {
        self.inner.search_content(roots, query, limits)
    }
    fn write_file(&self, p: &std::path::Path, bytes: &[u8]) -> io::Result<crate::host::Meta> {
        self.inner.write_file(p, bytes)
    }
    fn create_file_new(&self, p: &std::path::Path) -> io::Result<()> {
        self.inner.create_file_new(p)
    }
    fn create_dir(&self, p: &std::path::Path, recursive: bool) -> io::Result<()> {
        self.inner.create_dir(p, recursive)
    }
    fn rename(&self, from: &std::path::Path, to: &std::path::Path) -> io::Result<()> {
        self.inner.rename(from, to)
    }
    fn remove(&self, p: &std::path::Path, recursive: bool) -> io::Result<()> {
        self.inner.remove(p, recursive)
    }
    fn repo_root(&self, p: &std::path::Path) -> io::Result<Option<std::path::PathBuf>> {
        self.inner.repo_root(p)
    }
    fn git(&self, cwd: &std::path::Path, args: &[&str]) -> io::Result<crate::host::Output> {
        self.inner.git(cwd, args)
    }
    fn shells(&self) -> io::Result<crate::core::shells::ShellInventory> {
        self.inner.shells()
    }
    fn agent_sessions(
        &self,
        directory: Option<&std::path::Path>,
    ) -> io::Result<Vec<crate::core::agent_history::PastSession>> {
        self.inner.agent_sessions(directory)
    }
    fn watch(&self, dirs: &[std::path::PathBuf]) -> io::Result<crate::host::WatchSub> {
        self.inner.watch(dirs)
    }
}

fn tcp_peer_with_host(
    host: crate::host::SharedHost,
    services: Services,
) -> (
    TcpStream,
    std::thread::JoinHandle<io::Result<()>>,
    crate::daemon::control::ControlHelloOk,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let served = std::thread::spawn(move || {
        let (server, _) = listener.accept().unwrap();
        serve_with(server, host, services)
    });
    let mut peer = TcpStream::connect(address).unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    ControlClientMsg::Hello(ControlHello::host_rpc("cli-html", "localhost"))
        .encode(&mut peer)
        .unwrap();
    peer.flush().unwrap();
    let ControlServerMsg::HelloOk(ok) = ControlServerMsg::read(&mut peer).unwrap() else {
        panic!("expected HelloOk");
    };
    (peer, served, ok)
}

#[test]
fn html_preview_resolves_relative_to_pane_cwd_and_blocks_unsafe_targets() {
    let dir = tempfile::TempDir::new().unwrap();
    let project = dir.path().join("project");
    let sub = project.join("dist");
    std::fs::create_dir_all(&sub).unwrap();
    let index_html = sub.join("index.html");
    std::fs::write(&index_html, "<h1>Hi</h1>").unwrap();
    let env_file = project.join(".env");
    std::fs::write(&env_file, "SECRET=1").unwrap();
    let html_dir = project.join("dir.html");
    std::fs::create_dir_all(&html_dir).unwrap();

    let canonical_index = LocalHost::new()
        .canonicalize(&index_html)
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
    let workspace = store.workspace_create(None, None, None).unwrap().id;
    store
        .tab_create(workspace, None, machine::PaneSeed::bare(7), None, None)
        .unwrap();
    let pane_dir = Arc::new(TestPaneDir {
        panes: vec![crate::daemon::protocol::PaneInfo {
            pane_id: 7,
            title: "sh".into(),
            osc_title: None,
            cwd: Some(project.clone()),
            alive: true,
            owner: None,
        }],
    });
    let mut services = Services::with_machine(Arc::clone(&store));
    services.panes = Some(pane_dir);

    let (mut preview, preview_served) = tcp_peer(services.clone());
    hello(
        &mut preview,
        ControlHello::host_rpc("preview", "desktop").with_preview(),
    );
    attach_workspace(&mut preview, 1, &workspace.to_string());

    let (mut cli, cli_served, ok) = tcp_peer_with_host(LocalHost::new(), services.clone());
    assert!(
        ok.has_feature(crate::daemon::control::feature::PREVIEW_HTML),
        "handshake must advertise preview-html-v1"
    );

    // Relative POSIX-style path resolves against pane cwd and delivers canonical file.
    assert_eq!(
        call(
            &mut cli,
            1,
            ControlRequest::PreviewOpen {
                pane: 7,
                url: String::new(),
                scratch: String::new(),
                file: Some("dist/index.html".into()),
            }
        ),
        ControlReply::Ok(ReplyOk::Bool(true)),
    );
    let (got_ws, got_pane, got_url, got_file, _) = expect_preview(&mut preview);
    assert_eq!(got_ws, workspace.to_string());
    assert_eq!(got_pane, 7);
    assert_eq!(got_url, "");
    assert_eq!(got_file.as_deref(), Some(canonical_index.as_str()));

    // Missing file, non-HTML file, and directory.html are all rejected without delivery.
    assert!(matches!(
        call(
            &mut cli,
            2,
            ControlRequest::PreviewOpen {
                pane: 7,
                url: String::new(),
                scratch: String::new(),
                file: Some("missing.html".into()),
            }
        ),
        ControlReply::Err(err) if err.kind == WireErrorKind::NotFound
    ));
    assert!(matches!(
        call(
            &mut cli,
            3,
            ControlRequest::PreviewOpen {
                pane: 7,
                url: String::new(),
                scratch: String::new(),
                file: Some(".env".into()),
            }
        ),
        ControlReply::Err(err) if err.kind == WireErrorKind::InvalidInput
    ));
    assert!(matches!(
        call(
            &mut cli,
            4,
            ControlRequest::PreviewOpen {
                pane: 7,
                url: String::new(),
                scratch: String::new(),
                file: Some("dir.html".into()),
            }
        ),
        ControlReply::Err(err) if err.kind == WireErrorKind::InvalidInput
    ));
    expect_no_preview(&mut preview);

    // A legacy GUI link (`preview: true, preview_html: false`) does not receive HTML previews.
    let (mut legacy_preview, legacy_served) = tcp_peer(services.clone());
    let mut legacy_hello = ControlHello::host_rpc("legacy-preview", "desktop").with_preview();
    legacy_hello.preview_html = false;
    hello(&mut legacy_preview, legacy_hello);
    attach_workspace(&mut legacy_preview, 1, &workspace.to_string());
    assert_eq!(
        call(
            &mut cli,
            5,
            ControlRequest::PreviewOpen {
                pane: 7,
                url: String::new(),
                scratch: String::new(),
                file: Some("dist/index.html".into()),
            }
        ),
        ControlReply::Ok(ReplyOk::Bool(false)),
    );
    expect_no_preview(&mut legacy_preview);

    // Re-checking `is_html_preview_path` after canonicalize blocks symlinks pointing to non-HTML.
    let (mut symlink_cli, symlink_served, _) = tcp_peer_with_host(
        Arc::new(NonHtmlCanonicalHost {
            inner: LocalHost::new(),
        }),
        services,
    );
    assert!(matches!(
        call(
            &mut symlink_cli,
            1,
            ControlRequest::PreviewOpen {
                pane: 7,
                url: String::new(),
                scratch: String::new(),
                file: Some("dist/index.html".into()),
            }
        ),
        ControlReply::Err(err) if err.kind == WireErrorKind::InvalidInput
    ));

    drop(symlink_cli);
    symlink_served.join().unwrap().unwrap();
    drop(cli);
    cli_served.join().unwrap().unwrap();
    drop(legacy_preview);
    legacy_served.join().unwrap().unwrap();
    drop(preview);
    preview_served.join().unwrap().unwrap();
}
