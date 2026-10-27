use super::super::super::test_support::{drain, null_writer, test_state};
use super::super::super::{DaemonPane, OutputGate, attach_subscriber, observe_subscriber};
use super::*;
#[cfg(windows)]
use std::sync::Condvar;
use std::sync::mpsc::{self, Sender};
#[test]
fn death_notifies_observers_but_only_controllers_defer_the_reap() {
    let with_observer_only = Arc::new(Mutex::new(test_state(true)));
    let (observer_tx, observer_rx) = mpsc::channel();
    observe_subscriber(
        &mut with_observer_only.lock().unwrap(),
        observer_tx,
        Arc::new(OutputGate::new()),
        false,
    );
    drain(&observer_rx);
    let (dead_tx, dead_rx) = mpsc::channel();
    DeathReporter::new(move || dead_tx.send(()).unwrap())
        .report(&with_observer_only, &AtomicBool::new(false));
    assert!(matches!(
        observer_rx.try_recv(),
        Ok(DaemonMsg::Exited { code: None })
    ));
    assert!(
        dead_rx.try_recv().is_ok(),
        "read-only observers must not keep a dead pane in the registry"
    );

    let with_both = Arc::new(Mutex::new(test_state(true)));
    let (controller_tx, controller_rx) = mpsc::channel();
    let (observer_tx, observer_rx) = mpsc::channel();
    {
        let mut st = with_both.lock().unwrap();
        attach_subscriber(&mut st, controller_tx);
        observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);
    }
    drain(&controller_rx);
    drain(&observer_rx);
    let (dead_tx, dead_rx) = mpsc::channel();
    DeathReporter::new(move || dead_tx.send(()).unwrap())
        .report(&with_both, &AtomicBool::new(false));
    assert!(matches!(
        controller_rx.try_recv(),
        Ok(DaemonMsg::Exited { code: None })
    ));
    assert!(matches!(
        observer_rx.try_recv(),
        Ok(DaemonMsg::Exited { code: None })
    ));
    assert!(
        dead_rx.try_recv().is_err(),
        "an attached death is still the detach path's to reclaim"
    );
}

#[test]
fn reader_eof_with_subscriber_sends_exited_not_on_dead() {
    let state = Arc::new(Mutex::new(test_state(true)));
    let (sub_tx, sub_rx) = mpsc::channel();
    attach_subscriber(&mut state.lock().unwrap(), sub_tx);
    drain(&sub_rx);
    let dead = Arc::new(AtomicBool::new(false));
    let dead_flag = dead.clone();

    let handle = DaemonPane::spawn_reader(
        state.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(OutputGate::new()),
        Box::new(std::io::Cursor::new(b"tail".to_vec())),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
            local_group: |_| Some(true),
        },
        Arc::new(DeathReporter::new(move || {
            dead_flag.store(true, Ordering::SeqCst)
        })),
    );
    handle.join().unwrap();

    assert!(!state.lock().unwrap().alive);
    assert_eq!(state.lock().unwrap().ring.flatten(), b"tail");
    assert!(matches!(sub_rx.try_recv(), Ok(DaemonMsg::Output(b)) if b == b"tail"));
    assert!(matches!(
        sub_rx.try_recv(),
        Ok(DaemonMsg::Exited { code: None })
    ));
    assert!(
        !dead.load(Ordering::SeqCst),
        "an attached death is the detach path's to reclaim, not on_dead's"
    );
}

