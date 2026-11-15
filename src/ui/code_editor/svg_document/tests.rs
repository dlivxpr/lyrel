//! #283 的生产工作台接缝：SVG 源码、共享历史与安全原生预览。
//!
//! 这些测试只走工作台入口（`editor_open_on_host`、真实文本输入、Delete/Undo、
//! Save/Save As、关闭与重开）、真实 gpui 测试平台窗口和自有临时文件，断言
//! 可见与落盘行为；不检查源码接线，也不依赖协调目录的夹具。
//! 它们不是原生 OS 窗口，真实桌面、系统 IME 与 SSH 传输由单独验收覆盖。

use super::super::test_host::{FileHost, canonical_tempdir};
use super::super::*;
use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext, size};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// BOM + mixed CRLF/LF, so an untouched save and a targeted edit both have
/// representation to lose if the ordinary text codec is used by mistake.
const ORIGINAL: &[u8] = b"\xef\xbb\xbf<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40\" height=\"30\">\r\n  <rect width=\"40\" height=\"30\" fill=\"#c81e3c\"/>\n</svg>\n";

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
            "SVG document transition did not settle"
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

fn svg_view(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<SvgView> {
    app.read_with(cx, |app, _| {
        app.active_editor_input()
            .expect("a document in front")
            .svg()
            .expect("an SVG document")
            .clone()
    })
}

fn source_document(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<Document> {
    svg_view(app, cx).read_with(cx, |view, cx| view.document().read(cx).document().clone())
}

fn source_text(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> String {
    source_document(app, cx).read_with(cx, |document, _| document.source().to_owned())
}

fn preview_size(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Option<(f32, f32)> {
    svg_view(app, cx).read_with(cx, |view, cx| view.preview_size(cx))
}

fn preview_failure(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Option<SharedString> {
    svg_view(app, cx).read_with(cx, |view, cx| view.preview_failure(cx))
}

fn rendered_revision(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Option<u64> {
    svg_view(app, cx).read_with(cx, |view, cx| view.document().read(cx).rendered_revision())
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
                .is_some_and(|file| file.path == path && file.input.svg().is_some())
        })
    });
    active_id(app, cx)
}

fn open_with_preview(
    app: &Entity<LyrelApp>,
    cx: &mut VisualTestContext,
    host: SharedHost,
    path: &Path,
    expected: (f32, f32),
) -> BufferId {
    let id = open(app, cx, host, path);
    until(cx, |cx| preview_size(app, cx) == Some(expected));
    id
}

fn wait_preview(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, expected: (f32, f32)) {
    until(cx, |cx| preview_size(app, cx) == Some(expected));
}

/// Replaces the source range through the workbench's own text capability. The
/// source side is focused first, so this is the same path typing takes.
fn replace_source(
    app: &Entity<LyrelApp>,
    cx: &mut VisualTestContext,
    range: std::ops::Range<usize>,
    replacement: &str,
) {
    let replacement = replacement.to_owned();
    app.update_in(cx, |app, window, cx| {
        app.focus_editor(window, cx);
        let input = app.active_editor_input().expect("SVG document");
        input
            .text()
            .unwrap()
            .replace(range, &replacement, window, cx);
    });
    settle(cx);
}

fn type_source(
    app: &Entity<LyrelApp>,
    cx: &mut VisualTestContext,
    range: std::ops::Range<usize>,
    text: &str,
) {
    app.update_in(cx, |app, window, cx| {
        app.focus_editor(window, cx);
        let view = app
            .active_editor_input()
            .expect("SVG document")
            .svg()
            .expect("SVG presentation")
            .clone();
        view.update(cx, |view, cx| {
            let source = view.source().clone();
            source.update(cx, |source, cx| source.select_range(range, cx));
        });
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

/// The first `width="…"` attribute in source, including its quoted value.
fn width_range(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> std::ops::Range<usize> {
    let text = source_text(app, cx);
    let at = text.find("width=\"").expect("fixture width");
    let value = at + "width=\"".len();
    let close = text[value..].find('"').expect("closing quote") + value + 1;
    at..close
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
fn source_edit_preview_and_save_keep_one_document_and_exact_bytes(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let remote = canonical_tempdir();
    let local = canonical_tempdir();
    // The mapped Host's file and a same-named local decoy: only the owning
    // Host's bytes may be read or written.
    let remote_file = fixture(remote.path(), "scene.svg", ORIGINAL);
    let local_file = fixture(
        local.path(),
        "scene.svg",
        b"LOCAL DECOY MUST STAY UNCHANGED",
    );
    let host = FileHost::mapped(HostId(283), remote.path());
    let path = PathBuf::from("/scene.svg");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));

    let id = open_with_preview(&app, &mut cx, host.clone(), &path, (40., 30.));
    app.read_with(&cx, |app, cx| {
        let file = app.editor.buffers.get(id).expect("installed document");
        assert_eq!(file.language(), "svg");
        assert!(!file.dirty, "an untouched SVG opens clean");
        let format = file.format(cx).expect("source format");
        assert!(format.bom, "the UTF-8 BOM is part of the saved bytes");
        assert_eq!(format.encoding, encoding_rs::UTF_8);
        assert!(
            app.editor.local_file(id).is_none(),
            "no code language service"
        );
        assert!(app.svg_document_active());
    });
    assert_eq!(
        rendered_revision(&app, &cx),
        Some(source_document(&app, &cx).read_with(&cx, |document, _| document.revision())),
        "the installed preview names the shared revision it rendered"
    );
    cx.update(|window, _| {
        assert!(window.find("svg-mode-toggle").visible());
        assert!(window.find("status-svg-mode").visible());
        assert!(window.find("status-svg-size").visible());
        assert!(window.try_find("status-cursor").is_some());
        assert!(window.try_find("status-image-readonly").is_none());
    });
    // The default SVG side is source; the preview is the other side.
    assert_eq!(
        svg_view(&app, &cx).read_with(&cx, |view, _| view.mode()),
        SvgMode::Source
    );

    // View switching and the real framing keys work over that source.
    cx.update(|window, cx| window.click("svg-mode-toggle", cx));
    settle(&mut cx);
    let view = svg_view(&app, &cx);
    assert_eq!(view.read_with(&cx, |view, _| view.mode()), SvgMode::Preview);
    let view_id = view.entity_id();
    cx.update(|window, _| {
        assert!(window.find(("svg-preview", view_id)).visible());
    });
    cx.update(|window, cx| window.press("1", cx));
    settle(&mut cx);
    cx.update(|window, _| {
        assert_eq!(
            window.find(("svg-preview-pixels", view_id)).bounds().size,
            size(px(40.), px(30.)),
            "SVG 1:1 uses its CSS pixel dimensions, not the centering viewport"
        );
    });
    cx.update(|window, cx| window.press(&secondary("="), cx));
    settle(&mut cx);
    cx.update(|window, _| {
        assert_eq!(
            window.find(("svg-preview-pixels", view_id)).bounds().size,
            size(px(50.), px(37.5)),
            "zoom changes the real rendered SVG dimensions"
        );
    });
    cx.update(|window, cx| window.press("0", cx));
    settle(&mut cx);
    cx.update(|window, _| {
        assert_eq!(
            window.find(("svg-preview-pixels", view_id)).bounds().size,
            size(px(40.), px(30.)),
            "fit does not enlarge a small SVG"
        );
    });
    cx.update(|window, cx| window.click("svg-mode-toggle", cx));
    settle(&mut cx);
    assert_eq!(
        svg_view(&app, &cx).read_with(&cx, |view, _| view.mode()),
        SvgMode::Source
    );

    // Untouched save writes the same BOM and every mixed terminator.
    save(&app, &mut cx);
    assert_eq!(std::fs::read(&remote_file).unwrap(), ORIGINAL);

    // A targeted source edit changes only that attribute.
    let range = width_range(&app, &cx);
    type_source(&app, &mut cx, range, "width=\"64\"");
    let edited = replace_bytes(ORIGINAL, b"width=\"40\"", b"width=\"64\"");
    app.read_with(&cx, |app, _| {
        assert!(app.active_buffer().is_some_and(|file| file.dirty));
    });
    wait_preview(&app, &mut cx, (64., 30.));
    assert_eq!(
        rendered_revision(&app, &cx),
        Some(source_document(&app, &cx).read_with(&cx, |document, _| document.revision())),
        "the new preview describes the new revision, not the old one"
    );

    // Undo/redo are the same shared history; the preview follows both.
    cx.update(|window, cx| window.press(&secondary("z"), cx));
    wait_preview(&app, &mut cx, (40., 30.));
    assert_eq!(
        source_text(&app, &cx),
        String::from_utf8(ORIGINAL.to_vec())
            .unwrap()
            .replace("\r\n", "\n")
            .trim_start_matches('\u{feff}')
            .to_owned()
    );
    cx.update(|window, cx| window.press(&secondary("shift-z"), cx));
    wait_preview(&app, &mut cx, (64., 30.));

    // Save and reopen through the owning Host. Reopen is independent of the
    // in-memory history and must observe the exact same bytes.
    save(&app, &mut cx);
    assert_eq!(std::fs::read(&remote_file).unwrap(), edited);
    assert_eq!(
        std::fs::read(&local_file).unwrap(),
        b"LOCAL DECOY MUST STAY UNCHANGED",
        "the same path on this machine was never read or written"
    );

    // The dirty dot and unsaved close ask before sacrificing the edit.
    let range = width_range(&app, &cx);
    type_source(&app, &mut cx, range, "width=\"72\"");
    wait_preview(&app, &mut cx, (72., 30.));
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| cx.has_pending_prompt());
    cx.simulate_prompt_answer(t(L10nKey::EditorDiscard));
    settle(&mut cx);
    app.read_with(&cx, |app, _| {
        assert!(
            app.editor.buffers.get(id).is_none(),
            "discarded close released it"
        );
    });
    assert_eq!(
        std::fs::read(&remote_file).unwrap(),
        edited,
        "discarding an edit does not write it"
    );

    let reopened = open_with_preview(&app, &mut cx, host.clone(), &path, (64., 30.));
    assert_ne!(reopened, id, "a fresh buffer identity");
    assert_eq!(
        source_text(&app, &cx),
        String::from_utf8(edited.clone())
            .unwrap()
            .replace("\r\n", "\n")
            .trim_start_matches('\u{feff}')
            .to_owned()
    );
    assert_eq!(std::fs::read(&remote_file).unwrap(), edited);
}

#[gpui_kit::test]
fn split_groups_share_document_history_and_show_both_sides(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let path = fixture(dir.path(), "split.svg", ORIGINAL);
    let host: SharedHost = FileHost::new(HostId(284));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    open_with_preview(&app, &mut cx, host, &path, (40., 30.));

    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx), "the SVG splits");
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
        let file = app.active_buffer().expect("SVG buffer");
        (
            file.presentations
                .svg_existing(tab, right)
                .expect("front view"),
            file.presentations
                .svg_existing(tab, left)
                .expect("other view"),
        )
    });
    assert_ne!(front, other, "each group draws its own presentation");
    app.read_with(&cx, |app, cx| {
        let _ = app.active_buffer().expect("SVG buffer");
        assert_eq!(
            front.read(cx).document().entity_id(),
            other.read(cx).document().entity_id(),
            "one preview entity behind both"
        );
        assert_eq!(
            front.read(cx).document().read(cx).document().entity_id(),
            other.read(cx).document().read(cx).document().entity_id(),
            "one Document and history behind both"
        );
    });
    assert_eq!(other.read_with(&cx, |view, _| view.mode()), SvgMode::Source);
    assert_eq!(
        front.read_with(&cx, |view, _| view.mode()),
        SvgMode::Preview
    );

    // Typing in the left source updates the right preview.
    let range = width_range(&app, &cx);
    app.update_in(&mut cx, |app, window, cx| {
        let source = other.read(cx).source().clone();
        source.update(cx, |source, cx| source.select_range(range.clone(), cx));
        source.update(cx, |source, cx| source.focus_handle(cx).focus(window, cx));
        app.editor_split_follow_focus(window, cx);
    });
    cx.update(|window, cx| window.input("width=\"64\"", cx));
    settle(&mut cx);
    wait_preview(&app, &mut cx, (64., 30.));

    // The other presentation's Undo goes back through the same history.
    app.update_in(&mut cx, |app, window, cx| {
        let source = front.read(cx).source().clone();
        front.update(cx, |view, cx| view.toggle_mode(window, cx));
        source.update(cx, |source, cx| source.focus_handle(cx).focus(window, cx));
        app.editor_split_follow_focus(window, cx);
        cx.notify();
    });
    settle(&mut cx);
    cx.update(|window, cx| {
        window.press(&secondary("z"), cx);
    });
    wait_preview(&app, &mut cx, (40., 30.));
    assert_eq!(
        rendered_revision(&app, &cx),
        Some(source_document(&app, &cx).read_with(&cx, |document, _| document.revision())),
        "both presentations observe the reverted revision"
    );
    // Redo from the same presentation restores the edit.
    cx.simulate_keystrokes("secondary-shift-z");
    wait_preview(&app, &mut cx, (64., 30.));
}

