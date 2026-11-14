//! #280 的生产工作台接缝：静态图片作为真实文档打开、取景、关闭与失败。
//!
//! 这些测试只走工作台入口（`open_file_in_editor`、`editor_open_on_host`、
//! 键位与菜单动作）和真实 gpui 测试平台窗口，断言生产工作台的可见行为，
//! 不检查源码接线；夹具在测试内生成，不依赖协调目录或任何外部图片。
//! 这些不是原生 OS 窗口：真实桌面、DPI 与 SSH 传输由单独验收覆盖。

use super::super::test_host::FileHost;
use super::super::*;
use super::*;
// `image_document` has private `gif`/`tiff` submodules; these explicit
// imports keep the fixture encoders pointing at the registry crates.
use crate::ui::app::test_window::harness_with_tabs;
use ::gif;
use ::tiff;
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
            "image document transition did not settle"
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

fn solid(format: image::ImageFormat, width: u32, height: u32) -> Vec<u8> {
    let rgba = image::RgbaImage::from_pixel(width, height, image::Rgba([200, 30, 60, 255]));
    encode(format, image::DynamicImage::ImageRgba8(rgba))
}

/// JPEG 没有 alpha，编码器要 RGB。
fn solid_jpeg(width: u32, height: u32) -> Vec<u8> {
    let rgb = image::RgbImage::from_pixel(width, height, image::Rgb([200, 30, 60]));
    encode(
        image::ImageFormat::Jpeg,
        image::DynamicImage::ImageRgb8(rgb),
    )
}

fn encode(format: image::ImageFormat, image: image::DynamicImage) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, format).expect("fixture encodes");
    bytes.into_inner()
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
    active_id(app, cx)
}

