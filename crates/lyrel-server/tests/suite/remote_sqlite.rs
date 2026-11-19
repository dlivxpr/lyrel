#![cfg(unix)]
//! SQLite 共享 Host 契约经真实、严格 pin 的 native SSH 路由执行。
//!
//! 本专用 loopback fixture 的 Linux 客户端与 sshd 共享文件系统，允许既有
//! conformance writer 独立持有真实 SQLite 连接、检查 main/WAL 与 checkpoint。
//! 浏览请求仍必须经过生产 RemoteHost、SSH 和远端 daemon；不是跨机器或
//! Windows 原生窗口验收。常规套件不连接外部主机，缺少显式前提直接失败。
use lyrel_test_support::native_ssh as fixture;

use std::path::Path;

use fixture::NativeSshFixture;
use lyrel_core::host::conformance::{CASES, Sandbox};

struct TempSandbox(tempfile::TempDir);

impl Sandbox for TempSandbox {
    fn path(&self) -> &Path {
        self.0.path()
    }
}

#[test]
#[ignore = "requires the explicit private Linux loopback SSH fixture, a shared fixture home and matching candidate server; run alone with --ignored"]
fn sqlite_contracts_cross_the_real_native_ssh_route() {
    let fixture = NativeSshFixture::start(Path::new(env!("CARGO_BIN_EXE_lyrel-server")));
    let host = fixture.connect_control();
    assert!(
        host.peer().has_feature("sqlite-browse-v1"),
        "the actual SSH peer lacks the SQLite browsing capability"
    );
    let home = Path::new(fixture.remote_home());
    assert!(
        home.is_absolute() && home.is_dir(),
        "this explicit Linux loopback fixture requires a shared private remote home"
    );
    for (name, case) in CASES.iter().filter(|(name, _)| name.starts_with("sqlite_")) {
        let sandbox = TempSandbox(
            tempfile::Builder::new()
                .prefix("lyrel-sqlite-ssh-")
                .tempdir_in(home)
                .expect("create an owned shared loopback sandbox"),
        );
        eprintln!("real native SSH SQLite contract: {name}");
        case(&*host, &sandbox);
    }
}
