//! Production-workbench seam for draw.io previews.
//!
//! The tests open real files through `open_file_in_editor`/`editor_open_on_host`
//! in a gpui test window, then assert document identity, page state, read-only
//! behaviour and originating-Host reads. The native WebView surface is not
//! started in the automated harness; native rendering, keyboard/IME/focus and
//! platform acceptance are recorded separately.

use super::super::test_host::FileHost;
use super::super::*;
use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
use lyrel_ui_base::actions::CloseActiveTab;
use lyrel_workbench_editor::code_editor::drawio_engine::bridge::Command;
use lyrel_workbench_editor::code_editor::drawio_engine::surface::SurfaceEvent;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn settle(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.render_frame(cx);
    });
    cx.run_until_parked();
}

fn until(cx: &mut VisualTestContext, mut ready: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready(cx) {
        assert!(
            Instant::now() < deadline,
            "draw.io document transition did not settle"
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

const COMPRESSED_PAGE: &str = "jZJNc4QgDIZ/DXeFdsezdj8uPXnomUoUpmCcyFbtr++4wqrd2Zne4HkTkryBicKNZ5KdfkcFlvFEjUy8Mc6zJGGcz2BawCGChoxaULqC0vxAgDHsahT0u0CPaL3p9rDCtoXK75gkwmEfVqPdV+1kAw+grKR9pB9GeR3Gek1WfgHT6Fg5jfM5GYMD6LVUOGyQODJREKJfTm4swM7eRV+WvNMT9d4YQev/k8CXhG9pr2G2Al1H0PegohFLo36K0xNeWwXzAykT+aCNh7KT1awOJDsmcu2dDXJtrC3QIt1yhZKQ1RUTee8Jv2CjHKoMPmsm8tARkIfx6VTp3Ss3ngEdeJoYT2JCtHf6cx/WbfGXwPRmU1lgMnyQ5v706iHjp2BjvK7rummbPy+Ovw==";

/// One genuine draw.io inline page body: a real `mxGraphModel` element, the
/// shape draw.io itself writes. Escaped text is the *compressed* form, so a
/// fixture must not use it to stand for an uncompressed page.
fn inline_model(label: &str) -> String {
    format!(
        "<mxGraphModel dx=\"800\" dy=\"600\"><root><mxCell id=\"0\"/><mxCell id=\"1\" parent=\"0\"/><mxCell id=\"2\" value=\"{label}\" style=\"rounded=1;whiteSpace=wrap;html=1;\" vertex=\"1\" parent=\"1\"><mxGeometry x=\"80\" y=\"80\" width=\"200\" height=\"70\" as=\"geometry\"/></mxCell></root></mxGraphModel>"
    )
}

fn uncompressed(xml_pages: usize) -> Vec<u8> {
    let mut out = String::from("<mxfile host=\"lyrel.test\" type=\"device\">");
    for page in 0..xml_pages {
        out.push_str(&format!(
            "<diagram id=\"u{page}\" name=\"Page {}\">",
            page + 1
        ));
        out.push_str(&format!(
            "<mxGraphModel dx=\"800\" dy=\"600\"><root><mxCell id=\"0\"/><mxCell id=\"1\" parent=\"0\"/><mxCell id=\"2\" value=\"Uncompressed page {page}\" style=\"rounded=1;whiteSpace=wrap;html=1;\" vertex=\"1\" parent=\"1\"><mxGeometry x=\"80\" y=\"80\" width=\"200\" height=\"70\" as=\"geometry\"/></mxCell></root></mxGraphModel>"
        ));
        out.push_str("</diagram>");
    }
    out.push_str("</mxfile>");
    out.into_bytes()
}

fn compressed(pages: usize) -> Vec<u8> {
    let mut out = String::from("<mxfile host=\"lyrel.test\">");
    for page in 0..pages {
        out.push_str(&format!(
            "<diagram id=\"c{page}\" name=\"Packed {}\">{COMPRESSED_PAGE}</diagram>",
            page + 1
        ));
    }
    out.push_str("</mxfile>");
    out.into_bytes()
}

fn open(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, path: &Path) -> BufferId {
    app.update_in(cx, |app, window, cx| {
        app.open_file_in_editor(path, window, cx)
    });
    until(cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.active_buffer(cx)
                .is_some_and(|file| file.path.as_path() == path)
        })
    });
    app.read_with(cx, |app, _| {
        app.tab_code()
            .and_then(TabCode::active_id)
            .expect("a buffer in front")
    })
}

fn view(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<DrawioView> {
    app.read_with(cx, |app, cx| {
        app.active_editor_input(cx)
            .expect("a document in front")
            .drawio()
            .expect("a draw.io document")
            .clone()
    })
}

fn page(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> usize {
    view(app, cx).read_with(cx, |view, _| view.page())
}

fn pages(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> usize {
    view(app, cx).read_with(cx, |view, cx| view.page_count(cx))
}

fn dispatch<A: gpui_kit::Action>(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, action: A) {
    let handle = view(app, cx).read_with(cx, |view, _| view.handle());
    cx.update(|window, cx| handle.dispatch_action(&action, window, cx));
    settle(cx);
}

#[gpui_kit::test]
fn uncompressed_and_compressed_diagrams_open_as_read_only_documents(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let plain = fixture(dir.path(), "plain.drawio", &uncompressed(2));
    let packed = fixture(dir.path(), "packed.drawio", &compressed(3));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);

    let id = open(&app, &mut cx, &plain);
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("installed document");
        assert!(file.input.drawio().is_some(), "a draw.io document");
        assert!(!file.dirty, "a preview is never unsaved");
        assert!(app.drawio_document_active(cx));
        assert_eq!(
            app.editor_active_location(cx)
                .map(|(host, path)| (host, path.to_path_buf())),
            Some((HostId::LOCAL, plain.clone()))
        );
        assert!(
            app.editor.read(cx).local_file(id).is_none(),
            "not a text buffer"
        );
        assert!(file.input.text().is_none());
        assert!(!file.format(cx).is_some(), "no text save format");
    });
    assert_eq!(pages(&app, &cx), 2);
    assert_eq!(page(&app, &cx), 0);
    cx.update(|window, _| {
        assert!(window.find("drawio-format").visible());
        assert!(window.find("drawio-page-facts").visible());
        assert!(window.find("drawio-page-next").visible());
        assert!(window.find("status-drawio-editable").visible());
        assert!(
            window.try_find("status-cursor").is_none(),
            "a diagram has no caret"
        );
        assert!(window.try_find("status-wrap").is_none());
        assert!(window.try_find("status-eol").is_none());
    });

    // Close without a prompt: there is no text to lose.
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    settle(&mut cx);
    app.read_with(&cx, |app, cx| {
        assert!(app.editor.read(cx).buffers.get(id).is_none());
    });

    let packed_id = open(&app, &mut cx, &packed);
    assert_eq!(pages(&app, &cx), 3, "compressed pages are all declared");
    app.read_with(&cx, |app, cx| {
        assert!(app.editor.read(cx).buffers.get(packed_id).is_some());
        assert!(!app.active_buffer(cx).unwrap().dirty);
    });
}

