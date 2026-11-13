use super::super::test_support::{drain, test_state, ws};
use super::super::{RING_CAP, ReplayRing, attach_subscriber};
use super::output_marks::OscSniffer;
use super::*;
use crate::daemon::protocol::{PaneProgress, ProgressKind};

#[test]
fn progress_chunking_and_identical_reports_do_not_publish_duplicate_changes() {
    let bytes = b"\x1b]9;4;4;61\x1b\\";
    for cut in 0..=bytes.len() {
        let mut facts = PaneFacts::default();
        let mut sniffer = OscSniffer::new();
        let mut updates = Vec::new();
        for part in [&bytes[..cut], &bytes[cut..], bytes.as_slice()] {
            apply_signals(&mut facts, sniffer.feed(part), &mut |message| {
                updates.push(message)
            });
        }
        let progress: Vec<_> = updates
            .into_iter()
            .filter(|msg| matches!(msg, DaemonMsg::Progress(_)))
            .collect();
        assert_eq!(
            progress,
            vec![DaemonMsg::Progress(Some(PaneProgress::new(
                ProgressKind::Paused,
                Some(61)
            )))],
            "cut={cut}"
        );
        for invalid in [
            b"\x1b]9;4;9;32\x07".as_slice(),
            b"\x1b]9;4;1;101\x07",
            b"\x1b]9;4;2;bad\x07",
        ] {
            apply_signals(&mut facts, sniffer.feed(invalid), &mut |_| {
                panic!("invalid progress changed state")
            });
        }
    }
}

#[test]
fn progress_live_attach_survives_ring_eviction_but_persisted_text_never_seeds_new_shell() {
    let mut st = test_state(true);
    let mut sniffer = OscSniffer::new();
    let progress = b"\x1b]9;4;1;42\x07";
    apply_signals(&mut st.facts, sniffer.feed(progress), &mut |_| {});
    st.ring.append(progress);
    let persisted_before_eviction = st.ring.snapshot();
    st.ring.append(&vec![b'x'; RING_CAP]);
    let (tx, rx) = std::sync::mpsc::channel();
    attach_subscriber(&mut st, tx);
    let replay = drain(&rx);
    assert!(
        replay.iter().any(|msg| *msg
            == DaemonMsg::Progress(Some(PaneProgress::new(ProgressKind::Running, Some(42)))))
    );
    assert!(!replay.iter().any(|msg| matches!(msg, DaemonMsg::Snapshot(bytes) if bytes.windows(progress.len()).any(|window| window == progress))));
    // Disk scrollback is text, not authority. Replacement shells start with new
    // PaneFacts even when their restored screen still contains old OSC reports.
    let mut replacement = test_state(true);
    replacement.ring = ReplayRing::seeded(persisted_before_eviction, ws(80, 24));
    let (tx, rx) = std::sync::mpsc::channel();
    attach_subscriber(&mut replacement, tx);
    let replay = drain(&rx);
    assert!(replay.iter().any(|msg| matches!(msg, DaemonMsg::Snapshot(bytes) if bytes.windows(progress.len()).any(|window| window == progress))));
    assert!(replay.iter().any(|msg| *msg == DaemonMsg::Progress(None)));
    assert!(
        !replay
            .iter()
            .any(|msg| matches!(msg, DaemonMsg::Progress(Some(_))))
    );
}
