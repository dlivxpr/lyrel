use super::super::test_support::*;
use super::super::*;
use super::*;
use crate::host::local::LocalHost;
use crate::host::remote::RemoteHost;
use std::io::Write;
use std::os::unix::net::UnixStream;

#[test]
fn the_handshake_describes_this_server() {
    let p = pair();
    let peer = p.host.peer();
    assert_eq!(peer.control_version, CONTROL_VERSION);
    assert_eq!(
        peer.protocol_version,
        crate::daemon::protocol::PROTOCOL_VERSION
    );
    assert_eq!(peer.separator, std::path::MAIN_SEPARATOR);
    assert!(peer.has_feature(feature::CONTROL));
    assert!(peer.has_feature(feature::HOST_RPC));
    assert!(peer.has_feature(feature::CONTENT_SEARCH));
    assert!(
        peer.has_feature(feature::GIT_BLOB),
        "the content-aware Diff asks this name before sending a version read"
    );
    assert!(
        !peer.home.is_empty(),
        "the server's $HOME backs `new workspace in ~`"
    );
}

#[test]
fn a_bounded_git_blob_crosses_the_control_server() {
    use crate::host::Host as _;

    let p = pair();
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    let git = |args: &[&str]| {
        let out = p.host.git(root, args).unwrap();
        assert!(
            out.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "--quiet", "-b", "main"]);
    git(&["config", "core.autocrlf", "false"]);

    let file = p.host.join(root, "a.rs");
    p.host.write_file(&file, b"fn v1() {}\n").unwrap();
    git(&["add", "a.rs"]);
    p.host.write_file(&file, b"fn v3() {}\n").unwrap();

    assert_eq!(
        p.host.git_blob(root, ":a.rs", 1024).unwrap().unwrap(),
        b"fn v1() {}\n",
        "the server answers the index side the client asked for"
    );
    assert_eq!(
        p.host.git_blob(root, "HEAD:a.rs", 1024).unwrap(),
        None,
        "an unborn revision has no side"
    );
    let error = p.host.git_blob(root, ":a.rs", 4).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::FileTooLarge, "{error}");
}

#[test]
fn the_handshake_advertises_the_pane_daemons_features() {
    let p = pair();
    let peer = p.host.peer();
    for name in crate::daemon::protocol::DaemonVersion::current().features {
        assert!(
            peer.has_feature(&name),
            "the hello must carry the pane feature {name:?}: a pane \
             connection answers one message, so this handshake is the \
             only affordable place for a client to learn it"
        );
    }
    assert!(
        peer.has_feature(crate::daemon::protocol::FEATURE_RESIZE_ECHO),
        "the resize-echo deferral on remote routes hangs off this name"
    );
}

#[test]
fn a_version_mismatch_is_answered_then_closed() {
    let (server, mut client) = UnixStream::pair().unwrap();
    std::thread::spawn(move || {
        let _ = serve(server, LocalHost::new());
    });

    let mut hello = ControlHello::host_rpc("t", "h");
    hello.control_version = CONTROL_VERSION + 7;
    ControlClientMsg::Hello(hello).encode(&mut client).unwrap();
    client.flush().unwrap();

    match ControlServerMsg::read(&mut client).unwrap() {
        ControlServerMsg::HelloOk(ok) => assert_eq!(ok.control_version, CONTROL_VERSION),
        other => panic!("{other:?}"),
    }
    let mut buf = [0u8; 1];
    assert_eq!(
        client.read(&mut buf).unwrap(),
        0,
        "the server should hang up"
    );
}

#[test]
fn the_client_refuses_a_mismatched_peer() {
    let (server, client) = UnixStream::pair().unwrap();
    std::thread::spawn(move || {
        let mut s = server;
        let _ = ControlClientMsg::read(&mut s);
        let _ = ControlServerMsg::HelloOk(ControlHelloOk {
            control_version: CONTROL_VERSION + 1,
            protocol_version: 3,
            build: "other".into(),
            separator: '/',
            home: "/root".into(),
            agent_environment: None,
            features: vec![],
            instance: "other-instance".into(),
        })
        .encode(&mut s);
    });
    let hello = ControlHello::host_rpc("t", "h");
    let err = RemoteHost::over_unix(client, "test:skew", &hello).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::Unsupported, "{err}");
}
