use super::*;
use std::sync::mpsc;
#[cfg(windows)]
use super::super::{DaemonPane, DaemonMsg, ShellSpec};
#[cfg(windows)]
use super::super::test_support::ws;
    #[test]
    fn join_bounded_returns_true_when_the_thread_finishes() {
        let handle = std::thread::spawn(|| {});
        assert!(join_bounded(handle, Duration::from_secs(5)));
    }

    #[test]
    fn join_bounded_times_out_on_a_stuck_thread() {
        let (unblock, blocked) = mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            let _ = blocked.recv();
        });
        assert!(!join_bounded(handle, Duration::from_millis(50)));
        drop(unblock);
    }
    #[test]
    fn pty_size_clamps_and_computes_pixels() {
        let ps = pty_size(WinSize {
            cols: 80,
            rows: 24,
            cell_w: 8,
            cell_h: 17,
        });
        assert_eq!(ps.rows, 24);
        assert_eq!(ps.cols, 80);
        assert_eq!(ps.pixel_width, 80 * 8);
        assert_eq!(ps.pixel_height, 24 * 17);

        let z = pty_size(WinSize {
            cols: 0,
            rows: 0,
            cell_w: 0,
            cell_h: 0,
        });
        assert_eq!(z.rows, 1);
        assert_eq!(z.cols, 1);
        assert_eq!(z.pixel_width, 0);
        assert_eq!(z.pixel_height, 0);

        let big = pty_size(WinSize {
            cols: u16::MAX,
            rows: u16::MAX,
            cell_w: u16::MAX,
            cell_h: u16::MAX,
        });
        assert_eq!(big.pixel_width, u16::MAX);
        assert_eq!(big.pixel_height, u16::MAX);
    }
    #[cfg(windows)]
    #[test]
    fn closing_a_pane_reaps_console_clients_but_preserves_detached_services() {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{
            CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS, OpenProcess, PROCESS_SYNCHRONIZE,
            PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
        };

        const TEST: &str = "daemon::pane::lifecycle::tests::closing_a_pane_reaps_console_clients_but_preserves_detached_services";
        const DIRECTORY: &str = "LYREL_TEST_CONSOLE_OWNERSHIP";
        const ROLE: &str = "LYREL_TEST_CONSOLE_ROLE";
        let executable = std::env::current_exe().unwrap();
        if let Some(directory) = std::env::var_os(DIRECTORY) {
            if std::env::var_os(ROLE).is_some() {
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
            let directory = PathBuf::from(directory);
            for role in ["attached", "detached"] {
                let child = Command::new(&executable)
                    .args(["--exact", TEST, "--nocapture"])
                    .env(ROLE, role)
                    .creation_flags(if role == "detached" {
                        DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
                    } else {
                        0
                    })
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                std::fs::write(directory.join(role), child.id().to_string()).unwrap();
            }
            while !directory.join("exit").exists() {
                std::thread::sleep(Duration::from_millis(20));
            }
            return;
        }

        struct Clients(Vec<OwnedHandle>);
        impl Drop for Clients {
            fn drop(&mut self) {
                for handle in &self.0 {
                    unsafe {
                        TerminateProcess(handle.as_raw_handle(), 1);
                        WaitForSingleObject(handle.as_raw_handle(), 5_000);
                    }
                }
            }
        }
        for natural_exit in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let script = directory.path().join("start.ps1");
            std::fs::write(&script, format!(
                "$env:{DIRECTORY}='{}'\n& '{}' --exact {TEST} --nocapture\nexit $LASTEXITCODE\n",
                directory.path().display().to_string().replace('\'', "''"),
                executable.display().to_string().replace('\'', "''")
            )).unwrap();
            let pane = DaemonPane::spawn(
                u64::MAX - 10,
                Some(directory.path().into()),
                ws(80, 24),
                Some(ShellSpec {
                    program: "powershell.exe".into(),
                    args: vec![
                        "-NoProfile".into(),
                        "-NonInteractive".into(),
                        "-File".into(),
                        script.to_string_lossy().into_owned(),
                    ],
                    args_are_lyrel_defaults: false,
                }),
                None,
                None,
                None,
                false,
                || {},
            )
            .unwrap();
            let mut clients = Clients(Vec::new());
            for role in ["attached", "detached"] {
                let path = directory.path().join(role);
                let deadline = std::time::Instant::now() + Duration::from_secs(20);
                let pid = loop {
                    if let Some(pid) = std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|text| text.parse().ok())
                    {
                        break pid;
                    }
                    assert!(std::time::Instant::now() < deadline, "{role} did not start");
                    std::thread::sleep(Duration::from_millis(20));
                };
                let handle =
                    unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid) };
                assert!(!handle.is_null(), "open the actual {role} process");
                clients
                    .0
                    .push(unsafe { OwnedHandle::from_raw_handle(handle) });
            }
            if natural_exit {
                let (tx, rx) = mpsc::channel();
                pane.attach(tx);
                std::fs::write(directory.path().join("exit"), b"exit").unwrap();
                loop {
                    if matches!(
                        rx.recv_timeout(Duration::from_secs(20)).unwrap(),
                        DaemonMsg::Exited { .. }
                    ) {
                        break;
                    }
                }
            } else {
                pane.kill();
            }
            drop(pane);
            assert_eq!(
                unsafe { WaitForSingleObject(clients.0[0].as_raw_handle(), 10_000) },
                WAIT_OBJECT_0,
                "attached console client survived pane close (natural_exit={natural_exit})"
            );
            assert_eq!(
                unsafe { WaitForSingleObject(clients.0[1].as_raw_handle(), 0) },
                WAIT_TIMEOUT,
                "detached service was killed by pane close (natural_exit={natural_exit})"
            );
        }
    }
