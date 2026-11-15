//! #292 的生产工作台接缝：YAML 结构、源码与保存。
//!
//! 这些测试只走工作台入口：真实文件打开、真实指针／键盘、源码输入、
//! Undo/Redo、Save、关闭与重开，以及映射 Host 与同名本机诱饵。每个输入
//! 都在自己的临时目录生成，不依赖协调目录的临时夹具；断言的是可见行、
//! 源码 occurrence、文档 revision、history 与落盘字节，不是内部缓存形状。
//! 它们不是原生 OS 窗口或真实 SSH 验收，相关边界在交付记录中保留。

use super::super::test_host::{FileHost, canonical_tempdir};
use super::super::*;
use super::projection::{STRUCTURE_INPUT_BYTES, STRUCTURE_NODES};
use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::test::{ElementSnapshot, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext, size};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// BOM + mixed CRLF/LF + comments, anchors/aliases, a custom tag, a block
/// scalar, a complex key and two documents. Untouched save has representation
/// to lose if any whole-document serialization leaks into the path.
const ORIGINAL_TEXT: &str = "\u{feff}%YAML 1.2\r\n---\r\n# 保留注释\r\nname: '旧值' # keep\r\ndefaults: &defaults\r\n  retries: 3\r\n  message: \"中文\"\r\ncopy: *defaults\r\ntagged: !vendor/kind \"原样\"\r\nblock: |-\r\n  第一行\r\n  第二行\r\n? [left, right]\r\n: complex-key\r\n...\r\n---\nname: \"第二文档\"\nitems:\n  - one\n  - two\nflow: {a: 1, b: [x, y]}\n";

fn original() -> &'static [u8] {
    ORIGINAL_TEXT.as_bytes()
}

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
            "YAML document transition did not settle"
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

fn yaml_view(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<YamlView> {
    app.read_with(cx, |app, _| {
        app.active_editor_input()
            .expect("a document in front")
            .yaml()
            .expect("a YAML document")
            .clone()
    })
}

fn source_document(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<Document> {
    yaml_view(app, cx).read_with(cx, |view, cx| view.document().read(cx).document().clone())
}

fn source_text(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> String {
    source_document(app, cx).read_with(cx, |document, _| document.source().to_owned())
}

fn is_ready(view: &Entity<YamlView>, cx: &VisualTestContext) -> bool {
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
                .is_some_and(|file| file.path == path && file.input.yaml().is_some())
        })
    });
    active_id(app, cx)
}

fn open_ready(
    app: &Entity<LyrelApp>,
    cx: &mut VisualTestContext,
    host: SharedHost,
    path: &Path,
) -> (BufferId, Entity<YamlView>) {
    let id = open(app, cx, host, path);
    let view = yaml_view(app, cx);
    until(cx, |cx| is_ready(&view, cx));
    (id, view)
}

fn scoped(view: &Entity<YamlView>) -> gpui_kit::ElementId {
    ("yaml-document", view.entity_id()).into()
}

fn row(view: &Entity<YamlView>, cx: &mut VisualTestContext, label: &str) -> ElementSnapshot {
    let scope = scoped(view);
    cx.update(|window, _| window.within(scope).find_by_label(label))
}

fn maybe_row(
    view: &Entity<YamlView>,
    cx: &mut VisualTestContext,
    label: &str,
) -> Option<ElementSnapshot> {
    let scope = scoped(view);
    cx.update(|window, _| window.within(scope).try_find_by_label(label))
}

fn row_value(view: &Entity<YamlView>, cx: &mut VisualTestContext, label: &str) -> Option<String> {
    maybe_row(view, cx, label).and_then(|row| row.value().map(str::to_owned))
}

fn row_description(
    view: &Entity<YamlView>,
    cx: &mut VisualTestContext,
    label: &str,
) -> Option<String> {
    maybe_row(view, cx, label).and_then(|row| row.description().map(str::to_owned))
}

fn click_row(view: &Entity<YamlView>, cx: &mut VisualTestContext, label: &str) {
    let snapshot = row(view, cx, label);
    let id = snapshot.path().last().cloned().expect("a row element id");
    cx.update(|window, cx| window.click(id, cx));
    settle(cx);
}

fn selected_slice(
    app: &Entity<LyrelApp>,
    view: &Entity<YamlView>,
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

fn toggle_mode(view: &Entity<YamlView>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click(("yaml-mode-toggle", view.entity_id()), cx));
    settle(cx);
}

