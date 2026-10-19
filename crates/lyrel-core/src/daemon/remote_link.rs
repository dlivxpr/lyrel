use std::io;
use std::pin::Pin;
use std::process::Stdio;
use std::task::{Context, Poll};

use russh::Channel;
use russh::client::Msg;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::router::RouteChannel;
use super::ssh::ProcessStream;

pub enum RemoteLink {
    StreamLocal(russh::ChannelStream<russh::client::Msg>),

    SessionExec(russh::ChannelStream<russh::client::Msg>),

    Wsl(ProcessStream),

    LocalStdio(ProcessStream),
}

impl RemoteLink {
    pub fn stream_local(channel: Channel<Msg>) -> RemoteLink {
        RemoteLink::StreamLocal(channel.into_stream())
    }

    pub fn session_exec(channel: Channel<Msg>) -> RemoteLink {
        RemoteLink::SessionExec(channel.into_stream())
    }

    pub fn local_stdio(program: &str, args: &[&str]) -> io::Result<RemoteLink> {
        Ok(RemoteLink::LocalStdio(spawn_stdio(program, args)?))
    }

    pub fn wsl(distro: &str, server: &str, channel: RouteChannel) -> io::Result<RemoteLink> {
        super::install::wsl::validate_distro(distro)?;
        let args = super::install::wsl::wsl_args(distro, &wsl_link_argv(server, channel));
        Ok(RemoteLink::Wsl(spawn_stdio_owned(
            super::install::wsl::WSL_EXE,
            &args,
        )?))
    }

    pub fn wsl_shell(distro: &str, command: &str, channel: RouteChannel) -> io::Result<RemoteLink> {
        super::install::wsl::validate_distro(distro)?;
        let command = channel.bridge_command(command);
        let args = super::install::wsl::wsl_args(distro, &["sh", "-c", &command]);
        Ok(RemoteLink::Wsl(spawn_stdio_owned(
            super::install::wsl::WSL_EXE,
            &args,
        )?))
    }

    pub fn kind_label(&self) -> &'static str {
        match self {
            RemoteLink::StreamLocal(_) => "streamlocal",
            RemoteLink::SessionExec(_) => "session-exec",
            RemoteLink::Wsl(_) => "wsl-stdio",
            RemoteLink::LocalStdio(_) => "local-stdio",
        }
    }

    pub fn is_stdio_bridge(&self) -> bool {
        matches!(
            self,
            RemoteLink::SessionExec(_) | RemoteLink::Wsl(_) | RemoteLink::LocalStdio(_)
        )
    }

    pub fn is_ssh(&self) -> bool {
        matches!(
            self,
            RemoteLink::StreamLocal(_) | RemoteLink::SessionExec(_)
        )
    }
}

fn wsl_link_argv<'a>(server: &'a str, channel: RouteChannel) -> Vec<&'a str> {
    let mut argv = vec![server, "--stdio"];
    if channel == RouteChannel::Pane {
        argv.push("--pane");
    }
    argv
}

fn spawn_stdio(program: &str, args: &[&str]) -> io::Result<ProcessStream> {
    let owned: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
    spawn_stdio_owned(program, &owned)
}

fn spawn_stdio_owned(program: &str, args: &[String]) -> io::Result<ProcessStream> {
    let mut command = tokio::process::Command::new(program);
    crate::core::proc::hide_console_tokio(&mut command);
    let mut child = command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other(format!("{program} stdin unavailable")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other(format!("{program} stdout unavailable")))?;
    Ok(ProcessStream::from_parts(child, stdin, stdout))
}

pub const DEFAULT_REMOTE_SERVER_CMD: &str = "lyrel-server --stdio";

const MAX_SOCKET_PATH_BYTES: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteEntry {
    StreamLocal { socket: String },
    SessionExec { command: String },
}

impl RemoteEntry {
    pub fn kind_label(&self) -> &'static str {
        match self {
            RemoteEntry::StreamLocal { .. } => "streamlocal",
            RemoteEntry::SessionExec { .. } => "session-exec",
        }
    }
}

pub fn choose_entry(
    socket: Option<&str>,
    forwarding_allowed: bool,
    server_command: &str,
) -> RemoteEntry {
    match socket {
        Some(socket) if forwarding_allowed && !socket.is_empty() => RemoteEntry::StreamLocal {
            socket: socket.to_string(),
        },
        _ => RemoteEntry::SessionExec {
            command: server_command.to_string(),
        },
    }
}

