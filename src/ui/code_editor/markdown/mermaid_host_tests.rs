//! 原生字体消费者：生产 workbench 的 Mermaid 输入、派生图形、history 与 Host 保存。
use super::super::super::*;
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext};
use lyrel_markdown_editor::MermaidPresentation;
use std::time::{Duration, Instant};

fn settle(context: &mut VisualTestContext) {
    context.run_until_parked();
    context.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.render_frame(cx);
    });
    context.run_until_parked();
}
#[track_caller]
fn until(context: &mut VisualTestContext, mut ready: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        settle(context);
        if ready(context) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Mermaid workbench did not settle"
        );
        context.executor().advance_clock(Duration::from_millis(50));
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn ready(
    view: &Entity<lyrel_markdown_editor::EditorState>,
    context: &mut VisualTestContext,
) -> (f32, f32) {
    until(context, |context| {
        view.read_with(context, |view, _| {
            matches!(
                view.mermaid_presentation(2),
                Some(MermaidPresentation::Ready(..))
            )
        })
    });
    view.read_with(context, |view, _| {
        match view.mermaid_presentation(2).unwrap() {
            MermaidPresentation::Ready(_, width, height) => (width, height),
            _ => unreachable!(),
        }
    })
}
fn image(
    view: &Entity<lyrel_markdown_editor::EditorState>,
    context: &VisualTestContext,
) -> std::sync::Arc<gpui_kit::RenderImage> {
    view.read_with(context, |view, _| {
        match view.mermaid_presentation(2).unwrap() {
            MermaidPresentation::Ready(image, ..) => image,
            _ => panic!("expected the current native Mermaid image"),
        }
    })
}
fn ready_after(
    view: &Entity<lyrel_markdown_editor::EditorState>,
    previous: &std::sync::Arc<gpui_kit::RenderImage>,
    context: &mut VisualTestContext,
) -> (f32, f32) {
    until(context, |context| {
        view.read_with(context, |view, _| {
            matches!(
                view.mermaid_presentation(2),
                Some(MermaidPresentation::Ready(image, ..))
                    if !std::sync::Arc::ptr_eq(&image, previous)
            )
        })
    });
    ready(view, context)
}
fn save(app: &Entity<LyrelApp>, context: &mut VisualTestContext) {
    context.dispatch_action(crate::core::actions::EditorSave);
    until(context, |context| {
        app.read_with(context, |app, _| {
            app.active_buffer()
                .is_some_and(|file| !file.dirty && !file.saving)
        })
    });
}

