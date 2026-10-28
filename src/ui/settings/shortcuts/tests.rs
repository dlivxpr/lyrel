use crate::core::config::Config;
use crate::ui::settings::SettingsSection;
use crate::ui::settings::test_window::harness;
use crate::ui::settings_window::SettingsWindow;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};

fn begin_capture(app: &Entity<SettingsWindow>, vcx: &mut VisualTestContext, action: &str) {
    let action = action.to_string();
    app.update_in(vcx, |app, window, cx| {
        app.select_settings_section(SettingsSection::Keybindings, cx);
        app.start_recording_key(action, window, cx);
    });
}

/// Waits for `action`'s entry in config to read `expected` — compared as the
/// JSON the file gets, since that is what the reader of `config.json` sees.
fn wait_for_binding(vcx: &mut VisualTestContext, action: &str, expected: serde_json::Value) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        vcx.background_executor.run_until_parked();
        let got = vcx.update(|_, cx| {
            serde_json::to_value(cx.global::<Config>().keybindings.get(action))
                .expect("a binding serializes")
        });
        if got == expected {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "binding for {action} never became {expected:?} (last {got:?})"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Wait for conflict feedback without accepting the candidate.
fn wait_for_conflict(app: &Entity<SettingsWindow>, vcx: &mut VisualTestContext, owner: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        vcx.background_executor.run_until_parked();
        let other = app.update_in(vcx, |app, _, _| {
            app.active_settings()
                .and_then(|s| s.shortcuts.conflict().map(|(_, _, other)| other.to_string()))
        });
        if let Some(other) = other {
            assert_eq!(
                other, owner,
                "the conflict names the action that has the chord"
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "recording a taken chord never asked about it"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn replace_conflict(app: &Entity<SettingsWindow>, vcx: &mut VisualTestContext, owner: &str) {
    wait_for_conflict(app, vcx, owner);
    app.update_in(vcx, |app, _, cx| app.resolve_keybinding_conflict(true, cx));
}

fn pump_for(vcx: &mut VisualTestContext, duration: std::time::Duration) {
    let until = std::time::Instant::now() + duration;
    while std::time::Instant::now() < until {
        vcx.background_executor.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    vcx.background_executor.run_until_parked();
}

#[gpui_kit::test]
fn recording_a_shortcut_writes_the_override_and_ends_capture(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    begin_capture(&app, &mut vcx, "NewTab");
    vcx.simulate_keystrokes("secondary-shift-f9");
    // A list, because recording a shortcut on the Settings page *sets* it:
    // the row showed one chord and now shows another. A bare string in
    // config adds a chord beside the default (#868), which is not what
    // the person at the row just did.
    wait_for_binding(
        &mut vcx,
        "NewTab",
        serde_json::json!(["secondary-shift-f9"]),
    );

    let recording = app.update_in(&mut vcx, |app, _, _| {
        app.active_settings().map(|s| s.shortcuts.is_recording())
    });
    assert_eq!(
        recording,
        Some(false),
        "capture should end after committing"
    );
}

#[gpui_kit::test]
fn recording_a_two_chord_sequence_writes_the_full_spec(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    begin_capture(&app, &mut vcx, "CloseActiveTab");
    vcx.simulate_keystrokes("secondary-b");
    vcx.simulate_keystrokes("x");
    wait_for_binding(
        &mut vcx,
        "CloseActiveTab",
        serde_json::json!(["secondary-b x"]),
    );
}

#[gpui_kit::test]
fn recording_a_chord_another_action_also_has_takes_only_that_chord(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    vcx.update(|_, cx| {
        cx.global_mut::<Config>().keybindings = serde_json::from_value(serde_json::json!({
            "NextTab": ["ctrl-tab", "secondary-alt-n"],
        }))
        .expect("the binding loads");
        crate::ui::keymap::rebind(cx);
    });
    begin_capture(&app, &mut vcx, "NewTab");
    vcx.simulate_keystrokes("secondary-alt-n");
    replace_conflict(&app, &mut vcx, "NextTab");
    wait_for_binding(&mut vcx, "NewTab", serde_json::json!(["secondary-alt-n"]));
    // Emptying the other action was right when an action had one chord.
    // With two, it would take Ctrl+Tab away as well, for a keystroke that
    // was never on it.
    wait_for_binding(&mut vcx, "NextTab", serde_json::json!(["ctrl-tab"]));
}

#[gpui_kit::test]
fn recording_an_extra_default_chord_displaces_its_owner(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    begin_capture(&app, &mut vcx, "NewTab");
    vcx.simulate_keystrokes("alt-enter");
    // The row names the action that would lose the chord before it does.
    replace_conflict(&app, &mut vcx, "InsertNewline");
    wait_for_binding(&mut vcx, "NewTab", serde_json::json!(["alt-enter"]));
    wait_for_binding(&mut vcx, "InsertNewline", serde_json::json!([]));
}

/// #901, the half that happens in the UI: Alt+1…9 belongs to vim, and the
/// only gesture in the app that looks like "take this shortcut away" used
/// to *reset* the row instead — a no-op on a row nobody had overridden,
/// so the default appeared to restore itself however many times it was
/// pressed.
#[gpui_kit::test]
fn backspace_on_a_row_unbinds_the_action_rather_than_restoring_its_default(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx) = harness(cx);
    let shipped = vcx
        .update(|_, cx| crate::ui::keymap::effective_key("ActivateTab1", cx))
        .expect("Go to Tab 1 ships with a chord");

    begin_capture(&app, &mut vcx, "ActivateTab1");
    vcx.simulate_keystrokes("backspace");
    wait_for_binding(&mut vcx, "ActivateTab1", serde_json::json!([]));

    vcx.update(|_, cx| {
        assert_eq!(
            crate::ui::keymap::effective_key("ActivateTab1", cx),
            None,
            "the row has no chord left to show"
        );
        let typed = [
            gpui_kit::Keystroke::parse_with_os(&shipped, cx.platform_info().os)
                .expect("the chord parses"),
        ];
        let context = [gpui_kit::KeyContext::parse("Terminal").expect("the context parses")];
        assert!(
            cx.key_bindings()
                .borrow()
                .bindings_for_input(&typed, &context)
                .0
                .is_empty(),
            "{shipped} must reach the terminal now, not the tab switcher"
        );
    });

    // Reversible, and by the button that is already on the row: an
    // overridden action — unbound counts — shows **Reset**.
    app.update_in(&mut vcx, |app, _, cx| {
        app.reset_keybinding("ActivateTab1".to_string(), cx)
    });
    vcx.update(|_, cx| {
        assert_eq!(
            crate::ui::keymap::effective_key("ActivateTab1", cx).as_deref(),
            Some(shipped.as_str()),
            "Reset is the way back to the shipped chord"
        );
    });
}

#[gpui_kit::test]
fn escape_cancels_capture_without_writing(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.select_settings_section(SettingsSection::Keybindings, cx);
        app.start_recording_key("NewTab".to_string(), window, cx);
    });
    vcx.simulate_keystrokes("escape");
    vcx.background_executor.run_until_parked();

    let stored = vcx.update(|_, cx| cx.global::<Config>().keybindings.contains_key("NewTab"));
    assert!(!stored, "Esc must not persist a binding");
    let recording = app.update_in(&mut vcx, |app, _, _| {
        app.active_settings().map(|s| s.shortcuts.is_recording())
    });
    assert_eq!(recording, Some(false));
}

#[gpui_kit::test]
fn escape_after_a_chord_retires_its_pending_commit(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    begin_capture(&app, &mut vcx, "NewTab");
    vcx.simulate_keystrokes("secondary-shift-f9");
    vcx.simulate_keystrokes("escape");
    pump_for(&mut vcx, std::time::Duration::from_millis(800));
    vcx.update(|_, cx| assert!(!cx.global::<Config>().keybindings.contains_key("NewTab")));
    assert!(!app.read_with(&vcx, |app, _| app.recording_shortcut()));
}

#[gpui_kit::test]
fn backspace_removes_the_last_chord_without_unbinding_until_pressed_again(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    begin_capture(&app, &mut vcx, "NewTab");
    vcx.simulate_keystrokes("secondary-shift-f9");
    vcx.simulate_keystrokes("x");
    vcx.simulate_keystrokes("backspace");
    wait_for_binding(&mut vcx, "NewTab", serde_json::json!(["secondary-shift-f9"]));
    begin_capture(&app, &mut vcx, "NewTab");
    vcx.simulate_keystrokes("secondary-shift-f10");
    vcx.simulate_keystrokes("backspace");
    pump_for(&mut vcx, std::time::Duration::from_millis(800));
    assert!(app.read_with(&vcx, |app, _| app.recording_shortcut()));
    wait_for_binding(&mut vcx, "NewTab", serde_json::json!(["secondary-shift-f9"]));
    vcx.simulate_keystrokes("delete");
    wait_for_binding(&mut vcx, "NewTab", serde_json::json!([]));
}

#[gpui_kit::test]
fn closing_and_reopening_settings_drops_capture_and_rejects_the_old_timer(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    begin_capture(&app, &mut vcx, "NewTab");
    vcx.simulate_keystrokes("secondary-shift-f9");
    app.update_in(&mut vcx, |app, window, cx| app.close_settings(window, cx));
    assert!(!app.read_with(&vcx, |app, _| app.recording_shortcut()));
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Keybindings, window, cx);
        app.start_recording_key("CloseActiveTab".to_string(), window, cx);
    });
    vcx.simulate_keystrokes("secondary-shift-f10");
    wait_for_binding(&mut vcx, "CloseActiveTab", serde_json::json!(["secondary-shift-f10"]));
    pump_for(&mut vcx, std::time::Duration::from_millis(800));
    vcx.update(|_, cx| assert!(!cx.global::<Config>().keybindings.contains_key("NewTab")));
}

