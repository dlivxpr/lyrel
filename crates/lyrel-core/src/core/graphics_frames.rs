//! Lock-outside immutable frame construction. The caller reserves the entire
//! simultaneously live working set before decoding or allocating pixels.
use super::*;

pub(super) fn compose_frame(image: &mut Image, control: &Control, frame: &FramePreparation, work: &mut retention::Work) -> Result<(), &'static str> {
    if control.x.checked_add(image.width).is_none_or(|end| end > frame.width)
        || control.y.checked_add(image.height).is_none_or(|end| end > frame.height) {
        return Err("EINVAL:Frame rectangle outside image");
    }
    let canvas_bytes = frame.width as usize * frame.height as usize * 4;
    let patch_bytes = image.width as usize * image.height as usize * 4;
    // Patch decode releases its output scratch before the base is decoded.
    // A PNG/RGB base can temporarily own both native pixels and RGBA; a
    // background canvas has only its final RGBA allocation.
    let base_copies = if frame.base.is_some() { 2 } else { 1 };
    let peak = (patch_bytes * 2).max(patch_bytes + canvas_bytes * base_copies
        + frame.base.as_ref().map_or(0, |p| p.len()))
        .checked_add(image.data.capacity())
        .and_then(|n| n.checked_add(frame.scratch.max(if image.format == WireFormat::Png { 4 << 20 } else if image.compressed { 256 << 10 } else { 0 })))
        .ok_or("ENOSPC:Frame work size overflow")?;
    if !work.resize(peak) { return Err("ENOSPC:Frame composition budget exhausted"); }
    let patch = decode_pixels(image)?;
    let mut canvas = if let Some(packet) = &frame.base {
        let Some(GraphicsUpdate::Resource { image, .. }) = GraphicsUpdate::decode_frame_owned(packet.to_vec()) else {
            return Err("EINVAL:Invalid retained frame data");
        };
        let mut image = Arc::into_inner(image).expect("decoded packet has one source owner");
        decode_pixels(&mut image)?
    } else {
        control.offset_y.to_be_bytes().repeat(canvas_bytes / 4)
    };
    if canvas.len() != canvas_bytes { return Err("EINVAL:Invalid frame canvas"); }
    for row in 0..image.height as usize {
        let start = ((control.y as usize + row) * frame.width as usize + control.x as usize) * 4;
        for (destination, source) in canvas[start..start + image.width as usize * 4].chunks_exact_mut(4)
            .zip(patch[row * image.width as usize * 4..(row + 1) * image.width as usize * 4].chunks_exact(4)) {
            if control.offset_x == 1 { destination.copy_from_slice(source); }
            else { blend(destination, source); }
        }
    }
    drop(patch);
    *image = Image { width: frame.width, height: frame.height, data: canvas, format: WireFormat::Rgba, compressed: false };
    work.resize(image.data.capacity());
    Ok(())
}

fn blend(destination: &mut [u8], source: &[u8]) {
    let alpha = u32::from(source[3]);
    let inverse = 255 - alpha;
    let total = alpha * 255 + u32::from(destination[3]) * inverse;
    if total == 0 { destination.fill(0); return; }
    for channel in 0..3 {
        destination[channel] = ((u32::from(source[channel]) * alpha * 255
            + u32::from(destination[channel]) * u32::from(destination[3]) * inverse + total / 2) / total) as u8;
    }
    destination[3] = ((total + 127) / 255) as u8;
}

fn decode_pixels(image: &mut Image) -> Result<Vec<u8>, &'static str> {
    if image.format != WireFormat::Png {
        return image.take_rgba8().ok_or("EINVAL:Invalid frame pixels");
    }
    let mut decoder = png::Decoder::new_with_limits(std::io::Cursor::new(&image.data), png::Limits { bytes: 4 << 20 });
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| "EINVAL:Invalid frame PNG")?;
    let mut output = vec![0; reader.output_buffer_size().ok_or("ENOSPC:Frame PNG output too large")?];
    let info = reader.next_frame(&mut output).map_err(|_| "EINVAL:Invalid frame PNG pixels")?;
    let mut rgba = Vec::with_capacity(image.width as usize * image.height as usize * 4);
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err("EINVAL:Unexpanded frame PNG"),
    };
    for pixel in output[..info.buffer_size()].chunks_exact(channels) {
        match channels {
            1 => rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], 255]),
            2 => rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]),
            3 => rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]),
            _ => rgba.extend_from_slice(pixel),
        }
    }
    reader.finish().map_err(|_| "EINVAL:Invalid frame PNG end")?;
    Ok(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(pixel: [u8; 4]) -> Image {
        Image { width: 1, height: 1, data: pixel.to_vec(), format: WireFormat::Rgba, compressed: false }
    }

    #[test]
    fn prepared_root_edit_cannot_revive_deleted_replaced_or_newer_pixels() {
        for mutation in ["Ga=d,d=I,i=142", "Ga=t,i=142,s=1,v=1", "Ga=f,i=142,r=1,s=1,v=1,X=1"] {
            let mut state = GraphicsState::new(crate::daemon::protocol::WinSize { cols: 80, rows: 24, cell_w: 10, cell_h: 20 });
            state.apply(Control::parse(b"Ga=t,i=142,s=1,v=1").unwrap(), Some(image([255,0,0,255])));
            let control = Control::parse(b"Ga=f,i=142,r=1,s=1,v=1,X=1").unwrap();
            let key = state.reserve_resource_key();
            let mut prepared = GraphicsState::prepare_image(image([0,255,0,255]), state.work_owner(), key, &control, state.capture_frame(&control).unwrap()).unwrap();
            state.prepare_image_metadata(&control, &mut prepared).unwrap();
            let mutation = Control::parse(mutation.as_bytes()).unwrap();
            state.apply(mutation, (mutation.action != Action::Delete).then(|| image([0,0,255,255])));
            let effects = state.apply_prepared(control, prepared);
            assert!(!effects.iter().any(|effect| matches!(effect, GraphicsEffect::Frame(_))), "stale prepared edit must not publish pixels");
            let (replay, _work) = state.replay().unwrap();
            let pixels: Vec<_> = replay.iter().filter_map(|effect| {
                let GraphicsEffect::Frame(packet) = effect else { return None; };
                let GraphicsUpdate::Resource { image, .. } = GraphicsUpdate::decode_frame_owned(packet.to_vec()).unwrap() else { return None; };
                Some(image.to_rgba8().unwrap())
            }).collect();
            if mutation.action == Action::Delete { assert!(pixels.is_empty()); }
            else { assert_eq!(pixels, [vec![0,0,255,255]]); }
        }
    }
}