fn active_id(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> BufferId {
    app.read_with(cx, |app, _| {
        app.tab_code()
            .and_then(TabCode::active_id)
            .expect("a buffer in front")
    })
}

fn image_view(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<ImageView> {
    app.read_with(cx, |app, _| {
        app.active_editor_input()
            .expect("a document in front")
            .image()
            .expect("an image document")
            .clone()
    })
}

fn content(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> ImageContent {
    image_view(app, cx).read_with(cx, |view, _| view.content().clone())
}

fn ready_size(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> (i32, i32) {
    match content(app, cx) {
        ImageContent::Ready(bitmap) => {
            let size = bitmap.image().size(0);
            (size.width.0, size.height.0)
        }
        ImageContent::Failed(message) => panic!("expected a decoded picture, got: {message}"),
        ImageContent::Loading => panic!("expected a decoded picture, still loading"),
    }
}

fn dispatch<A: gpui_kit::Action>(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, action: A) {
    let handle = image_view(app, cx).read_with(cx, |view, _| view.handle());
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
fn a_png_opens_as_a_read_only_document_with_framing(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(
        dir.path(),
        "quadrants.png",
        &solid(image::ImageFormat::Png, 64, 40),
    );
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);

    app.read_with(&cx, |app, _| {
        let file = app.editor.buffers.get(id).expect("installed document");
        assert!(file.input.image().is_some(), "an image document");
        assert!(!file.dirty, "a picture is never unsaved");
        assert_eq!(file.path, path);
        assert!(app.image_document_active());
        assert_eq!(
            app.editor_active_location()
                .map(|(host, path)| (host, path.to_path_buf())),
            Some((HostId::LOCAL, path.clone()))
        );
        // 只读文档不是语言服务器能看的文本文件。
        assert!(app.editor.local_file(id).is_none());
    });
    assert_eq!(ready_size(&app, &cx), (64, 40));

    cx.update(|window, _| {
        assert!(window.find("image-fit").visible(), "framing controls");
        assert!(
            window.find("status-image-facts").visible(),
            "format and size"
        );
        assert!(
            window.find("status-image-zoom").visible(),
            "zoom percentage"
        );
        assert!(window.find("status-image-readonly").visible());
        assert!(window.find("status-problems").visible());
        assert!(
            window.try_find("status-cursor").is_none(),
            "a picture has no caret to report"
        );
        assert!(window.try_find("status-wrap").is_none());
        assert!(window.try_find("status-eol").is_none());
    });

    // 填满窗口与停靠共用同一份文档：画布大小变了，取景按新画布重算。
    app.update_in(&mut cx, |app, window, cx| {
        app.toggle_document_fill(cx);
        app.focus_editor(window, cx);
    });
    settle(&mut cx);
    cx.update(|window, _| {
        assert!(
            window.find("image-fit").visible(),
            "fill keeps the controls"
        );
        assert!(window.find("status-image-zoom").visible());
    });
    app.update_in(&mut cx, |app, window, cx| {
        app.toggle_document_fill(cx);
        app.focus_editor(window, cx);
    });
    settle(&mut cx);

    // 关闭不追问：没有未保存的编辑。
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    settle(&mut cx);
    app.read_with(&cx, |app, _| {
        assert!(app.editor.buffers.get(id).is_none(), "buffer released");
        assert!(
            app.tab_code().is_some_and(|code| code.files.is_empty()),
            "tab strip emptied"
        );
    });
}

#[gpui_kit::test]
fn all_five_static_formats_decode_as_themselves(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let cases: [(&str, Vec<u8>, ImageFormat); 5] = [
        (
            "shot.png",
            solid(image::ImageFormat::Png, 33, 17),
            ImageFormat::Png,
        ),
        ("shot.jpg", solid_jpeg(33, 17), ImageFormat::Jpeg),
        (
            "shot.webp",
            solid(image::ImageFormat::WebP, 33, 17),
            ImageFormat::WebP,
        ),
        (
            "shot.bmp",
            solid(image::ImageFormat::Bmp, 33, 17),
            ImageFormat::Bmp,
        ),
        (
            "shot.ico",
            solid(image::ImageFormat::Ico, 33, 17),
            ImageFormat::Ico,
        ),
    ];
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    for (name, bytes, format) in cases {
        let path = fixture(dir.path(), name, &bytes);
        open(&app, &mut cx, &path);
        assert_eq!(
            ready_size(&app, &cx),
            (33, 17),
            "{name} keeps its own dimensions"
        );
        image_view(&app, &cx).read_with(&cx, |view, cx| {
            assert_eq!(view.document().read(cx).format(), Some(format), "{name}");
            assert_eq!(view.scale(), 1., "a 33×17 picture is drawn at its size");
        });
        cx.update(|window, _| {
            assert!(
                window.find("image-actual").visible(),
                "{name} renders its framing controls"
            );
        });
        let previous = active_id(&app, &cx);
        app.update_in(&mut cx, |app, window, cx| {
            app.editor_close_buffer(previous, window, cx)
        });
        settle(&mut cx);
    }
}

#[gpui_kit::test]
fn failures_are_documents_instead_of_silent_text(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);

    // 1. 名字说是 PNG，字节是截断的 PNG：仍然要有位置，并说清楚。
    let broken = fixture(
        dir.path(),
        "broken.png",
        b"\x89PNG\r\n\x1a\nthis is not the rest of a png",
    );
    open(&app, &mut cx, &broken);
    match content(&app, &cx) {
        ImageContent::Failed(message) => assert!(
            message.contains(&t(L10nKey::EditorImageUndecodable)),
            "the failure names the decode, got: {message}"
        ),
        ImageContent::Ready(_) => panic!("a truncated png decoded"),
        ImageContent::Loading => panic!("a truncated png is still loading"),
    }
    cx.update(|window, _| {
        assert!(window.find("status-image-failed").visible());
        assert!(
            window.try_find("image-fit").is_none(),
            "no framing promised over bytes that are not a picture"
        );
    });

    // 2. 名字说是 PNG，内容是文本：不悄悄退回文本编辑器。
    let text = fixture(dir.path(), "notes.png", b"plain words, wrong extension");
    open(&app, &mut cx, &text);
    assert!(matches!(content(&app, &cx), ImageContent::Failed(_)));

    // 3. 没有图片扩展名，但字节是 PNG：内容探针认出来。
    let disguised = fixture(
        dir.path(),
        "picture.bin",
        &solid(image::ImageFormat::Png, 8, 8),
    );
    open(&app, &mut cx, &disguised);
    assert_eq!(ready_size(&app, &cx), (8, 8));

    // 4. 普通文本文件仍然是文本，不被探针抢走。
    let plain = fixture(dir.path(), "notes.txt", b"just text\n");
    open(&app, &mut cx, &plain);
    app.read_with(&cx, |app, _| {
        let front = app.active_buffer().expect("text document");
        assert!(front.input.code().is_some(), "still the text editor");
        assert!(!app.image_document_active());
    });
}

#[gpui_kit::test]
fn the_image_budget_refuses_without_falling_back_to_text(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);

    // 文件预算：不读完 32 MiB 字节就能说它太大。
    let mut oversized = Vec::with_capacity(33 * 1024 * 1024);
    oversized.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    oversized.resize(33 * 1024 * 1024, 0);
    let big = fixture(dir.path(), "huge.png", &oversized);
    open(&app, &mut cx, &big);
    match content(&app, &cx) {
        ImageContent::Failed(message) => assert!(
            message.contains(&t(L10nKey::MarkdownImageLimit)),
            "the byte budget is named, got: {message}"
        ),
        ImageContent::Ready(_) => panic!("a file over the image budget decoded"),
        ImageContent::Loading => panic!("a file over the image budget is loading"),
    }
    app.read_with(&cx, |app, _| {
        assert!(
            app.active_buffer()
                .is_some_and(|file| file.input.image().is_some())
        )
    });

    // 像素预算：每边上限是 8192，字节本身很小。
    let wide = fixture(
        dir.path(),
        "wide.png",
        &solid(image::ImageFormat::Png, 8193, 1),
    );
    open(&app, &mut cx, &wide);
    match content(&app, &cx) {
        ImageContent::Failed(message) => {
            assert!(message.contains(&t(L10nKey::MarkdownImageLimit)))
        }
        ImageContent::Ready(_) => panic!("a 8193-pixel-wide picture decoded"),
        ImageContent::Loading => panic!("a 8193-pixel-wide picture is loading"),
    }
}

#[gpui_kit::test]
fn keyboard_and_controls_frame_the_picture_and_pan_it(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(
        dir.path(),
        "wide.png",
        &solid(image::ImageFormat::Png, 900, 600),
    );
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    let view = image_view(&app, &cx);

    // 画面装得下时 fit 不放大，也不超出窗口。
    let fitted = view.read_with(&cx, |view, _| view.scale());
    assert!(fitted > 0. && fitted <= 1., "fit scale {fitted}");

    // 键盘：1 = 实际大小。
    cx.update(|window, cx| window.press("1", cx));
    settle(&mut cx);
    assert_eq!(
        view.read_with(&cx, |view, _| view.scale()),
        1.,
        "actual size"
    );

    // 键盘：⌘/Ctrl + = 放大，⌘/Ctrl + - 缩小，0 回到适应窗口。
    cx.update(|window, cx| window.press(&secondary("="), cx));
    settle(&mut cx);
    let zoomed = view.read_with(&cx, |view, _| view.scale());
    assert!(zoomed > 1., "zoom in went to {zoomed}");
    cx.update(|window, cx| window.press(&secondary("-"), cx));
    settle(&mut cx);
    assert!(
        (view.read_with(&cx, |view, _| view.scale()) - 1.).abs() < 0.001,
        "zoom out returns to actual size"
    );
    cx.update(|window, cx| window.press("0", cx));
    settle(&mut cx);
    assert!(
        (view.read_with(&cx, |view, _| view.scale()) - fitted).abs() < 0.001,
        "fit again"
    );

    // 状态行标的是画出来的比例：实际大小就是 100%，一次放大是 125%。
    cx.update(|window, cx| window.press("1", cx));
    settle(&mut cx);
    assert_eq!(
        view.read_with(&cx, |view, _| view.zoom_label().to_string()),
        "100%"
    );
    cx.update(|window, cx| window.press(&secondary("="), cx));
    settle(&mut cx);
    assert_eq!(
        view.read_with(&cx, |view, _| view.zoom_label().to_string()),
        "125%"
    );
    cx.update(|window, _| assert!(window.find("status-image-zoom").visible()));
    cx.update(|window, cx| window.press("0", cx));
    settle(&mut cx);

    // 放大到超出画布后，方向键推动滚动位置，并在边缘停住。
    for _ in 0..8 {
        dispatch(&app, &mut cx, ImageZoomIn);
    }
    settle(&mut cx);
    let max = view.read_with(&cx, |view, _| view.scroll().max_offset());
    assert!(max.x > px(0.), "the picture is wider than the canvas");
    cx.update(|window, cx| window.press("right", cx));
    settle(&mut cx);
    let after = view.read_with(&cx, |view, _| view.scroll().offset());
    assert!(after.x < px(0.), "Right walks towards the right edge");
    // 走到底：内容宽出多少就按多少次，最后一格必须停住而不是越界。
    let presses = (f32::from(max.x) / PAN_STEP).ceil() as usize + 2;
    for _ in 0..presses {
        cx.update(|window, cx| window.press("right", cx));
    }
    settle(&mut cx);
    let at_edge = view.read_with(&cx, |view, _| {
        (view.scroll().offset(), view.scroll().max_offset())
    });
    assert_eq!(
        at_edge.0.x, -at_edge.1.x,
        "panning stops at the last column instead of running past it"
    );
}

#[gpui_kit::test]
fn text_only_commands_leave_a_picture_alone(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let before = solid(image::ImageFormat::Png, 40, 30);
    let path = fixture(dir.path(), "read-only.png", &before);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);

    app.update_in(&mut cx, |app, window, cx| {
        // Save / Save As / Go to Line / Go to Symbol 都只为文本存在。
        app.editor_save_active(window, cx);
        app.editor_save_as_active(window, cx);
        app.editor_go_to_line(window, cx);
        app.editor_go_to_symbol(window, cx);
        app.toggle_line_ending(cx);
        let wrap_before = cx.global::<crate::core::config::Config>().editor_soft_wrap;
        app.toggle_document_wrap(window, cx);
        assert_eq!(
            cx.global::<crate::core::config::Config>().editor_soft_wrap,
            wrap_before,
            "wrap stays a text preference"
        );
        assert!(!app.editor.has_bar(), "no prompt opened over a picture");
        assert_eq!(
            app.editor_search_tabs().len(),
            1,
            "no symbol picker over a picture"
        );
        let file = app.editor.buffers.get(id).expect("still installed");
        assert!(!file.dirty);
        assert_eq!(file.format(cx), None, "a picture has no text format");
    });
    settle(&mut cx);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "nothing was written over the file"
    );

    // 编辑动作不会落到图片上：撤销没有可撤销的东西。
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_dispatch(&gpui_kit::component::input::Undo, window, cx);
    });
    settle(&mut cx);
    assert_eq!(ready_size(&app, &cx), (40, 30));
    app.read_with(&cx, |app, _| {
        assert!(!app.active_buffer().expect("still open").dirty);
    });
}