fn type_source(
    app: &Entity<LyrelApp>,
    cx: &mut VisualTestContext,
    range: std::ops::Range<usize>,
    text: &str,
) {
    let view = yaml_view(app, cx);
    app.update_in(cx, |app, window, cx| {
        app.focus_editor(window, cx);
        let source = view.read(cx).source().clone();
        source.update(cx, |source, cx| source.select_range(range, cx));
    });
    cx.update(|window, cx| window.input(text, cx));
    settle(cx);
}

fn type_edit_value(view: &Entity<YamlView>, cx: &mut VisualTestContext, text: &str) {
    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.focus_edit_value(window, cx));
        window.press("secondary-a", cx);
        window.input(text, cx);
    });
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

fn click_button(view: &Entity<YamlView>, cx: &mut VisualTestContext, id: &'static str) {
    cx.update(|window, cx| window.click((id, view.entity_id()), cx));
    settle(cx);
}

fn panel_open(view: &Entity<YamlView>, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, _| {
        window
            .try_find(("structure-edit", view.entity_id()))
            .is_some_and(|panel| panel.visible())
    })
}

fn panel_error(view: &Entity<YamlView>, cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(("structure-edit-error", view.entity_id()))
            .and_then(|error| error.value().map(str::to_owned))
    })
}

fn panel_notice(view: &Entity<YamlView>, cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(("structure-edit-notice", view.entity_id()))
            .and_then(|notice| notice.value().map(str::to_owned))
    })
}

