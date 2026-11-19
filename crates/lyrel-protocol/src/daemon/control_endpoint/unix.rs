//! Unix control socket discovery and client-side endpoint helpers.
//!
//! The address rule mirrors the pane endpoint (`transport::socket_path_for`):
//! a socket inside the config dir, falling back to a hashed name in
//! `$XDG_RUNTIME_DIR` or the temp dir when the config dir's path is too long
//! for `sun_path`. The two endpoints are siblings distinguished by name —
//! `control.sock` here, `daemon.sock` there — which is what the Windows arm
//! has always done (`control.port`/`daemon.port`).

use std::io;
use std::path::{Path, PathBuf};

use super::CONTROL_SOCK_ENV;
use crate::daemon::transport::{self, fnv1a64};

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
/// the wrong socket.
pub fn control_socket_path() -> io::Result<PathBuf> {
    if let Some(explicit) = std::env::var_os(CONTROL_SOCK_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(explicit));
    }

    let dir = crate::core::config::config_dir_path()
        .ok_or_else(|| io::Error::other("no config directory to place the control socket in"))?;

    let fallbacks: Vec<PathBuf> = [runtime_dir(), Some(std::env::temp_dir())]
        .into_iter()
        .flatten()
        .collect();
    socket_path_in(&dir, &fallbacks)
}

/// The socket file name inside a directory that is short enough to hold it.
pub(crate) const CONTROL_SOCK_FILE: &str = "control.sock";

/// The control socket path for `dir`, falling back to a hashed name in
/// `fallbacks` when `dir`'s path exceeds the `sun_path` limit.
pub fn socket_path_in(dir: &Path, fallbacks: &[PathBuf]) -> io::Result<PathBuf> {
    let inline = dir.join(CONTROL_SOCK_FILE);
    if fits(&inline) {
        return Ok(inline);
    }

    use std::os::unix::ffi::OsStrExt as _;
    // `-control` keeps this clear of the pane socket's fallback name, which
    // hashes the same directory and would otherwise land on the same file.
    let name = format!(
        "lyrel-{:016x}-control.sock",
        fnv1a64(dir.as_os_str().as_bytes())
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
/// matters to a daemon whose listener would not open. Those look exactly
/// like a live server in the error, and nothing is listening behind either.
/// Only a connect that completes tells them apart.
pub fn control_socket_answers(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// Whether a control server is answering at this config dir's endpoint.
pub fn control_endpoint_answers() -> bool {
    control_socket_path().is_ok_and(|path| control_socket_answers(&path))
}

/// Connect to the control server recorded for this config dir.
pub fn connect_control() -> io::Result<transport::Stream> {
    let path = control_socket_path()?;
    transport::connect_endpoint_at(&path)
}

/// Remove a stale control socket a dead server left behind. A socket that
/// still answers is left alone: it names a live server, whoever owns it.
pub fn remove_control_endpoint() {
    let Ok(path) = control_socket_path() else {
        return;
    };
    if control_socket_answers(&path) {
        return;
    }
    let _ = std::fs::remove_file(path);
}

/// Where the control endpoint lives, for logs and error messages. Empty when
/// no config dir names one.
pub fn control_endpoint_display() -> String {
    control_socket_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}
