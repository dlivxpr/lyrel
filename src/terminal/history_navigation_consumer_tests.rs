use super::*;

#[gpui_kit::test]
fn history_recall_snaps_the_viewport_back_to_the_prompt(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(view, vec!["echo hello".to_string()]);
            scroll_into_history(view, 10);
            view.scroll_frac = 0.5;

            view.handle_editor_key(&key("up"), cx);

            assert_eq!(view.cmd.text(), "echo hello", "↑ recalled the entry");
            assert_eq!(display_offset(view), 0, "and the viewport followed it down");
            assert_eq!(view.scroll_frac, 0., "the sub-line remainder reset too");
        })
        .unwrap();
}

#[gpui_kit::test]
fn ctrl_p_and_ctrl_n_walk_the_history(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(view, ["git status", "cargo build", "echo hello"]
                .into_iter()
                .map(String::from)
                .collect());

            view.handle_editor_key(&key("ctrl-p"), cx);
            assert_eq!(view.cmd.text(), "echo hello");
            view.handle_editor_key(&key("ctrl-p"), cx);
            assert_eq!(view.cmd.text(), "cargo build");
            view.handle_editor_key(&key("ctrl-n"), cx);
            assert_eq!(view.cmd.text(), "echo hello");
            view.handle_editor_key(&key("ctrl-n"), cx);
            assert_eq!(view.cmd.text(), "");
        })
        .unwrap();
}

#[gpui_kit::test]
fn up_and_ctrl_p_recall_the_last_command_matching_the_typed_prefix(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(view, ["git status", "echo hello", "git log", "ls"]
                .into_iter()
                .map(String::from)
                .collect());
            view.cmd.set("git");

            view.handle_editor_key(&key("up"), cx);
            assert_eq!(
                view.cmd.text(),
                "git log",
                "↑ skips ls and echo hello, which do not start with git"
            );
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(view.cmd.text(), "git status");
            view.handle_editor_key(&key("down"), cx);
            assert_eq!(view.cmd.text(), "git log");
            view.handle_editor_key(&key("down"), cx);
            assert_eq!(
                view.cmd.text(),
                "git",
                "↓ past the newest match restores the prefix"
            );

            view.cmd.set("echo");
            view.handle_editor_key(&key("ctrl-p"), cx);
            assert_eq!(view.cmd.text(), "echo hello");
        })
        .unwrap();
}

#[gpui_kit::test]
fn history_search_takes_its_prefix_from_the_text_left_of_the_cursor(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(view, ["git status", "echo hello", "git log", "ls"]
                .into_iter()
                .map(String::from)
                .collect());

            // Ctrl+A parks the cursor at the start. Nothing sits left of
            // it, so UP walks the whole list instead of filtering on a line
            // the user is about to edit in front of.
            view.cmd.set_with_cursor("git", 0);
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(view.cmd.text(), "ls", "an empty prefix filters nothing");

            // A cursor parked mid-line searches on what is behind it, and
            // DOWN past the newest match restores the whole line, the part
            // right of the cursor included.
            view.history.reset_navigation();
            view.cmd.set_with_cursor("git hello", 4);
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(
                view.cmd.text(),
                "git log",
                "the prefix is `git `, not the whole `git hello`"
            );
            view.handle_editor_key(&key("down"), cx);
            assert_eq!(view.cmd.text(), "git hello");
        })
        .unwrap();
}

#[gpui_kit::test]
fn ctrl_e_accepts_a_ghost_suggestion_at_the_end(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);

    window
        .update(cx, |view, _, cx| {
            assert!(view.input_active(), "the local editor owns a fresh prompt");
            seed_history(view, vec!["git log --oneline".to_string()]);
            view.cmd.set("git l");

            view.handle_editor_key(&key("ctrl-e"), cx);

            assert_eq!(view.cmd.text(), "git log --oneline");
            assert!(
                view.editor_handoff.is_none(),
                "accepting a local suggestion must not hand input to the shell"
            );
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "the local editor must consume Ctrl+E instead of forwarding 0x05"
    );
}

#[gpui_kit::test]
fn ctrl_e_only_moves_to_the_end_when_no_ghost_is_visible(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(view, vec!["git log --oneline".to_string()]);
            view.cmd.set_with_cursor("git l", 2);

            view.handle_editor_key(&key("ctrl-e"), cx);

            assert_eq!(view.cmd.text(), "git l");
            assert_eq!(view.cmd.cursor(), view.cmd.len());
        })
        .unwrap();
}
