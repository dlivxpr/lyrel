//! #294 的生产工作台接缝：XML 的有序结构、命名空间、混合内容与保存。
//!
//! 这些测试只走工作台入口：真实文件打开、真实指针／键盘、源码输入、
//! Undo/Redo、Save、另存为、失败写入、冲突、关闭与重开，以及映射 Host 与
//! 同名本机诱饵。每个输入都在自己的临时目录生成，不依赖协调目录的临时
//! 夹具；断言的是可见行、结构行顺序、源码 occurrence、文档 revision、
//! history 与落盘字节，不是内部缓存形状。它们不是原生 OS 窗口或真实 SSH
//! 验收，相关边界在交付记录中保留。

use super::super::test_host::{FileHost, canonical_tempdir};
use super::super::*;
use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::test::{ElementSnapshot, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext, size};
use lyrel_document::xml::projection;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// BOM + mixed CRLF/LF + XML declaration + PI + default/prefixed namespaces +
/// a single-quoted attribute with an entity + comment + CDATA + mixed content.
/// An untouched save has representation to lose if any serializer leaks into
/// the path.
const ORIGINAL: &[u8] = b"\xef\xbb\xbf<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<?fixture owned?>\r\n<root xmlns=\"urn:root\" xmlns:p='urn:part' p:flag='old&amp;raw'>\n  <!-- keep this comment -->\r\n  <p:item id=\"1\">first</p:item>\n  <![CDATA[raw < & text]]>\r\n  <p:item id=\"2\">second</p:item>\n  tail text\n</root>\r\n";

const SIMPLE: &[u8] = b"<?xml version=\"1.0\"?>\n<r xmlns=\"urn:root\" xmlns:p=\"urn:part\">\n  <p:a p:flag=\"x\"/>\n  <b>text</b>\n</r>\n";

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
            "XML document transition did not settle"
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

fn xml_view(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<XmlView> {
    app.read_with(cx, |app, _| {
        app.active_editor_input()
            .expect("a document in front")
            .xml()
            .expect("an XML document")
            .clone()
    })
}

fn source_document(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<Document> {
    xml_view(app, cx).read_with(cx, |view, cx| view.document().read(cx).document().clone())
}

fn source_text(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> String {
    source_document(app, cx).read_with(cx, |document, _| document.source().to_owned())
}

fn is_ready(view: &Entity<XmlView>, cx: &VisualTestContext) -> bool {
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
                .is_some_and(|file| file.path == path && file.input.xml().is_some())
        })
    });
    active_id(app, cx)
}

fn open_ready(
    app: &Entity<LyrelApp>,
    cx: &mut VisualTestContext,
    host: SharedHost,
    path: &Path,
) -> (BufferId, Entity<XmlView>) {
    let id = open(app, cx, host, path);
    let view = xml_view(app, cx);
    until(cx, |cx| is_ready(&view, cx));
    (id, view)
}

fn scoped(view: &Entity<XmlView>) -> gpui_kit::ElementId {
    ("xml-document", view.entity_id()).into()
}

fn row(view: &Entity<XmlView>, cx: &mut VisualTestContext, label: &str) -> ElementSnapshot {
    let scope = scoped(view);
    cx.update(|window, _| window.within(scope).find_by_label(label))
}

fn maybe_row(
    view: &Entity<XmlView>,
    cx: &mut VisualTestContext,
    label: &str,
) -> Option<ElementSnapshot> {
    let scope = scoped(view);
    cx.update(|window, _| window.within(scope).try_find_by_label(label))
}

fn row_value(view: &Entity<XmlView>, cx: &mut VisualTestContext, label: &str) -> Option<String> {
    maybe_row(view, cx, label).and_then(|row| row.value().map(str::to_owned))
}

fn row_badge(view: &Entity<XmlView>, cx: &mut VisualTestContext, label: &str) -> Option<String> {
    maybe_row(view, cx, label).and_then(|row| row.description().map(str::to_owned))
}

fn click_row(view: &Entity<XmlView>, cx: &mut VisualTestContext, label: &str) {
    let snapshot = row(view, cx, label);
    let id = snapshot.path().last().cloned().expect("a row element id");
    cx.update(|window, cx| window.click(id, cx));
    settle(cx);
}

fn ensure_expanded(view: &Entity<XmlView>, cx: &mut VisualTestContext, label: &str) {
    if maybe_row(view, cx, label).and_then(|row| row.expanded()) == Some(false) {
        click_row(view, cx, label);
    }
}

fn double_click_row(view: &Entity<XmlView>, cx: &mut VisualTestContext, label: &str) {
    let snapshot = row(view, cx, label);
    let id = snapshot.path().last().cloned().expect("a row element id");
    cx.update(|window, cx| window.double_click(id, cx));
    settle(cx);
}

fn selected_slice(
    app: &Entity<LyrelApp>,
    view: &Entity<XmlView>,
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

fn toggle_mode(view: &Entity<XmlView>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click(("xml-mode-toggle", view.entity_id()), cx));
    settle(cx);
}

