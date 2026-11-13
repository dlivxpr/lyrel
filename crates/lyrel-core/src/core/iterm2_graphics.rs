//! Static inline iTerm2 `OSC 1337;File=` admission.
//!
//! This module interprets only the bounded, direct `File=` form whose payload
//! is a static PNG. It has no filesystem, download, URL or renderer authority:
//! `name` is never decoded or resolved and the released protocol's download
//! (`inline=0`), multipart, PDF and animation facilities are rejected by the
//! framing layer before this parser runs. Sizing policy reuses the released
//! `rio-graphics` value types and `GraphicData::compute_display_dimensions`;
//! pixels and placement still belong to the existing daemon authority.
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use super::graphics_state::MAX_IMAGE_DIMENSION;
use super::kitty_graphics::{
    Action, Control, Image, MAX_IMAGE_BYTES, MAX_TRANSMISSION_BASE64, Medium, WireFormat,
};

/// Bound on the `key=value;...` section before the payload separator.
pub(crate) const MAX_HEADER_BYTES: usize = 8192;
/// Bound on the total `File=` body buffered by the OSC tokenizer: header,
/// encoded payload and a small separator allowance.
pub(crate) const MAX_FILE_BODY: usize =
    MAX_HEADER_BYTES + MAX_TRANSMISSION_BASE64 + MAX_TRANSMISSION_BASE64 / 8 + 2;
/// Bound on the base64 payload after whitespace is removed.
pub(crate) const MAX_FILE_BASE64: usize = MAX_TRANSMISSION_BASE64;

/// Whether this chunk ends with a prefix of the iTerm2 OSC opener. The
/// zero-copy graphics fast path must stay on the slow path in that case so a
/// split `ESC ] 1337;File=` is never mistaken for ordinary output.
pub(crate) fn ends_with_partial_opener(bytes: &[u8]) -> bool {
    const OPENER: &[u8] = b"\x1b]1337;";
    (1..OPENER.len()).any(|length| bytes.ends_with(&OPENER[..length]))
}

/// Parse the bytes after `File=` (parameters, `:`, base64 payload) into the
/// existing image resource/control representation. Returns a refusal reason for
/// every unsupported or malformed message; callers drop the bytes without
/// producing a placement or moving the cursor.
pub(crate) fn parse_file(mut body: Vec<u8>) -> Result<(Control, Image), &'static str> {
    if body.len() > MAX_FILE_BODY {
        return Err("ENOSPC:iTerm2 file sequence exceeds the encoded limit");
    }
    let colon = body
        .iter()
        .position(|byte| *byte == b':')
        .ok_or("EINVAL:iTerm2 file has no payload separator")?;
    if colon == 0 {
        return Err("EINVAL:iTerm2 file has no parameters");
    }
    if colon > MAX_HEADER_BYTES {
        return Err("ENOSPC:iTerm2 parameters exceed the header limit");
    }
    let header = parse_header(&body[..colon])?;
    let mut data = body.split_off(colon + 1);
    // Real producers may wrap base64 lines (the official imgcat script does on
    // systems whose base64 lacks `-w0`), so compaction is part of the format.
    let mut write = 0;
    for read in 0..data.len() {
        if !data[read].is_ascii_whitespace() {
            data[write] = data[read];
            write += 1;
        }
    }
    data.truncate(write);
    if data.is_empty() {
        return Err("EINVAL:iTerm2 file has no payload");
    }
    if data.len() > MAX_FILE_BASE64 {
        return Err("ENOSPC:iTerm2 payload exceeds the encoded limit");
    }
    if data.len() % 4 == 1 {
        return Err("EINVAL:iTerm2 base64 payload has an ambiguous length");
    }
    let padding = data.iter().rev().take(2).filter(|byte| **byte == b'=').count();
    let decoded_len = data.len() / 4 * 3 - padding;
    if decoded_len > MAX_IMAGE_BYTES {
        return Err("ENOSPC:iTerm2 decoded payload exceeds the image limit");
    }
    if data.len() % 4 != 0 {
        data.resize(data.len() + (4 - data.len() % 4), b'=');
    }
    let payload = BASE64
        .decode(&data)
        .map_err(|_| "EINVAL:iTerm2 base64 payload is malformed")?;
    if payload.len() > MAX_IMAGE_BYTES {
        return Err("ENOSPC:iTerm2 decoded payload exceeds the image limit");
    }
    let (format, width, height) = if payload.get(..8) == Some(b"\x89PNG\r\n\x1a\n") {
        if png_is_animated(&payload) {
            return Err("ENOTSUPPORTED:iTerm2 animated PNG is not supported");
        }
        (WireFormat::Png, 0, 0)
    } else if payload.get(..3) == Some(b"\xff\xd8\xff") {
        // Header-only dimension and decoded-budget admission; pixel decoding
        // happens in ordered preparation with a charged work lease.
        let (width, height) = jpeg_dimensions(&payload)?;
        (WireFormat::Jpeg, width, height)
    } else {
        return Err("ENOTSUPPORTED:iTerm2 payload is not a static PNG or JPEG");
    };
    let mut control = Control {
        iterm2: true,
        action: Action::TransmitAndDisplay,
        format,
        medium: Medium::Direct,
        quiet: 2,
        // Inline images occupy the background like Sixel; text written over
        // them must stay visible. Kitty's default overlay order is unchanged.
        z: -1,
        iterm2_width: header.width,
        iterm2_height: header.height,
        iterm2_preserve_aspect_ratio: header.preserve_aspect_ratio,
        iterm2_do_not_move: header.do_not_move,
        ..Control::default()
    };
    // Preserve a cell-box request for diagnostics and the existing placement
    // projection. Pixel and percent requests have no column count.
    if let Some(rio_graphics::ResizeParameter::Cells(cols)) = header.width {
        control.cols = cols;
    }
    if let Some(rio_graphics::ResizeParameter::Cells(rows)) = header.height {
        control.rows = rows;
    }
    Ok((
        control,
        Image {
            width,
            height,
            data: payload,
            format,
            compressed: false,
        },
    ))
}