fn replace_panel_text(
    view: &Entity<YamlView>,
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

fn apply_panel(view: &Entity<YamlView>, cx: &mut VisualTestContext) {
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

fn cancel_panel(view: &Entity<YamlView>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click(("structure-edit-cancel", view.entity_id()), cx));
    settle(cx);
}

#[gpui_kit::test]
fn mapped_host_multi_document_anchors_tags_and_exact_save(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let remote = canonical_tempdir();
    let local = canonical_tempdir();
    let remote_file = fixture(remote.path(), "config.yaml", original());
    let local_file = fixture(
        local.path(),
        "config.yaml",
        b"LOCAL DECOY MUST STAY UNCHANGED",
    );
    let host = FileHost::mapped(HostId(2921), remote.path());
    let path = PathBuf::from("/config.yaml");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));

    let (id, view) = open_ready(&app, &mut cx, host.clone(), &path);
    app.read_with(&cx, |app, cx| {
        let file = app.editor.buffers.get(id).expect("installed document");
        assert!(!file.dirty, "an untouched YAML opens clean");
        let format = file.format(cx).expect("source format");
        assert!(format.bom, "the UTF-8 BOM is part of the saved bytes");
        assert_eq!(format.encoding, encoding_rs::UTF_8);
        assert!(
            app.editor.local_file(id).is_none(),
            "native YAML is not a code language-service file"
        );
        assert!(app.yaml_document_active());
    });
    assert_eq!(
        view.read_with(&cx, |view, _| view.mode()),
        YamlMode::Structure
    );
    cx.update(|window, _| {
        assert!(
            window
                .find(("yaml-mode-toggle", view.entity_id()))
                .visible()
        );
        assert!(window.find("status-yaml-mode").visible());
        assert!(window.find("status-yaml-count").visible());
        assert!(window.try_find("status-image-readonly").is_none());
    });
    assert_eq!(row(&view, &mut cx, "$").expanded(), Some(true));
    assert_eq!(
        row_value(&view, &mut cx, "$#1[\"name\"]").as_deref(),
        Some("'旧值'"),
        "the original quoted scalar spelling stays visible"
    );
    assert_eq!(
        row_value(&view, &mut cx, "$#1[\"defaults\"][\"retries\"]").as_deref(),
        Some("3")
    );
    assert_eq!(
        row_description(&view, &mut cx, "$#1[\"defaults\"]").as_deref(),
        Some("&defaults"),
        "the anchored mapping is labelled, not expanded"
    );
    assert_eq!(
        row_value(&view, &mut cx, "$#1[\"copy\"]").as_deref(),
        Some("*defaults"),
        "an alias stays a reference"
    );
    assert_eq!(
        row_description(&view, &mut cx, "$#1[\"copy\"]").as_deref(),
        Some("*defaults → &defaults")
    );
    assert!(
        maybe_row(&view, &mut cx, "$#1[\"copy\"][\"retries\"]").is_none(),
        "the alias does not become a second copy of the mapping"
    );
    assert_eq!(
        row_value(&view, &mut cx, "$#1[\"tagged\"]").as_deref(),
        Some("\"原样\"")
    );
    assert!(
        row_description(&view, &mut cx, "$#1[\"tagged\"]")
            .as_deref()
            .is_some_and(|badge| badge.contains("!vendor/kind"))
    );
    assert!(
        row_value(&view, &mut cx, "$#1[\"block\"]")
            .as_deref()
            .is_some_and(|value| value.starts_with("|-") && value.contains("第一行"))
    );
    assert!(maybe_row(&view, &mut cx, "$#1[\"[left, right]\"]").is_some());
    assert_eq!(
        row_value(&view, &mut cx, "$#2[\"name\"]").as_deref(),
        Some("\"第二文档\"")
    );
    assert_eq!(
        row_value(&view, &mut cx, "$#2[\"items\"][0]").as_deref(),
        Some("one")
    );

    click_row(&view, &mut cx, "$#1[\"defaults\"][\"retries\"]");
    let slice = selected_slice(&app, &view, &cx).expect("selected entry");
    assert!(slice.contains("retries: 3"), "selected slice: {slice:?}");
    reveal_selected(&app, &mut cx);
    assert_eq!(view.read_with(&cx, |view, _| view.mode()), YamlMode::Source);
    let source_range = view.read_with(&cx, |view, cx| view.source().read(cx).selected_range());
    let text = source_text(&app, &cx);
    assert!(text[source_range].contains("retries"));

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
        Some("$#1")
    );

    // Untouched save writes the same BOM and every mixed terminator.
    save(&app, &mut cx);
    assert_eq!(std::fs::read(&remote_file).unwrap(), original());

    // A block scalar is edited in the shared panel's real multiline field:
    // the raw header and body are a source fragment, validated as one block
    // scalar, then spliced locally with the same commit/history path.
    click_row(&view, &mut cx, "$#1[\"block\"]");
    click_button(&view, &mut cx, "yaml-edit-value");
    assert!(panel_open(&view, &mut cx));
    assert_eq!(
        view.read_with(&cx, |view, _| view.mode()),
        YamlMode::Structure
    );
    let (_, raw) = view
        .read_with(&cx, |view, cx| view.edit_texts(cx))
        .expect("multiline value");
    assert!(
        raw.starts_with("|-") && raw.contains("第一行"),
        "raw: {raw:?}"
    );
    type_edit_value(&view, &mut cx, "|-\n  改行\n  第二行");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(
        source_text(&app, &cx).contains("|-\n  改行\n  第二行"),
        "block source: {:?}",
        source_text(&app, &cx)
    );

    // A flow collection key is a complete local fragment; the same panel
    // validates it as one mapping key instead of guessing a re-render.
    click_row(&view, &mut cx, "$#1[\"[left, right]\"]");
    click_button(&view, &mut cx, "yaml-rename-key");
    assert!(panel_open(&view, &mut cx));
    assert_eq!(
        view.read_with(&cx, |view, _| view.mode()),
        YamlMode::Structure
    );
    let (_, raw) = view
        .read_with(&cx, |view, cx| view.edit_texts(cx))
        .expect("complex key value");
    assert_eq!(raw, "[left, right]");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "[left, third]");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(source_text(&app, &cx).contains("[left, third]"));

    // Editing a value inside the anchored mapping explains the alias impact,
    // and the alias itself remains a reference.
    click_row(&view, &mut cx, "$#1[\"defaults\"][\"retries\"]");
    click_button(&view, &mut cx, "yaml-edit-value");
    assert!(panel_open(&view, &mut cx));
    let notice = panel_notice(&view, &mut cx).expect("anchor impact notice");
    assert!(notice.contains("&defaults"), "notice: {notice}");
    assert!(notice.contains("1 aliases"), "notice: {notice}");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "5");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(source_text(&app, &cx).contains("retries: 5"));
    assert!(source_text(&app, &cx).contains("copy: *defaults"));
    assert!(
        view.read_with(&cx, |view, _| view.mutation_notice())
            .is_some_and(|notice| notice.contains("Edited &defaults"))
    );
    assert_eq!(
        row_value(&view, &mut cx, "$#1[\"copy\"]").as_deref(),
        Some("*defaults")
    );

    // Editing the tagged scalar keeps the custom tag in front of the value.
    click_row(&view, &mut cx, "$#1[\"tagged\"]");
    click_button(&view, &mut cx, "yaml-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"改后\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(
        source_text(&app, &cx).contains("tagged: !vendor/kind \"改后\""),
        "tag stays with the edited scalar"
    );
    assert_eq!(
        row_value(&view, &mut cx, "$#1[\"tagged\"]").as_deref(),
        Some("\"改后\"")
    );

    save(&app, &mut cx);
    let edited = replace_bytes(
        &replace_bytes(
            &replace_bytes(
                &replace_bytes(original(), b"retries: 3", b"retries: 5"),
                "\"原样\"".as_bytes(),
                "\"改后\"".as_bytes(),
            ),
            "第一行".as_bytes(),
            "改行".as_bytes(),
        ),
        b"[left, right]",
        b"[left, third]",
    );
    assert_eq!(std::fs::read(&remote_file).unwrap(), edited);
    assert_eq!(
        std::fs::read(&local_file).unwrap(),
        b"LOCAL DECOY MUST STAY UNCHANGED",
        "the same path on this machine was never read or written"
    );

    // An independent parser proves the saved alias still resolves to the
    // edited value, not an expanded copy.
    use yaml_edit::{AnchorRegistry, YamlFile};
    let saved = std::fs::read(&remote_file).unwrap();
    let text =
        std::str::from_utf8(saved.strip_prefix(&[0xef, 0xbb, 0xbf]).expect("saved BOM")).unwrap();
    let file = YamlFile::parse(text).tree();
    let document = file.documents().next().expect("first document");
    let registry = AnchorRegistry::from_document(&document);
    let alias = document
        .as_mapping()
        .unwrap()
        .get("copy")
        .unwrap()
        .as_alias()
        .unwrap()
        .clone();
    let resolved = registry.resolve_alias(&alias).unwrap();
    assert_eq!(
        resolved
            .as_mapping()
            .unwrap()
            .get("retries")
            .unwrap()
            .as_scalar()
            .unwrap()
            .as_string(),
        "5"
    );
    let tagged = document
        .as_mapping()
        .unwrap()
        .get("tagged")
        .unwrap()
        .as_tagged()
        .unwrap()
        .clone();
    assert_eq!(tagged.tag().as_deref(), Some("!vendor/kind"));
    assert_eq!(tagged.as_string().as_deref(), Some("改后"));

    // Reopen through the owning Host observes the saved bytes independently.
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.is_empty())
    });
    let (_, reopened) = open_ready(&app, &mut cx, host, &path);
    assert_eq!(
        row_value(&reopened, &mut cx, "$#1[\"defaults\"][\"retries\"]").as_deref(),
        Some("5")
    );
    assert_eq!(
        row_value(&reopened, &mut cx, "$#1[\"copy\"]").as_deref(),
        Some("*defaults")
    );
    assert!(
        row_value(&reopened, &mut cx, "$#1[\"block\"]")
            .as_deref()
            .is_some_and(|value| value.contains("改行") && value.contains("|-"))
    );
    assert!(maybe_row(&reopened, &mut cx, "$#1[\"[left, third]\"]").is_some());
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        edited
    );
}

