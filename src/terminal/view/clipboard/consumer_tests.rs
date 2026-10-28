use super::*;
use crate::daemon::protocol::DaemonMsg;
use crate::terminal::view::test_support::*;
use gpui_kit::TestAppContext;
use rio_vt::crosswords::pos::Line;

#[gpui_kit::test]
fn ctrl_c_copy_consumes_the_selection_so_the_next_press_is_sigint(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);

    DaemonMsg::Output(b"hello world".to_vec())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..400 {
        cx.run_until_parked();
        let row: String = window
            .update(cx, |view, _, _| {
                let term = view.terminal.term.clone();
                let term = term.lock();
                (0..11).map(|c| term.grid[Line(0)][Column(c)].c()).collect()
            })
            .unwrap();
        if row == "hello world" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, window, cx| {
            view.on_select_start(0, 0, true, 1, false, cx);
            view.on_select_update(4, 0, false, cx);
            view.on_select_end(cx);
            assert!(view.has_selection(), "the drag must leave a selection");

            let consumed = view.handle_cmd_shortcut(&key("ctrl-c"), window, cx);
            assert!(matches!(consumed, CmdKey::Consumed));
            assert!(
                !view.has_selection(),
                "the Ctrl+C copy must consume the selection"
            );

            let fell_through = view.handle_cmd_shortcut(&key("ctrl-c"), window, cx);
            assert!(matches!(fell_through, CmdKey::FallThrough));
        })
        .unwrap();
    let text = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(text.as_deref(), Some("hello"));
}

#[gpui_kit::test]
fn cmd_v_pastes_on_the_alternate_screen(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("echo hi".into())));
    alt_screen_ready(&window, cx, &mut daemon);

    window
        .update(cx, |view, window, cx| {
            let pasted = view.handle_cmd_shortcut(&key("cmd-v"), window, cx);
            assert!(
                matches!(pasted, CmdKey::Consumed),
                "Cmd+V carries no control code and pastes on every screen"
            );
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"echo hi".to_vec());
    assert_eq!(next_input_until_timeout(&mut daemon), None);
}

/// The Ctrl+V half of the same key, through the real keymap: whether it
/// pastes is the `AlternatePaste` binding's decision, not this view's, so
/// these two drive it the way a user does rather than calling in.
#[cfg(not(target_os = "macos"))]
#[gpui_kit::test]
fn ctrl_v_pastes_at_a_prompt(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| crate::ui::keymap::init(cx));
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("echo hi".into())));
    window
        .update(cx, |view, window, cx| {
            assert!(!view.on_alt_screen());
            window.activate_window();
            view.focus_handle.focus(window, cx);
        })
        .unwrap();

    let mut vcx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_keystrokes("ctrl-v");

    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"echo hi".to_vec())
    );
}

#[cfg(not(target_os = "macos"))]
#[gpui_kit::test]
fn ctrl_v_reaches_a_full_screen_program_as_syn(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| crate::ui::keymap::init(cx));
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("echo hi".into())));
    alt_screen_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, window, cx| {
            window.activate_window();
            view.focus_handle.focus(window, cx);
        })
        .unwrap();

    let mut vcx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_keystrokes("ctrl-v");

    assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![0x16]));
}

/// The other half of that rule, which the binding's context cannot state.
///
/// gpui matches a keystroke against the frame it last *painted*, so
/// `!alt_screen` outlives the switch by a frame: launch a full-screen
/// program and hit Ctrl+V before the next paint and the keymap still
/// believes the pane is at a prompt. The action asks the grid itself, so
/// the clipboard never lands in a program that would run it as commands.
#[gpui_kit::test]
fn alternate_paste_asks_the_grid_and_not_the_last_frame(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("echo hi".into())));

    window
        .update(cx, |view, _window, cx| {
            assert!(!view.on_alt_screen());
            view.alternate_paste(cx);
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"echo hi".to_vec());

    alt_screen_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, _window, cx| view.alternate_paste(cx))
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "the paste is withheld even when the frame that matched said otherwise"
    );
}