#[gpui_kit::test]
fn scripts_and_nested_or_network_images_stay_closed(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let host: SharedHost = FileHost::new(HostId(285));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);

    // A script is refused a runtime, not executed: the shape still renders.
    let script = fixture(
        dir.path(),
        "script.svg",
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30"><script>require('fs').writeFileSync('pwned.txt','x')</script><rect width="40" height="30" fill="#123"/></svg>"##,
    );
    open_with_preview(&app, &mut cx, host.clone(), &script, (40., 30.));
    assert!(
        !dir.path().join("pwned.txt").exists(),
        "an SVG script never runs in the native preview"
    );

    // A nested same-named local file is not read: the resolver is closed.
    let decoy = fixture(dir.path(), "secret.png", b"DECOY MUST STAY UNCHANGED");
    let nested = fixture(
        dir.path(),
        "nested.svg",
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30"><image href="secret.png" width="10" height="10"/></svg>"#,
    );
    open(&app, &mut cx, host.clone(), &nested);
    until(&mut cx, |cx| {
        preview_failure(&app, cx)
            .is_some_and(|message| message.contains(t(L10nKey::MarkdownImageExternalReference)))
    });
    assert_eq!(
        std::fs::read(&decoy).unwrap(),
        b"DECOY MUST STAY UNCHANGED",
        "the nested file was not read"
    );

    // A data image is a nested resource too under Lyrel's stricter policy.
    let data = fixture(
        dir.path(),
        "data.svg",
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30"><image href="data:image/png;base64,iVBORw0KGgo=" width="10" height="10"/></svg>"#,
    );
    open(&app, &mut cx, host.clone(), &data);
    until(&mut cx, |cx| {
        preview_failure(&app, cx)
            .is_some_and(|message| message.contains(t(L10nKey::MarkdownImageExternalReference)))
    });

    // A network image opens no connection at all.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let network = fixture(
        dir.path(),
        "network.svg",
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30"><image href="http://{address}/leak" width="10" height="10"/></svg>"#
        )
        .as_bytes(),
    );
    open(&app, &mut cx, host, &network);
    until(&mut cx, |cx| {
        preview_failure(&app, cx)
            .is_some_and(|message| message.contains(t(L10nKey::MarkdownImageExternalReference)))
    });
    std::thread::sleep(Duration::from_millis(150));
    let mut connection = None;
    for _ in 0..10 {
        match listener.accept() {
            Ok(stream) => {
                connection = Some(stream);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(error) => panic!("fixture listener: {error}"),
        }
    }
    assert!(
        connection.is_none(),
        "the preview made no network request for a nested image"
    );

    // The last failed document still has editable source and a title/state.
    app.read_with(&cx, |app, _| {
        assert!(app.active_buffer().is_some_and(|file| file.input.is_text()));
    });
    let failed_id = svg_view(&app, &cx).entity_id();
    cx.update(|window, _| {
        assert!(window.find("svg-mode-toggle").visible());
        assert!(window.find(("svg-preview-failed", failed_id)).visible());
    });
}