#[cfg(windows)]
#[test]
fn closing_conpty_waits_for_delayed_reader_output_before_exit() {
    /// A master whose destruction opens the simulated ConPTY output pipe.
    /// The production `ConPtyMasterPty` performs the equivalent transition
    /// by calling `ClosePseudoConsole` from its destructor.
    struct SignallingMaster {
        released: Arc<(Mutex<bool>, Condvar)>,
    }

    impl Drop for SignallingMaster {
        fn drop(&mut self) {
            let (lock, ready) = &*self.released;
            *lock.lock().unwrap() = true;
            ready.notify_all();
        }
    }

    impl MasterPty for SignallingMaster {
        fn resize(&self, _size: PtySize) -> anyhow::Result<()> {
            Ok(())
        }

        fn get_size(&self) -> anyhow::Result<PtySize> {
            Ok(PtySize::default())
        }

        fn try_clone_reader(&self) -> anyhow::Result<Box<dyn Read + Send>> {
            Ok(Box::new(std::io::empty()))
        }

        fn take_writer(&self) -> anyhow::Result<Box<dyn Write + Send>> {
            Ok(Box::new(std::io::sink()))
        }
    }

    /// Models a reader that stays delayed well past any plausible grace
    /// period, then receives a final output chunk followed by EOF.
    struct DelayedTailReader {
        released: Arc<(Mutex<bool>, Condvar)>,
        tail: std::io::Cursor<Vec<u8>>,
        delayed: bool,
    }

    impl Read for DelayedTailReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.delayed {
                let (lock, ready) = &*self.released;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = ready.wait(released).unwrap();
                }
                drop(released);
                std::thread::sleep(Duration::from_millis(800));
                self.delayed = true;
            }
            self.tail.read(buf)
        }
    }

    let released = Arc::new((Mutex::new(false), Condvar::new()));
    let master: Mutex<Option<Box<dyn MasterPty + Send>>> =
        Mutex::new(Some(Box::new(SignallingMaster {
            released: released.clone(),
        })));
    let state = Arc::new(Mutex::new(test_state(true)));
    let (sub_tx, sub_rx) = mpsc::channel();
    attach_subscriber(&mut state.lock().unwrap(), sub_tx);
    drain(&sub_rx);

    let reader = DaemonPane::spawn_reader(
        state,
        Arc::new(AtomicBool::new(false)),
        Arc::new(OutputGate::new()),
        Box::new(DelayedTailReader {
            released,
            tail: std::io::Cursor::new(b"final output".to_vec()),
            delayed: false,
        }),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
            local_group: |_| Some(true),
        },
        Arc::new(DeathReporter::new(|| {})),
    );

    close_pty_master(&master);
    assert!(
        sub_rx.recv_timeout(Duration::from_millis(600)).is_err(),
        "closing the master must not publish Exited while the reader is still delayed"
    );
    assert!(matches!(
        sub_rx.recv_timeout(Duration::from_secs(1)),
        Ok(DaemonMsg::Output(bytes)) if bytes == b"final output"
    ));
    assert!(matches!(
        sub_rx.recv_timeout(Duration::from_secs(1)),
        Ok(DaemonMsg::Exited { code: None })
    ));
    assert!(sub_rx.try_recv().is_err(), "no output may follow Exited");
    reader.join().unwrap();
}

/// An inert stand-in for the ConPTY master: releasing it is instant, which
/// is what `ClosePseudoConsole` does once the pipe has no pending bytes.
#[cfg(windows)]
struct InertMaster;

#[cfg(windows)]
impl MasterPty for InertMaster {
    fn resize(&self, _size: PtySize) -> anyhow::Result<()> {
        Ok(())
    }

    fn get_size(&self) -> anyhow::Result<PtySize> {
        Ok(PtySize::default())
    }

    fn try_clone_reader(&self) -> anyhow::Result<Box<dyn Read + Send>> {
        Ok(Box::new(std::io::empty()))
    }

    fn take_writer(&self) -> anyhow::Result<Box<dyn Write + Send>> {
        Ok(Box::new(std::io::sink()))
    }
}

#[cfg(windows)]
fn exit_drain_fixture() -> (
    Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    Arc<Mutex<PaneState>>,
    Arc<AtomicBool>,
    Arc<DeathReporter>,
    mpsc::Receiver<DaemonMsg>,
) {
    let state = Arc::new(Mutex::new(test_state(true)));
    let (sub_tx, sub_rx) = mpsc::channel();
    attach_subscriber(&mut state.lock().unwrap(), sub_tx);
    drain(&sub_rx);
    (
        Arc::new(Mutex::new(Some(
            Box::new(InertMaster) as Box<dyn MasterPty + Send>
        ))),
        state,
        Arc::new(AtomicBool::new(false)),
        Arc::new(DeathReporter::new(|| {})),
        sub_rx,
    )
}

