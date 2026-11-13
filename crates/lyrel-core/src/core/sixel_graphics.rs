//! Static inline Sixel admission and the released Rio decoder. This module has
//! no grid, resource store, renderer, filesystem or protocol discovery authority.
use super::graphics_state::{MAX_IMAGE_DECODED_BYTES, retention};
use super::kitty_graphics::{Image, WireFormat};
use rio_vt::ansi::sixel;
use rio_vt::performer::handler::{Handler, Processor};
use rio_vt::performer::parser::Params;

const SIDE: usize = 4096; // rio-graphics 0.5.30 MAX_GRAPHIC_DIMENSIONS
const PALETTE_SCRATCH: usize = 16 << 10;

// Only allocation/relocation admission: colors and pixels are interpreted solely
// by Rio. Like PNG IHDR admission, this runs before the decoder can allocate.
// Account for every geometry-changing command, not just the declared raster:
// Sixel permits later drawing outside that declaration.
fn scratch(body: &[u8]) -> Option<usize> {
    let (mut width, mut height, mut x, mut y) = (0usize, 0usize, 0usize, 0usize);
    let mut repeat = 1usize;
    let mut pixel_work = 0usize;
    let mut i = 0;
    while let Some(&byte) = body.get(i) {
        i += 1;
        let mut required = None;
        match byte {
            b'!' | b'"' | b'#' => {
                let mut params = [0u16; 5];
                let mut position = 0usize;
                while let Some(&next) = body.get(i) {
                    match next {
                        b'0'..=b'9' => {
                            if position < params.len() {
                                params[position] = params[position]
                                    .saturating_mul(10)
                                    .saturating_add(u16::from(next - b'0'));
                            }
                        }
                        b';' => position += 1,
                        _ => break,
                    }
                    i += 1;
                }
                if byte == b'!' {
                    repeat = usize::from(params[0]).max(1);
                } else if byte == b'"' && position >= 3 {
                    required = Some((usize::from(params[2]), usize::from(params[3])));
                }
            }
            b'$' => x = 0,
            b'-' => {
                x = 0;
                y = y.checked_add(6)?;
            }
            b'?'..=b'~' => {
                x = x.checked_add(repeat)?;
                let dots = byte - b'?';
                pixel_work =
                    pixel_work.checked_add(repeat.checked_mul(dots.count_ones() as usize)?)?;
                if pixel_work > 64 << 20 {
                    return None;
                }
                repeat = 1;
                let rows = (u8::BITS - dots.leading_zeros()) as usize;
                required = Some((x, y.checked_add(rows)?));
            }
            _ => (),
        }
        if let Some((w, h)) = required {
            if w > SIDE || h > SIDE {
                return None;
            }
            if w > width {
                pixel_work = pixel_work.checked_add(width.checked_mul(height)?)?;
                if pixel_work > 64 << 20 {
                    return None;
                }
            }
            width = width.max(w);
            height = height.max(h);
            if width.checked_mul(height)?.checked_mul(4)? > MAX_IMAGE_DECODED_BYTES {
                return None;
            }
        }
    }
    let pixels = width.checked_mul(height)?;
    if pixels == 0 {
        return None;
    }
    // Rio's u16 index Vec can grow to <2*pixel_count elements. Old/new
    // allocations during growth cost <6 bytes/pixel; finish holds that Vec
    // plus exact RGBA (at most 8 bytes/pixel), palette and lookup table.
    pixels.checked_mul(8)?.checked_add(PALETTE_SCRATCH)
}

#[derive(Default)]
struct Decoder {
    parser: Option<sixel::Parser>,
    image: Option<Image>,
}
impl Handler for Decoder {
    fn sixel_graphic_start(&mut self, params: &Params) {
        self.parser = Some(sixel::Parser::new(params, None));
    }
    fn is_sixel_graphic_active(&self) -> bool {
        self.parser.is_some()
    }
    fn sixel_graphic_put(&mut self, byte: u8) -> Result<(), sixel::Error> {
        self.parser
            .as_mut()
            .ok_or(sixel::Error::NonExistentParser)?
            .put(byte)
    }
    fn sixel_graphic_reset(&mut self) {
        self.parser = None;
    }
    fn sixel_graphic_finish(&mut self) {
        if let Some(parser) = self.parser.take()
            && let Ok((graphic, _palette)) = parser.finish()
        {
            self.image = Some(Image {
                width: graphic.width as u32,
                height: graphic.height as u32,
                data: graphic.pixels,
                format: WireFormat::Rgba,
                compressed: false,
            });
        }
    }
}