#[gpui_kit::test]
fn page_controls_and_split_groups_keep_their_own_page(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "pages.drawio", &compressed(4));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);

    dispatch(&app, &mut cx, DrawioNextPage);
    assert_eq!(page(&app, &cx), 1);
    let page_facts = view(&app, &cx).read_with(&cx, |view, cx| {
        view.page_label(cx).expect("multi-page label").to_string()
    });
    assert!(
        page_facts.contains("2"),
        "the visible page moved: {page_facts}"
    );

    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx), "the diagram splits");
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
        let file = app.active_buffer(cx).expect("diagram");
        (
            file.presentations
                .drawio_existing(tab, right)
                .expect("front view"),
            file.presentations
                .drawio_existing(tab, left)
                .expect("other view"),
        )
    });
    assert_ne!(front, other);
    let primary = view(&app, &cx);
    let primary_is_front = primary == front;
    let (primary_before, other_before) = app.read_with(&cx, |_, cx| {
        (
            primary.read(cx).page(),
            if primary == front {
                other.read(cx).page()
            } else {
                front.read(cx).page()
            },
        )
    });
    assert_eq!(primary_before, 1, "the page moved before the split");
    assert_eq!(
        other_before, 1,
        "the other view starts from the page that was on screen"
    );

    dispatch(&app, &mut cx, DrawioNextPage);
    app.read_with(&cx, |app, cx| {
        let file = app.active_buffer(cx).expect("diagram");
        let (front, other) = (
            file.presentations.drawio_existing(tab, right).unwrap(),
            file.presentations.drawio_existing(tab, left).unwrap(),
        );
        let (moved, kept) = if primary_is_front {
            (&front, &other)
        } else {
            (&other, &front)
        };
        assert_eq!(moved.read(cx).page(), 2, "the dispatched group moved");
        assert_eq!(kept.read(cx).page(), 1, "the other group kept its page");
        assert_eq!(
            front.read(cx).document().entity_id(),
            other.read(cx).document().entity_id(),
            "one document behind both presentations"
        );
    });
}

#[gpui_kit::test]
fn text_commands_and_save_leave_a_diagram_alone(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let before = compressed(2);
    let path = fixture(dir.path(), "unedited.drawio", &before);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);

    app.update_in(&mut cx, |app, window, cx| {
        app.editor_save_active(window, cx);
        app.editor_go_to_line(window, cx);
        app.editor_go_to_symbol(window, cx);
        app.toggle_line_ending(cx);
        let wrap_before = cx
            .global::<lyrel_ui_base::config::Config>()
            .editor_soft_wrap;
        app.toggle_document_wrap(window, cx);
        assert_eq!(
            cx.global::<lyrel_ui_base::config::Config>()
                .editor_soft_wrap,
            wrap_before
        );
        assert!(!app.editor.read(cx).has_bar());
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        assert!(!file.dirty);
        assert!(file.format(cx).is_none());
        assert!(file.conflict.is_none());
    });
    settle(&mut cx);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "nothing was written over the original"
    );
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_dispatch(&gpui_kit::component::input::Undo, window, cx);
    });
    settle(&mut cx);
    assert_eq!(pages(&app, &cx), 2);
}

#[gpui_kit::test]
fn an_external_rewrite_replaces_the_page_list_and_retires_the_old_view(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "watched.drawio", &compressed(2));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.editor.read(cx).buffers.watching())
    });
    dispatch(&app, &mut cx, DrawioNextPage);
    assert_eq!(page(&app, &cx), 1);

    std::fs::write(&path, uncompressed(3)).unwrap();
    until(&mut cx, |cx| pages(&app, cx) == 3);
    app.read_with(&cx, |app, cx| {
        let file = app.active_buffer(cx).expect("still open");
        assert!(file.conflict.is_none(), "a clean preview reloads itself");
        assert!(!file.dirty);
        let view = file.input.drawio().unwrap();
        assert_eq!(
            view.read(cx).page(),
            1,
            "the page is clamped to the new list"
        );
        assert!(
            view.read(cx).document().read(cx).bytes().is_some(),
            "the new bytes are the document"
        );
    });
}

#[gpui_kit::test]
fn the_owning_host_supplies_the_diagram_bytes(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let local = fixture(dir.path(), "host.drawio", &compressed(2));
    let remote_dir = dir.path().join("remote");
    std::fs::create_dir_all(&remote_dir).unwrap();
    std::fs::write(remote_dir.join("host.drawio"), uncompressed(4)).unwrap();
    let host = FileHost::mapped(HostId(297), &remote_dir);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), Path::new("/host.drawio"), window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.active_buffer(cx)
                .is_some_and(|file| file.host.id() == HostId(297))
        })
    });
    assert_eq!(pages(&app, &cx), 4, "the remote bytes won");
    app.read_with(&cx, |app, cx| {
        let file = app.active_buffer(cx).expect("remote diagram");
        assert_eq!(file.path, Path::new("/host.drawio"));
        assert_ne!(file.path, local);
    });
}

