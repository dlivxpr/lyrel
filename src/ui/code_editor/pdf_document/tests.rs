//! #284 的生产工作台接缝：PDF 作为真实文档打开、翻页、缩放、密码、既有外观。
//!
//! 这些测试只走工作台入口（`open_file_in_editor`、`editor_open_on_host`、
//! 键位、按钮与真实 gpui 测试平台窗口），夹具由 `lyrel-document` 的自有生成器在
//! 测试进程内构造，不依赖协调目录或任何下载资源。它们不是原生 OS 窗口；
//! 真实桌面、SSH、系统 IME 与性能仍由单独验收覆盖。

use super::super::test_host::FileHost;
use crate::ui::app::LyrelApp;
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use lyrel_document::pdf::fixtures;
use lyrel_ui_base::host_ops::{HostId, SharedHost};
use lyrel_ui_base::i18n::{L10nKey, t};
use lyrel_workbench_editor::code_editor::BufferId;
use lyrel_workbench_editor::code_editor::pdf_document::work_gate::{Work, pause_work};
use lyrel_workbench_editor::code_editor::pdf_document::{
    PdfActualSize, PdfContent, PdfCopy, PdfFitPage, PdfNextPage, PdfView, PdfZoomIn,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
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
            "PDF document transition did not settle"
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
    active_id(app, cx)
}

fn active_id(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> BufferId {
    app.read_with(cx, |app, _| {
        app.tab_code()
            .and_then(TabCode::active_id)
            .expect("a buffer in front")
    })
}

fn pdf_view(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<PdfView> {
    app.read_with(cx, |app, cx| {
        app.active_editor_input(cx)
            .expect("a document in front")
            .pdf()
            .expect("a PDF document")
            .clone()
    })
}

fn content(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> PdfContent {
    pdf_view(app, cx).read_with(cx, |view, cx| view.document().read(cx).content().clone())
}

fn ready(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> bool {
    app.read_with(cx, |app, cx| {
        app.active_buffer(cx)
            .and_then(|file| file.input.pdf())
            .is_some_and(|view| {
                matches!(
                    view.read(cx).document().read(cx).content(),
                    PdfContent::Ready(_)
                )
            })
    })
}

fn status(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> String {
    pdf_view(app, cx).read_with(cx, |view, cx| view.status_text(cx).to_string())
}

fn dispatch<A: gpui_kit::Action>(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, action: A) {
    let handle = pdf_view(app, cx).read_with(cx, |view, _| view.handle());
    cx.update(|window, cx| handle.dispatch_action(&action, window, cx));
    settle(cx);
}

/// Ctrl on Windows/Linux, Cmd on macOS — the platform's `secondary-`.
fn secondary(chord: &str) -> String {
    match cfg!(target_os = "macos") {
        true => format!("cmd-{chord}"),
        false => format!("ctrl-{chord}"),
    }
}

#[gpui_kit::test]
fn a_pdf_opens_as_a_read_only_document_with_page_identity(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let bytes = fixtures::reading_document();
    let path = fixture(dir.path(), "reading.pdf", &bytes);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);

    app.read_with(&cx, |app, cx| {
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("installed document");
        assert!(file.input.pdf().is_some(), "a PDF document");
        assert!(!file.dirty, "a PDF is never unsaved");
        assert_eq!(file.path, path);
        assert!(app.pdf_document_active(cx));
        assert_eq!(
            app.editor_active_location(cx)
                .map(|(host, path)| (host, path.to_path_buf())),
            Some((HostId::LOCAL, path.clone()))
        );
        assert!(
            app.editor.read(cx).local_file(id).is_none(),
            "not a text file"
        );
    });
    assert!(ready(&app, &cx));
    let view = pdf_view(&app, &cx);
    view.read_with(&cx, |view, _| {
        assert_eq!(view.page_count(), 12);
        assert_eq!(view.page_number(), 1);
    });
    until(&mut cx, |cx| {
        !pdf_view(&app, cx)
            .read_with(cx, |view, _| view.loaded_pages())
            .is_empty()
    });
    assert!(
        status(&app, &cx).contains("Page 1 / 12"),
        "status: {}",
        status(&app, &cx)
    );

    cx.update(|window, _| {
        assert!(window.find("pdf-page-0").visible(), "the first page slot");
        assert!(window.find("pdf-page-identity").visible());
        assert!(window.find("pdf-fit-width").visible());
        assert!(window.find("pdf-fit-page").visible());
        assert!(window.find("pdf-zoom").visible());
        assert!(window.find("status-pdf-facts").visible());
        assert!(window.find("status-pdf-readonly").visible());
        assert!(window.find("status-pdf-zoom").visible());
        assert!(
            window.try_find("status-cursor").is_none(),
            "a PDF has no caret"
        );
        assert!(window.try_find("status-wrap").is_none());
        assert!(window.try_find("status-eol").is_none());
    });

    // 第三页旋转 90°：显示的页面宽高互换，来源字节未改。
    let document = view.read_with(&cx, |view, cx| view.document().read(cx).ready());
    let document = document.expect("parsed");
    assert_eq!(document.page_size(2), Some((792.0, 612.0)));
    assert_eq!(document.page_count(), 12);
    assert!(document.text(0).unwrap().text().contains("unique-page-01"));
    assert_eq!(document.outline().len(), 3);
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "original untouched");
}

#[gpui_kit::test]
fn page_navigation_and_zoom_reframe_the_page_window(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "reading.pdf", &fixtures::reading_document());
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| ready(&app, cx));
    settle(&mut cx);

    let view = pdf_view(&app, &cx);
    dispatch(&app, &mut cx, PdfNextPage);
    assert_eq!(view.read_with(&cx, |view, _| view.page_number()), 2);
    cx.update(|window, _| assert!(window.find("pdf-page-1").visible()));
    dispatch(&app, &mut cx, PdfNextPage);
    assert_eq!(view.read_with(&cx, |view, _| view.page_number()), 3);
    // PageUp/PageDown and Left/Right step pages inside the PDF context.
    cx.update(|window, cx| window.press("left", cx));
    settle(&mut cx);
    assert_eq!(view.read_with(&cx, |view, _| view.page_number()), 2);
    cx.update(|window, cx| window.press("pagedown", cx));
    settle(&mut cx);
    assert_eq!(view.read_with(&cx, |view, _| view.page_number()), 3);

    let before = view.read_with(&cx, |view, _| view.zoom_label().to_string());
    dispatch(&app, &mut cx, PdfZoomIn);
    let zoomed = view.read_with(&cx, |view, _| view.zoom_label().to_string());
    assert_ne!(zoomed, before, "zoom in reframes");
    assert!(
        view.read_with(&cx, |view, _| view.loaded_pages()).len()
            < view.read_with(&cx, |view, _| view.page_count()),
        "a long document never renders every page"
    );
    dispatch(&app, &mut cx, PdfActualSize);
    assert_eq!(
        view.read_with(&cx, |view, _| view.zoom_label().to_string()),
        "100%"
    );
    dispatch(&app, &mut cx, PdfFitPage);
    cx.update(|window, cx| window.press("0", cx));
    settle(&mut cx);
    assert!(
        view.read_with(&cx, |view, _| view.loaded_pages())
            .contains(&2)
    );

    // Keyboard framing chords are scoped to the PDF context.
    let fitted = view.read_with(&cx, |view, _| view.zoom_label().to_string());
    cx.update(|window, cx| window.press(&secondary("="), cx));
    settle(&mut cx);
    assert_ne!(
        view.read_with(&cx, |view, _| view.zoom_label().to_string()),
        fitted
    );
    cx.update(|window, cx| window.press("1", cx));
    settle(&mut cx);
    assert_eq!(
        view.read_with(&cx, |view, _| view.zoom_label().to_string()),
        "100%"
    );
}

#[gpui_kit::test]
fn the_page_window_evicts_retired_bitmaps(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "reading.pdf", &fixtures::reading_document());
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| {
        pdf_view(&app, cx).read_with(cx, |view, _| view.loaded_pages().contains(&0))
    });
    let view = pdf_view(&app, &cx);
    let first = view
        .read_with(&cx, |view, _| view.image(0))
        .expect("page 0 bitmap");
    let weak = Arc::downgrade(&first);
    drop(first);

    // Jump to the end of the twelve-page document; pages far outside the
    // viewport must drop their textures, not accumulate for the whole file.
    view.update(&mut cx, |view, cx| view.go_to_page(11, cx));
    until(&mut cx, |cx| {
        pdf_view(&app, cx).read_with(cx, |view, _| !view.loaded_pages().contains(&0))
    });
    assert!(
        weak.upgrade().is_none(),
        "the retired page bitmap was released"
    );
    let loaded = view.read_with(&cx, |view, _| view.loaded_pages());
    assert!(
        loaded.len() <= 6,
        "only the viewport window stays rasterized: {loaded:?}"
    );
    assert!(loaded.contains(&11), "the destination page rendered");
}