#[gpui_kit::test]
fn flow_block_key_value_and_sequence_edits_keep_comments_and_structure(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let text = "# head\r\nflow: {a: 1, b: [x, y]}\r\nblock:\r\n  one: 1\r\n  two: 2\r\nitems:\r\n  - alpha\r\n  - beta\r\n# tail\r\n";
    let path = fixture(dir.path(), "shapes.yaml", text.as_bytes());
    let host: SharedHost = FileHost::new(HostId(2922));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (_, view) = open_ready(&app, &mut cx, host, &path);

    // Rename a block key.
    click_row(&view, &mut cx, "$[\"block\"][\"one\"]");
    click_button(&view, &mut cx, "yaml-rename-key");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "first");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(source_text(&app, &cx).contains("  first: 1"));
    assert!(!source_text(&app, &cx).contains("  one: 1"));

    // Insert into a flow mapping and a flow sequence.
    click_row(&view, &mut cx, "$[\"flow\"]");
    click_button(&view, &mut cx, "yaml-add-member");
    replace_panel_text(&view, &mut cx, "structure-edit-key", "c");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "3");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(source_text(&app, &cx).contains("{a: 1, b: [x, y], c: 3}"));

    click_row(&view, &mut cx, "$[\"flow\"][\"b\"]");
    click_button(&view, &mut cx, "yaml-add-member");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "z");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(
        source_text(&app, &cx).contains("b: [x, y, z]"),
        "flow item source: {:?}",
        source_text(&app, &cx)
    );

    // Insert into a block mapping and a block sequence.
    click_row(&view, &mut cx, "$[\"block\"]");
    click_button(&view, &mut cx, "yaml-add-member");
    replace_panel_text(&view, &mut cx, "structure-edit-key", "three");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "3");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(source_text(&app, &cx).contains("  three: 3"));

    click_row(&view, &mut cx, "$[\"items\"]");
    click_button(&view, &mut cx, "yaml-add-member");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "gamma");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(source_text(&app, &cx).contains("  - gamma"));

    // Remove from block and flow without touching comments or editors.
    click_row(&view, &mut cx, "$[\"block\"][\"two\"]");
    click_button(&view, &mut cx, "yaml-remove-member");
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(!source_text(&app, &cx).contains("two: 2"));

    click_row(&view, &mut cx, "$[\"flow\"][\"b\"][1]");
    click_button(&view, &mut cx, "yaml-remove-member");
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(source_text(&app, &cx).contains("b: [x, z]"));

    assert!(source_text(&app, &cx).contains("# head"));
    assert!(source_text(&app, &cx).contains("# tail"));

    // Undo and redo from the tree itself use the same shared history. The tree
    // handler calls the source presentation's `undo_document`; it does not
    // switch to Source or create a second history.
    press(&app, &mut cx, &secondary("z"));
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(source_text(&app, &cx).contains("b: [x, y, z]"));
    press(&app, &mut cx, &secondary("shift-z"));
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(source_text(&app, &cx).contains("b: [x, z]"));

    save(&app, &mut cx);
    let expected = "# head\r\nflow: {a: 1, b: [x, z], c: 3}\r\nblock:\r\n  first: 1\r\n  three: 3\r\nitems:\r\n  - alpha\r\n  - beta\r\n  - gamma\r\n# tail\r\n";
    assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        expected.as_bytes()
    );
}

