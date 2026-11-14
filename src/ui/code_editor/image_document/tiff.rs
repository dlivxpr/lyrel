//! 多页 TIFF：枚举页表，按需解码单页。
//!
//! 解码器是正式 registry `tiff`（image-rs 的同一实现，MIT），不自制 TIFF
//! 解析。页尺寸来自 IFD 头，像素只在某个 presentation 请求该页时解码；
//! 打开的文档不预解码所有页。

use crate::ui::code_editor::markdown::images::{MAX_PIXELS, render};
use crate::ui::i18n::{L10nKey, t, t_fmt};
use image::RgbaImage;
use std::io::Cursor;
use std::sync::Arc;
use tiff::ColorType;
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

/// 超过这个页数的 TIFF 不进入文档；页表与页数本身也是一项预算。
pub(super) const MAX_PAGES: usize = 1024;
/// 单页解码期间 decoder 可分配的缓冲上限。16-bit RGBA 的每个像素需要
/// 8 字节，所以它与像素预算一起决定某页能否被解码。
pub(super) const PAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EDGE: u32 = 8192;
const IFD_VALUE_BYTES: usize = 1024 * 1024;
const INTERMEDIATE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub(super) enum TiffError {
    Pages { limit: usize },
    Corrupt(String),
}

impl std::fmt::Display for TiffError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pages { limit } => formatter.write_str(&t_fmt(
                L10nKey::EditorImagePagesLimit,
                &[("pages", &limit.to_string())],
            )),
            Self::Corrupt(message) => write!(
                formatter,
                "{} ({message})",
                t(L10nKey::EditorImageUndecodable)
            ),
        }
    }
}

impl std::error::Error for TiffError {}

#[derive(Debug)]
pub(super) enum TiffPageError {
    Missing,
    Budget { required: u64, limit: u64 },
    Unsupported(String),
    Corrupt(String),
}

impl std::fmt::Display for TiffPageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => formatter.write_str(t(L10nKey::EditorImagePageMissing)),
            Self::Budget { required, limit } => formatter.write_str(&t_fmt(
                L10nKey::EditorImagePageBytesLimit,
                &[
                    ("required", &mib(*required).to_string()),
                    ("limit", &mib(*limit).to_string()),
                ],
            )),
            Self::Unsupported(layout) => write!(
                formatter,
                "{} ({layout})",
                t(L10nKey::EditorImagePageUnsupported)
            ),
            Self::Corrupt(message) => write!(
                formatter,
                "{} ({message})",
                t(L10nKey::EditorImageUndecodable)
            ),
        }
    }
}

impl std::error::Error for TiffPageError {}

fn mib(bytes: u64) -> u64 {
    bytes.div_ceil(1024 * 1024)
}

/// 一份 TIFF 的页表：字节、页数与每页尺寸。可跨线程 clone。
#[derive(Clone)]
pub(crate) struct TiffPages {
    bytes: Arc<[u8]>,
    pages: Arc<[(u32, u32)]>,
}

impl TiffPages {
    pub(super) fn open(bytes: Arc<[u8]>) -> Result<Self, TiffError> {
        let mut decoder = Decoder::new(Cursor::new(Arc::clone(&bytes)))
            .map_err(|error| TiffError::Corrupt(error.to_string()))?;
        let mut pages = Vec::new();
        loop {
            let size = decoder
                .dimensions()
                .map_err(|error| TiffError::Corrupt(error.to_string()))?;
            pages.push(size);
            if pages.len() > MAX_PAGES {
                return Err(TiffError::Pages { limit: MAX_PAGES });
            }
            if !decoder.more_images() {
                break;
            }
            decoder
                .next_image()
                .map_err(|error| TiffError::Corrupt(error.to_string()))?;
        }
        Ok(Self {
            bytes,
            pages: Arc::from(pages),
        })
    }

    pub(super) fn len(&self) -> usize {
        self.pages.len()
    }