#[gpui_kit::test]
fn a_stale_page_result_cannot_replace_a_replaced_document(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "reading.pdf", &fixtures::reading_document());
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let (release, entered, finished) = pause_work(Work::FirstPage);
    open(&app, &mut cx, &path);
    until(&mut cx, |_| {
        entered.load(std::sync::atomic::Ordering::Acquire)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.editor.read(cx).buffers.watching())
    });
    let replacement = fixtures::forms_document(true);
    std::fs::write(&path, &replacement).unwrap();
    until(&mut cx, |cx| status(&app, cx) == "Page 1 / 1");
    release.send_blocking(()).unwrap();
    until(&mut cx, |_| {
        finished.load(std::sync::atomic::Ordering::Acquire)
    });
    until(&mut cx, |cx| {
        cx.update(|window, _| window.try_find("pdf-field-unicode279").is_some())
    });
    settle(&mut cx);
    assert_eq!(status(&app, &cx), "Page 1 / 1");
    cx.update(|window, _| {
        assert_eq!(
            window.find("pdf-field-unicode279").label(),
            Some("中文表单279")
        );
        assert!(window.try_find("pdf-page-1").is_none());
    });
    assert_eq!(std::fs::read(&path).unwrap(), replacement);
}

#[gpui_kit::test]
fn a_late_password_result_cannot_resurrect_reloaded_content(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(
        dir.path(),
        "replaced-locked.pdf",
        &fixtures::encrypted_reading_document(),
    );
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    let input = pdf_view(&app, &cx).read_with(&cx, |view, _| view.password_input());
    input.update_in(&mut cx, |input, window, cx| {
        input.set_value(fixtures::TEST_PASSWORD, window, cx)
    });
    let (release, entered, finished) = pause_work(Work::Unlock);
    cx.update(|window, cx| window.click("pdf-unlock", cx));
    until(&mut cx, |_| {
        entered.load(std::sync::atomic::Ordering::Acquire)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.editor.read(cx).buffers.watching())
    });
    let replacement = fixtures::forms_document(true);
    std::fs::write(&path, &replacement).unwrap();
    until(&mut cx, |cx| status(&app, cx) == "Page 1 / 1");
    release.send_blocking(()).unwrap();
    until(&mut cx, |_| {
        finished.load(std::sync::atomic::Ordering::Acquire)
    });
    until(&mut cx, |cx| {
        cx.update(|window, _| window.try_find("pdf-field-unicode279").is_some())
    });
    settle(&mut cx);
    assert_eq!(status(&app, &cx), "Page 1 / 1");
    assert_eq!(std::fs::read(&path).unwrap(), replacement);
}

