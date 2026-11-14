//! 图片格式识别：文件名与内容魔数。
//!
//! 工作台文档与 Markdown 预览共用这一张清单，避免两边各认一套格式。名字
//! 只在它真的声明了图片时生效；没有图片扩展名的文件由 [`ImageFormat::probe`]
//! 按真实魔数补认，绝不由扩展名决定解码器。

use std::path::Path;

/// 工作台认识的图片格式。
///
/// 前五种是静态位图，由 `markdown::images::decode_static` 解码；GIF 是逐帧
/// 动画，TIFF 按页解码。三种能力共用同一个文档与取景呈现，但解码路径与
/// 预算各自独立。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageFormat {
    Png,
    Jpeg,
    WebP,
    Bmp,
    Ico,
    Gif,
    Tiff,
}

impl ImageFormat {
    /// 名字声明的格式。只认真正表示图片的扩展名。
    pub(crate) fn from_extension(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match extension.as_str() {
            "png" => Self::Png,
            "jpg" | "jpeg" | "jpe" | "jfif" => Self::Jpeg,
            "webp" => Self::WebP,
            "bmp" | "dib" => Self::Bmp,
            "ico" => Self::Ico,
            "gif" => Self::Gif,
            "tif" | "tiff" => Self::Tiff,
            _ => return None,
        })
    }

    /// 字节开头说的格式，供没有图片扩展名的文件使用。
    ///
    /// BMP 故意不在探测清单里：`BM` 是两个普通大写字母，不足以把一份文本
    /// 从编辑器手里抢走。GIF 的 `GIF87a`/`GIF89a` 与 TIFF 的
    /// `II*\0`/`MM\0*`（含 BigTIFF 标记）都是二进制结构，不会被误认成文本。
    pub(crate) fn probe(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Some(Self::Png);
        }
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(Self::Jpeg);
        }
        if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            return Some(Self::WebP);
        }
        if bytes.starts_with(&[0x00, 0x00, 0x01, 0x00]) {
            return Some(Self::Ico);
        }
        if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            return Some(Self::Gif);
        }
        if bytes.starts_with(b"II\x2a\x00")
            || bytes.starts_with(b"MM\x00\x2a")
            || bytes.starts_with(b"II\x2b\x00")
            || bytes.starts_with(b"MM\x00\x2b")
        {
            return Some(Self::Tiff);
        }
        None
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::WebP => "WebP",
            Self::Bmp => "BMP",
            Self::Ico => "ICO",
            Self::Gif => "GIF",
            Self::Tiff => "TIFF",
        }
    }

    /// 由 `decode_static` 处理的五种静态位图。
    pub(crate) fn is_static(self) -> bool {
        matches!(
            self,
            Self::Png | Self::Jpeg | Self::WebP | Self::Bmp | Self::Ico
        )
    }
}
