//! 连接路由、协议协商、Spawn/Attach 判定与接入前图形 admission。
//! reader 接管之后不再读取此流；attach 预读字节完整移交单一 reader。
use super::{
    NativeSshSpec, PaneRoute, PtySource, RemoteTerminal, RestoreFrom, ShellSpec, SpawnCancellation,
    TermSize, win_size,
};
use crate::daemon::protocol::{ClientMsg, DaemonMsg};
use crate::daemon::transport::{self, Stream};
#[cfg(test)]
use std::io::Read as _;
use std::path::PathBuf;

pub(crate) struct PaneHandshake {
    stream: Stream,
    images: crate::terminal::images::ImageStore,
}

impl PaneHandshake {
    fn open(route: &PaneRoute, cell_w: u16, cell_h: u16) -> anyhow::Result<Self> {
        let mut stream = connect_routed(route)?;
        Self::negotiate_pane_protocol(&mut stream, attach_reply_wait(route))?;
        let images = crate::terminal::images::ImageStore::new();
        images.set_cell_metrics(cell_w, cell_h);
        Self::send_graphics_capacity(&mut stream, &images)?;
        Ok(Self { stream, images })
    }

    pub fn spawn_on(
        route: &PaneRoute,
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        cwd: Option<PathBuf>,
        shell: Option<ShellSpec>,
        owner: Option<String>,
        restore: Option<RestoreFrom>,
        cancellation: Option<&SpawnCancellation>,
    ) -> anyhow::Result<(RemoteTerminal, u64)> {
        let retry_cwd = cwd.clone();
        let retry_shell = shell.clone();
        let retry_owner = owner.clone();
        let retry_restore = restore.clone();
        match Self::spawn_once(
            route,
            size,
            cell_w,
            cell_h,
            cwd,
            shell,
            owner,
            restore,
            cancellation,
        ) {
            Ok(term) => Ok(term),
            Err(first_err) if route.is_local() && daemon_not_listening(&first_err) => {
                log::warn!(
                    "local daemon unavailable during spawn; attempting recovery: {first_err:#}"
                );
                if let Err(start_err) = crate::daemon::spawn::ensure_running() {
                    return Err(anyhow::anyhow!(
                        "daemon not running ({first_err}); starting one failed: {start_err}"
                    ));
                }
                Self::spawn_once(route, size, cell_w, cell_h, retry_cwd, retry_shell, retry_owner, retry_restore, cancellation)
                    .map_err(|second_err| {
                        anyhow::anyhow!(
                            "daemon not running ({first_err}); started one but Spawn still failed: {second_err}"
                        )
                    })
            }
            Err(first_err)
                if route.is_local() && daemon_disconnected_before_spawn_reply(&first_err) =>
            {
                log::warn!(
                    "spawn connection ended before reply; probing daemon without forcing a restart: {first_err:#}"
                );
                if let Err(restart_err) = crate::daemon::spawn::ensure_running() {
                    return Err(anyhow::anyhow!(
                        "daemon disconnected before Spawn reply ({first_err}); recovery failed: {restart_err}"
                    ));
                }
                Self::spawn_once(route, size, cell_w, cell_h, retry_cwd, retry_shell, retry_owner, retry_restore, cancellation).map_err(|second_err| {
                    anyhow::anyhow!(
                        "daemon disconnected before Spawn reply ({first_err}); recovery attempted but Spawn still failed: {second_err}"
                    )
                })
            }
            Err(err) => Err(err),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_once(
        route: &PaneRoute,
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        cwd: Option<PathBuf>,
        shell: Option<ShellSpec>,
        owner: Option<String>,
        restore: Option<RestoreFrom>,
        cancellation: Option<&SpawnCancellation>,
    ) -> anyhow::Result<(RemoteTerminal, u64)> {
        let Self { mut stream, images } = Self::open(route, cell_w, cell_h)?;
        let await_claim = route.is_local() && cancellation.is_some();
        if await_claim
            && !crate::daemon::spawn::local_daemon_supports(
                crate::daemon::protocol::FEATURE_SPAWN_CLAIM,
            )
        {
            anyhow::bail!(
                "the local daemon does not support cancellable pane creation; restart it to update"
            );
        }
        let win = win_size(size, cell_w, cell_h);

        let workspace = spawn_workspace(owner.as_deref(), route);
        let spawned_in = cwd.clone();

        let owner = owner.filter(|_| {
            route.is_local()
                && crate::daemon::spawn::local_daemon_supports(
                    crate::daemon::protocol::FEATURE_PANE_OWNER,
                )
        });

        // Only the local daemon's feature list is known here; a routed spawn
        // reaches a server whose build we have not asked about, and one that
        // predates the field would silently drop it. Sending it anyway would
        // cost nothing but would make the log claim a restore that never
        // happened, so the local case is the only one that asks.
        let restore = restore.and_then(|want| {
            let local = route.is_local();
            let supported = crate::daemon::spawn::local_daemon_supports(
                crate::daemon::protocol::FEATURE_RESTORE_SCROLLBACK,
            );
            if local && supported {
                return Some(want);
            }
            // Said out loud, because the symptom of dropping it here is a pane
            // that opens blank — which is also what a pane with nothing stored
            // looks like, and what a daemon that refused would produce. Three
            // causes, one appearance; without this line the only way to tell
            // them apart is to read the source.
            log::info!(
                "not asking to restore pane {}'s screen: local={local} \
                 daemon-supports-restore={supported}",
                want.pane_id
            );
            None
        });

        ClientMsg::Spawn {
            cwd,
            size: win,
            shell,
            owner,
            workspace,
            restore,
            allow_remote_clipboard_write: route.allow_remote_clipboard_write(),
            await_claim,
        }
        .encode(&mut stream)?;
        let reply = if let Some(cancellation) = cancellation.filter(|_| await_claim) {
            cancellable_spawn_reply_with_images(&mut stream, cancellation, &images)?
        } else {
            spawn_reply(&mut stream, attach_reply_wait(route), "Spawn", &images)?
        };
        let pane_id = match reply {
            DaemonMsg::Spawned { pane_id } => pane_id,
            // Passed through, not wrapped: the caller already logs which
            // spawn this was, and the window shows only this text.
            DaemonMsg::Error(msg) => return Err(anyhow::anyhow!(msg)),
            other => {
                return Err(anyhow::anyhow!(
                    "unexpected daemon reply to Spawn: {other:?}"
                ));
            }
        };

        let mut term = RemoteTerminal::from_stream_parts(
            stream,
            size,
            Vec::new(),
            PtySource::for_route(route),
            false,
            images,
        )?;
        term.connection
            .finish_spawn(route.clone(), await_claim.then_some((pane_id, win)));
        term.seed_cwd(spawned_in);
        Ok((term, pane_id))
    }

    fn negotiate_pane_protocol(
        stream: &mut Stream,
        wait: std::time::Duration,
    ) -> anyhow::Result<()> {
        let _ = stream.set_read_timeout(Some(wait));
        let result = crate::daemon::protocol::negotiate_pane(stream);
        let _ = stream.set_read_timeout(None);
        result?;
        Ok(())
    }
    fn send_graphics_capacity(
        stream: &mut Stream,
        images: &crate::terminal::images::ImageStore,
    ) -> anyhow::Result<()> {
        let (encoded_bytes, decoded_bytes) = images.graphics_capacity();
        ClientMsg::GraphicsCapacity {
            encoded_bytes,
            decoded_bytes,
            offers: true,
        }
        .encode(stream)?;
        Ok(())
    }

    pub fn attach_on(
        route: &PaneRoute,
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        pane_id: u64,
    ) -> anyhow::Result<RemoteTerminal> {
        let Self { mut stream, images } = Self::open(route, cell_w, cell_h)?;
        let win = win_size(size, cell_w, cell_h);

        ClientMsg::Attach {
            pane_id,
            size: win,
            allow_remote_clipboard_write: route.allow_remote_clipboard_write(),
        }
        .encode(&mut stream)?;
        let buffered = attach_reply_prefix_with_images(
            &mut stream,
            pane_id,
            attach_reply_wait(route),
            &images,
        )?;
        let mut term = RemoteTerminal::from_stream_parts(
            stream,
            size,
            buffered,
            PtySource::for_route(route),
            true,
            images,
        )?;
        term.connection.finish_spawn(route.clone(), None);
        Ok(term)
    }

    /// Opens the daemon connection a relink will adopt, and waits for the
    /// daemon's verdict on the `Attach`. Bytes read past the verdict are
    /// returned so the adopted reader loses none of the replay. Waiting here
    /// is what makes a relink's failure classifiable: fire-and-forget handed
    /// a "no such pane" refusal to the reader thread, which could only die
    /// with it — indistinguishable from the link dropping again.
    pub fn open_relink(
        route: &PaneRoute,
        pane_id: u64,
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        images: crate::terminal::images::ImageStore,
    ) -> anyhow::Result<(Stream, Vec<u8>)> {
        let mut stream = connect_routed(route)?;
        Self::negotiate_pane_protocol(&mut stream, attach_reply_wait(route))?;
        Self::send_graphics_capacity(&mut stream, &images)?;
        ClientMsg::Attach {
            pane_id,
            size: win_size(size, cell_w, cell_h),
            allow_remote_clipboard_write: route.allow_remote_clipboard_write(),
        }
        .encode(&mut stream)?;
        let buffered = attach_reply_prefix_with_images(
            &mut stream,
            pane_id,
            attach_reply_wait(route),
            &images,
        )?;
        Ok((stream, buffered))
    }

    /// `remote_start_dir` is a directory on the far host, carried as
    /// [`ClientMsg::SpawnNativeSsh`]'s `cwd`.
    pub fn spawn_native_ssh(
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        remote_start_dir: Option<PathBuf>,
        spec: Box<NativeSshSpec>,
    ) -> anyhow::Result<(RemoteTerminal, u64)> {
        match Self::spawn_native_ssh_once(
            size,
            cell_w,
            cell_h,
            remote_start_dir.clone(),
            spec.clone(),
        ) {
            Err(first_err) if daemon_disconnected_before_spawn_reply(&first_err) => {
                log::warn!(
                    "spawn connection ended before reply; probing daemon without forcing a restart: {first_err:#}"
                );
                if let Err(restart_err) = crate::daemon::spawn::ensure_running() {
                    return Err(anyhow::anyhow!(
                        "daemon disconnected before SpawnNativeSsh reply ({first_err}); recovery failed: {restart_err}"
                    ));
                }
                Self::spawn_native_ssh_once(size, cell_w, cell_h, remote_start_dir, spec).map_err(|second_err| {
                    anyhow::anyhow!(
                        "daemon disconnected before SpawnNativeSsh reply ({first_err}); recovery attempted but it still failed: {second_err}"
                    )
                })
            }
            other => other,
        }
    }

    fn spawn_native_ssh_once(
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        remote_start_dir: Option<PathBuf>,
        spec: Box<NativeSshSpec>,
    ) -> anyhow::Result<(RemoteTerminal, u64)> {
        let Self { mut stream, images } = Self::open(&PaneRoute::Local, cell_w, cell_h)?;
        let win = win_size(size, cell_w, cell_h);
        let endpoint = (spec.host.clone(), spec.port);
        let user = spec.user.clone();
        let auto_supplied_password = spec.password.is_some();

        ClientMsg::SpawnNativeSsh {
            cwd: remote_start_dir,
            size: win,
            spec,
        }
        .encode(&mut stream)?;
        // Native SSH is always dialled through the local daemon, whatever the
        // far end turns out to be, so this waits on the local budget.
        let pane_id = match spawn_reply(
            &mut stream,
            attach_reply_wait(&PaneRoute::Local),
            "SpawnNativeSsh",
            &images,
        )? {
            DaemonMsg::Spawned { pane_id } => pane_id,
            DaemonMsg::Error(msg) => {
                return Err(anyhow::anyhow!("daemon refused SpawnNativeSsh: {msg}"));
            }
            other => {
                return Err(anyhow::anyhow!(
                    "unexpected daemon reply to SpawnNativeSsh: {other:?}"
                ));
            }
        };

        // The local daemon dialled this one, but it opened no pty for it: the
        // pane is an ssh channel bridged straight through, so the bytes are the
        // far host's raw pty and no conhost ever sees them.
        let mut term = RemoteTerminal::from_stream_parts(
            stream,
            size,
            Vec::new(),
            PtySource::Raw,
            false,
            images,
        )?;
        term.ssh_endpoint = Some(endpoint);
        term.ssh_user = Some(user);
        term.auto_supplied_password = auto_supplied_password;
        Ok((term, pane_id))
    }
}

/// The workspace id a spawn carries, so the pane's shell gets `$LYREL_WS` and a
/// CLI running in it can use workspace-scoped verbs with no argument.
///
/// This is the same id as `owner`, but decided separately: `owner` is also
/// gated on FEATURE_PANE_OWNER, while the workspace field rides the owned spawn
/// kind and needs no feature probe. Local routes only — a remote server keeps
/// its own machine tree, and this id names a workspace in ours.
fn spawn_workspace(owner: Option<&str>, route: &PaneRoute) -> Option<String> {
    owner.filter(|_| route.is_local()).map(|id| id.to_string())
}

fn daemon_not_listening(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            )
        })
    })
}