#[gpui_kit::test]
fn an_external_rewrite_replaces_the_picture(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(
        dir.path(),
        "watched.png",
        &solid(image::ImageFormat::Png, 20, 20),
    );
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    assert_eq!(ready_size(&app, &cx), (20, 20));
    // 等监视真的装上再写,不然外部变化没有观察者。
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.watching())
    });
    std::fs::write(&path, solid(image::ImageFormat::Png, 90, 70)).unwrap();
    until(&mut cx, |cx| ready_size(&app, cx) == (90, 70));
    app.read_with(&cx, |app, _| {
        let file = app.active_buffer().expect("still open");
        assert!(file.conflict.is_none(), "a clean picture reloads itself");
        assert!(!file.dirty);
    });
}

#[gpui_kit::test]
fn a_deleted_picture_offers_close_not_a_write(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(
        dir.path(),
        "doomed.png",
        &solid(image::ImageFormat::Png, 20, 20),
    );
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    assert_eq!(ready_size(&app, &cx), (20, 20));

    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.watching())
    });
    std::fs::remove_file(&path).unwrap();
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.conflict.is_some())
        })
    });
    // 只读图片没有任何可写的命令:banner 给 Close,不给 Save/KeepMine。
    cx.update(|window, _| {
        assert!(window.find("editor-deleted-close").visible());
        assert!(window.try_find("editor-deleted-save").is_none());
        assert!(window.try_find("editor-conflict-keep").is_none());
    });
    // Close 是真实动作,不是摆设。
    cx.update(|window, cx| window.click("editor-deleted-close", cx));
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer()
                .is_none_or(|file| file.conflict.is_none())
        })
    });
}

