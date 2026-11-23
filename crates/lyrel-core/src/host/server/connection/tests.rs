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

#[test]
fn an_agent_preview_resolves_its_workspace_and_targets_only_preview_consumers() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
    let workspace = store.workspace_create(None, None, None).unwrap().id;
    store
        .tab_create(
            workspace,
            None,
            machine::PaneSeed::bare(7),
            None,
            None,
        )
        .unwrap();
    let services = Services::with_machine(Arc::clone(&store));

    let (mut preview, preview_served) = tcp_peer(services.clone());
    hello(
        &mut preview,
        ControlHello::host_rpc("preview", "desktop").with_preview(),
    );
    let (mut cli, cli_served) = tcp_peer(services.clone());
    hello(&mut cli, ControlHello::host_rpc("cli", "laptop"));

    assert_eq!(
        call(
            &mut cli,
            1,
            ControlRequest::PreviewOpen {
                pane: 7,
                url: "http://localhost:5173/app".into(),
                scratch: String::new(),
            }
        ),
        ControlReply::Ok(ReplyOk::Bool(true)),
        "the owning daemon delivered it to the preview consumer"
    );
    let event = ControlServerMsg::read(&mut preview).unwrap();
    let ControlServerMsg::Event(ControlEvent::PreviewOpen {
        workspace: got,
        pane,
        url,
        scratch,
    }) = event
    else {
        panic!("expected a preview event, got {event:?}");
    };
    assert_eq!(got, workspace.to_string());
    assert_eq!(pane, 7);
    assert_eq!(url, "http://localhost:5173/app");
    assert!(
        std::path::Path::new(&scratch).is_absolute(),
        "the daemon resolves the screenshot directory on its own machine: {scratch}"
    );

    // The pane is the authority, never a client-supplied workspace.
    assert!(matches!(
        call(
            &mut cli,
            2,
            ControlRequest::PreviewOpen {
                pane: 99,
                url: "http://localhost:5173".into(),
                scratch: String::new(),
            }
        ),
        ControlReply::Err(_)
    ));
    // The URL policy is applied before anything reaches a GUI.
    assert!(matches!(
        call(
            &mut cli,
            3,
            ControlRequest::PreviewOpen {
                pane: 7,
                url: "file:///etc/passwd".into(),
                scratch: String::new(),
            }
        ),
        ControlReply::Err(_)
    ));

    drop(cli);
    cli_served.join().unwrap().unwrap();
    drop(preview);
    preview_served.join().unwrap().unwrap();
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