fn type_source(
    app: &Entity<LyrelApp>,
    cx: &mut VisualTestContext,
    range: std::ops::Range<usize>,
    text: &str,
) {
    let view = xml_view(app, cx);
    app.update_in(cx, |app, window, cx| {
        app.focus_editor(window, cx);
        let source = view.read(cx).source().clone();
        source.update(cx, |source, cx| source.select_range(range, cx));
    });
    cx.update(|window, cx| window.input(text, cx));
    settle(cx);
}

fn type_source_on(
    _app: &Entity<LyrelApp>,
    view: &Entity<XmlView>,
    cx: &mut VisualTestContext,
    range: std::ops::Range<usize>,
    text: &str,
) {
    // The split's stale guard is about the shared Document revision, not the
    // keyboard route (that is covered by the single-presentation consumers).
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            let source = view.source().clone();
            source.update(cx, |source, cx| source.replace_range(range, text, cx));
        });
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

fn click_button(view: &Entity<XmlView>, cx: &mut VisualTestContext, id: &'static str) {
    cx.update(|window, cx| window.click((id, view.entity_id()), cx));
    settle(cx);
}

fn panel_open(view: &Entity<XmlView>, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, _| {
        window
            .try_find(("structure-edit", view.entity_id()))
            .is_some_and(|panel| panel.visible())
    })
}

fn panel_error(view: &Entity<XmlView>, cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(("structure-edit-error", view.entity_id()))
            .and_then(|error| error.value().map(str::to_owned))
    })
}

fn mutation_error(view: &Entity<XmlView>, cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(("xml-mutation-error", view.entity_id()))
            .and_then(|error| error.value().map(str::to_owned))
    })
}

fn type_edit_value(view: &Entity<XmlView>, cx: &mut VisualTestContext, text: &str) {
    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.focus_edit_value(window, cx));
        window.press("secondary-a", cx);
        window.input(text, cx);
    });
    settle(cx);
}

fn replace_panel_text(
    view: &Entity<XmlView>,
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

fn focus_panel_value(view: &Entity<XmlView>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.focus_edit_value(window, cx));
    });
    settle(cx);
}

fn apply_panel(view: &Entity<XmlView>, cx: &mut VisualTestContext) {
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

fn cancel_panel(view: &Entity<XmlView>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click(("structure-edit-cancel", view.entity_id()), cx));
    settle(cx);
}

