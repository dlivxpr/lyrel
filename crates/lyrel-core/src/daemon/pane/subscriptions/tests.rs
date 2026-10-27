use super::*;
use super::super::{
    attach_subscriber, attach_subscriber_with_permissions, fan_out_output, notify,
    observe_subscriber, record_output,
};
use super::super::test_support::{drain, test_state};
use std::sync::mpsc;
use std::path::PathBuf;

#[test]
fn gate_passes_below_high_water() {
    let gate = OutputGate::new();
    gate.add((OutputGate::HIGH_WATER - 1) as usize);
    let t0 = std::time::Instant::now();
    gate.wait_below_high_water();
    assert!(
        t0.elapsed() < Duration::from_millis(100),
        "no wait expected"
    );
}

#[test]
fn gate_parks_at_high_water_until_drained() {
    let gate = Arc::new(OutputGate::new());
    gate.add(OutputGate::HIGH_WATER as usize);

    let drainer = {
        let gate = gate.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            gate.sub(OutputGate::HIGH_WATER as usize);
        })
    };
    let t0 = std::time::Instant::now();
    gate.wait_below_high_water();
    let waited = t0.elapsed();
    drainer.join().unwrap();
    assert!(waited >= Duration::from_millis(40), "must park until sub()");
    assert!(
        waited < OutputGate::MAX_WAIT,
        "the drain, not the escape timeout, must unpark"
    );
}

#[test]
fn gate_reset_unparks_a_throttled_reader() {
    let gate = Arc::new(OutputGate::new());
    gate.add(OutputGate::HIGH_WATER as usize * 2);

    let resetter = {
        let gate = gate.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            gate.reset();
        })
    };
    let t0 = std::time::Instant::now();
    gate.wait_below_high_water();
    resetter.join().unwrap();
    assert!(t0.elapsed() < OutputGate::MAX_WAIT);
}

#[test]
fn gate_tolerates_negative_drift() {
    let gate = OutputGate::new();
    gate.sub(1024);
    gate.add(512);
    let t0 = std::time::Instant::now();
    gate.wait_below_high_water();
    assert!(t0.elapsed() < Duration::from_millis(100));
}


#[test]
fn attach_replays_state_in_order_and_installs_subscriber() {
    let mut st = test_state(true);
    st.ring.append(b"screen");
    st.facts = PaneFacts::new(Some(PathBuf::from("/work")), None, None, None);

    let (tx, rx) = mpsc::channel();
    let epoch = attach_subscriber(&mut st, tx);
    assert_eq!(epoch, 1);
    assert!(st.subscriptions.controller_epoch().is_some());
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(_))));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"screen"));
    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Cwd(p)) if p.as_path() == std::path::Path::new("/work"))
    );
    assert!(rx.try_recv().is_err());
}


#[test]
fn attach_replays_initial_cwd_even_before_shell_reports_osc7() {
    let mut st = test_state(true);
    st.facts = PaneFacts::new(Some(PathBuf::from("/Users/alice/clone/lyrel")), None, None, None);

    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);

    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(_))));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(_))));
    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Cwd(p)) if p == PathBuf::from("/Users/alice/clone/lyrel"))
    );
}

/// Issue #774: `btop` sends its alternate-screen and mouse-reporting
/// prefix once, when it starts, and then refreshes for hours. The ring
/// holds the last few megabytes of those refreshes and nothing of the
/// prefix, so a client that rebuilt its terminal from replayed bytes alone
/// came back on the primary screen with reporting off — and its wheel,
/// reading those modes, scrolled the scrollback of a screen that has none.
#[test]
fn attach_restores_modes_whose_bytes_the_ring_has_dropped() {
    let mut st = test_state(true);
    record_output(&mut st, b"\x1b[?1049h\x1b[?1002h\x1b[?1006h");
    // A long enough run of refreshes to push the prefix out of the front.
    record_output(&mut st, &vec![b'.'; RING_CAP]);
    assert!(
        !st.ring.flatten().windows(8).any(|w| w == b"\x1b[?1049h"),
        "the point of the test is that the prefix is gone from the ring"
    );

    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);

    // Ahead of the replayed screen, so the frames land in the buffer the
    // modes put the client on.
    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"\x1b[?1049h\x1b[?1002h\x1b[?1006h"),
        "the modes the ring lost must be re-sent, in the order they were set"
    );
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(_))));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(_))));
}

