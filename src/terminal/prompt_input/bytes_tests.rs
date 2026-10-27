use super::*;

#[test]
fn paste_bytes_strips_esc_to_prevent_bracketed_paste_escape() {
    assert_eq!(
        paste_bytes("ls -la", true),
        b"\x1b[200~ls -la\x1b[201~".to_vec()
    );

    let evil = "foo\x1b[201~\nrm -rf ~\n";
    let out = paste_bytes(evil, true);
    let end = b"\x1b[201~";
    let markers = out.windows(end.len()).filter(|w| *w == end).count();
    assert_eq!(markers, 1);
    let inner = &out[b"\x1b[200~".len()..out.len() - end.len()];
    assert!(!inner.contains(&0x1b));
    assert_eq!(inner, b"foo[201~\nrm -rf ~\n");

    assert_eq!(paste_bytes("a\x1b[201~b", false), b"a\x1b[201~b".to_vec());
}

#[test]
fn paste_bytes_normalizes_newlines_to_cr_without_bracketed_paste() {
    assert_eq!(paste_bytes("a\nb\r\nc\n", false), b"a\rb\rc\r".to_vec());
    assert_eq!(
        paste_bytes("a\nb", true),
        b"\x1b[200~a\nb\x1b[201~".to_vec()
    );
}

#[test]
fn paste_bytes_folds_crlf_so_a_windows_clipboard_pastes_like_any_other() {
    assert_eq!(
        paste_bytes("a\r\nb\r\n", true),
        b"\x1b[200~a\nb\n\x1b[201~".to_vec(),
        "CRLF must reach the app as one line break, not two"
    );
    assert_eq!(
        paste_bytes("a\r\nb", true),
        paste_bytes("a\nb", true),
        "a Windows clipboard must paste exactly like a Unix one"
    );
}

#[test]
fn submit_bytes_sends_a_multi_line_command_as_one_bracketed_paste() {
    assert_eq!(
        submit_bytes("echo a\necho b\necho c", true, false),
        b"\x1b[200~echo a\necho b\necho c\x1b[201~\r".to_vec()
    );
    let out = submit_bytes("a\nb\nc\nd", true, false);
    assert_eq!(out.iter().filter(|&&b| b == b'\r').count(), 1);
}

#[test]
fn submit_bytes_types_a_plain_single_line_instead_of_pasting_it() {
    // #660: inside a bracketed paste fish never runs `expand-abbr`, so an
    // abbreviation with arguments reached the shell verbatim and `j build`
    // died as "command not found". Raw bytes are what typing produces, and
    // that is the delivery every input-time expansion -- fish
    // abbreviations, zsh magic-space -- is bound to.
    assert_eq!(submit_bytes("j build", true, false), b"j build\r".to_vec());
    assert_eq!(
        submit_bytes("echo 'a  b' | cat", true, false),
        b"echo 'a  b' | cat\r".to_vec()
    );
    // Non-ASCII is text, not a control character.
    assert_eq!(
        submit_bytes("echo 中文", true, false),
        "echo 中文\r".as_bytes()
    );

    // A control character would be acted on by the shell's binding table
    // rather than inserted -- a Tab completes, a ^U kills the line -- so
    // those keep the paste framing.
    assert_eq!(
        submit_bytes("echo a\tb", true, false),
        b"\x1b[200~echo a\tb\x1b[201~\r".to_vec()
    );
    assert_eq!(
        submit_bytes("echo a\x15b", true, false),
        b"\x1b[200~echo a\x15b\x1b[201~\r".to_vec()
    );

    // Past the length bound the flat cost of a paste wins over the shell's
    // per-byte redraw. The bound is a latency ceiling, not a knee in the
    // curve -- see `types_cleanly`.
    let at_bound = format!("echo {}", "y".repeat(507));
    assert_eq!(at_bound.len(), 512);
    assert_eq!(
        submit_bytes(&at_bound, true, false),
        [at_bound.as_bytes(), b"\r"].concat()
    );
    let over_bound = format!("echo {}", "y".repeat(508));
    assert_eq!(over_bound.len(), 513);
    assert_eq!(
        submit_bytes(&over_bound, true, false),
        [b"\x1b[200~", over_bound.as_bytes(), b"\x1b[201~\r"].concat()
    );
}

#[test]
fn submit_bytes_keeps_pasted_text_inside_a_bracketed_paste() {
    // A paste's contract is that what went in is what runs. The typed path
    // hands the line to the shell's binding table, and a printable key can
    // be bound there: a fish user with `abbr -a l 'ls -la'` who pastes
    // `l /tmp` out of their notes must not run `ls -la /tmp`, and a zsh
    // user with `bindkey . rationalise-dot` must not have a pasted
    // `echo a...b` become `echo a../..b`. Both were reproduced on a pty.
    assert_eq!(
        submit_bytes("l /tmp", true, true),
        b"\x1b[200~l /tmp\x1b[201~\r".to_vec()
    );
    assert_eq!(
        submit_bytes("echo a...b", true, true),
        b"\x1b[200~echo a...b\x1b[201~\r".to_vec()
    );
    // Same text typed rather than pasted takes the typed path -- that is
    // the whole point, and it is the same delivery the user's own shell
    // prompt would have given it.
    assert_eq!(submit_bytes("l /tmp", true, false), b"l /tmp\r".to_vec());
    // A shell with no bracketed paste has no framing to fall back on, so
    // the mark changes nothing there.
    assert_eq!(submit_bytes("l /tmp", false, true), b"l /tmp\r".to_vec());
    // An empty buffer still skips the markers, pasted or not.
    assert_eq!(submit_bytes("", true, true), b"\r".to_vec());
}

#[test]
fn submit_bytes_falls_back_to_per_line_cr_without_bracketed_paste() {
    assert_eq!(submit_bytes("a\nb", false, false), b"a\rb\r".to_vec());
    assert_eq!(submit_bytes("a\r\nb", false, false), b"a\rb\r".to_vec());
}

#[test]
fn submit_bytes_normalizes_line_breaks_inside_the_paste() {
    assert_eq!(
        submit_bytes("a\r\nb", true, false),
        b"\x1b[200~a\nb\x1b[201~\r".to_vec()
    );
    assert_eq!(
        submit_bytes("a\rb", true, false),
        b"\x1b[200~a\nb\x1b[201~\r".to_vec()
    );
    assert_eq!(submit_bytes("a\rb", false, false), b"a\rb\r".to_vec());
}

#[test]
fn submit_bytes_strips_esc_and_skips_markers_on_an_empty_line() {
    let out = submit_bytes("foo\x1b[201~\nrm -rf ~", true, false);
    let end = b"\x1b[201~";
    assert_eq!(out.windows(end.len()).filter(|w| *w == end).count(), 1);
    assert_eq!(out, b"\x1b[200~foo[201~\nrm -rf ~\x1b[201~\r".to_vec());
    assert_eq!(submit_bytes("a\x1bb", false, false), b"ab\r".to_vec());
    // The same smuggling attempt on the typed path is just literal text at
    // the prompt: there is no paste to break out of, and no ESC survives to
    // reach the line editor as a command.
    assert_eq!(
        submit_bytes("foo\x1b[201~; rm -rf ~", true, false),
        b"foo[201~; rm -rf ~\r".to_vec()
    );

    assert_eq!(submit_bytes("", true, false), b"\r".to_vec());
}

