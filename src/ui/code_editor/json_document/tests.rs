//! #289 的生产工作台接缝：JSON／JSONC／JSONL 的结构、源码与保存。
//!
//! 这些测试只走工作台入口：真实文件打开、真实指针／键盘、源码输入、
//! Undo/Redo、Save、关闭与重开，以及映射 Host 与同名本机诱饵。每个输入
//! 都在自己的临时目录生成，不依赖协调目录的临时夹具；断言的是可见行、
//! 源码 occurrence、文档 revision、history 与落盘字节，不是内部缓存形状。
//! 它们不是原生 OS 窗口或真实 SSH 验收，相关边界在交付记录中保留。

use super::super::test_host::{FileHost, canonical_tempdir};
use super::super::*;
use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::test::{ElementSnapshot, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext, size};
use lyrel_document::json::projection;
use lyrel_markdown_editor::Document;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// BOM + mixed CRLF/LF + JSONC comments, duplicate keys, an exact decimal and
/// an escape. An untouched save has representation to lose if any Value
/// serialization or whole-file formatting leaks into the path.
const ORIGINAL: &[u8] = b"\xef\xbb\xbf{\r\n  // first comment\r\n  \"name\": \"first\",\n  \"name\": \"second\\u4e2d\",\r\n  \"n\": 12345678901234567890.00,\n  \"escaped\": \"line\\nbreak\",\r\n  \"nested\": { \"a\": 1 },\r\n}\n";

fn settle(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.render_frame(cx);
    });
    cx.run_until_parked();
}

fn until(cx: &mut VisualTestContext, mut ready: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready(cx) {
        assert!(
            Instant::now() < deadline,
            "JSON document transition did not settle"
        );
        settle(cx);
        cx.executor().advance_clock(Duration::from_millis(50));
        std::thread::sleep(Duration::from_millis(5));
    }
    settle(cx);
}

fn fixture(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn active_id(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> BufferId {
    app.read_with(cx, |app, _| {
        app.tab_code()
            .and_then(TabCode::active_id)
            .expect("a buffer in front")
    })
}

fn json_view(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<JsonView> {
    app.read_with(cx, |app, cx| {
        app.active_editor_input(cx)
            .expect("a document in front")
            .json()
            .expect("a JSON document")
            .clone()
    })
}

fn source_document(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<Document> {
    json_view(app, cx).read_with(cx, |view, cx| view.document().read(cx).document().clone())
}

fn source_text(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> String {
    source_document(app, cx).read_with(cx, |document, _| document.source().to_owned())
}

fn is_ready(view: &Entity<JsonView>, cx: &VisualTestContext) -> bool {
    view.read_with(cx, |view, cx| view.structure_ready(cx))
}

fn open(
    app: &Entity<LyrelApp>,
    cx: &mut VisualTestContext,
    host: SharedHost,
    path: &Path,
) -> BufferId {
    let path = path.to_path_buf();
    let at = path.clone();
    app.update_in(cx, |app, window, cx| {
        app.editor_open_on_host(host, &at, window, cx)
    });
    until(cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.active_buffer(cx)
                .is_some_and(|file| file.path == path && file.input.json().is_some())
        })
    });
    active_id(app, cx)
}

fn open_ready(
    app: &Entity<LyrelApp>,
    cx: &mut VisualTestContext,
    host: SharedHost,
    path: &Path,
) -> (BufferId, Entity<JsonView>) {
    let id = open(app, cx, host, path);
    let view = json_view(app, cx);
    until(cx, |cx| is_ready(&view, cx));
    (id, view)
}

fn scoped(view: &Entity<JsonView>) -> gpui_kit::ElementId {
    ("json-document", view.entity_id()).into()
}

fn row(view: &Entity<JsonView>, cx: &mut VisualTestContext, label: &str) -> ElementSnapshot {
    let scope = scoped(view);
    cx.update(|window, _| window.within(scope).find_by_label(label))
}

fn maybe_row(
    view: &Entity<JsonView>,
    cx: &mut VisualTestContext,
    label: &str,
) -> Option<ElementSnapshot> {
    let scope = scoped(view);
    cx.update(|window, _| window.within(scope).try_find_by_label(label))
}

fn row_value(view: &Entity<JsonView>, cx: &mut VisualTestContext, label: &str) -> Option<String> {
    maybe_row(view, cx, label).and_then(|row| row.value().map(str::to_owned))
}

fn click_row(view: &Entity<JsonView>, cx: &mut VisualTestContext, label: &str) {
    let snapshot = row(view, cx, label);
    let id = snapshot.path().last().cloned().expect("a row element id");
    cx.update(|window, cx| window.click(id, cx));
    settle(cx);
}

fn double_click_row(view: &Entity<JsonView>, cx: &mut VisualTestContext, label: &str) {
    let snapshot = row(view, cx, label);
    let id = snapshot.path().last().cloned().expect("a row element id");
    cx.update(|window, cx| window.double_click(id, cx));
    settle(cx);
}

fn selected_slice(
    app: &Entity<LyrelApp>,
    view: &Entity<JsonView>,
    cx: &VisualTestContext,
) -> Option<String> {
    let range = view.read_with(cx, |view, cx| {
        view.selected_location(cx).map(|location| location.range)
    })?;
    let text = source_text(app, cx);
    text.get(range).map(str::to_owned)
}

fn reveal_selected(app: &Entity<LyrelApp>, cx: &mut VisualTestContext) {
    app.update_in(cx, |app, window, cx| {
        app.focus_editor(window, cx);
    });
    cx.update(|window, cx| window.press("enter", cx));
    settle(cx);
}

fn press(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, key: &str) {
    app.update_in(cx, |app, window, cx| {
        app.focus_editor(window, cx);
    });
    cx.update(|window, cx| window.press(key, cx));
    settle(cx);
}

fn toggle_mode(view: &Entity<JsonView>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click(("json-mode-toggle", view.entity_id()), cx));
    settle(cx);
}

fn type_source(
    app: &Entity<LyrelApp>,
    cx: &mut VisualTestContext,
    range: std::ops::Range<usize>,
    text: &str,
) {
    let view = json_view(app, cx);
    app.update_in(cx, |app, window, cx| {
        app.focus_editor(window, cx);
        let source = view.read(cx).source().clone();
        source.update(cx, |source, cx| source.select_range(range, cx));
    });
    cx.update(|window, cx| window.input(text, cx));
    settle(cx);
}

fn save(app: &Entity<LyrelApp>, cx: &mut VisualTestContext) {
    cx.dispatch_action(crate::core::actions::EditorSave);
    until(cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.active_buffer(cx)
                .is_some_and(|file| !file.dirty && !file.saving)
        })
    });
}

fn secondary(chord: &str) -> String {
    match cfg!(target_os = "macos") {
        true => format!("cmd-{chord}"),
        false => format!("ctrl-{chord}"),
    }
}

fn replace_bytes(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let at = bytes
        .windows(from.len())
        .position(|window| window == from)
        .expect("fixture bytes");
    let mut replaced = bytes[..at].to_vec();
    replaced.extend_from_slice(to);
    replaced.extend_from_slice(&bytes[at + from.len()..]);
    replaced
}