pub(crate) fn decode(mut source: Image, work: &mut retention::Work) -> Result<Image, &'static str> {
    let q = source
        .data
        .iter()
        .position(|byte| *byte == b'q')
        .ok_or("EINVAL:Invalid Sixel header")?;
    let scratch =
        scratch(&source.data[q + 1..]).ok_or("ENOSPC:Sixel dimensions or work exceed limit")?;
    if !work.resize(source.data.capacity() + scratch) {
        return Err("ENOSPC:Sixel preparation budget exhausted");
    }
    let mut decoder = Decoder::default();
    let mut processor = Processor::default();
    processor.advance(&mut decoder, b"\x1b");
    processor.advance(&mut decoder, &source.data[..=q]);
    // The released VT parser owns header Params; the released Sixel parser
    // receives exactly the bytes examined by allocation admission (including
    // C0 bytes). No second VT interpretation can splice numeric operands.
    let parser = decoder
        .parser
        .as_mut()
        .ok_or("EINVAL:Invalid Sixel header")?;
    for &byte in &source.data[q + 1..] {
        parser.put(byte).map_err(|_| "EINVAL:Invalid Sixel image")?;
    }
    decoder.sixel_graphic_finish();
    let image = decoder.image.take().ok_or("EINVAL:Invalid Sixel image")?;
    source.data = Vec::new();
    work.resize(image.data.capacity());
    Ok(image)
}

#[cfg(test)]
mod tests {
    use crate::core::graphics_state::{GraphicsEffect, GraphicsState, GraphicsUpdate};
    use crate::core::kitty_graphics::{GraphicsSniffer, Segment, Sniffed};
    use crate::daemon::protocol::WinSize;

    fn state() -> GraphicsState {
        GraphicsState::new(WinSize {
            cols: 80,
            rows: 24,
            cell_w: 10,
            cell_h: 20,
        })
    }

    fn feed(
        state: &mut GraphicsState,
        sniffer: &mut GraphicsSniffer,
        bytes: &[u8],
    ) -> (Vec<GraphicsEffect>, Vec<u8>) {
        let parts = match sniffer.sniff(bytes) {
            Sniffed::Plain(bytes) => vec![Segment::Output(bytes.to_vec())],
            Sniffed::Segments(parts) => parts,
        };
        let mut effects = Vec::new();
        let mut output = Vec::new();
        for part in parts {
            let update = match part {
                Segment::Output(bytes) => {
                    output.extend_from_slice(&bytes);
                    state.output(&bytes)
                }
                Segment::Command { control, image } => state.apply(control, image),
                Segment::Reply(bytes) => {
                    effects.push(GraphicsEffect::Reply(bytes));
                    continue;
                }
            };
            for effect in &update {
                if let GraphicsEffect::Output(bytes) = effect {
                    output.extend_from_slice(bytes);
                }
            }
            effects.extend(update);
        }
        (effects, output)
    }

    const RED: &[u8] = b"\x1bP0;1q\"1;1;20;40#0;2;100;0;0!20~\x1b\\";

    #[test]
    fn sixel_layout_scroll_display_and_clear_share_authoritative_placements() {
        for (mode, cursor, expected) in [
            (b"".as_slice(), b"\x1b[6;5H".as_slice(), (5, 4, 0, 2)),
            (b"\x1b[?80h", b"\x1b[6;5H", (0, 0, 0, 2)),
            (b"", b"\x1b[24;5H", (22, 4, 0, 2)),
            (b"\x1b[?1049h", b"\x1b[24;5H", (22, 4, 0, 2)),
        ] {
            let (mut state, mut sniffer) = (state(), GraphicsSniffer::new());
            feed(&mut state, &mut sniffer, mode);
            feed(&mut state, &mut sniffer, cursor);
            let (effects, _) = feed(&mut state, &mut sniffer, RED);
            assert!(
                effects
                    .iter()
                    .any(|effect| matches!(effect, GraphicsEffect::Frame(_)))
            );
            let p = state.placements().next().unwrap();
            assert_eq!(
                (
                    p.anchor_row - state.history_origin(),
                    p.anchor_col,
                    p.clip_top,
                    p.clip_bottom
                ),
                expected
            );
            assert_eq!((p.span_cols, p.span_rows), (2, 2));
            feed(&mut state, &mut sniffer, b"\x1b[2J");
            assert_eq!(state.placements().count(), 0);
        }
    }

