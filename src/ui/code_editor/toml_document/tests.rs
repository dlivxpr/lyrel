//! #293 的生产工作台接缝：TOML 结构、局部编辑、源码与保存。
//!
//! 这些测试只走工作台入口：真实文件打开、真实指针／键盘、源码输入、
//! Undo/Redo、Save、关闭与重开，以及映射 Host 与同名本机诱饵。每个输入
//! 都在自己的临时目录生成，不依赖协调目录的临时夹具；断言的是可见行、
//! 源码范围、文档 revision、history 与落盘字节，不是内部缓存形状。
//! 它们不是原生 OS 窗口、系统 IME 或真实 SSH 验收，相关边界在交付记录中
//! 保留。

use super::super::test_host::{FileHost, canonical_tempdir};
use super::super::*;
use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::test::{ElementSnapshot, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext, size};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// BOM + mixed CRLF/LF + comments, dotted and quoted keys, a datetime, an
/// inline table, ordinary tables interleaved with a later child table, and an
/// array of tables interleaved with an unrelated table. Deleting a table or an
/// AoT entry must not consume the unrelated sections in between.
const ORIGINAL: &[u8] = b"\xef\xbb\xbf# root comment\r\ntitle = \"\xe4\xb8\xad\xe6\x96\x87\" # trailing\n\"dotted.key\" = 1979-05-27T07:32:00Z\r\ndotted.value = 0x10\ninline = { x = 1, \"y.z\" = 2, when = 1979-05-27 }\n\n[a]\r\nvalue = +1_024 # keep\n# comment in a\n[a.child]\r\ndate = 1979-05-27\n[b]\r\nunrelated = true\n# between tables\n[a.late]\r\nkeep = \"a\"\n[[jobs]]\r\nname = \"one\"\n[b.child]\r\nkeep = false\n[[jobs]]\r\nname = \"two\"\n";

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
            "TOML document transition did not settle"
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

fn toml_view(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<TomlView> {
    app.read_with(cx, |app, _| {
        app.active_editor_input()
            .expect("a document in front")
            .toml()
            .expect("a TOML document")
            .clone()
    })
}

fn source_document(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<Document> {
    toml_view(app, cx).read_with(cx, |view, cx| view.document().read(cx).document().clone())
}

fn source_text(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> String {
    source_document(app, cx).read_with(cx, |document, _| document.source().to_owned())
}

fn is_ready(view: &Entity<TomlView>, cx: &VisualTestContext) -> bool {
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
                .is_some_and(|file| file.path == path && file.input.toml().is_some())
        })
    });
    active_id(app, cx)
}

fn open_ready(
    app: &Entity<LyrelApp>,
    cx: &mut VisualTestContext,
    host: SharedHost,
    path: &Path,
) -> (BufferId, Entity<TomlView>) {
    let id = open(app, cx, host, path);
    let view = toml_view(app, cx);
    until(cx, |cx| is_ready(&view, cx));
    (id, view)
}

fn scoped(view: &Entity<TomlView>) -> gpui_kit::ElementId {
    ("toml-document", view.entity_id()).into()
}

fn row(view: &Entity<TomlView>, cx: &mut VisualTestContext, label: &str) -> ElementSnapshot {
    let scope = scoped(view);
    cx.update(|window, _| window.within(scope).find_by_label(label))
}

fn maybe_row(
    view: &Entity<TomlView>,
    cx: &mut VisualTestContext,
    label: &str,
) -> Option<ElementSnapshot> {
    let scope = scoped(view);
    cx.update(|window, _| window.within(scope).try_find_by_label(label))
}

fn row_value(view: &Entity<TomlView>, cx: &mut VisualTestContext, label: &str) -> Option<String> {
    maybe_row(view, cx, label).and_then(|row| row.value().map(str::to_owned))
}

fn click_row(view: &Entity<TomlView>, cx: &mut VisualTestContext, label: &str) {
    let snapshot = row(view, cx, label);
    let id = snapshot.path().last().cloned().expect("a row element id");
    cx.update(|window, cx| window.click(id, cx));
    settle(cx);
}

fn double_click_row(view: &Entity<TomlView>, cx: &mut VisualTestContext, label: &str) {
    let snapshot = row(view, cx, label);
    let id = snapshot.path().last().cloned().expect("a row element id");
    cx.update(|window, cx| window.double_click(id, cx));
    settle(cx);
}

fn ensure_expanded(view: &Entity<TomlView>, cx: &mut VisualTestContext, label: &str) {
    if let Some(row) = maybe_row(view, cx, label)
        && row.expanded() == Some(false)
    {
        click_row(view, cx, label);
    }
}