pub(super) fn lifecycle(cx: &mut TestAppContext) {
    use super::super::super::test_host::{FileHost, canonical_tempdir};
    let remote = canonical_tempdir();
    let path = remote.path().join("diagram.md");
    let original = "\u{feff}# Diagram\r\n\n```mermaid\r\nflowchart TD\nA[Start] --> B[Done]\r\n```\n\nKEEP  \r\n";
    std::fs::write(&path, original.as_bytes()).unwrap();
    let host = FileHost::mapped(HostId(242), remote.path());
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.simulate_resize(gpui_kit::size(px(1600.), px(1000.)));
    app.update_in(&mut context, |app, window, cx| {
        app.editor_open_on_host(host.clone(), Path::new("/diagram.md"), window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let first = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    let before = ready(&first, &mut context);
    let original_image = first.read_with(&context, |view, _| {
        match view.mermaid_presentation(2).unwrap() {
            MermaidPresentation::Ready(image, ..) => image,
            _ => unreachable!(),
        }
    });
    first.update_in(&mut context, |view, window, cx| {
        view.select_range(0..0, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("X", cx));
    settle(&mut context);
    first.read_with(&context, |view, _| {
        match view.mermaid_presentation(2).unwrap() {
            MermaidPresentation::Ready(image, ..) => assert!(
                std::sync::Arc::ptr_eq(&image, &original_image),
                "ordinary prose edit must retain native cache"
            ),
            _ => panic!("unchanged diagram must not re-enter loading"),
        }
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    app.update_in(&mut context, |app, window, cx| {
        assert!(app.editor_split(window, cx));
    });
    settle(&mut context);
    let view = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    ready(&view, &mut context);
    let previous_image = image(&view, &context);
    // The production go-to command enters the source of the rendered block.
    context.dispatch_action(crate::core::actions::EditorGoToLine);
    context.update(|window, cx| {
        window.input("5:22", cx);
        window.press("enter", cx);
    });
    settle(&mut context);
    view.update_in(&mut context, |view, window, cx| {
        let at = view.text().find("Done").unwrap();
        view.select_range(at..at + 4, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|_, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
            "A considerably wider destination".into(),
        ));
    });
    context.dispatch_action(gpui_kit::component::input::Paste);
    settle(&mut context);
    view.update_in(&mut context, |view, _, cx| view.select_range(0..0, cx));
    let changed = ready_after(&view, &previous_image, &mut context);
    // Merman 的真实默认值将普通节点的标签宽度同时下限/换行约束在 120px；
    // 原生字体会把完整长标签折成多行，增高而不是突破最小节点宽度。
    assert!(
        changed.0 >= before.0 && changed.1 > before.1,
        "native labels must change the actual diagram geometry: before={before:?}, changed={changed:?}, source={}",
        view.read_with(&context, |view, _| view.text().to_owned())
    );
    ready(&first, &mut context);
    first.read_with(&context, |first, cx| {
        assert_eq!(first.text(), view.read(cx).text())
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.update_in(&mut context, |view, _, cx| view.select_range(0..0, cx));
    let undone = ready(&view, &mut context);
    assert_eq!(undone, before);
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    view.update_in(&mut context, |view, _, cx| view.select_range(0..0, cx));
    assert_eq!(ready(&view, &mut context), changed);
    save(&app, &mut context);
    let expected = original.replace("Done", "A considerably wider destination");
    assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
    // A malformed current source must expose a recoverable failure, never an error picture.
    view.update_in(&mut context, |view, window, cx| {
        let at = view.text().find("flowchart TD").unwrap();
        view.select_range(at..at + "flowchart TD".len(), cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|_, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
            "not-a-mermaid-family".into(),
        ));
    });
    context.dispatch_action(gpui_kit::component::input::Paste);
    settle(&mut context);
    view.update_in(&mut context, |view, _, cx| view.select_range(0..0, cx));
    until(&mut context, |context| {
        view.read_with(context, |view, _| {
            matches!(
                view.mermaid_presentation(2),
                Some(MermaidPresentation::Failed(_))
            )
        })
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.update_in(&mut context, |view, _, cx| view.select_range(0..0, cx));
    assert_eq!(ready(&view, &mut context), changed);
    save(&app, &mut context);
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host.clone(), Path::new("/diagram.md"), window, cx);
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let reopened = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    assert_eq!(ready(&reopened, &mut context), changed);
    reopened.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        )
    });
}

struct CompletionGate {
    source: String,
    arrived: smol::channel::Sender<()>,
    release: smol::channel::Receiver<()>,
    returned: smol::channel::Sender<()>,
}
static COMPLETION_GATE: std::sync::Mutex<Option<CompletionGate>> = std::sync::Mutex::new(None);
struct Delayed {
    arrived: smol::channel::Receiver<()>,
    release: smol::channel::Sender<()>,
    returned: smol::channel::Receiver<()>,
}
impl Delayed {
    fn new(source: &str) -> Self {
        let (arrived_tx, arrived) = smol::channel::bounded(1);
        let (release, release_rx) = smol::channel::bounded(1);
        let (returned_tx, returned) = smol::channel::bounded(1);
        let mut gate = COMPLETION_GATE.lock().unwrap();
        assert!(gate.is_none());
        *gate = Some(CompletionGate {
            source: source.into(),
            arrived: arrived_tx,
            release: release_rx,
            returned: returned_tx,
        });
        Self {
            arrived,
            release,
            returned,
        }
    }
    fn wait(&self, context: &mut VisualTestContext) {
        until(context, |_| self.arrived.try_recv().is_ok());
    }
    fn release(&self, context: &mut VisualTestContext) {
        self.release.try_send(()).unwrap();
        until(context, |_| self.returned.try_recv().is_ok());
        settle(context);
    }
}
impl Drop for Delayed {
    fn drop(&mut self) {
        let _ = self.release.try_send(());
        *COMPLETION_GATE.lock().unwrap() = None;
    }
}
// The real native result is held only after rendering; no output is replaced.
pub(super) fn hold_completion(key: &super::Key) {
    let gate = {
        let mut gate = COMPLETION_GATE.lock().unwrap();
        if gate
            .as_ref()
            .is_some_and(|gate| key.source.contains(&gate.source))
        {
            gate.take()
        } else {
            None
        }
    };
    if let Some(gate) = gate {
        let _ = gate.arrived.send_blocking(());
        let _ = gate.release.recv_blocking();
        let _ = gate.returned.send_blocking(());
    }
}

pub(super) fn stale_completion(cx: &mut TestAppContext) {
    use super::super::super::test_host::canonical_tempdir;
    let root = canonical_tempdir();
    let path = root.path().join("stale.md");
    let other = root.path().join("other.md");
    let original = "# Diagram\n\n```mermaid\nflowchart TD\nA[HELD-OLD] --> B[Done]\n```\n\nKEEP\n";
    std::fs::write(&path, original).unwrap();
    std::fs::write(&other, "# Other\nDO NOT CHANGE\n").unwrap();
    let delayed = Delayed::new("HELD-OLD");
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let view = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    delayed.wait(&mut context);
    view.read_with(&context, |view, _| {
        assert!(matches!(
            view.mermaid_presentation(2),
            Some(MermaidPresentation::Loading(_))
        ))
    });
    view.update_in(&mut context, |view, window, cx| {
        let at = view.text().find("HELD-OLD").unwrap();
        view.select_range(at..at + 8, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("NEW CURRENT MUCH WIDER NODE", cx));
    view.update_in(&mut context, |view, _, cx| view.select_range(0..0, cx));
    let current = ready(&view, &mut context);
    delayed.release(&mut context);
    assert_eq!(
        ready(&view, &mut context),
        current,
        "late old native result cannot replace the current diagram"
    );
    let expected = original.replace("HELD-OLD", "NEW CURRENT MUCH WIDER NODE");
    view.read_with(&context, |view, _| assert_eq!(view.text(), expected));
    drop(delayed);

    let removed = Delayed::new("HELD-REMOVED");
    view.update_in(&mut context, |view, window, cx| {
        let at = view.text().find("NEW CURRENT MUCH WIDER NODE").unwrap();
        view.select_range(at..at + "NEW CURRENT MUCH WIDER NODE".len(), cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("HELD-REMOVED", cx));
    removed.wait(&mut context);
    view.update_in(&mut context, |view, window, cx| {
        let block = lyrel_markdown_editor::mermaid_blocks(view.text()).remove(0);
        view.select_range(block.range(), cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("Removed diagram\n", cx));
    settle(&mut context);
    save(&app, &mut context);
    let retained = view.read_with(&context, |view, _| view.text().to_owned());
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&other, window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| {
            app.active_buffer().is_some_and(|file| file.path == other)
        })
    });
    removed.release(&mut context);
    view.read_with(&context, |view, _| {
        assert_eq!(view.text(), retained);
        assert!(
            view.mermaid_presentation(2).is_none(),
            "removed owner cannot be resurrected"
        );
    });
    app.read_with(&context, |app, cx| {
        assert_eq!(
            app.active_editor_input()
                .unwrap()
                .read(cx)
                .text()
                .to_string(),
            "# Other\nDO NOT CHANGE\n"
        )
    });
    assert_eq!(std::fs::read(&path).unwrap(), retained.as_bytes());
}

pub(super) fn theme_zoom_scroll(cx: &mut TestAppContext) {
    use super::super::super::test_host::canonical_tempdir;
    let root = canonical_tempdir();
    let path = root.path().join("geometry.md");
    let original = format!(
        "# Diagram\n\n```mermaid\nflowchart TD\nA[GEOMETRY-HELD] --> B[Done]\n```\n\n{}TAIL\n",
        "paragraph remains source\n".repeat(180)
    );
    std::fs::write(&path, &original).unwrap();
    let delayed = Delayed::new("GEOMETRY-HELD");
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.simulate_resize(gpui_kit::size(px(1200.), px(800.)));
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let view = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    delayed.wait(&mut context);
    let tail = original.find("TAIL").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(tail..tail, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    until(&mut context, |context| {
        context.update(|window, cx| {
            view.read(cx)
                .range_to_bounds(&(tail..tail))
                .is_some_and(|caret| {
                    caret.top() >= px(0.) && caret.bottom() <= window.viewport_size().height
                })
        })
    });
    let before = view.read_with(&context, |view, _| {
        view.range_to_bounds(&(tail..tail)).unwrap()
    });
    delayed.release(&mut context);
    let initial = ready(&view, &mut context);
    let initial_image = image(&view, &context);
    settle(&mut context);
    let after = view.read_with(&context, |view, _| {
        view.range_to_bounds(&(tail..tail)).unwrap()
    });
    assert!(
        (before.top() - after.top()).abs() <= px(2.),
        "late diagram sizing must anchor the visible document row"
    );
    let (prior_size, prior_ui_size) = context.update(|_, cx| {
        use gpui_kit::component::ActiveTheme as _;
        (
            cx.theme().font_size,
            cx.global::<crate::core::config::Config>().ui_font_size,
        )
    });
    let zoom_size = prior_ui_size * 1.25;
    // 与 SettingsWindow::set_ui_font_size 相同：配置负责工作台 rem，Theme 负责 provider。
    app.update_in(&mut context, |_, _, cx| {
        cx.global_mut::<crate::core::config::Config>().ui_font_size = zoom_size;
        gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(zoom_size));
        cx.notify();
    });
    let zoomed = ready_after(&view, &initial_image, &mut context);
    assert!(
        zoomed.0 >= initial.0 && zoomed.1 > initial.1,
        "zoom must enlarge actual native diagram geometry: initial={initial:?}, zoomed={zoomed:?}"
    );
    context.update(|window, _| assert_eq!(window.rem_size(), px(zoom_size)));
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    // Hit-testing/input at the retained visible caret still targets the same source.
    let position = view.read_with(&context, |view, _| {
        view.range_to_bounds(&(tail..tail)).unwrap().center()
    });
    context.update(|window, cx| {
        window.pointer_move(position, None, cx);
        window.pointer_down(position, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(position, gpui_kit::MouseButton::Left, cx);
        window.input("X", cx);
    });
    settle(&mut context);
    view.read_with(&context, |view, _| assert!(view.text().contains("XTAIL")));
    context.dispatch_action(gpui_kit::component::input::Undo);
    save(&app, &mut context);
    assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
    app.update_in(&mut context, |_, _, cx| {
        cx.global_mut::<crate::core::config::Config>().ui_font_size = prior_ui_size;
        gpui_kit::component::Theme::update(cx, |theme| theme.font_size = prior_size);
        cx.notify();
    });
}