    #[test]
    fn sixel_refusal_preserves_kitty_resource_and_following_text_cursor() {
        let (mut state, mut sniffer) = (state(), GraphicsSniffer::new());
        feed(
            &mut state,
            &mut sniffer,
            b"\x1b[6;5H\x1b_Ga=T,i=7,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\",
        );
        let resource = state.placements().next().unwrap().resource;
        assert!(state.set_capacity(1024, 4));
        for refused in [
            RED,
            b"\x1bPq\"1;1;65535;65535~\x1b\\",
            b"\x1bPq#0;2;101;0;0~\x1b\\",
            b"\x1bPq#0;2;100;0;0~\x18",
        ] {
            let (effects, output) = feed(&mut state, &mut sniffer, refused);
            assert!(output.is_empty());
            assert!(!effects.iter().any(|effect| matches!(
                effect,
                GraphicsEffect::Frame(_) | GraphicsEffect::Update(GraphicsUpdate::Remove { .. })
            )));
            assert_eq!(state.placements().count(), 1);
            assert_eq!(state.placements().next().unwrap().resource, resource);
            let (_, output) = feed(&mut state, &mut sniffer, b"TAIL");
            assert_eq!(output, b"TAIL");
        }
        feed(&mut state, &mut sniffer, b"\x1b_Ga=p,i=7,p=2,C=1,q=2\x1b\\");
        let p = state.placements().last().unwrap();
        assert_eq!((p.anchor_row, p.anchor_col), (5, 20));
    }

    #[test]
    fn sixel_fragmentation_preserves_foreign_dcs_and_right_cursor_mode() {
        for fragment in [1, 2, 17, 4096] {
            let (mut state, mut sniffer) = (state(), GraphicsSniffer::new());
            let foreign = b"\x1bP=1s\x1b\\\x1bP+q544e\x1b\\";
            let mut bytes = foreign.to_vec();
            bytes.extend_from_slice(b"\x1b[6;5H\x1b[?8452h");
            bytes.extend_from_slice(RED);
            bytes.extend_from_slice(b"\x1b_Ga=T,i=8,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\");
            let mut output = Vec::new();
            for part in bytes.chunks(fragment) {
                output.extend(feed(&mut state, &mut sniffer, part).1);
            }
            assert!(output.starts_with(foreign));
            assert_eq!(state.placements().count(), 2);
            let kitty = state.placements().find(|p| p.image_id == 8).unwrap();
            assert_eq!((kitty.anchor_row, kitty.anchor_col), (6, 6));
        }
    }

    #[test]
    fn sixel_overlong_header_is_bounded_and_never_reaches_mirror_decoder() {
        let (mut state, mut sniffer) = (state(), GraphicsSniffer::new());
        let mut bytes = b"\x1bP".to_vec();
        bytes.extend_from_slice(&[b'0'; 65]);
        bytes.extend_from_slice(b"q#0;2;100;0;0~\x1b\\TAIL");
        let (effects, output) = feed(&mut state, &mut sniffer, &bytes);
        assert_eq!(output, b"TAIL");
        assert!(
            effects
                .iter()
                .all(|effect| !matches!(effect, GraphicsEffect::Frame(_)))
        );
    }

    #[test]
    fn sixel_repeated_overdraw_and_encoded_limit_refuse_without_text_loss() {
        for repeated in [true, false] {
            let (mut state, mut sniffer) = (state(), GraphicsSniffer::new());
            let mut bytes = b"\x1bPq\"1;1;4096;6#0;2;100;0;0".to_vec();
            if repeated {
                bytes.extend_from_slice(&b"!4096~$".repeat(3000));
            } else {
                bytes.extend_from_slice(&vec![b'?'; 8 << 20]);
            }
            bytes.extend_from_slice(b"\x1b\\TAIL");
            let (effects, output) = feed(&mut state, &mut sniffer, &bytes);
            assert_eq!(output, b"TAIL");
            assert!(
                effects
                    .iter()
                    .all(|effect| !matches!(effect, GraphicsEffect::Frame(_)))
            );
            assert_eq!(state.placements().count(), 0);
        }
    }
}
