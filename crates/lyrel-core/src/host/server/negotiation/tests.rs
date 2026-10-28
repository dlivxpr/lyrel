use super::*;
use super::super::*;
use super::super::test_support::*;
use crate::host::local::LocalHost;
use crate::host::remote::RemoteHost;
use std::os::unix::net::UnixStream;
use std::io::Write;

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
        !peer.home.is_empty(),
        "the server's $HOME backs `new workspace in ~`"
    );
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
