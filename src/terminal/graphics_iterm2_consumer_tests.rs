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

// #277: the second recorded producer is the official script's own
// `imgcat -l -n -W 40px -H 20px` output (336 bytes, SHA-256
// dc577e1046824a01f6222a29f8a3567b8ff0a199d292b8b27535175a66613ae9), kept as
// the literal bytes so the recovery test re-checks the recorded hash before
// asserting fresh-consumer restoration without retransmission.
const ITERM2_IMGCAT_OFFICIAL_40X20: &[u8] = b"\x1b]1337;File=inline=1;size=126;name=L21udC9jL1VzZXJzL2RsaXZ4L0FwcERhdGEvTG9jYWwvVGVtcC9seXJlbC1zcGVjMjU4L3d0LTI3\nNi9pdGVybTItZml4dHVyZS5wbmc=;width=40px;height=20px:iVBORw0KGgoAAAANSUhEUgAAADAAAAAYCAYAAAC8/X7cAAAARUlEQVR4nO3PsQ0AIAwDQW9AxQDZ\nf0jTAxGVG/SWvvfJKieTZzgAAAAAAAAAwKVtALqG7ETnFAnAG5I5DgAAAAAAAHwBWKJIfucQ1079\nAAAAAElFTkSuQmCC\x07\n";
const ITERM2_IMGCAT_OFFICIAL_40X20_SHA256: &str =
    "dc577e1046824a01f6222a29f8a3567b8ff0a199d292b8b27535175a66613ae9";