#[gpui_kit::test]
fn removing_the_last_block_member_keeps_an_empty_flow_collection(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let text = "one:\r\n  only: 1\r\ntwo:\r\n  - only\r\n";
    let path = fixture(dir.path(), "last.yaml", text.as_bytes());
    let host: SharedHost = FileHost::new(HostId(2925));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (_, view) = open_ready(&app, &mut cx, host, &path);

    click_row(&view, &mut cx, "$[\"one\"][\"only\"]");
    click_button(&view, &mut cx, "yaml-remove-member");
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(
        source_text(&app, &cx).contains("one:\n  {}\n"),
        "empty mapping: {:?}",
        source_text(&app, &cx)
    );

    click_row(&view, &mut cx, "$[\"two\"][0]");
    click_button(&view, &mut cx, "yaml-remove-member");
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(
        source_text(&app, &cx).contains("two:\n  []\n"),
        "empty sequence: {:?}",
        source_text(&app, &cx)
    );

    save(&app, &mut cx);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"one:\r\n  {}\r\ntwo:\r\n  []\r\n"
    );
}

#[gpui_kit::test]
fn utf16le_bom_and_line_endings_survive_a_tree_edit(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let text = "name: \"中文\"\r\nlist:\r\n  - one\r\n";
    let mut bytes = vec![0xff, 0xfe];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let path = fixture(dir.path(), "utf16.yaml", &bytes);
    let host: SharedHost = FileHost::new(HostId(2926));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (id, view) = open_ready(&app, &mut cx, host.clone(), &path);
    app.read_with(&cx, |app, cx| {
        let format = app
            .editor
            .buffers
            .get(id)
            .expect("document")
            .format(cx)
            .expect("format");
        assert_eq!(format.encoding, encoding_rs::UTF_16LE);
        assert!(format.bom);
    });
    assert_eq!(
        row_value(&view, &mut cx, "$[\"name\"]").as_deref(),
        Some("\"中文\"")
    );
    assert_eq!(
        row_value(&view, &mut cx, "$[\"list\"][0]").as_deref(),
        Some("one")
    );

    click_row(&view, &mut cx, "$[\"name\"]");
    click_button(&view, &mut cx, "yaml-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"改后\"");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    save(&app, &mut cx);

    let saved = std::fs::read(&path).unwrap();
    assert_eq!(&saved[..2], &[0xff, 0xfe]);
    let (decoded, _, _) = encoding_rs::UTF_16LE.decode(&saved[2..]);
    assert!(decoded.contains("name: \"改后\"\r\n"));
    assert!(decoded.contains("list:\r\n  - one\r\n"));
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        saved
    );

    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.is_empty())
    });
    let (_, reopened) = open_ready(&app, &mut cx, host, &path);
    assert_eq!(
        row_value(&reopened, &mut cx, "$[\"name\"]").as_deref(),
        Some("\"改后\"")
    );
}