fn selected_slice(
    app: &Entity<LyrelApp>,
    view: &Entity<TomlView>,
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

fn toggle_mode(view: &Entity<TomlView>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click(("toml-mode-toggle", view.entity_id()), cx));
    settle(cx);
}

fn type_source(
    app: &Entity<LyrelApp>,
    cx: &mut VisualTestContext,
    range: std::ops::Range<usize>,
    text: &str,
) {
    let view = toml_view(app, cx);
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
    replace_bytes_once(bytes, from, to)
}

fn replace_bytes_once(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let at = bytes
        .windows(from.len())
        .position(|window| window == from)
        .expect("fixture bytes");
    let mut replaced = bytes[..at].to_vec();
    replaced.extend_from_slice(to);
    replaced.extend_from_slice(&bytes[at + from.len()..]);
    replaced
}

fn replace_bytes_all(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    while let Some(at) = bytes[cursor..]
        .windows(from.len())
        .position(|window| window == from)
    {
        let at = cursor + at;
        output.extend_from_slice(&bytes[cursor..at]);
        output.extend_from_slice(to);
        cursor = at + from.len();
    }
    output.extend_from_slice(&bytes[cursor..]);
    output
}

fn remove_marker(bytes: &[u8], marker: &[u8]) -> Vec<u8> {
    replace_bytes(bytes, marker, b"")
}

fn insert_before(bytes: &[u8], marker: &[u8], inserted: &[u8]) -> Vec<u8> {
    replace_bytes(bytes, marker, &[inserted, marker].concat())
}

/// The source editor holds LF text without the BOM; saved bytes keep the
/// original representation. Compare structure-driven edits against this
/// normalized view while the save assertions use the raw expected bytes.
fn source_bytes(bytes: &[u8]) -> Vec<u8> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
            output.push(b'\n');
            index += 2;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    output
}

fn click_button(view: &Entity<TomlView>, cx: &mut VisualTestContext, id: &'static str) {
    cx.update(|window, cx| window.click((id, view.entity_id()), cx));
    settle(cx);
}

fn panel_open(view: &Entity<TomlView>, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, _| {
        window
            .try_find(("structure-edit", view.entity_id()))
            .is_some_and(|panel| panel.visible())
    })
}

fn panel_error(view: &Entity<TomlView>, cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(("structure-edit-error", view.entity_id()))
            .and_then(|error| error.value().map(str::to_owned))
    })
}

fn replace_panel_text(
    view: &Entity<TomlView>,
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

fn apply_panel(view: &Entity<TomlView>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click(("structure-edit-apply", view.entity_id()), cx));
    settle(cx);
}

fn cancel_panel(view: &Entity<TomlView>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click(("structure-edit-cancel", view.entity_id()), cx));
    settle(cx);
}

fn assert_original(remote_file: &Path, local_file: &Path) {
    assert_eq!(std::fs::read(remote_file).unwrap(), ORIGINAL);
    assert_eq!(
        std::fs::read(local_file).unwrap(),
        b"LOCAL DECOY MUST STAY UNCHANGED"
    );
}