/// #1074: an agent CLI pushes its kitty keyboard flags once at startup,
/// and a long session pushes that push out of the ring. A client that
/// reattached without it sent Escape as a bare `ESC`.
///
/// The stack goes back as it stood at the front of the ring, not as it
/// ends: the pop and push the ring still holds act on it from there, and
/// stacking them on the final state would leave the client a level deeper
/// than the program.
#[test]
fn attach_restores_kitty_keyboard_flags_the_ring_has_dropped() {
    let mut st = test_state(true);
    record_output(&mut st, b"\x1b[>1u");
    record_output(&mut st, &vec![b'.'; RING_CAP]);
    record_output(&mut st, b"\x1b[<u\x1b[>1u");

    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);

    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"\x1b[>1u"),
        "the push the ring lost must be re-sent ahead of it"
    );
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(_))));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(_))));
}

#[test]
fn kitty_keyboard_flags_the_ring_still_holds_are_left_to_it() {
    let mut st = test_state(true);
    record_output(&mut st, b"\x1b[>1u");

    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);

    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Size(_))),
        "nothing goes ahead of a ring that carries the push itself"
    );
}

#[test]
fn a_pane_that_left_the_alternate_screen_restores_no_modes() {
    let mut st = test_state(true);
    record_output(&mut st, b"\x1b[?1049h\x1b[?1002hvim\x1b[?1002l\x1b[?1049l");
    record_output(&mut st, b"$ ");

    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);

    // Straight to the ring: a shell prompt is not owed a mode frame, and
    // sending one would put the pane on a screen it had left.
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(_))));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(_))));
    assert!(rx.try_recv().is_err());
}

/// The other side of the same coin, and the common case: reconnect a minute
/// after opening `vim` and the ring still holds the whole session, prefix
/// included. Re-sending the prefix then would turn the ring's own `?1049h`
/// into a no-op, so the shell scrollback the ring holds ahead of it would be
/// painted into the alternate screen — which keeps no history and is thrown
/// away when the program exits, leaving the user back on a blank primary
/// buffer instead of the prompt they left behind.
#[test]
fn a_prefix_the_ring_still_carries_is_left_to_the_ring() {
    let mut st = test_state(true);
    record_output(&mut st, b"$ vim notes.md\r\n");
    record_output(&mut st, b"\x1b[?1049h\x1b[?1002h\x1b[?1006hthe file\r\n");

    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);

    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Size(_))),
        "the ring speaks for its own modes; nothing goes ahead of it"
    );
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(_))));
    assert!(rx.try_recv().is_err());
}

/// A mode the cap falls through is a mode the ring cannot set: the front
/// evicts the rest of the sequence too rather than replay its tail as
/// text, so the mode has to come from the fold.
#[test]
fn a_prefix_the_ring_cut_in_half_is_restored() {
    let mut st = test_state(true);
    record_output(&mut st, b"\x1b[?1049h");
    // Four bytes over the cap, so the cap falls just after the `ESC [ ? 1`
    // the sequence opens with, and the `049h` after it goes as well.
    record_output(&mut st, &vec![b'.'; RING_CAP - 4]);
    assert!(st.ring.flatten().iter().all(|&b| b == b'.'));

    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);
    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"\x1b[?1049h"),
        "the bytes left in the ring put no client on the alternate screen"
    );
}

/// An observer joins mid-session too, and reads the same pane state.
#[test]
fn observers_are_told_the_pane_modes_as_well() {
    let mut st = test_state(true);
    record_output(&mut st, b"\x1b[?1049h");
    record_output(&mut st, &vec![b'.'; RING_CAP]);

    let (tx, rx) = mpsc::channel();
    observe_subscriber(&mut st, tx, Arc::new(OutputGate::new()), false);
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"\x1b[?1049h"));
}

#[test]
fn attach_to_a_dead_pane_replays_exited() {
    let mut st = test_state(false);
    st.ring.append(b"final screen");

    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(_))));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(_))));
    assert!(matches!(
        rx.try_recv(),
        Ok(DaemonMsg::Exited { code: None })
    ));
}