#[gpui_kit::test]
fn malformed_and_non_diagram_files_report_their_own_failure(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let broken = fixture(dir.path(), "broken.drawio", b"<mxfile><diagram>");
    let not_diagram = fixture(
        dir.path(),
        "notes.xml",
        br#"<?xml version="1.0"?><notes><note>text</note></notes>"#,
    );
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &broken);
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("failure document");
        assert!(file.input.drawio().is_some(), "still a diagram place");
        let view = file.input.drawio().unwrap();
        assert_eq!(view.read(cx).page_count(cx), 0);
        assert!(view.read(cx).document().read(cx).failure().is_some());
    });
    cx.update(|window, _| {
        assert!(window.find("drawio-failed").visible());
        assert!(window.find("status-drawio-editable").visible());
    });
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    settle(&mut cx);

    // A generic .xml that is not a draw.io root stays source, not a failure
    // diagram and not an external application.
    let xml_id = open(&app, &mut cx, &not_diagram);
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(xml_id)
            .expect("text document");
        assert!(file.input.text().is_some(), "ordinary XML stays text");
        assert!(file.input.drawio().is_none());
    });
}

/// The native surface is not started by the automated harness, but its event
/// channel is the only owner of engine/page/zoom state. Drive that contract
/// directly so a bridge event cannot silently become a second authority.
#[gpui_kit::test]
fn surface_events_drive_engine_page_and_zoom_state(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "events.drawio", &compressed(3));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);

    let diagram = view(&app, &cx);
    diagram.update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(SurfaceEvent::Ready, cx);
        view.on_surface_event(
            SurfaceEvent::State {
                page: 1,
                pages: 3,
                zoom: 1.5,
            },
            cx,
        );
        view.on_surface_event(
            SurfaceEvent::Blocked {
                reason: "the diagram requested the network".into(),
            },
            cx,
        );
    });
    assert_eq!(
        page(&app, &cx),
        1,
        "a reported state moves the visible page"
    );
    view(&app, &cx).read_with(&cx, |view, cx| {
        assert!(view.blocked.is_some(), "a blocked request stays visible");
        let status = view.status_text(cx);
        assert!(
            status.contains("150%"),
            "the engine zoom is shown: {status}"
        );
    });

    view(&app, &cx).update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(
            SurfaceEvent::Failed {
                message: "engine stopped".into(),
            },
            cx,
        );
    });
    view(&app, &cx).read_with(&cx, |view, cx| {
        assert!(view.status_text(cx).contains("engine stopped"));
        assert!(view.surface.is_none(), "a failed surface is dropped");
    });

    // A close can race the asynchronous start: the recorded state must fall
    // back to idle instead of keeping a pending engine.
    view(&app, &cx).update_in(&mut cx, |view, _window, cx| {
        view.engine = super::EngineStatus::Starting;
        view.on_surface_event(SurfaceEvent::Closed, cx);
    });
    view(&app, &cx).read_with(&cx, |view, _| {
        assert!(
            matches!(view.engine, super::EngineStatus::Idle),
            "closing while starting falls back to idle"
        );
    });
}

/// One engine commit becomes the buffer's dirty authority and the one shared
/// undo history — the workbench's own undo, not an engine-local stack, is
/// what walks the document back through every presentation's edits.
#[gpui_kit::test]
fn an_engine_commit_marks_the_document_dirty_and_shares_one_undo_history(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let original = uncompressed(2);
    let path = fixture(dir.path(), "edited.drawio", &original);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);

    // A committed revision: the same file with one changed cell label.
    let committed: Arc<[u8]> = Arc::from(
        String::from_utf8(uncompressed(2))
            .unwrap()
            .replace("Uncompressed page 0", "Uncompressed page 0 edited")
            .into_bytes()
            .as_slice(),
    );
    let diagram = view(&app, &cx);
    diagram.update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(SurfaceEvent::Ready, cx);
        view.on_surface_event(
            SurfaceEvent::Commit {
                bytes: committed.clone(),
            },
            cx,
        );
    });
    settle(&mut cx);
    // The document took the commit; the buffer's dirty bit is the frame
    // sweep's job, run explicitly here so the assertion is not a race.
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        let document = view_of(file).read(cx).document().read(cx);
        assert_eq!(document.edit_revision(), 1, "the commit landed");
        assert_eq!(
            document.bytes().as_deref(),
            Some(committed.as_ref()),
            "the committed bytes are the document"
        );
        assert!(file.dirty, "a committed edit is unsaved work");
        assert!(document.can_undo());
        assert!(!document.can_redo());
    });

    // The workbench's own undo walks the shared document history — an edit
    // made on this presentation is undone the same way on any other.
    dispatch(&app, &mut cx, DrawioUndo);
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        assert!(
            !file.dirty,
            "undo back to the saved bytes reads clean again"
        );
        let document = view_of(file).read(cx).document().read(cx);
        assert!(!document.can_undo());
        assert!(document.can_redo());
        assert_eq!(
            document.bytes().as_deref(),
            Some(original.as_slice()),
            "the undo restored the opened bytes"
        );
    });
    dispatch(&app, &mut cx, DrawioRedo);
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        assert!(file.dirty, "the redo brings the edit back");
        let document = view_of(file).read(cx).document().read(cx);
        assert_eq!(document.bytes().as_deref(), Some(committed.as_ref()));
    });
}