#[gpui_kit::test]
fn mapped_host_structure_occurrences_source_edit_and_exact_save(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let remote = canonical_tempdir();
    let local = canonical_tempdir();
    // The mapped Host's file and a same-named local decoy: only the owning
    // Host's bytes may be read or written.
    let remote_file = fixture(remote.path(), "config.toml", ORIGINAL);
    let local_file = fixture(
        local.path(),
        "config.toml",
        b"LOCAL DECOY MUST STAY UNCHANGED",
    );
    let host = FileHost::mapped(HostId(293), remote.path());
    let path = PathBuf::from("/config.toml");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1800.), px(1200.)));

    let (id, view) = open_ready(&app, &mut cx, host.clone(), &path);
    app.read_with(&cx, |app, cx| {
        let file = app.editor.buffers.get(id).expect("installed document");
        assert!(!file.dirty, "an untouched TOML opens clean");
        let format = file.format(cx).expect("source format");
        assert!(format.bom, "the UTF-8 BOM is part of the saved bytes");
        assert_eq!(format.encoding, encoding_rs::UTF_8);
        assert!(
            app.editor.local_file(id).is_none(),
            "native TOML is not a code language-service file"
        );
        assert!(app.toml_document_active());
    });
    assert_eq!(
        view.read_with(&cx, |view, _| view.mode()),
        TomlMode::Structure,
        "TOML opens in the structure view"
    );
    cx.update(|window, _| {
        assert!(
            window
                .find(("toml-mode-toggle", view.entity_id()))
                .visible()
        );
        assert!(window.find("status-toml-mode").visible());
        assert!(window.find("status-toml-count").visible());
        assert!(window.try_find("status-image-readonly").is_none());
    });
    assert_eq!(
        row_value(&view, &mut cx, "$.title").as_deref(),
        Some("\"中文\"")
    );
    assert_eq!(
        row_value(&view, &mut cx, "$[\"dotted.key\"]").as_deref(),
        Some("1979-05-27T07:32:00Z"),
        "datetime keeps its exact source spelling"
    );
    assert_eq!(
        row_value(&view, &mut cx, "$.dotted.value").as_deref(),
        Some("0x10"),
        "integer radix spelling is never decoded"
    );
    assert_eq!(
        row_value(&view, &mut cx, "$.inline[\"y.z\"]").as_deref(),
        Some("2"),
        "quoted inline key keeps its own occurrence"
    );
    assert_eq!(
        row_value(&view, &mut cx, "$.a.value").as_deref(),
        Some("+1_024"),
        "underscored integer spelling is never decoded"
    );
    assert_eq!(
        row_value(&view, &mut cx, "$.a.child.date").as_deref(),
        Some("1979-05-27")
    );
    assert_eq!(
        row_value(&view, &mut cx, "$.jobs").as_deref(),
        Some("2 entries")
    );
    assert_eq!(
        row_value(&view, &mut cx, "$.jobs[1].name").as_deref(),
        Some("\"two\"")
    );

    // Pointer selection of the quoted key locates exactly that occurrence.
    click_row(&view, &mut cx, "$[\"dotted.key\"]");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.selected_path(cx))
            .as_deref(),
        Some("$[\"dotted.key\"]")
    );
    let slice = selected_slice(&app, &view, &cx).expect("selected occurrence");
    assert!(slice.contains("\"dotted.key\" = 1979-05-27T07:32:00Z"));
    assert!(!slice.contains("dotted.value"));

    // Enter reveals the exact statement in the shared source.
    reveal_selected(&app, &mut cx);
    assert_eq!(view.read_with(&cx, |view, _| view.mode()), TomlMode::Source);
    let source_range = view.read_with(&cx, |view, cx| view.source().read(cx).selected_range());
    let text = source_text(&app, &cx);
    assert!(text[source_range].contains("\"dotted.key\""));

    // Back to structure, then real keyboard navigation.
    toggle_mode(&view, &mut cx);
    press(&app, &mut cx, "home");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.selected_path(cx))
            .as_deref(),
        Some("$")
    );
    press(&app, &mut cx, "down");
    assert!(
        view.read_with(&cx, |view, cx| view.selected_path(cx))
            .as_deref()
            .is_some_and(|path| path != "$")
    );

    // Full value is the exact source slice, not a decoded projection.
    double_click_row(&view, &mut cx, "$.a.value");
    let full = view.read_with(&cx, |view, cx| view.full_value_text(cx));
    assert_eq!(full.as_deref(), Some("value = +1_024 # keep"));
    cx.update(|window, _| {
        assert!(
            window
                .find(("toml-full-value-text", view.entity_id()))
                .visible()
        );
        assert_eq!(
            window
                .find(("toml-full-value-text", view.entity_id()))
                .value(),
            Some("value = +1_024 # keep")
        );
    });
    cx.update(|window, cx| window.click(("toml-full-value-copy", view.entity_id()), cx));
    settle(&mut cx);
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some("value = +1_024 # keep".to_owned())
    );
    cx.update(|window, cx| window.click(("toml-full-value-close", view.entity_id()), cx));
    settle(&mut cx);
    assert!(
        view.read_with(&cx, |view, _| view.full_value_path())
            .is_none()
    );

    // Untouched save writes the same BOM and every mixed terminator.
    save(&app, &mut cx);
    assert_original(&remote_file, &local_file);

    // A targeted source edit changes only that value; structure follows the
    // new revision and keeps the other spellings and comments intact.
    toggle_mode(&view, &mut cx);
    let text = source_text(&app, &cx);
    let at = text.find("+1_024").expect("fixture value");
    type_source(&app, &mut cx, at..at + "+1_024".len(), "+2_048");
    until(&mut cx, |cx| is_ready(&view, cx));
    toggle_mode(&view, &mut cx);
    assert_eq!(
        row_value(&view, &mut cx, "$.a.value").as_deref(),
        Some("+2_048")
    );
    assert_eq!(
        row_value(&view, &mut cx, "$[\"dotted.key\"]").as_deref(),
        Some("1979-05-27T07:32:00Z")
    );

    // Undo/redo are the shared source history and the structure follows both.
    toggle_mode(&view, &mut cx);
    cx.update(|window, cx| window.press(&secondary("z"), cx));
    until(&mut cx, |cx| is_ready(&view, cx));
    toggle_mode(&view, &mut cx);
    assert_eq!(
        row_value(&view, &mut cx, "$.a.value").as_deref(),
        Some("+1_024")
    );
    toggle_mode(&view, &mut cx);
    cx.update(|window, cx| window.press(&secondary("shift-z"), cx));
    until(&mut cx, |cx| is_ready(&view, cx));
    toggle_mode(&view, &mut cx);
    assert_eq!(
        row_value(&view, &mut cx, "$.a.value").as_deref(),
        Some("+2_048")
    );

    save(&app, &mut cx);
    let edited = replace_bytes(ORIGINAL, b"+1_024", b"+2_048");
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
    assert_eq!(
        row_value(&reopened, &mut cx, "$.a.value").as_deref(),
        Some("+2_048")
    );
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        edited
    );
}

