use super::*;
use std::io::Write as _;
use std::os::unix::net::UnixStream;

#[test]
fn stale_mode_resets_target_only_the_dirty_bits() {
    let clean = Mode::SHOW_CURSOR | Mode::LINE_WRAP | Mode::BRACKETED_PASTE;
    assert!(stale_mode_resets(clean).is_empty());

    let hidden = Mode::LINE_WRAP;
    assert_eq!(stale_mode_resets(hidden), b"\x1b[?25h");

    let residue = Mode::ALT_SCREEN | Mode::MOUSE_DRAG | Mode::SGR_MOUSE;
    let seq = stale_mode_resets(residue);
    let text = String::from_utf8_lossy(&seq).into_owned();
    assert!(text.starts_with("\x1b[?1049l"));
    assert!(text.contains("\x1b[?25h"));
    assert!(text.contains("\x1b[?1002l"));
    assert!(text.contains("\x1b[?1006l"));
    assert!(text.ends_with("\x1b[=0;1u"));

    let kitty = Mode::SHOW_CURSOR | Mode::DISAMBIGUATE_ESC_CODES;
    assert_eq!(stale_mode_resets(kitty), b"\x1b[=0;1u");
}

#[test]
fn prompt_report_scrubs_stale_tui_modes() {
    let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
    let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

    DaemonMsg::Output(b"\x1b[?1049h\x1b[?25l\x1b[?1002h\x1b[?1006h".to_vec())
        .encode(&mut daemon_side)
        .unwrap();
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(255),
    }
    .encode(&mut daemon_side)
    .unwrap();
    daemon_side.flush().unwrap();

    let mut mode = Mode::NONE;
    for _ in 0..200 {
        mode = term.term.lock().mode();
        let scrubbed = !mode.contains(Mode::ALT_SCREEN)
            && mode.contains(Mode::SHOW_CURSOR)
            && !mode.intersects(Mode::MOUSE_MODE)
            && !mode.contains(Mode::SGR_MOUSE);
        if scrubbed && term.at_prompt() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        !mode.contains(Mode::ALT_SCREEN),
        "the prompt report must pull the grid off the stranded alt screen"
    );
    assert!(
        mode.contains(Mode::SHOW_CURSOR),
        "the prompt report must re-show the DECTCEM-hidden cursor"
    );
    assert!(
        !mode.intersects(Mode::MOUSE_MODE) && !mode.contains(Mode::SGR_MOUSE),
        "the prompt report must disable stale mouse reporting"
    );
}