struct Header {
    width: Option<rio_graphics::ResizeParameter>,
    height: Option<rio_graphics::ResizeParameter>,
    preserve_aspect_ratio: bool,
    do_not_move: bool,
}

fn parse_header(header: &[u8]) -> Result<Header, &'static str> {
    let mut parsed = Header {
        width: None,
        height: None,
        preserve_aspect_ratio: true,
        do_not_move: false,
    };
    let mut inline = false;
    for pair in header.split(|byte| *byte == b';') {
        if pair.is_empty() {
            continue;
        }
        let separator = pair
            .iter()
            .position(|byte| *byte == b'=')
            .ok_or("EINVAL:iTerm2 parameter has no value")?;
        let (key, value) = (&pair[..separator], &pair[separator + 1..]);
        match key {
            b"inline" => {
                if value != b"1" {
                    return Err("ENOTSUPPORTED:iTerm2 download is not supported");
                }
                inline = true;
            }
            b"width" => parsed.width = Some(parse_resize_parameter(value)?),
            b"height" => parsed.height = Some(parse_resize_parameter(value)?),
            // The released implementation treats every value except `0` as
            // preserving the aspect ratio.
            b"preserveAspectRatio" => parsed.preserve_aspect_ratio = value != b"0",
            b"doNotMoveCursor" => parsed.do_not_move = value == b"1",
            // `name` is a base64 filename hint and must never become a path;
            // `size` is an optional progress hint, and `type` is ignored in
            // favor of the payload's actual magic. Unknown keys activate
            // nothing in this implementation.
            b"name" | b"size" | b"type" => {}
            _ => {}
        }
    }
    if !inline {
        return Err("ENOTSUPPORTED:iTerm2 download is not supported");
    }
    Ok(parsed)
}

/// The released iTerm2 parser accepts `auto`, cell counts, pixels and window
/// percentages; this is the same grammar, parsed without the unbounded image
/// decoding that its `graphics` feature performs.
fn parse_resize_parameter(value: &[u8]) -> Result<rio_graphics::ResizeParameter, &'static str> {
    use rio_graphics::ResizeParameter;
    if value == b"auto" {
        return Ok(ResizeParameter::Auto);
    }
    let digits = value.iter().take_while(|byte| byte.is_ascii_digit()).count();
    if digits == 0 {
        return Err("EINVAL:iTerm2 image size has no number");
    }
    let number = std::str::from_utf8(&value[..digits])
        .ok()
        .and_then(|text| text.parse::<u32>().ok())
        .ok_or("EINVAL:iTerm2 image size is out of range")?;
    match &value[digits..] {
        b"" => Ok(ResizeParameter::Cells(number)),
        b"px" => Ok(ResizeParameter::Pixels(number)),
        b"%" => Ok(ResizeParameter::WindowPercent(number)),
        _ => Err("EINVAL:iTerm2 image size unit is unsupported"),
    }
}