#[gpui_kit::test]
fn table_aot_inline_and_key_commands_preserve_interleaved_bytes(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let path = fixture(dir.path(), "structures.toml", ORIGINAL);
    let host: SharedHost = FileHost::new(HostId(2930));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1800.), px(1200.)));
    let (_, view) = open_ready(&app, &mut cx, host.clone(), &path);
    let mut expected = ORIGINAL.to_vec();

    // Inline table: add a member and remove the first member without
    // serializing the whole table.
    click_row(&view, &mut cx, "$.inline");
    click_button(&view, &mut cx, "toml-add-key");
    assert!(panel_open(&view, &mut cx));
    cx.update(|window, cx| window.click(("structure-edit-key", view.entity_id()), cx));
    cx.update(|window, cx| window.input("z", cx));
    replace_panel_text(&view, &mut cx, "structure-edit-value", "3");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    expected = replace_bytes(
        &expected,
        b"when = 1979-05-27 }",
        b"when = 1979-05-27, z = 3 }",
    );
    assert_eq!(
        source_text(&app, &cx).as_bytes(),
        source_bytes(&expected).as_slice()
    );
    assert_eq!(
        row_value(&view, &mut cx, "$.inline.z").as_deref(),
        Some("3")
    );

    click_row(&view, &mut cx, "$.inline.x");
    click_button(&view, &mut cx, "toml-remove");
    until(&mut cx, |cx| is_ready(&view, cx));
    expected = remove_marker(&expected, b"x = 1, ");
    assert_eq!(
        source_text(&app, &cx).as_bytes(),
        source_bytes(&expected).as_slice()
    );
    assert!(row_value(&view, &mut cx, "$.inline.x").is_none());
    assert_eq!(
        row_value(&view, &mut cx, "$.inline[\"y.z\"]").as_deref(),
        Some("2")
    );

    // Add a key to [a]: it must land inside a's own segment, before its child
    // header, not after the interleaved unrelated tables.
    click_row(&view, &mut cx, "$.a");
    click_button(&view, &mut cx, "toml-add-key");
    cx.update(|window, cx| window.click(("structure-edit-key", view.entity_id()), cx));
    cx.update(|window, cx| window.input("new_key", cx));
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"新值\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    expected = insert_before(
        &expected,
        b"# comment in a\n",
        b"new_key = \"\xe6\x96\xb0\xe5\x80\xbc\"\n",
    );
    assert_eq!(
        source_text(&app, &cx).as_bytes(),
        source_bytes(&expected).as_slice()
    );
    assert_eq!(
        row_value(&view, &mut cx, "$.a.new_key").as_deref(),
        Some("\"新值\"")
    );

    // Add an ordinary child table under a: insertion goes after a's whole
    // subtree (including the later [a.late]), before the next unrelated
    // section, and does not move or delete [b].
    click_row(&view, &mut cx, "$.a");
    click_button(&view, &mut cx, "toml-add-table");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "new_table");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    expected = insert_before(&expected, b"[[jobs]]\r\nname = \"one\"", b"[a.new_table]\n");
    assert_eq!(
        source_text(&app, &cx).as_bytes(),
        source_bytes(&expected).as_slice()
    );
    assert!(maybe_row(&view, &mut cx, "$.a.new_table").is_some());
    assert_eq!(
        row_value(&view, &mut cx, "$.b.unrelated").as_deref(),
        Some("true")
    );

    // Add an entry to the existing array of tables. Appending after the last
    // entry must not disturb the interleaved [b.child].
    click_row(&view, &mut cx, "$.jobs");
    click_button(&view, &mut cx, "toml-add-entry");
    until(&mut cx, |cx| is_ready(&view, cx));
    expected.extend_from_slice(b"[[jobs]]\n");
    assert_eq!(
        source_text(&app, &cx).as_bytes(),
        source_bytes(&expected).as_slice()
    );
    assert_eq!(
        row_value(&view, &mut cx, "$.jobs").as_deref(),
        Some("3 entries")
    );

    // Rename the whole [a] subtree: [a.child] and [a.late] must follow, while
    // [b], [b.child] and both jobs entries keep their exact bytes.
    click_row(&view, &mut cx, "$.a");
    click_button(&view, &mut cx, "toml-rename");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "renamed");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    expected = replace_bytes(&expected, b"[a]\r\n", b"[renamed]\r\n");
    expected = replace_bytes(&expected, b"[a.child]\r\n", b"[renamed.child]\r\n");
    expected = replace_bytes(&expected, b"[a.late]\r\n", b"[renamed.late]\r\n");
    expected = replace_bytes(&expected, b"[a.new_table]\n", b"[renamed.new_table]\n");
    assert_eq!(
        source_text(&app, &cx).as_bytes(),
        source_bytes(&expected).as_slice()
    );
    ensure_expanded(&view, &mut cx, "$.renamed");
    assert!(maybe_row(&view, &mut cx, "$.renamed.child").is_some());
    assert_eq!(
        row_value(&view, &mut cx, "$.b.child.keep").as_deref(),
        Some("false")
    );

    // Rename the whole array of tables: every entry header follows while the
    // unrelated table physically between them is untouched.
    click_row(&view, &mut cx, "$.jobs");
    click_button(&view, &mut cx, "toml-rename");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "tasks");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    expected = replace_bytes_all(&expected, b"[[jobs]]", b"[[tasks]]");
    assert_eq!(
        source_text(&app, &cx).as_bytes(),
        source_bytes(&expected).as_slice()
    );
    assert_eq!(
        row_value(&view, &mut cx, "$.tasks").as_deref(),
        Some("3 entries")
    );
    ensure_expanded(&view, &mut cx, "$.tasks");

    // Delete the renamed ordinary table. Its own header, direct key/value,
    // child and late header/value lines go; unrelated comments, [b],
    // [b.child] and the tasks entries stay byte-identical.
    click_row(&view, &mut cx, "$.renamed");
    click_button(&view, &mut cx, "toml-remove");
    until(&mut cx, |cx| is_ready(&view, cx));
    for marker in [
        b"[renamed]\r\n".as_slice(),
        b"value = +1_024 # keep\n".as_slice(),
        b"new_key = \"\xe6\x96\xb0\xe5\x80\xbc\"\n".as_slice(),
        b"[renamed.child]\r\n".as_slice(),
        b"date = 1979-05-27\n".as_slice(),
        b"[renamed.late]\r\n".as_slice(),
        b"keep = \"a\"\n".as_slice(),
        b"[renamed.new_table]\n".as_slice(),
    ] {
        expected = remove_marker(&expected, marker);
    }
    // The extra empty entry's header is the last task header; the removals
    // above must leave both original entries plus the interleaved b.child.
    assert_eq!(
        source_text(&app, &cx).as_bytes(),
        source_bytes(&expected).as_slice()
    );
    assert!(maybe_row(&view, &mut cx, "$.renamed").is_none());
    assert!(maybe_row(&view, &mut cx, "$.b.unrelated").is_some());
    assert!(maybe_row(&view, &mut cx, "$.b.child.keep").is_some());
    assert_eq!(
        row_value(&view, &mut cx, "$.tasks").as_deref(),
        Some("3 entries")
    );

    // Delete the middle task entry: only that entry's header and direct value
    // disappear, and the unrelated [b.child] physically between entries stays.
    click_row(&view, &mut cx, "$.tasks[1]");
    click_button(&view, &mut cx, "toml-remove");
    until(&mut cx, |cx| is_ready(&view, cx));
    expected = remove_marker(&expected, b"[[tasks]]\r\nname = \"two\"\n");
    assert_eq!(
        source_text(&app, &cx).as_bytes(),
        source_bytes(&expected).as_slice()
    );
    assert!(maybe_row(&view, &mut cx, "$.b.child.keep").is_some());

    // Delete the whole remaining array of tables; [b] and [b.child] survive.
    click_row(&view, &mut cx, "$.tasks");
    click_button(&view, &mut cx, "toml-remove");
    until(&mut cx, |cx| is_ready(&view, cx));
    expected = remove_marker(&expected, b"[[tasks]]\r\nname = \"one\"\n");
    expected = remove_marker(&expected, b"[[tasks]]\n");
    assert_eq!(
        source_text(&app, &cx).as_bytes(),
        source_bytes(&expected).as_slice()
    );
    assert!(maybe_row(&view, &mut cx, "$.b.unrelated").is_some());
    assert!(maybe_row(&view, &mut cx, "$.b.child.keep").is_some());

    save(&app, &mut cx);
    assert_eq!(std::fs::read(&path).unwrap(), expected);
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        expected
    );

    // Independent reopen through the owning Host observes the same bytes.
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.is_empty())
    });
    let (_, reopened) = open_ready(&app, &mut cx, host, &path);
    assert!(maybe_row(&reopened, &mut cx, "$.b.child.keep").is_some());
    assert!(maybe_row(&reopened, &mut cx, "$.tasks").is_none());
}