fn iterm2_send_fragmented(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    serial: usize,
    bytes: &[u8],
    fragment: usize,
) {
    let request = format!(
        "SEND {serial} 0 {} {fragment}\n",
        graphics_pty_fixture::hex(bytes)
    );
    let input_bytes = request.len();
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    graphics_wait_fixture(view, cx, serial, input_bytes);
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
    bytes.extend_from_slice(
        base64::engine::general_purpose::STANDARD
            .encode(payload)
            .as_bytes(),
    );
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
            (
                image.placement.display_width,
                image.placement.display_height
            ),
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
fn iterm2_static_jpeg_is_normalized_in_ordered_preparation_and_painted(cx: &mut TestAppContext) {
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
    graphics_recovery_wait(
        cx,
        "static JPEG normalized by ordered daemon preparation",
        |cx| {
            view.read_with(cx, |view, _| {
                let images = graphics_visible_images(view);
                images.len() == 1
                    && images[0].data.is_some()
                    && (
                        images[0].placement.display_width,
                        images[0].placement.display_height,
                    ) == (display_w, display_h)
            })
        },
    );
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
fn iterm2_sizing_modes_use_released_dimensions_and_exact_paint_pixels(cx: &mut TestAppContext) {
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
                (
                    display_w, display_h, span_cols, span_rows, raw_cols, raw_rows
                ),
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
fn iterm2_refusals_keep_the_accepted_image_cursor_and_following_text(cx: &mut TestAppContext) {
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
    graphics_recovery_wait(
        cx,
        "synchronized iTerm2 frame commits image and text",
        |cx| {
            view.read_with(cx, |view, _| {
                let images = graphics_visible_images(view);
                images.len() == 1
                    && images[0].data.is_some()
                    && sync_presented_row(view, row)[col..].starts_with("INSYNC")
            })
        },
    );
    graphics_assert_production_paint(window, &view, cx);
    graphics_send_fixture(&view, cx, 31, b"\x1b[?1049h\x1b[2J");
    assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    graphics_send_fixture(&view, cx, 32, b"\x1b[?1049l");
    graphics_assert_production_paint(window, &view, cx);
    graphics_recovery_wait(
        cx,
        "main screen iTerm2 image returns after alternate screen",
        |cx| {
            view.read_with(cx, |view, _| {
                let images = graphics_visible_images(view);
                images.len() == 1 && images[0].data.is_some()
            })
        },
    );
    graphics_send_fixture(&view, cx, 33, b"\x1b[2J");
    assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    graphics_recovery_destroy(view, window, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn iterm2_fresh_consumer_restores_the_accepted_image_after_the_ring_trims(cx: &mut TestAppContext) {
    use sha2::Digest as _;
    // The recorded producer bytes are bound to the official script's real
    // output before they drive any assertion.
    let mut digest = String::with_capacity(64);
    for byte in sha2::Sha256::digest(ITERM2_IMGCAT_OFFICIAL_40X20) {
        use std::fmt::Write as _;
        write!(&mut digest, "{byte:02x}").unwrap();
    }
    assert_eq!(
        digest, ITERM2_IMGCAT_OFFICIAL_40X20_SHA256,
        "recorded official imgcat output hash drifted"
    );
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (prior, prior_window) = graphics_recovery_fresh(&daemon, pane, cx, 700);
    let mut bytes = b"\x1b[2J\x1b[3J\x1b[6;1HEARLY_ITERM2\r\n".to_vec();
    bytes.extend_from_slice(ITERM2_IMGCAT_OFFICIAL_40X20);
    bytes.extend_from_slice(b"AFTER_EARLY");
    graphics_send_fixture(&prior, cx, 701, &bytes);
    graphics_recovery_wait(
        cx,
        "early iTerm2 decoded by the production consumer",
        |cx| {
            prior.read_with(cx, |view, _| {
                graphics_visible_images(view)
                    .iter()
                    .any(|image| image.placement.z == -1 && image.data.is_some())
            })
        },
    );
    // The OSC body is extracted before the text ring, so the ring never holds
    // `File=` bytes; this load then trims the early text row the way any long
    // output trims older bytes.
    let path = std::env::temp_dir().join(format!("lyrel-iterm2-ring-{}.txt", std::process::id()));
    let line = sixel_ring_line();
    let mut file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    use std::io::Write as _;
    for _ in 0..9_200 {
        file.write_all(&line).unwrap();
    }
    file.flush().unwrap();
    drop(file);
    sixel_send_file(&prior, cx, 702, &path);
    // A second, still-replayable image placed after the trim: its text row is
    // inside the retained tail, so the fresh consumer must show it.
    let mut late = b"\x1b[8;1HLATE_ITERM2\r\n".to_vec();
    late.extend_from_slice(ITERM2_IMGCAT_OFFICIAL_40X20);
    late.extend_from_slice(b"AFTER_LATE");
    graphics_send_fixture(&prior, cx, 703, &late);
    graphics_recovery_wait(cx, "late iTerm2 decoded before detach", |cx| {
        prior.read_with(cx, |view, _| {
            graphics_visible_images(view)
                .iter()
                .filter(|image| image.placement.z == -1)
                .count()
                == 2
        })
    });
    // The producer now waits for its next request: it produced its bytes once.
    let stopped = graphics_recovery_stats(&prior, cx, 704);
    assert!(
        stopped
            .split_whitespace()
            .find_map(|field| field.strip_prefix("bytes="))
            .unwrap()
            .parse::<usize>()
            .unwrap()
            > 8 * 1024 * 1024,
        "the real sender must have written more than the production ring: {stopped}"
    );
    assert!(
        !prior.read_with(cx, |view, _| sixel_screen_text(view)
            .contains("EARLY_ITERM2")),
        "the ring trimmed the early text row"
    );
    graphics_recovery_destroy(prior, prior_window, cx);
    // Observe the actual replay bytes: the OSC body must not ride the ring,
    // and the tail must be the real >8 MiB ring, not a synthetic snapshot.
    let mut replay = daemon
        .panes()
        .observe(
            pane,
            crate::daemon::protocol::WinSize {
                cols: 80,
                rows: 24,
                cell_w: 10,
                cell_h: 20,
            },
        )
        .unwrap();
    replay
        .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut raw = Vec::new();
    let mut ring_bytes = 0;
    let mut ring_geometry_seen = false;
    loop {
        match replay.recv() {
            Ok(DaemonMsg::Size(_)) => ring_geometry_seen = true,
            Ok(DaemonMsg::Snapshot(bytes)) => {
                if ring_geometry_seen {
                    ring_bytes += bytes.len();
                }
                raw.extend(bytes);
            }
            Ok(DaemonMsg::Output(bytes)) => raw.extend(bytes),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("replay observer: {error}"),
        }
        if raw.windows(10).any(|part| part == b"AFTER_LATE") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "replay tail never reached the late marker"
        );
    }
    replay.detach().unwrap();
    // Replay sends evicted terminal modes before the ring's first Size, then
    // Size/Snapshot pairs for the actual bounded ring. Count only those
    // snapshots: a mode prelude or later live Output is not ring storage.
    // Boundary trimming inside the uniform SGR filler costs at most a few
    // bytes; the sender counter above independently proves saturation.
    let ring = 8 * 1024 * 1024;
    assert!(
        ring_bytes <= ring && ring_bytes >= ring - 8,
        "the replay is the real bounded ring tail: {ring_bytes} bytes"
    );
    assert!(
        !raw.windows(8).any(|part| part == b"\x1b]1337;"),
        "the producer's OSC never rides the text replay"
    );
    let (fresh, fresh_window) = graphics_recovery_fresh(&daemon, pane, cx, 705);
    fresh.read_with(cx, |view, _| {
        assert_eq!(view.pane_id, pane);
        let text = sixel_screen_text(view);
        assert!(!text.contains("EARLY_ITERM2"), "trimmed text cannot return");
        assert!(text.contains("LATE_ITERM2"), "retained text must replay");
        assert!(text.contains("AFTER_LATE"));
    });
    graphics_recovery_wait(cx, "restored iTerm2 paints without retransmission", |cx| {
        fresh.read_with(cx, |view, _| {
            graphics_visible_images(view).iter().any(|image| {
                image.placement.z == -1 && image.painted.load(std::sync::atomic::Ordering::Acquire)
            })
        })
    });
    graphics_assert_production_paint(fresh_window, &fresh, cx);
    fresh.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let painted: Vec<_> = images
            .iter()
            .filter(|image| {
                image.placement.z == -1 && image.painted.load(std::sync::atomic::Ordering::Acquire)
            })
            .collect();
        assert_eq!(
            painted.len(),
            1,
            "the trimmed early placement cannot paint; the retained one does"
        );
        let restored = painted[0];
        let rgba = restored.data.as_ref().unwrap().as_bytes(0).unwrap();
        assert_eq!(
            &rgba[..4],
            &[33, 0, 255, 255],
            "actual producer pixels reach the fresh consumer's BGRA buffer"
        );
        assert_eq!(
            (
                restored.placement.display_width,
                restored.placement.display_height
            ),
            (40, 20),
            "the restored placement keeps the producer's requested display pixels"
        );
        let row = sixel_screen_row(view, "LATE_ITERM2").expect("late text row");
        let (crop, _) = restored.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 1.);
        assert_eq!(
            crop.origin.y,
            px((row + 1) as f32 * 20.),
            "the restored image shares the row below LATE_ITERM2"
        );
    });
    // STAT emits no image bytes: the same sender counters prove the fresh
    // consumer restored authority instead of re-running the producer.
    assert_eq!(graphics_recovery_stats(&fresh, cx, 706), stopped);
    graphics_recovery_destroy(fresh, fresh_window, cx);
    std::fs::remove_file(&path).unwrap();
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn iterm2_do_not_move_cursor_and_scrolling_preserve_placement(cx: &mut TestAppContext) {
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
    // doNotMoveCursor=1: the producer's extension restores the pre-image
    // cursor, so text written next shares the anchor row and column.
    let mut bytes = b"\x1b[2J\x1b[6;5H".to_vec();
    bytes.extend_from_slice(&iterm2_file("width=4;height=2;doNotMoveCursor=1", &png));
    bytes.extend_from_slice(b"NOMOVE");
    graphics_send_fixture(&view, cx, 710, &bytes);
    graphics_recovery_wait(cx, "doNotMoveCursor image decoded and anchored", |cx| {
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            images.len() == 1
                && images[0].data.is_some()
                && sync_presented_row(view, 5)[4..].starts_with("NOMOVE")
        })
    });
    // An image at the bottom of a full-height scrolling region must scroll the
    // text grid exactly once and keep its anchor on the moved content.
    graphics_send_fixture(&view, cx, 711, b"\x1b[1;24r\x1b[24;1HSCROLL_ANCHOR");
    let mut scroll = iterm2_file("width=4;height=2", &png);
    scroll.extend_from_slice(b"BELOW_SCROLLED");
    graphics_send_fixture(&view, cx, 712, &scroll);
    graphics_recovery_wait(cx, "scrolled iTerm2 image decoded and painted", |cx| {
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            images.len() == 2
                && images.iter().all(|image| image.data.is_some())
                && sixel_screen_row(view, "SCROLL_ANCHOR").is_some()
                && sync_presented_row(view, 23)[17..].starts_with("BELOW_SCROLLED")
        })
    });
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let scrolled = images
            .iter()
            .find(|image| image.placement.z == -1 && image.anchor_col == 13)
            .expect("the scrolled placement stays visible at its anchor column");
        let text_row =
            sixel_screen_row(view, "SCROLL_ANCHOR").expect("the scrolled text row stays visible");
        let (crop, _) = scrolled.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 1.);
        assert_eq!(
            crop.origin.y,
            px(text_row as f32 * 20.),
            "the image scrolled with its anchor row's content"
        );
        let last = sync_presented_row(view, 23);
        assert!(
            last[17..].starts_with("BELOW_SCROLLED"),
            "the cursor ends right of the image on the scrolled last row: {last:?}"
        );
    });
    graphics_send_fixture(&view, cx, 713, b"\x1b[r");
    graphics_assert_production_paint(window, &view, cx);
    graphics_recovery_destroy(view, window, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn iterm2_fragmented_cancelled_truncated_and_oversized_inputs_are_nondestructive(
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
    graphics_send_fixture(&view, cx, 720, &bytes);
    graphics_recovery_wait(cx, "accepted iTerm2 image decoded", |cx| {
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            images.len() == 1 && images[0].data.is_some()
        })
    });
    // A fragmented send carries the identical OSC byte stream; the second
    // image must be accepted byte-boundary-independently.
    let mut fragmented = iterm2_file("width=4;height=2", &png);
    fragmented.extend_from_slice(b"FRAG_TAIL");
    iterm2_send_fragmented(&view, cx, 721, &fragmented, 7);
    graphics_recovery_wait(cx, "fragmented iTerm2 image decoded", |cx| {
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            images.len() == 2 && images.iter().all(|image| image.data.is_some())
        })
    });
    // Hostile refusals: CAN inside the payload, an escape-interrupted body
    // whose tail is protocol bytes and plain text, a declared-dimension
    // oversize PNG, and an over-limit header. None may disturb the accepted
    // images, move the cursor, or leak protocol text into the grid.
    let png_bytes = {
        use image::ImageEncoder as _;
        let mut buffer = Vec::new();
        image::codecs::png::PngEncoder::new(&mut buffer)
            .write_image(&[0u8; 8193 * 4], 8193, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        buffer
    };
    let mut refusals = Vec::new();
    // CAN is buffered inside the OSC body rather than terminating it; the
    // malformed payload is refused whole and only the plain text prints.
    refusals.extend_from_slice(b"\x1b]1337;File=inline=1;name=Zm9v:iVBORw0K\x18\x07CANCELLED_TAIL");
    refusals.extend_from_slice(b"\x1b]1337;File=inline=1;name=Zm9v:QUJD\x1b[0mTRUNC_TAIL");
    refusals.extend_from_slice(&iterm2_file("width=4;height=2", &png_bytes));
    let mut header = b"inline=1;".to_vec();
    header.extend(std::iter::repeat_n(b'x', 9000));
    header.extend_from_slice(b":AAAA");
    refusals.extend_from_slice(b"\x1b]1337;File=");
    refusals.extend_from_slice(&header);
    refusals.push(0x07);
    refusals.extend_from_slice(b"TAIL");
    graphics_send_fixture(&view, cx, 722, &refusals);
    let row = 5 + span_rows as usize - 1;
    let col = 4 + span_cols as usize;
    graphics_recovery_wait(cx, "hostile iTerm2 inputs leave accepted state", |cx| {
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            images.len() == 2
                && sync_presented_row(view, row)[col..].starts_with("BASE")
                && sixel_screen_text(view).contains("FRAG_TAIL")
                && sixel_screen_text(view).contains("CANCELLED_TAIL")
                && sixel_screen_text(view).contains("TRUNC_TAIL")
        })
    });
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(images.len(), 2, "refusals admit no extra resource");
        let line = sync_presented_row(view, row);
        // The second ACCEPTED image legitimately advances the cursor by
        // another span_cols (BASE at col, FRAG_TAIL at col + BASE + span_cols).
        // After it, every refused body must leave the cursor untouched: tails
        // land contiguously in write order — a refusal moving the cursor as
        // if placed would shift every later position.
        let mut expected: Vec<(usize, &str)> = vec![(col as usize, "BASE")];
        let mut offset = col as usize + "BASE".len() + span_cols as usize;
        for text in ["FRAG_TAIL", "CANCELLED_TAIL", "TRUNC_TAIL", "TAIL"] {
            expected.push((offset, text));
            offset += text.len();
        }
        for (offset, text) in expected {
            assert!(
                line[offset..].starts_with(text),
                "refusals moved the cursor: expected {text} at column {offset}: {line:?}"
            );
        }
        let text = sixel_screen_text(view);
        assert!(
            !text.contains("File=inline=1") && !text.contains("iVBOR"),
            "no protocol fragment leaks into terminal text: {text:?}"
        );
    });
    graphics_assert_production_paint(window, &view, cx);
    graphics_recovery_destroy(view, window, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn iterm2_resize_keeps_the_image_on_its_text_row(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start_with(|command, _| {
        command
            .env("LYREL_ITERM277_DIAG", "1")
            .stderr(std::process::Stdio::inherit());
    });
    let pane = daemon.spawn();
    let stream = daemon.stream(pane);
    let mut controller = stream.try_clone().unwrap();
    use std::io::Write as _;
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let mut diagnostic_observer = daemon
        .panes()
        .observe(
            pane,
            crate::daemon::protocol::WinSize {
                cols: 80,
                rows: 24,
                cell_w: 10,
                cell_h: 20,
            },
        )
        .unwrap();
    diagnostic_observer
        .set_recv_timeout(Some(std::time::Duration::from_millis(1)))
        .unwrap();
    let (view, window) = graphics_rooted_consumer(cx, pane, stream);
    let mut bytes = b"\x1b[2J\x1b[H\x1b[6;1HRESIZE_ITERM2\r\n".to_vec();
    bytes.extend_from_slice(&iterm2_file("width=4;height=2", &iterm2_fixture_png()));
    bytes.extend_from_slice(b"\x1b[20;1HBELOW_ITERM2");
    // [DEBUG-iterm277] Observe the actual daemon boundary without changing
    // the producer bytes or the fixture's completion condition.
    let request = format!("SEND 730 0 {} 0\n", graphics_pty_fixture::hex(&bytes));
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    let diagnostic_deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if view.read_with(cx, |view, _| {
            view.title == "KITTY_DONE_730;" && view.scrollback_text().contains("KITTY_DONE_730;")
        }) {
            break;
        }
        match diagnostic_observer.recv() {
            Ok(message) => {
                let text = format!("{message:?}");
                eprintln!("[DEBUG-iterm277] {}", &text[..text.len().min(1536)]);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => panic!("[DEBUG-iterm277] observer failed: {error}"),
        }
        assert!(
            std::time::Instant::now() < diagnostic_deadline,
            "[DEBUG-iterm277] resize setup did not complete: {} graphics={}",
            view.read_with(cx, |view, _| view.scrollback_text()),
            view.read_with(cx, |view, _| view.terminal.images().recovery_diagnostics())
        );
        graphics_consumer_progress(cx);
    }
    diagnostic_observer.detach().unwrap();
    graphics_recovery_wait(cx, "resize setup decoded", |cx| {
        view.read_with(cx, |view, _| {
            graphics_visible_images(view)
                .iter()
                .any(|image| image.data.is_some())
        })
    });
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let image = images.iter().find(|image| image.placement.z == -1).unwrap();
        let (crop, _) = image.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 1.);
        assert_eq!(
            crop.origin.y,
            px(6. * 20.),
            "anchored on the row below RESIZE_ITERM2"
        );
    });
    graphics_assert_production_paint(window, &view, cx);
    let target = graphics_window_geometry(window, &view, cx, TermSize::new(80, 30)).0;
    let authoritative_term = view.read_with(cx, |view, _| view.terminal.term.clone());
    ClientMsg::Resize(crate::daemon::protocol::WinSize {
        cols: 80,
        rows: 30,
        cell_w: 10,
        cell_h: 20,
    })
    .encode(&mut controller)
    .unwrap();
    controller.flush().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if authoritative_term.lock().screen_lines() == 30 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "real daemon Size was not consumed"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    cx.simulate_window_resize(window, target);
    graphics_send_fixture(&view, cx, 732, b"\x1b[0m");
    view.read_with(cx, |view, _| {
        let text = sixel_screen_text(view);
        assert!(text.contains("RESIZE_ITERM2") && text.contains("BELOW_ITERM2"));
        // Growing the grid pulls scrollback rows back above the anchor, so the
        // text row is wherever the production reflow put it — the contract is
        // that the image followed it, not that the row stayed at 6.
        let text_row = sixel_screen_row(view, "RESIZE_ITERM2").expect("anchor text row");
        let below_row = sixel_screen_row(view, "BELOW_ITERM2").expect("following text row");
        assert!(
            text_row < below_row,
            "the anchor row still precedes the following text: {text}"
        );
        let images = graphics_visible_images(view);
        let image = images.iter().find(|image| image.placement.z == -1).unwrap();
        let (crop, _) = image.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 1.);
        assert_eq!(
            crop.origin.y,
            px((text_row + 1) as f32 * 20.),
            "the image still shares the row below its text after resize"
        );
    });
    graphics_assert_production_paint(window, &view, cx);
    graphics_recovery_destroy(view, window, cx);
    daemon.panes().kill(pane).unwrap();
}