#[gpui_kit::test]
fn mapped_host_ordered_mixed_content_edits_and_exact_save(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let remote = canonical_tempdir();
    let local = canonical_tempdir();
    let remote_file = fixture(remote.path(), "document.xml", ORIGINAL);
    let local_file = fixture(
        local.path(),
        "document.xml",
        b"LOCAL DECOY MUST STAY UNCHANGED",
    );
    let host = FileHost::mapped(HostId(2940), remote.path());
    let path = PathBuf::from("/document.xml");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));

    let (id, view) = open_ready(&app, &mut cx, host.clone(), &path);
    app.read_with(&cx, |app, cx| {
        let file = app.editor.buffers.get(id).expect("installed document");
        assert!(!file.dirty, "an untouched XML opens clean");
        let format = file.format(cx).expect("source format");
        assert!(format.bom, "the UTF-8 BOM is part of the saved bytes");
        assert_eq!(format.encoding, encoding_rs::UTF_8);
        assert!(
            app.editor.local_file(id).is_none(),
            "native XML is not a code language-service file"
        );
        assert!(app.xml_document_active());
    });
    assert_eq!(
        view.read_with(&cx, |view, _| view.mode()),
        XmlMode::Structure,
        "XML opens in the structure view"
    );
    cx.update(|window, _| {
        assert!(window.find(("xml-mode-toggle", view.entity_id())).visible());
        assert!(window.find("status-xml-mode").visible());
        assert!(window.find("status-xml-count").visible());
        assert!(window.try_find("status-image-readonly").is_none());
    });

    // The structure retains the declaration, PI, namespaces, attributes,
    // comment, CDATA and mixed content in source order.
    assert_eq!(row(&view, &mut cx, "/root").expanded(), Some(true));
    assert_eq!(
        row_value(&view, &mut cx, "/root/@p:flag").as_deref(),
        Some("'old&amp;raw'"),
        "the single-quote style and entity spelling are displayed raw"
    );
    assert_eq!(
        row_value(&view, &mut cx, "/root/@xmlns:p").as_deref(),
        Some("'urn:part'")
    );
    assert_eq!(
        row_badge(&view, &mut cx, "/root/p:item[1]").as_deref(),
        Some("urn:part"),
        "the prefixed element keeps its resolved namespace"
    );
    assert_eq!(
        row_value(&view, &mut cx, "/root/cdata()[1]").as_deref(),
        Some("raw < & text")
    );
    assert_eq!(
        row_value(&view, &mut cx, "/root/comment()[1]").as_deref(),
        Some(" keep this comment ")
    );

    // Walking down from the comment proves CDATA, text, empty and non-empty
    // elements remain separate occurrences in source order.
    click_row(&view, &mut cx, "/root/comment()[1]");
    let mut order = Vec::new();
    for _ in 0..7 {
        press(&app, &mut cx, "down");
        order.push(
            view.read_with(&cx, |view, cx| view.selected_path(cx))
                .expect("selected row"),
        );
    }
    assert_eq!(
        order,
        [
            "/root/text()[2]",
            "/root/p:item[1]",
            "/root/text()[3]",
            "/root/cdata()[1]",
            "/root/text()[4]",
            "/root/p:item[2]",
            "/root/text()[5]",
        ]
    );

    // Enter reveals the exact element, including its end tag.
    click_row(&view, &mut cx, "/root/p:item[2]");
    assert!(
        selected_slice(&app, &view, &cx)
            .expect("selected element")
            .contains("<p:item id=\"2\">second</p:item>")
    );
    reveal_selected(&app, &mut cx);
    assert_eq!(view.read_with(&cx, |view, _| view.mode()), XmlMode::Source);
    let source_range = view.read_with(&cx, |view, cx| view.source().read(cx).selected_range());
    let text = source_text(&app, &cx);
    assert!(text[source_range].contains("<p:item id=\"2\">second</p:item>"));
    toggle_mode(&view, &mut cx);

    // Full value is the exact attribute spelling, not a decoded projection.
    double_click_row(&view, &mut cx, "/root/@p:flag");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.full_value_text(cx))
            .as_deref(),
        Some("p:flag='old&amp;raw'")
    );
    cx.update(|window, cx| window.click(("xml-full-value-copy", view.entity_id()), cx));
    settle(&mut cx);
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some("p:flag='old&amp;raw'".to_owned())
    );
    cx.update(|window, cx| window.click(("xml-full-value-close", view.entity_id()), cx));
    settle(&mut cx);

    // Untouched save writes the same BOM, declarations and every terminator.
    save(&app, &mut cx);
    assert_eq!(std::fs::read(&remote_file).unwrap(), ORIGINAL);

    // Edit the prefixed attribute value; the quote style, namespace
    // declarations, comment, CDATA and unrelated text stay byte-identical.
    click_row(&view, &mut cx, "/root/@p:flag");
    click_button(&view, &mut cx, "xml-edit-value");
    assert!(panel_open(&view, &mut cx));
    assert_eq!(
        view.read_with(&cx, |view, cx| view.edit_texts(cx)),
        Some((None, "old&amp;raw".to_owned()))
    );
    type_edit_value(&view, &mut cx, "new&lt;raw");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(!panel_open(&view, &mut cx));
    let text = source_text(&app, &cx);
    assert!(text.contains("p:flag='new&lt;raw'"));
    assert!(text.contains("xmlns=\"urn:root\""));
    assert!(text.contains("xmlns:p='urn:part'"));
    assert!(text.contains("<!-- keep this comment -->"));
    assert!(text.contains("<![CDATA[raw < & text]]>"));

    // Edit one text occurrence, the comment and CDATA in the shared panel.
    ensure_expanded(&view, &mut cx, "/root/p:item[2]");
    click_row(&view, &mut cx, "/root/p:item[2]/text()[1]");
    click_button(&view, &mut cx, "xml-edit-value");
    type_edit_value(&view, &mut cx, "中文 &amp; 文本");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert_eq!(
        row_value(&view, &mut cx, "/root/p:item[2]/text()[1]").as_deref(),
        Some("中文 &amp; 文本")
    );

    click_row(&view, &mut cx, "/root/comment()[1]");
    click_button(&view, &mut cx, "xml-edit-value");
    type_edit_value(&view, &mut cx, "新注释");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));

    click_row(&view, &mut cx, "/root/cdata()[1]");
    click_button(&view, &mut cx, "xml-edit-value");
    type_edit_value(&view, &mut cx, "甲 < 乙 & 丙");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    let text = source_text(&app, &cx);
    assert!(text.contains("<![CDATA[甲 < 乙 & 丙]]>"));
    assert!(text.contains("<!--新注释-->"));
    assert!(text.find("first").unwrap() < text.find("<![CDATA[").unwrap());
    assert!(
        text.find("<![CDATA[").unwrap()
            < text
                .find("<p:item id=\"2\">中文 &amp; 文本")
                .expect("edited second item")
    );

    // Tree focus keeps the tree open; its undo/redo is the shared history.
    assert_eq!(
        view.read_with(&cx, |view, _| view.mode()),
        XmlMode::Structure
    );
    cx.update(|window, cx| window.press(&secondary("z"), cx));
    until(&mut cx, |cx| is_ready(&view, cx));
    assert_eq!(
        view.read_with(&cx, |view, _| view.mode()),
        XmlMode::Structure
    );
    assert!(source_text(&app, &cx).contains("raw < & text"));
    assert!(!source_text(&app, &cx).contains("甲 < 乙 & 丙"));
    cx.update(|window, cx| window.press(&secondary("shift-z"), cx));
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(source_text(&app, &cx).contains("<![CDATA[甲 < 乙 & 丙]]>"));

    save(&app, &mut cx);
    let mut expected = replace_bytes(ORIGINAL, b"old&amp;raw", b"new&lt;raw");
    expected = replace_bytes(&expected, b"second", "中文 &amp; 文本".as_bytes());
    expected = replace_bytes(&expected, b" keep this comment ", "新注释".as_bytes());
    expected = replace_bytes(&expected, b"raw < & text", "甲 < 乙 & 丙".as_bytes());
    assert_eq!(std::fs::read(&remote_file).unwrap(), expected);
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
    ensure_expanded(&reopened, &mut cx, "/root/p:item[2]");
    assert_eq!(
        row_value(&reopened, &mut cx, "/root/p:item[2]/text()[1]").as_deref(),
        Some("中文 &amp; 文本")
    );
    assert_eq!(
        row_value(&reopened, &mut cx, "/root/cdata()[1]").as_deref(),
        Some("甲 < 乙 & 丙")
    );
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        expected
    );
}

