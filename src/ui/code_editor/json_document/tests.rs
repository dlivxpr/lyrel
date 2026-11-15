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
    app.read_with(cx, |app, _| {
        app.active_editor_input()
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
        app.read_with(cx, |app, _| {
            app.active_buffer()
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
        app.read_with(cx, |app, _| {
            app.active_buffer()
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
        let file = app.editor.buffers.get(id).expect("installed document");
        assert!(!file.dirty, "an untouched JSONC opens clean");
        let format = file.format(cx).expect("source format");
        assert!(format.bom, "the UTF-8 BOM is part of the saved bytes");
        assert_eq!(format.encoding, encoding_rs::UTF_8);
        assert!(
            app.editor.local_file(id).is_none(),
            "native JSON is not a code language-service file"
        );
        assert!(app.json_document_active());
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
        app.read_with(cx, |app, _| app.editor.buffers.is_empty())
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
    let (front, other) = app.read_with(&cx, |app, _| {
        let file = app.active_buffer().expect("JSON buffer");
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
    app.read_with(&cx, |app, _| {
        assert!(app.active_buffer().unwrap().input.text().is_some());
        assert!(!app.active_buffer().unwrap().dirty);
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
