//! Daemon-only Windows process policy atop the shared process helpers.

pub(crate) use lyrel_protocol::daemon::winproc::*;

/// ConPTY has no Unix foreground pgid. Inspect the complete owned shell tree
/// at each indirect command instead; cached titles/remote UI facts are not proof.
pub(crate) fn media_tree_is_local(root: u32) -> Option<bool> {
    let first = checked_snapshot()?;
    let local = classify_media_tree(&first, root, image_path)?;
    if !local {
        return Some(false);
    }
    // Refuse a fork/exit/PID reuse during probing rather than granting from a
    // mixture of generations. Ordinary direct graphics never pay this cost.
    let second = checked_snapshot()?;
    let members = |table: &[Proc]| {
        let mut ids = descendants(table, root);
        ids.push(root);
        let mut rows: Vec<_> = table
            .iter()
            .filter(|p| ids.contains(&p.pid))
            .map(|p| (p.pid, p.parent, p.name.clone()))
            .collect();
        rows.sort();
        rows
    };
    (members(&first) == members(&second)).then_some(true)
}

fn classify_media_tree(
    procs: &[Proc],
    root: u32,
    mut image: impl FnMut(u32) -> Option<std::path::PathBuf>,
) -> Option<bool> {
    if root == 0 || !procs.iter().any(|p| p.pid == root) {
        return None;
    }
    let mut ids = descendants(procs, root);
    if ids.len() > 512 {
        return None;
    }
    ids.push(root);
    for pid in ids {
        let path = image(pid)?;
        let name = path.file_name()?.to_str()?;
        if super::procinfo::media_namespace_bridge(name, true) {
            return Some(false);
        }
    }
    Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(pid: u32, parent: u32, name: &str) -> Proc {
        Proc {
            pid,
            parent,
            name: name.into(),
        }
    }

    #[test]
    fn media_locality_is_fresh_and_rejects_nested_namespace_bridges() {
        let mut tree = vec![p(100, 1, "pwsh.exe"), p(200, 100, "python.exe")];
        assert_eq!(
            classify_media_tree(&tree, 100, |_| Some(PathBuf::from(r"C:\local\python.exe"))),
            Some(true)
        );
        for bridge in [
            "ssh.exe",
            "wsl.exe",
            "wslhost.exe",
            "bash.exe",
            "plink.exe",
            "MOSH-CLIENT.EXE",
            "NsEnter.exe",
            "unshare",
        ] {
            tree.push(p(300, 200, bridge));
            assert_eq!(
                classify_media_tree(&tree, 100, |pid| Some(PathBuf::from(format!(
                    r"C:\local\{}",
                    tree.iter().find(|p| p.pid == pid).unwrap().name
                )))),
                Some(false)
            );
            tree.pop();
        }
        assert_eq!(classify_media_tree(&tree, 100, |_| None), None);
        assert_eq!(
            classify_media_tree(&tree, 999, |_| panic!("missing root")),
            None
        );
        assert_eq!(
            classify_media_tree(&tree, 100, |_| Some(PathBuf::from(r"C:\local\python.exe"))),
            Some(true)
        );
    }
}

