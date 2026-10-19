//! Wire-level transfer proof over a negotiated-compression connection.
//!
//! The ticket asks what `FakeSshd` cannot show: a session where `zlib` is the
//! *only* compression either side lists — so the handshake itself proves the
//! algorithm negotiated — carrying more than a channel window of checkable
//! bytes in both directions, and an SFTP server that keeps real files so the
//! `SftpManager` round trip and its failure wording are real.
//!
//! The client half is the production path: `connect::build_transport`,
//! `connect::build_config`, `ClientHandler`, `auth::authenticate` and
//! `SshConnection`. The server half is the registry `russh::server` plus a
//! `russh_sftp::server::Handler` rooted in a temp dir, so no user file,
//! `known_hosts` or credential is touched.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use russh::keys::PrivateKey;
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::server::{self, Auth, ChannelOpenHandle, Session};
use russh::{Channel, ChannelId, ChannelMsg};
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode,
};
use russh_sftp::server::StatusReply;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::daemon::protocol::{
    SftpEntryKind, SftpJobProgress, SftpJobState, SftpOp, SftpOpResult, SftpTransferKind,
    SftpTransferSpec,
};

use super::broker::PromptBroker;
use super::forward::RemoteForwardTable;
use super::handler::ClientHandler;
use super::session::SshConnection;
use super::sftp::SftpManager;
use super::test_support::base_spec;
use super::{ConnectionKey, SshManager, auth, connect};

/// The bytes the wire carries: fixed-width records whose only entropy is an
/// eight-digit line number, so deflate folds each 64-byte record back onto
/// earlier ones — well past the 2× margin the inflate-buffer regression needs
/// — while every byte still comes from `i` alone, so both ends of the wire
/// hold the same expectation without sharing a buffer.
fn wire_byte(i: usize) -> u8 {
    const TAIL: &[u8] = b"lyrel zlib verbatim transfer record\n";
    let line = i / 64;
    match i % 64 {
        c @ 0..=3 => b"line"[c],
        4 => b' ',
        c @ 5..=12 => b'0' + ((line / 10usize.pow(7 - (c - 5) as u32)) % 10) as u8,
        13 => b':',
        14 => b' ',
        c => TAIL[(c - 15) % TAIL.len()],
    }
}

fn wire_payload(len: usize) -> Vec<u8> {
    (0..len).map(wire_byte).collect()
}

/// `wire_payload` must compress by more than two: a less compressible stream
/// would not exercise the inflate side's bigger-than-input reads.
fn assert_compressible(payload: &[u8]) {
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(payload, 6);
    assert!(
        packed.len() * 2 < payload.len(),
        "{} bytes deflated to {} — the payload must compress better than 2x",
        payload.len(),
        packed.len()
    );
}

/// Digest of the bytes one side put on the wire; the peer recomputes it from
/// what *it* received, so a match proves the content, not the call returning.
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", md5::compute(bytes))
}

/// The SFTP handler's view of one open handle: a file for read/write, or a
/// directory remembered until `readdir` has been answered once.
enum OpenHandle {
    File(tokio::fs::File),
    Dir { path: PathBuf, done: bool },
}

/// An SFTP server backed by a real directory tree under `root`.
///
/// Remote paths are SFTP-style `/a/b` names; `map` lands them under `root`
/// and refuses `..` outright rather than resolving it, so a test's tree can
/// never escape the temp dir.
struct FsSftp {
    root: PathBuf,
    next: u64,
    handles: HashMap<String, OpenHandle>,
}

impl FsSftp {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            next: 0,
            handles: HashMap::new(),
        }
    }

    fn map(&self, remote: &str) -> Result<PathBuf, StatusReply> {
        let mut out = self.root.clone();
        for part in Path::new(remote.trim_start_matches('/')).components() {
            match part {
                Component::Normal(p) => out.push(p),
                Component::CurDir => {}
                _ => {
                    return Err(StatusCode::PermissionDenied
                        .with_message(format!("path {remote} escapes the test root")));
                }
            }
        }
        Ok(out)
    }

    fn attrs(&self, remote: String, id: u32) -> Result<Attrs, StatusReply> {
        let path = self.map(&remote)?;
        let meta = std::fs::metadata(&path).map_err(io_fail)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&meta),
        })
    }

    fn ok_status(id: u32) -> Status {
        Status {
            id,
            status_code: StatusCode::Ok,
            error_message: String::new(),
            language_tag: String::new(),
        }
    }
}