#[gpui_kit::test]
fn stale_and_invalid_source_reject_tree_edits_and_late_results_are_dropped(
    tcx: &mut TestAppContext,
) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let original = fixture(dir.path(), "split.yaml", b"value: \"one\"\nother: true\n");
    let host: SharedHost = FileHost::new(HostId(2923));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    open_ready(&app, &mut cx, host, &original);
    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx), "the YAML document splits");
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
        let file = app.active_buffer().expect("YAML buffer");
        (
            file.presentations.yaml_existing(tab, right).expect("front"),
            file.presentations.yaml_existing(tab, left).expect("other"),
        )
    });
    assert_eq!(
        other.read_with(&cx, |view, _| view.mode()),
        YamlMode::Structure
    );
    assert_eq!(
        front.read_with(&cx, |view, _| view.mode()),
        YamlMode::Source
    );

    click_row(&other, &mut cx, "$[\"value\"]");
    click_button(&other, &mut cx, "yaml-edit-value");
    assert!(panel_open(&other, &mut cx));
    app.update_in(&mut cx, |app, window, cx| {
        front.update(cx, |view, cx| {
            let focus = view.source().read(cx).focus_handle(cx);
            focus.focus(window, cx);
        });
        app.editor_split_follow_focus(window, cx);
    });
    let text = source_text(&app, &cx);
    let at = text.find("\"one\"").expect("fixture value");
    // An equal-length edit keeps the target's path and token range identical,
    // so the pending panel must be refused by its opening revision alone.
    type_source(&app, &mut cx, at..at + "\"one\"".len(), "\"two\"");
    until(&mut cx, |cx| is_ready(&front, cx) && is_ready(&other, cx));
    assert_eq!(
        panel_error(&other, &mut cx).as_deref(),
        Some(
            "The document changed, so this edit was not written. Close this panel and select the node again."
        )
    );
    apply_panel(&other, &mut cx);
    assert_eq!(
        source_text(&app, &cx),
        "value: \"two\"\nother: true\n",
        "the stale panel must not overwrite the newer equal-length value"
    );
    assert!(
        panel_open(&other, &mut cx),
        "a stale command stays open for a decision"
    );
    cancel_panel(&other, &mut cx);

    // A length-changing edit no longer matches the target's range; the stale
    // command still cannot write.
    click_row(&other, &mut cx, "$[\"value\"]");
    click_button(&other, &mut cx, "yaml-edit-value");
    assert!(panel_open(&other, &mut cx));
    app.update_in(&mut cx, |app, window, cx| {
        front.update(cx, |view, cx| {
            let focus = view.source().read(cx).focus_handle(cx);
            focus.focus(window, cx);
        });
        app.editor_split_follow_focus(window, cx);
    });
    let text = source_text(&app, &cx);
    let at = text.find("\"two\"").expect("edited value");
    type_source(&app, &mut cx, at..at + "\"two\"".len(), "\"source\"");
    until(&mut cx, |cx| is_ready(&front, cx) && is_ready(&other, cx));
    assert_eq!(
        panel_error(&other, &mut cx).as_deref(),
        Some(
            "The document changed, so this edit was not written. Close this panel and select the node again."
        )
    );
    apply_panel(&other, &mut cx);
    assert!(source_text(&app, &cx).contains("\"source\""));
    assert!(panel_open(&other, &mut cx));
    cancel_panel(&other, &mut cx);

    // Invalid source has no tree; a pending command cannot guess.
    click_row(&other, &mut cx, "$[\"value\"]");
    click_button(&other, &mut cx, "yaml-edit-value");
    app.update_in(&mut cx, |app, window, cx| {
        front.update(cx, |view, cx| {
            let focus = view.source().read(cx).focus_handle(cx);
            focus.focus(window, cx);
        });
        app.editor_split_follow_focus(window, cx);
    });
    let text = source_text(&app, &cx);
    let at = text.find("\"source\"").expect("edited value");
    type_source(&app, &mut cx, at..at + "\"source\"".len(), "\"unterminated");
    until(&mut cx, |cx| {
        other.read_with(cx, |view, cx| view.fatal_error(cx).is_some())
    });
    apply_panel(&other, &mut cx);
    assert!(panel_error(&other, &mut cx).is_some());
    assert!(source_text(&app, &cx).contains("\"unterminated"));
    cancel_panel(&other, &mut cx);

    // Repairing lets a fresh command land in the one shared history. Focus the
    // source presentation explicitly: closing the panel returned focus to the
    // structure group.
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
    click_row(&other, &mut cx, "$[\"value\"]");
    click_button(&other, &mut cx, "yaml-edit-value");
    replace_panel_text(&other, &mut cx, "structure-edit-value", "\"tree-final\"");
    apply_panel(&other, &mut cx);
    until(&mut cx, |cx| is_ready(&other, cx));
    assert!(source_text(&app, &cx).contains("\"tree-final\""));

    // Undo from the source presentation reverses the tree edit.
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

    // A paused parse must not install an older revision after a newer edit.
    let (release, entered) = super::test_support::pause_next_parse();
    let text = source_text(&app, &cx);
    let at = text.find("\"repaired\"").expect("repaired value");
    type_source(&app, &mut cx, at..at + "\"repaired\"".len(), "\"older\"");
    until(&mut cx, |_| {
        entered.load(std::sync::atomic::Ordering::SeqCst)
    });
    let text = source_text(&app, &cx);
    let at = text.find("\"older\"").expect("paused value");
    type_source(&app, &mut cx, at..at + "\"older\"".len(), "\"latest\"");
    drop(release);
    until(&mut cx, |cx| {
        is_ready(&other, cx)
            && row_value(&other, cx, "$[\"value\"]").as_deref() == Some("\"latest\"")
    });
    assert_eq!(
        row_value(&other, &mut cx, "$[\"value\"]").as_deref(),
        Some("\"latest\"")
    );

    save(&app, &mut cx);
    assert!(
        std::fs::read(&original)
            .unwrap()
            .starts_with(b"value: \"latest\"")
    );
}

