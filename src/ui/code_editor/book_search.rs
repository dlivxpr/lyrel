//! Full-book text index and matching for the EPUB reader.
//!
//! Glim's pinned reader has no search. Lyrel builds one text index from the
//! sanitized blocks of every spine chapter (never from rendered layout) and
//! matches queries in the background. The index is bounded by
//! `lyrel_epub::Limits::index_text_bytes`; a query returns at most
//! `search_hits` matches and reports truncation instead of claiming a complete
//! answer. Matching is exact, case-sensitive substring search over the same
//! normalized text the reader displays.

use std::ops::Range;

use lyrel_epub::{Chapter, Limits, MAX_QUERY_BYTES};

/// One chapter's block texts, concatenated for matching with stable spans.
#[derive(Clone, Debug)]
pub(crate) struct ChapterText {
    pub(crate) chapter: usize,
    pub(crate) text: String,
    spans: Vec<(usize, usize)>,
    blocks: Vec<String>,
}

/// The whole book's sanitized text, one entry per spine chapter.
#[derive(Clone, Debug, Default)]
pub(crate) struct BookText {
    pub(crate) chapters: Vec<ChapterText>,
    pub(crate) bytes: usize,
}

impl BookText {
    /// Builds the index from chapters read with `ImageMode::Skip`.
    ///
    /// Returns `None` when the bounded index would exceed its byte budget.
    pub(crate) fn new(chapters: Vec<Chapter>, limits: &Limits) -> Result<Self, usize> {
        let mut text = Self::default();
        for chapter in chapters {
            let mut blocks = Vec::with_capacity(chapter.blocks.len());
            let mut spans = Vec::with_capacity(chapter.blocks.len());
            let mut joined = String::new();
            for block in &chapter.blocks {
                if !joined.is_empty() {
                    joined.push('\n');
                }
                let start = joined.len();
                joined.push_str(&block.text);
                spans.push((start, joined.len()));
                blocks.push(block.text.clone());
            }
            text.bytes = text.bytes.saturating_add(joined.len());
            if text.bytes > limits.index_text_bytes {
                return Err(text.bytes);
            }
            text.chapters.push(ChapterText {
                chapter: chapter.index,
                text: joined,
                spans,
                blocks,
            });
        }
        Ok(text)
    }
}

/// One returned match, in block-local text coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchHit {
    pub(crate) chapter: usize,
    pub(crate) block: usize,
    pub(crate) range: Range<usize>,
    pub(crate) snippet: String,
}

/// The honest result of one query: maybe truncated at the hit budget.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SearchOutput {
    pub(crate) hits: Vec<SearchHit>,
    pub(crate) truncated: bool,
}

/// Matches `query` across every chapter, stopping at the hit budget.
pub(crate) fn search(
    book: &BookText,
    query: &str,
    limits: &Limits,
    cancel: &lyrel_epub::CancellationToken,
) -> SearchOutput {
    let mut output = SearchOutput::default();
    if query.is_empty() || query.len() > MAX_QUERY_BYTES {
        return output;
    }
    let cap = limits.search_hits;
    for chapter in &book.chapters {
        if cancel.is_cancelled() {
            break;
        }
        for (start, _) in chapter.text.match_indices(query) {
            if output.hits.len() >= cap {
                output.truncated = true;
                return output;
            }
            let Some(block) = chapter
                .spans
                .binary_search_by(|(block_start, block_end)| {
                    if *block_end <= start {
                        std::cmp::Ordering::Less
                    } else if *block_start > start {
                        std::cmp::Ordering::Greater
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .ok()
            else {
                continue;
            };
            let (block_start, block_end) = chapter.spans[block];
            let local_start = start - block_start;
            let local_end = (start + query.len() - block_start).min(block_end - block_start);
            if local_end <= local_start {
                continue;
            }
            output.hits.push(SearchHit {
                chapter: chapter.chapter,
                block,
                range: local_start..local_end,
                snippet: snippet(&chapter.blocks[block], local_start, local_end),
            });
        }
        if cancel.is_cancelled() {
            break;
        }
    }
    output
}

/// A short, character-safe window around one match.
fn snippet(block: &str, start: usize, end: usize) -> String {
    const CONTEXT: usize = 48;
    let before = floor_boundary(block, start.saturating_sub(CONTEXT));
    let after = ceil_boundary(block, (end + CONTEXT).min(block.len()));
    let mut snippet = String::new();
    if before > 0 {
        snippet.push('…');
    }
    snippet.push_str(&block[before..after]);
    if after < block.len() {
        snippet.push('…');
    }
    snippet
}

fn floor_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index < value.len() && !value.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chapter(index: usize, blocks: &[&str]) -> ChapterText {
        let mut text = String::new();
        let mut spans = Vec::new();
        for block in blocks {
            if !text.is_empty() {
                text.push('\n');
            }
            let start = text.len();
            text.push_str(block);
            spans.push((start, text.len()));
        }
        ChapterText {
            chapter: index,
            text,
            spans,
            blocks: blocks.iter().map(|block| (*block).to_owned()).collect(),
        }
    }

    #[test]
    fn search_spans_chapters_and_maps_block_local_ranges() {
        let book = BookText {
            chapters: vec![
                chapter(0, &["First needle here", "nothing"]),
                chapter(1, &["tail", "needle-only-final-chapter-279"]),
            ],
            bytes: 0,
        };
        let output = search(
            &book,
            "needle",
            &Limits::default(),
            &lyrel_epub::CancellationToken::new(),
        );
        assert_eq!(output.hits.len(), 2);
        assert_eq!(output.hits[0].chapter, 0);
        assert_eq!(output.hits[0].block, 0);
        assert_eq!(output.hits[1].block, 1);
        assert_eq!(&output.hits[1].snippet, "needle-only-final-chapter-279");
        assert!(!output.truncated);
    }

    #[test]
    fn unicode_search_preserves_match_and_bounded_context() {
        let query = "唯一😀needle";
        let block = format!("{}{query}{}", "文😀".repeat(80), "字😀".repeat(80));
        let book = BookText {
            chapters: vec![chapter(0, &[&block])],
            bytes: block.len(),
        };
        let output = search(
            &book,
            query,
            &Limits::default(),
            &lyrel_epub::CancellationToken::new(),
        );
        let [hit] = output.hits.as_slice() else {
            panic!("the full Unicode query must identify exactly one passage");
        };
        assert_eq!(&block[hit.range.clone()], query);
        assert!(hit.snippet.contains(query));
        assert!(hit.snippet.len() < block.len());
        assert!(!output.truncated);
    }
}