#[gpui_kit::test]
fn invalid_and_oversized_sources_keep_editing_and_recover(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let host: SharedHost = FileHost::new(HostId(286));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);

    // Invalid XML: the source is a document, the preview is a visible error.
    let broken = fixture(
        dir.path(),
        "broken.svg",
        b"\xef\xbb\xbf<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40\" height=\"30\">\r\n  <rect width=\"10\" height=\"10\"/>",
    );
    open(&app, &mut cx, host.clone(), &broken);
    until(&mut cx, |cx| {
        preview_failure(&app, cx)
            .is_some_and(|message| message.contains(t(L10nKey::EditorSvgInvalid)))
    });
    app.read_with(&cx, |app, _| {
        let file = app.active_buffer().expect("still open");
        assert!(file.input.text().is_some(), "source stays editable");
        assert!(!file.dirty, "an invalid file is not an edit");
    });
    // Repair by appending the missing end tag; only the preview changes.
    let len = source_text(&app, &cx).len();
    replace_source(&app, &mut cx, len..len, "</svg>");
    wait_preview(&app, &mut cx, (40., 30.));
    let repaired = {
        let mut bytes = std::fs::read(&broken).unwrap();
        bytes.extend_from_slice(b"</svg>");
        bytes
    };
    save(&app, &mut cx);
    assert_eq!(std::fs::read(&broken).unwrap(), repaired);

    // Over-budget dimensions fail visibly without destroying the source.
    let oversized = fixture(
        dir.path(),
        "oversized.svg",
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="20000" height="30"><rect width="10" height="10"/></svg>"#,
    );
    open(&app, &mut cx, host.clone(), &oversized);
    until(&mut cx, |cx| {
        preview_failure(&app, cx)
            .is_some_and(|message| message.contains(t(L10nKey::EditorSvgLimit)))
    });
    let text = source_text(&app, &cx);
    let at = text.find("width=\"20000\"").expect("oversized width");
    type_source(
        &app,
        &mut cx,
        at..at + "width=\"20000\"".len(),
        "width=\"40\"",
    );
    wait_preview(&app, &mut cx, (40., 30.));
    assert!(
        preview_failure(&app, &cx).is_none(),
        "a corrected source recovers the preview"
    );

    // Width and height absent but a viewBox present: the viewBox is the size.
    let viewed = fixture(
        dir.path(),
        "viewbox.svg",
        br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 80 40"><rect width="80" height="40"/></svg>"#,
    );
    open_with_preview(&app, &mut cx, host.clone(), &viewed, (80., 40.));
    // Even with neither, the browser default viewport is used, not 100×100.
    let dimensionless = fixture(
        dir.path(),
        "default.svg",
        br#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="10" height="10"/></svg>"#,
    );
    open_with_preview(&app, &mut cx, host, &dimensionless, (300., 150.));
}