#[gpui_kit::test]
fn add_rename_remove_keep_namespace_meaning_and_source_order(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let path = fixture(dir.path(), "namespaces.xml", SIMPLE);
    let host: SharedHost = FileHost::new(HostId(2941));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (_, view) = open_ready(&app, &mut cx, host.clone(), &path);

    // Add an attribute at the element's start-tag boundary.
    click_row(&view, &mut cx, "/r");
    click_button(&view, &mut cx, "xml-add-attribute");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.edit_texts(cx)),
        Some((Some(String::new()), String::new()))
    );
    replace_panel_text(&view, &mut cx, "structure-edit-key", "id");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "1&amp;2");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    let text = source_text(&app, &cx);
    assert!(text.contains("<r xmlns=\"urn:root\" xmlns:p=\"urn:part\" id=\"1&amp;2\">"));
    assert!(text.contains("<p:a p:flag=\"x\"/>"));

    // Add a prefixed child and then text; both use the same shared panel and
    // the same source history. The child is inserted before the close tag.
    click_row(&view, &mut cx, "/r");
    click_button(&view, &mut cx, "xml-add-child");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.edit_texts(cx)),
        Some((None, String::new()))
    );
    replace_panel_text(&view, &mut cx, "structure-edit-value", "p:c");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(source_text(&app, &cx).contains("  <p:c/>\n</r>"));
    assert_eq!(
        row_badge(&view, &mut cx, "/r/p:c[1]").as_deref(),
        Some("urn:part")
    );

    click_row(&view, &mut cx, "/r");
    click_button(&view, &mut cx, "xml-add-text");
    type_edit_value(&view, &mut cx, "tail text");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    let text = source_text(&app, &cx);
    assert!(text.contains("  <p:c/>\n  tail text\n</r>"));

    // Renaming to a prefix that is not in scope would change namespace
    // meaning; the whole-result check rejects it without touching source.
    let before = source_text(&app, &cx);
    ensure_expanded(&view, &mut cx, "/r");
    click_row(&view, &mut cx, "/r/p:a[1]");
    click_button(&view, &mut cx, "xml-rename");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "q:a");
    apply_panel(&view, &mut cx);
    assert!(panel_error(&view, &mut cx).is_some());
    assert_eq!(source_text(&app, &cx), before);
    cancel_panel(&view, &mut cx);

    // Renaming the same qualified name's local part keeps the prefix's
    // namespace meaning.
    click_row(&view, &mut cx, "/r/p:a[1]");
    click_button(&view, &mut cx, "xml-rename");
    replace_panel_text(&view, &mut cx, "structure-edit-value", "p:b");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert_eq!(
        row_badge(&view, &mut cx, "/r/p:b[1]").as_deref(),
        Some("urn:part")
    );
    assert!(source_text(&app, &cx).contains("<p:b p:flag=\"x\"/>"));

    // Deleting a namespace declaration that is still used is rejected.
    let before = source_text(&app, &cx);
    click_row(&view, &mut cx, "/r/@xmlns:p");
    click_button(&view, &mut cx, "xml-remove");
    assert_eq!(source_text(&app, &cx), before);
    assert!(source_text(&app, &cx).contains("xmlns:p=\"urn:part\""));
    assert!(mutation_error(&view, &mut cx).is_some());

    // An ordinary attribute and a non-root element can be removed locally.
    ensure_expanded(&view, &mut cx, "/r/p:b[1]");
    click_row(&view, &mut cx, "/r/p:b[1]/@p:flag");
    click_button(&view, &mut cx, "xml-remove");
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(!source_text(&app, &cx).contains("p:flag=\"x\""));
    assert!(source_text(&app, &cx).contains("<p:b/>"));
    click_row(&view, &mut cx, "/r/p:b[1]");
    click_button(&view, &mut cx, "xml-remove");
    until(&mut cx, |cx| is_ready(&view, cx));
    let text = source_text(&app, &cx);
    assert!(!text.contains("<p:b/>"));
    assert!(text.contains("<b>text</b>"));
    assert!(text.contains("  tail text\n</r>"));
    save(&app, &mut cx);
    let expected =
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), expected);
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.is_empty())
    });
    let (_, reopened) = open_ready(&app, &mut cx, host, &path);
    assert_eq!(
        row_value(&reopened, &mut cx, "/r/@id").as_deref(),
        Some("\"1&amp;2\"")
    );
    assert!(row_value(&reopened, &mut cx, "/r/p:c[1]").is_some());
}