#[test]
fn observe_replays_state_without_displacing_the_controller() {
    let mut st = test_state(true);
    st.ring.append(b"screen");
    st.facts = PaneFacts::new(Some(PathBuf::from("/work")), None, None, None);

    let (controller_tx, controller_rx) = mpsc::channel();
    let epoch = attach_subscriber(&mut st, controller_tx);
    drain(&controller_rx);

    let (observer_tx, observer_rx) = mpsc::channel();
    let id = observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);
    assert_eq!(
        st.subscriptions.subscriber_epoch, epoch,
        "observing must not bump the controller epoch"
    );
    assert!(st.subscriptions.controller_epoch().is_some(), "the controller keeps its seat");

    assert!(matches!(observer_rx.try_recv(), Ok(DaemonMsg::Size(_))));
    assert!(matches!(observer_rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"screen"));
    assert!(
        matches!(observer_rx.try_recv(), Ok(DaemonMsg::Cwd(p)) if p == PathBuf::from("/work"))
    );
    assert!(
        controller_rx.try_recv().is_err(),
        "an observer joining must be invisible to the controller"
    );

    notify(&mut st, DaemonMsg::Output(b"tick".to_vec()));
    assert!(matches!(controller_rx.try_recv(), Ok(DaemonMsg::Output(b)) if b == b"tick"));
    assert!(matches!(observer_rx.try_recv(), Ok(DaemonMsg::Output(b)) if b == b"tick"));

    st.subscriptions.unobserve(id, &mut st.graphics);
    notify(&mut st, DaemonMsg::Output(b"tock".to_vec()));
    assert!(matches!(controller_rx.try_recv(), Ok(DaemonMsg::Output(_))));
    assert!(
        observer_rx.try_recv().is_err(),
        "a departed observer hears nothing"
    );
}

#[test]
fn a_new_controller_preempts_the_old_while_observers_survive() {
    let mut st = test_state(true);

    let (first_tx, first_rx) = mpsc::channel();
    let first_epoch = attach_subscriber(&mut st, first_tx);
    drain(&first_rx);

    let (observer_tx, observer_rx) = mpsc::channel();
    observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);
    drain(&observer_rx);

    let (second_tx, second_rx) = mpsc::channel();
    let second_epoch = attach_subscriber(&mut st, second_tx);
    assert!(second_epoch > first_epoch);
    drain(&second_rx);

    notify(&mut st, DaemonMsg::Output(b"live".to_vec()));
    assert!(
        matches!(first_rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)),
        "the preempted controller's channel must be gone, exactly as before"
    );
    assert!(matches!(second_rx.try_recv(), Ok(DaemonMsg::Output(b)) if b == b"live"));
    assert!(
        matches!(observer_rx.try_recv(), Ok(DaemonMsg::Output(b)) if b == b"live"),
        "a controller handover must not evict read-only observers"
    );

    st.subscriptions.detach(first_epoch, &OutputGate::new(), &mut st.graphics);
    assert!(
        st.subscriptions.controller_epoch().is_some(),
        "a stale epoch's detach must not unseat the new controller"
    );
}

#[test]
fn a_gone_observer_is_pruned_on_the_next_broadcast() {
    let mut st = test_state(true);
    let (observer_tx, observer_rx) = mpsc::channel();
    observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);
    drop(observer_rx);

    notify(&mut st, DaemonMsg::Output(b"x".to_vec()));
    assert!(
        st.subscriptions.observers.is_empty(),
        "a dead observer must not accumulate"
    );
}

#[test]
fn a_stalled_observer_is_dropped_at_its_budget_while_the_controller_streams_on() {
    let mut st = test_state(true);
    let (controller_tx, controller_rx) = mpsc::channel();
    attach_subscriber(&mut st, controller_tx);
    drain(&controller_rx);

    let (observer_tx, observer_rx) = mpsc::channel();
    observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);
    drain(&observer_rx);

    let pane_gate = OutputGate::new();
    let chunk = vec![b'x'; 1024 * 1024];
    let sends = (OBSERVER_BUDGET / chunk.len() as i64) as usize + 2;
    for _ in 0..sends {
        fan_out_output(&mut st, &chunk, Vec::new(), &pane_gate);
        pane_gate.sub(chunk.len());
    }

    assert!(
        st.subscriptions.observers.is_empty(),
        "an observer past its budget must be pruned"
    );
    let mut controller_bytes = 0usize;
    while let Ok(DaemonMsg::Output(b)) = controller_rx.try_recv() {
        controller_bytes += b.len();
    }
    assert_eq!(
        controller_bytes,
        sends * chunk.len(),
        "the controller stream must stay complete"
    );
    let mut observer_bytes = 0i64;
    while let Ok(DaemonMsg::Output(b)) = observer_rx.try_recv() {
        observer_bytes += b.len() as i64;
    }
    assert!(
        observer_bytes <= OBSERVER_BUDGET,
        "a stalled observer must never hold more than its budget, held {observer_bytes}"
    );
}

