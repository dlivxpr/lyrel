use super::*;
use super::completion_consumer_fixture::*;

#[gpui_kit::test]
fn generator_results_merge_filter_select_and_accept_the_live_word(cx: &mut TestAppContext) {
    let cwd = tempfile::tempdir().unwrap();
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    window.update(cx, |view, window, cx| {
        view.cmd.set("git checkout ma");
        let (generation, count) = generator_request(view, cwd.path());
        let context = view.completion_context();
        view.completion.merge(generation, context, &view.cmd, parsed(&["main", "mainline", "feature"]));
        for _ in 1..count {
            let context = view.completion_context();
            view.completion.merge(generation, context, &view.cmd, Vec::new());
        }
        assert_eq!(shown(view), ["main", "mainline"]);
        assert!(view.render_completion_menu(cx).is_some());
        view.handle_editor_key(&key("down"), cx);
        view.handle_editor_key(&key("enter"), cx);
        assert_eq!(view.cmd.text(), "git checkout mainline ");
        type_char(view, "x", window, cx);
        assert_eq!(view.cmd.text(), "git checkout mainline x");
        let context = view.completion_context();
        view.completion.merge(generation, context, &view.cmd, parsed(&["malicious-late"]));
        assert_eq!(view.cmd.text(), "git checkout mainline x");
        assert!(view.render_completion_menu(cx).is_none());
    }).unwrap();
    assert_eq!(next_input_until_timeout(&mut daemon), None);
}

#[gpui_kit::test]
fn unmatched_generators_close_the_menu_only_after_all_answers(cx: &mut TestAppContext) {
    let cwd = tempfile::tempdir().unwrap();
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    window.update(cx, |view, _, cx| {
        for answers in [Vec::new(), parsed(&["main", "release"])] {
            view.cmd.set("git checkout zzqqx-no-match");
            let (generation, count) = generator_request(view, cwd.path());
            for answer in 0..count {
                let context = view.completion_context();
                view.completion.merge(generation, context, &view.cmd, answers.clone());
                if answer + 1 < count { assert!(shown(view).is_empty()); }
            }
            assert!(view.render_completion_menu(cx).is_none());
            assert_eq!(view.cmd.text(), "git checkout zzqqx-no-match");
        }
    }).unwrap();
}

#[gpui_kit::test]
fn canceled_generators_never_reopen_or_replace_a_new_menu(cx: &mut TestAppContext) {
    let cwd = tempfile::tempdir().unwrap();
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    window.update(cx, |view, _, cx| {
        view.cmd.set("git checkout ");
        let (stale, _) = generator_request(view, cwd.path());
        view.handle_editor_key(&key("escape"), cx);
        let context = view.completion_context();
        view.completion.merge(stale, context, &view.cmd, parsed(&["old"]));
        assert!(view.render_completion_menu(cx).is_none());
        view.cmd.set("zzqqx /work/");
        let current = remote_request(view);
        view.remote_path_results(current, Ok(entries(&["one", "two"], false)), cx);
        let context = view.completion_context();
        view.completion.merge(stale, context, &view.cmd, parsed(&["old"]));
        assert_eq!(shown(view), ["/work/one", "/work/two"]);
        view.handle_editor_key(&key("enter"), cx);
        assert_eq!(view.cmd.text(), "zzqqx /work/one ");
    }).unwrap();
}

#[gpui_kit::test]
fn remote_listing_failure_is_not_successful_empty_and_dismisses_on_a_key(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    window.update(cx, |view, window, cx| {
        view.cmd.set("zzqqx /work/");
        let request = remote_request(view);
        assert_eq!(view.remote_completion_notice_text().as_deref(), Some("listing remote…"));
        view.remote_path_results(request, Err(std::io::Error::other("boom")), cx);
        assert!(view.remote_completion_notice_text().unwrap().contains("boom"));
        let event = KeyDownEvent { keystroke: key("left"), is_held: false, prefer_character_input: false };
        view.on_key_down(&event, window, cx);
        assert!(view.remote_completion_notice_text().is_none());
    }).unwrap();
    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"zzqqx /work/".to_vec()));
    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"\t".to_vec()));
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    window.update(cx, |view, _, cx| {
        view.cmd.set("zzqqx /empty/");
        let request = remote_request(view);
        view.remote_path_results(request, Ok(Vec::new()), cx);
        assert!(view.remote_completion_notice_text().is_none());
        assert_eq!(view.cmd.text(), "");
    }).unwrap();
    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"zzqqx /empty/".to_vec()));
    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"\t".to_vec()));
}

#[gpui_kit::test]
fn earlier_input_cancellation_and_host_results_cannot_replace_current_suggestions(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    window.update(cx, |view, window, cx| {
        for change in 0..3 {
            view.cmd.set("zzqqx /work/");
            let stale = remote_request(view);
            match change {
                0 => { type_char(view, "x", window, cx); view.handle_editor_key(&key("backspace"), cx); },
                1 => view.handle_editor_key(&key("escape"), cx),
                _ => view.host_id = crate::ui::host_ops::HostId::from_connection_key("ssh-direct:other-completion-box:22"),
            }
            let current = remote_request(view);
            view.remote_path_results(stale, Err(std::io::Error::other("stale failure")), cx);
            assert_eq!(view.remote_completion_notice_text().as_deref(), Some("listing remote…"));
            view.remote_path_results(current, Ok(entries(&["one", "two"], false)), cx);
            assert_eq!(shown(view), ["/work/one", "/work/two"]);
            view.handle_editor_key(&key("enter"), cx);
            assert_eq!(view.cmd.text(), "zzqqx /work/one ");
        }
    }).unwrap();
    assert_eq!(next_input_until_timeout(&mut daemon), None);
}

#[gpui_kit::test]
fn a_late_remote_listing_leaves_a_line_the_editor_no_longer_owns_alone(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    window.update(cx, |view, _, cx| {
        view.cmd.set("zzqqx /work/");
        let stale = remote_request(view);
        view.editor_handoff = Some(view.terminal.prompt_cycle());
        view.remote_path_results(stale, Ok(Vec::new()), cx);
        assert_eq!(view.cmd.text(), "zzqqx /work/");
        assert!(view.render_completion_menu(cx).is_none());
    }).unwrap();
    assert_eq!(next_input_until_timeout(&mut daemon), None);
}

#[gpui_kit::test]
fn a_directory_change_rejects_the_previous_listing(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    let request = window.update(cx, |view, _, _| {
        view.cmd.set("zzqqx /work/");
        remote_request(view)
    }).unwrap();
    DaemonMsg::Cwd(std::path::PathBuf::from("/different-completion-directory")).encode(&mut daemon).unwrap();
    for _ in 0..200 {
        cx.run_until_parked();
        if window.update(cx, |view, _, _| view.cwd() == Some(std::path::PathBuf::from("/different-completion-directory"))).unwrap() { break }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    window.update(cx, |view, _, cx| {
        assert_eq!(view.cwd(), Some(std::path::PathBuf::from("/different-completion-directory")));
        view.remote_path_results(request, Ok(entries(&["obsolete"], false)), cx);
        assert_eq!(view.cmd.text(), "zzqqx /work/");
        assert!(view.render_completion_menu(cx).is_none());
    }).unwrap();
    assert_eq!(next_input_until_timeout(&mut daemon), None);
}
