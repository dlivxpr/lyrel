//! 版本化分段读取经生产 native SSH 路由到 Linux 的真实 daemon。
//!
//! 只在显式提供本轮私有 fixture（`LYREL_TEST_SSH_*`）并传入 `--ignored` 时
//! 运行；没有 fixture 的常规测试不会连接任何真实主机。
#[path = "../../../tests/support/native_ssh_graphics.rs"]
pub mod fixture;

use std::path::Path;

use fixture::NativeSshFixture;
use lyrel_core::host::read::read_file_segmented;
use lyrel_core::host::transfer::{CancelToken, SEGMENT_BYTES};
use lyrel_core::host::{Host, HostRead};

fn body(len: usize) -> Vec<u8> {
    (0..len as u32).map(|i| (i % 251) as u8).collect()
}

#[test]
#[ignore = "requires the explicit private Linux SSH fixture and a compatible candidate server; run alone with --ignored"]
fn a_read_session_streams_one_version_over_real_ssh() {
    let fixture = NativeSshFixture::start(Path::new(env!("CARGO_BIN_EXE_lyrel-server")));
    let host = fixture.connect_control();
    assert!(
        host.peer()
            .has_feature(lyrel_core::daemon::control::feature::FILE_RANGE),
        "the real SSH peer did not announce host-file-range-v1: {:?}",
        host.peer().features
    );

    let bytes = body(3 * 1024 * 1024 + 17);
    let path = fixture.remote_path("read-session.bin");
    host.write_file(&path, &bytes).unwrap();

    let mut progress = Vec::new();
    let outcome = read_file_segmented(
        &*host,
        &path,
        8 * 1024 * 1024,
        &CancelToken::default(),
        |update| progress.push(update),
    )
    .unwrap();
    assert!(
        outcome.bytes == bytes,
        "3 MiB arrives byte for byte over real SSH"
    );
    assert!(
        progress.len() > 3,
        "a real 3 MiB read uses bounded segments, got {}",
        progress.len()
    );
    let mut previous = 0;
    for update in &progress {
        assert!(
            update.read - previous <= SEGMENT_BYTES,
            "one segment carried {} bytes",
            update.read - previous
        );
        previous = update.read;
    }

    // A rewrite on the far side while one version is open must invalidate the
    // read instead of publishing a mix of both versions.
    let mut reader = host.open_read(&path).unwrap();
    let first = reader.read_segment(SEGMENT_BYTES).unwrap();
    assert_eq!(first.len() as u64, SEGMENT_BYTES);
    let mut changed = bytes.clone();
    for byte in &mut changed[SEGMENT_BYTES as usize..(SEGMENT_BYTES * 2) as usize] {
        *byte = 0xAA;
    }
    host.write_file(&path, &changed).unwrap();
    let error = reader.read_segment(SEGMENT_BYTES).unwrap_err();
    assert!(
        lyrel_core::host::read::is_file_changed(&error),
        "a real remote rewrite must invalidate the open version: {error}"
    );
}