#[gpui_kit::test]
fn mapped_host_structure_occurrences_source_edit_and_exact_save(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let remote = canonical_tempdir();
    let local = canonical_tempdir();
    // The mapped Host's file and a same-named local decoy: only the owning
    // Host's bytes may be read or written.
    let remote_file = fixture(remote.path(), "config.jsonc", ORIGINAL);
    let local_file = fixture(
        local.path(),
        "config.jsonc",
        b"LOCAL DECOY MUST STAY UNCHANGED",
    );
    let host = FileHost::mapped(HostId(289), remote.path());
    let path = PathBuf::from("/config.jsonc");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));

    let (id, view) = open_ready(&app, &mut cx, host.clone(), &path);
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("installed document");
        assert!(!file.dirty, "an untouched JSONC opens clean");
        let format = file.format(cx).expect("source format");
        assert!(format.bom, "the UTF-8 BOM is part of the saved bytes");
        assert_eq!(format.encoding, encoding_rs::UTF_8);
        assert!(
            app.editor.read(cx).local_file(id).is_none(),
            "native JSON is not a code language-service file"
        );
        assert!(app.json_document_active(cx));
    });
    assert_eq!(
        view.read_with(&cx, |view, _| view.mode()),
        JsonMode::Structure,
        "JSONC opens in the structure view"
    );
    cx.update(|window, _| {
        assert!(
            window
                .find(("json-mode-toggle", view.entity_id()))
                .visible()
        );
        assert!(window.find("status-json-mode").visible());
        assert!(window.find("status-json-count").visible());
        assert!(window.try_find("status-image-readonly").is_none());
    });
    assert!(row(&view, &mut cx, "$").expanded() == Some(true));
    assert_eq!(
        row_value(&view, &mut cx, "$[\"name\"]").as_deref(),
        Some("\"first\"")
    );
    assert_eq!(
        row_value(&view, &mut cx, "$[\"name\"] #2").as_deref(),
        Some("\"second\\u4e2d\"")
    );
    assert_eq!(
        row_value(&view, &mut cx, "$[\"n\"]").as_deref(),
        Some("12345678901234567890.00"),
        "the exact decimal spelling is displayed, never converted to f64"
    );
    assert_eq!(
        row_value(&view, &mut cx, "$[\"escaped\"]").as_deref(),
        Some("\"line\\nbreak\""),
        "the escape spelling stays visible"
    );

    // Pointer selection of the second duplicate locates that occurrence.
    click_row(&view, &mut cx, "$[\"name\"] #2");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.selected_path(cx))
            .as_deref(),
        Some("$[\"name\"] #2")
    );
    let slice = selected_slice(&app, &view, &cx).expect("selected occurrence");
    assert!(slice.contains("\"name\": \"second\\u4e2d\""));
    assert!(!slice.contains("first"));

    // Enter reveals the exact property occurrence in the shared source.
    reveal_selected(&app, &mut cx);
    assert_eq!(view.read_with(&cx, |view, _| view.mode()), JsonMode::Source);
    let source_range = view.read_with(&cx, |view, cx| view.source().read(cx).selected_range());
    let text = source_text(&app, &cx);
    assert!(text[source_range].contains("second\\u4e2d"));

    // Back to structure, then real keyboard navigation.
    toggle_mode(&view, &mut cx);
    press(&app, &mut cx, "home");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.selected_path(cx))
            .as_deref(),
        Some("$")
    );
    press(&app, &mut cx, "down");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.selected_path(cx))
            .as_deref(),
        Some("$[\"name\"]")
    );
    press(&app, &mut cx, "down");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.selected_path(cx))
            .as_deref(),
        Some("$[\"name\"] #2")
    );
    press(&app, &mut cx, "left");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.selected_path(cx))
            .as_deref(),
        Some("$")
    );
    press(&app, &mut cx, "right");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.selected_path(cx))
            .as_deref(),
        Some("$[\"name\"]")
    );

    // The container expands and collapses under pointer and keyboard.
    click_row(&view, &mut cx, "$[\"nested\"]");
    assert_eq!(row(&view, &mut cx, "$[\"nested\"]").expanded(), Some(false));
    click_row(&view, &mut cx, "$[\"nested\"]");
    assert_eq!(row(&view, &mut cx, "$[\"nested\"]").expanded(), Some(true));
    press(&app, &mut cx, "home");
    for _ in 0..5 {
        press(&app, &mut cx, "down");
    }
    press(&app, &mut cx, "space");
    assert_eq!(row(&view, &mut cx, "$[\"nested\"]").expanded(), Some(false));
    press(&app, &mut cx, "space");
    assert_eq!(row(&view, &mut cx, "$[\"nested\"]").expanded(), Some(true));

    // Full value is the exact source spelling, not a decoded projection.
    double_click_row(&view, &mut cx, "$[\"n\"]");
    let full = view.read_with(&cx, |view, cx| view.full_value_text(cx));
    assert_eq!(full.as_deref(), Some(r#""n": 12345678901234567890.00"#));
    cx.update(|window, _| {
        assert!(
            window
                .find(("json-full-value-text", view.entity_id()))
                .visible()
        );
        assert_eq!(
            window
                .find(("json-full-value-text", view.entity_id()))
                .value(),
            Some(r#""n": 12345678901234567890.00"#)
        );
    });
    cx.update(|window, cx| window.click(("json-full-value-copy", view.entity_id()), cx));
    settle(&mut cx);
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some(r#""n": 12345678901234567890.00"#.to_owned())
    );
    cx.update(|window, cx| window.click(("json-full-value-close", view.entity_id()), cx));
    settle(&mut cx);
    assert!(
        view.read_with(&cx, |view, _| view.full_value_path())
            .is_none()
    );

    // Untouched save writes the same BOM and every mixed terminator.
    save(&app, &mut cx);
    assert_eq!(std::fs::read(&remote_file).unwrap(), ORIGINAL);

    // A targeted source edit changes only that value; structure follows the
    // new revision and keeps the duplicate/key order intact.
    toggle_mode(&view, &mut cx);
    let text = source_text(&app, &cx);
    let at = text.find("\"first\"").expect("fixture value");
    type_source(&app, &mut cx, at..at + "\"first\"".len(), "\"edited\"");
    until(&mut cx, |cx| is_ready(&view, cx));
    toggle_mode(&view, &mut cx);
    assert_eq!(
        row_value(&view, &mut cx, "$[\"name\"]").as_deref(),
        Some("\"edited\"")
    );
    assert_eq!(
        row_value(&view, &mut cx, "$[\"name\"] #2").as_deref(),
        Some("\"second\\u4e2d\""),
        "the other duplicate occurrence is untouched"
    );
    assert_eq!(
        row_value(&view, &mut cx, "$[\"n\"]").as_deref(),
        Some("12345678901234567890.00")
    );

    // Undo/redo are the shared source history and the structure follows both.
    toggle_mode(&view, &mut cx);
    cx.update(|window, cx| window.press(&secondary("z"), cx));
    until(&mut cx, |cx| is_ready(&view, cx));
    toggle_mode(&view, &mut cx);
    assert_eq!(
        row_value(&view, &mut cx, "$[\"name\"]").as_deref(),
        Some("\"first\"")
    );
    toggle_mode(&view, &mut cx);
    cx.update(|window, cx| window.press(&secondary("shift-z"), cx));
    until(&mut cx, |cx| is_ready(&view, cx));
    toggle_mode(&view, &mut cx);
    assert_eq!(
        row_value(&view, &mut cx, "$[\"name\"]").as_deref(),
        Some("\"edited\"")
    );

    save(&app, &mut cx);
    let edited = replace_bytes(ORIGINAL, b"\"first\"", b"\"edited\"");
    assert_eq!(std::fs::read(&remote_file).unwrap(), edited);
    assert_eq!(
        std::fs::read(&local_file).unwrap(),
        b"LOCAL DECOY MUST STAY UNCHANGED",
        "the same path on this machine was never read or written"
    );

    // Reopen through the owning Host observes the saved bytes independently.
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.editor.read(cx).buffers.is_empty())
    });
    let (_, reopened) = open_ready(&app, &mut cx, host, &path);
    assert!(row_value(&reopened, &mut cx, "$[\"name\"]").as_deref() == Some("\"edited\""));
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        edited
    );
}

#[gpui_kit::test]
fn split_presentations_share_document_and_keep_folds_selection_and_full_value(
    tcx: &mut TestAppContext,
) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let path = fixture(dir.path(), "split.jsonc", ORIGINAL);
    let host: SharedHost = FileHost::new(HostId(290));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    open_ready(&app, &mut cx, host, &path);

    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx), "the JSON document splits");
    });
    settle(&mut cx);
    let (tab, left, right) = app.read_with(&cx, |app, _| {
        let code = app.tab_code().expect("editor group");
        let tab = app.tabs.get(app.tabs.active_index()).unwrap().id();
        let right = code.focused_group();
        let left = match code.group == right {
            true => code.split.as_ref().expect("two groups").id,
            false => code.group,
        };
        (tab, left, right)
    });
    let (front, other) = app.read_with(&cx, |app, cx| {
        let file = app.active_buffer(cx).expect("JSON buffer");
        (
            file.presentations.json_existing(tab, right).expect("front"),
            file.presentations.json_existing(tab, left).expect("other"),
        )
    });
    assert_eq!(
        other.read_with(&cx, |view, _| view.mode()),
        JsonMode::Structure
    );
    assert_eq!(
        front.read_with(&cx, |view, _| view.mode()),
        JsonMode::Source,
        "the split starts with the other side of the source"
    );
    toggle_mode(&front, &mut cx);
    assert_eq!(
        front.read_with(&cx, |view, _| view.mode()),
        JsonMode::Structure
    );

    // Folds and selection are presentation-local.
    assert_eq!(row(&other, &mut cx, "$[\"nested\"]").expanded(), Some(true));
    click_row(&other, &mut cx, "$[\"nested\"]");
    assert_eq!(
        row(&other, &mut cx, "$[\"nested\"]").expanded(),
        Some(false)
    );
    assert_eq!(
        row(&front, &mut cx, "$[\"nested\"]").expanded(),
        Some(true),
        "the other presentation keeps its own fold"
    );
    click_row(&other, &mut cx, "$[\"name\"]");
    click_row(&front, &mut cx, "$[\"n\"]");
    assert_ne!(
        other.read_with(&cx, |view, cx| view.selected_path(cx)),
        front.read_with(&cx, |view, cx| view.selected_path(cx))
    );

    // The full-value panel belongs to one presentation.
    double_click_row(&other, &mut cx, "$[\"n\"]");
    assert!(
        other
            .read_with(&cx, |view, _| view.full_value_path())
            .is_some()
    );
    assert!(
        front
            .read_with(&cx, |view, _| view.full_value_path())
            .is_none()
    );
    cx.update(|_, cx| other.update(cx, |view, cx| view.close_full_value(cx)));

    // A source edit in the front presentation updates both structures.
    toggle_mode(&front, &mut cx);
    let text = source_text(&app, &cx);
    let at = text.find("\"first\"").expect("fixture value");
    type_source(&app, &mut cx, at..at + "\"first\"".len(), "\"shared\"");
    until(&mut cx, |cx| is_ready(&front, cx) && is_ready(&other, cx));
    toggle_mode(&front, &mut cx);
    assert_eq!(
        row_value(&other, &mut cx, "$[\"name\"]").as_deref(),
        Some("\"shared\"")
    );
    assert_eq!(
        row_value(&front, &mut cx, "$[\"name\"]").as_deref(),
        Some("\"shared\"")
    );

    // Undo from the other presentation's source reverses the shared history.
    toggle_mode(&other, &mut cx);
    app.update_in(&mut cx, |app, window, cx| {
        other.update(cx, |view, cx| {
            let focus = view.source().read(cx).focus_handle(cx);
            focus.focus(window, cx);
        });
        app.editor_split_follow_focus(window, cx);
    });
    cx.update(|window, cx| window.press(&secondary("z"), cx));
    until(&mut cx, |cx| is_ready(&front, cx) && is_ready(&other, cx));
    assert_eq!(
        row_value(&front, &mut cx, "$[\"name\"]").as_deref(),
        Some("\"first\"")
    );
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.revision()),
        front
            .read_with(&cx, |view, cx| view
                .document()
                .read(cx)
                .installed_revision())
            .expect("both structures describe the reverted revision")
    );
}