/// Maps filesystem failures to the SFTP status a client should read. The
/// message is fixed English: `io::Error::to_string` is localized on Windows
/// and would hide the cause from a test assertion.
fn io_fail(e: std::io::Error) -> StatusReply {
    match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile.with_message("no such file"),
        std::io::ErrorKind::AlreadyExists => {
            StatusCode::Failure.with_message("file already exists")
        }
        std::io::ErrorKind::PermissionDenied => {
            StatusCode::PermissionDenied.with_message("permission denied")
        }
        _ => StatusCode::Failure.with_message(e.to_string()),
    }
}

impl russh_sftp::server::Handler for FsSftp {
    type Error = StatusReply;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported.with_message("the test server does not implement that request")
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let path = self.map(&filename)?;
        let file = tokio::fs::File::from_std(
            std::fs::OpenOptions::from(pflags)
                .open(&path)
                .map_err(io_fail)?,
        );
        self.next += 1;
        let name = format!("f{}", self.next);
        self.handles.insert(name.clone(), OpenHandle::File(file));
        Ok(Handle { id, handle: name })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.handles.remove(&handle);
        Ok(Self::ok_status(id))
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let Some(OpenHandle::File(file)) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure.with_message(format!("unknown handle {handle}")));
        };
        file.seek(SeekFrom::Start(offset)).await.map_err(io_fail)?;
        let mut buf = vec![0u8; len as usize];
        let n = file.read(&mut buf).await.map_err(io_fail)?;
        if n == 0 {
            return Err(StatusCode::Eof.into());
        }
        buf.truncate(n);
        Ok(Data { id, data: buf })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let Some(OpenHandle::File(file)) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure.with_message(format!("unknown handle {handle}")));
        };
        file.seek(SeekFrom::Start(offset)).await.map_err(io_fail)?;
        file.write_all(&data).await.map_err(io_fail)?;
        file.flush().await.map_err(io_fail)?;
        Ok(Self::ok_status(id))
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.attrs(path, id)
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.attrs(path, id)
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        self.map(&path)?;
        Ok(Self::ok_status(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let mapped = self.map(&path)?;
        let meta = std::fs::metadata(&mapped).map_err(io_fail)?;
        if !meta.is_dir() {
            return Err(StatusCode::Failure.with_message(format!("{path} is not a directory")));
        }
        self.next += 1;
        let name = format!("d{}", self.next);
        self.handles.insert(
            name.clone(),
            OpenHandle::Dir {
                path: mapped,
                done: false,
            },
        );
        Ok(Handle { id, handle: name })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        let Some(OpenHandle::Dir { path, done }) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure.with_message(format!("unknown handle {handle}")));
        };
        if *done {
            return Err(StatusCode::Eof.into());
        }
        *done = true;
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&*path).map_err(io_fail)? {
            let entry = entry.map_err(io_fail)?;
            let meta = entry.metadata().map_err(io_fail)?;
            files.push(File::new(
                entry.file_name().to_string_lossy().into_owned(),
                FileAttributes::from(&meta),
            ));
        }
        Ok(Name { id, files })
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let mut out = String::from("/");
        for part in Path::new(path.trim_start_matches('/')).components() {
            if let Component::Normal(p) = part {
                out.push_str(&p.to_string_lossy());
                out.push('/');
            }
        }
        while out.len() > 1 && out.ends_with('/') {
            out.pop();
        }
        Ok(Name {
            id,
            files: vec![File::dummy(out)],
        })
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let path = self.map(&filename)?;
        std::fs::remove_file(&path).map_err(io_fail)?;
        Ok(Self::ok_status(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let path = self.map(&path)?;
        std::fs::create_dir_all(&path).map_err(io_fail)?;
        Ok(Self::ok_status(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        let path = self.map(&path)?;
        std::fs::remove_dir(&path).map_err(io_fail)?;
        Ok(Self::ok_status(id))
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        let from = self.map(&oldpath)?;
        let to = self.map(&newpath)?;
        std::fs::rename(&from, &to).map_err(io_fail)?;
        Ok(Self::ok_status(id))
    }
}

/// What the test wants back out of the server: the bytes it received on the
/// `xfer` channel, and a count of client CHANNEL_CLOSEs.
struct SshdOut {
    done_tx: mpsc::UnboundedSender<Vec<u8>>,
    closed: Arc<AtomicUsize>,
}

/// A loopback sshd that answers `none` auth, runs `xfer` as an echo verifier
/// and `sftp` against a temp dir, and counts client channel closes.
struct LoopbackSshd {
    channels: HashMap<ChannelId, Channel<server::Msg>>,
    /// The channel `exec "xfer"` was accepted on; only its data is copied.
    xfer: Option<ChannelId>,
    /// The bytes it has received, in wire order.
    incoming: Vec<u8>,
    out: SshdOut,
    root: PathBuf,
}

impl server::Handler for LoopbackSshd {
    type Error = russh::Error;

    async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<server::Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        command: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        match command {
            b"xfer" => {
                session.channel_success(channel)?;
                // Drop the session's copy of the channel: russh mirrors every
                // CHANNEL_DATA into its internal buffer for `into_stream`
                // consumers, and a hundred pending messages would stall the
                // whole session loop. `handler.data` still sees every byte.
                self.channels.remove(&channel);
                self.xfer = Some(channel);
            }
            _ => session.channel_failure(channel)?,
        }
        Ok(())
    }

    /// `data` lands here as the wire chunked it — no chunk-boundary promises,
    /// which is what makes the byte-for-byte check meaningful. Every channel's
    /// data arrives through this callback, sftp's included; only the `xfer`
    /// channel's bytes belong to the copy check.
    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.xfer == Some(channel) {
            self.incoming.extend_from_slice(data);
        }
        Ok(())
    }

    /// Client EOF ends the copy: echo every byte received back down the same
    /// channel, then a digest of what the *server* holds as stderr, then the
    /// exit status and EOF. sshd does not close the channel for the client —
    /// it reports the command finished and waits to be dismissed, which is
    /// what lets the test watch the client's own close arrive.
    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.xfer != Some(channel) {
            return Ok(());
        }
        let received = std::mem::take(&mut self.incoming);
        let _ = self.out.done_tx.send(received.clone());
        for chunk in received.chunks(16 * 1024) {
            session.data(channel, chunk.to_vec())?;
        }
        let trailer = format!("len={} md5={}\n", received.len(), digest(&received));
        session.extended_data(channel, 1, trailer.into_bytes())?;
        session.exit_status_request(channel, 0)?;
        session.eof(channel)?;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            session.channel_failure(channel)?;
            return Ok(());
        }
        session.channel_success(channel)?;
        if let Some(ch) = self.channels.remove(&channel) {
            russh_sftp::server::run(ch.into_stream(), FsSftp::new(self.root.clone())).await;
        }
        Ok(())
    }

    async fn channel_close(
        &mut self,
        _channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.out.closed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// One zlib-only loopback connection through the production client stack.
struct ZlibLink {
    conn: Arc<SshConnection>,
    /// The server's verbatim account of the `xfer` upload — one send on
    /// `channel_eof`. Unused by the sftp tests.
    received: mpsc::UnboundedReceiver<Vec<u8>>,
    closed: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for ZlibLink {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl ZlibLink {
    /// Awaits the server seeing `n` CHANNEL_CLOSEs from this client.
    async fn wait_for_closed(&self, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.closed.load(Ordering::SeqCst) < n {
            assert!(
                Instant::now() < deadline,
                "the server saw {} of {n} closes",
                self.closed.load(Ordering::SeqCst)
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

/// Runs `f` on the daemon's SSH runtime — the one `SftpManager` blocks on —
/// from a plain test thread, which is where production callers stand too.
fn block_on<F: Future>(f: F) -> F::Output {
    SshManager::global().handle().block_on(f)
}

/// Dials a loopback server whose offer is `[zlib]` — never `none` — through
/// the real transport/config/auth/handler stack. `scratch` isolates the
/// host-key decision from the user's known_hosts.
async fn connect_zlib(root: &Path, scratch: &Path) -> ZlibLink {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback port");
    let addr = listener.local_addr().expect("bound address");

    let mut server_config = server::Config::default();
    server_config.inactivity_timeout = None;
    server_config.preferred.compression = Cow::Owned(vec![russh::compression::ZLIB]);
    server_config
        .keys
        .push(PrivateKey::from(Ed25519Keypair::from_seed(&[11; 32])));
    let (done_tx, done_rx) = mpsc::unbounded_channel();
    let closed = Arc::new(AtomicUsize::new(0));
    let sshd = LoopbackSshd {
        channels: HashMap::new(),
        xfer: None,
        incoming: Vec::new(),
        out: SshdOut {
            done_tx,
            closed: Arc::clone(&closed),
        },
        root: root.to_path_buf(),
    };
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept the test client");
        let running = server::run_stream(Arc::new(server_config), socket, sshd)
            .await
            .expect("server handshake");
        let _ = running.await;
    });

    let mut spec = base_spec();
    spec.host = "127.0.0.1".into();
    spec.port = addr.port();
    spec.verify_host_keys = false;
    spec.skip_banner = true;
    // zlib is the client's *only* offer too: a completed handshake can only
    // mean both directions negotiated it.
    spec.algorithms.compression = vec!["zlib".into()];

    let transport = connect::build_transport(&spec, None)
        .await
        .expect("loopback transport");
    let config = connect::build_config(&spec);
    assert_eq!(
        config.preferred.compression.as_ref(),
        &[russh::compression::ZLIB],
        "the client must offer zlib and nothing else"
    );
    let handler = ClientHandler {
        host: spec.host.clone(),
        port: spec.port,
        verify_host_keys: false,
        skip_banner: true,
        known_hosts_path: Some(scratch.join("known_hosts")),
        broker: PromptBroker::new(Box::new(|_| true)),
        remote_forwards: RemoteForwardTable::default(),
    };
    let remote_forwards = handler.remote_forwards.clone();
    let mut handle = russh::client::connect_stream(config, transport, handler)
        .await
        .expect("client handshake against a zlib-only server");
    auth::authenticate(&mut handle, &spec, &PromptBroker::new(Box::new(|_| true)))
        .await
        .expect("none auth");
    let conn = SshConnection::new(handle, ConnectionKey::from_spec(&spec), remote_forwards);
    ZlibLink {
        conn,
        received: done_rx,
        closed,
        server,
    }
}

/// The whole fixture and the transfer fit one budget; a stall is a failure,
/// not a hang.
const BUDGET: Duration = Duration::from_secs(60);

#[test]
fn a_zlib_only_session_moves_megabytes_both_ways_verbatim() {
    block_on(async {
        let dir = tempfile::tempdir().expect("scratch dir");
        let mut link =
            tokio::time::timeout(BUDGET, connect_zlib(dir.path(), &dir.path().join("trust")))
                .await
                .expect("the zlib-only handshake must complete");

        let payload = wire_payload(3 * 1024 * 1024 + 513);
        assert_compressible(&payload);
        let mut channel = tokio::time::timeout(BUDGET, async {
            let channel = link
                .conn
                .open_session_channel()
                .await
                .expect("a session channel on the compressed link");
            channel.exec(true, "xfer").await.expect("exec xfer");
            channel
        })
        .await
        .expect("channel open must not stall");

        // Upload in chunks that straddle packet and window boundaries; the
        // 2 MiB default window forces at least one adjust round trip. The
        // whole send sits inside the budget so a stall fails instead of
        // hanging the suite.
        tokio::time::timeout(BUDGET, async {
            for chunk in payload.chunks(64 * 1024 + 17) {
                channel
                    .data(chunk)
                    .await
                    .expect("data within the negotiated window");
            }
            channel.eof().await.expect("eof the upload half");
        })
        .await
        .expect("the upload must finish, not stall");

        // The server reports status + EOF and waits; the client reads to EOF,
        // then sends the close it would send for any finished command.
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut status = None;
        let mut saw_eof = false;
        tokio::time::timeout(BUDGET, async {
            while let Some(msg) = channel.wait().await {
                match msg {
                    ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                    ChannelMsg::ExtendedData { data, .. } => stderr.extend_from_slice(&data),
                    ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                    ChannelMsg::Eof => {
                        saw_eof = true;
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("the echo must finish, not stall");

        assert_eq!(status, Some(0), "the exec must report its exit status");
        assert!(saw_eof, "the server's EOF must arrive");
        assert_eq!(
            stdout.len(),
            payload.len(),
            "the echo must be exactly as long as the upload"
        );
        assert_eq!(
            stdout, payload,
            "every echoed byte must match what was sent"
        );
        let trailer = String::from_utf8(stderr).expect("the digest trailer is utf-8");
        assert_eq!(
            trailer.trim_end(),
            format!("len={} md5={}", payload.len(), digest(&payload)),
            "the server's own account of what it received must match the upload"
        );

        let mut received = Vec::new();
        while let Ok(chunk) = link.received.try_recv() {
            received.extend_from_slice(&chunk);
        }
        assert_eq!(
            received, payload,
            "the server-side buffer itself holds the upload verbatim"
        );

        // The client closes a finished command's channel; russh replies on
        // the server's behalf and the handler counts the close arriving.
        // russh drops the reply before it reaches this `Channel`'s queue —
        // a channel that already sent its own close is off the established
        // list — so the server's count is the observable half of the RFC
        // 4254 §5.3 handshake.
        channel.close().await.expect("close the finished channel");
        link.wait_for_closed(1).await;
        drop(link);
    });
}

/// put/list/read over the compressed link, with the failures a user actually
/// meets: a missing file, a missing directory, a duplicate create.
#[test]
fn sftp_round_trips_content_and_reports_real_errors() {
    // `SftpManager` blocks on the SSH runtime itself, so this test stays on
    // its own thread — putting the ops inside `block_on` would nest.
    let dir = tempfile::tempdir().expect("scratch dir");
    let remote_root = dir.path().join("remote");
    std::fs::create_dir_all(&remote_root).expect("remote root");
    // The timeout is built inside the future: `tokio::time::timeout` needs a
    // live runtime at construction, and the test thread has none.
    let link = block_on(async {
        tokio::time::timeout(
            BUDGET,
            connect_zlib(&remote_root, &dir.path().join("trust")),
        )
        .await
    })
    .expect("the zlib-only handshake must complete");

    let sftp = SftpManager::global();
    let content = wire_payload(512 * 1024 + 33);

    // A write under a directory that does not exist must fail, and the
    // message must say which — a bare "Failure" would hide it.
    let denied = sftp
        .put_bytes(&link.conn, "/missing-parent/file.bin", &content, &|_| {})
        .expect_err("a missing parent directory cannot hold a file");
    assert!(
        denied.to_lowercase().contains("no such file"),
        "the failure should name the cause: {denied}"
    );

    assert_eq!(
        sftp.op(
            &link.conn,
            &SftpOp::Mkdir {
                path: "/inbox".into(),
            },
        ),
        SftpOpResult::Done,
        "mkdir over sftp"
    );
    sftp.put_bytes(&link.conn, "/inbox/blob.bin", &content, &|_| {})
        .expect("upload over the compressed link");

    let entries = sftp.list(&link.conn, "/inbox").expect("list /inbox");
    let blob = entries
        .iter()
        .find(|e| e.name == "blob.bin")
        .expect("the upload must appear in the listing");
    assert_eq!(blob.size, content.len() as u64);
    assert_eq!(blob.kind, SftpEntryKind::File);

    let read_back = sftp.op(
        &link.conn,
        &SftpOp::ReadFile {
            path: "/inbox/blob.bin".into(),
            max_bytes: 4 * 1024 * 1024,
        },
    );
    match read_back {
        SftpOpResult::File { bytes, entry } => {
            assert_eq!(bytes, content, "the download must be byte-exact");
            assert_eq!(entry.size, content.len() as u64);
        }
        other => panic!("ReadFile must return the file's bytes: {other:?}"),
    }

    let missing = sftp.op(
        &link.conn,
        &SftpOp::ReadFile {
            path: "/inbox/gone.bin".into(),
            max_bytes: 1024,
        },
    );
    let SftpOpResult::Error(msg) = missing else {
        panic!("reading an absent file must fail: {missing:?}");
    };
    assert!(
        msg.to_lowercase().contains("no such file"),
        "the absent file's error should say so: {msg}"
    );

    let absent_dir = sftp
        .list(&link.conn, "/no-such-dir")
        .expect_err("a missing directory cannot be listed");
    assert!(
        absent_dir.to_lowercase().contains("no such file"),
        "the missing directory's error should say so: {absent_dir}"
    );

    let duplicate = sftp.op(
        &link.conn,
        &SftpOp::CreateFile {
            path: "/inbox/blob.bin".into(),
        },
    );
    let SftpOpResult::Error(msg) = duplicate else {
        panic!("creating over an existing file must fail: {duplicate:?}");
    };
    assert!(
        msg.to_lowercase().contains("exist"),
        "the duplicate create should say the file exists: {msg}"
    );

    drop(link);
}

/// The queued transfer jobs — the Files panel's path — move a file up and the
/// same file back down, content checked on both legs.
#[test]
fn sftp_transfer_jobs_round_trip_a_file() {
    let dir = tempfile::tempdir().expect("scratch dir");
    let remote_root = dir.path().join("remote");
    std::fs::create_dir_all(&remote_root).expect("remote root");
    let link = block_on(async {
        tokio::time::timeout(
            BUDGET,
            connect_zlib(&remote_root, &dir.path().join("trust")),
        )
        .await
    })
    .expect("the zlib-only handshake must complete");

    let sftp = SftpManager::global();
    let local_up = dir.path().join("up.bin");
    let content = wire_payload(768 * 1024 + 7);
    std::fs::write(&local_up, &content).expect("write the upload source");

    assert_eq!(
        sftp.op(
            &link.conn,
            &SftpOp::Mkdir {
                path: "/xfer".into(),
            },
        ),
        SftpOpResult::Done,
        "the upload's parent directory"
    );

    let up_id = sftp
        .start_transfer(
            &link.conn,
            SftpTransferSpec {
                pane_id: 7,
                kind: SftpTransferKind::Upload,
                local: local_up.clone(),
                remote: "/xfer/blob.bin".into(),
                recursive: false,
            },
        )
        .expect("queue the upload");
    let up = await_job(sftp, 7, up_id);
    assert_eq!(up.state, SftpJobState::Done, "upload: {:?}", up.error);
    assert_eq!(up.bytes_done, content.len() as u64);
    assert_eq!(up.bytes_total, content.len() as u64);
    assert_eq!(
        std::fs::read(remote_root.join("xfer/blob.bin")).expect("the remote copy exists"),
        content,
        "the upload must land byte-exact"
    );

    let local_down = dir.path().join("down.bin");
    let down_id = sftp
        .start_transfer(
            &link.conn,
            SftpTransferSpec {
                pane_id: 7,
                kind: SftpTransferKind::Download,
                local: local_down.clone(),
                remote: "/xfer/blob.bin".into(),
                recursive: false,
            },
        )
        .expect("queue the download");
    let down = await_job(sftp, 7, down_id);
    assert_eq!(down.state, SftpJobState::Done, "download: {:?}", down.error);
    assert_eq!(down.bytes_done, content.len() as u64);
    assert_eq!(
        std::fs::read(&local_down).expect("the downloaded file exists"),
        content,
        "the download must land byte-exact"
    );

    // A download of a file that is not there must end in an Error whose
    // message carries the cause.
    let gone_id = sftp
        .start_transfer(
            &link.conn,
            SftpTransferSpec {
                pane_id: 7,
                kind: SftpTransferKind::Download,
                local: dir.path().join("never.bin"),
                remote: "/xfer/absent.bin".into(),
                recursive: false,
            },
        )
        .expect("queue the doomed download");
    let gone = await_job(sftp, 7, gone_id);
    assert_eq!(gone.state, SftpJobState::Error);
    let msg = gone.error.expect("an error carries a message");
    assert!(
        msg.to_lowercase().contains("no such file"),
        "the missing download's error should say so: {msg}"
    );

    drop(link);
}

/// Waits out a transfer job; `start_transfer` runs it on the SSH runtime, so
/// the job's terminal state is the truth, not the call returning.
fn await_job(sftp: &SftpManager, pane_id: u64, job_id: u64) -> SftpJobProgress {
    let deadline = Instant::now() + BUDGET;
    loop {
        let jobs = sftp.list_jobs(pane_id);
        if let Some(job) = jobs.iter().find(|j| j.job_id == job_id)
            && job.state != SftpJobState::Running
        {
            return job.clone();
        }
        assert!(Instant::now() < deadline, "job {job_id} never settled");
        std::thread::sleep(Duration::from_millis(20));
    }
}