#[gpui_kit::test]
fn budget_limits_keep_source_editable_and_hide_tree_edits(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let mut big = String::from("key: 1\n#");
    big.push_str(&"x".repeat(STRUCTURE_INPUT_BYTES));
    big.push('\n');
    let big_path = fixture(dir.path(), "big.yaml", big.as_bytes());
    let many_path = fixture(
        dir.path(),
        "many.yaml",
        "- item\n".repeat(STRUCTURE_NODES + 1).as_bytes(),
    );
    let host: SharedHost = FileHost::new(HostId(2924));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));

    open(&app, &mut cx, host.clone(), &big_path);
    let view = yaml_view(&app, &cx);
    until(&mut cx, |cx| {
        view.read_with(cx, |view, cx| view.fatal_error(cx).is_some())
    });
    let (title, detail) = view
        .read_with(&cx, |view, cx| view.fatal_error(cx))
        .expect("input budget error");
    assert!(title.contains("unavailable"), "title: {title}");
    assert!(detail.contains("1 MiB"), "detail: {detail}");
    cx.update(|window, _| {
        assert!(window.find("status-yaml-error").visible());
        assert!(
            window
                .try_find(("yaml-edit-value", view.entity_id()))
                .is_none()
        );
    });
    toggle_mode(&view, &mut cx);
    assert_eq!(view.read_with(&cx, |view, _| view.mode()), YamlMode::Source);

    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.is_empty())
    });
    open(&app, &mut cx, host, &many_path);
    let many = yaml_view(&app, &cx);
    until(&mut cx, |cx| {
        many.read_with(cx, |view, cx| view.fatal_error(cx).is_some())
    });
    let (title, detail) = many
        .read_with(&cx, |view, cx| view.fatal_error(cx))
        .expect("node budget error");
    assert!(title.contains("unavailable"), "title: {title}");
    assert!(detail.contains("20000"), "detail: {detail}");
    assert!(
        many.read_with(&cx, |view, cx| view.node_count(cx))
            .is_none()
    );
}

