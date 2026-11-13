// #276: static iTerm2 `OSC 1337;File=` images through the production seam —
// real PTY bytes -> daemon authority -> ordered pane stream -> rooted
// TerminalView / TerminalElement paint. The recorded producer below is the
// exact stdout of the official iTerm2 imgcat script (SHA-256
// 56f39f8c730eb7a64fed2bdb9e2c0bd2448f72e822caf5b4b17c68a76da52af8):
//
//   bash imgcat-official.sh -l -W 4 -H 2 iterm2-fixture.png
//   (Ubuntu 26.04 uutils base64 wraps its 76-column output; the fixture PNG is
//   48x24, SHA-256 3d6a133dd26429d40b4acb1c72784afa65642a16e6b38c91a6e20320465f041e)
//
// The capture is real protocol output, not a synthetic daemon event: it keeps
// the producer's base64-wrapped lines, absolute-file `name=`, BEL terminator
// and trailing newline. This is headless consumer evidence, not a native
// window, WSL or SSH result.
const ITERM2_IMGCAT_LEGACY_W4H2: &[u8] = b"\x1b]1337;File=inline=1;size=126;name=L21udC9jL1VzZXJzL2RsaXZ4L0FwcERhdGEvTG9jYWwvVGVtcC9seXJlbC1zcGVjMjU4L3d0LTI3\nNi9pdGVybTItZml4dHVyZS5wbmc=;width=4;height=2:iVBORw0KGgoAAAANSUhEUgAAADAAAAAYCAYAAAC8/X7cAAAARUlEQVR4nO3PsQ0AIAwDQW9AxQDZ\nf0jTAxGVG/SWvvfJKieTZzgAAAAAAAAAwKVtALqG7ETnFAnAG5I5DgAAAAAAAHwBWKJIfucQ1079\nAAAAAElFTkSuQmCC\x07\n";

#[gpui_kit::test]
fn iterm2_production_consumer_paints_official_imgcat_pixels_cursor_and_text(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    let mut bytes = b"\x1b[2J\x1b[6;5H".to_vec();
    bytes.extend_from_slice(ITERM2_IMGCAT_LEGACY_W4H2);
    bytes.extend_from_slice(b"AFTER_IT_276");
    graphics_send_fixture(&view, cx, 1, &bytes);
    graphics_recovery_wait(
        cx,
        "recorded imgcat iTerm2 image decoded by the production consumer",
        |cx| {
            view.read_with(cx, |view, _| {
                let images = graphics_visible_images(view);
                images.len() == 1 && images[0].data.is_some()
            })
        },
    );
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let image = &images[0];
        assert_eq!(
            (image.anchor_col, image.anchor_row),
            (4, 5),
            "the image anchors at the first row after `CSI 6;5H`"
        );
        assert_eq!(
            (image.placement.cols, image.placement.rows),
            (4, 2),
            "the producer's requested cell box stays the source placement box"
        );
        assert_eq!(
            (image.placement.span_cols, image.placement.span_rows),
            (4, 1),
            "preserveAspectRatio resolves the 48x24 image to 40x20 device pixels"
        );
        assert_eq!(
            image.placement.z, -1,
            "iTerm2 pixels stay under following glyphs, like Sixel"
        );
        let data = image
            .data
            .as_ref()
            .expect("decoded RGBA")
            .as_bytes(0)
            .expect("frame 0");
        assert_eq!(
            &data[..4],
            &[33, 0, 255, 255],
            "the official fixture's top-left pixel reaches the paint RGBA buffer"
        );
        assert!(image.painted.load(std::sync::atomic::Ordering::Acquire));
    });
    // The daemon resolved the protocol's cell/pixel box; the paint must use
    // those exact device pixels instead of refitting the image to rounded
    // cell spans.
    let crop = view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        images[0]
            .paint_bounds(
                gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)),
                gpui_kit::px(10.),
                gpui_kit::px(20.),
                0,
                0,
                1.,
            )
            .0
    });
    assert_eq!(
        (
            crop.origin.x,
            crop.origin.y,
            crop.size.width,
            crop.size.height
        ),
        (
            gpui_kit::px(40.),
            gpui_kit::px(100.),
            gpui_kit::px(40.),
            gpui_kit::px(20.)
        ),
        "40x20 at the second-pane row under 10x20 metrics"
    );
    view.read_with(cx, |view, _| {
        let text = sixel_screen_text(view);
        assert!(
            !text.contains("File=inline=1") && !text.contains("iVBOR"),
            "the protocol payload never enters terminal text: {text:?}"
        );
        assert!(text.contains("AFTER_IT_276"));
        let row = sixel_screen_row(view, "AFTER_IT_276").expect("following text row");
        assert_eq!(
            row, 6,
            "the producer's trailing newline leaves the last image row"
        );
        let line = sync_presented_row(view, row);
        assert!(
            line[8..].starts_with("AFTER_IT_276"),
            "iTerm2 leaves the cursor right of the image: {line:?}"
        );
    });
    graphics_recovery_destroy(view, window, cx);
    daemon.panes().kill(pane).unwrap();
}