    /// 解码一页为 RGBA。像素与纹理都由调用方作为一页的呈现接管。
    pub(super) fn decode(&self, page: usize) -> Result<RgbaImage, TiffPageError> {
        if page >= self.pages.len() {
            return Err(TiffPageError::Missing);
        }
        let mut limits = Limits::default();
        limits.decoding_buffer_size = PAGE_BYTES as usize;
        limits.intermediate_buffer_size = INTERMEDIATE_BYTES;
        limits.ifd_value_size = IFD_VALUE_BYTES;
        let mut decoder = Decoder::new(Cursor::new(Arc::clone(&self.bytes)))
            .map_err(|error| TiffPageError::Corrupt(error.to_string()))?
            .with_limits(limits);
        decoder
            .seek_to_image(page)
            .map_err(|error| TiffPageError::Corrupt(error.to_string()))?;
        let (width, height) = decoder
            .dimensions()
            .map_err(|error| TiffPageError::Corrupt(error.to_string()))?;
        if width == 0 || height == 0 {
            return Err(TiffPageError::Corrupt("empty TIFF page".into()));
        }
        let pixels = u64::from(width) * u64::from(height);
        if width > MAX_EDGE || height > MAX_EDGE || pixels > MAX_PIXELS {
            return Err(TiffPageError::Budget {
                required: pixels.saturating_mul(4),
                limit: MAX_PIXELS * 4,
            });
        }
        let color = decoder
            .colortype()
            .map_err(|error| TiffPageError::Corrupt(error.to_string()))?;
        let required = pixels.saturating_mul(sample_bytes(&color));
        if required > PAGE_BYTES {
            return Err(TiffPageError::Budget {
                required,
                limit: PAGE_BYTES,
            });
        }
        let colormap = match color {
            ColorType::Palette(_) => decoder.get_tag_u16_vec(Tag::ColorMap).ok(),
            _ => None,
        };
        let result = decoder
            .read_image()
            .map_err(|error| TiffPageError::Corrupt(error.to_string()))?;
        into_rgba(width, height, &color, result, colormap.as_deref())
    }
}

/// 解码后一页像素，交给 `/image_document` 在后台转成纹理。
pub(super) fn render_page(page: RgbaImage) -> Arc<gpui_kit::RenderImage> {
    render(page)
}

fn sample_bytes(color: &ColorType) -> u64 {
    match color {
        ColorType::Gray(depth) | ColorType::Palette(depth) => u64::from(*depth).div_ceil(8),
        ColorType::GrayA(depth) => 2 * u64::from(*depth).div_ceil(8),
        ColorType::RGB(depth) | ColorType::YCbCr(depth) | ColorType::Lab(depth) => {
            3 * u64::from(*depth).div_ceil(8)
        }
        ColorType::RGBA(depth) => 4 * u64::from(*depth).div_ceil(8),
        ColorType::CMYK(depth) => 4 * u64::from(*depth).div_ceil(8),
        ColorType::CMYKA(depth) => 5 * u64::from(*depth).div_ceil(8),
        ColorType::Multiband {
            bit_depth,
            num_samples,
        } => u64::from(*num_samples) * u64::from(*bit_depth).div_ceil(8),
        _ => 0,
    }
}

fn expected_samples(color: &ColorType) -> usize {
    match color {
        ColorType::Gray(_) | ColorType::Palette(_) => 1,
        ColorType::GrayA(_) => 2,
        ColorType::RGB(_) | ColorType::YCbCr(_) | ColorType::Lab(_) => 3,
        ColorType::RGBA(_) | ColorType::CMYK(_) => 4,
        ColorType::CMYKA(_) => 5,
        ColorType::Multiband { num_samples, .. } => usize::from(*num_samples),
        _ => 1,
    }
}

fn exact(samples: usize, expected: usize) -> Result<(), TiffPageError> {
    if samples == expected {
        Ok(())
    } else {
        Err(TiffPageError::Corrupt(format!(
            "expected {expected} samples, got {samples}"
        )))
    }
}

