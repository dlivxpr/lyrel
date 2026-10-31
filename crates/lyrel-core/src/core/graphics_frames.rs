//! Lock-outside immutable frame construction. The caller reserves the entire
//! simultaneously live working set before decoding or allocating pixels.
use super::*;

pub(super) fn compose_frame(
    image: &mut Image,
    control: &Control,
    frame: &FramePreparation,
    work: &mut retention::Work,
) -> Result<(), &'static str> {
    if control.action == Action::Compose {
        return compose_retained(image, control, frame, work);
    }
    if control
        .x
        .checked_add(image.width)
        .is_none_or(|end| end > frame.width)
        || control
            .y
            .checked_add(image.height)
            .is_none_or(|end| end > frame.height)
    {
        return Err("EINVAL:Frame rectangle outside image");
    }
    let canvas_bytes = frame.width as usize * frame.height as usize * 4;
    let patch_bytes = image.width as usize * image.height as usize * 4;
    // Patch decode releases its output scratch before the base is decoded.
    // A PNG/RGB base can temporarily own both native pixels and RGBA; a
    // background canvas has only its final RGBA allocation.
    let base_copies = if frame.base.is_some() { 2 } else { 1 };
    let peak = (patch_bytes * 2)
        .max(patch_bytes + canvas_bytes * base_copies + frame.base.as_ref().map_or(0, |p| p.len()))
        .checked_add(image.data.capacity())
        .and_then(|n| n.checked_add(frame.scratch.max(decoder_scratch(image))))
        .ok_or("ENOSPC:Frame work size overflow")?;
    if !work.resize(peak) {
        return Err("ENOSPC:Frame composition budget exhausted");
    }
    let patch = decode_pixels(image)?;
    let mut canvas = if let Some(packet) = &frame.base {
        let Some(GraphicsUpdate::Resource { image, .. }) =
            GraphicsUpdate::decode_frame_owned(packet.to_vec())
        else {
            return Err("EINVAL:Invalid retained frame data");
        };
        let mut image = Arc::into_inner(image).expect("decoded packet has one source owner");
        decode_pixels(&mut image)?
    } else {
        control.offset_y.to_be_bytes().repeat(canvas_bytes / 4)
    };
    if canvas.len() != canvas_bytes {
        return Err("EINVAL:Invalid frame canvas");
    }
    for row in 0..image.height as usize {
        let start = ((control.y as usize + row) * frame.width as usize + control.x as usize) * 4;
        for (destination, source) in canvas[start..start + image.width as usize * 4]
            .chunks_exact_mut(4)
            .zip(
                patch[row * image.width as usize * 4..(row + 1) * image.width as usize * 4]
                    .chunks_exact(4),
            )
        {
            if control.offset_x == 1 {
                destination.copy_from_slice(source);
            } else {
                blend(destination, source);
            }
        }
    }
    drop(patch);
    *image = Image {
        width: frame.width,
        height: frame.height,
        data: canvas,
        format: WireFormat::Rgba,
        compressed: false,
    };
    work.resize(image.data.capacity());
    Ok(())
}

fn decode_packet(
    packet: &crate::daemon::protocol::GraphicsPacket,
) -> Result<Vec<u8>, &'static str> {
    let Some(GraphicsUpdate::Resource { image, .. }) =
        GraphicsUpdate::decode_frame_owned(packet.to_vec())
    else {
        return Err("EINVAL:Invalid retained frame data");
    };
    decode_pixels(&mut Arc::into_inner(image).expect("decoded packet has one source owner"))
}

