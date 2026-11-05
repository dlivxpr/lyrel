//! Per-pane history, through a real shell.
//!
//! The daemon's half of this feature is a filename and some cleanup, and unit
//! tests cover it. The half that can actually be wrong lives in the integration
//! snippet: it has to run late enough to see the `HISTFILE` the user's rc set,
//! seed from it, and repoint before the shell loads any history. Nothing but a
//! real bash reading a real rc file can say whether that happened.

#![cfg(unix)]

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use lyrel_core::client::PaneClient;
use lyrel_core::daemon::protocol::{DaemonMsg, ShellSpec, WinSize};

const READY_WITHIN: Duration = Duration::from_secs(30);
const STREAM_WITHIN: Duration = Duration::from_secs(30);
const SETTLE_WITHIN: Duration = Duration::from_secs(10);

struct Daemon {
    child: Child,
    dir: tempfile::TempDir,
}

impl Daemon {
    /// A daemon whose config asks for per-pane history, and whose panes get a
    /// `HOME` of their own with an rc file and a history file already in it.
    fn start() -> Daemon {
        // Where this user keeps their history, said the way a user says it.
        Self::with_home("HISTFILE=\"$HOME/.bash_history\"\n", "echo from_before\n")
    }

    fn with_home(bashrc: &str, history: &str) -> Daemon {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            r#"{"per_pane_history": true}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join(".bashrc"), bashrc).unwrap();
        std::fs::write(dir.path().join(".bash_history"), history).unwrap();

        Self::from_dir(dir)
    }

    fn from_dir(dir: tempfile::TempDir) -> Daemon {
        let child = Command::new(env!("CARGO_BIN_EXE_lyrel-server"))
            .arg("--daemon")
            .arg("--config-dir")
            .arg(dir.path())
            .env("HOME", dir.path())
            .env("LYREL_DATA_DIR", dir.path())
            .env("LYREL_CONTROL_SOCK", dir.path().join("control.sock"))
            .env_remove("ZDOTDIR")
            .env_remove("LYREL_USER_ZDOTDIR")
            .env_remove("LYREL_SHELL_INTEGRATION")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start lyrel-server --daemon");
        let daemon = Daemon { child, dir };
        daemon.await_ready();
        daemon
    }

    fn panes(&self) -> PaneClient {
        PaneClient::at(self.dir.path().join("daemon.sock"))
    }

    fn global_history(&self) -> String {
        std::fs::read_to_string(self.dir.path().join(".bash_history")).unwrap_or_default()
    }

    fn pane_history(&self, pane_id: u64) -> Option<String> {
        std::fs::read_to_string(
            self.dir
                .path()
                .join("history")
                .join(format!("pane-{pane_id}")),
        )
        .ok()
    }