#[gpui_kit::test]
fn the_owning_host_decides_which_bytes_are_shown(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let local_bytes = solid(image::ImageFormat::Png, 10, 10);
    let path = fixture(dir.path(), "pic.png", &local_bytes);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let local = open(&app, &mut cx, &path);

    // 同一路径、另一个 Host：身份不同，不共用缓冲区。
    let second: SharedHost = FileHost::new(HostId(69));
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_open_on_host(second, &path, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.tab_code()
                .and_then(TabCode::active_id)
                .is_some_and(|id| id != local)
        })
    });
    app.read_with(&cx, |app, _| {
        assert_eq!(app.editor_active_location().unwrap().0, HostId(69));
    });
    assert_eq!(ready_size(&app, &cx), (10, 10), "the plain host's own copy");

    // 映射 Host 的根落在别的目录：读到的是它自己的图，不是本地同名文件。
    let decoy = super::super::test_host::canonical_tempdir();
    std::fs::write(
        decoy.path().join("pic.png"),
        solid(image::ImageFormat::Png, 30, 40),
    )
    .unwrap();
    let mapped: SharedHost = FileHost::mapped(HostId(70), decoy.path());
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_open_on_host(mapped, Path::new("/pic.png"), window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.tab_code()
                .and_then(TabCode::active_id)
                .is_some_and(|id| {
                    id != local
                        && app
                            .editor_active_location()
                            .is_some_and(|(host, _)| host == HostId(70))
                })
        })
    });
    assert_eq!(
        ready_size(&app, &cx),
        (30, 40),
        "the mapped Host's own file"
    );

    // 切回本地那份仍然是它自己的图,两份源字节都没被碰过。
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
    assert_eq!(ready_size(&app, &cx), (10, 10), "the local original");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        local_bytes,
        "local bytes were left alone"
    );
    let decoy =
        image::load_from_memory(&std::fs::read(decoy.path().join("pic.png")).unwrap()).unwrap();
    assert_eq!(
        (decoy.width(), decoy.height()),
        (30, 40),
        "the decoy bytes were left alone"
    );
}

#[gpui_kit::test]
fn split_groups_keep_their_own_framing_over_one_document(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(
        dir.path(),
        "split.png",
        &solid(image::ImageFormat::Png, 60, 60),
    );
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx), "the picture splits");
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
        let file = app.active_buffer().expect("picture");
        (
            file.presentations
                .image_existing(tab, right)
                .expect("front view"),
            file.presentations
                .image_existing(tab, left)
                .expect("other view"),
        )
    });
    assert_ne!(front, other, "each group draws its own view");
    app.read_with(&cx, |app, cx| {
        assert!(app.active_buffer().is_some());
        assert_eq!(
            front.read(cx).document().entity_id(),
            other.read(cx).document().entity_id(),
            "one decoded document behind both"
        );
    });

    // 在右侧（有焦点）放大，左侧的取景不变。
    let before = other.read_with(&cx, |view, _| view.scale());
    dispatch(&app, &mut cx, ImageZoomIn);
    app.read_with(&cx, |app, cx| {
        let file = app.active_buffer().expect("picture");
        let (front, other) = (
            file.presentations.image_existing(tab, right).unwrap(),
            file.presentations.image_existing(tab, left).unwrap(),
        );
        assert!(front.read(cx).scale() > 1., "the focused group zoomed");
        assert_eq!(
            other.read(cx).scale(),
            before,
            "the other group kept its framing"
        );
    });
}