#[gpui_kit::test]
fn encrypted_pdf_prompts_and_recovers_from_a_wrong_password(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let bytes = fixtures::encrypted_reading_document();
    let path = fixture(dir.path(), "locked.pdf", &bytes);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);

    assert!(matches!(
        content(&app, &cx),
        PdfContent::Locked { wrong: false }
    ));
    let view = pdf_view(&app, &cx);
    cx.update(|window, _| {
        assert!(window.find("pdf-password").visible());
        assert!(window.try_find("pdf-failed").is_none());
    });
    let input = view.read_with(&cx, |view, _| view.password_input());

    // Wrong password: visible, no spinner, and the entered value is cleared
    // instead of becoming document state.
    app.update_in(&mut cx, |_, window, cx| {
        input.update(cx, |input, cx| {
            input.set_value(fixtures::WRONG_PASSWORD, window, cx)
        });
    });
    cx.update(|window, cx| window.click("pdf-unlock", cx));
    until(&mut cx, |cx| {
        matches!(content(&app, cx), PdfContent::Locked { wrong: true })
    });
    assert!(status(&app, &cx).contains("Wrong password"));
    assert_eq!(
        input.read_with(&cx, |input, _| input.value().to_string()),
        ""
    );
    cx.update(|window, _| assert!(window.find("pdf-password").visible()));

    // Correct password: real pages, text and outline are recovered.
    app.update_in(&mut cx, |_, window, cx| {
        input.update(cx, |input, cx| {
            input.set_value(fixtures::TEST_PASSWORD, window, cx)
        });
    });
    cx.update(|window, cx| window.click("pdf-unlock", cx));
    until(&mut cx, |cx| ready(&app, cx));
    assert_eq!(
        input.read_with(&cx, |input, _| input.value().to_string()),
        ""
    );
    let document = view.read_with(&cx, |view, cx| {
        assert!(
            view.document.read(cx).bytes.is_none(),
            "only the locked document retains retry bytes"
        );
        view.document.read(cx).ready().expect("unlocked")
    });
    assert_eq!(document.page_count(), 12);
    assert!(document.text(0).unwrap().text().contains("unique-page-01"));
    assert_eq!(document.outline().len(), 3);
    until(&mut cx, |cx| status(&app, cx) == "Page 1 / 12");
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "original untouched");
}

#[gpui_kit::test]
fn existing_appearances_render_and_missing_values_are_read_only_overlays(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);

    // Existing /AP streams and /AS state dictionaries render natively.
    let existing = fixture(
        dir.path(),
        "appearances.pdf",
        &fixtures::forms_document(false),
    );
    open(&app, &mut cx, &existing);
    until(&mut cx, |cx| ready(&app, cx));
    let view = pdf_view(&app, &cx);
    view.read_with(&cx, |view, cx| {
        let document = view.document.read(cx).ready().unwrap();
        let fields = document.fields();
        assert_eq!(fields.len(), 2);
        assert!(fields.iter().all(|field| field.has_appearance));
        assert!(fields.iter().all(|field| !field.needs_text_overlay()));
        assert_eq!(fields[0].value, "FILLED AP 279");
        assert_eq!(fields[1].value, "Yes");
        assert_eq!(document.render_page(0, 1.0, &|| false).unwrap().width, 612);
    });
    cx.update(|window, _| {
        assert!(window.find("pdf-page-0").visible());
        assert!(window.try_find("pdf-field-filled279").is_none());
        assert!(window.try_find("pdf-unlock").is_none());
    });
    settle(&mut cx);
    assert_eq!(
        std::fs::read(&existing).unwrap(),
        fixtures::forms_document(false),
        "opening never normalizes or rewrites appearances"
    );

    // Missing /AP + NeedAppearances: the real UTF-16 values are drawn as
    // read-only text at their decrypted widget geometry.
    let missing = fixture(dir.path(), "missing.pdf", &fixtures::forms_document(true));
    open(&app, &mut cx, &missing);
    until(&mut cx, |cx| {
        ready(&app, cx)
            && pdf_view(&app, cx).read_with(cx, |view, _| !view.loaded_pages().is_empty())
    });
    let view = pdf_view(&app, &cx);
    view.read_with(&cx, |view, cx| {
        let fields = view.document.read(cx).ready().unwrap().fields().to_vec();
        assert_eq!(fields[0].value, "中文表单279");
        assert_eq!(fields[1].value, "第一行\n第二行");
        assert!(fields[1].multiline);
        assert_eq!(
            fields[1].alignment,
            lyrel_document::pdf::FieldAlignment::Right
        );
        assert!(fields.iter().all(|field| field.needs_text_overlay()));
    });
    cx.update(|window, _| {
        let single = window.find("pdf-field-unicode279");
        assert!(single.visible());
        assert_eq!(single.label(), Some("中文表单279"));
        let multi = window.find("pdf-field-multiline279");
        assert!(multi.visible());
        assert_eq!(multi.label(), Some("第一行\n第二行"));
        assert!(window.try_find("pdf-unlock").is_none());
    });
    assert_eq!(
        std::fs::read(&missing).unwrap(),
        fixtures::forms_document(true),
        "overlay display leaves the source untouched"
    );

    // A PDFDocEncoding value keeps its real characters in the same read-only
    // overlay instead of showing Latin-1 control bytes.
    let docenc = fixtures::pdf_doc_encoding_form_document();
    let docenc_path = fixture(dir.path(), "docenc.pdf", &docenc);
    open(&app, &mut cx, &docenc_path);
    until(&mut cx, |cx| {
        ready(&app, cx)
            && pdf_view(&app, cx).read_with(cx, |view, _| !view.loaded_pages().is_empty())
    });
    let view = pdf_view(&app, &cx);
    view.read_with(&cx, |view, cx| {
        let fields = view.document.read(cx).ready().unwrap().fields().to_vec();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].value, "Doc \u{02d8} \u{2022} \u{fb01} \u{20ac} é");
        assert!(fields[0].needs_text_overlay());
    });
    cx.update(|window, _| {
        let field = window.find("pdf-field-docenc279");
        assert!(field.visible());
        assert_eq!(
            field.label(),
            Some("Doc \u{02d8} \u{2022} \u{fb01} \u{20ac} é")
        );
    });
    assert_eq!(
        std::fs::read(&docenc_path).unwrap(),
        docenc,
        "PDFDocEncoding display leaves the source untouched"
    );
}