#[gpui_kit::test]
fn invalid_budget_and_late_results_preserve_source_and_never_publish_stale_structure(
    tcx: &mut TestAppContext,
) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let host: SharedHost = FileHost::new(HostId(2931));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));

    // Invalid TOML: the structure reports the error, source stays whole and
    // editable, and a repair installs the tree again.
    let broken_bytes = b"\xef\xbb\xbf# broken\r\na = \n[b]\r\n";
    let broken = fixture(dir.path(), "broken.toml", broken_bytes);
    let _ = open(&app, &mut cx, host.clone(), &broken);
    let broken_view = toml_view(&app, &cx);
    until(&mut cx, |cx| {
        broken_view.read_with(cx, |view, cx| view.fatal_error(cx).is_some())
    });
    cx.update(|window, _| {
        assert!(
            window
                .find(("toml-structure-error-body", broken_view.entity_id()))
                .visible()
        );
        assert!(
            window
                .find(("toml-structure-error", broken_view.entity_id()))
                .visible()
        );
    });
    app.read_with(&cx, |app, _| {
        assert!(app.active_buffer().unwrap().input.text().is_some());
        assert!(!app.active_buffer().unwrap().dirty);
    });
    toggle_mode(&broken_view, &mut cx);
    let text = source_text(&app, &cx);
    let at = text.find("a = \n").expect("broken value");
    type_source(&app, &mut cx, at..at + "a = ".len(), "a = 1");
    until(&mut cx, |cx| is_ready(&broken_view, cx));
    toggle_mode(&broken_view, &mut cx);
    assert_eq!(
        row_value(&broken_view, &mut cx, "$.a").as_deref(),
        Some("1")
    );
    save(&app, &mut cx);
    assert_eq!(
        std::fs::read(&broken).unwrap(),
        replace_bytes(broken_bytes, b"a = \n", b"a = 1\n")
    );

    // Over the structure input budget: explicit failure, source still opens,
    // edits and saves byte-for-byte.
    let big_bytes = format!(
        "title = \"{}\"\n",
        "x".repeat(projection::STRUCTURE_INPUT_BYTES + 1024)
    )
    .into_bytes();
    let big = fixture(dir.path(), "big.toml", &big_bytes);
    let _ = open(&app, &mut cx, host.clone(), &big);
    let big_view = toml_view(&app, &cx);
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

    // A paused parse must never publish a stale revision. The edits collapse
    // to the latest revision, and only that projection is installed.
    let late_bytes = b"value = \"first\"\n";
    let late = fixture(dir.path(), "late.toml", late_bytes);
    let (release, entered) = test_support::pause_next_parse();
    let _ = open(&app, &mut cx, host, &late);
    let late_view = toml_view(&app, &cx);
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
        row_value(&late_view, &mut cx, "$.value").as_deref(),
        Some("\"latest\"")
    );
}

