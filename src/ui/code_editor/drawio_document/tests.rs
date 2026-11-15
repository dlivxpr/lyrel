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
use std::path::{Path, PathBuf};
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

fn uncompressed(xml_pages: usize) -> Vec<u8> {
    let mut out = String::from("<mxfile host=\"lyrel.test\" type=\"device\">");
    for page in 0..xml_pages {
        out.push_str(&format!(
            "<diagram id=\"u{page}\" name=\"Page {}\">",
            page + 1
        ));
        out.push_str("&lt;mxGraphModel dx=\"800\" dy=\"600\"&gt;&lt;root&gt;&lt;mxCell id=\"0\"/&gt;&lt;mxCell id=\"1\" parent=\"0\"/&gt;&lt;mxCell id=\"2\" value=\"Uncompressed page {page}\" style=\"rounded=1;whiteSpace=wrap;html=1;\" vertex=\"1\" parent=\"1\"&gt;&lt;mxGeometry x=\"80\" y=\"80\" width=\"200\" height=\"70\" as=\"geometry\"/&gt;&lt;/mxCell&gt;&lt;/root&gt;&lt;/mxGraphModel&gt;");
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
        app.read_with(cx, |app, _| {
            app.active_buffer()
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
    app.read_with(cx, |app, _| {
        app.active_editor_input()
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
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let plain = fixture(dir.path(), "plain.drawio", &uncompressed(2));
    let packed = fixture(dir.path(), "packed.drawio", &compressed(3));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);

    let id = open(&app, &mut cx, &plain);
    app.read_with(&cx, |app, cx| {
        let file = app.editor.buffers.get(id).expect("installed document");
        assert!(file.input.drawio().is_some(), "a draw.io document");
        assert!(!file.dirty, "a preview is never unsaved");
        assert!(app.drawio_document_active());
        assert_eq!(
            app.editor_active_location()
                .map(|(host, path)| (host, path.to_path_buf())),
            Some((HostId::LOCAL, plain.clone()))
        );
        assert!(app.editor.local_file(id).is_none(), "not a text buffer");
        assert!(file.input.text().is_none());
        assert!(!file.format(cx).is_some(), "no text save format");
    });
    assert_eq!(pages(&app, &cx), 2);
    assert_eq!(page(&app, &cx), 0);
    cx.update(|window, _| {
        assert!(window.find("drawio-format").visible());
        assert!(window.find("drawio-page-facts").visible());
        assert!(window.find("drawio-page-next").visible());
        assert!(window.find("status-drawio-readonly").visible());
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
    app.read_with(&cx, |app, _| {
        assert!(app.editor.buffers.get(id).is_none());
    });

    let packed_id = open(&app, &mut cx, &packed);
    assert_eq!(pages(&app, &cx), 3, "compressed pages are all declared");
    app.read_with(&cx, |app, _| {
        assert!(app.editor.buffers.get(packed_id).is_some());
        assert!(!app.active_buffer().unwrap().dirty);
    });
}

#[gpui_kit::test]
fn page_controls_and_split_groups_keep_their_own_page(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
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
    let (front, other) = app.read_with(&cx, |app, _| {
        let file = app.active_buffer().expect("diagram");
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
        let file = app.active_buffer().expect("diagram");
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
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let before = compressed(2);
    let path = fixture(dir.path(), "read-only.drawio", &before);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);

    app.update_in(&mut cx, |app, window, cx| {
        app.editor_save_active(window, cx);
        app.editor_save_as_active(window, cx);
        app.editor_go_to_line(window, cx);
        app.editor_go_to_symbol(window, cx);
        app.toggle_line_ending(cx);
        let wrap_before = cx.global::<crate::core::config::Config>().editor_soft_wrap;
        app.toggle_document_wrap(window, cx);
        assert_eq!(
            cx.global::<crate::core::config::Config>().editor_soft_wrap,
            wrap_before
        );
        assert!(!app.editor.has_bar());
        let file = app.editor.buffers.get(id).expect("still installed");
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
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "watched.drawio", &compressed(2));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.watching())
    });
    dispatch(&app, &mut cx, DrawioNextPage);
    assert_eq!(page(&app, &cx), 1);

    std::fs::write(&path, uncompressed(3)).unwrap();
    until(&mut cx, |cx| pages(&app, cx) == 3);
    app.read_with(&cx, |app, cx| {
        let file = app.active_buffer().expect("still open");
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
    crate::ui::i18n::set_locale("en");
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
        app.read_with(cx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.host.id() == HostId(297))
        })
    });
    assert_eq!(pages(&app, &cx), 4, "the remote bytes won");
    app.read_with(&cx, |app, _| {
        let file = app.active_buffer().expect("remote diagram");
        assert_eq!(file.path, Path::new("/host.drawio"));
        assert_ne!(file.path, local);
    });
}

#[gpui_kit::test]
fn malformed_and_non_diagram_files_report_their_own_failure(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
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
        let file = app.editor.buffers.get(id).expect("failure document");
        assert!(file.input.drawio().is_some(), "still a diagram place");
        let view = file.input.drawio().unwrap();
        assert_eq!(view.read(cx).page_count(cx), 0);
        assert!(view.read(cx).document().read(cx).failure().is_some());
    });
    cx.update(|window, _| {
        assert!(window.find("drawio-failed").visible());
        assert!(window.find("status-drawio-readonly").visible());
    });
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    settle(&mut cx);

    // A generic .xml that is not a draw.io root stays source, not a failure
    // diagram and not an external application.
    let xml_id = open(&app, &mut cx, &not_diagram);
    app.read_with(&cx, |app, _| {
        let file = app.editor.buffers.get(xml_id).expect("text document");
        assert!(file.input.text().is_some(), "ordinary XML stays text");
        assert!(file.input.drawio().is_none());
    });
}

/// The native surface is not started by the automated harness, but its event
/// channel is the only owner of engine/page/zoom state. Drive that contract
/// directly so a bridge event cannot silently become a second authority.
#[gpui_kit::test]
fn surface_events_drive_engine_page_and_zoom_state(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "events.drawio", &compressed(3));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);

    app.update_in(&mut cx, |app, _window, cx| {
        let view = app
            .active_editor_input()
            .and_then(|input| input.drawio().cloned())
            .expect("a diagram in front");
        view.update(cx, |view, cx| {
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
    });
    assert_eq!(
        page(&app, &cx),
        1,
        "a reported state moves the visible page"
    );
    view(&app, &cx).read_with(&cx, |view, cx| {
        assert!(view.blocked.is_some(), "a blocked request stays visible");
        let status = view.status_text(cx);
        assert!(status.contains("150%"), "the engine zoom is shown: {status}");
    });

    app.update_in(&mut cx, |app, _window, cx| {
        let view = app
            .active_editor_input()
            .and_then(|input| input.drawio().cloned())
            .expect("a diagram in front");
        view.update(cx, |view, cx| {
            view.on_surface_event(
                SurfaceEvent::Failed {
                    message: "engine stopped".into(),
                },
                cx,
            );
        });
    });
    view(&app, &cx).read_with(&cx, |view, cx| {
        assert!(view.status_text(cx).contains("engine stopped"));
        assert!(view.surface.is_none(), "a failed surface is dropped");
    });

    // A close can race the asynchronous start: the recorded state must fall
    // back to idle instead of keeping a pending engine.
    app.update_in(&mut cx, |app, _window, cx| {
        let view = app
            .active_editor_input()
            .and_then(|input| input.drawio().cloned())
            .expect("a diagram in front");
        view.update(cx, |view, cx| {
            view.engine = super::EngineStatus::Starting;
            view.on_surface_event(SurfaceEvent::Closed, cx);
        });
    });
    view(&app, &cx).read_with(&cx, |view, _| {
        assert!(
            matches!(view.engine, super::EngineStatus::Idle),
            "closing while starting falls back to idle"
        );
    });
}