/// A grandchild that inherited the ConPTY output pipe keeps it open after
/// the shell is gone, so the reader never sees EOF and can never publish
/// the exit. The monitor's drain window is the only thing that stops such
/// a pane from reading as alive forever.
#[cfg(windows)]
#[test]
fn a_pty_that_never_reaches_eof_still_reports_the_exit() {
    struct NeverEofReader;

    impl Read for NeverEofReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            // Long enough to outlast the window below, bounded so the
            // thread cannot outlive the test binary.
            std::thread::sleep(Duration::from_secs(5));
            Ok(0)
        }
    }

    let (master, state, shutting_down, death, sub_rx) = exit_drain_fixture();
    let _reader = DaemonPane::spawn_reader(
        state.clone(),
        shutting_down.clone(),
        Arc::new(OutputGate::new()),
        Box::new(NeverEofReader),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
            local_group: |_| Some(true),
        },
        death.clone(),
    );

    drain_then_report(
        master,
        state,
        shutting_down,
        death,
        Duration::from_millis(50),
    );
    assert!(
        matches!(
            sub_rx.recv_timeout(Duration::from_secs(1)),
            Ok(DaemonMsg::Exited { code: None })
        ),
        "the monitor must report the exit once its drain window elapses"
    );
}

/// The ordinary case: EOF arrives, the reader reports, and the monitor
/// neither waits out its window nor publishes a second `Exited`.
#[cfg(windows)]
#[test]
fn the_exit_monitor_defers_to_the_reader_that_saw_eof() {
    let (master, state, shutting_down, death, sub_rx) = exit_drain_fixture();
    let reader = DaemonPane::spawn_reader(
        state.clone(),
        shutting_down.clone(),
        Arc::new(OutputGate::new()),
        Box::new(std::io::empty()),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
            local_group: |_| Some(true),
        },
        death.clone(),
    );

    let started = std::time::Instant::now();
    drain_then_report(master, state, shutting_down, death, Duration::from_secs(30));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the monitor must stop waiting as soon as the reader has reported"
    );
    assert!(matches!(
        sub_rx.recv_timeout(Duration::from_secs(1)),
        Ok(DaemonMsg::Exited { code: None })
    ));
    assert!(
        sub_rx.try_recv().is_err(),
        "the idempotent reporter must not publish a second Exited"
    );
    reader.join().unwrap();
}

#[test]
fn reader_drains_late_text_without_reviving_graphics_after_exit_report() {
    struct ControlledReader {
        entered: Option<Sender<()>>,
        chunks: mpsc::Receiver<Vec<u8>>,
    }

    impl Read for ControlledReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if let Some(entered) = self.entered.take() {
                entered.send(()).unwrap();
            }
            let bytes = self
                .chunks
                .recv_timeout(Duration::from_secs(5))
                .map_err(|error| std::io::Error::other(format!("controlled reader: {error}")))?;
            assert!(bytes.len() <= buf.len());
            buf[..bytes.len()].copy_from_slice(&bytes);
            Ok(bytes.len())
        }
    }

    let state = Arc::new(Mutex::new(test_state(true)));
    let (sub_tx, sub_rx) = mpsc::channel();
    attach_subscriber(&mut state.lock().unwrap(), sub_tx);
    drain(&sub_rx);
    let shutting_down = Arc::new(AtomicBool::new(false));
    let death = Arc::new(DeathReporter::new(|| {}));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (chunks_tx, chunks_rx) = mpsc::channel();
    let reader = DaemonPane::spawn_reader(
        state.clone(),
        shutting_down.clone(),
        Arc::new(OutputGate::new()),
        Box::new(ControlledReader {
            entered: Some(entered_tx),
            chunks: chunks_rx,
        }),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
            local_group: |_| Some(true),
        },
        death.clone(),
    );
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    // Same ordering as the Windows fallback monitor: report the death
    // while the reader is blocked, then let pending PTY bytes arrive.
    death.report(&state, &shutting_down);
    assert!(state.lock().unwrap().graphics.is_none());
    assert!(matches!(
        sub_rx.recv_timeout(Duration::from_secs(1)),
        Ok(DaemonMsg::Exited { .. })
    ));
    chunks_tx.send(b"late ".to_vec()).unwrap();
    chunks_tx
        .send(b"\x1b_Ga=T,i=9,f=32,s=1,v=1,q=2;/wAA".to_vec())
        .unwrap();
    chunks_tx.send(b"/w==\x1b\\final ".to_vec()).unwrap();
    chunks_tx.send(b"\x1b]5522;type=write:id=late\x1b\\\x1b]5522;type=wdata:mime=aW1hZ2UvcG5n;cG5nLWRhdGE=\x1b\\\x1b]5522;type=wdata\x1b\\tail".to_vec()).unwrap();
    chunks_tx.send(Vec::new()).unwrap();
    reader.join().unwrap();

    let mut output = Vec::new();
    for message in sub_rx.try_iter() {
        match message {
            DaemonMsg::Output(bytes) => output.extend(bytes),
            DaemonMsg::Graphics(_) => panic!("graphics cannot revive after the pane exits"),
            DaemonMsg::ClipboardWrite(_) => {
                panic!("clipboard permission must still be enforced")
            }
            DaemonMsg::Exited { .. } => panic!("exit reporting remains idempotent"),
            _ => {}
        }
    }
    assert_eq!(output, b"late final tail");
    let state = state.lock().unwrap();
    assert_eq!(state.ring.flatten(), b"late final tail");
    assert!(state.graphics.is_none());
}
#[test]
fn reader_eof_without_subscriber_fires_on_dead() {
    let state = Arc::new(Mutex::new(test_state(true)));
    let (dead_tx, dead_rx) = mpsc::channel();

    let handle = DaemonPane::spawn_reader(
        state.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(OutputGate::new()),
        Box::new(std::io::Cursor::new(Vec::new())),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
            local_group: |_| Some(true),
        },
        Arc::new(DeathReporter::new(move || dead_tx.send(()).unwrap())),
    );
    handle.join().unwrap();

    assert!(!state.lock().unwrap().alive);
    assert!(dead_rx.try_recv().is_ok(), "unattached death → on_dead");
}