#[gpui_kit::test]
fn invalid_source_dtd_denial_budgets_and_late_parse_never_publish_stale(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let host: SharedHost = FileHost::new(HostId(2942));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));

    // Invalid XML: the structure reports the parse error, source stays whole
    // and editable, and a repair installs the tree again.
    let broken_bytes = b"\xef\xbb\xbf<r>\r\n  <a>ok</a>\n  <b>\r\n</r>\n";
    let broken = fixture(dir.path(), "broken.xml", broken_bytes);
    let _ = open(&app, &mut cx, host.clone(), &broken);
    let broken_view = xml_view(&app, &cx);
    until(&mut cx, |cx| {
        broken_view.read_with(cx, |view, cx| view.fatal_error(cx).is_some())
    });
    cx.update(|window, _| {
        assert!(
            window
                .find(("xml-structure-error-body", broken_view.entity_id()))
                .visible()
        );
        assert!(
            window
                .find(("xml-structure-error", broken_view.entity_id()))
                .visible()
        );
    });
    app.read_with(&cx, |app, _| {
        assert!(app.active_buffer().unwrap().input.text().is_some());
        assert!(!app.active_buffer().unwrap().dirty);
    });
    toggle_mode(&broken_view, &mut cx);
    let text = source_text(&app, &cx);
    let at = text.find("<b>").expect("broken element");
    type_source(&app, &mut cx, at..at + 3, "<b/>");
    until(&mut cx, |cx| is_ready(&broken_view, cx));
    toggle_mode(&broken_view, &mut cx);
    assert!(row_value(&broken_view, &mut cx, "/r/b[1]").is_some());
    save(&app, &mut cx);
    assert_eq!(
        std::fs::read(&broken).unwrap(),
        replace_bytes(broken_bytes, b"<b>\r\n</r>", b"<b/>\r\n</r>")
    );

    // An external DTD/entity declaration is never resolved; the visible
    // denial names it and the source remains byte-identical and editable.
    let sentinel = fixture(dir.path(), "never-read.txt", b"PRIVATE SENTINEL 294");
    let denied_bytes =
        b"<!DOCTYPE r SYSTEM 'file:///never-read-294.xml'>\n<r>&external;</r>\n".to_vec();
    let denied = fixture(dir.path(), "denied.xml", &denied_bytes);
    let _ = open(&app, &mut cx, host.clone(), &denied);
    let denied_view = xml_view(&app, &cx);
    until(&mut cx, |cx| {
        denied_view.read_with(cx, |view, cx| view.fatal_error(cx).is_some())
    });
    let (title, detail) = denied_view
        .read_with(&cx, |view, cx| view.fatal_error(cx))
        .expect("visible DTD denial");
    assert!(title.contains("DTD"));
    assert!(detail.contains("external"));
    assert_eq!(source_text(&app, &cx).as_bytes(), denied_bytes.as_slice());
    save(&app, &mut cx);
    assert_eq!(std::fs::read(&denied).unwrap(), denied_bytes);
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"PRIVATE SENTINEL 294",
        "no external file content reached or changed the document"
    );
    toggle_mode(&denied_view, &mut cx);
    type_source(&app, &mut cx, 0..0, "<!-- edited -->\n");
    assert!(source_text(&app, &cx).starts_with("<!-- edited -->"));
    toggle_mode(&denied_view, &mut cx);
    until(&mut cx, |cx| {
        denied_view.read_with(cx, |view, cx| view.fatal_error(cx).is_some())
    });

    // Over the structure input budget: explicit failure, source still opens,
    // edits and saves byte-for-byte.
    let big_bytes =
        format!("<r>{}</r>", "x".repeat(projection::XML_INPUT_BYTES + 1024)).into_bytes();
    let big = fixture(dir.path(), "big.xml", &big_bytes);
    let _ = open(&app, &mut cx, host.clone(), &big);
    let big_view = xml_view(&app, &cx);
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

    // Node budget: too many occurrences are refused, not silently truncated.
    let many_bytes = format!("<r>{}</r>", "<a/>".repeat(projection::XML_NODES + 2)).into_bytes();
    let many = fixture(dir.path(), "many.xml", &many_bytes);
    let _ = open(&app, &mut cx, host.clone(), &many);
    let many_view = xml_view(&app, &cx);
    until(&mut cx, |cx| {
        many_view.read_with(cx, |view, cx| view.fatal_error(cx).is_some())
    });
    assert!(many_view.read_with(&cx, |view, cx| {
        view.fatal_error(cx)
            .is_some_and(|(_, detail)| detail.contains("node"))
    }));
    toggle_mode(&many_view, &mut cx);
    save(&app, &mut cx);
    assert_eq!(std::fs::read(&many).unwrap(), many_bytes);

    // A paused parse must never publish a stale revision. The edits collapse
    // to the latest revision, and only that projection is installed.
    let late_bytes = b"<r><v>first</v></r>\n";
    let late = fixture(dir.path(), "late.xml", late_bytes);
    let (release, entered) = test_support::pause_next_parse();
    let _ = open(&app, &mut cx, host, &late);
    let late_view = xml_view(&app, &cx);
    until(&mut cx, |_| {
        entered.load(std::sync::atomic::Ordering::SeqCst)
    });
    toggle_mode(&late_view, &mut cx);
    let text = source_text(&app, &cx);
    let at = text.find("first").expect("fixture value");
    type_source(&app, &mut cx, at..at + "first".len(), "mid");
    let text = source_text(&app, &cx);
    let at = text.find("mid").expect("edited value");
    type_source(&app, &mut cx, at..at + "mid".len(), "latest");
    let revision = source_document(&app, &cx).read_with(&cx, |document, _| document.revision());
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
    click_row(&late_view, &mut cx, "/r/v[1]");
    assert_eq!(
        row_value(&late_view, &mut cx, "/r/v[1]/text()[1]").as_deref(),
        Some("latest")
    );
}

