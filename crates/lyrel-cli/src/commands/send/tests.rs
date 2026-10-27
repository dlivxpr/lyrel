use crate::address;
use crate::address::Context;
use crate::commands::execute;
use crate::commands::test_support::cli;
use crate::commands::test_support::json_of;
use crate::commands::test_support::mock;
use crate::commands::test_support::run_cli;

#[test]
fn send_reaches_the_pane_socket_seam_not_the_control_socket() {
    let mut backend = mock();
    run_cli(
        &["lyrel", "send", "%1", "make -j8", "--enter"],
        &Context::default(),
        &mut backend,
    );
    assert_eq!(
        backend.sent,
        vec![(1, b"make -j8".to_vec()), (1, b"\r".to_vec())]
    );
    assert!(backend.control_calls.is_empty());

    let ctx = Context {
        pane: Some("%3".into()),
        ..Context::default()
    };
    backend.sent.clear();
    run_cli(&["lyrel", "send", "echo hi"], &ctx, &mut backend);
    assert_eq!(backend.sent, vec![(3, b"echo hi".to_vec())]);
}

/// The keystrokes text cannot express. Each goes out as its own write, in
/// the order given, because a pane reads them as separate key events —
/// which is what walking a menu and then confirming it requires.
#[test]
fn send_key_presses_keys_in_order() {
    let mut backend = mock();
    run_cli(
        &["lyrel", "send", "%1", "--key", "down", "--key", "enter"],
        &Context::default(),
        &mut backend,
    );
    assert_eq!(
        backend.sent,
        vec![(1, b"\x1b[B".to_vec()), (1, b"\r".to_vec())]
    );

    // Text and keys compose: type the answer, then press the key that
    // submits it in whatever the pane is showing.
    let mut backend = mock();
    run_cli(
        &["lyrel", "send", "%1", "y", "--key", "enter"],
        &Context::default(),
        &mut backend,
    );
    assert_eq!(backend.sent, vec![(1, b"y".to_vec()), (1, b"\r".to_vec())]);

    // Interrupting takes no text at all — the case that made TEXT optional.
    let mut backend = mock();
    let json = json_of(run_cli(
        &["lyrel", "send", "%1", "--key", "C-c"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(backend.sent, vec![(1, vec![0x03])]);
    assert_eq!(json["keys"], serde_json::json!(["c-c"]));
    assert_eq!(json["sent"], "", "nothing was typed");
}

/// A lone address still has to be the missing-text error it always was —
/// otherwise `lyrel send %42` would silently do nothing at all.
#[test]
fn send_still_refuses_a_bare_address_when_there_is_nothing_to_press() {
    let mut backend = mock();
    let err = execute(
        cli(&["lyrel", "send", "%1"]),
        &Context::default(),
        &mut backend,
    )
    .expect_err("a bare address sends nothing and must say so");
    assert!(err.to_string().contains("needs TEXT"), "{err}");
    assert!(backend.sent.is_empty());

    // And outside a lyrel shell, with neither text nor keys, the complaint
    // is about the missing input rather than the missing pane.
    let mut backend = mock();
    let err = execute(cli(&["lyrel", "send"]), &Context::default(), &mut backend)
        .expect_err("send with no arguments has nothing to do");
    assert!(err.to_string().contains("--key"), "{err}");
}

/// A mistyped address must not degrade into text aimed at the caller's
/// own pane: `send %3x --key C-c` used to type `%3x` where you sat and
/// then interrupt whatever was in front of you. The guard only fires when
/// the `%` is followed by a digit — "tried to write an address" — so vim's
/// `%s/…` and `%!sort` keep working as text. The `Context::default()`
/// every other send test uses can never reach this branch: without
/// $LYREL_PANE the fallback errors OUTSIDE_SHELL before the guard matters
/// (#538).
#[test]
fn a_broken_address_errors_instead_of_typing_into_the_callers_pane() {
    let ctx = Context {
        pane: Some("5".into()),
        ..Context::default()
    };

    let mut backend = mock();
    let err = execute(
        cli(&["lyrel", "send", "%3x", "--key", "C-c"]),
        &ctx,
        &mut backend,
    )
    .expect_err("a broken address is an error, not text for your own pane");
    assert!(err.to_string().contains("pane address"), "{err}");
    assert!(
        backend.sent.is_empty(),
        "nothing reached any pane — not the text, not the key"
    );

    // Any digit after the `%` is the same reach for an address.
    let mut backend = mock();
    let err = execute(cli(&["lyrel", "send", "%42a"]), &ctx, &mut backend)
        .expect_err("still an address-shaped error, not text");
    assert!(err.to_string().contains("pane address"), "{err}");
    assert!(backend.sent.is_empty());
}

/// A lone bare id now reads as an address, so `send 83` no longer types
/// "83" into the caller's pane — it says it has nothing to send. That is
/// the one behaviour this change takes away, and it has to fail loudly
/// rather than quietly press keys somewhere else: `--enter` presses a key
/// now (#581), but it still does not turn an unmarked id into a target.
#[test]
fn a_lone_bare_id_refuses_loudly_rather_than_retargeting() {
    let ctx = Context {
        pane: Some("5".into()),
        ..Context::default()
    };
    let mut backend = mock();
    let err = execute(cli(&["lyrel", "send", "83"]), &ctx, &mut backend)
        .expect_err("a bare id has nothing to send");
    assert!(err.to_string().contains("needs TEXT"), "{err}");
    // The escape hatch for typing it anyway is in the message.
    assert!(err.to_string().contains("send %PANE 83"), "{err}");
    assert!(
        backend.sent.is_empty(),
        "no keystroke reached pane 83 or pane 5"
    );

    // `--enter` counts as the keystroke it always was (#581) — but not
    // enough to promote an *unmarked* id, or `send 2 --enter` meaning "type
    // 2 and run it" would press Enter in pane 2 instead. Both ways out are
    // named, because either could have been meant.
    let mut backend = mock();
    let err = execute(cli(&["lyrel", "send", "83", "--enter"]), &ctx, &mut backend)
        .expect_err("--enter alone does not make a bare id a target");
    assert!(err.to_string().contains("send %83 --enter"), "{err}");
    assert!(err.to_string().contains("send %PANE 83 --enter"), "{err}");
    assert!(
        backend.sent.is_empty(),
        "no keystroke reached pane 83 or pane 5"
    );
}

/// `--enter` is documented as shorthand for `--key enter`, so it has to
/// give a lone address something to do exactly as `--key` does — it used to
/// report "needs TEXT … or a --key to press" and press nothing (#581).
#[test]
fn enter_alone_presses_enter_at_the_address_it_was_given() {
    let mut backend = mock();
    let json = json_of(run_cli(
        &["lyrel", "send", "%42", "--enter"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(backend.sent, vec![(42, b"\r".to_vec())]);
    assert_eq!(json["sent"], "", "nothing was typed");
    assert_eq!(json["keys"], serde_json::json!(["enter"]));
    assert_eq!(json["enter"], true);

    // With no address at all it is the caller's own pane, the same as
    // `send --key enter` already was.
    let ctx = Context {
        pane: Some("5".into()),
        ..Context::default()
    };
    let mut backend = mock();
    run_cli(&["lyrel", "send", "--enter"], &ctx, &mut backend);
    assert_eq!(backend.sent, vec![(5, b"\r".to_vec())]);

    // The long way round stays open for a bare id, and means the same
    // thing: an explicit `--key` promotes either spelling.
    let mut backend = mock();
    run_cli(
        &["lyrel", "send", "83", "--key", "enter"],
        &ctx,
        &mut backend,
    );
    assert_eq!(backend.sent, vec![(83, b"\r".to_vec())]);
    let mut backend = mock();
    run_cli(&["lyrel", "send", "%83", "--enter"], &ctx, &mut backend);
    assert_eq!(backend.sent, vec![(83, b"\r".to_vec())]);
}

/// The narrowing has to leave real text alone: `%` followed by a non-digit
/// is nobody's idea of a pane address, and driving vim's ex commands is a
/// documented use of `send`.
#[test]
fn percent_led_text_still_types_into_the_current_pane() {
    let ctx = Context {
        pane: Some("5".into()),
        ..Context::default()
    };
    let mut backend = mock();
    run_cli(&["lyrel", "send", "%s/a/b/"], &ctx, &mut backend);
    assert_eq!(backend.sent, vec![(5, b"%s/a/b/".to_vec())]);

    let mut backend = mock();
    run_cli(&["lyrel", "send", "%!sort", "--enter"], &ctx, &mut backend);
    assert_eq!(
        backend.sent,
        vec![(5, b"%!sort".to_vec()), (5, b"\r".to_vec())]
    );

    // Nothing marks these as addresses, so the narrowing must leave them
    // typing: `3x` has no `%`, and `+5` only looked numeric to
    // `u64::from_str`.
    for text in ["3x", "+5", "50%"] {
        let mut backend = mock();
        run_cli(&["lyrel", "send", text], &ctx, &mut backend);
        assert_eq!(
            backend.sent,
            vec![(5, text.as_bytes().to_vec())],
            "'{text}' is text, not an address"
        );
    }
}

/// The explicit address slot takes the bare id `pane ls --json` prints,
/// not only the `%`-marked spelling (#538).
#[test]
fn a_bare_id_works_as_an_explicit_address() {
    let mut backend = mock();
    run_cli(
        &["lyrel", "send", "83", "--key", "C-c"],
        &Context::default(),
        &mut backend,
    );
    assert_eq!(backend.sent, vec![(83, vec![0x03])]);

    // A lone bare id with nothing to press is the missing-text error, same
    // as a lone `%83` — parseable address, nothing to do with it.
    let mut backend = mock();
    let err = execute(
        cli(&["lyrel", "send", "83"]),
        &Context::default(),
        &mut backend,
    )
    .expect_err("a bare address sends nothing and must say so");
    assert!(err.to_string().contains("needs TEXT"), "{err}");
    assert!(backend.sent.is_empty());
}

#[test]
fn send_outside_a_shell_without_an_address_names_the_fix() {
    let mut backend = mock();
    let err = execute(
        cli(&["lyrel", "send", "echo hi"]),
        &Context::default(),
        &mut backend,
    )
    .unwrap_err();
    assert_eq!(err.to_string(), address::OUTSIDE_SHELL);

    let err = execute(
        cli(&["lyrel", "send", "%1"]),
        &Context::default(),
        &mut backend,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("TEXT"),
        "an address with nothing to send is a mistake, not empty input: {err}"
    );
}

fn procs_with_paste_mode(mode: Option<bool>) -> lyrel_core::daemon::protocol::PaneProcs {
    lyrel_core::daemon::protocol::PaneProcs {
        context: Some(lyrel_core::daemon::protocol::PaneContext {
            local_pty: true,
            bracketed_paste: mode,
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn send_from_file_sends_the_bytes_verbatim_and_never_echoes_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("token");
    std::fs::write(&path, b"s3cr\\et\n\xff").unwrap();
    let mut backend = mock();
    let json = json_of(run_cli(
        &["lyrel", "send", "%1", "--from-file", path.to_str().unwrap()],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(backend.sent, vec![(1, b"s3cr\\et\n\xff".to_vec())]);
    assert!(
        json["sent"].is_null(),
        "the secret must not be echoed: {json}"
    );
    assert!(!json.to_string().contains("s3cr"), "{json}");
    assert_eq!(json["source"], "file");
    assert_eq!(json["bytes"], 9);
    assert!(
        backend.procs_calls.is_empty(),
        "without --paste the pane is not asked anything"
    );

    // Keys still follow the payload, the way they follow typed text.
    let mut backend = mock();
    run_cli(
        &[
            "lyrel",
            "send",
            "%1",
            "--from-file",
            path.to_str().unwrap(),
            "--enter",
        ],
        &Context::default(),
        &mut backend,
    );
    assert_eq!(
        backend.sent,
        vec![(1, b"s3cr\\et\n\xff".to_vec()), (1, b"\r".to_vec())]
    );
}

#[test]
fn send_from_file_takes_its_positional_as_an_address_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("payload");
    std::fs::write(&path, b"x").unwrap();
    let mut backend = mock();
    let err = execute(
        cli(&[
            "lyrel",
            "send",
            "echo",
            "--from-file",
            path.to_str().unwrap(),
        ]),
        &Context::default(),
        &mut backend,
    )
    .unwrap_err();
    let err = format!("{err:#}");
    assert!(err.contains("has to be the pane"), "{err}");
    assert!(backend.sent.is_empty(), "nothing may be typed");

    // With no address it is this pane, as it is for text.
    let ctx = Context {
        pane: Some("%3".into()),
        ..Context::default()
    };
    run_cli(
        &["lyrel", "send", "--from-file", path.to_str().unwrap()],
        &ctx,
        &mut backend,
    );
    assert_eq!(backend.sent, vec![(3, b"x".to_vec())]);
}

#[test]
fn send_paste_frames_only_for_a_pane_that_switched_bracketed_paste_on() {
    let mut backend = mock();
    backend.procs_reply = procs_with_paste_mode(Some(true));
    let json = json_of(run_cli(
        &[
            "lyrel",
            "send",
            "%1",
            "a\r\nb\x1b[201~c",
            "--paste",
            "--enter",
        ],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(
        backend.sent,
        vec![
            (1, b"\x1b[200~a\nb[201~c\x1b[201~".to_vec()),
            (1, b"\r".to_vec())
        ],
        "the GUI's framing: CRLF folded, ESC stripped so the text cannot close the paste"
    );
    assert_eq!(json["bracketed"], true);
    assert_eq!(json["bracketed_paste_mode"], true);

    // Off: brackets would reach the program as keystrokes, so the text goes
    // the way a paste into such a pane always has — a CR per line break.
    let mut backend = mock();
    backend.procs_reply = procs_with_paste_mode(Some(false));
    let json = json_of(run_cli(
        &["lyrel", "send", "%1", "a\nb", "--paste"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(backend.sent, vec![(1, b"a\rb".to_vec())]);
    assert_eq!(json["bracketed"], false);
    assert_eq!(json["bracketed_paste_mode"], false);

    // A server that cannot say is not taken as a yes.
    let mut backend = mock();
    backend.procs_reply = procs_with_paste_mode(None);
    let json = json_of(run_cli(
        &["lyrel", "send", "%1", "a\nb", "--paste"],
        &Context::default(),
        &mut backend,
    ));
    assert_eq!(backend.sent, vec![(1, b"a\rb".to_vec())]);
    assert!(json["bracketed_paste_mode"].is_null(), "{json}");
}

#[test]
fn send_paste_needs_something_to_paste() {
    let mut backend = mock();
    let err = execute(
        cli(&["lyrel", "send", "%1", "--paste", "--enter"]),
        &Context::default(),
        &mut backend,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("--paste needs text"), "{err}");
    assert!(backend.sent.is_empty());
}