#[gpui_kit::test]
fn edits_survive_save_as_failed_writes_conflicts_and_cancelled_close(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let original = fixture(dir.path(), "persist.yaml", b"text: \"one\"\n");
    let target = dir.path().join("persisted.yaml");
    let host = FileHost::new(HostId::LOCAL);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (_, view) = open_ready(&app, &mut cx, host.clone(), &original);
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.watching())
    });

    click_row(&view, &mut cx, "$[\"text\"]");
    click_button(&view, &mut cx, "yaml-edit-value");
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
    assert_eq!(
        std::fs::read(&original).unwrap(),
        b"text: \"one\"\n",
        "Save As leaves the original untouched"
    );

    click_row(&view, &mut cx, "$[\"text\"]");
    click_button(&view, &mut cx, "yaml-edit-value");
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
    dismiss_save_error(&mut cx);
    save(&app, &mut cx);
    let retried = replace_bytes(&saved, b"\"changed\"", b"\"retried\"");
    assert_eq!(std::fs::read(&target).unwrap(), retried);

    click_row(&view, &mut cx, "$[\"text\"]");
    click_button(&view, &mut cx, "yaml-edit-value");
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
        app.read_with(&cx, |app, _| app
            .active_buffer()
            .is_some_and(|file| file.dirty)),
        "the committed tree edit must be dirty before the external rewrite"
    );
    std::fs::write(&target, b"text: external\n").unwrap();
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

    click_row(&view, &mut cx, "$[\"text\"]");
    click_button(&view, &mut cx, "yaml-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "\"still-open\"");
    cx.update(|window, cx| window.press("enter", cx));
    settle(&mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(
        source_text(&app, &cx).contains("\"still-open\""),
        "the tree edit reaches the shared source"
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
    save(&app, &mut cx);
    let still_open = replace_bytes(&kept, b"\"kept\"", b"\"still-open\"");
    assert_eq!(std::fs::read(&target).unwrap(), still_open);

    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.is_empty())
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
#[ignore = "requires the explicit private Linux SSH fixture, a matching same-commit local router and the running private remote daemon; run alone with --ignored"]
fn edits_save_through_the_real_native_ssh_route(tcx: &mut TestAppContext) {
    use lyrel_core::host::Host as _;

    crate::ui::i18n::set_locale("en");
    let binary = std::env::var_os("LYREL_TEST_SSH_LOCAL_SERVER").expect(
        "native SSH prerequisite: LYREL_TEST_SSH_LOCAL_SERVER must name the same-commit desktop router binary; also provide the explicit LYREL_TEST_SSH_* fixture endpoint",
    );
    let fixture = crate::native_ssh_graphics_fixture::NativeSshFixture::start(Path::new(&binary));
    let host = fixture.connect_control();
    let script = b"defaults: &defaults\n  retries: 3\ncopy: *defaults\n";
    let _ = fixture.install_script(&host, "editing.yaml", script, Vec::new());
    let path = fixture.remote_path("editing.yaml");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (_, view) = open_ready(&app, &mut cx, host, &path);

    click_row(&view, &mut cx, "$[\"defaults\"][\"retries\"]");
    click_button(&view, &mut cx, "yaml-edit-value");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "5");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    save(&app, &mut cx);

    let expected = replace_bytes(script, b"retries: 3", b"retries: 5");
    let verifier = fixture.connect_control_as(
        "fixture:native-ssh-292-verify",
        "native-ssh-yaml-292",
        "isolated-fixture",
    );
    let bytes = verifier
        .read_file(&path, expected.len() as u64 + 1)
        .expect("independent remote read");
    eprintln!(
        "yaml292 real native SSH saved {} bytes at {}",
        bytes.len(),
        path.display()
    );
    assert_eq!(bytes, expected);
    assert!(
        bytes
            .windows(15)
            .any(|window| window == b"copy: *defaults".as_slice())
    );
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        expected
    );
}
