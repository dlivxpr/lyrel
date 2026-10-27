use super::last_lines;
use crate::address::Context;
use crate::commands::test_support::human;
use crate::commands::test_support::json_of;
use crate::commands::test_support::mock;
use crate::commands::test_support::run_cli;
use crate::commands::test_support::segment;
use serde_json::json;

#[test]
fn capture_and_procs_are_wired_through_the_backend() {
    let mut backend = mock();
    backend.capture_segments = vec![segment(b"$ make\r\nok\r\n")];
    let out = run_cli(
        &["lyrel", "capture", "%2", "--scrollback"],
        &Context::default(),
        &mut backend,
    );
    assert_eq!(backend.captured, vec![(2, true)]);
    assert_eq!(
        human(out),
        "$ make\r\nok\r\n",
        "without --plain the pane's bytes are passed through untouched"
    );

    run_cli(&["lyrel", "procs", "%1"], &Context::default(), &mut backend);
    assert_eq!(backend.procs_calls, vec![1]);
}

#[test]
fn capture_plain_replays_the_bytes_through_a_grid() {
    let mut backend = mock();
    // Coloured, CR-overwritten, and wrapped past the 20-column pane: three
    // things the raw form shows verbatim and `--plain` has to resolve.
    backend.capture_segments = vec![segment(
        b"\x1b[32m$ make\x1b[0m\r\n10%\r100%\r\nabcdefghijklmnopqrstuvwxyz\r\n",
    )];
    let raw = human(run_cli(
        &["lyrel", "capture", "%2"],
        &Context::default(),
        &mut backend,
    ));
    assert!(
        raw.contains("\x1b[32m"),
        "the default keeps escapes: {raw:?}"
    );

    let plain = human(run_cli(
        &["lyrel", "capture", "%2", "--plain"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(plain, "$ make\n100%\nabcdefghijklmnopqrstuvwxyz");
}

#[test]
fn capture_json_carries_whichever_form_was_asked_for() {
    let mut backend = mock();
    backend.capture_segments = vec![segment(b"\x1b[31mred\x1b[0m\r\n")];
    let raw = json_of(run_cli(
        &["lyrel", "capture", "%2"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(raw["text"], json!("\u{1b}[31mred\u{1b}[0m\r\n"));

    let plain = json_of(run_cli(
        &["lyrel", "capture", "%2", "--plain"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(plain["text"], json!("red"));
    assert_eq!(plain["pane"], json!(2));
}

#[test]
fn capture_tail_keeps_the_last_lines_of_either_form() {
    let mut backend = mock();
    let replay = b"one\r\ntwo\r\nthree\r\nfour\r\n";
    backend.capture_segments = vec![segment(replay)];

    let plain = human(run_cli(
        &["lyrel", "capture", "%2", "--plain", "--tail", "2"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(plain, "three\nfour");

    let raw = human(run_cli(
        &["lyrel", "capture", "%2", "--tail", "2"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(
        raw, "three\r\nfour\r\n",
        "the raw form still hands back the pane's own bytes, CR included"
    );

    // More lines asked for than exist is the whole answer, not an error —
    // `tail -n 99` of a three-line file is the file.
    let all = human(run_cli(
        &["lyrel", "capture", "%2", "--plain", "--tail", "99"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(all, "one\ntwo\nthree\nfour");

    // And `--json` reports the tail it printed, over the byte count of the
    // whole replay: the two together are what say a tail was taken.
    let tailed = json_of(run_cli(
        &["lyrel", "capture", "%2", "--plain", "--tail", "1"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(tailed["text"], json!("four"));
    assert_eq!(tailed["bytes"], json!(replay.len()));
}

#[test]
fn a_tail_counts_lines_the_way_tail_does() {
    // A trailing newline ends the last line rather than opening an empty
    // one, which is the difference between `tail -n 1` answering "b" and
    // answering nothing at all.
    assert_eq!(last_lines("a\nb\n", 1), "b\n");
    assert_eq!(last_lines("a\nb", 1), "b");
    assert_eq!(last_lines("a\nb\n", 2), "a\nb\n");
    assert_eq!(last_lines("a\nb\n", 9), "a\nb\n");
    assert_eq!(last_lines("", 3), "");
    assert_eq!(last_lines("\n", 1), "\n");
    assert_eq!(
        last_lines("keep\n\n\n", 2),
        "\n\n",
        "blank lines are lines; a tail is not a filter"
    );
}

#[test]
fn capture_json_counts_the_bytes_the_replay_carried() {
    // The whole point of the field: `text` is empty in both of these, and
    // only `bytes` says which of the two happened. The first is a pane that
    // printed nothing; the second printed a screenful and then cleared it,
    // so the bytes are real and the grid they drive is blank.
    let mut backend = mock();
    let blank = json_of(run_cli(
        &["lyrel", "capture", "%2", "--plain"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(blank["text"], json!(""));
    assert_eq!(blank["bytes"], json!(0));

    let cleared = b"\x1b[2J\x1b[3J\x1b[H";
    backend.capture_segments = vec![segment(cleared)];
    let wiped = json_of(run_cli(
        &["lyrel", "capture", "%2", "--plain"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(wiped["text"], json!(""));
    assert_eq!(
        wiped["bytes"],
        json!(cleared.len()),
        "an empty render has to be told apart from an empty replay"
    );

    // And the count is of the replay, not of what came out of the grid:
    // escapes are bytes the pane produced even though no text survives them.
    let coloured_bytes = b"\x1b[31mred\x1b[0m\r\n";
    backend.capture_segments = vec![segment(coloured_bytes)];
    let coloured = json_of(run_cli(
        &["lyrel", "capture", "%2", "--plain"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(coloured["text"], json!("red"));
    assert_eq!(coloured["bytes"], json!(coloured_bytes.len()));
}