fn into_rgba(
    width: u32,
    height: u32,
    color: &ColorType,
    result: DecodingResult,
    colormap: Option<&[u16]>,
) -> Result<RgbaImage, TiffPageError> {
    let pixels = width as usize * height as usize;
    let expected = pixels * expected_samples(color);
    let mut out = vec![0u8; pixels * 4];
    match (color, result) {
        (ColorType::Gray(8), DecodingResult::U8(samples)) => {
            exact(samples.len(), expected)?;
            for (pixel, value) in out.chunks_exact_mut(4).zip(samples) {
                pixel.copy_from_slice(&[value, value, value, 255]);
            }
        }
        (ColorType::Gray(16), DecodingResult::U16(samples)) => {
            exact(samples.len(), expected)?;
            for (pixel, value) in out.chunks_exact_mut(4).zip(samples) {
                let value = u16_to_u8(value);
                pixel.copy_from_slice(&[value, value, value, 255]);
            }
        }
        (ColorType::GrayA(8), DecodingResult::U8(samples)) => {
            exact(samples.len(), expected)?;
            for (pixel, value) in out.chunks_exact_mut(4).zip(samples.chunks_exact(2)) {
                pixel.copy_from_slice(&[value[0], value[0], value[0], value[1]]);
            }
        }
        (ColorType::GrayA(16), DecodingResult::U16(samples)) => {
            exact(samples.len(), expected)?;
            for (pixel, value) in out.chunks_exact_mut(4).zip(samples.chunks_exact(2)) {
                let grey = u16_to_u8(value[0]);
                pixel.copy_from_slice(&[grey, grey, grey, u16_to_u8(value[1])]);
            }
        }
        (ColorType::RGB(8), DecodingResult::U8(samples)) => {
            exact(samples.len(), expected)?;
            for (pixel, value) in out.chunks_exact_mut(4).zip(samples.chunks_exact(3)) {
                pixel.copy_from_slice(&[value[0], value[1], value[2], 255]);
            }
        }
        (ColorType::RGB(16), DecodingResult::U16(samples)) => {
            exact(samples.len(), expected)?;
            for (pixel, value) in out.chunks_exact_mut(4).zip(samples.chunks_exact(3)) {
                pixel.copy_from_slice(&[
                    u16_to_u8(value[0]),
                    u16_to_u8(value[1]),
                    u16_to_u8(value[2]),
                    255,
                ]);
            }
        }
        (ColorType::RGBA(8), DecodingResult::U8(samples)) => {
            exact(samples.len(), expected)?;
            out.copy_from_slice(&samples);
        }
        (ColorType::RGBA(16), DecodingResult::U16(samples)) => {
            exact(samples.len(), expected)?;
            for (pixel, value) in out.chunks_exact_mut(4).zip(samples.chunks_exact(4)) {
                pixel.copy_from_slice(&[
                    u16_to_u8(value[0]),
                    u16_to_u8(value[1]),
                    u16_to_u8(value[2]),
                    u16_to_u8(value[3]),
                ]);
            }
        }
        (ColorType::CMYK(8), DecodingResult::U8(samples)) => {
            exact(samples.len(), expected)?;
            for (pixel, value) in out.chunks_exact_mut(4).zip(samples.chunks_exact(4)) {
                let [red, green, blue] = cmyk_to_rgb(value[0], value[1], value[2], value[3]);
                pixel.copy_from_slice(&[red, green, blue, 255]);
            }
        }
        (ColorType::CMYK(16), DecodingResult::U16(samples)) => {
            exact(samples.len(), expected)?;
            for (pixel, value) in out.chunks_exact_mut(4).zip(samples.chunks_exact(4)) {
                let [red, green, blue] = cmyk_to_rgb(
                    u16_to_u8(value[0]),
                    u16_to_u8(value[1]),
                    u16_to_u8(value[2]),
                    u16_to_u8(value[3]),
                );
                pixel.copy_from_slice(&[red, green, blue, 255]);
            }
        }
        (ColorType::CMYKA(8), DecodingResult::U8(samples)) => {
            exact(samples.len(), expected)?;
            for (pixel, value) in out.chunks_exact_mut(4).zip(samples.chunks_exact(5)) {
                let [red, green, blue] = cmyk_to_rgb(value[0], value[1], value[2], value[3]);
                pixel.copy_from_slice(&[red, green, blue, value[4]]);
            }
        }
        (ColorType::YCbCr(8), DecodingResult::U8(samples)) => {
            exact(samples.len(), expected)?;
            for (pixel, value) in out.chunks_exact_mut(4).zip(samples.chunks_exact(3)) {
                let [red, green, blue] = ycbcr_to_rgb(value[0], value[1], value[2]);
                pixel.copy_from_slice(&[red, green, blue, 255]);
            }
        }
        (ColorType::YCbCr(16), DecodingResult::U16(samples)) => {
            exact(samples.len(), expected)?;
            for (pixel, value) in out.chunks_exact_mut(4).zip(samples.chunks_exact(3)) {
                let [red, green, blue] = ycbcr_to_rgb(
                    u16_to_u8(value[0]),
                    u16_to_u8(value[1]),
                    u16_to_u8(value[2]),
                );
                pixel.copy_from_slice(&[red, green, blue, 255]);
            }
        }
        (
            ColorType::Multiband {
                bit_depth,
                num_samples,
            },
            DecodingResult::U8(samples),
        ) if matches!(*bit_depth, 8) && matches!(*num_samples, 1 | 2) => {
            exact(samples.len(), expected)?;
            match num_samples {
                1 => {
                    for (pixel, value) in out.chunks_exact_mut(4).zip(samples) {
                        pixel.copy_from_slice(&[value, value, value, 255]);
                    }
                }
                _ => {
                    for (pixel, value) in out.chunks_exact_mut(4).zip(samples.chunks_exact(2)) {
                        pixel.copy_from_slice(&[value[0], value[0], value[0], value[1]]);
                    }
                }
            }
        }
        (ColorType::Palette(depth), DecodingResult::U8(samples)) => {
            exact(samples.len(), expected)?;
            let map = colormap
                .ok_or_else(|| TiffPageError::Unsupported("palette without a color map".into()))?;
            let entries = 1usize << usize::from((*depth).min(16));
            if map.len() < entries * 3 {
                return Err(TiffPageError::Corrupt(
                    "color map is shorter than its palette".into(),
                ));
            }
            for (pixel, index) in out.chunks_exact_mut(4).zip(samples) {
                let index = usize::from(index);
                if index >= entries {
                    return Err(TiffPageError::Corrupt(
                        "palette index outside the color map".into(),
                    ));
                }
                pixel.copy_from_slice(&[
                    u16_to_u8(map[index]),
                    u16_to_u8(map[entries + index]),
                    u16_to_u8(map[entries * 2 + index]),
                    255,
                ]);
            }
        }
        _ => {
            return Err(TiffPageError::Unsupported(format!(
                "{color:?} page samples"
            )));
        }
    }
    RgbaImage::from_raw(width, height, out)
        .ok_or_else(|| TiffPageError::Corrupt("decoded page does not match its dimensions".into()))
}

fn u16_to_u8(value: u16) -> u8 {
    ((u32::from(value) * 255 + 32_767) / 65_535) as u8
}

fn cmyk_to_rgb(cyan: u8, magenta: u8, yellow: u8, black: u8) -> [u8; 3] {
    let black = 255 - u16::from(black);
    [
        (((255 - u16::from(cyan)) * black) / 255) as u8,
        (((255 - u16::from(magenta)) * black) / 255) as u8,
        (((255 - u16::from(yellow)) * black) / 255) as u8,
    ]
}

fn ycbcr_to_rgb(luma: u8, blue_chroma: u8, red_chroma: u8) -> [u8; 3] {
    let luma = f32::from(luma);
    let blue = f32::from(blue_chroma) - 128.;
    let red = f32::from(red_chroma) - 128.;
    let clamp = |value: f32| value.round().clamp(0., 255.) as u8;
    [
        clamp(luma + 1.402 * red),
        clamp(luma - 0.344_136 * blue - 0.714_136 * red),
        clamp(luma + 1.772 * blue),
    ]
}

#[cfg(test)]
#[path = "tiff/tests.rs"]
mod tests;