/// How long an `Attach` may stay silent before it is called unanswered.
///
/// Only a silent connection ever pays this: a dead pane answers `Error` at
/// once and a live one answers `Size` at once, so the budget is not a cost in
/// the common case. What bounds it is the caller — a local restore attaches
/// synchronously on the UI thread, one pane after another, so N silent panes
/// hold the window still for N times this. What argues for more is the other
/// side of the same silence: a daemon merely slow to serve — an execve handoff
/// keeps the listener and its backlog across the exec, and a fresh daemon is
/// adopting panes and seeding ids before it can take an `Attach` — would have
/// served this connection a moment later, and calling it unanswered spawns a
/// fresh pane over a live one, carries the live pane's history onto it and
/// starts an agent resume against a session the old process still holds. The
/// [`AttachUnanswered`] verdict is confirmed against that case in `attach_on`
/// rather than by waiting longer here.
fn attach_reply_wait(route: &PaneRoute) -> std::time::Duration {
    match route.is_local() {
        true => std::time::Duration::from_secs(2),
        false => std::time::Duration::from_secs(15),
    }
}

fn graphics_admission_reply(
    message: DaemonMsg,
    stream: &mut Stream,
    images: &crate::terminal::images::ImageStore,
) -> std::io::Result<Option<DaemonMsg>> {
    match message {
        DaemonMsg::GraphicsOffer {
            token,
            key,
            width,
            height,
            encoded_bytes,
            metadata_bytes,
        } => {
            let accepted = images.offer(token, key, width, height, encoded_bytes, metadata_bytes);
            if let Err(error) = (ClientMsg::GraphicsGrant { token, accepted }).encode(stream) {
                images.cancel_offer(token);
                return Err(error);
            }
            Ok(None)
        }
        DaemonMsg::GraphicsCancel { token } => {
            images.cancel_offer(token);
            Ok(None)
        }
        message => Ok(Some(message)),
    }
}