#[gpui_kit::test]
fn split_presentations_share_document_and_reject_stale_tree_edits(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let original = fixture(dir.path(), "split.toml", ORIGINAL);
    let host: SharedHost = FileHost::new(HostId(2932));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1800.), px(1200.)));
    open_ready(&app, &mut cx, host, &original);

    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx), "the TOML document splits");
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
        let file = app.active_buffer().expect("TOML buffer");
        (
            file.presentations.toml_existing(tab, right).expect("front"),
            file.presentations.toml_existing(tab, left).expect("other"),
        )
    });
    assert_eq!(
        other.read_with(&cx, |view, _| view.mode()),
        TomlMode::Structure
    );
    assert_eq!(
        front.read_with(&cx, |view, _| view.mode()),
        TomlMode::Source,
        "the split starts with the other side of the source"
    );
    toggle_mode(&front, &mut cx);

    // Folds and selection are presentation-local.
    assert_eq!(row(&other, &mut cx, "$.inline").expanded(), Some(true));
    click_row(&other, &mut cx, "$.inline");
    assert_eq!(row(&other, &mut cx, "$.inline").expanded(), Some(false));
    assert_eq!(
        row(&front, &mut cx, "$.inline").expanded(),
        Some(true),
        "the other presentation keeps its own fold"
    );
    click_row(&other, &mut cx, "$.title");
    click_row(&front, &mut cx, "$.a.value");
    assert_ne!(
        other.read_with(&cx, |view, cx| view.selected_path(cx)),
        front.read_with(&cx, |view, cx| view.selected_path(cx))
    );

    // The full-value panel belongs to one presentation.
    double_click_row(&other, &mut cx, "$.a.value");
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
    let at = text.find("+1_024").expect("fixture value");
    type_source(&app, &mut cx, at..at + "+1_024".len(), "+9_999");
    until(&mut cx, |cx| is_ready(&front, cx) && is_ready(&other, cx));
    toggle_mode(&front, &mut cx);
    assert_eq!(
        row_value(&other, &mut cx, "$.a.value").as_deref(),
        Some("+9_999")
    );
    assert_eq!(
        row_value(&front, &mut cx, "$.a.value").as_deref(),
        Some("+9_999")
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
        row_value(&front, &mut cx, "$.a.value").as_deref(),
        Some("+1_024")
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

    // Open a tree command in the structure presentation, then change the
    // source from the other presentation. The pending command must not write.
    toggle_mode(&other, &mut cx);
    assert_eq!(
        other.read_with(&cx, |view, _| view.mode()),
        TomlMode::Structure
    );
    toggle_mode(&front, &mut cx);
    assert_eq!(
        front.read_with(&cx, |view, _| view.mode()),
        TomlMode::Source
    );
    click_row(&other, &mut cx, "$.a.value");
    click_button(&other, &mut cx, "toml-edit-value");
    assert!(panel_open(&other, &mut cx));
    app.update_in(&mut cx, |app, window, cx| {
        front.update(cx, |view, cx| {
            let focus = view.source().read(cx).focus_handle(cx);
            focus.focus(window, cx);
        });
        app.editor_split_follow_focus(window, cx);
    });
    let text = source_text(&app, &cx);
    let at = text.find("+1_024").expect("fixture value");
    type_source(&app, &mut cx, at..at + "+1_024".len(), "+7_777");
    until(&mut cx, |cx| is_ready(&front, cx) && is_ready(&other, cx));
    assert_eq!(
        panel_error(&other, &mut cx).as_deref(),
        Some(
            "The document changed, so this edit was not written. Close this panel and select the node again."
        )
    );
    replace_panel_text(&other, &mut cx, "structure-edit-value", "+8_888");
    apply_panel(&other, &mut cx);
    let text = source_text(&app, &cx);
    assert!(text.contains("+7_777"));
    assert!(!text.contains("+8_888"));
    assert!(
        panel_open(&other, &mut cx),
        "a stale command stays open for a decision"
    );
    cancel_panel(&other, &mut cx);
    assert!(!panel_open(&other, &mut cx));

    // A fresh tree command lands in the one shared history and undo from the
    // source presentation reverses it.
    click_row(&other, &mut cx, "$.a.value");
    click_button(&other, &mut cx, "toml-edit-value");
    replace_panel_text(&other, &mut cx, "structure-edit-value", "+3_333");
    apply_panel(&other, &mut cx);
    until(&mut cx, |cx| is_ready(&other, cx));
    assert!(source_text(&app, &cx).contains("+3_333"));
    app.update_in(&mut cx, |app, window, cx| {
        front.update(cx, |view, cx| {
            let focus = view.source().read(cx).focus_handle(cx);
            focus.focus(window, cx);
        });
        app.editor_split_follow_focus(window, cx);
    });
    cx.update(|window, cx| window.press(&secondary("z"), cx));
    until(&mut cx, |cx| is_ready(&front, cx) && is_ready(&other, cx));
    assert!(source_text(&app, &cx).contains("+7_777"));
    assert!(!source_text(&app, &cx).contains("+3_333"));
    // Closing is about the presentation contract, not an unsaved prompt; the
    // cancelled-close path is covered in the persistence test below.
    save(&app, &mut cx);

    // Closing one presentation is the existing split contract: the other
    // presentation still owns the buffer, and only the last close retires the
    // document.
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    settle(&mut cx);
    assert!(
        app.read_with(&cx, |app, _| {
            let group = app.tab_code().map(|code| code.group);
            app.active_buffer().is_some_and(|file| {
                group.is_some_and(|group| file.presentations.toml_existing(tab, group).is_some())
            })
        }),
        "the other presentation still owns the document"
    );
    assert!(
        app.read_with(&cx, |app, _| !app.editor.buffers.is_empty()),
        "one presentation close does not retire the shared document"
    );
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.is_empty())
    });
}