/// 树焦点下的普通撤销／重做：树命令就是共享文档的一次 history；键盘不离开
/// 结构树、模式不切换，另一 presentation 的源码与独立保存字节都跟着同一份
/// 文档。
#[gpui_kit::test]
fn tree_focused_undo_redo_reverses_the_tree_command_and_saves_exact_bytes(
    tcx: &mut TestAppContext,
) {
    crate::ui::i18n::set_locale("en");
    let remote = canonical_tempdir();
    let local = canonical_tempdir();
    let remote_file = fixture(remote.path(), "history.jsonc", ORIGINAL);
    let local_file = fixture(
        local.path(),
        "history.jsonc",
        b"LOCAL DECOY MUST STAY UNCHANGED",
    );
    let host = FileHost::mapped(HostId(2906), remote.path());
    let path = PathBuf::from("/history.jsonc");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    open_ready(&app, &mut cx, host.clone(), &path);
    let original_text = source_text(&app, &cx);

    // Split: the focused side keeps the tree, the other side shows the source.
    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx), "the JSON document splits");
    });
    settle(&mut cx);
    let (tab, left, right) = app.read_with(&cx, |app, _| {
        let code = app.tab_code().expect("editor group");
        let tab = app.tabs.get(app.tabs.active_index()).unwrap().id();
        let right = code.focused_group();
        let left = match code.group == right {
            true => code.split.as_ref().expect("two groups").id,
            false => code.group,
        };
        (tab, left, right)
    });
    let (front, other) = app.read_with(&cx, |app, cx| {
        let file = app.active_buffer(cx).expect("JSON buffer");
        (
            file.presentations.json_existing(tab, right).expect("front"),
            file.presentations.json_existing(tab, left).expect("other"),
        )
    });
    toggle_mode(&front, &mut cx);
    toggle_mode(&other, &mut cx);
    assert_eq!(
        front.read_with(&cx, |view, _| view.mode()),
        JsonMode::Structure
    );
    assert_eq!(
        other.read_with(&cx, |view, _| view.mode()),
        JsonMode::Source
    );

    // A tree command in the focused tree is one ordinary source history entry.
    click_row(&front, &mut cx, "$[\"name\"] #2");
    click_button(&front, &mut cx, "json-edit-value");
    replace_panel_text(&front, &mut cx, "structure-edit-value", "\"tree-edited\"");
    apply_panel(&front, &mut cx);
    until(&mut cx, |cx| is_ready(&front, cx) && is_ready(&other, cx));
    let edited_text = source_text(&app, &cx);
    assert!(edited_text.contains("\"tree-edited\""));
    assert_eq!(
        other.read_with(&cx, |view, cx| view.source().read(cx).text().to_owned()),
        edited_text,
        "the other presentation's source follows the tree command"
    );

    // Still the tree, still Structure: plain undo is the shared history.
    let tree_focused = cx.update(|window, cx| {
        front.read_with(cx, |view, cx| {
            view.focus_handle(cx).contains_focused(window, cx)
        })
    });
    assert!(tree_focused, "the tree owns the keyboard after applying");
    cx.update(|window, cx| window.press(&secondary("z"), cx));
    until(&mut cx, |cx| is_ready(&front, cx) && is_ready(&other, cx));
    assert_eq!(
        front.read_with(&cx, |view, _| view.mode()),
        JsonMode::Structure,
        "undo from the tree must not switch to the source mode"
    );
    assert_eq!(source_text(&app, &cx), original_text);
    assert_eq!(
        other.read_with(&cx, |view, cx| view.source().read(cx).text().to_owned()),
        original_text,
        "one undo restores the whole source the other presentation shows"
    );
    assert_eq!(
        row_value(&front, &mut cx, "$[\"name\"] #2").as_deref(),
        Some("\"second\\u4e2d\"")
    );

    // Plain redo, again from the tree.
    cx.update(|window, cx| window.press(&secondary("shift-z"), cx));
    until(&mut cx, |cx| is_ready(&front, cx) && is_ready(&other, cx));
    assert_eq!(
        front.read_with(&cx, |view, _| view.mode()),
        JsonMode::Structure
    );
    assert_eq!(source_text(&app, &cx), edited_text);
    assert_eq!(
        other.read_with(&cx, |view, cx| view.source().read(cx).text().to_owned()),
        edited_text
    );
    assert_eq!(
        row_value(&front, &mut cx, "$[\"name\"] #2").as_deref(),
        Some("\"tree-edited\"")
    );

    // The owning Host's bytes, and an independent reopen of them.
    let expected = replace_bytes(ORIGINAL, b"\"second\\u4e2d\"", b"\"tree-edited\"");
    save(&app, &mut cx);
    assert_eq!(std::fs::read(&remote_file).unwrap(), expected);
    assert_eq!(
        std::fs::read(&local_file).unwrap(),
        b"LOCAL DECOY MUST STAY UNCHANGED"
    );

    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    settle(&mut cx);
    assert!(
        app.read_with(&cx, |app, cx| app.active_buffer(cx).is_some()),
        "closing one group must retain the shared document in the other"
    );
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.editor.read(cx).buffers.is_empty())
    });
    let (_, reopened) = open_ready(&app, &mut cx, host, &path);
    assert_eq!(
        row_value(&reopened, &mut cx, "$[\"name\"] #2").as_deref(),
        Some("\"tree-edited\"")
    );
    assert_eq!(std::fs::read(&remote_file).unwrap(), expected);
}

#[gpui_kit::test]
fn jsonl_records_keep_line_identity_invalid_lines_and_exact_save(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let original = b"\xef\xbb\xbf{\"id\": 1, \"name\": \"one\"}\r\n{\"id\": 2, \"name\": \"two\"}\n{\"id\": 3,}\n{\"id\": 4, \"name\": \"four\"}\r\n";
    let path = fixture(dir.path(), "records.jsonl", original);
    let host: SharedHost = FileHost::new(HostId(291));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (_, view) = open_ready(&app, &mut cx, host, &path);
    assert_eq!(
        view.read_with(&cx, |view, _| view.mode()),
        JsonMode::Structure
    );
    assert!(
        maybe_row(&view, &mut cx, "$").is_none(),
        "JSONL is not projected as one JSON array"
    );
    assert_eq!(
        row_value(&view, &mut cx, "JSONL").as_deref(),
        Some("4 records")
    );
    assert_eq!(
        view.read_with(&cx, |view, cx| view.record_count(cx)),
        Some(4)
    );
    assert_eq!(view.read_with(&cx, |view, cx| view.invalid_records(cx)), 1);
    cx.update(|window, _| {
        assert!(window.find("status-json-invalid").visible());
        assert!(window.find("status-json-count").visible());
    });
    assert_eq!(
        row_value(&view, &mut cx, "line 1").as_deref(),
        Some("{ 2 keys }")
    );
    assert_eq!(
        row_value(&view, &mut cx, "line 2").as_deref(),
        Some("{ 2 keys }")
    );
    assert_eq!(
        row_value(&view, &mut cx, "line 3").as_deref(),
        Some("Trailing commas are not allowed")
    );

    // Record selection is line-based: the second record's exact line is
    // revealed, not an array element.
    click_row(&view, &mut cx, "line 2");
    reveal_selected(&app, &mut cx);
    let text = source_text(&app, &cx);
    let range = view.read_with(&cx, |view, cx| view.source().read(cx).selected_range());
    assert_eq!(&text[range], "{\"id\": 2, \"name\": \"two\"}");

    // Editing one record keeps every other line's bytes.
    let at = text.find("\"two\"").expect("record value");
    type_source(&app, &mut cx, at..at + "\"two\"".len(), "\"changed\"");
    until(&mut cx, |cx| is_ready(&view, cx));
    toggle_mode(&view, &mut cx);
    // Record folds are presentation state and survive the source edit.
    click_row(&view, &mut cx, "line 2");
    assert_eq!(
        row_value(&view, &mut cx, "line 2[\"name\"]").as_deref(),
        Some("\"changed\"")
    );
    save(&app, &mut cx);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        replace_bytes(original, b"\"two\"", b"\"changed\"")
    );

    // Repair the invalid line; other records stay visible while it is broken
    // and become valid without touching their own lines.
    toggle_mode(&view, &mut cx);
    let text = source_text(&app, &cx);
    let at = text.find("{\"id\": 3,}").expect("invalid record");
    type_source(&app, &mut cx, at..at + "{\"id\": 3,}".len(), "{\"id\": 3}");
    until(&mut cx, |cx| is_ready(&view, cx));
    toggle_mode(&view, &mut cx);
    assert_eq!(view.read_with(&cx, |view, cx| view.invalid_records(cx)), 0);
    assert_eq!(
        row_value(&view, &mut cx, "line 3").as_deref(),
        Some("{ 1 keys }")
    );
    assert!(row_value(&view, &mut cx, "line 1").is_some());
    assert!(row_value(&view, &mut cx, "line 4").is_some());
    save(&app, &mut cx);
    let expected = replace_bytes(
        &replace_bytes(original, b"\"two\"", b"\"changed\""),
        b"{\"id\": 3,}",
        b"{\"id\": 3}",
    );
    assert_eq!(std::fs::read(&path).unwrap(), expected);
}