#[gpui_kit::test]
fn paste_to_the_pty_consumes_the_selection(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.select_all(cx);
            assert!(view.has_selection());
            view.paste("echo hi".into(), cx);
            assert!(
                !view.has_selection(),
                "a PTY paste must consume the selection"
            );
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"echo hi".to_vec());
}

#[gpui_kit::test]
fn ctrl_c_copy_consumes_the_editor_selection_at_the_prompt(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);

    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(0),
    }
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..400 {
        cx.run_until_parked();
        let active = window.update(cx, |view, _, _| view.input_active()).unwrap();
        if active {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, window, cx| {
            assert!(view.input_active(), "the inline editor must be active");
            view.input.editor_mut().insert_str("echo hi");
            view.input.editor_mut().select_all();

            let consumed = view.handle_cmd_shortcut(&key("ctrl-c"), window, cx);
            assert!(matches!(consumed, CmdKey::Consumed));
            assert!(
                view.input.editor().selection().is_none(),
                "the Ctrl+C copy must consume the editor selection"
            );

            let fell_through = view.handle_cmd_shortcut(&key("ctrl-c"), window, cx);
            assert!(matches!(fell_through, CmdKey::FallThrough));
        })
        .unwrap();
    let text = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(text.as_deref(), Some("echo hi"));
}

#[gpui_kit::test]
fn allowed_remote_clipboard_image_reaches_the_system_clipboard(cx: &mut TestAppContext) {
    use gpui_kit::ClipboardEntry;

    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, _| {
            let mut spec: crate::daemon::protocol::NativeSshSpec = serde_json::from_str(
                r#"{"host":"build-box","port":22,"user":"me","auth_mode":"auto"}"#,
            )
            .unwrap();
            spec.remote_clipboard_write = true;
            view.ssh_spec = Some(Box::new(spec));
        })
        .unwrap();

    let pixel = image::RgbaImage::from_pixel(1, 1, image::Rgba([4, 5, 6, 255]));
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(pixel)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let write = lyrel_core::core::clipboard::ClipboardWrite {
        mime: "image/png".into(),
        data: png.clone(),
        id: Some("copy-1".into()),
    };
    DaemonMsg::ClipboardWrite(write.encode_frame())
        .encode(&mut daemon)
        .unwrap();

    for _ in 0..400 {
        cx.run_until_parked();
        let copied = cx.update(|cx| {
            cx.read_from_clipboard().and_then(|item| {
                item.entries().iter().find_map(|entry| match entry {
                    ClipboardEntry::Image(image) => Some(image.bytes.clone()),
                    _ => None,
                })
            })
        });
        if copied.as_deref() == Some(png.as_slice()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let copied = cx.update(|cx| cx.read_from_clipboard());
    assert!(copied.is_some_and(|item| {
        item.entries()
            .iter()
            .any(|entry| matches!(entry, ClipboardEntry::Image(image) if image.bytes == png))
    }));
    assert_eq!(
        next_input(&mut daemon),
        lyrel_core::core::clipboard::response(Some("copy-1"), "DONE")
    );
}

#[gpui_kit::test]
fn disabled_remote_clipboard_image_is_rejected_without_overwriting(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, _| {
            view.ssh_spec = Some(Box::new(
                serde_json::from_str(
                    r#"{"host":"build-box","port":22,"user":"me","auth_mode":"auto"}"#,
                )
                .unwrap(),
            ));
        })
        .unwrap();
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("keep me".into())));

    let write = lyrel_core::core::clipboard::ClipboardWrite {
        mime: "image/png".into(),
        data: vec![1, 2, 3],
        id: Some("copy-2".into()),
    };
    DaemonMsg::ClipboardWrite(write.encode_frame())
        .encode(&mut daemon)
        .unwrap();
    let mut reply = None;
    for _ in 0..100 {
        cx.run_until_parked();
        reply = next_input_until_timeout(&mut daemon);
        if reply.is_some() {
            break;
        }
    }

    assert_eq!(
        reply,
        Some(lyrel_core::core::clipboard::response(
            Some("copy-2"),
            "EPERM"
        ))
    );
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .as_deref(),
        Some("keep me")
    );
}
