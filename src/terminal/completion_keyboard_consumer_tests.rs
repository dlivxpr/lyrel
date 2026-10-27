use super::*;
use super::completion_consumer_fixture::*;

#[gpui_kit::test]
fn accepting_a_candidate_quotes_it_for_the_shell(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    window.update(cx, |view, window, cx| {
        for (line, name, expected) in [
            ("cd My", "My Documents", "cd 'My Documents'/"),
        ] {
            view.cmd.set(line);
            let request = remote_request(view);
            view.remote_path_results(request, Ok(entries(&[name], true)), cx);
            assert_eq!(view.cmd.text(), expected);
            type_char(view, "x", window, cx);
            assert_eq!(view.cmd.text(), format!("{expected}x"));
        }
        view.cmd.set("cd ~/My");
        let candidate = completion::Candidate {
            text: "~/My Documents".into(), kind: CandidateKind::Dir,
            start: 3, end: 6, description: None, icon: None,
        };
        let shell = view.shell_program();
        view.completion.offer(&mut view.cmd, 3, 6, vec![candidate], 0, shell.as_deref());
        assert_eq!(view.cmd.text(), "cd ~/'My Documents'/");
        view.cmd.set("git commit --mess");
        view.tab_pressed(true, cx);
        assert_eq!(view.cmd.text(), "git commit --message ");
    }).unwrap();
    assert_eq!(next_input_until_timeout(&mut daemon), None, "accepting only edits the input");
}

#[gpui_kit::test]
fn a_candidate_needing_escapes_is_never_half_applied(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    window.update(cx, |view, window, cx| {
        view.cmd.set("cd My");
        let request = remote_request(view);
        view.remote_path_results(request, Ok(entries(&["My Documents", "My Music"], true)), cx);
        assert_eq!(view.cmd.text(), "cd My", "the unsafe common prefix stays out of the input");
        assert_eq!(shown(view), ["My Music", "My Documents"]);
        assert!(view.render_completion_menu(cx).is_some());
        view.handle_editor_key(&key("enter"), cx);
        assert_eq!(view.cmd.text(), "cd 'My Music'/");
        type_char(view, "x", window, cx);
        assert_eq!(view.cmd.text(), "cd 'My Music'/x");
    }).unwrap();
    assert_eq!(next_input_until_timeout(&mut daemon), None);
}

#[gpui_kit::test]
fn insert_newline_action_closes_the_completion_menu_but_enter_still_accepts(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    window.update(cx, |view, window, cx| {
        view.cmd.set("zzqqx /work/");
        let request = remote_request(view);
        view.remote_path_results(request, Ok(entries(&["one", "two"], false)), cx);
        assert_eq!(shown(view), ["/work/one", "/work/two"]);
        view.insert_newline_action(cx);
        assert!(view.render_completion_menu(cx).is_none());
        assert_eq!(view.cmd.text(), "zzqqx /work/\n");
        view.cmd.set("zzqqx /work/");
        let request = remote_request(view);
        view.remote_path_results(request, Ok(entries(&["one", "two"], false)), cx);
        view.handle_editor_key(&key("down"), cx);
        view.handle_editor_key(&key("enter"), cx);
        assert_eq!(view.cmd.text(), "zzqqx /work/two ");
        type_char(view, "x", window, cx);
        assert_eq!(view.cmd.text(), "zzqqx /work/two x");
    }).unwrap();
    assert_eq!(next_input_until_timeout(&mut daemon), None);
    window.update(cx, |view, _, cx| view.handle_editor_key(&key("enter"), cx)).unwrap();
    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"zzqqx /work/two x\r".to_vec()));
}

#[gpui_kit::test]
fn tab_completion_off_sends_every_tab_to_the_shell(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    cx.update(|cx| {
        let mut config = cx.global::<Config>().clone();
        config.tab_completion = false;
        cx.set_global(config);
    });
    window.update(cx, |view, window, cx| {
        for ch in ["c", "d", " "] { type_char(view, ch, window, cx); }
        view.tab_pressed(true, cx);
        assert!(view.render_completion_menu(cx).is_none());
        assert_eq!(view.cmd.text(), "");
        view.tab_pressed(true, cx);
    }).unwrap();
    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"cd ".to_vec()));
    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"\t".to_vec()));
    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"\t".to_vec()));
}