#[gpui_kit::test]
fn invalid_budget_and_late_results_preserve_source_and_never_publish_stale_structure(
    tcx: &mut TestAppContext,
) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let host: SharedHost = FileHost::new(HostId(292));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));

    // Invalid JSON: the structure reports the parse error, source stays whole
    // and editable, and a repair installs the tree again.
    let broken_bytes = b"\xef\xbb\xbf{\r\n  \"a\": 1,\n  \"b\": \r\n}\n";
    let broken = fixture(dir.path(), "broken.json", broken_bytes);
    let _ = open(&app, &mut cx, host.clone(), &broken);
    let broken_view = json_view(&app, &cx);
    until(&mut cx, |cx| {
        broken_view.read_with(cx, |view, cx| view.fatal_error(cx).is_some())
    });
    cx.update(|window, _| {
        assert!(
            window
                .find(("json-structure-error-body", broken_view.entity_id()))
                .visible()
        );
        assert!(
            window
                .find(("json-structure-error", broken_view.entity_id()))
                .visible()
        );
    });
    app.read_with(&cx, |app, cx| {
        assert!(app.active_buffer(cx).unwrap().input.text().is_some());
        assert!(!app.active_buffer(cx).unwrap().dirty);
    });
    toggle_mode(&broken_view, &mut cx);
    let text = source_text(&app, &cx);
    let at = text.find("\"b\": \n").expect("broken value");
    type_source(&app, &mut cx, at..at + "\"b\": ".len(), "\"b\": 2");
    until(&mut cx, |cx| is_ready(&broken_view, cx));
    toggle_mode(&broken_view, &mut cx);
    assert_eq!(
        row_value(&broken_view, &mut cx, "$[\"b\"]").as_deref(),
        Some("2")
    );
    save(&app, &mut cx);
    assert_eq!(
        std::fs::read(&broken).unwrap(),
        replace_bytes(broken_bytes, b"\"b\": ", b"\"b\": 2")
    );

    // Over the structure input budget: explicit failure, source still opens,
    // edits and saves byte-for-byte.
    let big_bytes = format!(
        "{{\"pad\": \"{}\"}}",
        "x".repeat(projection::STRUCTURE_INPUT_BYTES + 1024)
    )
    .into_bytes();
    let big = fixture(dir.path(), "big.json", &big_bytes);
    let _ = open(&app, &mut cx, host.clone(), &big);
    let big_view = json_view(&app, &cx);
    until(&mut cx, |cx| {
        big_view.read_with(cx, |view, cx| view.fatal_error(cx).is_some())
    });
    assert!(big_view.read_with(&cx, |view, cx| {
        view.fatal_error(cx)
            .is_some_and(|(_, detail)| detail.contains("1 MiB"))
    }));
    toggle_mode(&big_view, &mut cx);
    assert_eq!(source_text(&app, &cx).len(), big_bytes.len());
    save(&app, &mut cx);
    assert_eq!(std::fs::read(&big).unwrap(), big_bytes);

    // A small source with a long ancestor key must not amplify into huge paths.
    let paths_bytes = format!(
        "{{\"{}\":[{}]}}",
        "x".repeat(65_536),
        "0,".repeat(255) + "0"
    )
    .into_bytes();
    let paths = fixture(dir.path(), "long-paths.json", &paths_bytes);
    let _ = open(&app, &mut cx, host.clone(), &paths);
    let paths_view = json_view(&app, &cx);
    until(&mut cx, |cx| {
        paths_view.read_with(cx, |view, cx| view.fatal_error(cx).is_some())
    });
    cx.update(|window, _| {
        assert!(
            window
                .find(("json-structure-error-body", paths_view.entity_id()))
                .visible()
        );
    });
    toggle_mode(&paths_view, &mut cx);
    assert_eq!(source_text(&app, &cx).as_bytes(), paths_bytes);
    save(&app, &mut cx);
    assert_eq!(std::fs::read(&paths).unwrap(), paths_bytes);

    // A paused parse must never publish a stale revision. The edits collapse
    // to the latest revision, and only that projection is installed.
    let late_bytes = b"{\"value\": \"first\"}\n";
    let late = fixture(dir.path(), "late.json", late_bytes);
    let (release, entered) = test_support::pause_next_parse();
    let _ = open(&app, &mut cx, host, &late);
    let late_view = json_view(&app, &cx);
    until(&mut cx, |_| {
        entered.load(std::sync::atomic::Ordering::SeqCst)
    });
    toggle_mode(&late_view, &mut cx);
    let text = source_text(&app, &cx);
    let at = text.find("\"first\"").expect("fixture value");
    type_source(&app, &mut cx, at..at + "\"first\"".len(), "\"second\"");
    let text = source_text(&app, &cx);
    let at = text.find("\"second\"").expect("edited value");
    type_source(&app, &mut cx, at..at + "\"second\"".len(), "\"latest\"");
    let revision = source_document(&app, &cx).read_with(&cx, |document, _| document.revision());
    assert!(source_text(&app, &cx).contains("\"latest\""));
    assert!(
        late_view
            .read_with(&cx, |view, cx| view
                .document()
                .read(cx)
                .installed_revision())
            .is_none()
    );
    release.send(()).unwrap();
    until(&mut cx, |cx| is_ready(&late_view, cx));
    assert_eq!(
        late_view.read_with(&cx, |view, cx| view
            .document()
            .read(cx)
            .installed_revision()),
        Some(revision)
    );
    toggle_mode(&late_view, &mut cx);
    assert_eq!(
        row_value(&late_view, &mut cx, "$[\"value\"]").as_deref(),
        Some("\"latest\"")
    );
}

fn click_button(view: &Entity<JsonView>, cx: &mut VisualTestContext, id: &'static str) {
    cx.update(|window, cx| window.click((id, view.entity_id()), cx));
    settle(cx);
}

fn panel_open(view: &Entity<JsonView>, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, _| {
        window
            .try_find(("structure-edit", view.entity_id()))
            .is_some_and(|panel| panel.visible())
    })
}

fn panel_error(view: &Entity<JsonView>, cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(("structure-edit-error", view.entity_id()))
            .and_then(|error| error.value().map(str::to_owned))
    })
}

fn replace_panel_text(
    view: &Entity<JsonView>,
    cx: &mut VisualTestContext,
    id: &'static str,
    text: &str,
) {
    cx.update(|window, cx| {
        window.click((id, view.entity_id()), cx);
        window.press("secondary-a", cx);
        window.input(text, cx);
    });
    settle(cx);
}

fn apply_panel(view: &Entity<JsonView>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click(("structure-edit-apply", view.entity_id()), cx));
    settle(cx);
}

fn dismiss_save_error(cx: &mut VisualTestContext) {
    // 失败通知会遮住右下角的 Apply；像用户一样先关闭通知，不等待动画碰巧移开。
    until(cx, |cx| {
        cx.update(|window, _| {
            window
                .try_find("notification")
                .is_some_and(|notification| notification.visible())
        })
    });
    cx.update(|window, cx| {
        window.hover("notification", cx);
        window.within("notification").click("close", cx);
    });
    until(cx, |cx| {
        cx.update(|window, _| window.try_find("notification").is_none())
    });
}

fn cancel_panel(view: &Entity<JsonView>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click(("structure-edit-cancel", view.entity_id()), cx));
    settle(cx);
}

#[gpui_kit::test]
fn tree_value_edit_targets_the_selected_duplicate_and_saves_exact_bytes(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let remote = canonical_tempdir();
    let local = canonical_tempdir();
    let remote_file = fixture(remote.path(), "edit.jsonc", ORIGINAL);
    let local_file = fixture(
        local.path(),
        "edit.jsonc",
        b"LOCAL DECOY MUST STAY UNCHANGED",
    );
    let host = FileHost::mapped(HostId(2900), remote.path());
    let path = PathBuf::from("/edit.jsonc");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (_, view) = open_ready(&app, &mut cx, host.clone(), &path);

    // The second duplicate occurrence is the target; its exact source spelling
    // prefills the editor, and the first occurrence is never touched.
    click_row(&view, &mut cx, "$[\"name\"] #2");
    click_button(&view, &mut cx, "json-edit-value");
    assert!(panel_open(&view, &mut cx));
    assert_eq!(
        view.read_with(&cx, |view, cx| view.edit_texts(cx)),
        Some((None, "\"second\\u4e2d\"".to_owned()))
    );
    assert_eq!(
        cx.update(|window, _| window
            .find(("structure-edit-value", view.entity_id()))
            .focused()),
        Some(true),
        "the value field owns the keyboard while the panel is open"
    );
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"tree-edited\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(!panel_open(&view, &mut cx));
    assert_eq!(
        row_value(&view, &mut cx, "$[\"name\"]").as_deref(),
        Some("\"first\"")
    );
    assert_eq!(
        row_value(&view, &mut cx, "$[\"name\"] #2").as_deref(),
        Some("\"tree-edited\"")
    );
    assert_eq!(
        row_value(&view, &mut cx, "$[\"n\"]").as_deref(),
        Some("12345678901234567890.00")
    );
    let text = source_text(&app, &cx);
    assert!(text.contains("// first comment"));
    assert!(text.contains("\"escaped\": \"line\\nbreak\""));

    // The one command is one ordinary history entry and one exact save.
    save(&app, &mut cx);
    let expected = replace_bytes(ORIGINAL, b"\"second\\u4e2d\"", b"\"tree-edited\"");
    assert_eq!(std::fs::read(&remote_file).unwrap(), expected);
    assert_eq!(
        std::fs::read(&local_file).unwrap(),
        b"LOCAL DECOY MUST STAY UNCHANGED"
    );

    // Undo/redo in the shared source presentation reverse the tree command.
    toggle_mode(&view, &mut cx);
    press(&app, &mut cx, &secondary("z"));
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(source_text(&app, &cx).contains("\"second\\u4e2d\""));
    toggle_mode(&view, &mut cx);
    assert_eq!(
        row_value(&view, &mut cx, "$[\"name\"] #2").as_deref(),
        Some("\"second\\u4e2d\"")
    );
    toggle_mode(&view, &mut cx);
    press(&app, &mut cx, &secondary("shift-z"));
    until(&mut cx, |cx| is_ready(&view, cx));
    toggle_mode(&view, &mut cx);
    assert_eq!(
        row_value(&view, &mut cx, "$[\"name\"] #2").as_deref(),
        Some("\"tree-edited\"")
    );
    save(&app, &mut cx);
    assert_eq!(std::fs::read(&remote_file).unwrap(), expected);

    // Independent reopen through the owning Host observes the saved bytes.
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.editor.read(cx).buffers.is_empty())
    });
    let (_, reopened) = open_ready(&app, &mut cx, host, &path);
    assert_eq!(
        row_value(&reopened, &mut cx, "$[\"name\"] #2").as_deref(),
        Some("\"tree-edited\"")
    );
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        expected
    );
}

