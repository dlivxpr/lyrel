//! Narrow Unix consumer fixtures shared by negotiation, lifecycle, and request tests.
use super::*;
use crate::daemon::control::{ControlHello, ControlHelloOk};
use crate::host::local::LocalHost;
use crate::host::remote::RemoteHost;
use std::io::Write;
use std::os::unix::net::UnixStream;

pub(super) struct Pair {
    pub(super) host: Arc<RemoteHost>,
}

pub(super) fn pair_with(server_host: SharedHost) -> Pair {
    let (server, client) = UnixStream::pair().unwrap();
    std::thread::spawn(move || {
        let _ = serve(server, server_host);
    });
    let hello = ControlHello::host_rpc("test-token", "test-host");
    let host = RemoteHost::over_unix(client, "test:pair", &hello).unwrap();
    Pair { host }
}

pub(super) fn pair() -> Pair {
    pair_with(LocalHost::new())
}

pub(super) fn raw() -> (UnixStream, ControlHelloOk) {
    raw_with(Services::none())
}

pub(super) fn raw_with(services: Services) -> (UnixStream, ControlHelloOk) {
    raw_hello(services, ControlHello::host_rpc("t", "h")).0
}

pub(super) fn raw_hello(
    services: Services,
    hello: ControlHello,
) -> ((UnixStream, ControlHelloOk), std::thread::JoinHandle<()>) {
    let (server, mut client) = UnixStream::pair().unwrap();
    let served = std::thread::spawn(move || {
        let _ = serve_with(server, LocalHost::new(), services);
    });
    ControlClientMsg::Hello(hello).encode(&mut client).unwrap();
    client.flush().unwrap();
    let ok = match ControlServerMsg::read(&mut client).unwrap() {
        ControlServerMsg::HelloOk(ok) => ok,
        other => panic!("{other:?}"),
    };
    ((client, ok), served)
}

pub(super) fn workspace_services() -> (Services, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().unwrap();
    let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
    (Services::with_machine(store), dir)
}

pub(super) fn ask(client: &mut UnixStream, req_id: u64, req: ControlRequest) -> ControlReply {
    ControlClientMsg::Request { req_id, req }
        .encode(&mut *client)
        .unwrap();
    client.flush().unwrap();
    loop {
        match ControlServerMsg::read(&mut *client).unwrap() {
            ControlServerMsg::Response { req_id: got, reply } if got == req_id => {
                return reply;
            }
            ControlServerMsg::Event(_) => continue,
            other => panic!("unexpected frame: {other:?}"),
        }
    }
}
