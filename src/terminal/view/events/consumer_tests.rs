use super::*;
use crate::daemon::protocol::DaemonMsg;
use crate::terminal::view::test_support::*;
use gpui_kit::TestAppContext;
use rio_vt::crosswords::pos::{Column, Line};

#[gpui_kit::test]
fn pty_write_events_reach_the_daemon_as_input(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.handle_event(PaneEvent::PtyWrite("ping".into()), cx);
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"ping".to_vec());
}

#[gpui_kit::test]
fn child_exit_marks_the_view_exited(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.handle_event(PaneEvent::Exit, cx);
            assert!(view.terminal.exited);
            assert_eq!(view.title, "lyrel — process exited");
        })
        .unwrap();
}

#[gpui_kit::test]
fn text_area_size_request_replies_with_the_current_geometry(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    let want = window
        .update(cx, |view, _, cx| {
            let size = view.terminal.size();
            let fmt = std::sync::Arc::new(|ws: rio_vt::event::WindowSize| {
                format!("{}x{}", ws.width, ws.height)
            });
            view.handle_event(PaneEvent::TextAreaSizeRequest(fmt), cx);
            format!(
                "{}x{}",
                (size.cols as f32 * view.cell_width.as_f32()).round() as u16,
                (size.rows as f32 * view.line_height.as_f32()).round() as u16
            )
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), want.into_bytes());
}

#[gpui_kit::test]
fn child_exit_emits_the_close_event_but_disconnect_does_not(cx: &mut TestAppContext) {
    use std::cell::Cell;
    use std::rc::Rc;

    let subscribe = |window: &gpui_kit::WindowHandle<TerminalView>, cx: &mut TestAppContext| {
        let got = Rc::new(Cell::new(false));
        let seen = got.clone();
        window
            .update(cx, |_, _, cx| {
                let this = cx.entity();
                cx.subscribe(&this, move |_, _, _: &ChildExited, _| seen.set(true))
                    .detach();
            })
            .unwrap();
        got
    };
    let wait_exited = |window: &gpui_kit::WindowHandle<TerminalView>, cx: &mut TestAppContext| {
        for _ in 0..400 {
            cx.run_until_parked();
            let exited = window
                .update(cx, |view, _, _| view.terminal.exited)
                .unwrap();
            if exited {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the view never noticed the exit");
    };

    let (window, mut daemon) = harness(cx);
    let got = subscribe(&window, cx);
    DaemonMsg::Exited { code: Some(0) }
        .encode(&mut daemon)
        .unwrap();
    wait_exited(&window, cx);
    assert!(got.get(), "a genuine child exit must emit ChildExited");

    let (window, daemon) = harness(cx);
    let got = subscribe(&window, cx);
    drop(daemon);
    wait_exited(&window, cx);
    assert!(!got.get(), "a daemon disconnect must not emit ChildExited");
}

#[gpui_kit::test]
fn daemon_output_reaches_the_grid_through_the_event_pump(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);

    let read_row = |cx: &mut TestAppContext, len: usize| -> String {
        window
            .update(cx, |view, _, _| {
                let term = view.terminal.term.clone();
                let term = term.lock();
                let grid = &term.grid;
                (0..len).map(|c| grid[Line(0)][Column(c)].c()).collect()
            })
            .unwrap()
    };
    let wait_for = |cx: &mut TestAppContext, want: &str| {
        let mut got = String::new();
        for _ in 0..400 {
            cx.run_until_parked();
            got = read_row(cx, want.chars().count());
            if got == want {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        got
    };

    DaemonMsg::Output(b"hello".to_vec())
        .encode(&mut daemon)
        .unwrap();
    assert_eq!(wait_for(cx, "hello"), "hello");

    DaemonMsg::Output(b" again".to_vec())
        .encode(&mut daemon)
        .unwrap();
    assert_eq!(wait_for(cx, "hello again"), "hello again");
}