const ITERM2_FIXTURE_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAADAAAAAYCAYAAAC8/X7cAAAARUlEQVR4nO3PsQ0AIAwDQW9AxQDZf0jTAxGVG/SWvvfJKieTZzgAAAAAAAAAwKVtALqG7ETnFAnAG5I5DgAAAAAAAHwBWKJIfucQ1079AAAAAElFTkSuQmCC";

fn iterm2_fixture_png() -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(ITERM2_FIXTURE_PNG_BASE64)
        .unwrap()
}

fn iterm2_jpeg() -> Vec<u8> {
    let rgb = image::RgbImage::from_fn(2, 1, |_, _| image::Rgb([255, 0, 0]));
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 95)
        .encode_image(&rgb)
        .unwrap();
    bytes
}

fn iterm2_file(parameters: &str, payload: &[u8]) -> Vec<u8> {
    use base64::Engine as _;
    let mut bytes = format!("\x1b]1337;File=inline=1;{parameters}:").into_bytes();
    bytes.extend_from_slice(base64::engine::general_purpose::STANDARD.encode(payload).as_bytes());
    bytes.push(0x07);
    bytes
}

fn iterm2_paint_size(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    scale: f32,
) -> (f32, f32) {
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let (crop, _) = images[0].paint_bounds(
            gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)),
            gpui_kit::px(10.),
            gpui_kit::px(20.),
            0,
            0,
            scale,
        );
        (crop.size.width.as_f32(), crop.size.height.as_f32())
    })
}