const EDITABLE: &[u8] = b"{\n  // keep this comment\n  \"empty\": {},\n  \"items\": [],\n  \"text\": \"a\\u0062\",\n  \"exp\": 1e2,\n  \"neg\": -0,\n  \"big\": 184467440737095516170,\n}\n";

#[gpui_kit::test]
fn add_rename_and_remove_keep_jsonc_comments_and_empty_containers(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let path = fixture(dir.path(), "editable.jsonc", EDITABLE);
    let host: SharedHost = FileHost::new(HostId(2901));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (_, view) = open_ready(&app, &mut cx, host, &path);

    // An empty single-line object gains a member without reformatting it.
    click_row(&view, &mut cx, "$[\"empty\"]");
    click_button(&view, &mut cx, "json-add-member");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.edit_texts(cx)),
        Some((Some(String::new()), String::new()))
    );
    cx.update(|window, cx| window.input("x", cx));
    replace_panel_text(&view, &mut cx, "structure-edit-value", "1e2");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(source_text(&app, &cx).contains("\"empty\": {\"x\": 1e2}"));
    assert!(source_text(&app, &cx).contains("// keep this comment"));

    // An empty array gains an element with its exact spelling.
    click_row(&view, &mut cx, "$[\"items\"]");
    click_button(&view, &mut cx, "json-add-member");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.edit_texts(cx)),
        Some((None, String::new()))
    );
    replace_panel_text(&view, &mut cx, "structure-edit-value", "-0");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(source_text(&app, &cx).contains("\"items\": [-0]"));

    // Renaming one key keeps its value escape and every other member.
    click_row(&view, &mut cx, "$[\"text\"]");
    click_button(&view, &mut cx, "json-rename-key");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.edit_texts(cx)),
        Some((None, "text".to_owned()))
    );
    replace_panel_text(&view, &mut cx, "structure-edit-value", "renamed");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    let text = source_text(&app, &cx);
    assert!(text.contains("\"renamed\": \"a\\u0062\""));
    assert!(text.contains("\"empty\": {\"x\": 1e2}"));
    assert!(text.contains("\"items\": [-0]"));
    assert!(text.contains("\"exp\": 1e2"));
    assert!(text.contains("\"neg\": -0"));
    assert!(text.contains("// keep this comment"));

    // Removing the selected member leaves the comment, key order and the
    // exact large literal untouched.
    click_row(&view, &mut cx, "$[\"big\"]");
    click_button(&view, &mut cx, "json-remove-member");
    until(&mut cx, |cx| is_ready(&view, cx));
    let text = source_text(&app, &cx);
    assert!(!text.contains("\"big\""));
    assert!(!text.contains("184467440737095516170"));
    assert!(text.contains("// keep this comment"));
    assert!(
        text.find("\"empty\"").unwrap() < text.find("\"items\"").unwrap(),
        "the retained members keep source order"
    );
    assert!(
        text.find("\"items\"").unwrap() < text.find("\"renamed\"").unwrap(),
        "renaming does not move the member"
    );
    save(&app, &mut cx);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap())
    );
}

#[gpui_kit::test]
fn tree_edits_survive_save_as_failed_writes_conflicts_and_cancelled_close(
    tcx: &mut TestAppContext,
) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let original = fixture(dir.path(), "persist.jsonc", EDITABLE);
    let target = dir.path().join("persisted.jsonc");
    let host = FileHost::new(HostId::LOCAL);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (_, view) = open_ready(&app, &mut cx, host.clone(), &original);
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.editor.read(cx).buffers.watching())
    });

    // A tree edit, then Save As to a new owning-Host target.
    click_row(&view, &mut cx, "$[\"text\"]");
    click_button(&view, &mut cx, "json-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"changed\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    cx.dispatch_action(crate::core::actions::EditorSaveAs);
    settle(&mut cx);
    cx.simulate_new_path_selection(|_| Some(target.clone()));
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.active_buffer(cx)
                .is_some_and(|file| file.path == target && !file.dirty && !file.saving)
        })
    });
    let saved = source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap());
    assert_eq!(std::fs::read(&target).unwrap(), saved);
    assert_eq!(std::fs::read(&original).unwrap(), EDITABLE);

    // A failed owning-Host write leaves the edit dirty and the target bytes
    // unchanged; an ordinary retry then lands the same bytes.
    click_row(&view, &mut cx, "$[\"text\"]");
    click_button(&view, &mut cx, "json-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"retried\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    host.fail_next_write();
    cx.dispatch_action(crate::core::actions::EditorSave);
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.active_buffer(cx)
                .is_some_and(|file| file.dirty && !file.saving)
        })
    });
    assert_eq!(std::fs::read(&target).unwrap(), saved);
    dismiss_save_error(&mut cx);
    save(&app, &mut cx);
    let retried = replace_bytes(&saved, b"\"changed\"", b"\"retried\"");
    assert_eq!(std::fs::read(&target).unwrap(), retried);

    // A dirty external write is a conflict, not a silent replacement. Keep
    // mine and save the selected tree edit to the owning Host.
    click_row(&view, &mut cx, "$[\"text\"]");
    click_button(&view, &mut cx, "json-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"kept\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    let pending_edit = view.read_with(&cx, |view, cx| view.edit_texts(cx));
    assert!(
        pending_edit.is_none(),
        "the tree edit must be committed before the external rewrite: {pending_edit:?}"
    );
    assert!(source_text(&app, &cx).contains("\"kept\""));
    assert!(
        app.read_with(&cx, |app, cx| app
            .active_buffer(cx)
            .is_some_and(|file| file.dirty)),
        "the committed tree edit must be dirty before the external rewrite"
    );
    std::fs::write(&target, b"{\"external\": true}\n").unwrap();
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.active_buffer(cx)
                .is_some_and(|file| file.conflict.is_some())
        })
    });
    assert!(source_text(&app, &cx).contains("\"kept\""));
    cx.update(|window, cx| window.click("editor-conflict-keep", cx));
    settle(&mut cx);
    save(&app, &mut cx);
    let kept = replace_bytes(&retried, b"\"retried\"", b"\"kept\"");
    assert_eq!(std::fs::read(&target).unwrap(), kept);
    settle(&mut cx);
    assert!(
        source_text(&app, &cx).contains("\"kept\""),
        "a stale watcher event must not reload the external content after keep-mine and save"
    );

    // Cancelling an unsaved close keeps the document and its bytes.
    click_row(&view, &mut cx, "$[\"text\"]");
    click_button(&view, &mut cx, "json-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"still-open\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    let panel = view.read_with(&cx, |view, cx| view.edit_texts(cx));
    let error = panel_error(&view, &mut cx);
    assert!(
        panel.is_none(),
        "the tree edit must apply before close; panel={panel:?} error={error:?}"
    );
    assert!(
        source_text(&app, &cx).contains("\"still-open\""),
        "the tree edit must reach the shared source"
    );
    assert!(
        app.read_with(&cx, |app, cx| {
            app.active_buffer(cx).is_some_and(|file| file.dirty)
        }),
        "the applied tree edit leaves the document dirty"
    );
    assert!(
        app.update_in(&mut cx, |app, window, cx| app.editor_has_focus(window, cx)),
        "the applied tree edit returns focus to the editor panel"
    );
    cx.dispatch_action(crate::core::actions::CloseActiveTab);
    settle(&mut cx);
    assert!(
        cx.has_pending_prompt(),
        "an unsaved tree edit must prompt before close"
    );
    cx.simulate_prompt_answer(crate::ui::i18n::t(crate::ui::i18n::L10nKey::Cancel));
    settle(&mut cx);
    assert!(
        app.read_with(&cx, |app, cx| app.active_buffer(cx).is_some()),
        "cancelled close keeps the document"
    );
    assert_eq!(std::fs::read(&target).unwrap(), kept);
    save(&app, &mut cx);
    let still_open = replace_bytes(&kept, b"\"kept\"", b"\"still-open\"");
    assert_eq!(std::fs::read(&target).unwrap(), still_open);

    // An independent reopen sees the final tree-edited bytes.
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.editor.read(cx).buffers.is_empty())
    });
    let (_, reopened) = open_ready(&app, &mut cx, host, &target);
    assert_eq!(
        row_value(&reopened, &mut cx, "$[\"text\"]").as_deref(),
        Some("\"still-open\"")
    );
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        still_open
    );
}