#[test]
fn reader_eof_during_shutdown_is_silent() {
    let state = Arc::new(Mutex::new(test_state(true)));
    let dead = Arc::new(AtomicBool::new(false));
    let dead_flag = dead.clone();

    let handle = DaemonPane::spawn_reader(
        state.clone(),
        Arc::new(AtomicBool::new(true)),
        Arc::new(OutputGate::new()),
        Box::new(std::io::Cursor::new(Vec::new())),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
            local_group: |_| Some(true),
        },
        Arc::new(DeathReporter::new(move || {
            dead_flag.store(true, Ordering::SeqCst)
        })),
    );
    handle.join().unwrap();

    assert!(!state.lock().unwrap().alive);
    assert!(!dead.load(Ordering::SeqCst));
}

#[test]
fn death_reporter_notifies_once_across_racing_callers() {
    let state = Arc::new(Mutex::new(test_state(true)));
    let (sub_tx, sub_rx) = mpsc::channel();
    attach_subscriber(&mut state.lock().unwrap(), sub_tx);
    drain(&sub_rx);
    let shutting_down = AtomicBool::new(false);
    let calls = Arc::new(AtomicBool::new(false));
    let calls_flag = calls.clone();
    let death = DeathReporter::new(move || calls_flag.store(true, Ordering::SeqCst));

    death.report(&state, &shutting_down);
    death.report(&state, &shutting_down);

    assert!(!state.lock().unwrap().alive);
    assert!(matches!(
        sub_rx.try_recv(),
        Ok(DaemonMsg::Exited { code: None })
    ));
    assert!(
        sub_rx.try_recv().is_err(),
        "a second report must not re-notify"
    );
}

#[test]
fn death_reporter_fires_on_dead_at_most_once() {
    let state = Arc::new(Mutex::new(test_state(true)));
    let shutting_down = AtomicBool::new(false);
    let (dead_tx, dead_rx) = mpsc::channel();
    let death = DeathReporter::new(move || dead_tx.send(()).unwrap());

    death.report(&state, &shutting_down);
    death.report(&state, &shutting_down);

    assert!(dead_rx.try_recv().is_ok(), "unattached death → on_dead");
    assert!(dead_rx.try_recv().is_err(), "on_dead must fire only once");
}
