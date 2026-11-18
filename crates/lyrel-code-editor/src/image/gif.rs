//! GIF 动画：顺序解码、合成与循环。
//!
//! 这是 Glim `glim-services/src/media.rs` 中 `GifPlayback` 的移植（MIT，
//! 上游 revision 与原始摘要见同目录 `UPSTREAM.toml`）。保留的行为：
//!
//! - `gif` 解码器按 RGBA 输出，逐帧只读一行，interlace 按
//!   `(0,8) (4,8) (2,4) (1,2)` 的 pass 顺序填回画布；
//! - 合成本身维护 `Keep` / `Background` / `Previous` disposal、上一帧矩形
//!   与恢复画布，透明像素不覆盖已有内容；
//! - finite repeat 在 EOF 重开解码器并清空画布，没有累计帧数上限；
//! - 0/1 centisecond 的帧延时按浏览器习惯转成 100ms，其余按 delay × 10ms。
//!
//! 本地适配：
//!
//! - 输入是 owning-Host 读到的字节，不再有本机路径或临时文件权威；
//! - 去掉了上游按准入压力降低合成画布分辨率的预览路径。Lyrel 在打开时
//!   一次核准 canvas + restore + 输出帧 + 行暂存的预算，超出就明确拒绝，
//!   因此实际大小（100%）始终是源文件的真实分辨率，不做静默降质；
//! - 没有 Glim `ResourcePool` 或 `RasterFrame`：输出交给 Lyrel 的
//!   `Bitmap`/纹理租约。上游 `media/raster.rs` 的 `fitted_dimensions` 与
//!   `sample_nearest` 因此不被使用；
//! - 帧调度（可见时才解码、隐藏时停止帧工作）属于 `image_document`。

use ::image::RgbaImage;
use std::io::{Cursor, Read};
use std::sync::Arc;
use std::time::Duration;

/// 一次动画的合成预算：canvas、必要时的一帧恢复画布、输出帧与一行暂存
/// 都是 RGBA 缓冲。64 MiB 是 Lyrel 自己的上限，不是上游数字或实测结果；
/// 超出时整份动画明确拒绝，不降低分辨率继续播放。
pub const COMPOSITE_BYTES: u64 = 64 * 1024 * 1024;

/// 与图片文档的像素预算一致的最长边。
const MAX_EDGE: u32 = 8192;

#[derive(Debug)]
pub enum GifError {
    /// canvas + restore + output + row 超出动画合成预算。
    Budget {
        required: u64,
        limit: u64,
    },
    EmptyCanvas,
    NoFrames,
    Truncated,
    Decode(String),
}

/// The workbench localizes these; `Display` stays a stable English diagnostic
/// for logs and tests.
impl std::fmt::Display for GifError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Budget { required, limit } => write!(
                formatter,
                "the animation needs {} MiB but its budget is {} MiB",
                mib(*required),
                mib(*limit)
            ),
            Self::EmptyCanvas => formatter.write_str("the animation has an empty canvas"),
            Self::NoFrames => formatter.write_str("the animation contains no frames"),
            Self::Truncated => formatter.write_str("the animation ends before a frame is complete"),
            Self::Decode(message) => write!(formatter, "the image could not be decoded: {message}"),
        }
    }
}

impl std::error::Error for GifError {}

impl From<::gif::DecodingError> for GifError {
    fn from(error: ::gif::DecodingError) -> Self {
        Self::Decode(error.to_string())
    }
}

fn mib(bytes: u64) -> u64 {
    bytes.div_ceil(1024 * 1024)
}

/// Upstream `GifPlayback`'s bounded state, with Lyrel's Host-owned bytes.
pub struct GifPlayback {
    input: Arc<[u8]>,
    decoder: ::gif::Decoder<Box<dyn Read + Send>>,
    canvas: RgbaImage,
    restore: Option<RgbaImage>,
    disposal: ::gif::DisposalMethod,
    rectangle: (u32, u32, u32, u32),
    frames: usize,
    loops: u32,
    finished: bool,
    pending_frame: bool,
}

impl GifPlayback {
    pub fn new(bytes: Arc<[u8]>) -> Result<Self, GifError> {
        let decoder = Self::decoder(&bytes)?;
        let logical = (u32::from(decoder.width()), u32::from(decoder.height()));
        if logical.0 == 0 || logical.1 == 0 {
            return Err(GifError::EmptyCanvas);
        }
        // 打开时一次核准整份合成状态：canvas、一帧恢复画布、输出帧与一行。
        // 上游在每次 next_frame 时向 ResourcePool 申请；Lyrel 没有把那套
        // 全应用资源池搬进来，但准入仍然显式且不会中途降质。
        let pixels = u64::from(logical.0) * u64::from(logical.1);
        let required = pixels
            .checked_mul(12)
            .and_then(|bytes| bytes.checked_add(u64::from(logical.0) * 4))
            .ok_or(GifError::Budget {
                required: u64::MAX,
                limit: COMPOSITE_BYTES,
            })?;
        if logical.0 > MAX_EDGE || logical.1 > MAX_EDGE || required > COMPOSITE_BYTES {
            return Err(GifError::Budget {
                required,
                limit: COMPOSITE_BYTES,
            });
        }
        let canvas = RgbaImage::new(logical.0, logical.1);
        Ok(Self {
            input: bytes,
            decoder,
            canvas,
            restore: None,
            disposal: ::gif::DisposalMethod::Keep,
            rectangle: (0, 0, 0, 0),
            frames: 0,
            loops: 0,
            finished: false,
            pending_frame: false,
        })
    }