#[cfg(test)]
mod ctrl_c_tests {
    use std::io::{Read as _, Write as _};
    use std::os::windows::process::CommandExt as _;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    const RESULT_ENV: &str = "LYREL_CTRL_C_INHERITED_IGNORE_RESULT";
    const TEST_NAME: &str =
        "daemon::winproc::ctrl_c_tests::a_pane_survives_an_inherited_ignore_ctrl_c";

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// The reported bug (#451, #314): nothing in a lyrel pane could be
    /// interrupted, so `go run` and `npm install` ran on after Ctrl+C.
    ///
    /// The keystroke path was never at fault — the pane does write 0x03 into the
    /// pty, and conhost does turn it into a keypress. What never happened was
    /// the CTRL_C_EVENT, because the daemon carried Windows' "ignore Ctrl+C"
    /// process state and every ConPTY child inherits it.
    ///
    /// So the state has to be *inherited* here rather than switched on in
    /// place — that is the shape the daemon was in, and it is the only version a
    /// single `SetConsoleCtrlHandler(NULL, FALSE)` has to be able to undo. An
    /// intermediate process created exactly as the daemon used to be supplies
    /// it, and then runs both arms: a pane spawned without the fix must hang,
    /// and a pane spawned after it must not. Testing only the second arm would
    /// pass on its own on a machine whose runner never had the bit set.
    #[test]
    fn a_pane_survives_an_inherited_ignore_ctrl_c() {
        if let Some(result) = std::env::var_os(RESULT_ENV) {
            let unfixed = interruptible();
            super::allow_ctrl_c_in_children();
            let fixed = interruptible();
            std::fs::write(result, format!("{unfixed} {fixed}")).expect("write the arms' verdict");
            return;
        }

        let result =
            std::env::temp_dir().join(format!("lyrel-pane-ctrl-c-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&result);
        let status = Command::new(std::env::current_exe().expect("locate the test executable"))
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(RESULT_ENV, &result)
            // Exactly how the daemon used to be created. CREATE_NEW_PROCESS_GROUP
            // is what leaves the inherited ignore behind.
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("run the arms in a process that inherits the ignore");
        assert!(status.success(), "the inner test process must finish");

        let verdict = std::fs::read_to_string(&result).unwrap_or_else(|_| "MISSING".into());
        let _ = std::fs::remove_file(&result);
        assert_eq!(
            verdict.trim(),
            "false true",
            "a pane spawned under an inherited ignore must be interruptible only after \
             allow_ctrl_c_in_children (got `{}`); `true true` means the ignore never took \
             hold and the test proves nothing, `false false` is the bug in #451 and #314",
            verdict.trim()
        );
    }

    /// Runs a shell in a pane the way `DaemonPane::spawn` does, interrupts a
    /// command that would otherwise never end, and reports whether the shell
    /// came back.
    ///
    /// The observable is the shell, not the interrupted command: after the ^C,
    /// `cmd` gets its prompt back and acts on the `exit` typed behind it. If the
    /// ^C is swallowed, `ping` still owns the console, the `exit` is never read,
    /// and the shell outlives the deadline. Nothing here reads a message, so it
    /// holds on a non-English Windows too.
    fn interruptible() -> bool {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open a pty the way a pane does");
        let mut child = pty
            .slave
            .spawn_command(CommandBuilder::new("cmd.exe"))
            .expect("spawn the shell");
        drop(pty.slave);

        // ConPTY stalls its client once the output pipe fills, so this has to
        // keep running whatever else happens. The byte count it keeps is how the
        // test paces itself instead of guessing at sleeps.
        let mut reader = pty.master.try_clone_reader().expect("clone the reader");
        let written = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = std::sync::Arc::clone(&written);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                counter.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
            }
        });
        let bytes = || written.load(std::sync::atomic::Ordering::Relaxed);
        // Waits for `rounds` separate batches of output. Two of them means the
        // running command is producing its own lines on its own schedule, which
        // is the only reliable sign that it — not the shell — holds the console.
        // Interrupting before that reaches nobody, and would look like this bug
        // on a slow machine for a reason that has nothing to do with the fix.
        let settle = |rounds: usize| {
            let give_up = Instant::now() + Duration::from_secs(20);
            for _ in 0..rounds {
                let before = bytes();
                while bytes() == before && Instant::now() < give_up {
                    std::thread::sleep(Duration::from_millis(50));
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        };

        let mut writer = pty.master.take_writer().expect("take the writer");
        let mut type_in = |keys: &[u8]| {
            writer.write_all(keys).expect("write to the pty");
            writer.flush().expect("flush the pty");
        };
        settle(1); // the shell's banner and first prompt
        // Runs until interrupted, and holds the console while it does.
        type_in(b"ping -t 127.0.0.1\r");
        settle(3); // the echoed line, then replies on ping's own clock
        type_in(&[0x03]);
        std::thread::sleep(Duration::from_millis(500));
        type_in(b"exit\r");

        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            match child.try_wait().expect("poll the shell") {
                Some(_) => return true,
                None if Instant::now() >= deadline => {
                    // Killing the shell leaves the `ping` it never interrupted
                    // running forever, so take the subtree with it. Read the
                    // subtree while the shell is still alive: once it is gone
                    // its pid can be handed to somebody else, and a snapshot
                    // taken after that names a stranger's children.
                    let doomed = child
                        .process_id()
                        .map(|shell| super::descendants(&super::snapshot(), shell))
                        .unwrap_or_default();
                    let _ = child.kill();
                    let _ = child.wait();
                    super::terminate_and_wait_all(&doomed, Instant::now() + Duration::from_secs(2));
                    return false;
                }
                None => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    }
}