fn compose_retained(
    image: &mut Image,
    c: &Control,
    frame: &FramePreparation,
    work: &mut retention::Work,
) -> Result<(), &'static str> {
    let source = frame
        .source
        .as_ref()
        .expect("composition captures its source");
    let destination = frame
        .base
        .as_ref()
        .expect("composition captures its destination");
    let canvas = frame.width as usize * frame.height as usize * 4;
    // The source decode retires its packet copy and native scratch before the
    // destination decode begins. Existing shared packets keep their own leases.
    let same = c.rows == c.cols;
    let peak = if same {
        destination.len() + canvas * 2
    } else {
        (source.len() + canvas * 2).max(destination.len() + canvas * 2 + canvas.max(source.len()))
    }
    .checked_add(frame.scratch)
    .ok_or("ENOSPC:Frame work size overflow")?;
    if !work.resize(peak) {
        return Err("ENOSPC:Frame composition budget exhausted");
    }
    let source = if same {
        None
    } else {
        Some(decode_packet(source)?)
    };
    let mut destination = decode_packet(destination)?;
    if source.as_ref().is_some_and(|s| s.len() != canvas) || destination.len() != canvas {
        return Err("EINVAL:Invalid frame canvas");
    }
    let width = if c.w == 0 { frame.width } else { c.w } as usize;
    let height = if c.h == 0 { frame.height } else { c.h } as usize;
    for row in 0..height {
        let src = ((c.offset_y as usize + row) * frame.width as usize + c.offset_x as usize) * 4;
        let dst = ((c.y as usize + row) * frame.width as usize + c.x as usize) * 4;
        if let Some(source) = &source {
            if c.no_move {
                destination[dst..dst + width * 4].copy_from_slice(&source[src..src + width * 4]);
            } else {
                for (under, over) in destination[dst..dst + width * 4]
                    .chunks_exact_mut(4)
                    .zip(source[src..src + width * 4].chunks_exact(4))
                {
                    blend(under, over);
                }
            }
        } else if c.no_move {
            destination.copy_within(src..src + width * 4, dst);
        } else {
            // Capture one pixel on the stack. Validated disjoint rectangles
            // never read a pixel written by this command, even on the same row.
            for col in 0..width {
                let over: [u8; 4] = destination[src + col * 4..src + (col + 1) * 4]
                    .try_into()
                    .unwrap();
                blend(&mut destination[dst + col * 4..dst + (col + 1) * 4], &over);
            }
        }
    }
    drop(source);
    *image = Image {
        width: frame.width,
        height: frame.height,
        data: destination,
        format: WireFormat::Rgba,
        compressed: false,
    };
    work.resize(image.data.capacity());
    Ok(())
}

impl GraphicsState {
    pub(super) fn delete_frame(&mut self, c: &Control, effects: &mut Vec<GraphicsEffect>) {
        let Some(root) = self.lookup(c) else {
            return;
        };
        let count = self.animation_frames(root).count() as u32;
        if count == 1 {
            if c.delete == b'F' {
                self.remove_resource(root, effects);
            }
            return;
        }
        // The published deletion selector follows Kitty's clamped 1-based r:
        // omitted/zero removes the first; beyond the sequence removes the last.
        let number = c.rows.max(1).min(count);
        let target = self.frame_storage_key(root, number).unwrap();
        let old = self.resources[&target].content_key;
        let selected = self.resources[&root].selected;
        if target == root {
            let next = self.frame_storage_key(root, 2).unwrap();
            let promoted = self.resources.remove(&next).unwrap();
            let resource = self.resources.get_mut(&root).unwrap();
            resource.image = promoted.image;
            resource.resident = promoted.resident;
            resource.content_key = promoted.content_key;
            resource.gap_ms = promoted.gap_ms;
            effects.push(GraphicsEffect::Update(GraphicsUpdate::Frame {
                key: resource.content_key,
                root,
                number: 1,
            }));
        } else {
            self.resources.remove(&target);
        }
        // Renumber mutable children only: the stable root stays frame one
        // even after its immutable pixel owner has been promoted.
        self.resources.get_mut(&root).unwrap().mutation += 1;
        for resource in self
            .resources
            .values_mut()
            .filter(|r| r.root == Some(root) && r.frame_number > number)
        {
            resource.frame_number -= 1;
            effects.push(GraphicsEffect::Update(GraphicsUpdate::Frame {
                key: resource.content_key,
                root,
                number: resource.frame_number,
            }));
        }
        if selected == old {
            let replacement = self.frame_key(root, number.min(count - 1)).unwrap();
            self.select_content(root, replacement, effects);
        }
        // A stable root can be both a former pixel key and the placement shell.
        // Frame{number:1} retires that old pixel owner without deleting the shell.
        if old != root {
            effects.push(GraphicsEffect::Update(GraphicsUpdate::Remove {
                placements: Vec::new(),
                resources: vec![old],
            }));
        }
        self.frame_deleted(root, selected == old, effects);
    }
}