/// A commit that no longer parses is refused outright: the document keeps
/// the last good bytes instead of taking a truncated serialization.
#[gpui_kit::test]
fn an_unparseable_commit_keeps_the_last_good_bytes(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let original = uncompressed(1);
    let path = fixture(dir.path(), "truncated.drawio", &original);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);
    let diagram = view(&app, &cx);
    diagram.update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(SurfaceEvent::Ready, cx);
        view.on_surface_event(
            SurfaceEvent::Commit {
                bytes: Arc::from(b"<mxfile><diagram".as_slice()),
            },
            cx,
        );
    });
    settle(&mut cx);
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        assert!(
            file.dirty,
            "the canvas holds a refused edit, so the document is unsaved"
        );
        let document = view_of(file).read(cx).document().read(cx);
        assert_eq!(document.edit_revision(), 0);
        assert_eq!(document.bytes().as_deref(), Some(original.as_slice()));
        let reason = view_of(file)
            .read(cx)
            .unconfirmed_reason()
            .expect("the refusal is visible");
        assert!(
            reason.contains("engine commit was refused"),
            "got {reason:?}"
        );
    });
}

/// A save defers through the engine's sync flush: the committed bytes — not
/// the bytes the file was opened with — are what land on disk through the
/// same Host write path.
#[gpui_kit::test]
fn a_save_writes_the_committed_bytes_and_clears_dirty(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "saved.drawio", &uncompressed(1));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);
    let committed: Arc<[u8]> = Arc::from(
        String::from_utf8(uncompressed(1))
            .unwrap()
            .replace("Uncompressed page 0", "Saved edit")
            .into_bytes()
            .as_slice(),
    );
    let diagram = view(&app, &cx);
    diagram.update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(SurfaceEvent::Ready, cx);
        view.on_surface_event(
            SurfaceEvent::Commit {
                bytes: committed.clone(),
            },
            cx,
        );
    });
    settle(&mut cx);
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_save_file(id, false, window, cx);
    });
    settle(&mut cx);
    confirm_pending_sync(&app, &mut cx);
    // The Host write and its UI acknowledgement complete on different executors.
    until(&mut cx, |cx| {
        std::fs::read(&path).unwrap().as_slice() == committed.as_ref()
            && app.read_with(cx, |app, cx| {
                app.editor
                    .read(cx)
                    .buffers
                    .get(id)
                    .is_some_and(|file| !file.dirty)
            })
    });
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        assert!(!file.dirty, "the saved bytes are the document");
        assert!(file.conflict.is_none());
    });
    assert_eq!(
        std::fs::read(&path).unwrap().as_slice(),
        committed.as_ref(),
        "the committed serialization is what was written"
    );
}

/// An external rewrite under a dirty diagram becomes a conflict with real
/// content on both sides, not a silent overwrite of engine work.
#[gpui_kit::test]
fn an_external_rewrite_under_a_dirty_diagram_conflicts(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "raced.drawio", &compressed(2));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.editor.read(cx).buffers.watching())
    });
    let committed: Arc<[u8]> = Arc::from(uncompressed(2).as_slice());
    let diagram = view(&app, &cx);
    diagram.update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(SurfaceEvent::Ready, cx);
        view.on_surface_event(
            SurfaceEvent::Commit {
                bytes: committed.clone(),
            },
            cx,
        );
    });
    settle(&mut cx);
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    app.read_with(&cx, |app, cx| {
        assert!(app.editor.read(cx).buffers.get(id).unwrap().dirty);
    });
    std::fs::write(&path, uncompressed(3)).unwrap();
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.editor
                .read(cx)
                .buffers
                .get(id)
                .is_some_and(|file| file.conflict.is_some())
        })
    });
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        assert!(file.dirty, "the engine's edits are still the document");
        let document = view_of(file).read(cx).document().read(cx);
        assert_eq!(
            document.bytes().as_deref(),
            Some(committed.as_ref()),
            "the external write did not replace live edits"
        );
    });
}

/// A byte-identical commit — the save-time sync flush — does not touch dirty
/// state or push a history entry; it is not an edit.
#[gpui_kit::test]
fn an_identical_commit_is_not_an_edit(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let bytes = uncompressed(1);
    let path = fixture(dir.path(), "flushed.drawio", &bytes);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);
    let diagram = view(&app, &cx);
    diagram.update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(SurfaceEvent::Ready, cx);
        view.on_surface_event(
            SurfaceEvent::Commit {
                bytes: Arc::from(bytes.as_slice()),
            },
            cx,
        );
    });
    settle(&mut cx);
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        assert!(!file.dirty, "an identical sync is not an edit");
        let document = view_of(file).read(cx).document().read(cx);
        assert_eq!(document.edit_revision(), 0, "no content revision");
        assert!(!document.can_undo());
    });
}

fn view_of(file: &OpenFile) -> &Entity<DrawioView> {
    file.input.drawio().expect("a draw.io document")
}

/// Answers the sync request a save just armed, then runs the frame sweep so
/// the deferred write can re-enter. `None` means this save had no live
/// surface to wait for and writes the document bytes directly. The test's
/// `SyncConfirmed` is the same event the native hosts deliver after the
/// page's own POST is accepted.
fn confirm_pending_sync(app: &Entity<LyrelApp>, cx: &mut VisualTestContext) -> Option<u64> {
    let diagram = view(app, cx);
    let request = diagram.read_with(cx, |view, _| view.pending_sync_token());
    if let Some(request) = request {
        diagram.update_in(cx, |view, _window, cx| {
            view.on_surface_event(SurfaceEvent::SyncConfirmed { request }, cx);
        });
    }
    app.update_in(cx, |app, window, cx| app.editor_sync(window, cx));
    request
}