#[gpui_kit::test]
fn malformed_and_oversized_pdfs_fail_visibly(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);

    let broken = fixture(dir.path(), "broken.pdf", b"not a PDF at all\n");
    open(&app, &mut cx, &broken);
    assert!(matches!(content(&app, &cx), PdfContent::Failed(_)));
    assert!(status(&app, &cx).contains("Couldn’t open PDF"));
    cx.update(|window, _| {
        assert!(window.find("pdf-failed").visible());
        assert!(window.try_find("pdf-password").is_none());
    });
    assert_eq!(
        std::fs::read(&broken).unwrap(),
        b"not a PDF at all\n",
        "a failed PDF stays the original bytes"
    );

    let mut oversized = Vec::with_capacity(33 * 1024 * 1024);
    oversized.extend_from_slice(b"%PDF-1.7\n");
    oversized.resize(33 * 1024 * 1024, 0);
    let big = fixture(dir.path(), "huge.pdf", &oversized);
    open(&app, &mut cx, &big);
    match content(&app, &cx) {
        PdfContent::Failed(message) => assert!(
            message.contains(&t(L10nKey::EditorPdfTooLarge)),
            "the PDF budget is named, got: {message}"
        ),
        _ => panic!("a PDF over the viewing budget parsed"),
    }
    cx.update(|window, _| assert!(window.find("pdf-failed").visible()));
}

#[gpui_kit::test]
fn text_only_commands_leave_a_pdf_alone(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let before = fixtures::reading_document();
    let path = fixture(dir.path(), "read-only.pdf", &before);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);

    app.update_in(&mut cx, |app, window, cx| {
        app.editor_save_active(window, cx);
        app.editor_save_as_active(window, cx);
        app.editor_go_to_line(window, cx);
        app.editor_go_to_symbol(window, cx);
        app.toggle_line_ending(cx);
        assert!(
            !app.editor.read(cx).has_bar(),
            "no prompt opened over a PDF"
        );
        let file = app
            .editor
            .read(cx)
            .buffers
            .get(id)
            .expect("still installed");
        assert!(!file.dirty);
        assert_eq!(file.format(cx), None, "a PDF has no text format");
    });
    settle(&mut cx);
    assert_eq!(std::fs::read(&path).unwrap(), before, "nothing was written");
    assert!(ready(&app, &cx));
}

#[gpui_kit::test]
fn the_owning_host_decides_which_pdf_bytes_are_shown(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let local_bytes = fixtures::reading_document();
    let path = fixture(dir.path(), "doc.pdf", &local_bytes);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let local = open(&app, &mut cx, &path);
    assert_eq!(
        pdf_view(&app, &cx).read_with(&cx, |view, _| view.page_count()),
        12
    );

    // Same path, another Host: a distinct buffer acquired through that Host's
    // own versioned read, not the local one.
    let host = FileHost::new(HostId(84));
    let shared: SharedHost = host.clone();
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_open_on_host(shared, &path, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.tab_code()
                .and_then(TabCode::active_id)
                .is_some_and(|id| id != local)
        })
    });
    app.read_with(&cx, |app, cx| {
        assert_eq!(app.editor_active_location(cx).unwrap().0, HostId(84));
    });
    assert_eq!(
        pdf_view(&app, &cx).read_with(&cx, |view, _| view.page_count()),
        12
    );
    assert_eq!(
        host.open_reads.load(Ordering::SeqCst),
        1,
        "the PDF came from one bounded versioned session"
    );

    // A mapped Host has its own root: the decoy PDF, not the local file.
    let decoy = super::super::test_host::canonical_tempdir();
    std::fs::write(
        decoy.path().join("doc.pdf"),
        fixtures::forms_document(false),
    )
    .unwrap();
    let mapped: SharedHost = FileHost::mapped(HostId(85), decoy.path());
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_open_on_host(mapped, Path::new("/doc.pdf"), window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.tab_code()
                .and_then(TabCode::active_id)
                .is_some_and(|id| {
                    id != local
                        && app
                            .editor_active_location(cx)
                            .is_some_and(|(host, _)| host == HostId(85))
                })
        })
    });
    assert_eq!(
        pdf_view(&app, &cx).read_with(&cx, |view, _| view.page_count()),
        1,
        "the mapped Host's own PDF"
    );
    assert_eq!(std::fs::read(&path).unwrap(), local_bytes);

    // Switching back shows the original local copy unchanged.
    app.update_in(&mut cx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.tab_code()
                .and_then(TabCode::active_id)
                .is_some_and(|id| id == local)
        })
    });
    assert_eq!(
        pdf_view(&app, &cx).read_with(&cx, |view, _| view.page_count()),
        12
    );
    assert_eq!(std::fs::read(&path).unwrap(), local_bytes);
}

