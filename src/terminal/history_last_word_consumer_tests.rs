use super::*;

#[gpui_kit::test]
fn meta_dot_walks_back_through_the_last_words(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            let meta_dot = gpui_kit::Keystroke {
                modifiers: gpui_kit::Modifiers {
                    alt: true,
                    ..Default::default()
                },
                key: ".".to_string(),
                key_char: None,
            };
            seed_history(view, ["git status", "cargo build --release", "echo hello world"]
                .into_iter()
                .map(String::from)
                .collect());
            view.cmd.set("ls ");

            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(view.cmd.text(), "ls world", "newest entry's last word");
            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(view.cmd.text(), "ls --release", "repeat steps one back");
            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(view.cmd.text(), "ls status");
            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(view.cmd.text(), "ls status");
            assert_eq!(view.cmd.cursor(), "ls status".chars().count());
        })
        .unwrap();
}

#[gpui_kit::test]
fn an_intervening_key_restarts_the_last_word_walk(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            let meta_dot = gpui_kit::Keystroke {
                modifiers: gpui_kit::Modifiers {
                    alt: true,
                    ..Default::default()
                },
                key: ".".to_string(),
                key_char: None,
            };
            seed_history(view, ["cargo build --release", "echo hello world"]
                .into_iter()
                .map(String::from)
                .collect());

            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(view.cmd.text(), "world");
            view.handle_editor_key(&key("left"), cx);
            view.handle_editor_key(&key("end"), cx);
            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(
                view.cmd.text(),
                "worldworld",
                "a fresh walk appends rather than replacing the earlier word"
            );
        })
        .unwrap();
}

#[gpui_kit::test]
fn an_intervening_ime_commit_restarts_the_last_word_walk(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);
    window
        .update(cx, |view, _, cx| {
            let meta_dot = gpui_kit::Keystroke {
                modifiers: gpui_kit::Modifiers {
                    alt: true,
                    ..Default::default()
                },
                key: ".".to_string(),
                key_char: None,
            };
            seed_history(view, ["cargo build --release", "echo hello world"]
                .into_iter()
                .map(String::from)
                .collect());

            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(view.cmd.text(), "world");
            view.commit_text("x", cx);
            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(
                view.cmd.text(),
                "worldxworld",
                "the typed char survives; the walk starts over after it"
            );
        })
        .unwrap();
}

#[gpui_kit::test]
fn meta_dot_over_a_selection_records_where_the_word_landed(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            let meta_dot = gpui_kit::Keystroke {
                modifiers: gpui_kit::Modifiers {
                    alt: true,
                    ..Default::default()
                },
                key: ".".to_string(),
                key_char: None,
            };
            seed_history(view, ["cargo build --release", "echo hello world"]
                .into_iter()
                .map(String::from)
                .collect());
            view.cmd.set("ls foo");
            view.cmd.set_cursor(3);
            view.cmd.extend_to(6);

            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(
                view.cmd.text(),
                "ls world",
                "the word replaced the selection"
            );
            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(
                view.cmd.text(),
                "ls --release",
                "the repeat swapped the word, not some other span"
            );
        })
        .unwrap();
}