/// The entry a server's location settles without asking the remote anything,
/// or `None` when [`REMOTE_ENV_PROBE`] and [`choose_entry`] have to decide.
///
/// A Windows server is always reached by session exec. The probe is a POSIX
/// `sh` script that no Windows shell runs, and the stream-local forward it
/// would find a socket for does not exist there: a Windows daemon listens on a
/// loopback TCP port guarded by a token, not on a unix socket. Asking anyway
/// would only spend a round trip learning that.
pub fn fixed_entry(server_binary: &str, command: &str) -> Option<RemoteEntry> {
    super::install::asset::is_windows_sftp_path(server_binary).then(|| RemoteEntry::SessionExec {
        command: command.to_string(),
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteEnv {
    pub control_sock: Option<String>,
    pub config_dir: Option<String>,
    pub xdg_runtime_dir: Option<String>,
    pub home: Option<String>,
    pub tmpdir: Option<String>,
}

const ENV_MARKER: &str = "__lyrel_env__";

pub const REMOTE_ENV_PROBE: &str = concat!(
    "sh -c 'printf \"__lyrel_env__ %s\\n\" ",
    "\"sock=${LYREL_CONTROL_SOCK-}\" \"cfg=${LYREL_CONFIG_DIR-}\" ",
    "\"xdg=${XDG_RUNTIME_DIR-}\" ",
    "\"home=${HOME-}\" \"tmp=${TMPDIR-}\"'"
);

impl RemoteEnv {
    pub fn parse_probe(out: &str) -> RemoteEnv {
        let mut env = RemoteEnv::default();
        for line in out.lines() {
            let Some(rest) = line.trim().strip_prefix(ENV_MARKER) else {
                continue;
            };
            let Some((key, value)) = rest.trim_start().split_once('=') else {
                continue;
            };
            let value = (!value.is_empty()).then(|| value.to_string());
            match key {
                "sock" => env.control_sock = value,
                "cfg" => env.config_dir = value,
                "xdg" => env.xdg_runtime_dir = value,
                "home" => env.home = value,
                "tmp" => env.tmpdir = value,
                _ => {}
            }
        }
        env
    }
}

pub fn remote_control_socket(env: &RemoteEnv) -> Option<String> {
    if let Some(explicit) = env.control_sock.as_deref().filter(|s| !s.is_empty()) {
        return Some(explicit.to_string());
    }

    // The remote server is launched with `--stdio` and no `--config-dir`, so it
    // opens its control socket in the config dir — `$LYREL_CONFIG_DIR` if the
    // remote sets one, otherwise `$HOME/.config/lyrel`. This has to mirror
    // `host::server::control_socket_path` exactly: it is the same rule applied
    // to an environment we probed instead of our own.
    let dir = match env.config_dir.as_deref().filter(|d| !d.is_empty()) {
        Some(cfg) => cfg.to_string(),
        None => {
            let home = env.home.as_deref().filter(|h| !h.is_empty())?;
            posix_join(&posix_join(home, ".config"), "lyrel")
        }
    };
    let runtime = env.xdg_runtime_dir.as_deref().filter(|d| !d.is_empty());

    let inline = posix_join(&dir, "control.sock");
    if fits(&inline) {
        return Some(inline);
    }

    let name = format!(
        "lyrel-{:016x}-control.sock",
        crate::host::fnv1a64(dir.as_bytes())
    );
    let tmp = env
        .tmpdir
        .as_deref()
        .filter(|d| !d.is_empty())
        .unwrap_or("/tmp");
    [runtime, Some(tmp)]
        .into_iter()
        .flatten()
        .map(|base| posix_join(base, &name))
        .find(|candidate| fits(candidate))
}

fn posix_join(base: &str, name: &str) -> String {
    format!("{}/{name}", base.trim_end_matches('/'))
}

fn fits(path: &str) -> bool {
    path.len() <= MAX_SOCKET_PATH_BYTES
}

impl AsyncRead for RemoteLink {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            RemoteLink::StreamLocal(s) | RemoteLink::SessionExec(s) => {
                Pin::new(s).poll_read(cx, buf)
            }
            RemoteLink::Wsl(s) | RemoteLink::LocalStdio(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for RemoteLink {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            RemoteLink::StreamLocal(s) | RemoteLink::SessionExec(s) => {
                Pin::new(s).poll_write(cx, buf)
            }
            RemoteLink::Wsl(s) | RemoteLink::LocalStdio(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            RemoteLink::StreamLocal(s) | RemoteLink::SessionExec(s) => Pin::new(s).poll_flush(cx),
            RemoteLink::Wsl(s) | RemoteLink::LocalStdio(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            RemoteLink::StreamLocal(s) | RemoteLink::SessionExec(s) => {
                Pin::new(s).poll_shutdown(cx)
            }
            RemoteLink::Wsl(s) | RemoteLink::LocalStdio(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

impl std::fmt::Debug for RemoteLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("RemoteLink")
            .field(&self.kind_label())
            .finish()
    }
}

#[cfg(test)]
pub(super) fn test_stdio_echo_link() -> RemoteLink {
    #[cfg(windows)]
    {
        test_powershell_link(
            "[Console]::OpenStandardInput().CopyTo([Console]::OpenStandardOutput())",
        )
    }
    #[cfg(not(windows))]
    {
        RemoteLink::local_stdio("/bin/cat", &[]).expect("spawn the system byte-echo process")
    }
}

#[cfg(all(test, windows))]
fn test_powershell_link(command: &str) -> RemoteLink {
    let system_root = std::path::PathBuf::from(
        std::env::var_os("SystemRoot").expect("Windows provides SystemRoot"),
    );
    let powershell = system_root.join("System32/WindowsPowerShell/v1.0/powershell.exe");
    RemoteLink::local_stdio(
        powershell
            .to_str()
            .expect("system PowerShell path is Unicode"),
        &[
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            command,
        ],
    )
    .expect("spawn the system PowerShell process")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

    #[test]
    fn only_a_pane_link_asks_for_the_pane_socket() {
        let server = "/home/me/.local/share/lyrel/bin/lyrel-server-26.7.6";
        assert_eq!(
            wsl_link_argv(server, RouteChannel::Control),
            vec![server, "--stdio"]
        );
        assert_eq!(
            wsl_link_argv(server, RouteChannel::Pane),
            vec![server, "--stdio", "--pane"]
        );
    }

    #[tokio::test]
    async fn a_local_stdio_child_round_trips_bytes() {
        let mut link = test_stdio_echo_link();
        assert_eq!(link.kind_label(), "local-stdio");
        assert!(link.is_stdio_bridge());
        assert!(!link.is_ssh());

        let payload = b"hello remote\n\0\xff\r\n";
        link.write_all(payload).await.unwrap();
        link.flush().await.unwrap();

        let mut got = vec![0u8; payload.len()];
        link.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, payload);
    }

    #[tokio::test]
    async fn shutdown_closes_the_write_half_and_the_peer_sees_eof() {
        let mut link = test_stdio_echo_link();
        link.write_all(b"bye").await.unwrap();
        link.shutdown().await.unwrap();

        let mut rest = Vec::new();
        link.read_to_end(&mut rest).await.unwrap();
        assert_eq!(rest, b"bye");
    }

    #[tokio::test]
    async fn dropping_the_link_kills_the_child() {
        use std::time::Duration;

        #[cfg(windows)]
        let mut link =
            test_powershell_link("[Console]::WriteLine($PID); [Threading.Thread]::Sleep(300000)");
        #[cfg(not(windows))]
        let mut link = RemoteLink::local_stdio(
            "/bin/sh",
            &["-c", "printf '%s\\n' \"$$\"; exec /bin/sleep 300"],
        )
        .unwrap();

        let mut pid_line = String::new();
        tokio::time::timeout(Duration::from_secs(10), async {
            tokio::io::BufReader::new(&mut link)
                .read_line(&mut pid_line)
                .await
        })
        .await
        .expect("child reports readiness")
        .unwrap();
        let pid: u32 = pid_line.trim().parse().expect("child reports its PID");

        #[cfg(windows)]
        assert!(!crate::daemon::winproc::wait_for_exit(pid, Duration::ZERO));
        #[cfg(unix)]
        assert_eq!(unsafe { libc::kill(pid as libc::pid_t, 0) }, 0);

        drop(link);

        #[cfg(windows)]
        assert!(crate::daemon::winproc::wait_for_exit(
            pid,
            Duration::from_secs(5)
        ));
        #[cfg(unix)]
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if unsafe { libc::kill(pid as libc::pid_t, 0) } == -1 {
                    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("dropped link terminates and reaps its child");
    }

    #[test]
    fn every_variant_has_a_distinct_label() {
        let labels = ["streamlocal", "session-exec", "wsl-stdio", "local-stdio"];
        let mut sorted = labels.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), labels.len(), "labels must be distinguishable");
    }

    #[test]
    fn the_entry_falls_back_exactly_when_streamlocal_cannot_be_used() {
        let cmd = "lyrel-server --stdio";
        assert_eq!(
            choose_entry(Some("/run/user/1000/lyrel/daemon.sock"), true, cmd),
            RemoteEntry::StreamLocal {
                socket: "/run/user/1000/lyrel/daemon.sock".into()
            }
        );
        assert_eq!(
            choose_entry(Some("/run/user/1000/lyrel/daemon.sock"), false, cmd),
            RemoteEntry::SessionExec {
                command: cmd.into()
            }
        );
        assert_eq!(
            choose_entry(None, true, cmd),
            RemoteEntry::SessionExec {
                command: cmd.into()
            }
        );
        assert_eq!(
            choose_entry(None, false, cmd),
            RemoteEntry::SessionExec {
                command: cmd.into()
            }
        );
    }

    #[test]
    fn a_windows_server_is_reached_by_session_exec_without_a_probe() {
        let command = r#""C:\Users\me\AppData\Local\lyrel\bin\lyrel-server-c3p4.exe" --stdio"#;
        assert_eq!(
            fixed_entry(
                "/C:/Users/me/AppData/Local/lyrel/bin/lyrel-server-c3p4.exe",
                command
            ),
            Some(RemoteEntry::SessionExec {
                command: command.into()
            })
        );
        assert_eq!(
            fixed_entry(
                "/home/me/.local/share/lyrel/bin/lyrel-server-c3p4",
                "'/home/me/.local/share/lyrel/bin/lyrel-server-c3p4' --stdio"
            ),
            None,
            "a unix server still goes through the probe"
        );
    }

    #[test]
    fn the_env_probe_survives_a_chatty_remote() {
        let out = "Welcome to Ubuntu!\n\
                   __lyrel_env__ sock=\n\
                   __lyrel_env__ xdg=/run/user/1000\n\
                   __lyrel_env__ home=/home/me\n\
                   __lyrel_env__ tmp=\n\
                   You have mail.\n";
        let env = RemoteEnv::parse_probe(out);
        assert_eq!(env.control_sock, None);
        assert_eq!(env.xdg_runtime_dir.as_deref(), Some("/run/user/1000"));
        assert_eq!(env.home.as_deref(), Some("/home/me"));
        assert_eq!(env.tmpdir, None);
    }

    #[test]
    fn the_remote_socket_path_matches_the_servers_own_order() {
        let explicit = RemoteEnv {
            control_sock: Some("/tmp/mine.sock".into()),
            xdg_runtime_dir: Some("/run/user/1000".into()),
            home: Some("/home/me".into()),
            ..RemoteEnv::default()
        };
        assert_eq!(
            remote_control_socket(&explicit).as_deref(),
            Some("/tmp/mine.sock"),
            "an explicit $LYREL_CONTROL_SOCK outranks everything"
        );

        // $XDG_RUNTIME_DIR no longer places the socket: the server opens it in
        // its config dir, so the path follows that and only falls back to the
        // runtime dir when the config dir is too long for sun_path.
        let explicit_cfg = RemoteEnv {
            config_dir: Some("/home/me/.config/lyrel".into()),
            xdg_runtime_dir: Some("/run/user/1000".into()),
            home: Some("/home/me".into()),
            ..RemoteEnv::default()
        };
        assert_eq!(
            remote_control_socket(&explicit_cfg).as_deref(),
            Some("/home/me/.config/lyrel/control.sock"),
            "an explicit $LYREL_CONFIG_DIR names the directory"
        );

        let trailing = RemoteEnv {
            config_dir: Some("/home/me/.config/lyrel/".into()),
            ..RemoteEnv::default()
        };
        assert_eq!(
            remote_control_socket(&trailing).as_deref(),
            Some("/home/me/.config/lyrel/control.sock")
        );

        let home_only = RemoteEnv {
            home: Some("/home/me".into()),
            ..RemoteEnv::default()
        };
        assert_eq!(
            remote_control_socket(&home_only).as_deref(),
            Some("/home/me/.config/lyrel/control.sock"),
            "with no $LYREL_CONFIG_DIR the remote server uses $HOME/.config/lyrel"
        );

        assert_eq!(remote_control_socket(&RemoteEnv::default()), None);
    }

    #[test]
    fn a_deep_runtime_dir_never_yields_an_unbindable_path() {
        let deep = format!("/run/user/1000/{}", "nested/".repeat(12));
        let env = RemoteEnv {
            control_sock: None,
            config_dir: Some(deep.clone()),
            xdg_runtime_dir: Some(deep.clone()),
            home: Some("/home/me".into()),
            tmpdir: Some("/tmp".into()),
        };
        let path = remote_control_socket(&env).expect("the temp dir is short enough");
        assert!(
            path.len() <= MAX_SOCKET_PATH_BYTES,
            "{path} ({} bytes) would be rejected by bind()",
            path.len()
        );
        assert!(
            path.starts_with("/tmp/lyrel-"),
            "unexpected fallback: {path}"
        );

        let hopeless = RemoteEnv {
            control_sock: None,
            config_dir: Some(deep.clone()),
            xdg_runtime_dir: Some(deep.clone()),
            home: Some("/home/me".into()),
            tmpdir: Some(deep),
        };
        assert_eq!(remote_control_socket(&hopeless), None);
    }
}