#[gpui_kit::test]
fn tree_edits_survive_save_as_failed_writes_conflicts_and_cancelled_close(
    tcx: &mut TestAppContext,
) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let original = fixture(dir.path(), "persist.toml", ORIGINAL);
    let target = dir.path().join("persisted.toml");
    let host = FileHost::new(HostId::LOCAL);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1800.), px(1200.)));
    let (_, view) = open_ready(&app, &mut cx, host.clone(), &original);
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.watching())
    });

    // A tree edit, then Save As to a new owning-Host target.
    click_row(&view, &mut cx, "$.title");
    click_button(&view, &mut cx, "toml-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"changed\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    cx.dispatch_action(crate::core::actions::EditorSaveAs);
    settle(&mut cx);
    cx.simulate_new_path_selection(|_| Some(target.clone()));
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.path == target && !file.dirty && !file.saving)
        })
    });
    let saved = source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap());
    assert_eq!(std::fs::read(&target).unwrap(), saved);
    assert_eq!(std::fs::read(&original).unwrap(), ORIGINAL);

    // A save that is already in flight must not clear newer dirty work when it
    // completes. Pause the write, edit the tree again, then let the old save
    // finish: the target keeps the old bytes and the document stays dirty.
    let release = host.pause_next_write();
    cx.dispatch_action(crate::core::actions::EditorSave);
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer().is_some_and(|file| file.saving)
        })
    });
    click_row(&view, &mut cx, "$.title");
    click_button(&view, &mut cx, "toml-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"newer\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    release.send(()).unwrap();
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.dirty && !file.saving)
        })
    });
    assert_eq!(std::fs::read(&target).unwrap(), saved);
    assert!(source_text(&app, &cx).contains("\"newer\""));
    save(&app, &mut cx);
    let saved = replace_bytes(&saved, b"\"changed\"", b"\"newer\"");
    assert_eq!(std::fs::read(&target).unwrap(), saved);

    // A failed owning-Host write leaves the edit dirty and the target bytes
    // unchanged; an ordinary retry then lands the same bytes.
    click_row(&view, &mut cx, "$.title");
    click_button(&view, &mut cx, "toml-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"retried\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    host.fail_next_write();
    cx.dispatch_action(crate::core::actions::EditorSave);
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.dirty && !file.saving)
        })
    });
    assert_eq!(std::fs::read(&target).unwrap(), saved);
    save(&app, &mut cx);
    let retried = replace_bytes(&saved, b"\"newer\"", b"\"retried\"");
    assert_eq!(std::fs::read(&target).unwrap(), retried);

    // A dirty external write is a conflict, not a silent replacement. Keep
    // mine and save the selected tree edit to the owning Host.
    click_row(&view, &mut cx, "$.title");
    click_button(&view, &mut cx, "toml-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"kept\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    std::fs::write(&target, b"title = \"external\"\n").unwrap();
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer()
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
    click_row(&view, &mut cx, "$.title");
    click_button(&view, &mut cx, "toml-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"still-open\"");
    cx.update(|window, cx| window.press("enter", cx));
    settle(&mut cx);
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
        app.read_with(&cx, |app, _| {
            app.active_buffer().is_some_and(|file| file.dirty)
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
        app.read_with(&cx, |app, _| app.active_buffer().is_some()),
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
        app.read_with(cx, |app, _| app.editor.buffers.is_empty())
    });
    let (_, reopened) = open_ready(&app, &mut cx, host, &target);
    assert_eq!(
        row_value(&reopened, &mut cx, "$.title").as_deref(),
        Some("\"still-open\"")
    );
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        still_open
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
    let script = b"# remote comment\n\"quoted.key\" = 1979-05-27T07:32:00Z\n[a]\r\nvalue = +1_024\n[[jobs]]\nname = \"one\"\n[b]\nunrelated = true\n[[jobs]]\nname = \"two\"\n";
    let _ = fixture.install_script(&host, "editing.toml", script, Vec::new());
    let path = fixture.remote_path("editing.toml");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1800.), px(1200.)));
    let (_, view) = open_ready(&app, &mut cx, host, &path);

    click_row(&view, &mut cx, "$.a.value");
    click_button(&view, &mut cx, "toml-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "+2_048");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));

    click_row(&view, &mut cx, "$.jobs");
    click_button(&view, &mut cx, "toml-rename");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "tasks");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    save(&app, &mut cx);

    let expected = replace_bytes_all(script, b"[[jobs]]", b"[[tasks]]");
    let expected = replace_bytes(&expected, b"+1_024", b"+2_048");
    // A second production control client proves the owning Host's bytes.
    let verifier = fixture.connect_control_as(
        "fixture:native-ssh-293-verify",
        "native-ssh-toml-293",
        "isolated-fixture",
    );
    let bytes = verifier
        .read_file(&path, expected.len() as u64 + 1)
        .expect("independent remote read");
    eprintln!(
        "toml293 real native SSH saved {} bytes at {}",
        bytes.len(),
        path.display()
    );
    assert_eq!(bytes, expected);
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        expected
    );
}