#[gpui_kit::test]
fn stale_and_invalid_source_reject_tree_edits_and_split_keeps_one_history(
    tcx: &mut TestAppContext,
) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let original = fixture(dir.path(), "split-edit.jsonc", EDITABLE);
    let host: SharedHost = FileHost::new(HostId(2903));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    open_ready(&app, &mut cx, host, &original);

    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx), "the JSON document splits");
    });
    settle(&mut cx);
    let (tab, left, right) = app.read_with(&cx, |app, _| {
        let code = app.tab_code().expect("editor group");
        let tab = app.tabs.get(app.tabs.active_index()).unwrap().id();
        let right = code.focused_group();
        let left = match code.group == right {
            true => code.split.as_ref().expect("two groups").id,
            false => code.group,
        };
        (tab, left, right)
    });
    let (front, other) = app.read_with(&cx, |app, cx| {
        let file = app.active_buffer(cx).expect("JSON buffer");
        (
            file.presentations.json_existing(tab, right).expect("front"),
            file.presentations.json_existing(tab, left).expect("other"),
        )
    });
    assert_eq!(
        other.read_with(&cx, |view, _| view.mode()),
        JsonMode::Structure
    );
    assert_eq!(
        front.read_with(&cx, |view, _| view.mode()),
        JsonMode::Source
    );

    // Open a tree command in the structure presentation, then change the
    // source from the other presentation. The pending command must not write.
    click_row(&other, &mut cx, "$[\"text\"]");
    click_button(&other, &mut cx, "json-edit-value");
    assert!(panel_open(&other, &mut cx));
    app.update_in(&mut cx, |app, window, cx| {
        front.update(cx, |view, cx| {
            let focus = view.source().read(cx).focus_handle(cx);
            focus.focus(window, cx);
        });
        app.editor_split_follow_focus(window, cx);
    });
    let text = source_text(&app, &cx);
    let at = text.find("\"a\\u0062\"").expect("fixture value");
    // 同长度修改保留 path 与 token range；旧面板仍必须按打开时的 revision 拒绝。
    type_source(&app, &mut cx, at..at + "\"a\\u0062\"".len(), "\"changed\"");
    until(&mut cx, |cx| is_ready(&front, cx) && is_ready(&other, cx));
    assert_eq!(
        panel_error(&other, &mut cx).as_deref(),
        Some(
            "The document changed, so this edit was not written. Close this panel and select the node again."
        )
    );
    apply_panel(&other, &mut cx);
    let text = source_text(&app, &cx);
    assert!(text.contains("\"changed\""));
    assert!(!text.contains("\"a\\u0062\""));
    assert!(
        panel_open(&other, &mut cx),
        "a stale command stays open for a decision"
    );
    cancel_panel(&other, &mut cx);
    assert!(!panel_open(&other, &mut cx));

    // Make the source invalid from the source presentation: the structure
    // view reports the error and a pending tree command cannot write guesses.
    click_row(&other, &mut cx, "$[\"text\"]");
    click_button(&other, &mut cx, "json-edit-value");
    app.update_in(&mut cx, |app, window, cx| {
        front.update(cx, |view, cx| {
            let focus = view.source().read(cx).focus_handle(cx);
            focus.focus(window, cx);
        });
        app.editor_split_follow_focus(window, cx);
    });
    let text = source_text(&app, &cx);
    let at = text.find("\"changed\"").expect("edited value");
    type_source(
        &app,
        &mut cx,
        at..at + "\"changed\"".len(),
        "\"unterminated",
    );
    until(&mut cx, |cx| {
        other.read_with(cx, |view, cx| view.fatal_error(cx).is_some())
    });
    apply_panel(&other, &mut cx);
    assert!(panel_error(&other, &mut cx).is_some());
    assert!(source_text(&app, &cx).contains("\"unterminated"));
    assert_eq!(std::fs::read(&original).unwrap(), EDITABLE);
    cancel_panel(&other, &mut cx);

    // Repairing the source lets a fresh tree command land in the one shared
    // history, and undo from the source presentation reverses it.
    app.update_in(&mut cx, |app, window, cx| {
        front.update(cx, |view, cx| {
            let focus = view.source().read(cx).focus_handle(cx);
            focus.focus(window, cx);
        });
        app.editor_split_follow_focus(window, cx);
    });
    let text = source_text(&app, &cx);
    let at = text.find("\"unterminated").expect("broken value");
    type_source(
        &app,
        &mut cx,
        at..at + "\"unterminated".len(),
        "\"repaired\"",
    );
    until(&mut cx, |cx| is_ready(&front, cx) && is_ready(&other, cx));
    click_row(&other, &mut cx, "$[\"text\"]");
    click_button(&other, &mut cx, "json-edit-value");
    replace_panel_text(&other, &mut cx, "structure-edit-value", "\"tree-final\"");
    apply_panel(&other, &mut cx);
    until(&mut cx, |cx| is_ready(&other, cx));
    assert!(source_text(&app, &cx).contains("\"tree-final\""));
    app.update_in(&mut cx, |app, window, cx| {
        front.update(cx, |view, cx| {
            let focus = view.source().read(cx).focus_handle(cx);
            focus.focus(window, cx);
        });
        app.editor_split_follow_focus(window, cx);
    });
    cx.update(|window, cx| window.press(&secondary("z"), cx));
    until(&mut cx, |cx| is_ready(&front, cx) && is_ready(&other, cx));
    assert!(source_text(&app, &cx).contains("\"repaired\""));
    assert!(!source_text(&app, &cx).contains("\"tree-final\""));

    // Closing the document while a panel is open drops the callback and
    // leaves the owning Host bytes untouched. Closing one presentation is the
    // existing split contract: the other presentation still owns the buffer,
    // and only closing the last one retires the document.
    save(&app, &mut cx);
    let saved = std::fs::read(&original).unwrap();
    click_row(&other, &mut cx, "$[\"text\"]");
    click_button(&other, &mut cx, "json-edit-value");
    assert!(panel_open(&other, &mut cx));
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    settle(&mut cx);
    assert!(
        app.read_with(&cx, |app, cx| {
            let group = app.tab_code().map(|code| code.group);
            app.active_buffer(cx).is_some_and(|file| {
                group.is_some_and(|group| file.presentations.json_existing(tab, group).is_some())
            })
        }),
        "the other presentation still owns the document"
    );
    assert!(
        app.read_with(&cx, |app, cx| !app.editor.read(cx).buffers.is_empty()),
        "one presentation close does not retire the shared document"
    );
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.editor.read(cx).buffers.is_empty())
    });
    assert_eq!(std::fs::read(&original).unwrap(), saved);
}

#[gpui_kit::test]
fn jsonl_records_edit_values_keys_arrays_and_add_remove_lines(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let remote = canonical_tempdir();
    let local = canonical_tempdir();
    let original: &[u8] = b"\xef\xbb\xbf{\"id\": 1, \"name\": \"one\", \"big\": 184467440737095516170, \"dup\": 1, \"dup\": 2}\r\n\n{\"id\": 2, \"items\": [1e2, -0, \"\\u4e2d\"]}\n{\"id\": 3,}\r\n42\r\n";
    let remote_file = fixture(remote.path(), "records.jsonl", original);
    let local_file = fixture(
        local.path(),
        "records.jsonl",
        b"LOCAL DECOY MUST STAY UNCHANGED",
    );
    let host = FileHost::mapped(HostId(2910), remote.path());
    let path = PathBuf::from("/records.jsonl");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (_, view) = open_ready(&app, &mut cx, host.clone(), &path);

    assert_eq!(
        view.read_with(&cx, |view, _| view.mode()),
        JsonMode::Structure
    );
    assert_eq!(
        view.read_with(&cx, |view, cx| view.record_count(cx)),
        Some(4)
    );
    assert_eq!(view.read_with(&cx, |view, cx| view.invalid_records(cx)), 1);
    assert_eq!(
        row_value(&view, &mut cx, "JSONL").as_deref(),
        Some("4 records")
    );
    assert_eq!(
        row_value(&view, &mut cx, "line 1[\"dup\"] #2").as_deref(),
        Some("2")
    );
    assert_eq!(
        row_value(&view, &mut cx, "line 3[\"items\"]").as_deref(),
        Some("[ 3 items ]"),
        "the invalid line is a separate record, not an array element"
    );
    assert_eq!(
        row_value(&view, &mut cx, "line 5").as_deref(),
        Some("42"),
        "a scalar record is one real record line"
    );
    cx.update(|window, _| {
        for id in [
            "json-edit-value",
            "json-rename-key",
            "json-add-member",
            "json-remove-member",
            "json-add-record",
            "json-remove-record",
        ] {
            assert!(
                window.find((id, view.entity_id())).visible(),
                "JSONL record editing must expose {id}"
            );
        }
    });

    // A duplicate-key occurrence is renamed and edited without touching its
    // sibling, the exact large literal or the escape spelling on other lines.
    click_row(&view, &mut cx, "line 1[\"dup\"] #2");
    click_button(&view, &mut cx, "json-edit-value");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.edit_texts(cx)),
        Some((None, "2".to_owned()))
    );
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"中文\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert_eq!(
        row_value(&view, &mut cx, "line 1[\"dup\"]").as_deref(),
        Some("1")
    );
    assert_eq!(
        row_value(&view, &mut cx, "line 1[\"dup\"] #2").as_deref(),
        Some("\"中文\"")
    );
    assert!(
        source_text(&app, &cx).contains("\"big\": 184467440737095516170"),
        "the unrelated exact number is untouched"
    );

    click_row(&view, &mut cx, "line 3[\"id\"]");
    click_button(&view, &mut cx, "json-rename-key");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.edit_texts(cx)),
        Some((None, "id".to_owned()))
    );
    replace_panel_text(&view, &mut cx, "structure-edit-value", "编号");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert_eq!(
        row_value(&view, &mut cx, "line 3[\"编号\"]").as_deref(),
        Some("2")
    );
    assert!(
        source_text(&app, &cx).contains("\"items\": [1e2, -0, \"\\u4e2d\"]"),
        "renaming one key leaves array spelling and escapes in its record"
    );

    // An array inside a record gains a Unicode item; the record stays one line.
    click_row(&view, &mut cx, "line 3[\"items\"]");
    click_button(&view, &mut cx, "json-add-member");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"追加\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(source_text(&app, &cx).contains("\"items\": [1e2, -0, \"\\u4e2d\", \"追加\"]"));

    // Add a whole record after line 1. The blank line and every later
    // physical line remain; only the new record line is inserted.
    click_row(&view, &mut cx, "line 1");
    click_button(&view, &mut cx, "json-add-record");
    assert!(panel_open(&view, &mut cx));
    replace_panel_text(
        &view,
        &mut cx,
        "structure-edit-value",
        "{\"id\": 9, \"name\": \"你好🙂\"}",
    );
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert_eq!(
        view.read_with(&cx, |view, cx| view.record_count(cx)),
        Some(5)
    );
    assert_eq!(
        row_value(&view, &mut cx, "line 2").as_deref(),
        Some("{ 2 keys }")
    );
    assert_eq!(view.read_with(&cx, |view, cx| view.invalid_records(cx)), 1);

    // The invalid line is still a selectable record and can be removed as a
    // physical line without parsing or rewriting its neighbors.
    click_row(&view, &mut cx, "line 5");
    click_button(&view, &mut cx, "json-remove-record");
    until(&mut cx, |cx| is_ready(&view, cx));
    assert_eq!(view.read_with(&cx, |view, cx| view.invalid_records(cx)), 0);
    assert_eq!(
        view.read_with(&cx, |view, cx| view.record_count(cx)),
        Some(4)
    );
    assert_eq!(
        row_value(&view, &mut cx, "line 5").as_deref(),
        Some("42"),
        "the record after the deleted line keeps its own bytes and moves up one line"
    );

    // A scalar record itself can be replaced as one line.
    click_row(&view, &mut cx, "line 5");
    click_button(&view, &mut cx, "json-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "43");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert_eq!(row_value(&view, &mut cx, "line 5").as_deref(), Some("43"));

    // The record command is ordinary shared history: the source presentation
    // undoes and redoes it without a second editable copy.
    toggle_mode(&view, &mut cx);
    press(&app, &mut cx, &secondary("z"));
    until(&mut cx, |cx| is_ready(&view, cx));
    toggle_mode(&view, &mut cx);
    assert_eq!(row_value(&view, &mut cx, "line 5").as_deref(), Some("42"));
    toggle_mode(&view, &mut cx);
    press(&app, &mut cx, &secondary("shift-z"));
    until(&mut cx, |cx| is_ready(&view, cx));
    toggle_mode(&view, &mut cx);
    assert_eq!(row_value(&view, &mut cx, "line 5").as_deref(), Some("43"));

    save(&app, &mut cx);
    let expected = replace_bytes(original, b"\"dup\": 2", "\"dup\": \"中文\"".as_bytes());
    let expected = replace_bytes(&expected, b"\"id\": 2", "\"编号\": 2".as_bytes());
    let expected = replace_bytes(
        &expected,
        b"\"\\u4e2d\"]",
        b"\"\\u4e2d\", \"\xe8\xbf\xbd\xe5\x8a\xa0\"]",
    );
    let marker = "\"dup\": \"中文\"}\r\n";
    let replacement = "\"dup\": \"中文\"}\r\n{\"id\": 9, \"name\": \"你好🙂\"}\r\n";
    let expected = replace_bytes(&expected, marker.as_bytes(), replacement.as_bytes());
    let expected = replace_bytes(&expected, b"{\"id\": 3,}\r\n", b"");
    let expected = replace_bytes(&expected, b"\n42\r\n", b"\n43\r\n");
    assert_eq!(
        std::fs::read(&remote_file).unwrap(),
        expected,
        "every untouched record keeps its own bytes and mixed terminator"
    );
    assert_eq!(
        std::fs::read(&local_file).unwrap(),
        b"LOCAL DECOY MUST STAY UNCHANGED",
        "the same path on this machine was never read or written"
    );

    // Independent reopen through the owning Host sees the complete stream.
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.editor.read(cx).buffers.is_empty())
    });
    let (_, reopened) = open_ready(&app, &mut cx, host, &path);
    assert_eq!(
        reopened.read_with(&cx, |view, cx| view.record_count(cx)),
        Some(4)
    );
    assert_eq!(
        reopened.read_with(&cx, |view, cx| view.invalid_records(cx)),
        0
    );
    assert_eq!(
        row_value(&reopened, &mut cx, "line 1[\"dup\"] #2").as_deref(),
        Some("\"中文\"")
    );
    assert_eq!(
        row_value(&reopened, &mut cx, "line 2[\"name\"]").as_deref(),
        Some("\"你好🙂\"")
    );
    assert_eq!(
        row_value(&reopened, &mut cx, "line 4[\"items\"]").as_deref(),
        Some("[ 4 items ]")
    );
    assert_eq!(
        row_value(&reopened, &mut cx, "line 5").as_deref(),
        Some("43")
    );
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        expected
    );
}