#[gpui_kit::test]
fn a_picture_whose_tab_closed_before_its_read_lands_leaves_nothing_behind(
    tcx: &mut TestAppContext,
) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = fixture(
        dir.path(),
        "late.png",
        &solid(image::ImageFormat::Png, 12, 12),
    );
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 2);
    let host = FileHost::new(HostId(71));
    let gate = host.pause_next_read();
    let shared: SharedHost = host.clone();
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_open_on_host(shared, &path, window, cx)
    });
    settle(&mut cx);
    app.read_with(&cx, |app, _| {
        assert_eq!(app.editor.buffers.iter().count(), 0, "still reading");
    });
    // 读取还没回来，标签页先关掉。
    app.update_in(&mut cx, |app, window, cx| app.close_tab(0, window, cx));
    settle(&mut cx);
    let _ = gate.send(());
    settle(&mut cx);
    settle(&mut cx);
    app.read_with(&cx, |app, _| {
        assert_eq!(
            app.editor.buffers.iter().count(),
            0,
            "a late read installs no document into a closed tab"
        );
    });

    // 同一个窗口里再打开一次仍然成功：没有卡住的 pending 状态。
    let id = open(&app, &mut cx, &path);
    app.read_with(&cx, |app, _| {
        assert!(
            app.editor
                .buffers
                .get(id)
                .is_some_and(|file| file.input.image().is_some())
        )
    });
    assert_eq!(ready_size(&app, &cx), (12, 12));
}

/// 3×1 的 GIF 夹具：调色板四色（0 红、1 绿、2 蓝、3 透明）。
fn gif_fixture(
    frames: &[(u16, Vec<u8>, gif::DisposalMethod)],
    repeat: gif::Repeat,
    delay: u16,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = gif::Encoder::new(
            &mut bytes,
            3,
            1,
            &[255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0],
        )
        .expect("palette encodes");
        encoder.set_repeat(repeat).expect("repeat encodes");
        for (left, pixels, dispose) in frames {
            encoder
                .write_frame(&gif::Frame {
                    left: *left,
                    width: pixels.len() as u16,
                    height: 1,
                    delay,
                    dispose: *dispose,
                    transparent: Some(3),
                    buffer: std::borrow::Cow::Borrowed(pixels),
                    ..Default::default()
                })
                .expect("frame encodes");
        }
    }
    bytes
}

fn tiff_fixture(pages: &[(u32, u32, [u8; 3])]) -> Vec<u8> {
    use tiff::encoder::colortype::RGB8;
    let mut bytes = Vec::new();
    {
        let mut encoder =
            tiff::encoder::TiffEncoder::new(std::io::Cursor::new(&mut bytes)).expect("tiff starts");
        for (width, height, colour) in pages {
            let image = encoder
                .new_image::<RGB8>(*width, *height)
                .expect("page starts");
            let mut pixels = Vec::with_capacity((*width * *height * 3) as usize);
            for _ in 0..*width * *height {
                pixels.extend_from_slice(colour);
            }
            image.write_data(&pixels).expect("page writes");
        }
    }
    bytes
}

/// 帧/页的 RGBA 像素；`render` 产物是 GPUI 消费的 BGRA。
fn frame_pixels(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Vec<[u8; 4]> {
    let ImageContent::Ready(bitmap) = content(app, cx) else {
        panic!("expected a decoded frame");
    };
    let image = bitmap.image();
    image
        .as_bytes(0)
        .expect("one frame")
        .chunks_exact(4)
        .map(|pixel| [pixel[2], pixel[1], pixel[0], pixel[3]])
        .collect()
}

fn advance(cx: &mut VisualTestContext, millis: u64) {
    cx.executor().advance_clock(Duration::from_millis(millis));
    settle(cx);
}

fn show_tab(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, index: usize) {
    app.update_in(cx, |app, window, cx| {
        app.tabs.select(index);
        app.focus_editor(window, cx);
        cx.notify();
    });
    settle(cx);
}

fn image_document(
    app: &Entity<LyrelApp>,
    cx: &VisualTestContext,
) -> gpui_kit::WeakEntity<ImageDocument> {
    image_view(app, cx).read_with(cx, |view, _| view.document().downgrade())
}

#[gpui_kit::test]
fn gif_animation_composites_frames_and_honours_finite_loops(tcx: &mut TestAppContext) {
    use gif::DisposalMethod::{Background, Keep, Previous};
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let bytes = gif_fixture(
        &[
            (0, vec![0, 0, 0], Keep),
            (1, vec![1], Background),
            (0, vec![2], Previous),
            (2, vec![1], Keep),
        ],
        gif::Repeat::Finite(4),
        10,
    );
    let path = fixture(dir.path(), "loop.gif", &bytes);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| {
        matches!(content(&app, cx), ImageContent::Ready(_))
    });
    let red = [255, 0, 0, 255];
    let green = [0, 255, 0, 255];
    let blue = [0, 0, 255, 255];
    let transparent = [0, 0, 0, 0];
    let cycle = [
        [red, red, red],
        [red, green, red],
        [blue, transparent, red],
        [red, transparent, green],
    ];
    let first = frame_pixels(&app, &cx);
    let start = cycle
        .iter()
        .position(|frame| frame.as_slice() == first.as_slice())
        .expect("the first visible frame is one of the fixture's frames");
    let mut seen = 1;
    while seen < 8 {
        let previous = frame_pixels(&app, &cx);
        until(&mut cx, |cx| frame_pixels(&app, cx) != previous);
        let frame = frame_pixels(&app, &cx);
        assert_eq!(
            frame,
            cycle[(start + seen) % cycle.len()],
            "frame {seen} after starting at {start}"
        );
        seen += 1;
    }
    // 动画走完预定圈数后停在最后一帧，而不是循环取累计帧数。
    let mut previous = frame_pixels(&app, &cx);
    let mut stable = 0;
    for _ in 0..20 {
        advance(&mut cx, 100);
        let now = frame_pixels(&app, &cx);
        if now == previous {
            stable += 1;
            if stable >= 3 {
                break;
            }
        } else {
            stable = 0;
            previous = now;
        }
    }
    assert!(
        stable >= 3,
        "the finite animation stops instead of looping forever"
    );
    cx.update(|window, _| {
        assert!(window.find("image-fit").visible());
        assert!(
            window
                .find("status-image-facts")
                .label()
                .is_some_and(|facts| facts.contains("GIF") && facts.contains("3 × 1")),
            "the status names the animation and its source size"
        );
    });
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "the source is untouched"
    );
}