#[cfg(test)]
fn cancellable_spawn_reply(
    stream: &mut Stream,
    cancellation: &SpawnCancellation,
) -> anyhow::Result<DaemonMsg> {
    cancellable_spawn_reply_with_images(
        stream,
        cancellation,
        &crate::terminal::images::ImageStore::new(),
    )
}

/// Polling only bounds cancellation latency, not pane creation. Read's caller
/// retains partial frames, so a delayed or split Spawned frame is never lost.
fn cancellable_spawn_reply_with_images(
    stream: &mut Stream,
    cancellation: &SpawnCancellation,
    images: &crate::terminal::images::ImageStore,
) -> anyhow::Result<DaemonMsg> {
    struct Reader<'a> {
        stream: &'a mut Stream,
        cancellation: &'a SpawnCancellation,
    }
    impl std::io::Read for Reader<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            loop {
                if self.cancellation.cancelled() {
                    return Err(std::io::Error::other("pane spawn cancelled"));
                }
                match self.stream.read(bytes) {
                    Err(error) if would_block(&error) => continue,
                    result => return result,
                }
            }
        }
    }
    stream.set_read_timeout(Some(std::time::Duration::from_millis(100)))?;
    let reply = loop {
        let message = DaemonMsg::read(&mut Reader {
            stream: &mut *stream,
            cancellation,
        });
        match message.and_then(|message| graphics_admission_reply(message, stream, images)) {
            Ok(Some(message)) => break Ok(message),
            Ok(None) => continue,
            Err(error) => break Err(error),
        }
    };
    let reset = stream.set_read_timeout(None);
    if cancellation.cancelled() {
        let _ = stream.shutdown(std::net::Shutdown::Both);
        images.cancel_unclaimed_offers();
        anyhow::bail!("pane spawn cancelled");
    }
    reset?;
    if reply.is_err() {
        images.cancel_unclaimed_offers();
    }
    reply.map_err(|error| anyhow::Error::new(error).context("reading the daemon's answer to Spawn"))
}

