use super::test_support::SlowGit;
use super::*;
use super::super::serve;
use crate::daemon::control::{ControlHello, ControlServerMsg};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

#[test]
fn cancelling_an_inflight_request_drops_its_reply_but_keeps_the_link_usable() {
    let (host, started, release, finished) = SlowGit::gated();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let served = std::thread::spawn(move || {
        let (server, _) = listener.accept().unwrap();
        serve(server, host).unwrap();
    });
    let mut peer = TcpStream::connect(address).unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    ControlClientMsg::Hello(ControlHello::host_rpc("cancel-token", "cancel-consumer"))
        .encode(&mut peer)
        .unwrap();
    peer.flush().unwrap();
    assert!(matches!(ControlServerMsg::read(&mut peer).unwrap(), ControlServerMsg::HelloOk(_)));

    ControlClientMsg::Request {
        req_id: 41,
        req: ControlRequest::Git { cwd: ".".into(), args: vec!["status".into()] },
    }
    .encode(&mut peer)
    .unwrap();
    peer.flush().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();

    // Cancel only after the Host has entered Git, and leave that work blocked
    // until a different correlated request has answered on this same link.
    ControlClientMsg::Cancel { req_id: 41 }.encode(&mut peer).unwrap();
    ControlClientMsg::Request { req_id: 42, req: ControlRequest::Ping }
        .encode(&mut peer)
        .unwrap();
    peer.flush().unwrap();
    assert_eq!(
        ControlServerMsg::read(&mut peer).unwrap(),
        ControlServerMsg::Response { req_id: 42, reply: ControlReply::Ok(ReplyOk::Pong) }
    );

    release.send(()).unwrap();
    finished.recv_timeout(Duration::from_secs(5)).unwrap();
    // Give the completed worker a full read interval to leak a response. The
    // retired correlation must remain silent, not just lose a race to Ping.
    peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let silence = ControlServerMsg::read(&mut peer).unwrap_err();
    assert!(matches!(silence.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut), "{silence}");

    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    ControlClientMsg::Request { req_id: 43, req: ControlRequest::Ping }
        .encode(&mut peer)
        .unwrap();
    peer.flush().unwrap();
    assert_eq!(
        ControlServerMsg::read(&mut peer).unwrap(),
        ControlServerMsg::Response { req_id: 43, reply: ControlReply::Ok(ReplyOk::Pong) }
    );
    drop(peer);
    served.join().unwrap();
}