/// A remote diagram's committed bytes save back through the owning mapped
/// host — the file on the other side changes and the original stays put.
#[gpui_kit::test]
fn a_mapped_host_round_trips_the_committed_diagram(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let remote_dir = dir.path().join("remote298");
    std::fs::create_dir_all(&remote_dir).unwrap();
    std::fs::write(remote_dir.join("remote.drawio"), uncompressed(1)).unwrap();
    let host = FileHost::mapped(HostId(298), &remote_dir);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), Path::new("/remote.drawio"), window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.active_buffer(cx)
                .is_some_and(|file| file.host.id() == HostId(298))
        })
    });
    let committed: Arc<[u8]> = Arc::from(uncompressed(2).as_slice());
    let diagram = view(&app, &cx);
    diagram.update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(SurfaceEvent::Ready, cx);
        view.on_surface_event(
            SurfaceEvent::Commit {
                bytes: committed.clone(),
            },
            cx,
        );
    });
    settle(&mut cx);
    let id = app.read_with(&cx, |app, cx| {
        app.active_buffer(cx).expect("remote diagram").id()
    });
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_save_file(id, false, window, cx);
    });
    settle(&mut cx);
    confirm_pending_sync(&app, &mut cx);
    until(&mut cx, |_cx| {
        std::fs::read(remote_dir.join("remote.drawio"))
            .unwrap()
            .as_slice()
            == committed.as_ref()
    });
    assert_eq!(
        std::fs::read(remote_dir.join("remote.drawio"))
            .unwrap()
            .as_slice(),
        committed.as_ref(),
        "the mapped host wrote the committed serialization back"
    );
}

/// The engine's own Ctrl+S is a workbench save: the surface event drains
/// through the frame sweep into the same Host write path as the toolbar.
#[gpui_kit::test]
fn an_engine_save_request_writes_through_the_workbench(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "engine-save.drawio", &uncompressed(1));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);
    let committed: Arc<[u8]> = Arc::from(
        String::from_utf8(uncompressed(1))
            .unwrap()
            .replace("Uncompressed page 0", "Engine save")
            .into_bytes()
            .as_slice(),
    );
    let diagram = view(&app, &cx);
    diagram.update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(SurfaceEvent::Ready, cx);
        view.on_surface_event(
            SurfaceEvent::Commit {
                bytes: committed.clone(),
            },
            cx,
        );
        view.on_surface_event(SurfaceEvent::Save, cx);
    });
    settle(&mut cx);
    // The frame sweep owns the request. Wait for both the Host write and
    // its asynchronous acknowledgement in the editor.
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    settle(&mut cx);
    confirm_pending_sync(&app, &mut cx);
    until(&mut cx, |cx| {
        std::fs::read(&path).unwrap().as_slice() == committed.as_ref()
            && app.read_with(cx, |app, cx| {
                app.editor
                    .read(cx)
                    .buffers
                    .get(id)
                    .is_some_and(|file| !file.dirty)
            })
    });
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    app.read_with(&cx, |app, cx| {
        assert!(
            !app.editor.read(cx).buffers.get(id).unwrap().dirty,
            "the engine save cleared dirty"
        );
    });
    assert_eq!(
        std::fs::read(&path).unwrap().as_slice(),
        committed.as_ref(),
        "the engine's Ctrl+S wrote its committed serialization through the Host"
    );
}

/// A commit that rewrites one page must not cost the file its other pages,
/// its compressed representation, its unknown attributes or its embedded
/// resources: the bytes the workbench serves back decode to both pages, and
/// the untouched page is still there verbatim.
#[gpui_kit::test]
fn a_partial_commit_keeps_every_page_attribute_and_embedded_resource(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let untouched = format!(
        "<diagram id=\"keep\" name=\"Keep\" custom-attr=\"kept\" background=\"#ffeeaa\">{COMPRESSED_PAGE}</diagram>"
    );
    let edited = format!(
        "<diagram id=\"edit\" name=\"Edit\">{}</diagram>",
        inline_model("Edited page")
    );
    let original = format!(
        "<mxfile host=\"lyrel.test\" version=\"24.7.5\" custom=\"kept\" compressed=\"true\">{edited}{untouched}</mxfile>"
    );
    let path = fixture(dir.path(), "multi.drawio", original.as_bytes());
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);

    // One page's model changed; every other node is copied through untouched,
    // exactly the shape the engine's own writePageBack produces for an edit.
    let rewritten = original.replacen("Edited page", "Rewritten page", 1);
    let committed: Arc<[u8]> = Arc::from(rewritten.as_bytes());
    let diagram = view(&app, &cx);
    diagram.update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(SurfaceEvent::Ready, cx);
        view.on_surface_event(
            SurfaceEvent::Commit {
                bytes: committed.clone(),
            },
            cx,
        );
    });
    settle(&mut cx);
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        assert!(file.dirty, "the rewritten page is unsaved work");
        let document = view_of(file).read(cx).document().read(cx);
        assert_eq!(document.page_count(), 2, "both pages survive the commit");
        let bytes = document.bytes().expect("ready bytes");
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.contains("custom-attr=\"kept\""), "page attribute kept");
        assert!(text.contains("custom=\"kept\""), "mxfile attribute kept");
        assert!(
            text.contains(COMPRESSED_PAGE),
            "the untouched page's compressed payload is byte-identical"
        );
        assert!(
            text.contains("Rewritten page"),
            "the edited page carries the engine's serialization"
        );
    });
    // An independent consumer decodes the served bytes back to two pages.
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        let bytes = view_of(file)
            .read(cx)
            .document()
            .read(cx)
            .bytes()
            .expect("ready bytes");
        let source =
            lyrel_document::drawio::parse(&bytes).expect("the committed file still parses");
        assert_eq!(source.pages.len(), 2, "independent decode sees two pages");
    });
}

