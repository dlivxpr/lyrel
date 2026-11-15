//! #286 的生产工作台回归：spine、全书搜索和独立阅读状态。
//! 使用自建便携 EPUB、真实 Host 与真实工作台事件；不依赖协调目录或 OS 窗口。

use super::super::test_host::{FileHost, canonical_tempdir};
use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
use lyrel_core::host::Host as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

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
            "EPUB workbench transition did not settle"
        );
        settle(cx);
        cx.executor().advance_clock(Duration::from_millis(50));
        std::thread::sleep(Duration::from_millis(5));
    }
    settle(cx);
}

fn active(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<BookView> {
    app.read_with(cx, |app, _| {
        app.active_editor_input()
            .expect("document")
            .book()
            .expect("EPUB reader")
            .clone()
    })
}

fn open(
    app: &Entity<LyrelApp>,
    cx: &mut VisualTestContext,
    host: SharedHost,
    path: &Path,
) -> BufferId {
    app.update_in(cx, |app, window, cx| {
        app.editor_open_on_host(host, path, window, cx)
    });
    until(cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.active_buffer().is_some_and(|file| {
                file.path == path
                    && file
                        .input
                        .book()
                        .is_some_and(|view| view.read(cx).block_count() > 0)
            })
        })
    });
    app.read_with(cx, |app, _| app.active_buffer().unwrap().id())
}

