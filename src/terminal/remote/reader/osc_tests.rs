use super::{MAX_OSC_NOTES, Note, OscNotifyScanner, VecDeque, parse_osc_notification};

fn scan(chunks: &[&[u8]]) -> Vec<Note> {
    let mut s = OscNotifyScanner::default();
    let mut out = VecDeque::new();
    for c in chunks {
        s.feed(c, &mut out);
    }
    out.into()
}

/// A burst of notifications shows only its newest few, once.
#[test]
fn a_burst_of_notes_is_handed_over_as_its_newest_few() {
    use crate::daemon::protocol::DaemonMsg;
    use crate::terminal::size::TermSize;

    crate::core::config::pin_test_config_dir();
    let (client, mut daemon) = crate::terminal::view::test_stream_pair();
    let term = super::RemoteTerminal::from_stream(client, TermSize::new(80, 24)).unwrap();
    let burst: Vec<u8> = (0..40)
        .flat_map(|n| format!("\x1b]9;note {n}\x07").into_bytes())
        .collect();
    DaemonMsg::Output(burst).encode(&mut daemon).unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let taken = loop {
        let batch = term.take_osc_notes();
        if !batch.is_empty() || std::time::Instant::now() > deadline {
            break batch;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let bodies: Vec<_> = taken.into_iter().map(|(_, body)| body).collect();
    assert_eq!(bodies, ["note 37", "note 38", "note 39"]);
    assert!(term.take_osc_notes().is_empty(), "the rest were dropped");
}

#[test]
fn an_unread_queue_keeps_only_the_newest_notes() {
    let chunks: Vec<Vec<u8>> = (0..MAX_OSC_NOTES + 2)
        .map(|n| format!("\x1b]9;note {n}\x07").into_bytes())
        .collect();
    let got = scan(&chunks.iter().map(Vec::as_slice).collect::<Vec<_>>());
    assert_eq!(got.len(), MAX_OSC_NOTES);
    assert_eq!(got[0], (None, "note 2".to_string()));
}

#[test]
fn osc9_bel_and_st_terminators() {
    assert_eq!(
        scan(&[b"\x1b]9;Build done\x07"]),
        vec![(None, "Build done".to_string())]
    );
    assert_eq!(
        scan(&[b"\x1b]9;Tests passed\x1b\\"]),
        vec![(None, "Tests passed".to_string())]
    );
}

#[test]
fn osc777_notify_title_and_body() {
    assert_eq!(
        scan(&[b"\x1b]777;notify;Title;Body text\x07"]),
        vec![(Some("Title".to_string()), "Body text".to_string())]
    );
    assert_eq!(
        scan(&[b"\x1b]777;notify;Just a message\x1b\\"]),
        vec![(None, "Just a message".to_string())]
    );
}

#[test]
fn split_across_reads_is_reassembled() {
    assert_eq!(
        scan(&[b"\x1b]9;Hel", b"lo wor", b"ld\x07"]),
        vec![(None, "Hello world".to_string())]
    );
    assert_eq!(
        scan(&[b"\x1b]9;Ping\x1b", b"\\"]),
        vec![(None, "Ping".to_string())]
    );
}

#[test]
fn uninteresting_osc_is_ignored_cheaply() {
    assert_eq!(
        scan(&[b"\x1b]52;c;bWFueSBieXRlcw==\x07\x1b]0;my title\x07"]),
        vec![]
    );
    assert_eq!(
        scan(&[b"\x1b]0;title\x07\x1b]9;After\x07"]),
        vec![(None, "After".to_string())]
    );
}

#[test]
fn claude_codes_kitty_and_ghostty_channels() {
    assert_eq!(
        scan(&[
            b"\x1b]99;i=3:d=0:p=title;Claude Code\x07\x1b]99;i=3:p=body;Done\x07",
            b"\x1b]99;i=3:d=1:a=focus;\x07",
        ]),
        vec![(Some("Claude Code".to_string()), "Done".to_string())]
    );
    assert_eq!(
        scan(&[b"\x1b]777;notify;Claude Code;Done\x07"]),
        vec![(Some("Claude Code".to_string()), "Done".to_string())]
    );
}

#[test]
fn conemu_osc9_subcommands_are_not_notifications() {
    assert_eq!(scan(&[b"\x1b]9;4;1;50\x07"]), vec![]);
    assert_eq!(scan(&[b"\x1b]9;9;/home/u\x07"]), vec![]);
    assert_eq!(scan(&[b"\x1b]9;12\x07"]), vec![], "a prompt mark");
    assert_eq!(scan(&[b"\x1b]9;11;a comment\x07"]), vec![]);
    assert_eq!(scan(&[b"\x1b]9;42\x07"]), vec![(None, "42".to_string())]);
}

#[test]
fn parse_rejects_empty_and_unrelated() {
    let n = &mut Default::default();
    assert_eq!(parse_osc_notification(n, b"9;"), None);
    assert_eq!(parse_osc_notification(n, b"777;notify;"), None);
    assert_eq!(parse_osc_notification(n, b"8;;https://example.com"), None);
}

#[test]
fn resyncs_on_new_osc_after_an_unterminated_one() {
    assert_eq!(
        scan(&[b"\x1b]9;dropped\x1b]9;kept\x07"]),
        vec![(None, "kept".to_string())]
    );
    assert_eq!(
        scan(&[b"\x1b]0;title\x1b]9;After title\x07"]),
        vec![(None, "After title".to_string())]
    );
}