#[gpui_kit::test]
fn split_groups_keep_their_own_page_and_zoom(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "split.pdf", &fixtures::reading_document());
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx), "the PDF splits");
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
        let file = app.active_buffer(cx).expect("PDF");
        (
            file.presentations
                .pdf_existing(tab, right)
                .expect("front view"),
            file.presentations
                .pdf_existing(tab, left)
                .expect("other view"),
        )
    });
    let before = other.read_with(&cx, |view, _| view.zoom_label().to_string());
    dispatch(&app, &mut cx, PdfZoomIn);
    assert!(
        front.read_with(&cx, |view, _| view.zoom_label().to_string()) != before,
        "the focused group zooms"
    );
    assert_eq!(
        other.read_with(&cx, |view, _| view.zoom_label().to_string()),
        before,
        "the other group keeps its framing"
    );
}

#[gpui_kit::test]
fn an_external_rewrite_replaces_the_pdf(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "watched.pdf", &fixtures::reading_document());
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    assert_eq!(
        pdf_view(&app, &cx).read_with(&cx, |view, _| view.page_count()),
        12
    );
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.editor.read(cx).buffers.watching())
    });
    let replacement = fixtures::forms_document(false);
    std::fs::write(&path, &replacement).unwrap();
    until(&mut cx, |cx| {
        ready(&app, cx) && pdf_view(&app, cx).read_with(cx, |view, _| view.page_count() == 1)
    });
    app.read_with(&cx, |app, cx| {
        let file = app.active_buffer(cx).expect("still open");
        assert!(file.conflict.is_none(), "a clean PDF reloads itself");
        assert!(!file.dirty);
    });
    assert_eq!(std::fs::read(&path).unwrap(), replacement);
}

#[gpui_kit::test]
fn a_pdf_whose_tab_closed_before_its_read_lands_leaves_nothing_behind(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "late.pdf", &fixtures::reading_document());
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 2);
    let host = FileHost::new(HostId(86));
    let gate = host.pause_next_read();
    let shared: SharedHost = host.clone();
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_open_on_host(shared, &path, window, cx)
    });
    settle(&mut cx);
    app.read_with(&cx, |app, cx| {
        assert_eq!(
            app.editor.read(cx).buffers.iter().count(),
            0,
            "still reading"
        );
    });
    app.update_in(&mut cx, |app, window, cx| app.close_tab(0, window, cx));
    settle(&mut cx);
    let _ = gate.send(());
    settle(&mut cx);
    settle(&mut cx);
    app.read_with(&cx, |app, cx| {
        assert_eq!(
            app.editor.read(cx).buffers.iter().count(),
            0,
            "a late read installs no document into a closed tab"
        );
    });

    let id = open(&app, &mut cx, &path);
    app.read_with(&cx, |app, cx| {
        assert!(
            app.editor
                .read(cx)
                .buffers
                .get(id)
                .is_some_and(|file| file.input.pdf().is_some())
        )
    });
}

#[gpui_kit::test]
fn a_pdf_opens_through_the_owning_hosts_versioned_read(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let bytes = fixtures::reading_document();
    let path = fixture(dir.path(), "versioned.pdf", &bytes);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let host = FileHost::new(HostId(287));
    let shared: SharedHost = host.clone();
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_open_on_host(shared, &path, window, cx)
    });
    until(&mut cx, |cx| ready(&app, cx));
    assert_eq!(
        host.open_reads.load(Ordering::SeqCst),
        1,
        "the PDF came from one bounded versioned session"
    );
    assert_eq!(
        pdf_view(&app, &cx).read_with(&cx, |view, _| view.page_count()),
        12
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

// #285：大纲/链接跳转、选区复制与文档内查找的工作台行为。它们仍然只走真实
// 键位、按钮与拖拽——不触碰内部状态去伪造一次成功。

fn find_count(cx: &mut VisualTestContext) -> String {
    cx.update(|window, _| {
        window
            .find("pdf-find-count")
            .label()
            .unwrap_or_default()
            .to_string()
    })
}

#[gpui_kit::test]
fn the_outline_navigates_named_destinations_by_click_and_keyboard(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let bytes = fixtures::reading_document();
    let path = fixture(dir.path(), "outline.pdf", &bytes);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| ready(&app, cx));
    settle(&mut cx);

    let view = pdf_view(&app, &cx);
    cx.update(|window, _| assert!(window.find("pdf-toc-toggle").visible()));
    cx.update(|window, cx| window.click("pdf-toc-toggle", cx));
    settle(&mut cx);
    cx.update(|window, _| {
        assert!(window.find("pdf-toc").visible(), "the outline panel");
        assert_eq!(
            window.find("pdf-toc-0").label(),
            Some("First explicit page")
        );
        assert_eq!(
            window.find("pdf-toc-1").label(),
            Some("Middle named destination")
        );
        assert_eq!(window.find("pdf-toc-2").label(), Some("Last named action"));
    });

    // 指针点击中间条目：命名目的地指向第 6 页的坐标锚点。
    cx.update(|window, cx| window.click("pdf-toc-1", cx));
    settle(&mut cx);
    view.read_with(&cx, |view, _| {
        assert_eq!(view.page_number(), 6, "the named destination page");
        let offset = view.scroll_offset();
        assert!(
            f32::from(offset.y) < 0.,
            "the destination anchor scrolls off page top: {:?}",
            offset
        );
    });

    // 键盘：下/上移动游标、Enter 跳到最后一页（无坐标的 /Fit 目的地）。
    cx.update(|window, cx| window.press("down", cx));
    settle(&mut cx);
    cx.update(|window, cx| window.press("enter", cx));
    settle(&mut cx);
    view.read_with(&cx, |view, _| {
        assert_eq!(view.page_number(), 12, "the named action jumps to its page")
    });
    cx.update(|window, _| assert!(window.find("pdf-page-11").visible()));

    cx.update(|window, cx| window.press("up", cx));
    settle(&mut cx);
    cx.update(|window, cx| window.press("enter", cx));
    settle(&mut cx);
    assert_eq!(
        view.read_with(&cx, |view, _| view.page_number()),
        6,
        "the previous outline entry"
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "original untouched");
}