fn blend(destination: &mut [u8], source: &[u8]) {
    let alpha = u32::from(source[3]);
    let inverse = 255 - alpha;
    let total = alpha * 255 + u32::from(destination[3]) * inverse;
    if total == 0 {
        destination.fill(0);
        return;
    }
    for channel in 0..3 {
        destination[channel] = ((u32::from(source[channel]) * alpha * 255
            + u32::from(destination[channel]) * u32::from(destination[3]) * inverse
            + total / 2)
            / total) as u8;
    }
    destination[3] = ((total + 127) / 255) as u8;
}

fn decode_pixels(image: &mut Image) -> Result<Vec<u8>, &'static str> {
    if image.format != WireFormat::Png {
        return image.take_rgba8().ok_or("EINVAL:Invalid frame pixels");
    }
    let mut decoder = png_decoder(&image.data);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|_| "EINVAL:Invalid frame PNG")?;
    let mut output = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or("ENOSPC:Frame PNG output too large")?
    ];
    let info = reader
        .next_frame(&mut output)
        .map_err(|_| "EINVAL:Invalid frame PNG pixels")?;
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
    reader
        .finish()
        .map_err(|_| "EINVAL:Invalid frame PNG end")?;
    Ok(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(pixel: [u8; 4]) -> Image {
        Image {
            width: 1,
            height: 1,
            data: pixel.to_vec(),
            format: WireFormat::Rgba,
            compressed: false,
        }
    }

    #[test]
    fn prepared_root_edit_cannot_revive_deleted_replaced_or_newer_pixels() {
        for mutation in [
            "Ga=d,d=I,i=142",
            "Ga=t,i=142,s=1,v=1",
            "Ga=f,i=142,r=1,s=1,v=1,X=1",
        ] {
            let mut state = GraphicsState::new(crate::daemon::protocol::WinSize {
                cols: 80,
                rows: 24,
                cell_w: 10,
                cell_h: 20,
            });
            state.apply(
                Control::parse(b"Ga=t,i=142,s=1,v=1").unwrap(),
                Some(image([255, 0, 0, 255])),
            );
            let control = Control::parse(b"Ga=f,i=142,r=1,s=1,v=1,X=1").unwrap();
            let key = state.reserve_resource_key();
            let mut prepared = GraphicsState::prepare_image(
                image([0, 255, 0, 255]),
                state.work_owner(),
                key,
                &control,
                state.capture_frame(&control).unwrap(),
            )
            .unwrap();
            state
                .prepare_image_metadata(&control, &mut prepared)
                .unwrap();
            let mutation = Control::parse(mutation.as_bytes()).unwrap();
            state.apply(
                mutation,
                (mutation.action != Action::Delete).then(|| image([0, 0, 255, 255])),
            );
            let effects = state.apply_prepared(control, prepared);
            assert!(
                !effects
                    .iter()
                    .any(|effect| matches!(effect, GraphicsEffect::Frame(_))),
                "stale prepared edit must not publish pixels"
            );
            let (replay, _work) = state.replay().unwrap();
            let pixels: Vec<_> = replay
                .iter()
                .filter_map(|effect| {
                    let GraphicsEffect::Frame(packet) = effect else {
                        return None;
                    };
                    let GraphicsUpdate::Resource { image, .. } =
                        GraphicsUpdate::decode_frame_owned(packet.to_vec()).unwrap()
                    else {
                        return None;
                    };
                    Some(image.to_rgba8().unwrap())
                })
                .collect();
            if mutation.action == Action::Delete {
                assert!(pixels.is_empty());
            } else {
                assert_eq!(pixels, [vec![0, 0, 255, 255]]);
            }
        }
    }

    #[test]
    fn prepared_compose_rejects_delete_renumber_edit_and_replacement_with_snapshot_owners() {
        for mutation in [
            "Ga=d,d=f,i=143,r=1",
            "Ga=d,d=f,i=143,r=2",
            "Ga=f,i=143,r=2,X=1",
            "Ga=t,i=143",
            "Ga=d,d=I,i=143",
        ] {
            let mut state = GraphicsState::new(crate::daemon::protocol::WinSize {
                cols: 80,
                rows: 24,
                cell_w: 10,
                cell_h: 20,
            });
            state.apply(
                Control::parse(b"Ga=T,i=143,C=1").unwrap(),
                Some(image([255, 0, 0, 255])),
            );
            state.apply(
                Control::parse(b"Ga=f,i=143").unwrap(),
                Some(image([0, 255, 0, 255])),
            );
            let c = Control::parse(b"Ga=c,i=143,r=2,c=1,C=1").unwrap();
            let key = state.reserve_resource_key();
            let mut prepared = GraphicsState::prepare_image(
                image([0; 4]),
                state.work_owner(),
                key,
                &c,
                state.capture_frame(&c).unwrap(),
            )
            .unwrap();
            state.prepare_image_metadata(&c, &mut prepared).unwrap();
            let (snapshot, _work) = state.replay().unwrap();
            let m = Control::parse(mutation.as_bytes()).unwrap();
            state.apply(
                m,
                (m.action != Action::Delete).then(|| image([0, 0, 255, 255])),
            );
            let (before, _before_work) = state.replay().unwrap();
            let current: Vec<_> = before
                .iter()
                .filter_map(|e| {
                    if let GraphicsEffect::Frame(p) = e {
                        Some(p.to_vec())
                    } else {
                        None
                    }
                })
                .collect();
            let effects = state.apply_prepared(c, prepared);
            assert!(effects.iter().all(|e| !matches!(
                e,
                GraphicsEffect::Frame(_) | GraphicsEffect::Update(GraphicsUpdate::Select { .. })
            )));
            assert!(effects.iter().any(
                |e| matches!(e, GraphicsEffect::Reply(bytes) if bytes.starts_with(b"\x1b_Gi=143;E"))
            ));
            let (after, _after_work) = state.replay().unwrap();
            assert_eq!(
                after
                    .iter()
                    .filter_map(|e| if let GraphicsEffect::Frame(p) = e {
                        Some(p.to_vec())
                    } else {
                        None
                    })
                    .collect::<Vec<_>>(),
                current
            );
            let old: Vec<_> = snapshot
                .iter()
                .filter_map(|e| {
                    let GraphicsEffect::Frame(p) = e else {
                        return None;
                    };
                    let GraphicsUpdate::Resource { image, .. } =
                        GraphicsUpdate::decode_frame_owned(p.to_vec()).unwrap()
                    else {
                        return None;
                    };
                    Some(image.data.clone())
                })
                .collect();
            assert_eq!(
                old,
                [vec![255, 0, 0, 255], vec![0, 255, 0, 255]],
                "an old snapshot remains immutable, not a route back into authority"
            );
        }
    }

    #[test]
    fn deletion_of_each_selected_position_keeps_independent_referenced_pixels_and_dense_numbers() {
        for removed in 1usize..=4 {
            let mut state = GraphicsState::new(crate::daemon::protocol::WinSize {
                cols: 80,
                rows: 24,
                cell_w: 10,
                cell_h: 20,
            });
            state.apply(
                Control::parse(b"Ga=t,I=143").unwrap(),
                Some(image([10, 0, 0, 255])),
            );
            for red in [20, 30, 40] {
                state.apply(
                    Control::parse(b"Ga=f,I=143,c=1,X=1").unwrap(),
                    Some(image([red, 0, 0, 255])),
                );
            }
            state.apply(
                Control::parse(format!("Ga=a,I=143,c={removed}").as_bytes()).unwrap(),
                None,
            );
            state.apply(
                Control::parse(format!("Ga=d,d=f,I=143,r={removed}").as_bytes()).unwrap(),
                None,
            );
            let root = state
                .lookup(&Control {
                    number: 143,
                    ..Control::default()
                })
                .unwrap();
            let expected: Vec<_> = [10u8, 20, 30, 40]
                .into_iter()
                .enumerate()
                .filter_map(|(i, red)| (i + 1 != removed).then_some(red))
                .collect();
            for (index, red) in expected.iter().enumerate() {
                let content = state.frame_key(root, index as u32 + 1).unwrap();
                let resource = state
                    .resources
                    .values()
                    .find(|r| r.content_key == content)
                    .unwrap();
                let packet = resource.resident.lock().unwrap().packet.clone().unwrap();
                let GraphicsUpdate::Resource { image, .. } =
                    GraphicsUpdate::decode_frame_owned(packet.to_vec()).unwrap()
                else {
                    panic!("retained pixels");
                };
                assert_eq!(image.data, [*red, 0, 0, 255]);
            }
            assert!(state.frame_key(root, 4).is_none());
            assert_eq!(
                state.resources[&root].selected,
                state.frame_key(root, (removed as u32).min(3)).unwrap()
            );
        }
    }

    #[test]
    fn composition_reserves_temporary_peak_before_decode_without_damaging_retained_pixels() {
        let mut state = GraphicsState::new(crate::daemon::protocol::WinSize {
            cols: 80,
            rows: 24,
            cell_w: 10,
            cell_h: 20,
        });
        state.apply(
            Control::parse(b"Ga=t,i=143").unwrap(),
            Some(image([255, 0, 0, 255])),
        );
        state.apply(
            Control::parse(b"Ga=f,i=143").unwrap(),
            Some(image([0, 255, 0, 255])),
        );
        let c = Control::parse(b"Ga=c,i=143,r=2,c=1,C=1").unwrap();
        let frame = state.capture_frame(&c).unwrap();
        let mut busy = state.retention.work(state.owner);
        assert!(busy.resize(state.retention.limits.pane_work - 1));
        let key = state.reserve_resource_key();
        assert!(matches!(
            GraphicsState::prepare_image(
                Image {
                    width: 0,
                    height: 0,
                    data: Vec::new(),
                    format: WireFormat::Rgba,
                    compressed: false
                },
                state.work_owner(),
                key,
                &c,
                frame
            ),
            Err("ENOSPC:Frame composition budget exhausted")
        ));
        drop(busy);
        let (snapshot, _work) = state.replay().unwrap();
        let pixels: Vec<_> = snapshot
            .iter()
            .filter_map(|e| {
                let GraphicsEffect::Frame(p) = e else {
                    return None;
                };
                let GraphicsUpdate::Resource { image, .. } =
                    GraphicsUpdate::decode_frame_owned(p.to_vec()).unwrap()
                else {
                    return None;
                };
                Some(image.data.clone())
            })
            .collect();
        assert_eq!(pixels, [vec![255, 0, 0, 255], vec![0, 255, 0, 255]]);
        assert!(
            state
                .apply(c, None)
                .iter()
                .any(|effect| matches!(effect, GraphicsEffect::Frame(_))),
            "work credit returns after its actual owner drops"
        );
    }
}