fn fixture(dir: &Path, name: &str, fixed: bool) -> PathBuf {
    let path = dir.join(name);
    let mut archive = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let layout = if fixed {
        "<meta property=\"rendition:layout\">pre-paginated</meta>"
    } else {
        ""
    };
    let opf = format!(
        r#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Owned reader book</dc:title><dc:creator>Lyrel fixture</dc:creator>{layout}</metadata><manifest><item id="last" href="last.xhtml" media-type="application/xhtml+xml"/><item id="first" href="first.xhtml" media-type="application/xhtml+xml"/><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="pixel" href="pixel.png" media-type="image/png"/></manifest><spine><itemref idref="first"/><itemref idref="last"/></spine></package>"#
    );
    let mut first = String::from("<html><body><h1 id=\"start\">Spine first chapter</h1><p>");
    first.push_str(
        &"Readable paragraph with enough words to wrap at the reader column width. ".repeat(12),
    );
    first.push_str("</p>");
    for index in 0..3000 {
        first.push_str(&format!("<p>Complete long chapter paragraph {index}.</p>"));
    }
    first.push_str("<a href=\"last.xhtml#target\">Final chapter</a></body></html>");
    let last = r#"<html><body><h1 id="target">Final chapter</h1><p>needle-only-final-chapter-279 中文</p><img src="pixel.png" alt="Owned illustration"/></body></html>"#;
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(16, 8, image::Rgba([24, 72, 168, 255]))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let entries = [
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", br#"<container><rootfiles><rootfile full-path="OPS/book.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#.to_vec()),
        ("OPS/book.opf", opf.into_bytes()),
        ("OPS/nav.xhtml", br#"<html xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><ol><li><a href="first.xhtml#start">Spine first chapter</a></li><li><a href="last.xhtml#target">Final chapter</a></li></ol></nav></body></html>"#.to_vec()),
        ("OPS/first.xhtml", first.into_bytes()),
        ("OPS/last.xhtml", last.as_bytes().to_vec()),
        ("OPS/pixel.png", png.into_inner()),
    ];
    for (index, (name, bytes)) in entries.into_iter().enumerate() {
        let method = if index == 0 {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        };
        archive
            .start_file(
                name,
                zip::write::SimpleFileOptions::default().compression_method(method),
            )
            .unwrap();
        archive.write_all(&bytes).unwrap();
    }
    archive.finish().unwrap();
    path
}

#[gpui_kit::test]
fn spine_navigation_and_full_book_search_reach_the_last_chapter(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let path = fixture(dir.path(), "reference.epub", false);
    let original = std::fs::read(&path).unwrap();
    let host = FileHost::new(HostId(286101));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, host.clone(), &path);
    active(&app, &cx).read_with(&cx, |view, _| {
        assert_eq!(view.chapter(), 0);
        assert_eq!(view.block_text(0), Some("Spine first chapter"));
        assert!(view.block_count() >= 3002, "the long chapter is complete");
    });
    cx.update(|window, cx| window.click("book-search", cx));
    settle(&mut cx);
    cx.update(|window, cx| {
        window.click("book-search-input", cx);
        assert_eq!(window.find("book-search-input").focused(), Some(true));
        window.input("needle-only-final-chapter-279", cx);
    });
    until(&mut cx, |cx| {
        active(&app, cx).read_with(cx, |view, _| !view.search_results().is_empty())
    });
    cx.update(|window, cx| window.click("book-search-next", cx));
    until(&mut cx, |cx| {
        active(&app, cx).read_with(cx, |view, _| {
            view.chapter() == 1
                && view
                    .block_text(1)
                    .is_some_and(|text| text.contains("needle-only-final-chapter-279"))
        })
    });
    cx.update(|window, cx| window.click("book-search-close", cx));
    settle(&mut cx);
    let focus = active(&app, &cx).read_with(&cx, |view, _| view.handle());
    cx.update(|window, cx| assert!(focus.contains_focused(window, cx)));
    cx.dispatch_action(BookPreviousChapter);
    until(&mut cx, |cx| {
        active(&app, cx).read_with(cx, |view, _| {
            view.chapter() == 0 && view.block_text(0) == Some("Spine first chapter")
        })
    });
    cx.update(|window, cx| window.click(("book-side-item", 1usize), cx));
    until(&mut cx, |cx| {
        active(&app, cx).read_with(cx, |view, _| {
            view.chapter() == 1 && view.block_text(0) == Some("Final chapter")
        })
    });
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(host.writes.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[gpui_kit::test]
fn typography_theme_bookmarks_and_position_restore_without_writing_the_book(
    tcx: &mut TestAppContext,
) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let path = fixture(dir.path(), "persistent.epub", false);
    let original = std::fs::read(&path).unwrap();
    let host = FileHost::new(HostId(286102));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let first_id = open(&app, &mut cx, host.clone(), &path);
    let before = active(&app, &cx).read_with(&cx, |view, cx| view.document().read(cx).prefs());
    cx.update(|window, cx| {
        window.click("book-font-larger", cx);
        window.click("book-spacing", cx);
        window.click("book-width", cx);
        window.click("book-theme", cx);
        window.click("book-theme", cx);
    });
    settle(&mut cx);
    let chosen = active(&app, &cx).read_with(&cx, |view, cx| view.document().read(cx).prefs());
    assert!(chosen.font_scale > before.font_scale);
    assert_ne!(chosen.line_height, before.line_height);
    assert_ne!(chosen.page_width_rem, before.page_width_rem);
    assert_eq!(chosen.theme, lyrel_epub::ReaderTheme::Dark);
    cx.update(|window, cx| window.click(("book-side-item", 1usize), cx));
    until(&mut cx, |cx| {
        active(&app, cx).read_with(cx, |view, _| {
            view.chapter() == 1 && view.block_text(0) == Some("Final chapter")
        })
    });
    cx.update(|window, cx| window.click("book-bookmark", cx));
    settle(&mut cx);
    let record_path = super::super::book_document::record_file(host.id(), &path).unwrap();
    until(&mut cx, |_| {
        std::fs::read(&record_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<lyrel_epub::ReaderRecord>(&bytes).ok())
            .is_some_and(|record| {
                record.reading.chapter == 1
                    && record.bookmarks.len() == 1
                    && record.prefs.theme == lyrel_epub::ReaderTheme::Dark
            })
    });
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    settle(&mut cx);
    let second_id = open(&app, &mut cx, host.clone(), &path);
    assert_ne!(
        first_id, second_id,
        "reopen does not retain the first document"
    );
    active(&app, &cx).read_with(&cx, |view, cx| {
        assert_eq!(view.chapter(), 1);
        let doc = view.document().read(cx);
        assert_eq!(doc.prefs(), chosen);
        assert_eq!(doc.bookmarks()[0].chapter, 1);
    });
    cx.update(|window, cx| window.click("book-bookmarks", cx));
    settle(&mut cx);
    cx.update(|window, cx| window.click(("book-side-item", 0usize), cx));
    settle(&mut cx);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(host.writes.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[gpui_kit::test]
fn unsupported_and_corrupt_books_stay_visible_read_only_documents(tcx: &mut TestAppContext) {
    let dir = canonical_tempdir();
    let fixed = fixture(dir.path(), "fixed.epub", true);
    let corrupt = dir.path().join("corrupt.epub");
    std::fs::write(&corrupt, b"not an EPUB archive").unwrap();
    let host = FileHost::new(HostId(286103));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    for (path, unsupported) in [(&fixed, true), (&corrupt, false)] {
        let original = std::fs::read(path).unwrap();
        app.update_in(&mut cx, |app, window, cx| {
            app.editor_open_on_host(host.clone(), path, window, cx)
        });
        until(&mut cx, |cx| {
            app.read_with(cx, |app, _| {
                app.active_buffer()
                    .is_some_and(|file| file.path == *path && file.input.book().is_some())
            })
        });
        active(&app, &cx).read_with(&cx, |view, cx| {
            assert!(if unsupported {
                matches!(view.document().read(cx).state(), BookState::Unsupported(_))
            } else {
                matches!(view.document().read(cx).state(), BookState::Failed(_))
            });
        });
        cx.update(|window, _| assert!(window.find("book-unavailable").visible()));
        assert_eq!(std::fs::read(path).unwrap(), original);
    }
    assert_eq!(host.writes.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[gpui_kit::test]
fn same_chapter_search_theme_and_reopen_keep_the_last_reading_blocks(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let path = fixture(dir.path(), "search-position.epub", false);
    let original = std::fs::read(&path).unwrap();
    let host = FileHost::new(HostId(286104));
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, host.clone(), &path);
    until(&mut cx, |cx| {
        cx.update(|window, _| window.find(("book-block", 0u64)).bounds().size.height > px(16.))
    });
    let first = cx.update(|window, _| window.find(("book-block", 0u64)).bounds());
    cx.simulate_event(gpui_kit::ScrollEvent {
        position: first.center(),
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-120.))),
        ..Default::default()
    });
    settle(&mut cx);
    cx.update(|window, cx| window.click("book-search", cx));
    settle(&mut cx);
    cx.update(|window, cx| {
        window.click("book-search-input", cx);
        window.input("Complete long chapter paragraph 2999.", cx);
    });
    until(&mut cx, |cx| {
        active(&app, cx).read_with(cx, |view, _| !view.search_results().is_empty())
    });
    cx.update(|window, cx| window.click("book-search-next", cx));
    until(&mut cx, |cx| {
        active(&app, cx).read_with(cx, |view, cx| view.progress_label(cx).as_ref() == "50%")
    });
    cx.update(|window, cx| {
        window.click("book-theme", cx);
        window.click("book-theme", cx);
    });
    settle(&mut cx);
    cx.update(|window, _| {
        assert!(
            window.find(("book-block", 3001u64)).visible(),
            "the final paragraph remains visible after changing the theme"
        );
    });
    let record_path = super::super::book_document::record_file(host.id(), &path).unwrap();
    until(&mut cx, |_| {
        std::fs::read(&record_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<lyrel_epub::ReaderRecord>(&bytes).ok())
            .is_some_and(|record| {
                record.reading.chapter == 0
                    && record.reading.fraction > 0.99
                    && record.prefs.theme == ReaderTheme::Dark
            })
    });
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    settle(&mut cx);
    open(&app, &mut cx, host, &path);
    until(&mut cx, |cx| {
        active(&app, cx).read_with(cx, |view, cx| {
            view.progress_label(cx).as_ref() == "50%"
                && view.document().read(cx).prefs().theme == ReaderTheme::Dark
        })
    });
    cx.update(|window, _| {
        let row = window.find(("book-block", 3001u64));
        assert!(
            row.visible(),
            "the final paragraph stays in the restored viewport"
        );
        assert!(
            row.bounds().size.height > px(16.),
            "rich text has a measured line"
        );
    });
    assert_eq!(std::fs::read(&path).unwrap(), original);
}