/// Legacy synchronous/native-SSH callers retain their bounded exchange. GUI
/// shell creation instead uses `cancellable_spawn_reply`: expiration is not a
/// failed Spawn, because the daemon may still be creating its real pane.
fn spawn_reply(
    stream: &mut Stream,
    wait: std::time::Duration,
    what: &str,
    images: &crate::terminal::images::ImageStore,
) -> anyhow::Result<DaemonMsg> {
    let _ = stream.set_read_timeout(Some(wait));
    let reply = loop {
        match DaemonMsg::read(stream)
            .and_then(|message| graphics_admission_reply(message, stream, images))
        {
            Ok(Some(message)) => break Ok(message),
            Ok(None) => continue,
            Err(error) => break Err(error),
        }
    };
    let _ = stream.set_read_timeout(None);
    if reply.is_err() {
        images.cancel_unclaimed_offers();
    }
    match reply {
        Ok(msg) => Ok(msg),
        Err(e) if would_block(&e) => {
            log::warn!("daemon request timed out; request={what} timeout={wait:?}");
            Err(anyhow::anyhow!("no answer to {what} within {wait:?}"))
        }
        Err(e) => {
            Err(anyhow::Error::new(e).context(format!("reading the daemon's answer to {what}")))
        }
    }
}

/// An `Attach` that produced no bytes within its wait — nobody served the
/// connection. Distinct from a refusal (`Error` frame) and from a hangup so
/// the caller can say the true thing: the pane may well still exist.
#[derive(Debug)]
struct AttachUnanswered {
    pane_id: u64,
    wait: std::time::Duration,
}

