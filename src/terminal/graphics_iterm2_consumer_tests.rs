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

/// The daemon reports the measured cell size in device pixels; the producer's
/// protocol box is resolved against that, not the attach-only fixture size.
fn iterm2_cell_metrics(
    view: &Entity<TerminalView>,
    window: gpui_kit::AnyWindowHandle,
    cx: &mut TestAppContext,
) -> (u32, u32, f32) {
    let scale = window
        .update(cx, |_, window, _| window.scale_factor())
        .unwrap();
    view.read_with(cx, |view, _| {
        let geom = view.cell_geom.expect("rooted frame geometry");
        (
            (geom.cell_width.as_f32() * scale).round().max(1.0) as u32,
            (geom.line_height.as_f32() * scale).round().max(1.0) as u32,
            scale,
        )
    })
}

/// Protocol box fit, independent of the daemon's implementation: the expected
/// value comes from the iTerm2 width/height grammar and aspect policy.
fn iterm2_fit(
    image_w: u32,
    image_h: u32,
    box_w: u32,
    box_h: u32,
    preserve_aspect_ratio: bool,
) -> (u32, u32) {
    if !preserve_aspect_ratio {
        return (box_w, box_h);
    }
    let scale = (box_w as f64 / image_w as f64).min(box_h as f64 / image_h as f64);
    (
        (image_w as f64 * scale).round() as u32,
        (image_h as f64 * scale).round() as u32,
    )
}

fn iterm2_span(display: u32, cell: u32) -> u32 {
    display.div_ceil(cell).max(1)
}

