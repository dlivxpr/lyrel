use super::*;

/// Seeds a pane's history for the ghost tests: `(command, dirs, exit)`,
/// oldest first, with `cwd` as the directory the pane is in.
fn seed_ghost_history(
    view: &mut TerminalView,
    entries: &[(&str, &[&str], Option<i32>)],
    cwd: Option<&str>,
    frequent: Option<(&str, u32)>,
) {
    let mut history = history_fixture(entries.iter().map(|(cmd, _, _)| cmd.to_string()).collect());
    for (cmd, dirs, exit) in entries {
        history.cwds.insert(
            cmd.to_string(),
            dirs.iter().map(|d| d.to_string()).collect(),
        );
        history.meta.insert(
            cmd.to_string(),
            crate::terminal::history::EntryMeta {
                ts: None,
                exit: *exit,
            },
        );
    }
    if let Some((line, count)) = frequent {
        history.counts.insert(line.to_string(), count);
    }
    view.history = TerminalHistory::from_loaded(history);
    view.history.rank_for_cwd(cwd.map(std::path::Path::new));
}

#[gpui_kit::test]
fn ghost_suggests_the_command_just_run_over_a_frequent_one(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, _| {
            seed_ghost_history(
                view,
                &[
                    ("git checkout main", &["/work/proj"], Some(0)),
                    ("git commit -m x", &["/work/proj"], Some(0)),
                ],
                Some("/work/proj"),
                Some(("git checkout main", 20)),
            );
            view.input.editor_mut().set("git c");
            assert_eq!(
                view.ghost_suggestion().as_deref(),
                Some("git commit -m x"),
                "recency decides the ghost, however often the older one ran"
            );
        })
        .unwrap();
}

#[gpui_kit::test]
fn ghost_prefers_this_directory_then_falls_back_to_anywhere(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, _| {
            seed_ghost_history(
                view,
                &[
                    ("cargo build", &["/work/proj"], None),
                    ("cargo test --all", &["/elsewhere"], None),
                    ("npm run dev", &["/elsewhere"], None),
                ],
                Some("/work/proj"),
                None,
            );
            view.input.editor_mut().set("cargo ");
            assert_eq!(
                view.ghost_suggestion().as_deref(),
                Some("cargo build"),
                "an older match run here beats a newer one run elsewhere"
            );
            view.input.editor_mut().set("npm ");
            assert_eq!(
                view.ghost_suggestion().as_deref(),
                Some("npm run dev"),
                "with nothing run here, the newest match anywhere is offered"
            );
            view.input.editor_mut().set("python ");
            assert_eq!(view.ghost_suggestion(), None);
        })
        .unwrap();
}

#[gpui_kit::test]
fn ghost_skips_commands_that_failed(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, _| {
            seed_ghost_history(
                view,
                &[
                    ("make test", &["/work/proj"], Some(0)),
                    ("make tset", &["/work/proj"], Some(2)),
                    ("make lint", &["/work/proj"], Some(1)),
                ],
                Some("/work/proj"),
                None,
            );
            view.input.editor_mut().set("make t");
            assert_eq!(
                view.ghost_suggestion().as_deref(),
                Some("make test"),
                "the newer typo exited 2, so the older success is offered"
            );
            view.input.editor_mut().set("make l");
            assert_eq!(
                view.ghost_suggestion(),
                None,
                "a command whose only run failed is never offered"
            );
        })
        .unwrap();
}

#[gpui_kit::test]
fn ghost_names_what_up_arrow_recalls(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_ghost_history(
                view,
                &[
                    ("git status", &[], None),
                    ("git log --oneline", &[], None),
                    ("ls", &[], None),
                    ("git push", &[], None),
                    ("echo hi", &[], None),
                ],
                None,
                Some(("git status", 50)),
            );
            for typed in ["git", "git l", "e", "l"] {
                view.history.reset_navigation();
                view.input.editor_mut().set(typed);
                let ghost = view.ghost_suggestion();
                view.handle_editor_key(&key("up"), cx);
                assert_eq!(
                    ghost.as_deref(),
                    Some(view.input.editor().text().as_str()),
                    "ghost and ↑ disagree on {typed:?}"
                );
            }
        })
        .unwrap();
}

#[gpui_kit::test]
fn submitting_a_command_again_moves_it_to_the_newest_entry(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);

    window
        .update(cx, |view, _, cx| {
            seed_history(view, ["git status", "ls", "make"]
                .into_iter()
                .map(String::from)
                .collect());
            view.input.editor_mut().set("git status");
            view.submit_command(cx);
            assert_eq!(
                view.history.entries(),
                ["ls", "make", "git status"],
                "one entry per command, at its latest run, as a reload leaves it"
            );

            view.input.editor_mut().set("");
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(view.input.editor().text(), "git status");
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(view.input.editor().text(), "make", "↑ never lands on a command twice");
        })
        .unwrap();
}