#[gpui_kit::test]
fn iterm2_static_jpeg_is_normalized_in_ordered_preparation_and_painted(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    let mut bytes = b"\x1b[2J\x1b[6;5H".to_vec();
    bytes.extend_from_slice(&iterm2_file("width=2;height=1", &iterm2_jpeg()));
    bytes.extend_from_slice(b"AFTER_JPEG");
    graphics_send_fixture(&view, cx, 2, &bytes);
    graphics_recovery_wait(cx, "static JPEG normalized by ordered daemon preparation", |cx| {
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            images.len() == 1
                && images[0].data.is_some()
                && (images[0].placement.display_width, images[0].placement.display_height)
                    == (20, 10)
        })
    });
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let image = &images[0];
        assert_eq!(
            (image.placement.span_cols, image.placement.span_rows),
            (2, 1),
            "preserveAspectRatio fits the 2x1 JPEG into the requested 2x1 cell box"
        );
        let data = image
            .data
            .as_ref()
            .expect("decoded RGBA")
            .as_bytes(0)
            .expect("frame 0");
        assert!(
            data[2] > 180 && data[0] < 80 && data[1] < 80,
            "the red JPEG reaches the BGRA paint buffer: {data:?}"
        );
        let line = sync_presented_row(view, 5);
        assert!(
            line[6..].starts_with("AFTER_JPEG"),
            "iTerm2 cursor sits right of the 20x10 image: {line:?}"
        );
    });
    assert_eq!(iterm2_paint_size(&view, cx, 1.), (20., 10.));
    graphics_recovery_destroy(view, window, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn iterm2_sizing_modes_use_released_dimensions_and_exact_paint_pixels(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    let png = iterm2_fixture_png();
    for (serial, parameters, expected) in [
        (10usize, "width=auto;height=auto", (48u32, 24u32, 5u32, 2u32, 0u32, 0u32)),
        (11, "width=4;height=2", (40, 20, 4, 1, 4, 2)),
        (12, "width=100px;height=24px", (48, 24, 5, 2, 0, 0)),
        (13, "width=50%;height=auto", (400, 200, 40, 10, 0, 0)),
        (14, "width=8;height=3;preserveAspectRatio=0", (80, 60, 8, 3, 8, 3)),
    ] {
        let mut bytes = b"\x1b[2J\x1b[6;5H".to_vec();
        bytes.extend_from_slice(&iterm2_file(parameters, &png));
        bytes.extend_from_slice(b"T");
        graphics_send_fixture(&view, cx, serial, &bytes);
        graphics_recovery_wait(cx, "iTerm2 sizing mode reaches paint geometry", |cx| {
            view.read_with(cx, |view, _| {
                let images = graphics_visible_images(view);
                images.len() == 1
                    && images[0].data.is_some()
                    && (
                        images[0].placement.display_width,
                        images[0].placement.display_height,
                    ) == (expected.0, expected.1)
            })
        });
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            let image = &images[0];
            assert_eq!(
                (
                    image.placement.display_width,
                    image.placement.display_height,
                    image.placement.span_cols,
                    image.placement.span_rows,
                    image.placement.cols,
                    image.placement.rows,
                ),
                expected,
                "parameters {parameters}"
            );
        });
        let size = iterm2_paint_size(&view, cx, 1.);
        assert_eq!(
            (size.0 as u32, size.1 as u32),
            (expected.0, expected.1),
            "paint uses the exact resolved device pixels for {parameters}"
        );
        graphics_assert_production_paint(window, &view, cx);
    }
    graphics_recovery_destroy(view, window, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn iterm2_refusals_keep_the_accepted_image_cursor_and_following_text(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    let png = iterm2_fixture_png();
    let mut bytes = b"\x1b[2J\x1b[6;5H".to_vec();
    bytes.extend_from_slice(&iterm2_file("width=4;height=2", &png));
    bytes.extend_from_slice(b"BASE");
    graphics_send_fixture(&view, cx, 20, &bytes);
    graphics_recovery_wait(cx, "accepted iTerm2 image decoded", |cx| {
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            images.len() == 1 && images[0].data.is_some()
        })
    });
    let key = view.read_with(cx, |view, _| graphics_visible_images(view)[0].placement.key);
    let mut refusals = Vec::new();
    refusals.extend_from_slice(
        b"\x1b]1337;File=inline=0;size=126;name=Zm9v:AAAA\x07",
    );
    refusals.extend_from_slice(
        b"\x1b]1337;MultipartFile=inline=1;size=126;name=Zm9v\x07\
         \x1b]1337;FilePart=aGVsbG8=\x07\x1b]1337;FileEnd\x07",
    );
    refusals.extend_from_slice(
        b"\x1b]1337;File=inline=1;name=Zm9v:\xff\xd8\xff\xe0\x00\x10JFIF\x00garbage\x07",
    );
    refusals.extend_from_slice(b"\x1b]1337;File=inline=1;name=Zm9v:R0lGODlhAQABAAAAACw=\x07");
    refusals.extend_from_slice(b"\x1b]1337;File=inline=1;name=Zm9v:JVBERi0xLjcK\x07");
    let mut oversized = b"\x1b]1337;File=inline=1;".to_vec();
    oversized.extend(std::iter::repeat_n(b'x', 9000));
    oversized.extend_from_slice(b":AAAA\x07");
    refusals.extend_from_slice(&oversized);
    refusals.extend_from_slice(b"TAIL");
    graphics_send_fixture(&view, cx, 21, &refusals);
    graphics_recovery_wait(cx, "refusals leave the accepted image and text", |cx| {
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            images.len() == 1
                && images[0].placement.key == key
                && sync_presented_row(view, 5).contains("BASETAIL")
        })
    });
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].placement.key, key);
        let row = sync_presented_row(view, 5);
        assert!(
            row[8..].starts_with("BASETAIL"),
            "refusals must not move the cursor or replace pixels: {row:?}"
        );
        assert!(
            !sixel_screen_text(view).contains("File=inline=1")
                && !sixel_screen_text(view).contains("MultipartFile")
        );
    });
    graphics_assert_production_paint(window, &view, cx);
    graphics_recovery_destroy(view, window, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn iterm2_synchronized_frame_and_alternate_screen_keep_ordered_presentation(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    let png = iterm2_fixture_png();
    let mut bytes = b"\x1b[2J\x1b[?2026h\x1b[6;5H".to_vec();
    bytes.extend_from_slice(&iterm2_file("width=auto;height=auto", &png));
    bytes.extend_from_slice(b"INSYNC\x1b[?2026l");
    graphics_send_fixture(&view, cx, 30, &bytes);
    graphics_recovery_wait(cx, "synchronized iTerm2 frame commits image and text", |cx| {
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            images.len() == 1
                && images[0].data.is_some()
                && sync_presented_row(view, 6)[9..].starts_with("INSYNC")
        })
    });
    graphics_assert_production_paint(window, &view, cx);
    graphics_send_fixture(&view, cx, 31, b"\x1b[?1049h\x1b[2J");
    assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    graphics_send_fixture(&view, cx, 32, b"\x1b[?1049l");
    graphics_assert_production_paint(window, &view, cx);
    graphics_recovery_wait(cx, "main screen iTerm2 image returns after alternate screen", |cx| {
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            images.len() == 1 && images[0].data.is_some()
        })
    });
    graphics_send_fixture(&view, cx, 33, b"\x1b[2J");
    assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    graphics_recovery_destroy(view, window, cx);
    daemon.panes().kill(pane).unwrap();
}