#[gpui_kit::test]
fn save_as_and_external_reload_keep_the_owning_host(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let remote_file = fixture(dir.path(), "original.svg", ORIGINAL);
    let host = FileHost::new(HostId(287));
    let path = remote_file.clone();
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open_with_preview(&app, &mut cx, host.clone(), &path, (40., 30.));

    // Save As adopts the new target only after the owning Host writes it.
    let range = width_range(&app, &cx);
    type_source(&app, &mut cx, range, "width=\"64\"");
    wait_preview(&app, &mut cx, (64., 30.));
    cx.dispatch_action(crate::core::actions::EditorSaveAs);
    settle(&mut cx);
    let target = dir.path().join("renamed.svg");
    cx.update(|window, cx| {
        window.press("secondary-a", cx);
        window.input(&target.display().to_string(), cx);
        window.press("enter", cx);
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.path == target && !file.dirty && !file.saving)
        })
    });
    let renamed_bytes = replace_bytes(ORIGINAL, b"width=\"40\"", b"width=\"64\"");
    assert_eq!(std::fs::read(&target).unwrap(), renamed_bytes);
    assert_eq!(
        std::fs::read(&remote_file).unwrap(),
        ORIGINAL,
        "the source keeps its own bytes after Save As"
    );

    // Independent reopen observes the saved bytes and the same preview.
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.is_empty())
    });
    open_with_preview(&app, &mut cx, host, &target, (64., 30.));
}