// Delivering timers directly at the existing owner interface makes a matching
// generation from another session deterministic, without wall-clock races.
#[gpui_kit::test]
fn rebuilt_session_rejects_a_matching_generation_from_previous_session(cx: &mut TestAppContext) {
    use super::editor::{Capture, Commit};
    let (app, mut vcx) = harness(cx);
    begin_capture(&app, &mut vcx, "NewTab");
    let stale = app.update_in(&mut vcx, |app, _, cx| {
        let key = gpui_kit::Keystroke::parse_with_os("secondary-shift-f9", cx.platform_info().os)
            .expect("the chord parses");
        match app.active_settings_mut().unwrap().shortcuts.capture(&key) {
            Capture::Schedule(token) => token,
            _ => panic!("capturing a chord schedules its commit"),
        }
    });
    app.update_in(&mut vcx, |app, window, cx| {
        app.close_settings(window, cx);
        app.open_settings_section(SettingsSection::Keybindings, window, cx);
        app.start_recording_key("CloseActiveTab".to_string(), window, cx);
        let key = gpui_kit::Keystroke::parse_with_os("secondary-shift-f10", cx.platform_info().os)
            .expect("the chord parses");
        let owner = &mut app.active_settings_mut().unwrap().shortcuts;
        let current = match owner.capture(&key) {
            Capture::Schedule(token) => token,
            _ => panic!("capturing a chord schedules its commit"),
        };
        assert!(matches!(owner.commit(stale, cx), Commit::Ignored));
        let Commit::Edit(edit) = owner.commit(current, cx) else {
            panic!("the current recording remains committable");
        };
        app.apply_shortcut_change(Some(edit), cx);
    });
    wait_for_binding(&mut vcx, "CloseActiveTab", serde_json::json!(["secondary-shift-f10"]));
    vcx.update(|_, cx| assert!(!cx.global::<Config>().keybindings.contains_key("NewTab")));
}