#[gpui_kit::test]
fn jsonl_record_edits_share_history_save_as_conflict_and_cancelled_close(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let original_bytes = b"{\"id\":1,\"name\":\"one\"}\n{\"id\":2,\"name\":\"two\"}\n";
    let original = fixture(dir.path(), "lifecycle.ndjson", original_bytes);
    let target = dir.path().join("persisted.ndjson");
    let host = FileHost::new(HostId::LOCAL);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (_, view) = open_ready(&app, &mut cx, host.clone(), &original);
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.editor.read(cx).buffers.watching())
    });

    // A record insertion, then Save As to another owning-Host path.
    click_row(&view, &mut cx, "line 2");
    click_button(&view, &mut cx, "json-add-record");
    replace_panel_text(
        &view,
        &mut cx,
        "structure-edit-value",
        "{\"id\":3,\"name\":\"三\"}",
    );
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert_eq!(
        view.read_with(&cx, |view, cx| view.record_count(cx)),
        Some(3)
    );
    cx.dispatch_action(crate::core::actions::EditorSaveAs);
    settle(&mut cx);
    cx.simulate_new_path_selection(|_| Some(target.clone()));
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.active_buffer(cx)
                .is_some_and(|file| file.path == target && !file.dirty && !file.saving)
        })
    });
    let saved = source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap());
    assert_eq!(std::fs::read(&target).unwrap(), saved);
    assert_eq!(std::fs::read(&original).unwrap(), original_bytes);

    // A failed owning-Host write leaves the record edit dirty and the target
    // bytes unchanged; an ordinary retry lands the same bytes.
    click_row(&view, &mut cx, "line 1[\"name\"]");
    click_button(&view, &mut cx, "json-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"失败\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    host.fail_next_write();
    cx.dispatch_action(crate::core::actions::EditorSave);
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.active_buffer(cx)
                .is_some_and(|file| file.dirty && !file.saving)
        })
    });
    assert_eq!(std::fs::read(&target).unwrap(), saved);
    dismiss_save_error(&mut cx);
    save(&app, &mut cx);
    let failed = replace_bytes(&saved, b"\"name\":\"one\"", "\"name\":\"失败\"".as_bytes());
    assert_eq!(std::fs::read(&target).unwrap(), failed);

    // A dirty external write is a conflict, not a silent replacement. Keep
    // mine and save the pending record edit to the owning Host.
    click_row(&view, &mut cx, "line 2[\"name\"]");
    click_button(&view, &mut cx, "json-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"保留\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    let pending_edit = view.read_with(&cx, |view, cx| view.edit_texts(cx));
    assert!(
        pending_edit.is_none(),
        "the record edit must be committed before the external rewrite: {pending_edit:?}"
    );
    assert!(source_text(&app, &cx).contains("\"保留\""));
    assert!(
        app.read_with(&cx, |app, cx| app
            .active_buffer(cx)
            .is_some_and(|file| file.dirty)),
        "the committed record edit must be dirty before the external rewrite"
    );
    std::fs::write(&target, b"{\"external\":true}\n").unwrap();
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.active_buffer(cx)
                .is_some_and(|file| file.conflict.is_some())
        })
    });
    assert!(source_text(&app, &cx).contains("\"保留\""));
    cx.update(|window, cx| window.click("editor-conflict-keep", cx));
    settle(&mut cx);
    save(&app, &mut cx);
    let kept = replace_bytes(&failed, b"\"name\":\"two\"", "\"name\":\"保留\"".as_bytes());
    assert_eq!(std::fs::read(&target).unwrap(), kept);

    // Cancelling an unsaved close keeps the document and the pending record.
    click_row(&view, &mut cx, "line 3[\"name\"]");
    click_button(&view, &mut cx, "json-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"关闭\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    let panel = view.read_with(&cx, |view, cx| view.edit_texts(cx));
    let error = panel_error(&view, &mut cx);
    assert!(
        panel.is_none(),
        "the record edit must apply before close; panel={panel:?} error={error:?}"
    );
    assert!(
        source_text(&app, &cx).contains("\"关闭\""),
        "the record edit must reach the shared source"
    );
    cx.dispatch_action(crate::core::actions::CloseActiveTab);
    settle(&mut cx);
    assert!(
        cx.has_pending_prompt(),
        "an unsaved record edit must prompt before close"
    );
    cx.simulate_prompt_answer(crate::ui::i18n::t(crate::ui::i18n::L10nKey::Cancel));
    settle(&mut cx);
    assert!(
        app.read_with(&cx, |app, cx| app.active_buffer(cx).is_some()),
        "cancelled close keeps the record document"
    );
    assert_eq!(std::fs::read(&target).unwrap(), kept);
    save(&app, &mut cx);
    let closed = replace_bytes(
        &kept,
        "\"name\":\"三\"".as_bytes(),
        "\"name\":\"关闭\"".as_bytes(),
    );
    assert_eq!(std::fs::read(&target).unwrap(), closed);

    // An independent reopen verifies the full record stream.
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.editor.read(cx).buffers.is_empty())
    });
    let (_, reopened) = open_ready(&app, &mut cx, host, &target);
    assert_eq!(
        reopened.read_with(&cx, |view, cx| view.record_count(cx)),
        Some(3)
    );
    assert_eq!(
        reopened.read_with(&cx, |view, cx| view.invalid_records(cx)),
        0
    );
    assert_eq!(
        row_value(&reopened, &mut cx, "line 1[\"name\"]").as_deref(),
        Some("\"失败\"")
    );
    assert_eq!(
        row_value(&reopened, &mut cx, "line 2[\"name\"]").as_deref(),
        Some("\"保留\"")
    );
    assert_eq!(
        row_value(&reopened, &mut cx, "line 3[\"name\"]").as_deref(),
        Some("\"关闭\"")
    );
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        closed
    );
}