    fn await_ready(&self) {
        let deadline = Instant::now() + READY_WITHIN;
        loop {
            if self.panes().version().is_ok() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the daemon did not open its pane endpoint within {READY_WITHIN:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn size() -> WinSize {
    WinSize {
        cols: 100,
        rows: 30,
        cell_w: 8,
        cell_h: 16,
    }
}

fn bash() -> ShellSpec {
    ShellSpec {
        program: "/bin/bash".into(),
        args: Vec::new(),
        args_are_lyrel_defaults: false,
    }
}

fn windows_contain(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn collect_until(session: &mut lyrel_core::client::PaneSession, marker: &[u8]) -> Vec<u8> {
    let mut seen: Vec<u8> = Vec::new();
    loop {
        match session.recv() {
            Ok(DaemonMsg::Output(bytes)) | Ok(DaemonMsg::Snapshot(bytes)) => {
                seen.extend_from_slice(&bytes);
                if windows_contain(&seen, marker) {
                    return seen;
                }
            }
            Ok(DaemonMsg::Exited { code }) => panic!(
                "the pane exited ({code:?}) before {:?} appeared; saw {:?}",
                String::from_utf8_lossy(marker),
                String::from_utf8_lossy(&seen)
            ),
            Ok(_) => {}
            Err(e) => panic!(
                "the pane stream ended early: {e}; saw {:?}",
                String::from_utf8_lossy(&seen)
            ),
        }
    }
}

fn wait_until(mut done: impl FnMut() -> bool, what: &str) {
    let deadline = Instant::now() + SETTLE_WITHIN;
    while !done() {
        assert!(Instant::now() < deadline, "{what} within {SETTLE_WITHIN:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_pane_writes_its_own_history_and_hands_it_back_when_it_closes() {
    let daemon = Daemon::start();
    let panes = daemon.panes();
    let mut session = panes
        .spawn(None, size(), Some(bash()), None, None)
        .expect("spawn a bash pane");
    let pane_id = session.pane_id();
    session
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");

    session
        .input(b"echo lyrel_ran_here\r")
        .expect("the shell takes input");
    collect_until(&mut session, b"lyrel_ran_here");

    // Seeded from the user's file, so the pane is not starting blank. Waited
    // for by its contents rather than its name: the snippet seeds with a
    // redirection, which creates the file before `tail` has written a byte
    // into it, and a pane caught in that window reads as one that lost the
    // user's history.
    wait_until(
        || {
            daemon
                .pane_history(pane_id)
                .is_some_and(|h| h.contains("echo from_before"))
        },
        "the pane never got a history file of its own, seeded from the user's",
    );
    let seeded = daemon.pane_history(pane_id).unwrap();
    assert!(
        seeded.contains("echo from_before"),
        "a pane whose history starts empty has lost the user their history; got {seeded:?}"
    );

    // Nothing has leaked into the shared file while the pane is open — that
    // separation is the entire point of the feature.
    assert!(
        !daemon.global_history().contains("lyrel_ran_here"),
        "the command should be in this pane's file, not in everybody's"
    );

    // A clean exit is when bash writes its history out, and the pane closing is
    // what makes the daemon hand it back.
    session.input(b"exit\r").expect("ask the shell to exit");
    loop {
        match session.recv() {
            Ok(DaemonMsg::Exited { .. }) => break,
            Ok(_) => {}
            Err(e) => panic!("the pane stream ended before Exited: {e}"),
        }
    }
    drop(session);

    wait_until(
        || daemon.global_history().contains("lyrel_ran_here"),
        "the closed pane's commands never reached the history they came from",
    );
    let global = daemon.global_history();
    assert_eq!(
        global.matches("echo from_before").count(),
        1,
        "the seed is already there; handing it back would duplicate it. Got {global:?}"
    );
    assert!(
        daemon.pane_history(pane_id).is_none(),
        "a closed pane leaves no file behind"
    );
}

/// The snippet seeds with `tail … > "$LYREL_HISTFILE"`, and a redirection
/// creates the file before the command that fills it writes a byte. So the
/// pane's file existing does not mean the pane's history exists, and anything
/// that reads it on the strength of the name being there can read an empty one
/// — the pane looking, for that moment, exactly like a pane that lost the
/// user's history.
///
/// The window is a `tail` wide, which is why this drives it with a `HISTFILE`
/// that takes a known second to produce its bytes rather than hoping to land
/// inside it.
#[test]
fn a_pane_whose_seed_is_still_copying_is_not_an_empty_history() {
    let daemon = Daemon::with_home(
        "mkfifo \"$HOME/slow_history\" 2>/dev/null\n\
         HISTFILE=\"$HOME/slow_history\"\n\
         ( sleep 1; printf 'echo from_before\\n' > \"$HOME/slow_history\" ) &\n",
        "echo from_before\n",
    );
    let panes = daemon.panes();
    let session = panes
        .spawn(None, size(), Some(bash()), None, None)
        .expect("spawn a bash pane");
    let pane_id = session.pane_id();

    wait_until(
        || {
            daemon
                .pane_history(pane_id)
                .is_some_and(|h| h.contains("echo from_before"))
        },
        "the pane never got a history file of its own, seeded from the user's",
    );
    let seeded = daemon.pane_history(pane_id).unwrap();
    assert!(
        seeded.contains("echo from_before"),
        "a pane whose history starts empty has lost the user their history; got {seeded:?}"
    );
}

/// A user with more history than their caps allow — which is most users, since
/// bash defaults both to 500. The exit rewrite keeps only the last
/// HISTSIZE/HISTFILESIZE lines, so without the integration snippet raising the
/// caps for the pane's private file, the file ends up shorter than its own seed
/// mark, the merge reads that as "replaced under us", and the pane's commands
/// silently never reach the history they were promised to.
#[test]
fn a_small_histfilesize_cannot_swallow_the_merge_back() {
    let mut seed = String::new();
    for i in 0..400 {
        seed.push_str(&format!("echo seeded_{i}\n"));
    }
    let daemon = Daemon::with_home(
        "HISTFILE=\"$HOME/.bash_history\"\nHISTSIZE=50\nHISTFILESIZE=50\n",
        &seed,
    );
    let panes = daemon.panes();
    let mut session = panes
        .spawn(None, size(), Some(bash()), None, None)
        .expect("spawn a bash pane");
    session
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");

    session
        .input(b"echo lyrel_survives_truncation\r")
        .expect("the shell takes input");
    collect_until(&mut session, b"lyrel_survives_truncation");

    session.input(b"exit\r").expect("ask the shell to exit");
    loop {
        match session.recv() {
            Ok(DaemonMsg::Exited { .. }) => break,
            Ok(_) => {}
            Err(e) => panic!("the pane stream ended before Exited: {e}"),
        }
    }
    drop(session);

    wait_until(
        || {
            daemon
                .global_history()
                .contains("lyrel_survives_truncation")
        },
        "the exit rewrite truncated the pane's file below its seed and the merge dropped it",
    );
}

#[test]
fn two_panes_do_not_see_each_others_commands() {
    let daemon = Daemon::start();
    let panes = daemon.panes();

    let mut first = panes
        .spawn(None, size(), Some(bash()), None, None)
        .expect("spawn the first pane");
    let first_id = first.pane_id();
    first
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");
    let mut second = panes
        .spawn(None, size(), Some(bash()), None, None)
        .expect("spawn the second pane");
    let second_id = second.pane_id();
    second
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");

    first
        .input(b"echo lyrel_first_pane\r")
        .expect("the first shell takes input");
    collect_until(&mut first, b"lyrel_first_pane");
    second
        .input(b"echo lyrel_second_pane\r")
        .expect("the second shell takes input");
    collect_until(&mut second, b"lyrel_second_pane");

    // `history -a` is what bash does at exit; asking for it now keeps the test
    // from having to end either shell to look at what it has written.
    first.input(b"history -a\r").expect("flush the first");
    collect_until(&mut first, b"history -a");
    second.input(b"history -a\r").expect("flush the second");
    collect_until(&mut second, b"history -a");

    wait_until(
        || {
            daemon
                .pane_history(first_id)
                .is_some_and(|h| h.contains("lyrel_first_pane"))
        },
        "the first pane never recorded its own command",
    );
    wait_until(
        || {
            daemon
                .pane_history(second_id)
                .is_some_and(|h| h.contains("lyrel_second_pane"))
        },
        "the second pane never recorded its own command",
    );

    assert!(
        !daemon
            .pane_history(first_id)
            .unwrap()
            .contains("lyrel_second_pane"),
        "the first pane is reading the second one's commands, which is the thing this replaces"
    );
    assert!(
        !daemon
            .pane_history(second_id)
            .unwrap()
            .contains("lyrel_first_pane"),
        "the second pane is reading the first one's commands"
    );

    panes.kill(first_id).expect("close the first pane");
    panes.kill(second_id).expect("close the second pane");
}

fn zsh() -> Option<ShellSpec> {
    let program = std::env::var("LYREL_TEST_ZSH").unwrap_or_else(|_| "zsh".into());
    if !Command::new(&program).arg("--version").output().is_ok_and(|out| out.status.success()) {
        eprintln!("skipping: zsh unavailable; set LYREL_TEST_ZSH to an isolated executable");
        return None;
    }
    Some(ShellSpec {
        program,
        args: vec!["-l".into()],
        args_are_lyrel_defaults: true,
    })
}

fn zsh_history_journey(configured_zdotdir: bool) {
    let Some(shell) = zsh() else { return };
    let dir = tempfile::TempDir::new().unwrap();
    let user = dir.path().join("user zsh");
    std::fs::create_dir(&user).unwrap();
    let runtime = [
        ("LYREL_TEST_ZSH_FPATH", "fpath"),
        ("LYREL_TEST_ZSH_MODULE_PATH", "module_path"),
    ].into_iter().filter_map(|(env, var)| {
        std::env::var(env).ok().map(|path| format!("{var}=('{path}' ${var})\n"))
    }).collect::<String>();
    std::fs::write(user.join(".zshenv"), &runtime).unwrap();
    std::fs::write(
        dir.path().join(".zshenv"),
        format!("{runtime}export ZDOTDIR=\"$HOME/user zsh\"\n"),
    ).unwrap();
    std::fs::write(user.join(".zprofile"), "USER_STARTUP=profile\n").unwrap();
    std::fs::write(user.join(".zshrc"), concat!(
        "USER_STARTUP=$USER_STARTUP,rc\n",
        "HISTFILE=\"$HOME/custom history\"\n",
        "SAVEHIST=100\nHISTSIZE=100\nsetopt inc_append_history\nPS1='fixture> '\n",
    )).unwrap();
    std::fs::write(user.join(".zlogin"), "USER_STARTUP=$USER_STARTUP,login\n").unwrap();
    let origin = dir.path().join("custom history");
    std::fs::write(&origin, "echo custom_seed_command\n").unwrap();
    let mut config = serde_json::json!({"per_pane_history": true});
    if configured_zdotdir {
        config["env"] = serde_json::json!({"ZDOTDIR": user});
    }
    std::fs::write(dir.path().join("config.json"), config.to_string()).unwrap();
    let daemon = Daemon::from_dir(dir);
    let mut session = daemon.panes().spawn(None, size(), Some(shell), None, None).unwrap();
    let pane = session.pane_id();
    session.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    session.input(b"builtin printf '\\033]777;startup;%s;%s;%s\\a' \"$USER_STARTUP\" \"$LYREL_SHELL_INTEGRATION\" \"$ZDOTDIR\"\r").unwrap();
    let out = collect_until(&mut session, format!("{}\x07", user.display()).as_bytes());
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("\x1b]777;startup;profile,rc,login;1;"), "user startup and integration must both run: {text:?}");
    assert!(text.contains(user.to_str().unwrap()), "the live ZDOTDIR returns to the user's directory: {text:?}");
    wait_until(|| daemon.pane_history(pane).is_some_and(|h| h.contains("echo custom_seed_command")), "the custom history was not seeded");
    assert_eq!(std::fs::read_to_string(&origin).unwrap(), "echo custom_seed_command\n", "opening a pane must not rewrite its origin");
    session.input(b"echo custom_live_command\r").unwrap();
    collect_until(&mut session, b"\x1b]133;D;0;aid=lyrel");
    session.input(b"exit\r").unwrap();
    loop {
        match session.recv() {
            Ok(DaemonMsg::Exited { .. }) => break,
            Ok(_) => {},
            Err(e) => panic!("the zsh pane ended before Exited: {e}"),
        }
    }
    drop(session);
    wait_until(|| std::fs::read_to_string(&origin).unwrap().contains("echo custom_live_command"), "the custom origin did not receive the pane's commands");
    assert_eq!(std::fs::read_to_string(&origin).unwrap().matches("echo custom_seed_command").count(), 1);
    assert!(daemon.pane_history(pane).is_none(), "retiring the pane removes its private history");
}

#[test]
fn zsh_configured_zdotdir_preserves_integration_and_custom_history() {
    zsh_history_journey(true);
}

#[test]
fn zshenv_relocated_zdotdir_preserves_integration_and_custom_history() {
    zsh_history_journey(false);
}
