use super::*;
use crate::terminal::view::input::TerminalInputHandler;
use gpui_kit::InputHandler;

#[gpui_kit::test]
fn ime_mark_commit_blur_and_resize_preserve_the_current_input_owner(cx: &mut TestAppContext) {
    let (window, terminal, mut daemon) = rooted_harness(cx);
    DaemonMsg::Prompt { active: true, at_prompt: true, last_exit: None }.encode(&mut daemon).unwrap();
    let mut ready = false;
    for _ in 0..200 {
        cx.run_until_parked();
        ready = window.update(cx, |_, _, cx| terminal.read(cx).input_active()).unwrap();
        if ready { break; }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(ready, "the real pane prompt reaches the editor");
    let mut handler = TerminalInputHandler::new(terminal.clone(), None);
    let _blur_target = window.update(cx, |_, window, cx| {
        terminal.update(cx, |view, cx| view.focus_handle.focus(window, cx));
        handler.replace_and_mark_text_in_range(None, "ni😀", None, window, cx);
        assert_eq!(handler.marked_text_range(window, cx), Some(0..4));
        assert_eq!(terminal.read(cx).input.editor().text(), "");
        terminal.update(cx, |view, cx| view.set_grid_size(40, 12, px(8.), px(17.), 1., cx));
        assert_eq!(handler.marked_text_range(window, cx), Some(0..4), "resize does not commit a preedit");
        handler.replace_text_in_range(None, "你好😀", window, cx);
        assert_eq!(handler.marked_text_range(window, cx), None);
        assert_eq!(terminal.read(cx).input.editor().text(), "你好😀");
        handler.replace_and_mark_text_in_range(None, "discard", None, window, cx);
        let other = cx.focus_handle();
        other.focus(window, cx);
        other
    }).unwrap();
    cx.run_until_parked();
    window.update(cx, |_, window, cx| {
        assert_eq!(handler.marked_text_range(window, cx), None, "blur cancels rather than commits");
        terminal.update(cx, |view, cx| view.focus_handle.focus(window, cx));
        handler.replace_and_mark_text_in_range(None, "hao", None, window, cx);
        handler.replace_text_in_range(None, "好", window, cx);
        assert_eq!(terminal.read(cx).input.editor().text(), "你好😀好");
    }).unwrap();
    assert_eq!(next_input_until_timeout(&mut daemon), None, "the shell has not received an unsubmitted draft");
    DaemonMsg::Output(b"\x1b[?1049h".to_vec()).encode(&mut daemon).unwrap();
    let mut fullscreen = false;
    for _ in 0..200 {
        cx.run_until_parked();
        fullscreen = window.update(cx, |_, _, cx| terminal.read(cx).on_alt_screen()).unwrap();
        if fullscreen { break; }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(fullscreen, "the fullscreen program takes the real pane grid");
    window.update(cx, |_, window, cx| {
        handler.replace_and_mark_text_in_range(None, "quan", None, window, cx);
        handler.replace_text_in_range(None, "全屏", window, cx);
        assert_eq!(terminal.read(cx).input.editor().text(), "你好😀好", "the fullscreen commit does not edit the shell draft");
        assert_eq!(handler.marked_text_range(window, cx), None);
    }).unwrap();
    assert_eq!(next_input_until_timeout(&mut daemon), Some("全屏".as_bytes().to_vec()));
    assert_eq!(next_input_until_timeout(&mut daemon), None);
}

#[gpui_kit::test]
fn opening_search_discards_the_terminals_uncommitted_composition(cx: &mut TestAppContext) {
    let (window, view, _daemon) = rooted_harness(cx);
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.focus_handle.focus(window, cx);
                view.set_marked_text("ce'shi".into(), cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| {
            window.dispatch_action(Box::new(FindInTerminal), cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| {
            let view = view.read(cx);
            assert_eq!(view.input.marked(), "", "preedit belongs to the old focus");
            let search = view.search.as_ref().unwrap();
            assert_eq!(search.input.read(cx).value(), "");
            assert!(search.input.read(cx).focus_handle(cx).is_focused(window));
        })
        .unwrap();
}
