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
