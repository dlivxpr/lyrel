//! 工作台文本只读快照；代码共享 Rope，Markdown 共享源文 String，不复制整份文档。
use gpui_kit::component::input::{Position, Rope, RopeExt as _};
use std::{ops::Range, sync::Arc};
#[derive(Clone)]
pub(crate) enum TextSnapshot {
    Code(Rope),
    Markdown(Arc<String>),
}
impl From<Rope> for TextSnapshot {
    fn from(text: Rope) -> Self {
        Self::Code(text)
    }
}
impl Default for TextSnapshot {
    fn default() -> Self {
        Self::Code(Rope::new())
    }
}
impl PartialEq for TextSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.bytes().eq(other.bytes())
    }
}
impl Eq for TextSnapshot {}
impl std::fmt::Debug for TextSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextSnapshot")
            .field("len", &self.len())
            .finish()
    }
}
impl std::fmt::Display for TextSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for chunk in self.chunks() {
            f.write_str(chunk)?;
        }
        Ok(())
    }
}
pub(crate) struct TextPoint {
    pub(crate) row: usize,
    pub(crate) column: usize,
}
impl TextSnapshot {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Code(text) => text.len(),
            Self::Markdown(text) => text.len(),
        }
    }
    pub(crate) fn chunks(&self) -> impl Iterator<Item = &str> {
        let (code, markdown) = match self {
            Self::Code(text) => (Some(text), None),
            Self::Markdown(text) => (None, Some(text.as_str())),
        };
        code.into_iter()
            .flat_map(|text| text.chunks())
            .chain(markdown)
    }
    pub(crate) fn reverse_chunks(&self) -> impl Iterator<Item = &str> {
        let (code, markdown) = match self {
            Self::Code(text) => (Some(text), None),
            Self::Markdown(text) => (None, Some(text.as_str())),
        };
        code.into_iter()
            .flat_map(|text| text.chunks_at(text.len()).0.reversed())
            .chain(markdown)
    }
    pub(crate) fn bytes(&self) -> impl Iterator<Item = u8> {
        self.chunks().flat_map(str::bytes)
    }
    pub(crate) fn chars(&self) -> impl Iterator<Item = char> {
        self.chunks().flat_map(str::chars)
    }
    pub(crate) fn reverse_chars(&self) -> impl Iterator<Item = char> {
        self.reverse_chunks().flat_map(|chunk| chunk.chars().rev())
    }
    pub(crate) fn chunk_at(&self, at: usize) -> (&str, usize) {
        match self {
            Self::Code(text) => {
                let (mut chunks, start) = text.chunks_at(at);
                (chunks.next().unwrap_or(""), start)
            }
            Self::Markdown(text) => (text.as_str(), 0),
        }
    }
    pub(crate) fn floor_char_boundary(&self, mut at: usize) -> usize {
        at = at.min(self.len());
        match self {
            Self::Code(text) => text.floor_char_boundary(at),
            Self::Markdown(text) => text.floor_char_boundary(at),
        }
    }
    pub(crate) fn ceil_char_boundary(&self, at: usize) -> usize {
        match self {
            Self::Code(text) => text.ceil_char_boundary(at.min(text.len())),
            Self::Markdown(text) => text.ceil_char_boundary(at.min(text.len())),
        }
    }
    pub(crate) fn slice(&self, range: Range<usize>) -> String {
        match self {
            Self::Code(text) => text.slice(range).to_string(),
            Self::Markdown(text) => text[range].to_owned(),
        }
    }
    pub(crate) fn lines_len(&self) -> usize {
        self.bytes().filter(|&b| b == b'\n').count() + 1
    }
    pub(crate) fn offset_to_point(&self, at: usize) -> TextPoint {
        let at = self.floor_char_boundary(at);
        let mut row = 0;
        let mut column = 0;
        for byte in self.bytes().take(at) {
            if byte == b'\n' {
                row += 1;
                column = 0;
            } else {
                column += 1;
            }
        }
        TextPoint { row, column }
    }
    pub(crate) fn line_start_offset(&self, row: usize) -> usize {
        if row == 0 {
            return 0;
        }
        self.bytes()
            .enumerate()
            .filter(|(_, byte)| *byte == b'\n')
            .nth(row - 1)
            .map_or(self.len(), |(at, _)| at + 1)
    }
    pub(crate) fn offset_to_position(&self, at: usize) -> Position {
        match self {
            Self::Code(text) => text.offset_to_position(at),
            Self::Markdown(text) => {
                let at = text.floor_char_boundary(at.min(text.len()));
                let start = text[..at].rfind('\n').map_or(0, |at| at + 1);
                Position::new(
                    text[..at].bytes().filter(|&b| b == b'\n').count() as u32,
                    text[start..at].encode_utf16().count() as u32,
                )
            }
        }
    }
    pub(crate) fn position_to_offset(&self, position: &Position) -> usize {
        match self {
            Self::Code(text) => text.position_to_offset(position),
            Self::Markdown(text) => {
                let start = self.line_start_offset(position.line as usize);
                let line = text[start..].split('\n').next().unwrap_or("");
                let mut units = 0;
                for (at, ch) in line.char_indices() {
                    if units + ch.len_utf16() > position.character as usize {
                        return start + at;
                    }
                    units += ch.len_utf16();
                }
                start + line.len()
            }
        }
    }
    pub(crate) fn word_range(&self, at: usize) -> Option<Range<usize>> {
        match self {
            Self::Code(text) => text.word_range(at),
            Self::Markdown(text) => {
                use unicode_segmentation::UnicodeSegmentation;
                text.unicode_word_indices()
                    .find(|(start, word)| *start <= at && at <= *start + word.len())
                    .map(|(start, word)| start..start + word.len())
            }
        }
    }
}