#[gpui_kit::test]
fn hiding_an_animation_stops_its_frames_and_closing_stops_the_task(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let bytes = gif_fixture(
        &[
            (0, vec![0, 0, 0], gif::DisposalMethod::Keep),
            (0, vec![1, 1, 1], gif::DisposalMethod::Keep),
            (0, vec![2, 2, 2], gif::DisposalMethod::Keep),
        ],
        gif::Repeat::Infinite,
        10,
    );
    let path = fixture(dir.path(), "hidden.gif", &bytes);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 2);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| {
        matches!(content(&app, cx), ImageContent::Ready(_))
    });

    // 切到终端标签页：图片 presentation 不再被画，帧任务必须停下。
    show_tab(&app, &mut cx, 1);
    advance(&mut cx, 300);
    let paused = frame_pixels(&app, &cx);
    for _ in 0..4 {
        advance(&mut cx, 250);
    }
    assert_eq!(
        frame_pixels(&app, &cx),
        paused,
        "a hidden animation decodes no further frames"
    );

    // 回到图片：从停下的那一帧继续，而不是从头或跳帧。
    show_tab(&app, &mut cx, 0);
    until(&mut cx, |cx| frame_pixels(&app, cx) != paused);

    // 关闭释放文档；弱引用失效后任务不可能再对死文档工作。
    let document = image_document(&app, &cx);
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    settle(&mut cx);
    assert!(
        document.upgrade().is_none(),
        "closing the last presentation releases the document and its frame task"
    );
    advance(&mut cx, 500);
    app.read_with(&cx, |app, _| {
        assert_eq!(app.editor.buffers.iter().count(), 0);
    });
}

#[gpui_kit::test]
fn a_multipage_tiff_navigates_pages_with_identity_and_keeps_framing(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let bytes = tiff_fixture(&[
        (40, 20, [200, 10, 10]),
        (20, 60, [10, 200, 10]),
        (70, 10, [10, 10, 200]),
    ]);
    let path = fixture(dir.path(), "pages.tiff", &bytes);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| ready_size(&app, cx) == (40, 20));
    let view = image_view(&app, &cx);
    assert_eq!(
        view.read_with(&cx, |view, cx| view.page_label(cx)),
        Some(SharedString::from("Page 1 of 3"))
    );
    cx.update(|window, _| {
        assert!(window.find("image-page-facts").visible());
        assert_eq!(
            window.find("status-image-page").label(),
            Some("Page 1 of 3")
        );
        assert!(
            window
                .find("status-image-facts")
                .label()
                .is_some_and(|facts| facts.contains("TIFF") && facts.contains("40 × 20"))
        );
    });

    // 实际大小与缩放模式随页保留；页尺寸成为新的取景依据。
    cx.update(|window, cx| window.press("1", cx));
    settle(&mut cx);
    assert_eq!(view.read_with(&cx, |view, _| view.scale()), 1.);
    cx.update(|window, cx| window.press("pagedown", cx));
    until(&mut cx, |cx| ready_size(&app, cx) == (20, 60));
    assert_eq!(view.read_with(&cx, |view, _| view.scale()), 1.);
    assert_eq!(view.read_with(&cx, |view, _| view.page()), 1);
    cx.update(|window, _| {
        assert_eq!(
            window.find("status-image-page").label(),
            Some("Page 2 of 3")
        );
        assert!(
            window
                .find("status-image-facts")
                .label()
                .is_some_and(|facts| facts.contains("20 × 60"))
        );
    });
    let focused = view.read_with(&cx, |view, _| view.handle());
    cx.update(|window, cx| assert!(focused.contains_focused(window, cx)));

    // 工具栏按钮与键盘是同一组命令。
    cx.update(|window, cx| window.click("image-page-next", cx));
    until(&mut cx, |cx| ready_size(&app, cx) == (70, 10));
    cx.update(|window, cx| window.click("image-page-previous", cx));
    until(&mut cx, |cx| ready_size(&app, cx) == (20, 60));
    cx.update(|window, cx| window.press("pageup", cx));
    until(&mut cx, |cx| ready_size(&app, cx) == (40, 20));

    // 翻页清掉上一页的平移偏移，缩放模式保留；fit 仍按当前页重算。
    for _ in 0..8 {
        dispatch(&app, &mut cx, ImageZoomIn);
    }
    cx.update(|window, cx| window.press("right", cx));
    settle(&mut cx);
    assert!(
        view.read_with(&cx, |view, _| view.scroll().offset()).x < px(0.),
        "pan moved the canvas"
    );
    cx.update(|window, cx| window.press("pagedown", cx));
    until(&mut cx, |cx| ready_size(&app, cx) == (20, 60));
    assert_eq!(
        view.read_with(&cx, |view, _| view.scroll().offset()),
        Point::default(),
        "a page change starts at its own origin"
    );
    cx.update(|window, cx| window.press("0", cx));
    settle(&mut cx);
    assert!(view.read_with(&cx, |view, _| view.scale()) <= 1.);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "the source is untouched"
    );
}

