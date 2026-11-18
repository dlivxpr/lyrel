//! Coding-agent hook runner: the entry point the `lyrel` binary dispatches for
//! agent lifecycle events, plus the controlling-terminal fallback used when
//! the pane's daemon socket cannot take the report.
//!
//! Hook installation, state, diagnostics and the pure report-payload builders
//! live in `lyrel-protocol` and are re-exported here unchanged. This file
//! keeps the half that talks to the daemon (the pane client, the wire feature
//! gates, the pane environment) and to the controlling terminal.

pub use lyrel_protocol::core::agent_hooks::*;

use std::io;
use std::io::{IsTerminal as _, Read as _};

use crate::core::cli_agent::{AGENT_EVENT_SENTINEL, CLIAgent};

const MAX_STDIN: u64 = 64 * 1024;

pub fn run_agent_hook(agent: &str, event: &str) {
    detach_console();
    if std::env::var_os(LYREL_ENV_MARKER).is_none() || CLIAgent::from_slug(agent).is_none() {
        return;
    }
    let mut input = String::new();
    if !std::io::stdin().is_terminal() {
        let _ = std::io::stdin().take(MAX_STDIN).read_to_string(&mut input);
    }
    let Some(event) = effective_event(agent, event, &input) else {
        return;
    };
    let payload = build_hook_payload(agent, event, &input);
    let requests = payload.get("request").is_some() || payload.get("requests").is_some();
    let results = payload.get("result").is_some() || payload.get("opencode_presence").is_some();
    if requests || results {
        let version = crate::client::PaneClient::local().version();
        for (required, feature) in [
            (requests, crate::daemon::protocol::FEATURE_AGENT_REQUESTS),
            (results, crate::daemon::protocol::FEATURE_AGENT_RESULTS),
        ] {
            if required
                && !version
                    .as_ref()
                    .is_ok_and(|version| version.has_feature(feature))
            {
                eprintln!("lyrel: daemon does not support {feature}; update/reconnect the daemon");
                std::process::exit(2);
            }
        }
    }
    let body = payload.to_string();
    if let Err(error) = report_to_daemon(&body)
        && !write_to_controlling_tty(&hook_sequence(&body))
    {
        eprintln!("lyrel: agent status delivery failed: {error}; controlling terminal unavailable");
        std::process::exit(2);
    }
}

/// Hand the report to the daemon that owns this pane, over its socket.
///
/// The tty is the channel of last resort. The agent draws on it too, and a
/// tty write is not atomic — on macOS the kernel parks a writer mid-write
/// whenever the output queue is full, which is exactly when an agent is busy
/// redrawing — so the agent's output can land inside the sequence, break it,
/// and leave the rest of it on screen as text. The socket has no such
/// neighbour. It only works where the daemon runs, though: a hook the
/// environment points at no daemon, at a daemon that predates this message,
/// or at a pane it does not run in (see `DaemonPane::report_agent_event`)
/// falls back to the tty.
fn report_to_daemon(body: &str) -> io::Result<()> {
    let pane = std::env::var(crate::daemon::pane::LYREL_PANE_ENV)
        .ok()
        .and_then(|p| p.trim().parse::<u64>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "LYREL_PANE is unavailable"))?;
    crate::client::PaneClient::local().report_agent_event(pane, std::process::id(), body)
}

#[cfg(not(unix))]
fn detach_console() {
    use windows_sys::Win32::System::Console::FreeConsole;
    unsafe {
        FreeConsole();
    }
}

#[cfg(unix)]
fn detach_console() {}

/// The report as it goes out on the tty: the body wrapped in the OSC 777
/// notification the daemon's reader looks for.
fn hook_sequence(body: &str) -> Vec<u8> {
    format!("\x1b]777;notify;{AGENT_EVENT_SENTINEL};{body}\x07").into_bytes()
}

