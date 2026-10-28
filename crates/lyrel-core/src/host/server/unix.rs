//! Unix control socket discovery, private binding, and listener ownership.
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::host::SharedHost;

use super::{CONTROL_SOCK_ENV, Services, serve_with};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};

const MAX_SOCKET_PATH_BYTES: usize = 100;

/// The control endpoint, derived from the config dir exactly the way the
/// pane endpoint is (`transport::socket_path_for`).
///
/// It used to sit in `$XDG_RUNTIME_DIR/lyrel` or `~/.local/share/lyrel`,
/// which meant it did not follow `--config-dir` while the pane socket did.
/// Two consequences, both real: a `--config-dir` instance published the
/// *default* control socket to the shells it spawned, so a CLI inside it
/// talked to a different server entirely; and because both endpoints were
/// named `daemon.sock`, telling them apart depended on their directories
/// differing — so deriving one from the other by filename silently yielded
/// the wrong socket. They are siblings now, distinguished by name, which is
/// what the Windows arm has always done (`control.port`/`daemon.port`).
pub fn control_socket_path() -> io::Result<PathBuf> {
    if let Some(explicit) = std::env::var_os(CONTROL_SOCK_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(explicit));
    }

    let dir = crate::core::config::config_dir_path().ok_or_else(|| {
        io::Error::other("no config directory to place the control socket in")
    })?;

    let fallbacks: Vec<PathBuf> = [runtime_dir(), Some(std::env::temp_dir())]
        .into_iter()
        .flatten()
        .collect();
    socket_path_in(&dir, &fallbacks)
}

pub(super) const CONTROL_SOCK_FILE: &str = "control.sock";

pub(crate) fn socket_path_in(dir: &Path, fallbacks: &[PathBuf]) -> io::Result<PathBuf> {
    let inline = dir.join(CONTROL_SOCK_FILE);
    if fits(&inline) {
        return Ok(inline);
    }

    use std::os::unix::ffi::OsStrExt as _;
    // `-control` keeps this clear of the pane socket's fallback name, which
    // hashes the same directory and would otherwise land on the same file.
    let name = format!(
        "lyrel-{:016x}-control.sock",
        crate::host::fnv1a64(dir.as_os_str().as_bytes())
    );
    for base in fallbacks {
        let candidate = base.join(&name);
        if fits(&candidate) {
            return Ok(candidate);
        }
    }
    Err(io::Error::other(format!(
        "no directory short enough for a control socket ({MAX_SOCKET_PATH_BYTES}-byte \
         sun_path limit); set {CONTROL_SOCK_ENV} to one that is"
    )))
}

fn runtime_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
}

fn fits(p: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt as _;
    p.as_os_str().as_bytes().len() <= MAX_SOCKET_PATH_BYTES
}

/// Whether a control server is answering at `path` right now.
///
/// This, and not the errno a failed bind carries, is the question that
/// matters to a daemon whose listener would not open. [`bind_control_socket`]
/// does clear the ordinary leftover — it connects first and unlinks a socket
/// nobody is behind — but what it cannot clear it hands back as `AddrInUse`
/// all the same: a directory standing where the socket goes, a file owned by
/// another user. Those look exactly like a live server in the error, and
/// nothing is listening behind either. Only a connect that completes tells
/// them apart.
pub(crate) fn control_socket_answers(path: &Path) -> bool {
    UnixStream::connect(path).is_ok()
}

pub fn control_endpoint_answers() -> bool {
    control_socket_path().is_ok_and(|path| control_socket_answers(&path))
}

pub fn bind_control_socket(path: &Path) -> io::Result<UnixListener> {
    let parent = path.parent().unwrap_or(Path::new("."));
    if !parent.exists() {
        std::fs::create_dir_all(parent)?;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }

    if path.exists() {
        match UnixStream::connect(path) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!(
                        "a control server is already listening on {}",
                        path.display()
                    ),
                ));
            }
            Err(_) => {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    let listener = bind_private(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

pub(super) fn bind_private(path: &Path) -> io::Result<UnixListener> {
    static UMASK: Mutex<()> = Mutex::new(());
    let _held = UMASK.lock().unwrap_or_else(|e| e.into_inner());

    let previous = unsafe { libc::umask(0o077) };
    let bound = UnixListener::bind(path);
    unsafe { libc::umask(previous) };
    bound
}

pub fn serve_listener(listener: UnixListener, host: SharedHost) {
    serve_listener_with(listener, host, Services::none())
}

pub fn serve_listener_with(listener: UnixListener, host: SharedHost, services: Services) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let host = Arc::clone(&host);
                let services = services.clone();
                let spawned = std::thread::Builder::new()
                    .name("lyrel-control-conn".into())
                    .spawn(move || {
                        if let Err(e) = serve_with(stream, host, services) {
                            log::warn!("control connection failed: {e}");
                        }
                    });
                if let Err(e) = spawned {
                    log::warn!("could not start a control connection thread: {e}");
                }
            }
            Err(e) => log::warn!("control accept failed: {e}"),
        }
    }
}

pub fn spawn_control_listener(host: SharedHost) -> io::Result<PathBuf> {
    spawn_control_listener_with(host, Services::none())
}

pub fn spawn_control_listener_with(
    host: SharedHost,
    services: Services,
) -> io::Result<PathBuf> {
    // Anchor uptime here, not at the first Status request. The GUI can host
    // the control listener too, and there `run_with` never runs — so
    // without this the OnceLock is first set by whoever asks, and every
    // answer reports a server that just started.
    super::server_started();
    let path = control_socket_path()?;
    let listener = bind_control_socket(&path)?;
    std::thread::Builder::new()
        .name("lyrel-control-listener".into())
        .spawn(move || serve_listener_with(listener, host, services))?;
    Ok(path)
}

pub(super) fn control_endpoint_display() -> String {
    control_socket_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