impl std::fmt::Display for AttachUnanswered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the daemon did not answer Attach for pane {} within {:?} (an attached pane replays \
             its screen at once, so nobody is serving this connection)",
            self.pane_id, self.wait
        )
    }
}

impl std::error::Error for AttachUnanswered {}

/// Whether `err` is an `Attach` that went unanswered, as opposed to refused or
/// hung up on. The pane behind an unanswered attach may still be alive.
#[cfg(all(test, unix))]
fn attach_unanswered(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.downcast_ref::<AttachUnanswered>().is_some())
}

/// The daemon explicitly reported that the requested pane does not exist.
/// Other Error frames (including graphics admission failures) are retryable
/// attach failures, not evidence that the original session is gone.
#[derive(Debug)]
struct AttachRefused {
    message: String,
}

impl std::fmt::Display for AttachRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "daemon refused Attach: {}", self.message)
    }
}

impl std::error::Error for AttachRefused {}

/// Whether the daemon explicitly reported the requested pane missing. Only
/// this verdict permits abandoning a relink or spawning a replacement shell.
pub fn attach_refused(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.downcast_ref::<AttachRefused>().is_some())
}

/// Reads far enough into the daemon's answer to an `Attach` to classify it, and
/// hands back every byte read so the reader thread loses none of the replay.
///
/// Silence is a verdict, not a quiet pane. A daemon that attached replays the
/// pane's ring before it reads a byte of our input, and the ring always holds
/// a segment, so the first frame on a good attach is a `Size` — a quiet pane
/// still sends that. An `Attach` that produced nothing within `wait` is one
/// nobody is serving: a daemon still mid-restart, a socket held open by a
/// process that will never read it. Taken for success it gave the window a
/// pane drawn as restored whose every keystroke went into that socket — no
/// fresh shell, no restored-screen banner, no agent resume, and Ctrl-C did
/// nothing (#673). Treat it as an attach failure and retain the pane identity;
/// only an explicit missing-pane reply allows a replacement shell.
#[cfg(test)]
pub(super) fn attach_reply_prefix(
    stream: &mut Stream,
    pane_id: u64,
    wait: std::time::Duration,
) -> anyhow::Result<Vec<u8>> {
    attach_reply_prefix_with_images(
        stream,
        pane_id,
        wait,
        &crate::terminal::images::ImageStore::new(),
    )
}

fn attach_reply_prefix_with_images(
    stream: &mut Stream,
    pane_id: u64,
    wait: std::time::Duration,
    images: &crate::terminal::images::ImageStore,
) -> anyhow::Result<Vec<u8>> {
    let result = attach_reply_prefix_inner(stream, pane_id, wait, images);
    if result.is_err() {
        images.cancel_unclaimed_offers();
        let _ = stream.set_read_timeout(None);
    }
    result
}