fn iterm2_paint_size(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    cell_w: u32,
    cell_h: u32,
    scale: f32,
) -> (u32, u32) {
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let (crop, _) = images[0].paint_bounds(
            gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)),
            gpui_kit::px(cell_w as f32 / scale),
            gpui_kit::px(cell_h as f32 / scale),
            0,
            0,
            scale,
        );
        (
            (crop.size.width.as_f32() * scale).round() as u32,
            (crop.size.height.as_f32() * scale).round() as u32,
        )
    })
}

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
    let (cell_w, cell_h, scale) = iterm2_cell_metrics(&view, window, cx);
    let (display_w, display_h) = iterm2_fit(48, 24, 4 * cell_w, 2 * cell_h, true);
    let span_cols = iterm2_span(display_w, cell_w);
    let span_rows = iterm2_span(display_h, cell_h);
    let mut bytes = b"\x1b[2J\x1b[6;5H".to_vec();
    bytes.extend_from_slice(ITERM2_IMGCAT_LEGACY_W4H2);
    bytes.extend_from_slice(b"AFTER_IT_276");
    graphics_send_fixture(&view, cx, 1, &bytes);
    // Consumer-visible cursor/text placement is available before pixel decode:
    // the real imgcat capture ends with LF, so the following text is exactly
    // span_rows rows below the anchor. Without the adapter no image rows advance
    // the cursor, so this assertion is sensitive to a missing adapter.
    view.read_with(cx, |view, _| {
        let row = sixel_screen_row(view, "AFTER_IT_276")
            .expect("following text visible after the PTY fixture barrier");
        assert_eq!(
            row,
            5 + span_rows as usize,
            "iTerm2 advanced the cursor through its rows and the producer's LF"
        );
        assert!(
            sync_presented_row(view, row).contains("AFTER_IT_276"),
            "following text remains visible"
        );
    });
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
    let origin = view.read_with(cx, |view, _| {
        let term = view.terminal.term.lock();
        term.grid.lines_evicted() as i64 + term.grid.history_size() as i64
    });
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let image = &images[0];
        assert_eq!(
            (image.anchor_col, image.anchor_row - origin),
            (4, 5),
            "the image anchors at the first screen row after `CSI 6;5H`"
        );
        assert_eq!(
            (image.placement.display_width, image.placement.display_height),
            (display_w, display_h),
            "the daemon resolves the cell box against measured metrics"
        );
        assert_eq!(
            (image.placement.span_cols, image.placement.span_rows),
            (span_cols, span_rows),
            "the occupied span follows the resolved display pixels"
        );
        assert_eq!(
            (image.placement.cols, image.placement.rows),
            (4, 2),
            "the producer's requested cell box is retained"
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
    assert_eq!(
        iterm2_paint_size(&view, cx, cell_w, cell_h, scale),
        (display_w, display_h),
        "paint uses the exact resolved device pixels"
    );
    view.read_with(cx, |view, _| {
        let text = sixel_screen_text(view);
        assert!(
            !text.contains("File=inline=1") && !text.contains("iVBOR"),
            "the protocol payload never enters terminal text: {text:?}"
        );
        assert!(text.contains("AFTER_IT_276"));
    });
    graphics_recovery_destroy(view, window, cx);
    daemon.panes().kill(pane).unwrap();
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
    let (cell_w, cell_h, scale) = iterm2_cell_metrics(&view, window, cx);
    let (display_w, display_h) = iterm2_fit(2, 1, 2 * cell_w, cell_h, true);
    let span_cols = iterm2_span(display_w, cell_w);
    let span_rows = iterm2_span(display_h, cell_h);
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
                    == (display_w, display_h)
        })
    });
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let image = &images[0];
        assert_eq!(
            (image.placement.span_cols, image.placement.span_rows),
            (span_cols, span_rows),
            "JPEG is normalized before placement sizing"
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
        let row = 5 + span_rows as usize - 1;
        let line = sync_presented_row(view, row);
        assert!(
            line[4 + span_cols as usize..].starts_with("AFTER_JPEG"),
            "iTerm2 cursor sits right of the normalized JPEG: {line:?}"
        );
    });
    assert_eq!(
        iterm2_paint_size(&view, cx, cell_w, cell_h, scale),
        (display_w, display_h)
    );
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
    let (cell_w, cell_h, scale) = iterm2_cell_metrics(&view, window, cx);
    let png = iterm2_fixture_png();
    let natural = iterm2_fit(48, 24, 48, 24, true);
    let cells = iterm2_fit(48, 24, 4 * cell_w, 2 * cell_h, true);
    let pixels = iterm2_fit(48, 24, 100, 24, true);
    let percent = iterm2_fit(48, 24, 40 * cell_w, 20 * cell_w, true);
    let stretch = (8 * cell_w, 3 * cell_h);
    for (serial, parameters, (display_w, display_h), (raw_cols, raw_rows)) in [
        (10usize, "width=auto;height=auto", natural, (0u32, 0u32)),
        (11, "width=4;height=2", cells, (4, 2)),
        (12, "width=100px;height=24px", pixels, (0, 0)),
        (13, "width=50%;height=auto", percent, (0, 0)),
        (
            14,
            "width=8;height=3;preserveAspectRatio=0",
            stretch,
            (8, 3),
        ),
    ] {
        let span_cols = iterm2_span(display_w, cell_w);
        let span_rows = iterm2_span(display_h, cell_h);
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
                    ) == (display_w, display_h)
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
                (display_w, display_h, span_cols, span_rows, raw_cols, raw_rows),
                "parameters {parameters}"
            );
        });
        assert_eq!(
            iterm2_paint_size(&view, cx, cell_w, cell_h, scale),
            (display_w, display_h),
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
    let (cell_w, cell_h, _) = iterm2_cell_metrics(&view, window, cx);
    let png = iterm2_fixture_png();
    let (display_w, display_h) = iterm2_fit(48, 24, 4 * cell_w, 2 * cell_h, true);
    let span_cols = iterm2_span(display_w, cell_w);
    let span_rows = iterm2_span(display_h, cell_h);
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
    refusals.extend_from_slice(b"\x1b]1337;File=inline=0;size=126;name=Zm9v:AAAA\x07");
    refusals.extend_from_slice(
        b"\x1b]1337;MultipartFile=inline=1;size=126;name=Zm9v\x07\
         \x1b]1337;FilePart=aGVsbG8=\x07\x1b]1337;FileEnd\x07",
    );
    // Invalid JPEG magic is valid base64 on the wire; do not write raw NUL/0xff
    // bytes into the PTY input line.
    refusals.extend_from_slice(b"\x1b]1337;File=inline=1;name=Zm9v:/9j/4A==\x07");
    refusals.extend_from_slice(b"\x1b]1337;File=inline=1;name=Zm9v:R0lGODlhAQABAAAAACw=\x07");
    refusals.extend_from_slice(b"\x1b]1337;File=inline=1;name=Zm9v:JVBERi0xLjcK\x07");
    refusals.extend_from_slice(b"TAIL");
    graphics_send_fixture(&view, cx, 21, &refusals);
    let row = 5 + span_rows as usize - 1;
    let col = 4 + span_cols as usize;
    graphics_recovery_wait(cx, "refusals leave the accepted image and text", |cx| {
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            images.len() == 1
                && images[0].placement.key == key
                && sync_presented_row(view, row).contains("BASETAIL")
        })
    });
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].placement.key, key);
        let line = sync_presented_row(view, row);
        assert!(
            line[col..].starts_with("BASETAIL"),
            "refusals must not move the cursor or replace pixels: {line:?}"
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
    let (cell_w, cell_h, _) = iterm2_cell_metrics(&view, window, cx);
    let png = iterm2_fixture_png();
    let (display_w, display_h) = iterm2_fit(48, 24, 48, 24, true);
    let span_cols = iterm2_span(display_w, cell_w);
    let span_rows = iterm2_span(display_h, cell_h);
    let mut bytes = b"\x1b[2J\x1b[?2026h\x1b[6;5H".to_vec();
    bytes.extend_from_slice(&iterm2_file("width=auto;height=auto", &png));
    bytes.extend_from_slice(b"INSYNC\x1b[?2026l");
    graphics_send_fixture(&view, cx, 30, &bytes);
    let row = 5 + span_rows as usize - 1;
    let col = 4 + span_cols as usize;
    graphics_recovery_wait(cx, "synchronized iTerm2 frame commits image and text", |cx| {
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            images.len() == 1
                && images[0].data.is_some()
                && sync_presented_row(view, row)[col..].starts_with("INSYNC")
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