#[gpui_kit::test]
fn page_link_annotations_jump_to_their_destination(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "links.pdf", &fixtures::links_document());
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| ready(&app, cx));
    settle(&mut cx);

    let view = pdf_view(&app, &cx);
    cx.update(|window, _| {
        assert!(window.find("pdf-link-0-0").visible(), "internal link");
        assert!(window.find("pdf-link-0-1").visible(), "external link");
    });
    // 旋转 90° 页上的内部链接：归一化矩形换算后仍指向第二页。
    cx.update(|window, cx| window.click("pdf-link-0-0", cx));
    settle(&mut cx);
    assert_eq!(view.read_with(&cx, |view, _| view.page_number()), 2);
    cx.update(|window, _| assert!(window.find("pdf-link-1-0").visible()));

    // 目标页上的命名链接指回其自身的 /Fit 目的地，不发生导航漂移。
    cx.update(|window, cx| window.click("pdf-link-1-0", cx));
    settle(&mut cx);
    assert_eq!(view.read_with(&cx, |view, _| view.page_number()), 2);
}

#[gpui_kit::test]
fn a_rotated_page_selects_and_copies_its_decoded_text(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let bytes = fixtures::links_document();
    let path = fixture(dir.path(), "rotated.pdf", &bytes);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| ready(&app, cx));
    settle(&mut cx);

    // 首页带 /Rotate 90：文本层的归一化矩形按显示后的页面给出，拖拽仍应
    // 选中同一段文字，复制出解码后的字符串而不是按未旋转坐标错位。
    let page_bounds = cx.update(|window, _| window.find("pdf-page-0").bounds());
    assert!(
        page_bounds.size.width > page_bounds.size.height,
        "the rotated page is displayed landscape: {:?}",
        page_bounds.size
    );
    let from = page_bounds.origin
        + gpui_kit::point(
            page_bounds.size.width * 0.02,
            page_bounds.size.height * 0.02,
        );
    let to = page_bounds.origin
        + gpui_kit::point(
            page_bounds.size.width * 0.98,
            page_bounds.size.height * 0.98,
        );
    cx.update(|window, cx| window.drag(from, to, cx));
    until(&mut cx, |cx| {
        pdf_view(&app, cx).read_with(cx, |view, _| {
            view.has_selection() && view.text_page(0).is_some()
        })
    });
    cx.update(|window, cx| window.press(&secondary("c"), cx));
    settle(&mut cx);
    let copied = cx
        .update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
        .unwrap_or_default();
    assert!(
        copied.contains("Rotated link page"),
        "the rotated page's text is selectable: {copied:?}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "original untouched");
}

#[gpui_kit::test]
fn the_find_bar_counts_matches_and_steps_with_keys(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "find.pdf", &fixtures::reading_document());
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| ready(&app, cx));
    settle(&mut cx);

    let view = pdf_view(&app, &cx);
    cx.update(|window, cx| window.click("pdf-find", cx));
    settle(&mut cx);
    cx.update(|window, _| assert!(window.find("pdf-find-bar").visible()));

    cx.update(|window, cx| window.input("unique-page-1", cx));
    // 12 页里匹配 "unique-page-10"、"unique-page-11"、"unique-page-12" 三个。
    until(&mut cx, |cx| find_count(cx).starts_with("1 / 3"));
    view.read_with(&cx, |view, _| {
        assert_eq!(view.match_count(), 3);
        assert_eq!(view.current_match_index(), Some(0));
        assert_eq!(view.page_number(), 10, "the first match jumps to its page");
    });
    cx.update(|window, _| assert!(window.find("pdf-match-9").visible()));

    // Enter 前进并翻页；Shift+Enter 后退；末位回绕到第一命中。
    cx.update(|window, cx| window.press("enter", cx));
    settle(&mut cx);
    view.read_with(&cx, |view, _| {
        assert_eq!(view.current_match_index(), Some(1));
        assert_eq!(view.page_number(), 11);
    });
    cx.update(|window, _| assert!(window.find("pdf-match-10").visible()));
    cx.update(|window, cx| window.press("enter", cx));
    settle(&mut cx);
    cx.update(|window, cx| window.press("shift-enter", cx));
    settle(&mut cx);
    view.read_with(&cx, |view, _| {
        assert_eq!(view.current_match_index(), Some(1));
        assert_eq!(view.page_number(), 11);
    });
    cx.update(|window, cx| window.press("shift-enter", cx));
    settle(&mut cx);
    view.read_with(&cx, |view, _| {
        assert_eq!(view.current_match_index(), Some(0));
        assert_eq!(view.page_number(), 10);
    });

    // Escape 关掉查找条并清掉高亮。
    cx.update(|window, cx| window.press("escape", cx));
    settle(&mut cx);
    cx.update(|window, _| {
        assert!(window.try_find("pdf-find-bar").is_none(), "find bar closed");
        assert!(
            window.try_find("pdf-match-9").is_none(),
            "highlights cleared"
        );
    });
    assert_eq!(
        view.read_with(&cx, |view, _| view.match_count()),
        0,
        "closing clears the results"
    );
}