#[gpui_kit::test]
fn external_reload_and_dirty_conflict_keep_source_and_bytes(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let path = fixture(dir.path(), "watched.svg", ORIGINAL);
    let host: SharedHost = lyrel_core::host::local::LocalHost::new();
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open_with_preview(&app, &mut cx, host, &path, (40., 30.));
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.watching())
    });

    // A clean external rewrite is reloaded through the source codec and
    // reaches the preview; no edit is invented and no conflict is raised.
    let rewritten = b"\xef\xbb\xbf<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"50\" height=\"20\">\r\n<rect width=\"50\" height=\"20\"/>\n</svg>\n";
    std::fs::write(&path, rewritten).unwrap();
    until(&mut cx, |cx| preview_size(&app, cx) == Some((50., 20.)));
    app.read_with(&cx, |app, _| {
        assert!(!app.active_buffer().unwrap().dirty);
        assert!(app.active_buffer().unwrap().conflict.is_none());
    });
    assert_eq!(
        source_document(&app, &cx).read_with(&cx, |document, _| document.encode().unwrap()),
        rewritten
    );

    // A dirty edit plus another external write is a conflict, not a silent
    // replacement. Keep mine keeps the source, then saves it.
    let range = width_range(&app, &cx);
    type_source(&app, &mut cx, range, "width=\"60\"");
    wait_preview(&app, &mut cx, (60., 20.));
    let conflicting =
        b"\xef\xbb\xbf<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"/>\n";
    std::fs::write(&path, conflicting).unwrap();
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.conflict.is_some())
        })
    });
    assert!(source_text(&app, &cx).contains("width=\"60\""));
    cx.update(|window, cx| window.click("editor-conflict-keep", cx));
    settle(&mut cx);
    save(&app, &mut cx);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        replace_bytes(rewritten, b"width=\"50\"", b"width=\"60\"")
    );
}