/// A deferred save asks **every** live presentation for its model — one
/// engine left un-flushed would write bytes older than its canvas — and the
/// write happens once, only after the confirmation, with no second sync.
///
/// A commit retires sibling surfaces (each presentation reloads to the newer
/// document), so the two engines are saved-as while both are live and the
/// confirmation arrives afterwards.
#[gpui_kit::test]
fn a_deferred_save_flushes_every_presentation_and_writes_once(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let original = uncompressed(1);
    let path = fixture(dir.path(), "flush-all.drawio", &original);
    let target = dir.path().join("flush-all-target.drawio");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);
    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx), "the diagram splits");
    });
    settle(&mut cx);
    let views = app.read_with(&cx, |app, cx| {
        let file = app.editor.read(cx).buffers.get(id).expect("diagram");
        let mut views = vec![view_of(file).clone()];
        for (_, _, view) in file.presentations.drawios() {
            if !views.contains(&view) {
                views.push(view);
            }
        }
        views
    });
    assert_eq!(views.len(), 2, "two live presentations");
    for view in &views {
        view.update_in(&mut cx, |view, _window, cx| {
            view.on_surface_event(SurfaceEvent::Ready, cx);
            view.attach_test_surface(cx);
        });
    }
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_save_as_active(window, cx);
    });
    cx.simulate_new_path_selection(|_| Some(target.clone()));
    settle(&mut cx);
    // Both live engines were asked, and nothing was written yet.
    for (index, view) in views.iter().enumerate() {
        assert_eq!(
            view.read_with(&cx, |view, _| view.test_surface_commands()),
            vec![Command::Sync],
            "presentation {index} was not flushed"
        );
    }
    assert!(
        !target.exists(),
        "the write waits for the engine's confirmation"
    );
    let committed: Arc<[u8]> = Arc::from(
        String::from_utf8(uncompressed(1))
            .unwrap()
            .replace("Uncompressed page 0", "Flushed by both")
            .into_bytes()
            .as_slice(),
    );
    let request = views[0]
        .read_with(&cx, |view, _| view.pending_sync_token())
        .expect("the primary requested a sync");
    views[0].update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(
            SurfaceEvent::Commit {
                bytes: committed.clone(),
            },
            cx,
        );
    });
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    assert!(
        !target.exists(),
        "a commit without this sync's own acknowledgement is not the save's confirmation"
    );
    views[0].update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(SurfaceEvent::SyncConfirmed { request }, cx);
    });
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    until(&mut cx, |_cx| {
        std::fs::read(&target).is_ok_and(|bytes| bytes.as_slice() == committed.as_ref())
    });
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    // The committer wrote without a second sync; the sibling reloaded to the
    // newer document, which retires its old engine session instead of asking
    // it to serialize bytes it no longer owns.
    assert_eq!(
        views[0].read_with(&cx, |view, _| view.test_surface_commands()),
        vec![Command::Sync],
        "the confirmed re-entry must not flush the committer again"
    );
    assert!(
        views[1]
            .read_with(&cx, |view, _| view.test_surface_commands())
            .is_empty(),
        "the reloaded sibling retired its stale surface"
    );
    assert_eq!(
        std::fs::read(&path).unwrap().as_slice(),
        original.as_slice(),
        "the original file is untouched by a save-as"
    );
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        assert!(!file.dirty, "the confirmed model is on disk");
        assert_eq!(file.path.as_path(), target.as_path());
    });
}

/// A save waits for every live presentation's own sync request: an ordinary
/// commit, a sibling's confirmation, and a foreign token cannot release it.
#[gpui_kit::test]
fn a_save_waits_for_each_presentations_own_sync_confirmation(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let original = uncompressed(1);
    let path = fixture(dir.path(), "barrier.drawio", &original);
    let target = dir.path().join("barrier-target.drawio");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);
    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx), "the diagram splits");
    });
    settle(&mut cx);
    let views = app.read_with(&cx, |app, cx| {
        let file = app.editor.read(cx).buffers.get(id).expect("diagram");
        let mut views = vec![view_of(file).clone()];
        for (_, _, view) in file.presentations.drawios() {
            if !views.contains(&view) {
                views.push(view);
            }
        }
        views
    });
    assert_eq!(views.len(), 2, "two live presentations");
    for view in &views {
        view.update_in(&mut cx, |view, _window, cx| {
            view.on_surface_event(SurfaceEvent::Ready, cx);
            view.attach_test_surface(cx);
        });
    }
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_save_as_active(window, cx);
    });
    cx.simulate_new_path_selection(|_| Some(target.clone()));
    settle(&mut cx);
    // Both engines were asked; each ticket names its own request.
    let requests: Vec<u64> = views
        .iter()
        .map(|view| {
            view.read_with(&cx, |view, _| view.pending_sync_token())
                .expect("each presentation requested a sync")
        })
        .collect();
    let (primary_request, sibling_request) = (requests[0], requests[1]);
    // Each presentation numbers its own session from zero, so both tokens may
    // be equal; what separates them is the view that owns the request.
    views[1].update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(
            SurfaceEvent::Commit {
                bytes: Arc::from(original.as_slice()),
            },
            cx,
        );
        view.on_surface_event(
            SurfaceEvent::SyncConfirmed {
                request: sibling_request,
            },
            cx,
        );
    });
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    assert!(
        !target.exists(),
        "the sibling's own confirmation does not release the primary's request"
    );
    // A request number this view never sent and an ordinary commit are not
    // confirmations.
    views[0].update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(
            SurfaceEvent::SyncConfirmed {
                request: primary_request + 1,
            },
            cx,
        );
        view.on_surface_event(
            SurfaceEvent::Commit {
                bytes: Arc::from(original.as_slice()),
            },
            cx,
        );
    });
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    assert!(
        !target.exists(),
        "only the primary's own request releases it"
    );
    views[0].update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(
            SurfaceEvent::SyncConfirmed {
                request: primary_request,
            },
            cx,
        );
    });
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    until(&mut cx, |_cx| {
        std::fs::read(&target).is_ok_and(|bytes| bytes.as_slice() == original.as_slice())
    });
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        assert!(!file.dirty, "the confirmed model is on disk");
    });
}