/// Reject an animated PNG before the decoder ignores its animation control
/// chunk. The chunk walk is bounded by the encoded payload already admitted.
fn png_is_animated(png: &[u8]) -> bool {
    let mut offset = 8;
    while let Some(header) = png.get(offset..offset + 8) {
        let length = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        if &header[4..8] == b"acTL" {
            return true;
        }
        let Some(next) = offset
            .checked_add(12)
            .and_then(|next| next.checked_add(length))
        else {
            return true;
        };
        if next <= offset || next > png.len() {
            return false;
        }
        offset = next;
    }
    false
}

/// Read only the JPEG frame header and admit its decoded RGBA budget before any
/// pixel allocation. The decoder used in ordered preparation is the published
/// registry `image` release, not the `rio-vt` graphics feature.
fn jpeg_dimensions(payload: &[u8]) -> Result<(u32, u32), &'static str> {
    let mut reader = image::ImageReader::with_format(
        std::io::Cursor::new(payload),
        image::ImageFormat::Jpeg,
    );
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    reader.limits(limits);
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| "EINVAL:iTerm2 JPEG header is malformed")?;
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .filter(|bytes| *bytes != 0 && *bytes <= MAX_IMAGE_BYTES)
        .ok_or("ENOSPC:iTerm2 JPEG decoded dimensions exceed the image limit")?;
    if width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err("ENOSPC:iTerm2 JPEG dimensions exceed the image limit");
    }
    Ok((width, height))
}

/// Normalize a static JPEG to the raw RGBA representation the established
/// image authority already transports and paints. Runs in ordered preparation,
/// outside the pane lock, with a charged peak for the decoder and conversion.
pub(crate) fn normalize_jpeg(
    mut image: Image,
    work: &mut super::graphics_state::retention::Work,
) -> Result<Image, &'static str> {
    if image.format != WireFormat::Jpeg {
        return Ok(image);
    }
    if image.width == 0 || image.height == 0 {
        return Err("EINVAL:iTerm2 JPEG dimensions are missing");
    }
    let bytes = (image.width as usize)
        .checked_mul(image.height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .filter(|bytes| *bytes != 0 && *bytes <= MAX_IMAGE_BYTES)
        .ok_or("ENOSPC:iTerm2 JPEG decoded dimensions exceed the image limit")?;
    // RGB decode (3 bytes/pixel) and the RGBA conversion (4 bytes/pixel) can
    // coexist; two RGBA allocations cover that peak with the charged lease.
    let budget = bytes.checked_mul(2).ok_or("ENOSPC:iTerm2 JPEG budget overflow")?;
    if !work.resize(image.data.capacity() + budget) {
        return Err("ENOSPC:iTerm2 JPEG preparation budget exhausted");
    }
    let mut reader = image::ImageReader::with_format(
        std::io::Cursor::new(&image.data),
        image::ImageFormat::Jpeg,
    );
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(image.width);
    limits.max_image_height = Some(image.height);
    limits.max_alloc = Some(budget as u64);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|_| "EINVAL:Invalid JPEG data")?
        .into_rgba8();
    let (width, height) = decoded.dimensions();
    if width != image.width || height != image.height || decoded.len() != bytes {
        return Err("EINVAL:JPEG pixel payload does not match dimensions");
    }
    let rgba = decoded.into_raw();
    if rgba.len() != bytes {
        return Err("EINVAL:JPEG pixel payload does not match dimensions");
    }
    if !work.resize(image.data.capacity() + rgba.capacity()) {
        return Err("ENOSPC:iTerm2 JPEG preparation budget exhausted");
    }
    image.width = width;
    image.height = height;
    image.data = rgba;
    image.format = WireFormat::Rgba;
    image.compressed = false;
    work.resize(image.data.capacity());
    Ok(image)
}