#[gpui_kit::test]
fn unicode_search_finds_text_on_the_last_page_and_reanchors_after_zoom(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let bytes = fixtures::reading_document();
    let path = fixture(dir.path(), "unicode-find.pdf", &bytes);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| ready(&app, cx));
    settle(&mut cx);

    let view = pdf_view(&app, &cx);
    cx.update(|window, cx| window.click("pdf-find", cx));
    settle(&mut cx);
    cx.update(|window, cx| window.input("unique-page-12", cx));
    until(&mut cx, |cx| find_count(cx) == "1 / 1");
    view.read_with(&cx, |view, _| {
        assert_eq!(
            view.page_number(),
            12,
            "the only match lands on the last page"
        );
    });
    cx.update(|window, _| assert!(window.find("pdf-match-11").visible()));

    // 缩放改变页面像素，命中的归一化矩形仍锚定在同一段文字上。
    dispatch(&app, &mut cx, PdfZoomIn);
    settle(&mut cx);
    cx.update(|window, _| {
        assert!(
            window.find("pdf-match-11").visible(),
            "highlight after zoom"
        );
        assert!(window.find("pdf-page-11").visible());
    });
    assert_eq!(
        view.read_with(&cx, |view, _| view.match_count()),
        1,
        "zooming keeps the same matches"
    );

    // 关闭查找后仍在末页阅读／选字；缩放不能退回旧像素偏移对应的第 10 页，
    // 更不能只保留“第 12 页”字段而让真实末页与选区移出视口。
    cx.update(|window, cx| window.press("escape", cx));
    settle(&mut cx);
    // 停靠视口可能只容纳命中附近的一段；先适应整页，避免从视口外的页顶拖拽。
    dispatch(&app, &mut cx, PdfFitPage);
    until(&mut cx, |cx| {
        view.read_with(cx, |view, _| view.text_page(11).is_some())
    });
    let bounds = cx.update(|window, _| window.find("pdf-page-11").bounds());
    let from = bounds.origin + gpui_kit::point(bounds.size.width * 0.02, bounds.size.height * 0.02);
    let to = bounds.origin + gpui_kit::point(bounds.size.width * 0.95, bounds.size.height * 0.22);
    let viewport = view.read_with(&cx, |view, _| view.scroll().bounds());
    assert!(from.y >= viewport.origin.y && to.y < viewport.origin.y + viewport.size.height);
    cx.update(|window, cx| window.drag(from, to, cx));
    until(&mut cx, |cx| {
        view.read_with(cx, |view, _| view.has_selection())
    });
    cx.update(|window, cx| window.press(&secondary("c"), cx));
    let expected = "lyrel #279 PDF page 12\nfidelity marker; common search target\nunique-page-12";
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some(expected.to_owned())
    );
    for id in [
        "pdf-zoom-in",
        "pdf-zoom-out",
        "pdf-fit-page",
        "pdf-fit-width",
    ] {
        cx.update(|window, cx| window.click(id, cx));
        settle(&mut cx);
        cx.update(|window, _| {
            assert!(window.find("pdf-page-11").visible(), "last page after {id}");
            assert!(
                window.find("pdf-selection-11").visible(),
                "the selected text remains in the real viewport after {id}"
            );
        });
        assert_eq!(view.read_with(&cx, |view, _| view.page_number()), 12);
    }
    dispatch(&app, &mut cx, PdfActualSize);
    cx.update(|window, _| assert!(window.find("pdf-selection-11").visible()));
    dispatch(&app, &mut cx, PdfCopy);
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some(expected.to_owned())
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "original untouched");
}

#[gpui_kit::test]
fn cjk_search_and_selection_copy_join_glyph_runs(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "cjk.pdf", &fixtures::unicode_text_document());
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| ready(&app, cx));
    settle(&mut cx);

    let view = pdf_view(&app, &cx);
    // 搜索两个 CJK 码点：Type3 字形经 /ToUnicode 映射为"中文"，两行各一次。
    cx.update(|window, cx| window.click("pdf-find", cx));
    settle(&mut cx);
    cx.update(|window, cx| window.input("中文", cx));
    until(&mut cx, |cx| find_count(cx) == "1 / 3");
    view.read_with(&cx, |view, _| {
        assert_eq!(view.match_count(), 3, "CJK occurrences across both lines");
    });
    cx.update(|window, _| assert!(window.find("pdf-match-0").visible()));
    cx.update(|window, cx| window.press("escape", cx));
    settle(&mut cx);

    // 拖拽框选两行中文文本并复制：相邻 CJK run 不插入臆造空格，
    // 行间保留换行。
    let page_bounds = cx.update(|window, _| window.find("pdf-page-0").bounds());
    let from = page_bounds.origin
        + gpui_kit::point(
            page_bounds.size.width * 0.05,
            page_bounds.size.height * 0.06,
        );
    let to = page_bounds.origin
        + gpui_kit::point(
            page_bounds.size.width * 0.55,
            // 拖过第二行基线；页面元素原点相对滚动列还有 PAGE_PAD 的上边距，
            // 20% 会落在两行之间而只选到第一行。
            page_bounds.size.height * 0.30,
        );
    cx.update(|window, cx| window.drag(from, to, cx));
    until(&mut cx, |cx| {
        pdf_view(&app, cx).read_with(cx, |view, _| {
            view.has_selection() && view.text_page(0).is_some()
        })
    });
    cx.update(|window, _| assert!(window.find("pdf-selection-0").visible()));

    // 复制走真实键盘 chord；剪贴板拿到的是解码后的完整 Unicode 文本。
    cx.update(|window, cx| window.press(&secondary("c"), cx));
    settle(&mut cx);
    let copied = cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(
        copied.as_deref(),
        Some("中文 中文\n中文"),
        "adjacent CJK runs join without invented spaces"
    );
}