#[gpui_kit::test]
fn stale_pending_edit_and_split_keep_one_history(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let original = fixture(dir.path(), "split-edit.xml", SIMPLE);
    let host: SharedHost = FileHost::new(HostId(2943));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    open_ready(&app, &mut cx, host, &original);

    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx), "the XML document splits");
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
        let file = app.active_buffer().expect("XML buffer");
        (
            file.presentations.xml_existing(tab, right).expect("front"),
            file.presentations.xml_existing(tab, left).expect("other"),
        )
    });
    assert_eq!(
        other.read_with(&cx, |view, _| view.mode()),
        XmlMode::Structure
    );
    assert_eq!(front.read_with(&cx, |view, _| view.mode()), XmlMode::Source);

    // Folds are presentation-local.
    assert_eq!(row(&other, &mut cx, "/r").expanded(), Some(true));
    click_row(&other, &mut cx, "/r");
    assert_eq!(row(&other, &mut cx, "/r").expanded(), Some(false));
    toggle_mode(&front, &mut cx);
    assert_eq!(
        row(&front, &mut cx, "/r").expanded(),
        Some(true),
        "the other presentation keeps its own fold"
    );

    // A same-length source edit leaves the selected text node's path, range
    // and kind unchanged. The panel's opened revision is the guard: the
    // pending value must not write over the newer source.
    click_row(&other, &mut cx, "/r");
    click_row(&other, &mut cx, "/r/b[1]");
    click_row(&other, &mut cx, "/r/b[1]/text()[1]");
    click_button(&other, &mut cx, "xml-edit-value");
    assert!(panel_open(&other, &mut cx));
    type_edit_value(&other, &mut cx, "pending");
    app.update_in(&mut cx, |app, window, cx| {
        front.update(cx, |view, cx| {
            let focus = view.source().read(cx).focus_handle(cx);
            focus.focus(window, cx);
        });
        app.editor_split_follow_focus(window, cx);
    });
    let text = source_text(&app, &cx);
    let at = text.find("text</b>").expect("fixture text");
    type_source_on(&app, &front, &mut cx, at..at + "text".len(), "TEXT");
    assert!(
        source_text(&app, &cx).contains("<b>TEXT</b>"),
        "the source edit must land before testing the stale panel"
    );
    until(&mut cx, |cx| is_ready(&front, cx) && is_ready(&other, cx));
    focus_panel_value(&other, &mut cx);
    apply_panel(&other, &mut cx);
    assert_eq!(
        panel_error(&other, &mut cx).as_deref(),
        Some(crate::ui::i18n::t(
            crate::ui::i18n::L10nKey::EditorXmlEditStale
        ))
    );
    assert!(source_text(&app, &cx).contains("<b>TEXT</b>"));
    assert!(!source_text(&app, &cx).contains("<b>pending</b>"));
    assert!(
        panel_open(&other, &mut cx),
        "a stale command stays open for a decision"
    );
    cancel_panel(&other, &mut cx);

    // A different-length edit is also rejected without writing.
    click_row(&other, &mut cx, "/r/b[1]/text()[1]");
    click_button(&other, &mut cx, "xml-edit-value");
    assert!(panel_open(&other, &mut cx));
    app.update_in(&mut cx, |app, window, cx| {
        front.update(cx, |view, cx| {
            let focus = view.source().read(cx).focus_handle(cx);
            focus.focus(window, cx);
        });
        app.editor_split_follow_focus(window, cx);
    });
    let text = source_text(&app, &cx);
    let at = text.find("TEXT</b>").expect("fixture text");
    type_source_on(&app, &front, &mut cx, at..at + "TEXT".len(), "source-wins");
    assert!(
        source_text(&app, &cx).contains("<b>source-wins</b>"),
        "the source edit must land before testing the stale panel"
    );
    until(&mut cx, |cx| is_ready(&front, cx) && is_ready(&other, cx));
    focus_panel_value(&other, &mut cx);
    apply_panel(&other, &mut cx);
    assert_eq!(
        panel_error(&other, &mut cx).as_deref(),
        Some(crate::ui::i18n::t(
            crate::ui::i18n::L10nKey::EditorXmlEditStale
        ))
    );
    let text = source_text(&app, &cx);
    assert!(text.contains("<b>source-wins</b>"));
    assert!(!text.contains("<b>TEXT</b>"));
    assert!(
        panel_open(&other, &mut cx),
        "a stale command stays open for a decision"
    );
    cancel_panel(&other, &mut cx);

    // A fresh command from the tree lands in the one shared history; undo
    // from the source presentation reverses it.
    click_row(&other, &mut cx, "/r/b[1]/text()[1]");
    click_button(&other, &mut cx, "xml-edit-value");
    type_edit_value(&other, &mut cx, "tree-final");
    focus_panel_value(&other, &mut cx);
    apply_panel(&other, &mut cx);
    until(&mut cx, |cx| is_ready(&other, cx));
    assert!(source_text(&app, &cx).contains("<b>tree-final</b>"));
    cx.update(|_, cx| {
        front.update(cx, |view, cx| {
            let source = view.source().clone();
            source.update(cx, |source, cx| {
                source.undo_document(cx);
            });
        });
    });
    until(&mut cx, |cx| is_ready(&front, cx) && is_ready(&other, cx));
    assert!(source_text(&app, &cx).contains("<b>source-wins</b>"));
    assert!(!source_text(&app, &cx).contains("<b>tree-final</b>"));
    save(&app, &mut cx);
    assert_eq!(
        std::fs::read(&original).unwrap(),
        replace_bytes(SIMPLE, b"text", b"source-wins")
    );
}