#[gpui_kit::test]
fn a_late_page_decode_cannot_replace_the_current_page(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let bytes = tiff_fixture(&[
        (40, 20, [200, 10, 10]),
        (20, 60, [10, 200, 10]),
        (70, 10, [10, 10, 200]),
    ]);
    let path = fixture(dir.path(), "race.tiff", &bytes);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| ready_size(&app, cx) == (40, 20));
    let view = image_view(&app, &cx);
    // 同一帧里连翻两页：第 1 页的异步解码会在第 2 页成为当前页之后落地。
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.next_page(cx);
            view.next_page(cx);
        })
    });
    until(&mut cx, |cx| ready_size(&app, cx) == (70, 10));
    assert_eq!(
        view.read_with(&cx, |view, _| view.page()),
        2,
        "the later request is the visible page"
    );
    cx.update(|window, _| {
        assert_eq!(
            window.find("status-image-page").label(),
            Some("Page 3 of 3")
        );
    });
    // 回到第 2 页仍是它自己的尺寸，没有被第 3 页的缓存顶替。
    cx.update(|window, cx| window.press("pageup", cx));
    until(&mut cx, |cx| ready_size(&app, cx) == (20, 60));
    assert_eq!(view.read_with(&cx, |view, _| view.page()), 1);
}

#[gpui_kit::test]
fn split_tiff_groups_keep_independent_pages_and_live_tiles(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let bytes = tiff_fixture(&[
        (40, 20, [200, 10, 10]),
        (20, 60, [10, 200, 10]),
        (70, 10, [10, 10, 200]),
    ]);
    let path = fixture(dir.path(), "split.tiff", &bytes);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| ready_size(&app, cx) == (40, 20));
    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx), "the TIFF splits");
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
    let (front, other) = app.read_with(&cx, |app, _cx| {
        let file = app.active_buffer().expect("TIFF");
        (
            file.presentations
                .image_existing(tab, right)
                .expect("front view"),
            file.presentations
                .image_existing(tab, left)
                .expect("other view"),
        )
    });
    assert_eq!(
        front.read_with(&cx, |view, _| view.page()),
        0,
        "a split starts at the page it was showing"
    );
    // 右侧翻到第 3 页，左侧仍在第 1 页；缓存churn 不能收回仍在画的纹理。
    cx.update(|window, cx| window.press("pagedown", cx));
    cx.update(|window, cx| window.press("pagedown", cx));
    until(&mut cx, |cx| {
        front.read_with(cx, |view, _| {
            matches!(view.content(), ImageContent::Ready(bitmap) if bitmap.image().size(0).width.0 == 70)
        })
    });
    assert_eq!(other.read_with(&cx, |view, _| view.page()), 0);
    assert!(matches!(
        other.read_with(&cx, |view, _| view.content().clone()),
        ImageContent::Ready(bitmap)
            if bitmap.image().size(0).width.0 == 40 && bitmap.image().size(0).height.0 == 20
    ));
    dispatch(&app, &mut cx, ImageZoomIn);
    app.read_with(&cx, |app, cx| {
        let file = app.active_buffer().expect("TIFF");
        let front = file.presentations.image_existing(tab, right).unwrap();
        assert!(front.read(cx).scale() > 1., "the focused page zoomed");
    });
    // 右侧回到第 1 页仍能读到自己那份，不与左侧错页。
    cx.update(|window, cx| window.press("pageup", cx));
    cx.update(|window, cx| window.press("pageup", cx));
    until(&mut cx, |cx| {
        front.read_with(cx, |view, _| {
            matches!(view.content(), ImageContent::Ready(bitmap) if bitmap.image().size(0).width.0 == 40)
        })
    });
    assert_eq!(other.read_with(&cx, |view, _| view.page()), 0);
}