    fn decoder(input: &Arc<[u8]>) -> Result<::gif::Decoder<Box<dyn Read + Send>>, GifError> {
        let mut options = ::gif::DecodeOptions::new();
        options.set_color_output(::gif::ColorOutput::RGBA);
        options.check_frame_consistency(true);
        let input = Box::new(Cursor::new(Arc::clone(input))) as Box<dyn Read + Send>;
        Ok(options.read_info(input)?)
    }

    /// 解码下一帧并合成为完整画布。
    ///
    /// `None` 表示有限循环已经播完；此后保持最后一帧。任何解码错误都返回
    /// `Err`，调用者把动画标成可见失败，而不是留下半帧冒充完整内容。
    pub fn next_frame(&mut self) -> Result<Option<(RgbaImage, Duration)>, GifError> {
        if self.finished {
            return Ok(None);
        }
        if !self.pending_frame {
            if self.decoder.next_frame_info()?.is_none() {
                if self.frames == 0 {
                    return Err(GifError::NoFrames);
                }
                self.loops = self.loops.saturating_add(1);
                if let ::gif::Repeat::Finite(repeats) = self.decoder.repeat()
                    && self.loops > u32::from(repeats)
                {
                    self.finished = true;
                    return Ok(None);
                }
                self.decoder = Self::decoder(&self.input)?;
                self.canvas.fill(0);
                self.restore = None;
                self.disposal = ::gif::DisposalMethod::Keep;
                self.frames = 0;
                if self.decoder.next_frame_info()?.is_none() {
                    return Err(GifError::NoFrames);
                }
            }
            self.pending_frame = true;
        }
        // 上一帧的 disposal 先落地：Background 清掉它的矩形，Previous 恢复
        // 保存过的画布；Keep 留着。
        match self.disposal {
            ::gif::DisposalMethod::Background => {
                let (left, top, width, height) = self.rectangle;
                let stride = self.canvas.width() as usize * 4;
                let pixels: &mut [u8] = self.canvas.as_mut();
                for y in top..top + height {
                    let start = y as usize * stride + left as usize * 4;
                    pixels[start..start + width as usize * 4].fill(0);
                }
            }
            ::gif::DisposalMethod::Previous => {
                if let Some(restore) = self.restore.take() {
                    self.canvas = restore;
                }
            }
            _ => {}
        }
        let info = self
            .decoder
            .current_frame_info()
            .ok_or(GifError::Truncated)?;
        let (source_left, source_top, width, height, interlaced, dispose, delay) = (
            u32::from(info.left),
            u32::from(info.top),
            u32::from(info.width),
            u32::from(info.height),
            info.interlaced,
            info.dispose,
            info.delay,
        );
        self.frames += 1;
        // 上游允许合成画布小于逻辑尺寸并做最近邻采样；Lyrel 不降质，画布
        // 始终等于逻辑尺寸，所以这里没有缩放因子。
        let (cw, ch) = self.canvas.dimensions();
        self.restore = None;
        self.restore = (dispose == ::gif::DisposalMethod::Previous).then(|| self.canvas.clone());
        let mut row = vec![0; width as usize * 4];
        let passes: &[(u32, u32)] = if interlaced {
            &[(0, 8), (4, 8), (2, 4), (1, 2)]
        } else {
            &[(0, 1)]
        };
        for &(first, step) in passes {
            for y in (first..height).step_by(step as usize) {
                if row.is_empty() {
                    continue;
                }
                if !self.decoder.fill_buffer(&mut row)? {
                    return Err(GifError::Truncated);
                }
                let dy = source_top + y;
                if dy >= ch {
                    continue;
                }
                for x in 0..width {
                    let dx = source_left + x;
                    if dx >= cw {
                        continue;
                    }
                    let offset = x as usize * 4;
                    let pixel = &row[offset..offset + 4];
                    // 透明像素不覆盖画布；这是 GIF 的合成规则，不是把
                    // 透明色当作黑色。
                    if pixel[3] != 0 {
                        self.canvas.put_pixel(
                            dx,
                            dy,
                            ::image::Rgba([pixel[0], pixel[1], pixel[2], pixel[3]]),
                        );
                    }
                }
            }
        }
        self.pending_frame = false;
        self.disposal = dispose;
        self.rectangle = (source_left, source_top, width, height);
        // 与浏览器一致：0/1 centisecond 的帧不能形成零时长忙循环。
        let delay_ms = if delay <= 1 {
            100
        } else {
            u32::from(delay) * 10
        };
        Ok(Some((
            self.canvas.clone(),
            Duration::from_millis(u64::from(delay_ms)),
        )))
    }
}

#[cfg(test)]
#[path = "gif/tests.rs"]
mod tests;