#[gpui_kit::test]
fn tree_edits_survive_save_as_failed_writes_conflicts_and_cancelled_close(
    tcx: &mut TestAppContext,
) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let original = fixture(dir.path(), "persist.xml", SIMPLE);
    let target = dir.path().join("persisted.xml");
    let host = FileHost::new(HostId::LOCAL);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (_, view) = open_ready(&app, &mut cx, host.clone(), &original);
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.watching())
    });

    // A tree edit, then Save As to a new owning-Host target.
    click_row(&view, &mut cx, "/r/b[1]");
    click_row(&view, &mut cx, "/r/b[1]/text()[1]");
    click_button(&view, &mut cx, "xml-edit-value");
    type_edit_value(&view, &mut cx, "changed");
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
    assert_eq!(std::fs::read(&original).unwrap(), SIMPLE);

    // A failed owning-Host write leaves the edit dirty and the target bytes
    // unchanged; an ordinary retry then lands the same bytes.
    click_row(&view, &mut cx, "/r/b[1]/text()[1]");
    click_button(&view, &mut cx, "xml-edit-value");
    type_edit_value(&view, &mut cx, "retried");
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
    let retried = replace_bytes(&saved, b"<b>changed</b>", b"<b>retried</b>");
    assert_eq!(std::fs::read(&target).unwrap(), retried);

    // An older in-flight save may land its captured bytes, but it must not
    // mark the newer edit clean. The old file content and the newer source
    // both remain observable.
    let release = host.pause_next_write();
    click_row(&view, &mut cx, "/r/b[1]/text()[1]");
    click_button(&view, &mut cx, "xml-edit-value");
    type_edit_value(&view, &mut cx, "paused");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    cx.dispatch_action(crate::core::actions::EditorSave);
    until(&mut cx, |_| {
        host.entered.load(std::sync::atomic::Ordering::SeqCst)
    });
    click_row(&view, &mut cx, "/r/b[1]/text()[1]");
    click_button(&view, &mut cx, "xml-edit-value");
    type_edit_value(&view, &mut cx, "newer");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    release.send(()).unwrap();
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| !file.saving && file.dirty)
        })
    });
    assert!(source_text(&app, &cx).contains("<b>newer</b>"));
    let paused = replace_bytes(&retried, b"<b>retried</b>", b"<b>paused</b>");
    assert_eq!(std::fs::read(&target).unwrap(), paused);
    if app.read_with(&cx, |app, _| {
        app.active_buffer()
            .is_some_and(|file| file.conflict.is_some())
    }) {
        cx.update(|window, cx| window.click("editor-conflict-keep", cx));
        settle(&mut cx);
    }
    save(&app, &mut cx);
    let newer = replace_bytes(&retried, b"<b>retried</b>", b"<b>newer</b>");
    assert_eq!(std::fs::read(&target).unwrap(), newer);

    // A dirty external write is a conflict, not a silent replacement.
    click_row(&view, &mut cx, "/r/b[1]/text()[1]");
    click_button(&view, &mut cx, "xml-edit-value");
    type_edit_value(&view, &mut cx, "kept");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    std::fs::write(&target, b"<external/>\n").unwrap();
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.conflict.is_some())
        })
    });
    assert!(source_text(&app, &cx).contains("<b>kept</b>"));
    cx.update(|window, cx| window.click("editor-conflict-keep", cx));
    settle(&mut cx);
    save(&app, &mut cx);
    let kept = replace_bytes(&newer, b"<b>newer</b>", b"<b>kept</b>");
    assert_eq!(std::fs::read(&target).unwrap(), kept);
    settle(&mut cx);
    assert!(source_text(&app, &cx).contains("<b>kept</b>"));

    // Cancelling an unsaved close keeps the document and its bytes.
    click_row(&view, &mut cx, "/r/b[1]/text()[1]");
    click_button(&view, &mut cx, "xml-edit-value");
    type_edit_value(&view, &mut cx, "still-open");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    assert!(
        panel_error(&view, &mut cx).is_none(),
        "the production Apply command applies the tree edit"
    );
    assert!(source_text(&app, &cx).contains("<b>still-open</b>"));
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
    let still_open = replace_bytes(&kept, b"<b>kept</b>", b"<b>still-open</b>");
    assert_eq!(std::fs::read(&target).unwrap(), still_open);

    // An independent reopen sees the final tree-edited bytes.
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.is_empty())
    });
    let (_, reopened) = open_ready(&app, &mut cx, host, &target);
    click_row(&reopened, &mut cx, "/r/b[1]");
    assert_eq!(
        row_value(&reopened, &mut cx, "/r/b[1]/text()[1]").as_deref(),
        Some("still-open")
    );
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        still_open
    );
}