#[gpui_kit::test]
fn corrupt_and_oversized_animations_and_pages_fail_visibly(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);

    for (name, bytes) in [
        ("broken.gif", b"GIF89a and then plain words".to_vec()),
        ("broken.tiff", b"II\x2a\x00 not a tiff".to_vec()),
    ] {
        let path = fixture(dir.path(), name, &bytes);
        open(&app, &mut cx, &path);
        until(&mut cx, |cx| {
            matches!(content(&app, cx), ImageContent::Failed(_))
        });
        cx.update(|window, _| {
            assert!(window.find("status-image-failed").visible());
            assert!(
                window.try_find("image-fit").is_none(),
                "{name} promises no framing over bytes that are not a picture"
            );
        });
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "{name} is untouched");
    }

    // 截断的动画：首帧可能还在，但失败必须可见，不能停在一张半帧上。
    let full = gif_fixture(
        &[
            (0, vec![0, 0, 0], gif::DisposalMethod::Keep),
            (0, vec![1, 1, 1], gif::DisposalMethod::Keep),
        ],
        gif::Repeat::Finite(0),
        10,
    );
    let mut cut = full.clone();
    cut.truncate(full.len() * 3 / 4);
    let path = fixture(dir.path(), "truncated.gif", &cut);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| {
        matches!(content(&app, cx), ImageContent::Failed(_))
    });
    cx.update(|window, _| assert!(window.find("status-image-failed").visible()));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        cut,
        "the source is untouched"
    );

    // 动画合成预算超出时打开即拒绝，不静默降采样。
    let mut over = gif_fixture(
        &[(0, vec![0, 0, 0], gif::DisposalMethod::Keep)],
        gif::Repeat::Finite(0),
        10,
    );
    over[6..8].copy_from_slice(&3000u16.to_le_bytes());
    over[8..10].copy_from_slice(&2000u16.to_le_bytes());
    let path = fixture(dir.path(), "huge.gif", &over);
    open(&app, &mut cx, &path);
    match content(&app, &cx) {
        ImageContent::Failed(message) => assert!(
            message.contains("MiB"),
            "the animation budget is named, got: {message}"
        ),
        _ => panic!("an over-budget animation was not refused"),
    }

    // 单页超出像素预算：可见的页失败，不退回静态图或别的页。
    let bytes = tiff_fixture(&[(8193, 1, [1, 2, 3])]);
    let path = fixture(dir.path(), "wide.tiff", &bytes);
    open(&app, &mut cx, &path);
    until(&mut cx, |cx| {
        matches!(content(&app, cx), ImageContent::Failed(_))
    });
    cx.update(|window, _| {
        assert!(window.find("status-image-failed").visible());
        assert!(
            window.try_find("image-page-facts").is_none(),
            "a single-page TIFF does not promise page navigation"
        );
    });
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "the source is untouched"
    );
}

#[gpui_kit::test]
fn the_owning_host_supplies_animation_and_page_bytes(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let local = super::super::test_host::canonical_tempdir();
    let decoy = super::super::test_host::canonical_tempdir();
    let local_gif = gif_fixture(
        &[(0, vec![0, 0, 0], gif::DisposalMethod::Keep)],
        gif::Repeat::Finite(0),
        10,
    );
    let remote_gif = {
        let mut bytes = Vec::new();
        {
            let mut encoder =
                gif::Encoder::new(&mut bytes, 2, 2, &[0, 255, 0, 0, 0, 0]).expect("palette");
            encoder.set_repeat(gif::Repeat::Finite(0)).expect("repeat");
            encoder
                .write_frame(&gif::Frame {
                    width: 2,
                    height: 2,
                    delay: 10,
                    buffer: std::borrow::Cow::Owned(vec![0, 0, 0, 0]),
                    ..Default::default()
                })
                .expect("frame");
        }
        bytes
    };
    let local_tiff = tiff_fixture(&[(10, 10, [1, 2, 3])]);
    let remote_tiff = tiff_fixture(&[(30, 20, [9, 9, 9]), (20, 30, [7, 7, 7])]);
    fixture(local.path(), "anim.gif", &local_gif);
    fixture(local.path(), "pages.tiff", &local_tiff);
    fixture(decoy.path(), "anim.gif", &remote_gif);
    fixture(decoy.path(), "pages.tiff", &remote_tiff);

    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let shared: SharedHost = FileHost::mapped(HostId(72), decoy.path());
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_open_on_host(shared.clone(), Path::new("/anim.gif"), window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.editor_active_location()
                .is_some_and(|(host, _)| host == HostId(72))
        })
    });
    until(&mut cx, |cx| ready_size(&app, cx) == (2, 2));

    app.update_in(&mut cx, |app, window, cx| {
        app.editor_open_on_host(shared, Path::new("/pages.tiff"), window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.editor_active_location()
                .is_some_and(|(host, path)| host == HostId(72) && path.ends_with("pages.tiff"))
        })
    });
    until(&mut cx, |cx| ready_size(&app, cx) == (30, 20));
    assert_eq!(
        image_view(&app, &cx).read_with(&cx, |view, cx| view.page_label(cx)),
        Some(SharedString::from("Page 1 of 2"))
    );
    assert_eq!(
        std::fs::read(local.path().join("anim.gif")).unwrap(),
        local_gif
    );
    assert_eq!(
        std::fs::read(local.path().join("pages.tiff")).unwrap(),
        local_tiff
    );
}