#[gpui_kit::test]
fn conflict_cancel_and_replace_preserve_the_actual_workbench_action_target(cx: &mut TestAppContext) {
    let (workspace, mut vcx) = crate::ui::app::test_window::harness(cx);
    vcx.update(|_, cx| {
        cx.global_mut::<Config>().keybindings = serde_json::from_value(serde_json::json!({
            "ToggleLeftPanel": ["secondary-shift-f9"],
        })).expect("the binding loads");
        crate::ui::keymap::rebind(cx);
    });
    let app = crate::ui::settings::test_window::open(&workspace, &mut vcx);
    begin_capture(&app, &mut vcx, "ToggleRightPanel");
    vcx.simulate_keystrokes("secondary-shift-f9");
    wait_for_conflict(&app, &mut vcx, "ToggleLeftPanel");
    app.update_in(&mut vcx, |app, _, cx| app.resolve_keybinding_conflict(false, cx));
    app.update_in(&mut vcx, |app, window, cx| app.close_settings(window, cx));
    vcx.run_until_parked();
    workspace.update_in(&mut vcx, |app, window, cx| app.focus_active(window, cx));
    vcx.run_until_parked();
    let before = workspace.read_with(&vcx, |app, _| (app.sidebar_collapsed, app.right_panel_visible));
    vcx.simulate_keystrokes("secondary-shift-f9");
    vcx.run_until_parked();
    assert_eq!(workspace.read_with(&vcx, |app, _| (app.sidebar_collapsed, app.right_panel_visible)), (!before.0, before.1));

    let app = crate::ui::settings::test_window::open(&workspace, &mut vcx);
    begin_capture(&app, &mut vcx, "ToggleRightPanel");
    vcx.simulate_keystrokes("secondary-shift-f9");
    replace_conflict(&app, &mut vcx, "ToggleLeftPanel");
    app.update_in(&mut vcx, |app, window, cx| app.close_settings(window, cx));
    vcx.run_until_parked();
    workspace.update_in(&mut vcx, |app, window, cx| app.focus_active(window, cx));
    vcx.run_until_parked();
    let before = workspace.read_with(&vcx, |app, _| (app.sidebar_collapsed, app.right_panel_visible));
    vcx.simulate_keystrokes("secondary-shift-f9");
    vcx.run_until_parked();
    assert_eq!(workspace.read_with(&vcx, |app, _| (app.sidebar_collapsed, app.right_panel_visible)), (before.0, !before.1));
}

#[gpui_kit::test]
fn escape_dismisses_a_conflict_without_installing_or_closing_settings(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    begin_capture(&app, &mut vcx, "NewTab");
    vcx.simulate_keystrokes("alt-enter");
    wait_for_conflict(&app, &mut vcx, "InsertNewline");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    app.read_with(&vcx, |app, _| {
        assert!(app.has_settings());
        assert!(app.active_settings().unwrap().shortcuts.conflict().is_none());
    });
    vcx.update(|_, cx| {
        assert!(!cx.global::<Config>().keybindings.contains_key("NewTab"));
        assert!(!cx.global::<Config>().keybindings.contains_key("InsertNewline"));
    });
}