use super::*;
use crate::size::TermSize;
use lyrel_core::daemon::protocol::ProgramNote;

#[test]
fn a_burst_of_live_notes_keeps_the_newest_ids_and_ignores_snapshot_osc() {
    lyrel_ui_base::config::pin_test_config_dir();
    let (client, mut daemon) = crate::view::test_stream_pair();
    let term = RemoteTerminal::from_stream(client, TermSize::new(80, 24)).unwrap();
    DaemonMsg::Snapshot(b"\x1b]9;historical notice\x07".to_vec())
        .encode(&mut daemon)
        .unwrap();
    for id in 1..=5 {
        DaemonMsg::ProgramNote(ProgramNote {
            id: uuid::Uuid::from_u128(id),
            title: None,
            body: "identical text".into(),
        })
        .encode(&mut daemon)
        .unwrap();
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let notes = term.osc_notes.lock().unwrap();
        if notes.back().is_some_and(|note| note.id.as_u128() == 5) {
            break;
        }
        drop(notes);
        assert!(
            std::time::Instant::now() < deadline,
            "last note never arrived"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let ids: Vec<_> = term
        .take_osc_notes()
        .into_iter()
        .map(|note| note.id.as_u128())
        .collect();
    assert_eq!(ids, [3, 4, 5]);
    assert!(term.take_osc_notes().is_empty());
}