/// A refused canvas commit is unsaved work with a visible reason: it prompts
/// before closing, and a save retries the engine instead of writing the older
/// document bytes.
#[gpui_kit::test]
fn a_refused_canvas_stays_unsaved_and_saves_or_closes_only_with_its_answer(
    tcx: &mut TestAppContext,
) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let original = uncompressed(1);
    let path = fixture(dir.path(), "refused.drawio", &original);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);
    let diagram = view(&app, &cx);
    diagram.update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(SurfaceEvent::Ready, cx);
        view.attach_test_surface(cx);
        view.on_surface_event(
            SurfaceEvent::CommitRejected {
                reason: "HTTP 413".into(),
            },
            cx,
        );
    });
    settle(&mut cx);
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        assert!(
            file.dirty,
            "the document bytes are clean, but the canvas holds a refused edit"
        );
    });
    cx.update(|window, _| {
        let note = window.find("drawio-unconfirmed");
        assert!(note.visible(), "the refusal is visible on the canvas");
        assert_eq!(note.value(), Some("HTTP 413"));
    });

    // A clean close would drop the only copy: it prompts and Cancel keeps the
    // tab and its draft.
    let tab = app.read_with(&cx, |app, _| app.tabs[app.tabs.active_index()].id());
    cx.dispatch_action(CloseActiveTab);
    settle(&mut cx);
    assert!(
        cx.has_pending_prompt(),
        "an unsaved canvas asks before closing"
    );
    cx.simulate_prompt_answer(t(L10nKey::Cancel));
    settle(&mut cx);
    app.read_with(&cx, |app, _| {
        assert!(
            app.tabs.iter().any(|entry| entry.id() == tab),
            "cancelling keeps the tab"
        );
    });

    // Saving retries the engine. Its refusal fails this exact request and
    // leaves the file's last good bytes alone.
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_save_active(window, cx)
    });
    settle(&mut cx);
    let request = view(&app, &cx)
        .read_with(&cx, |view, _| view.pending_sync_token())
        .expect("the save asked the canvas for its model");
    view(&app, &cx).update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(
            SurfaceEvent::SyncRejected {
                request,
                reason: "HTTP 413".into(),
            },
            cx,
        );
    });
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        assert!(
            !file.is_waiting_for_drawio_save(),
            "the refusal settles the save"
        );
        assert!(file.dirty, "the refused model is still unsaved");
    });
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original,
        "a refused save never writes older document bytes"
    );
    until(&mut cx, |cx| {
        cx.update(|window, _| {
            window
                .try_find("notification")
                .is_some_and(|notification| notification.visible())
        })
    });
    cx.update(|window, _| {
        assert!(
            window.try_find("notification").is_some_and(|n| n.visible()),
            "the save failure is visible"
        );
    });

    // Fixing the canvas (here: undoing back to the saved model) clears the
    // warning and the unsaved state without a reload or a second authority.
    view(&app, &cx).update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(
            SurfaceEvent::Commit {
                bytes: Arc::from(original.as_slice()),
            },
            cx,
        );
    });
    settle(&mut cx);
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        assert!(!file.dirty, "the fixed canvas matches the saved bytes");
    });
    cx.update(|window, _| {
        assert!(
            window.try_find("drawio-unconfirmed").is_none(),
            "the visible warning clears with the accepted model"
        );
    });
}

/// A flush that never confirms is a failed save: the old bytes stay on disk,
/// the document stays dirty, and a close waiting on that save is cancelled
/// rather than dropping the engine's work.
#[gpui_kit::test]
fn an_unconfirmed_flush_fails_the_save_instead_of_writing_stale_bytes(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let original = uncompressed(1);
    let path = fixture(dir.path(), "never-confirmed.drawio", &original);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);
    let committed: Arc<[u8]> = Arc::from(
        String::from_utf8(uncompressed(1))
            .unwrap()
            .replace("Uncompressed page 0", "Never confirmed")
            .into_bytes()
            .as_slice(),
    );
    let diagram = view(&app, &cx);
    diagram.update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(SurfaceEvent::Ready, cx);
        view.attach_test_surface(cx);
        view.on_surface_event(
            SurfaceEvent::Commit {
                bytes: committed.clone(),
            },
            cx,
        );
    });
    app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
    // Ask to close the dirty tab and choose Save: the close now waits for the
    // save, which waits for a flush the engine never answers.
    let tab = app.read_with(&cx, |app, _| app.tabs[app.tabs.active_index()].id());
    cx.dispatch_action(CloseActiveTab);
    settle(&mut cx);
    assert!(
        cx.has_pending_prompt(),
        "a dirty diagram asks before closing"
    );
    cx.simulate_prompt_answer(t(L10nKey::Save));
    settle(&mut cx);
    // The save armed a flush on the live engine; without this the assertions
    // below would also hold for a save that never started.
    assert_eq!(
        view(&app, &cx).read_with(&cx, |view, _| view.test_surface_commands()),
        vec![Command::Sync],
        "the close-waiting save asked the engine for its model"
    );
    // Let the (short, test-only) deadline pass without a confirmation, driving
    // the frame sweep each pass so the expiry is actually processed.
    let expiry = Instant::now() + Duration::from_secs(5);
    loop {
        app.update_in(&mut cx, |app, window, cx| app.editor_sync(window, cx));
        settle(&mut cx);
        let expired = app.read_with(&cx, |app, cx| {
            app.editor
                .read(cx)
                .buffers
                .get(id)
                .is_some_and(|file| file.dirty && !file.is_waiting_for_drawio_save())
        });
        if expired {
            break;
        }
        assert!(Instant::now() < expiry, "the flush never expired");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        std::fs::read(&path).unwrap().as_slice(),
        original.as_slice(),
        "an unconfirmed engine model is never replaced by stale bytes"
    );
    app.read_with(&cx, |app, cx| {
        assert!(
            app.tabs.iter().any(|entry| entry.id() == tab),
            "the cancelled close keeps the tab and its unsaved work"
        );
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        assert!(file.dirty, "the engine's model is still unsaved");
        assert!(
            !file.is_waiting_for_drawio_save(),
            "the failed flush is cleared for a retry"
        );
    });
}