fn attach_reply_prefix_inner(
    stream: &mut Stream,
    pane_id: u64,
    wait: std::time::Duration,
    images: &crate::terminal::images::ImageStore,
) -> anyhow::Result<Vec<u8>> {
    use std::io::Read as _;

    let _ = stream.set_read_timeout(Some(wait));
    let mut buffered: Vec<u8> = Vec::new();
    let mut scratch = [0u8; 4096];
    let mut kind = None;
    while kind.is_none() {
        match stream.read(&mut scratch) {
            Ok(0) => {
                let _ = stream.set_read_timeout(None);
                return Err(anyhow::anyhow!(
                    "the daemon closed the connection without answering Attach for pane {pane_id}"
                ));
            }
            Ok(n) => buffered.extend_from_slice(&scratch[..n]),
            Err(e) if would_block(&e) => break,
            Err(e) => {
                let _ = stream.set_read_timeout(None);
                return Err(anyhow::Error::new(e).context(format!(
                    "reading the daemon's answer to Attach for pane {pane_id}"
                )));
            }
        }
        kind = crate::daemon::protocol::peek_frame_kind(&buffered);
        while kind.is_some_and(crate::daemon::protocol::is_graphics_admission_kind) {
            let Some((frame_kind, payload)) = crate::daemon::protocol::take_frame(&mut buffered)?
            else {
                kind = None;
                break;
            };
            let message = DaemonMsg::from_frame(frame_kind, payload)?;
            graphics_admission_reply(message, stream, images)?;
            kind = crate::daemon::protocol::peek_frame_kind(&buffered);
        }
    }
    let _ = stream.set_read_timeout(None);
    if buffered.is_empty() {
        return Err(anyhow::Error::new(AttachUnanswered { pane_id, wait }));
    }
    if !kind.is_some_and(crate::daemon::protocol::is_error_kind) {
        return Ok(buffered);
    }
    let message = read_error_frame(stream, &mut buffered, wait).ok_or_else(|| {
        anyhow::anyhow!("incomplete daemon error answering Attach for pane {pane_id}")
    })?;
    if message == format!("no such pane {pane_id}") {
        Err(anyhow::Error::new(AttachRefused { message }))
    } else {
        Err(anyhow::anyhow!("daemon refused Attach: {message}"))
    }
}

fn read_error_frame(
    stream: &mut Stream,
    buffered: &mut Vec<u8>,
    wait: std::time::Duration,
) -> Option<String> {
    use std::io::Read as _;

    let _ = stream.set_read_timeout(Some(wait));
    let mut scratch = [0u8; 1024];
    let message = loop {
        match crate::daemon::protocol::take_frame(buffered) {
            Ok(Some(frame)) => match DaemonMsg::from_frame(frame.0, frame.1) {
                Ok(DaemonMsg::Error(message)) => break Some(message),
                _ => break None,
            },
            Ok(None) => match stream.read(&mut scratch) {
                Ok(0) => break None,
                Ok(n) => buffered.extend_from_slice(&scratch[..n]),
                Err(_) => break None,
            },
            Err(_) => break None,
        }
    };
    let _ = stream.set_read_timeout(None);
    message
}

fn would_block(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

fn daemon_disconnected_before_spawn_reply(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::BrokenPipe
            )
        })
    })
}

pub(super) fn connect() -> anyhow::Result<Stream> {
    transport::connect().map_err(|e| {
        anyhow::Error::new(e).context(format!(
            "connect to daemon at {}",
            transport::endpoint_display()
        ))
    })
}

pub(super) fn connect_routed(route: &PaneRoute) -> anyhow::Result<Stream> {
    if let PaneRoute::Unroutable(reason) = route {
        return Err(anyhow::anyhow!("{reason}"));
    }
    let Some(header) = route.header() else {
        return connect();
    };

    lyrel_core::host::guard_off_ui();

    // No `ensure_wsl_server` here on purpose. The daemon runs exactly the same
    // probe inside `router::open_link` before it opens the link, so asking from
    // this side too bought nothing and cost a second full round of `wsl.exe`
    // invocations — five of them, serially, on every single pane. On a machine
    // where a `wsl.exe` round trip is slow (issue #454 measured 3.3s) that
    // duplicate was half of the wait before a new tab could take a key.
    //
    // Nothing is lost by dropping it: the returned path was discarded, the
    // failure is reported just as well through the route ack below, and the
    // first-install consent question still reaches this process — the daemon
    // runs its probe under `RouteSetup::blocking`, which installs the relay
    // that turns the question into a frame on this very connection.
    let mut stream = connect()?;
    let ack = crate::daemon::router::negotiate(&mut stream, header)
        .map_err(|e| anyhow::anyhow!("route this pane to {}: {e}", header.describe()))?;
    log::debug!(
        "pane routed to {} over {}",
        header.describe(),
        ack.link.as_deref().unwrap_or("?")
    );
    Ok(stream)
}

#[cfg(all(test, unix))]
mod attach_tests;
#[cfg(test)]
mod spawn_tests;