/// Resolve the protocol's width/height box against this pane's current cell and
/// text-area metrics using the released `rio-graphics` sizing policy.
pub(crate) fn display_size(
    image_width: u32,
    image_height: u32,
    control: &Control,
    cell_width: u32,
    cell_height: u32,
    view_width: u32,
    view_height: u32,
) -> Option<(u32, u32)> {
    use rio_graphics::{ColorType, GraphicData, GraphicId, ResizeCommand, ResizeParameter};
    if image_width == 0 || image_height == 0 {
        return None;
    }
    let graphic = GraphicData {
        id: GraphicId::new(0),
        width: image_width as usize,
        height: image_height as usize,
        color_type: ColorType::Rgba,
        pixels: Vec::new(),
        is_opaque: false,
        resize: Some(ResizeCommand {
            width: control.iterm2_width.unwrap_or(ResizeParameter::Auto),
            height: control.iterm2_height.unwrap_or(ResizeParameter::Auto),
            preserve_aspect_ratio: control.iterm2_preserve_aspect_ratio,
        }),
        display_width: None,
        display_height: None,
        transmit_time: rio_graphics::time::Instant::now(),
    };
    let (width, height) = graphic.compute_display_dimensions(
        usize::try_from(cell_width.max(1)).ok()?,
        usize::try_from(cell_height.max(1)).ok()?,
        usize::try_from(view_width).ok()?,
        usize::try_from(view_height).ok()?,
    );
    let width = u32::try_from(width).ok()?;
    let height = u32::try_from(height).ok()?;
    if width == 0
        || height == 0
        || width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || width as usize > rio_graphics::MAX_GRAPHIC_DIMENSIONS[0]
        || height as usize > rio_graphics::MAX_GRAPHIC_DIMENSIONS[1]
    {
        return None;
    }
    Some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let pixels = vec![0x7f; width as usize * height as usize * 4];
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(&pixels).unwrap();
        bytes
    }

    fn file(parameters: &str, payload: &[u8]) -> Vec<u8> {
        let mut body = format!("{parameters}:").into_bytes();
        body.extend_from_slice(BASE64.encode(payload).as_bytes());
        body
    }

    fn control(width: Option<rio_graphics::ResizeParameter>, height: Option<rio_graphics::ResizeParameter>, preserve: bool) -> Control {
        Control {
            iterm2: true,
            iterm2_width: width,
            iterm2_height: height,
            iterm2_preserve_aspect_ratio: preserve,
            ..Control::default()
        }
    }

    #[test]
    fn released_sizing_policy_covers_cells_pixels_percent_auto_and_stretch() {
        use rio_graphics::ResizeParameter::{Auto, Cells, Pixels, WindowPercent};
        let size = |control: &Control| display_size(48, 24, control, 10, 20, 800, 480).unwrap();
        assert_eq!(size(&control(None, None, true)), (48, 24));
        assert_eq!(size(&control(Some(Cells(4)), Some(Cells(2)), true)), (40, 20));
        assert_eq!(size(&control(Some(Cells(8)), Some(Cells(3)), false)), (80, 60));
        assert_eq!(size(&control(Some(Pixels(100)), Some(Pixels(24)), true)), (48, 24));
        assert_eq!(size(&control(Some(WindowPercent(50)), Some(Auto), true)), (400, 200));
        assert_eq!(size(&control(Some(Cells(4)), Some(Auto), true)), (40, 20));
        assert_eq!(size(&control(Some(Auto), Some(Pixels(50)), true)), (100, 50));
        assert_eq!(size(&control(Some(Pixels(0)), Some(Auto), true)), (48, 24));
    }

    #[test]
    fn static_png_file_reuses_the_existing_resource_control() {
        let image = png(2, 1);
        let (control, parsed) =
            parse_file(file("inline=1;size=10;name=Zm9v;width=4;height=2", &image)).unwrap();
        assert!(control.iterm2);
        assert_eq!(control.action, Action::TransmitAndDisplay);
        assert_eq!(control.format, WireFormat::Png);
        assert_eq!(control.quiet, 2);
        assert_eq!(control.z, -1);
        assert_eq!((control.cols, control.rows), (4, 2));
        assert_eq!((control.iterm2_width, control.iterm2_height), (
            Some(rio_graphics::ResizeParameter::Cells(4)),
            Some(rio_graphics::ResizeParameter::Cells(2))
        ));
        assert_eq!(parsed.data, image);
        assert_eq!(parsed.format, WireFormat::Png);
    }

    #[test]
    fn refuses_downloads_other_formats_animation_and_malformed_payloads() {
        let image = png(2, 1);
        for body in [
            file("inline=0", &image),
            file("", &image),
            file("inline=1", b"%PDF-1.7\n"),
            file("inline=1", b"GIF89a\x01\x00"),
            file("inline=1", b"\xff\xd8\xff\xe0"),
            file("inline=1", b"not a png"),
            file("inline=1;width=4p", &image),
            file("inline=1;height=2%px", &image),
        ] {
            assert!(parse_file(body).is_err());
        }
        let mut animated = png(2, 1);
        let end = animated.len() - 12;
        animated.splice(end..end, [0, 0, 0, 8, b'a', b'c', b'T', b'L', 0, 0, 0, 1, 0, 0, 0, 0]);
        assert!(parse_file(file("inline=1", &animated)).is_err());
        // A declared `size` is a progress hint, not a second length authority.
        assert!(parse_file(file("inline=1;size=1", &image)).is_ok());
        // The header cap is enforced before any payload parsing; the GUI
        // refusal test keeps PTY input bounded.
        let mut oversized = b"inline=1;".to_vec();
        oversized.extend(std::iter::repeat_n(b'x', 9000));
        oversized.push(b':');
        oversized.extend_from_slice(b"AAAA");
        assert!(parse_file(oversized).is_err());
    }

    #[test]
    fn static_jpeg_is_admitted_by_header_and_normalized_to_rgba() {
        let mut jpeg = Vec::new();
        let rgb = image::RgbImage::from_fn(2, 1, |x, _| image::Rgb([x as u8 * 100, 0, 0]));
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode_image(&rgb)
            .unwrap();
        let (control, image) = parse_file(file("inline=1;width=4;height=2", &jpeg)).unwrap();
        assert_eq!(control.format, WireFormat::Jpeg);
        assert_eq!((control.cols, control.rows), (4, 2));
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.format, WireFormat::Jpeg);
        let retention = crate::core::graphics_state::retention::Retention::shared();
        let mut work = retention.work(retention.owner());
        let normalized = normalize_jpeg(image, &mut work).unwrap();
        assert_eq!(normalized.format, WireFormat::Rgba);
        assert_eq!((normalized.width, normalized.height), (2, 1));
        assert_eq!(normalized.data.len(), 2 * 1 * 4);
    }

    #[test]
    fn accepts_the_official_producers_wrapped_base64() {
        let image = png(2, 1);
        let encoded = BASE64.encode(&image);
        let wrapped = encoded
            .as_bytes()
            .chunks(16)
            .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
            .collect::<Vec<_>>()
            .join("\n");
        let mut body = b"inline=1;size=9;name=Zm9v\n".to_vec();
        body.push(b':');
        body.extend_from_slice(wrapped.as_bytes());
        let (control, parsed) = parse_file(body).unwrap();
        assert!(control.iterm2);
        assert_eq!(parsed.data, image);
    }

    #[test]
    fn file_osc_reaches_the_authority_and_foreign_osc_stays_passthrough() {
        use crate::core::kitty_graphics::{Event, GraphicsSniffer};
        let image = png(2, 1);
        let mut message = b"\x1b]0;title\x07TEXT".to_vec();
        message.extend_from_slice(b"\x1b]1337;File=");
        message.extend_from_slice(&file("inline=1", &image));
        message.push(0x07);
        message.extend_from_slice(b"TAIL");
        let mut passthrough = Vec::new();
        let events =
            GraphicsSniffer::new().feed(&message, |run| passthrough.extend_from_slice(run));
        assert_eq!(passthrough, b"\x1b]0;title\x07TEXTTAIL");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::Command { control, image } => {
                assert!(control.iterm2);
                assert_eq!(control.action, Action::TransmitAndDisplay);
                assert!(image.is_some());
            }
            other => panic!("expected an iTerm2 command, got {other:?}"),
        }
    }
}
