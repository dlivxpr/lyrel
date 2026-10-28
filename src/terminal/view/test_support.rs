//! Shared socket-backed terminal consumer setup; no capability assertions here.

use super::*;
use crate::daemon::protocol::{ClientMsg, DaemonMsg};
use crate::daemon::transport::Stream;
use crate::terminal::remote::PtySource;
use gpui_kit::{Entity, TestAppContext};

pub(super) fn history_fixture(entries: Vec<String>) -> crate::terminal::history::History {
crate::terminal::history::History {
    entries,
    ..Default::default()
}
}

pub(super) fn seed_history(view: &mut TerminalView, entries: Vec<String>) {
view.history = TerminalHistory::from_loaded(history_fixture(entries));
}

pub(super) fn harness(
    cx: &mut TestAppContext,
) -> (gpui_kit::WindowHandle<TerminalView>, Stream) {
    let pty = if cfg!(windows) {
        PtySource::LocalConpty
    } else {
        PtySource::Raw
    };
    harness_on(cx, pty)
}

pub(super) fn harness_on(
    cx: &mut TestAppContext,
    pty: PtySource,
) -> (gpui_kit::WindowHandle<TerminalView>, Stream) {
    // Building a view reads the config. Whether that hit the real user
    // directory used to come down to which test happened to pin the
    // scratch dir first.
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let (client_side, daemon_side) = super::test_stream_pair();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let window = cx.add_window(|window, cx| {
        let terminal = RemoteTerminal::from_stream_with(
            client_side,
            TermSize::new(80, 24),
            Vec::new(),
            pty,
        )
        .expect("socketpair-backed terminal");
        TerminalView::with_terminal(terminal, 1, window, cx)
    });
    (window, daemon_side)
}

/// The same pane, but hung under a `gpui_component::Root` the way the real
/// window hangs it.
///
/// `harness` makes the view its own root, which is enough for anything
/// that never paints — but gpui-component's text input reaches for `Root`
/// while painting, so any test that lets a frame draw with the search bar
/// (or any other input) on screen needs this one instead.
pub(super) fn rooted_harness(
    cx: &mut TestAppContext,
) -> (
    gpui_kit::WindowHandle<gpui_kit::component::Root>,
    Entity<TerminalView>,
    Stream,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let (client_side, daemon_side) = super::test_stream_pair();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let built: std::rc::Rc<std::cell::RefCell<Option<Entity<TerminalView>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let out = built.clone();
    let window = cx.add_window(move |window, cx| {
        let terminal = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24))
            .expect("socketpair-backed terminal");
        let view = cx.new(|cx| TerminalView::with_terminal(terminal, 1, window, cx));
        *out.borrow_mut() = Some(view.clone());
        gpui_kit::component::Root::new(view, window, cx)
    });
    window
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    cx.background_executor.run_until_parked();
    let view = built.borrow_mut().take().expect("the pane was built");
    (window, view, daemon_side)
}