/// A phone opening a pane with a long history: the replay is a full ring,
/// which is the whole observer budget. Once the writer has put it on the
/// wire, live output must still reach the observer.
/// The pane keeps printing while a full ring's replay is still going out
/// to the phone; that output is the observer's too.
#[test]
fn an_observer_still_reading_a_full_ring_replay_gets_live_output() {
    let mut st = test_state(true);
    st.ring.append(&vec![b'h'; RING_CAP]);
    let (observer_tx, observer_rx) = mpsc::channel();
    observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);

    fan_out_output(&mut st, b"tick", Vec::new(), &OutputGate::new());
    assert_eq!(st.subscriptions.observers.len(), 1, "the observer is still subscribed");
    assert!(
        observer_rx
            .try_iter()
            .any(|m| matches!(m, DaemonMsg::Output(b) if b == b"tick"))
    );
}

/// One dropped at its budget hears why, and then its channel closes, so
/// its stream ends rather than going quiet for good.
#[test]
fn an_observer_dropped_at_its_budget_is_told_and_let_go() {
    let mut st = test_state(true);
    let (observer_tx, observer_rx) = mpsc::channel();
    observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);
    drain(&observer_rx);

    let chunk = vec![b'x'; 1024 * 1024];
    while !st.subscriptions.observers.is_empty() {
        fan_out_output(&mut st, &chunk, Vec::new(), &OutputGate::new());
    }
    let last = observer_rx.try_iter().last();
    assert!(matches!(last, Some(DaemonMsg::Error(m)) if m == OBSERVER_BEHIND));
    assert!(matches!(
        observer_rx.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
}

#[cfg(unix)]
#[test]
fn an_observer_that_drained_a_full_ring_replay_keeps_getting_output() {
    use std::os::unix::net::UnixStream;
    let mut st = test_state(true);
    st.ring.append(&vec![b'h'; RING_CAP]);
    let (observer_tx, observer_rx) = mpsc::channel();
    let observer_gate = Arc::new(OutputGate::new());
    observe_subscriber(&mut st, observer_tx, observer_gate.clone(), false);

    // The daemon's own writer puts the replay on the wire, as for a phone.
    let (mut client, server) = UnixStream::pair().unwrap();
    let writer =
        crate::daemon::server::spawn_writer(observer_rx, server, observer_gate.clone());
    let mut replayed = 0;
    while replayed < RING_CAP {
        if let DaemonMsg::Snapshot(b) = DaemonMsg::read(&mut client).unwrap() {
            replayed += b.len();
        }
    }
    let paid = std::time::Instant::now() + Duration::from_secs(5);
    while observer_gate.queued_bytes() > 0 && std::time::Instant::now() < paid {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(observer_gate.queued_bytes(), 0, "the replay is paid off");

    let pane_gate = OutputGate::new();
    fan_out_output(&mut st, b"live", Vec::new(), &pane_gate);
    assert_eq!(st.subscriptions.observers.len(), 1, "the observer is still subscribed");
    assert_eq!(
        DaemonMsg::read(&mut client).unwrap(),
        DaemonMsg::Output(b"live".to_vec())
    );
    st.subscriptions.observers.clear();
    writer.join().unwrap();
}

/// A controller's gate starts at its replay, so crediting that replay as
/// it is written brings the gate back to zero rather than below it.
#[test]
fn a_controller_gate_is_charged_for_its_replay() {
    let mut st = test_state(true);
    st.ring.append(b"screen");
    let gate = OutputGate::new();
    gate.add(123);
    let (tx, rx) = mpsc::channel();
    attach_subscriber_with_permissions(&mut st, tx, &gate, false, false);
    assert_eq!(gate.queued_bytes(), b"screen".len() as i64);
    while let Ok(msg) = rx.try_recv() {
        gate.sub(gated_len(&msg));
    }
    assert_eq!(gate.queued_bytes(), 0);
}

#[test]
fn a_draining_observer_under_the_cap_stays_subscribed() {
    let mut st = test_state(true);
    let (observer_tx, observer_rx) = mpsc::channel();
    let observer_gate = Arc::new(OutputGate::new());
    observe_subscriber(&mut st, observer_tx, observer_gate.clone(), false);
    drain(&observer_rx);

    let pane_gate = OutputGate::new();
    let chunk = vec![b'y'; 1024 * 1024];
    let sends = (OBSERVER_BUDGET / chunk.len() as i64) as usize * 3;
    let mut got = 0usize;
    for _ in 0..sends {
        fan_out_output(&mut st, &chunk, Vec::new(), &pane_gate);
        pane_gate.sub(chunk.len());
        while let Ok(DaemonMsg::Output(b)) = observer_rx.try_recv() {
            observer_gate.sub(b.len());
            got += b.len();
        }
    }
    assert_eq!(
        st.subscriptions.observers.len(),
        1,
        "an observer that keeps draining must stay subscribed"
    );
    assert_eq!(got, sends * chunk.len(), "and must miss no bytes");
}