#[cfg(unix)]
fn write_to_controlling_tty(bytes: &[u8]) -> bool {
    if write_dev(std::path::Path::new("/dev/tty"), bytes) {
        return true;
    }
    if let Some(dev) = ancestor_tty_device() {
        return write_dev(&dev, bytes);
    }
    false
}

/// Write `bytes` to the terminal device at `path`.
///
/// The device is opened without waiting. A blocking open of a terminal whose
/// other end has gone — the pane closed while the agent was still running its
/// session-end hook — waits for a carrier that never comes, and on macOS it
/// waits holding the lock every lookup under `/dev` needs: the hook hangs, the
/// agent it belongs to cannot finish exiting, and every new terminal, `ps`
/// and `tty` on the machine hangs behind them. Once open, the device goes back
/// to blocking writes, so a full output queue delays the sequence rather than
/// cutting it short; a terminal with no other end fails the write instead.
#[cfg(unix)]
fn write_dev(path: &std::path::Path, bytes: &[u8]) -> bool {
    use std::io::Write as _;
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let Ok(mut tty) = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
    else {
        return false;
    };
    let fd = tty.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return false;
    }
    tty.write_all(bytes).and_then(|_| tty.flush()).is_ok()
}

#[cfg(unix)]
fn ancestor_tty_device() -> Option<std::path::PathBuf> {
    use std::process::Command;
    let mut pid = unsafe { libc::getppid() };
    for _ in 0..8 {
        if pid <= 1 {
            break;
        }
        let out = Command::new("ps")
            .args(["-o", "tty=", "-o", "ppid=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let line = String::from_utf8_lossy(&out.stdout);
        let mut fields = line.split_whitespace();
        let tty = fields.next().unwrap_or("");
        let ppid: i32 = fields.next().and_then(|s| s.parse().ok()).unwrap_or(1);
        if !tty.is_empty() && tty != "??" && tty != "?" {
            return Some(std::path::PathBuf::from(format!("/dev/{tty}")));
        }
        pid = ppid;
    }
    None
}

#[cfg(not(unix))]
fn write_to_controlling_tty(bytes: &[u8]) -> bool {
    let procs = crate::daemon::winproc::snapshot();
    let ancestors = ancestor_pids(&procs);

    let name_of = |pid: u32| {
        procs
            .iter()
            .find(|p| p.pid == pid)
            .map(|p| p.name.to_ascii_lowercase())
    };
    let shell = ancestors.iter().copied().find(|&pid| {
        procs
            .iter()
            .find(|p| p.pid == pid)
            .and_then(|p| name_of(p.parent))
            .is_some_and(|n| is_lyrel_host_exe(&n))
    });

    if let Some(pid) = shell {
        if attach_and_write(pid, bytes) {
            return true;
        }
    }

    let mut any = false;
    for pid in ancestors {
        any |= attach_and_write(pid, bytes);
    }
    any
}

#[cfg(any(not(unix), test))]
fn is_lyrel_host_exe(name: &str) -> bool {
    matches!(name, "lyrel-app.exe" | "lyrel-server.exe" | "lyrel.exe")
        || (name.ends_with(".exe")
            && crate::daemon::install::asset::dialect_from_path(name).is_some())
}

#[cfg(not(unix))]
fn attach_and_write(pid: u32, bytes: &[u8]) -> bool {
    use windows_sys::Win32::System::Console::{AttachConsole, FreeConsole};
    unsafe {
        FreeConsole();
        if AttachConsole(pid) == 0 {
            return false;
        }
    }
    let ok = write_conout(bytes);
    unsafe {
        FreeConsole();
    }
    ok
}

#[cfg(not(unix))]
fn write_conout(bytes: &[u8]) -> bool {
    use std::io::Write as _;
    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")
    {
        Ok(mut out) => out.write_all(bytes).and_then(|_| out.flush()).is_ok(),
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn ancestor_pids(procs: &[crate::daemon::winproc::Proc]) -> Vec<u32> {
    let parent_of = |pid: u32| procs.iter().find(|p| p.pid == pid).map(|p| p.parent);
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut cur = std::process::id();
    seen.insert(cur);
    for _ in 0..16 {
        match parent_of(cur) {
            Some(parent) if parent != 0 && seen.insert(parent) => {
                out.push(parent);
                cur = parent;
            }
            _ => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resolved tty must be an openable device path.
    #[cfg(unix)]
    #[test]
    fn ancestor_tty_device_is_none_or_a_dev_path() {
        match ancestor_tty_device() {
            None => {}
            Some(dev) => assert!(
                dev.starts_with("/dev/"),
                "a resolved tty must be an openable device path, got {dev:?}"
            ),
        }
    }

    /// A hook whose pane closed under it gives up instead of waiting on the
    /// terminal's other end for good.
    #[cfg(unix)]
    #[test]
    fn a_terminal_with_its_other_end_gone_fails_the_write_at_once() {
        // Both ends are opened close-on-exec from the start: the other tests
        // spawn processes on threads of their own, and a child that inherited
        // the pane's end would keep it open after it is closed here, so the
        // write would go through.
        let flags = libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC;
        let master = unsafe { libc::posix_openpt(flags) };
        assert!(master >= 0);
        assert_eq!(unsafe { libc::grantpt(master) }, 0);
        assert_eq!(unsafe { libc::unlockpt(master) }, 0);
        let name = unsafe { std::ffi::CStr::from_ptr(libc::ptsname(master)) }
            .to_string_lossy()
            .into_owned();
        let path = std::ffi::CString::new(name.clone()).unwrap();
        let slave = unsafe { libc::open(path.as_ptr(), flags) };
        assert!(slave >= 0);
        // The agent still holds its end; the pane's is gone.
        unsafe { libc::close(master) };

        // A child another test forked a moment ago still holds a copy of the
        // pane's end until it gets to exec, and a write in that moment goes
        // through; one made once it has let go fails. None may wait.
        let mut wrote = Ok(true);
        for _ in 0..50 {
            let (tx, rx) = std::sync::mpsc::channel();
            let name = name.clone();
            std::thread::spawn(move || {
                let _ = tx.send(write_dev(std::path::Path::new(&name), b"x"));
            });
            wrote = rx.recv_timeout(std::time::Duration::from_secs(5));
            if wrote != Ok(true) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        unsafe { libc::close(slave) };
        assert_eq!(wrote, Ok(false));
    }

    #[test]
    fn every_lyrel_daemon_host_takes_the_console_fast_path() {
        for name in [
            "lyrel-app.exe",
            "lyrel-server.exe",
            "lyrel.exe",
            "lyrel-server-c12p7.exe",
        ] {
            assert!(is_lyrel_host_exe(name), "{name} hosts lyrel shells");
        }
        for name in [
            "explorer.exe",
            "cmd.exe",
            "lyrel",
            "lyrel-app",
            "wt.exe",
            "otherterm-server-c12p7.exe",
            "not-lyrel-server-c12p7.exe",
            "lyrel-server-c12p7.exe.old",
        ] {
            assert!(
                !is_lyrel_host_exe(name),
                "{name} is not a lyrel host process"
            );
        }
    }

    #[test]
    fn a_remote_hook_names_the_server_its_own_installer_placed() {
        let host = crate::host::local::LocalHost::new();
        let dialect = crate::daemon::install::RemoteProtocol::of_this_build();
        let name = crate::daemon::install::asset::binary_name(dialect.control, dialect.protocol);

        let unix = HookTarget::remote(&*host, std::path::PathBuf::from("/home/me"));
        assert_eq!(
            unix.exe,
            std::path::PathBuf::from(format!("/home/me/.local/share/lyrel/bin/{name}"))
        );

        let windows = HookTarget::remote(&*host, std::path::PathBuf::from(r"C:\Users\me"));
        assert_eq!(
            windows.exe,
            std::path::PathBuf::from(format!(
                r"C:\Users\me\AppData\Local\lyrel\bin\{name}.exe"
            ))
        );
    }
}