pub(super) fn prompt_ready(
    window: &gpui_kit::WindowHandle<TerminalView>,
    cx: &mut TestAppContext,
    daemon: &mut Stream,
) {
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(daemon)
    .unwrap();
    for _ in 0..200 {
        if window
            .update(cx, |view, _, _| view.terminal.at_prompt())
            .unwrap()
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the prompt report never reached the view");
}

pub(super) fn alt_screen_ready(
    window: &gpui_kit::WindowHandle<TerminalView>,
    cx: &mut TestAppContext,
    daemon: &mut Stream,
) {
    DaemonMsg::Output(b"\x1b[?1049h".to_vec())
        .encode(daemon)
        .unwrap();
    for _ in 0..400 {
        cx.run_until_parked();
        if window
            .update(cx, |view, _, _| view.on_alt_screen())
            .unwrap()
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the alternate-screen switch never reached the grid");
}

pub(super) fn next_input(daemon: &mut Stream) -> Vec<u8> {
    loop {
        match ClientMsg::read(daemon).expect("client socket stays open") {
            ClientMsg::Input(bytes) => return bytes,
            _ => continue,
        }
    }
}

pub(super) fn type_char(
    view: &mut TerminalView,
    ch: &str,
    window: &mut Window,
    cx: &mut Context<TerminalView>,
) {
    if cfg!(target_os = "macos") {
        let _ = window;
        view.commit_text(ch, cx);
    } else {
        let ev = KeyDownEvent {
            keystroke: gpui_kit::Keystroke {
                modifiers: gpui_kit::Modifiers::default(),
                key: ch.to_string(),
                key_char: Some(ch.to_string()),
            },
            is_held: false,
            prefer_character_input: false,
        };
        view.on_key_down(&ev, window, cx);
    }
}

pub(super) fn next_input_until_timeout(daemon: &mut Stream) -> Option<Vec<u8>> {
    use std::io::ErrorKind;

    daemon
        .set_read_timeout(Some(std::time::Duration::from_millis(250)))
        .unwrap();
    loop {
        match ClientMsg::read(daemon) {
            Ok(ClientMsg::Input(bytes)) => return Some(bytes),
            Ok(_) => continue,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return None;
            }
            Err(e) => panic!("client socket failed before Input: {e}"),
        }
    }
}

/// `next_input_until_timeout` with the executor run between reads: a reply
/// crosses the reader thread and the view before it is written.
pub(super) fn pumped_input(cx: &mut TestAppContext, daemon: &mut Stream, tries: usize) -> Option<String> {
    (0..tries)
        .find_map(|_| {
            cx.run_until_parked();
            next_input_until_timeout(daemon)
        })
        .map(|bytes| String::from_utf8(bytes).unwrap())
}

pub(super) fn wait_for_input_active(
    window: &gpui_kit::WindowHandle<TerminalView>,
    cx: &mut TestAppContext,
) {
    for _ in 0..200 {
        cx.run_until_parked();
        let active = window.update(cx, |view, _, _| view.input_active()).unwrap();
        if active {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the local editor never engaged at the prompt");
}

pub(super) fn key(spec: &'static str) -> gpui_kit::Keystroke {
    gpui_kit::Keystroke::parse(spec).expect("valid keystroke spec")
}

pub(super) fn scroll_into_history(view: &TerminalView, offset: usize) {
    let mut parser = rio_vt::performer::handler::Processor::default();
    let mut term = view.terminal.term.lock();
    parser.advance(&mut *term, &b"line\r\n".repeat(60));
    term.scroll_display(Scroll::Delta(offset as i32));
    assert_eq!(
        term.grid.display_offset(),
        offset,
        "the viewport starts parked in the scrollback"
    );
}

pub(super) fn display_offset(view: &TerminalView) -> usize {
    view.terminal.term.lock().grid.display_offset()
}

pub(super) fn bind_to_a_disconnected_remote_workspace(
    view: &mut TerminalView,
    cx: &mut Context<TerminalView>,
) -> crate::core::session::WorkspaceId {
    use crate::core::session::{
        RemoteRef, RemoteTarget, WindowViews, WorkspaceId, WorkspaceStore,
    };
    use crate::terminal::PaneWorkspace;
    let host = RemoteRef::new(
        RemoteTarget::direct("me", "build-box", 22),
        WorkspaceId::new(),
    );
    let entry = crate::core::session::WindowView::on_remote(host.clone());
    let id = entry.id;
    WorkspaceStore::install_for_test(
        cx,
        WindowViews {
            views: vec![entry],
            active: None,
        },
    );
    view.set_workspace(Some(PaneWorkspace {
        workspace: id,
        target: host.target,
        spec: Some(Box::new(
            serde_json::from_str(
                r#"{"host":"build-box","port":22,"user":"me","auth_mode":"auto"}"#,
            )
            .unwrap(),
        )),
        label: None,
        resize_echo: false,
    }));
    id
}