/// Save As writes the confirmed engine model to the chosen target and leaves
/// the original file untouched.
#[gpui_kit::test]
fn save_as_writes_the_confirmed_model_to_the_chosen_target(tcx: &mut TestAppContext) {
    lyrel_ui_base::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let original = uncompressed(1);
    let path = fixture(dir.path(), "save-as-source.drawio", &original);
    let target = dir.path().join("save-as-target.drawio");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);
    let committed: Arc<[u8]> = Arc::from(
        String::from_utf8(uncompressed(1))
            .unwrap()
            .replace("Uncompressed page 0", "Save as target")
            .into_bytes()
            .as_slice(),
    );
    let diagram = view(&app, &cx);
    diagram.update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(SurfaceEvent::Ready, cx);
        view.attach_test_surface(cx);
        view.on_surface_event(
            SurfaceEvent::Commit {
                bytes: committed.clone(),
            },
            cx,
        );
    });
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_save_as_active(window, cx);
    });
    cx.simulate_new_path_selection(|_| Some(target.clone()));
    settle(&mut cx);
    // The engine answers the exact flush the save-as asked for.
    confirm_pending_sync(&app, &mut cx);
    until(&mut cx, |_cx| {
        std::fs::read(&target).is_ok_and(|bytes| bytes.as_slice() == committed.as_ref())
    });
    assert_eq!(
        std::fs::read(&target).unwrap().as_slice(),
        committed.as_ref(),
        "the chosen target received the confirmed model"
    );
    assert_eq!(
        std::fs::read(&path).unwrap().as_slice(),
        original.as_slice(),
        "save-as never rewrites the original"
    );
    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        assert_eq!(
            file.path.as_path(),
            target.as_path(),
            "the buffer now owns the new path"
        );
        assert!(!file.dirty);
    });
}

/// The same diagram lifecycle over a real SSH Host: open from the remote,
/// commit an edit, save back through the owning Host, then prove the remote
/// bytes with an independent control client — and that a same-named local
/// decoy never changed.
///
/// Requires the private loopback fixture (explicit `LYREL_TEST_SSH_*` and a
/// same-commit router binary); the engine surface is the production event
/// seam, not a native window.
#[gpui_kit::test]
#[ignore = "requires the explicit private SSH fixture and a same-commit local router binary; run alone with --ignored"]
fn a_remote_diagram_saves_through_the_real_ssh_route(tcx: &mut TestAppContext) {
    use lyrel_core::host::Host as _;

    lyrel_ui_base::i18n::set_locale("en");
    let binary = std::env::var_os("LYREL_TEST_SSH_LOCAL_SERVER").expect(
        "real SSH prerequisite: LYREL_TEST_SSH_LOCAL_SERVER names the same-commit router binary; also provide the explicit LYREL_TEST_SSH_* fixture endpoint",
    );
    let ssh = crate::native_ssh_graphics_fixture::NativeSshFixture::start(Path::new(&binary));
    let host = ssh.connect_control();
    let original = uncompressed(2);
    let name = "remote-298.drawio";
    let remote = ssh.remote_path(name);
    // The fixture installer creates the private remote directory and proves
    // the upload through the owning Host before the editor opens it.
    let _ = ssh.install_script(&host, name, &original, Vec::new());
    // A same-named local decoy must never be the file this buffer writes.
    let decoy_dir = super::super::test_host::canonical_tempdir();
    let decoy = self::fixture(decoy_dir.path(), name, b"decoy");
    let committed: Arc<[u8]> = Arc::from(
        String::from_utf8(uncompressed(2))
            .unwrap()
            .replace("Uncompressed page 0", "Remote engine edit")
            .into_bytes()
            .as_slice(),
    );

    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &remote, window, cx);
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.active_buffer(cx).is_some())
    });
    let id = app.read_with(&cx, |app, cx| {
        app.active_buffer(cx).expect("remote diagram").id()
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.editor
                .read(cx)
                .buffers
                .get(id)
                .is_some_and(|file| file.host.id() == host.id() && !file.host.id().is_local())
        })
    });
    let diagram = view(&app, &cx);
    diagram.update_in(&mut cx, |view, _window, cx| {
        view.on_surface_event(SurfaceEvent::Ready, cx);
        view.on_surface_event(
            SurfaceEvent::Commit {
                bytes: committed.clone(),
            },
            cx,
        );
    });
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_save_file(id, false, window, cx);
    });
    settle(&mut cx);
    confirm_pending_sync(&app, &mut cx);
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| {
            !app.editor.read(cx).buffers.get(id).unwrap().dirty
        })
    });
    // Reopen the same remote path: the document the Host now holds parses and
    // carries the engine's change.
    let reopened = host
        .read_file(&remote, committed.len() as u64 + 1)
        .expect("reopen the remote diagram");
    assert_eq!(
        reopened,
        committed.to_vec(),
        "the owning Host wrote the commit"
    );
    let source =
        lyrel_document::drawio::parse(&reopened).expect("the saved remote file still parses");
    assert_eq!(source.pages.len(), 2);
    // A second control client proves the bytes are on the remote, and the
    // local decoy is untouched.
    let verifier = ssh.connect_control_as(
        "fixture:native-ssh-298-verify",
        "native-ssh-drawio-298",
        "isolated-fixture",
    );
    let bytes = verifier
        .read_file(&remote, committed.len() as u64 + 1)
        .expect("independent remote read");
    assert_eq!(bytes, committed.to_vec());
    assert_eq!(std::fs::read(&decoy).unwrap(), b"decoy");
}