#[gpui_kit::test]
fn a_ligature_glyph_copies_as_its_full_unicode_text(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let bytes = fixtures::ligature_text_document();
    let path = fixture(dir.path(), "ligature.pdf", &bytes);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| ready(&app, cx));
    settle(&mut cx);

    // 一个 Type3 glyph 经 /ToUnicode 映射到 "ffi" 两个码点：选区与剪贴板
    // 都必须是展开后的完整文本，而不是字形名或单字节。
    let page_bounds = cx.update(|window, _| window.find("pdf-page-0").bounds());
    let from = page_bounds.origin
        + gpui_kit::point(
            page_bounds.size.width * 0.05,
            page_bounds.size.height * 0.06,
        );
    let to = page_bounds.origin
        + gpui_kit::point(
            page_bounds.size.width * 0.60,
            page_bounds.size.height * 0.13,
        );
    cx.update(|window, cx| window.drag(from, to, cx));
    until(&mut cx, |cx| {
        pdf_view(&app, cx).read_with(cx, |view, _| {
            view.has_selection() && view.text_page(0).is_some()
        })
    });
    cx.update(|window, _| assert!(window.find("pdf-selection-0").visible()));

    cx.update(|window, cx| window.press(&secondary("c"), cx));
    settle(&mut cx);
    let copied = cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(
        copied.as_deref(),
        Some("office"),
        "the ligature glyph expands to its mapped letters"
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "original untouched");
}

#[gpui_kit::test]
fn a_scan_only_pdf_says_so_instead_of_pretending_matches(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let bytes = fixtures::scan_only_document();
    let path = fixture(dir.path(), "scan.pdf", &bytes);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| ready(&app, cx));
    settle(&mut cx);

    cx.update(|window, cx| window.click("pdf-find", cx));
    settle(&mut cx);
    let view = pdf_view(&app, &cx);
    cx.update(|window, cx| window.input("anything", cx));
    until(&mut cx, |cx| find_count(cx) == "No text layer");
    assert!(
        status(&app, &cx).contains("No text layer"),
        "status admits no text layer: {}",
        status(&app, &cx)
    );
    view.read_with(&cx, |view, _| {
        assert_eq!(view.match_count(), 0);
    });
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "original untouched");
}

#[gpui_kit::test]
fn a_stale_sweep_cannot_report_matches_for_a_replaced_document(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "stale.pdf", &fixtures::reading_document());
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| ready(&app, cx));
    settle(&mut cx);

    let view = pdf_view(&app, &cx);
    cx.update(|window, cx| window.click("pdf-find", cx));
    settle(&mut cx);
    let (release, entered, finished) = pause_work(Work::Search);
    cx.update(|window, cx| window.input("unique", cx));
    // 扫掠进入后台任务后先停住；此时文件被外部改写。
    until(&mut cx, |_| {
        entered.load(std::sync::atomic::Ordering::Acquire)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.editor.read(cx).buffers.watching())
    });
    std::fs::write(&path, &fixtures::forms_document(true)).unwrap();
    until(&mut cx, |cx| status(&app, cx).starts_with("Page 1 / 1"));
    release.send_blocking(()).unwrap();
    until(&mut cx, |_| {
        finished.load(std::sync::atomic::Ordering::Acquire)
    });
    // 旧扫掠结果按 seq/revision 丢弃；新文档的文本层不包含查询词，计数
    // 如实显示无匹配，旧字节的命中一条都不落地。
    until(&mut cx, |cx| find_count(cx) == "No matches");
    view.read_with(&cx, |view, _| {
        assert_eq!(
            view.match_count(),
            0,
            "no matches leaked from the old bytes"
        );
    });
}

#[gpui_kit::test]
fn split_groups_keep_their_own_selection(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(dir.path(), "split-sel.pdf", &fixtures::reading_document());
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| ready(&app, cx));
    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx), "the PDF splits");
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
        let file = app.active_buffer(cx).expect("PDF");
        (
            file.presentations
                .pdf_existing(tab, right)
                .expect("front view"),
            file.presentations
                .pdf_existing(tab, left)
                .expect("other view"),
        )
    });

    // 在前面那个分屏里拖拽选中文本；另一边保持没有选区。
    assert_eq!(
        pdf_view(&app, &cx),
        front,
        "the focused pane is the one we act on"
    );
    let page_bounds = cx.update(|window, _| {
        window
            .within(("pdf-document", front.entity_id()))
            .find("pdf-page-0")
            .bounds()
    });
    let from = page_bounds.origin
        + gpui_kit::point(
            page_bounds.size.width * 0.05,
            page_bounds.size.height * 0.06,
        );
    let to = page_bounds.origin
        + gpui_kit::point(
            page_bounds.size.width * 0.55,
            page_bounds.size.height * 0.08,
        );
    cx.update(|window, cx| window.drag(from, to, cx));
    until(&mut cx, |cx| {
        pdf_view(&app, cx).read_with(cx, |view, _| view.has_selection())
    });
    front.read_with(&cx, |view, _| assert!(view.has_selection()));
    other.read_with(&cx, |view, _| {
        assert!(!view.has_selection(), "the other pane keeps no selection")
    });
}