#[gpui_kit::test]
fn jsonl_record_budget_is_explicit_and_source_stays_editable(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let host: SharedHost = FileHost::new(HostId(2912));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let at_budget_bytes = "0\n".repeat(projection::STRUCTURE_RECORDS).into_bytes();
    let at_budget = fixture(dir.path(), "at-budget.jsonl", &at_budget_bytes);
    let (_, view) = open_ready(&app, &mut cx, host.clone(), &at_budget);
    assert_eq!(
        view.read_with(&cx, |view, cx| view.record_count(cx)),
        Some(projection::STRUCTURE_RECORDS)
    );
    assert!(view.read_with(&cx, |view, cx| view.structure_ready(cx)));
    press(&app, &mut cx, "end");
    let last_record = format!("line {}", projection::STRUCTURE_RECORDS);
    assert_eq!(
        view.read_with(&cx, |view, cx| view.selected_path(cx))
            .as_deref(),
        Some(last_record.as_str())
    );
    save(&app, &mut cx);
    assert_eq!(std::fs::read(&at_budget).unwrap(), at_budget_bytes);
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.editor.read(cx).buffers.is_empty())
    });

    // One more record exceeds the record budget before the node budget; the
    // source remains whole, editable and byte-exact.
    let over_bytes = "0\n".repeat(projection::STRUCTURE_RECORDS + 1).into_bytes();
    let over = fixture(dir.path(), "over-budget.jsonl", &over_bytes);
    let _ = open(&app, &mut cx, host, &over);
    let over_view = json_view(&app, &cx);
    until(&mut cx, |cx| {
        over_view.read_with(cx, |view, cx| view.fatal_error(cx).is_some())
    });
    assert!(over_view.read_with(&cx, |view, cx| {
        view.fatal_error(cx).is_some_and(|(_, detail)| {
            detail.contains(projection::STRUCTURE_RECORDS.to_string().as_str())
        })
    }));
    toggle_mode(&over_view, &mut cx);
    assert_eq!(source_text(&app, &cx).as_bytes(), over_bytes);
    save(&app, &mut cx);
    assert_eq!(std::fs::read(&over).unwrap(), over_bytes);
}

#[gpui_kit::test]
fn jsonl_late_parse_and_stale_record_actions_never_write(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let original = b"{\"id\":1,\"name\":\"first\"}\n{\"id\":2,\"name\":\"two\"}\n";
    let path = fixture(dir.path(), "late.jsonl", original);
    let host: SharedHost = FileHost::new(HostId(2913));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (release, entered) = test_support::pause_next_parse();
    let _ = open(&app, &mut cx, host, &path);
    let view = json_view(&app, &cx);
    until(&mut cx, |_| {
        entered.load(std::sync::atomic::Ordering::SeqCst)
    });
    toggle_mode(&view, &mut cx);
    let text = source_text(&app, &cx);
    let at = text.find("\"first\"").expect("fixture value");
    type_source(&app, &mut cx, at..at + "\"first\"".len(), "\"mid\"");
    let text = source_text(&app, &cx);
    let at = text.find("\"mid\"").expect("edited value");
    type_source(&app, &mut cx, at..at + "\"mid\"".len(), "\"latest\"");
    let revision = source_document(&app, &cx).read_with(&cx, |document, _| document.revision());
    assert!(
        view.read_with(&cx, |view, cx| view
            .document()
            .read(cx)
            .installed_revision())
            .is_none(),
        "no stale projection may be installed while the newer edit is pending"
    );
    release.send(()).unwrap();
    until(&mut cx, |cx| is_ready(&view, cx));
    assert_eq!(
        view.read_with(&cx, |view, cx| view
            .document()
            .read(cx)
            .installed_revision()),
        Some(revision)
    );
    toggle_mode(&view, &mut cx);
    assert_eq!(
        row_value(&view, &mut cx, "line 1[\"name\"]").as_deref(),
        Some("\"latest\"")
    );

    // Open a record command in one presentation, then change the shared
    // source from the other. The stale command must not write.
    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx), "the JSONL document splits");
    });
    settle(&mut cx);
    let (tab, left, right) = app.read_with(&cx, |app, _| {
        let code = app.tab_code().expect("editor group");
        let tab = app.tabs.get(app.tabs.active_index()).unwrap().id();
        let right = code.focused_group();
        let left = match code.group == right {
            true => code.split.as_ref().expect("two groups").id,
            false => code.group,
        };
        (tab, left, right)
    });
    let (front, other) = app.read_with(&cx, |app, cx| {
        let file = app.active_buffer(cx).expect("JSONL buffer");
        (
            file.presentations.json_existing(tab, right).expect("front"),
            file.presentations.json_existing(tab, left).expect("other"),
        )
    });
    assert_eq!(
        other.read_with(&cx, |view, _| view.mode()),
        JsonMode::Structure
    );
    assert_eq!(
        front.read_with(&cx, |view, _| view.mode()),
        JsonMode::Source
    );
    click_row(&other, &mut cx, "line 2");
    click_button(&other, &mut cx, "json-add-record");
    assert!(panel_open(&other, &mut cx));
    app.update_in(&mut cx, |app, window, cx| {
        front.update(cx, |view, cx| {
            let focus = view.source().read(cx).focus_handle(cx);
            focus.focus(window, cx);
        });
        app.editor_split_follow_focus(window, cx);
    });
    let text = source_text(&app, &cx);
    let at = text.find("\"latest\"").expect("edited value");
    type_source(
        &app,
        &mut cx,
        at..at + "\"latest\"".len(),
        "\"source-wins\"",
    );
    until(&mut cx, |cx| is_ready(&front, cx) && is_ready(&other, cx));
    assert_eq!(
        panel_error(&other, &mut cx).as_deref(),
        Some(crate::ui::i18n::t(
            crate::ui::i18n::L10nKey::EditorJsonEditStale
        ))
    );
    apply_panel(&other, &mut cx);
    let text = source_text(&app, &cx);
    assert!(text.contains("\"source-wins\""));
    assert!(
        text.lines().count() == 2,
        "the stale add-record command must not insert a third line"
    );
    assert!(
        panel_open(&other, &mut cx),
        "a stale record command stays open for a decision"
    );
    cancel_panel(&other, &mut cx);
    assert!(!panel_open(&other, &mut cx));
    save(&app, &mut cx);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        replace_bytes(original, b"\"first\"", b"\"source-wins\"")
    );
}

#[gpui_kit::test]
#[ignore = "requires the explicit private Linux SSH fixture, a matching same-commit local router and the running private remote daemon; run alone with --ignored"]
fn tree_edits_save_through_the_real_native_ssh_route(tcx: &mut TestAppContext) {
    use lyrel_core::host::Host as _;

    crate::ui::i18n::set_locale("en");
    let binary = std::env::var_os("LYREL_TEST_SSH_LOCAL_SERVER").expect(
        "native SSH prerequisite: LYREL_TEST_SSH_LOCAL_SERVER must name the same-commit desktop router binary; also provide the explicit LYREL_TEST_SSH_* fixture endpoint",
    );
    let fixture = crate::native_ssh_graphics_fixture::NativeSshFixture::start(Path::new(&binary));
    let host = fixture.connect_control();
    let script = b"{\n  // remote comment\n  \"dup\": 1,\n  \"dup\": 2,\n  \"big\": 184467440737095516170,\n}\n";
    let _ = fixture.install_script(&host, "editing.jsonc", script, Vec::new());
    let path = fixture.remote_path("editing.jsonc");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (_, view) = open_ready(&app, &mut cx, host, &path);

    click_row(&view, &mut cx, "$[\"dup\"] #2");
    click_button(&view, &mut cx, "json-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"remote-edited\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    save(&app, &mut cx);

    let expected = replace_bytes(script, b"\"dup\": 2", b"\"dup\": \"remote-edited\"");
    // A second production control client proves the owning Host's bytes.
    let verifier = fixture.connect_control_as(
        "fixture:native-ssh-290-verify",
        "native-ssh-json-290",
        "isolated-fixture",
    );
    let bytes = verifier
        .read_file(&path, expected.len() as u64 + 1)
        .expect("independent remote read");
    eprintln!(
        "json290 real native SSH saved {} bytes at {}",
        bytes.len(),
        path.display()
    );
    assert_eq!(bytes, expected);
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        expected
    );
}

#[gpui_kit::test]
#[ignore = "requires the explicit private Linux SSH fixture, a matching same-commit local router and the running private remote daemon; run alone with --ignored"]
fn jsonl_record_edits_save_through_the_real_native_ssh_route(tcx: &mut TestAppContext) {
    use lyrel_core::host::Host as _;

    crate::ui::i18n::set_locale("en");
    let binary = std::env::var_os("LYREL_TEST_SSH_LOCAL_SERVER").expect(
        "native SSH prerequisite: LYREL_TEST_SSH_LOCAL_SERVER must name the same-commit desktop router binary; also provide the explicit LYREL_TEST_SSH_* fixture endpoint",
    );
    let fixture = crate::native_ssh_graphics_fixture::NativeSshFixture::start(Path::new(&binary));
    let host = fixture.connect_control();
    let script = b"{\"id\": 1, \"name\": \"one\"}\n{\"id\": 2, \"big\": 184467440737095516170}\n";
    let _ = fixture.install_script(&host, "records.jsonl", script, Vec::new());
    let path = fixture.remote_path("records.jsonl");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (_, view) = open_ready(&app, &mut cx, host, &path);

    click_row(&view, &mut cx, "line 2[\"big\"]");
    click_button(&view, &mut cx, "json-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "1e400");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    click_row(&view, &mut cx, "line 1");
    click_button(&view, &mut cx, "json-add-record");
    replace_panel_text(
        &view,
        &mut cx,
        "structure-edit-value",
        "{\"id\": 3, \"name\": \"远端\"}",
    );
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    save(&app, &mut cx);

    let expected = replace_bytes(script, b"184467440737095516170", b"1e400");
    let expected = replace_bytes(
        &expected,
        b"{\"id\": 1, \"name\": \"one\"}\n",
        "{\"id\": 1, \"name\": \"one\"}\n{\"id\": 3, \"name\": \"远端\"}\n".as_bytes(),
    );
    let verifier = fixture.connect_control_as(
        "fixture:native-ssh-291-verify",
        "native-ssh-jsonl-291",
        "isolated-fixture",
    );
    let bytes = verifier
        .read_file(&path, expected.len() as u64 + 1)
        .expect("independent remote read");
    eprintln!(
        "jsonl291 real native SSH saved {} bytes at {}",
        bytes.len(),
        path.display()
    );
    assert_eq!(bytes, expected);
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        expected
    );
}