#[gpui_kit::test]
fn long_mixed_content_displays_order_and_edits_one_occurrence(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\xef\xbb\xbf<r xmlns:p=\"urn:part\">\r\n");
    for index in 0..1500 {
        bytes.extend_from_slice(
            format!(
                "  <record id=\"{index}\">text{index}<![CDATA[cdata{index}]]><!--comment{index}--><p:item/></record>\r\n"
            )
            .as_bytes(),
        );
    }
    bytes.extend_from_slice(b"  <marker>last-marker-294</marker>\r\n</r>\n");
    let path = fixture(dir.path(), "long.xml", &bytes);
    let host: SharedHost = FileHost::new(HostId(2944));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (_, view) = open_ready(&app, &mut cx, host, &path);
    assert_eq!(
        view.read_with(&cx, |view, cx| view.element_count(cx)),
        Some(3002)
    );
    press(&app, &mut cx, "end");
    press(&app, &mut cx, "up");
    press(&app, &mut cx, "up");
    press(&app, &mut cx, "up");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.selected_path(cx))
            .as_deref(),
        Some("/r/record[1500]")
    );
    click_row(&view, &mut cx, "/r/record[1500]");
    assert_eq!(
        row_value(&view, &mut cx, "/r/record[1500]/text()[1]").as_deref(),
        Some("text1499")
    );
    assert_eq!(
        row_value(&view, &mut cx, "/r/record[1500]/cdata()[1]").as_deref(),
        Some("cdata1499")
    );
    click_row(&view, &mut cx, "/r/record[1500]/text()[1]");
    click_button(&view, &mut cx, "xml-edit-value");
    type_edit_value(&view, &mut cx, "终段文本");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    let text = source_text(&app, &cx);
    assert!(text.contains("<record id=\"1499\">终段文本<![CDATA[cdata1499]]>"));
    assert!(text.contains("<record id=\"0\">text0<![CDATA[cdata0]]>"));
    assert!(text.contains("<!--comment1499-->"));
    assert!(text.contains("<marker>last-marker-294</marker>"));

    // The tail is far beyond the virtualized viewport; navigate like a user,
    // then inspect the last marker in the real structure tree.
    press(&app, &mut cx, "end");
    press(&app, &mut cx, "up");
    assert_eq!(
        view.read_with(&cx, |view, cx| view.selected_path(cx))
            .as_deref(),
        Some("/r/marker[1]")
    );
    click_row(&view, &mut cx, "/r/marker[1]");
    assert_eq!(
        row_value(&view, &mut cx, "/r/marker[1]/text()[1]").as_deref(),
        Some("last-marker-294")
    );
    save(&app, &mut cx);
    let saved = std::fs::read(&path).unwrap();
    assert_eq!(
        saved,
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap())
    );
    assert!(
        saved
            .windows(9)
            .any(|window| window == "<p:item/>".as_bytes())
    );
}

#[gpui_kit::test]
#[ignore = "requires the explicit private Linux SSH fixture, a matching same-commit local router and the running private remote daemon; run alone with --ignored"]
fn xml_edits_save_through_the_real_native_ssh_route(tcx: &mut TestAppContext) {
    use lyrel_core::host::Host as _;

    crate::ui::i18n::set_locale("en");
    let binary = std::env::var_os("LYREL_TEST_SSH_LOCAL_SERVER").expect(
        "native SSH prerequisite: LYREL_TEST_SSH_LOCAL_SERVER must name the same-commit desktop router binary; also provide the explicit LYREL_TEST_SSH_* fixture endpoint",
    );
    let fixture = crate::native_ssh_graphics_fixture::NativeSshFixture::start(Path::new(&binary));
    let host = fixture.connect_control();
    let script = b"<?xml version=\"1.0\"?>\n<r xmlns:p=\"urn:part\">\n  <!-- remote comment -->\n  <p:item p:flag='old&amp;raw'>first</p:item>\n  <![CDATA[raw < & text]]>\n</r>\n";
    let _ = fixture.install_script(&host, "editing.xml", script, Vec::new());
    let path = fixture.remote_path("editing.xml");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    let (_, view) = open_ready(&app, &mut cx, host, &path);

    ensure_expanded(&view, &mut cx, "/r/p:item[1]");
    click_row(&view, &mut cx, "/r/p:item[1]/@p:flag");
    click_button(&view, &mut cx, "xml-edit-value");
    type_edit_value(&view, &mut cx, "remote-edited");
    apply_panel(&view, &mut cx);
    until(&mut cx, |cx| is_ready(&view, cx));
    save(&app, &mut cx);

    let expected = replace_bytes(script, b"old&amp;raw", b"remote-edited");
    let verifier = fixture.connect_control_as(
        "fixture:native-ssh-294-verify",
        "native-ssh-xml-294",
        "isolated-fixture",
    );
    let bytes = verifier
        .read_file(&path, expected.len() as u64 + 1)
        .expect("independent remote read");
    eprintln!(
        "xml294 real native SSH saved {} bytes at {}",
        bytes.len(),
        path.display()
    );
    assert_eq!(bytes, expected);
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        expected
    );
}
