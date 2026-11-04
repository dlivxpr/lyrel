use super::*;

#[test]
fn alt_screen_exit_discards_the_boundary_input_before_recording_shell_text() {
    let mut typeahead = Typeahead::new();
    let mut last_blocked = true;
    typeahead.observe(RawInput::Text("stale"), false);

    observe_typeahead_for_owner(
        &mut typeahead,
        &mut last_blocked,
        RawInput::Key {
            key: "c",
            plain: false,
        },
        false,
    );
    assert_eq!(
        typeahead.drain(),
        None,
        "the Ctrl-C crossing TUI exit must not become a tainted shell record"
    );

    observe_typeahead_for_owner(
        &mut typeahead,
        &mut last_blocked,
        RawInput::Text("ls"),
        false,
    );
    assert_eq!(typeahead.drain(), Some("ls".to_string()));
}

#[test]
fn agent_interrupt_discards_typeahead_without_an_alt_screen_transition() {
    let mut typeahead = Typeahead::new();
    let mut last_blocked = false;
    typeahead.observe(RawInput::Text("agent input"), false);
    typeahead.observe(
        RawInput::Key {
            key: "up",
            plain: true,
        },
        false,
    );

    observe_typeahead_for_owner(
        &mut typeahead,
        &mut last_blocked,
        RawInput::Interrupt,
        false,
    );
    assert_eq!(
        typeahead.drain(),
        None,
        "Ctrl-C must cancel a stable non-ALT_SCREEN agent gap"
    );

    observe_typeahead_for_owner(
        &mut typeahead,
        &mut last_blocked,
        RawInput::Text("ls"),
        false,
    );
    assert_eq!(typeahead.drain(), Some("ls".to_string()));
}

#[test]
fn known_agent_input_is_discarded_at_both_ownership_boundaries() {
    let mut typeahead = Typeahead::new();
    let mut last_blocked = false;
    typeahead.observe(RawInput::Text("stale shell gap"), false);

    observe_typeahead_for_owner(
        &mut typeahead,
        &mut last_blocked,
        RawInput::Text("agent input"),
        true,
    );
    observe_typeahead_for_owner(
        &mut typeahead,
        &mut last_blocked,
        RawInput::Key {
            key: "up",
            plain: true,
        },
        true,
    );
    assert_eq!(typeahead.drain(), None);

    observe_typeahead_for_owner(
        &mut typeahead,
        &mut last_blocked,
        RawInput::Text("boundary input"),
        false,
    );
    assert_eq!(typeahead.drain(), None);

    observe_typeahead_for_owner(
        &mut typeahead,
        &mut last_blocked,
        RawInput::Text("ls"),
        false,
    );
    assert_eq!(typeahead.drain(), Some("ls".to_string()));
}

#[test]
fn ctrl_c_and_ctrl_d_discard_foreground_typeahead() {
    let ctrl = gpui_kit::Modifiers {
        control: true,
        ..Default::default()
    };
    assert!(matches!(
        typeahead_boundary("c", &ctrl),
        Some(RawInput::Interrupt)
    ));
    assert!(matches!(
        typeahead_boundary("d", &ctrl),
        Some(RawInput::EndOfInput)
    ));
    assert!(typeahead_boundary("u", &ctrl).is_none());

    let ctrl_alt = gpui_kit::Modifiers {
        control: true,
        alt: true,
        ..Default::default()
    };
    assert!(typeahead_boundary("c", &ctrl_alt).is_none());
    assert!(typeahead_boundary("d", &ctrl_alt).is_none());
}

fn assert_alt_screen_ownership_without_candidates_does_not_wait_for_grid(
    ownership: fn(&PromptInput, &RemoteTerminal) -> bool,
) {
    use crate::daemon::protocol::DaemonMsg;
    use crate::terminal::{TermSize, view::test_stream_pair};
    use std::sync::mpsc;
    use std::time::Duration;

    crate::core::config::pin_test_config_dir();
    let (client, mut daemon) = test_stream_pair();
    let terminal = RemoteTerminal::from_stream(client, TermSize::new(80, 24)).unwrap();
    DaemonMsg::Output(b"\x1b[?1049h".to_vec())
        .encode(&mut daemon)
        .unwrap();
    let mut alt_screen = false;
    for _ in 0..200 {
        alt_screen = terminal.term.lock().mode().contains(Mode::ALT_SCREEN);
        if alt_screen {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(alt_screen, "the real terminal reader must enter ALT_SCREEN");
    assert!(!terminal.shell_vi_mode());
    let input = PromptInput::new(true);

    let grid = terminal.term.clone();
    let (acquired, ready) = mpsc::sync_channel(0);
    let (completed, completion) = mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _held = grid.lock();
        acquired.send(()).unwrap();
        // A quiet holder emits no output or wakeup. The timeout only bounds a
        // broken implementation; success requires ownership to finish first.
        completion.recv_timeout(Duration::from_secs(3)).is_ok()
    });
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    let shell_owns_prompt = ownership(&input, &terminal);
    let _ = completed.send(());
    let completed_before_grid_release = holder.join().unwrap();

    assert!(!shell_owns_prompt, "ALT_SCREEN input belongs to the TUI");
    assert!(
        completed_before_grid_release,
        "ownership without shell vi mode or a handoff must finish before the quiet grid holder releases"
    );
}

#[test]
fn live_shell_ownership_without_candidates_does_not_wait_for_alt_screen_grid() {
    assert_alt_screen_ownership_without_candidates_does_not_wait_for_grid(
        PromptInput::shell_owns_prompt,
    );
}

#[test]
fn live_ctrl_c_ownership_without_candidates_does_not_wait_for_alt_screen_grid() {
    assert_alt_screen_ownership_without_candidates_does_not_wait_for_grid(
        PromptInput::handoff_active,
    );
}