#[gpui_kit::test]
fn a_late_preview_never_overwrites_a_newer_revision(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let path = fixture(dir.path(), "late.svg", ORIGINAL);
    let host: SharedHost = FileHost::new(HostId(288));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);

    // Pause an admitted parse, then make multiple edits while it is in flight.
    // No stale revision may be published; pending edits collapse to the latest.
    let (release, entered) = test_support::pause_next_render();
    open(&app, &mut cx, host, &path);
    until(&mut cx, |_| {
        entered.load(std::sync::atomic::Ordering::SeqCst)
    });
    let range = width_range(&app, &cx);
    type_source(&app, &mut cx, range, "width=\"56\"");
    let range = width_range(&app, &cx);
    type_source(&app, &mut cx, range, "width=\"64\"");
    let revision = source_document(&app, &cx).read_with(&cx, |document, _| document.revision());
    assert!(source_text(&app, &cx).contains("width=\"64\""));
    assert_eq!(rendered_revision(&app, &cx), None);

    release.send(()).unwrap();
    until(&mut cx, |cx| {
        let rendered = rendered_revision(&app, cx);
        assert!(rendered.is_none() || rendered == Some(revision));
        preview_size(&app, cx) == Some((64., 30.))
    });
    assert_eq!(rendered_revision(&app, &cx), Some(revision));

    // A later ordinary edit still installs normally.
    let range = width_range(&app, &cx);
    type_source(&app, &mut cx, range, "width=\"48\"");
    wait_preview(&app, &mut cx, (48., 30.));
}
