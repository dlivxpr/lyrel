//! Derived visible text shared by find and the OS spelling adapter.
use super::*;

pub(crate) struct SearchSurfaceChanged;
impl EventEmitter<SearchSurfaceChanged> for SourcePresentation {}

#[derive(Default)]
pub(crate) struct SearchText {
    pub(crate) text: Rope,
    fragments: Vec<(Range<usize>, Range<usize>)>,
}
impl SearchText {
    pub(super) fn new(source: &Rope, projection: &Projection, live: bool) -> Self {
        if !live {
            return Self {
                text: source.clone(),
                fragments: vec![(0..source.len(), 0..source.len())],
            };
        }
        let mut text = String::new();
        let mut fragments = Vec::new();
        for leaf in &projection.leaves {
            if leaf.deferred {
                if !text.is_empty() {
                    text.push('\n');
                }
                let start = text.len();
                text.push_str(&source.slice(leaf.range.clone()).to_string());
                fragments.push((start..text.len(), leaf.range.clone()));
                continue;
            }
            if leaf.text.is_empty() {
                continue;
            }
            // A structural gap is a separator, not editable source. In particular,
            // adjacent table cells must never become one searchable word.
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            let start = text.len();
            text.push_str(&leaf.text);
            fragments.extend(
                leaf.visible_fragments().map(|(display, source)| {
                    ((start + display.start)..(start + display.end), source)
                }),
            );
        }
        Self {
            text: text.into(),
            fragments,
        }
    }
    pub(crate) fn source_ranges(&self, range: Range<usize>) -> Vec<Range<usize>> {
        let mut ranges: Vec<Range<usize>> = Vec::new();
        let first = self
            .fragments
            .partition_point(|(visible, _)| visible.end <= range.start);
        for (visible, source) in self.fragments[first..]
            .iter()
            .take_while(|(visible, _)| visible.start < range.end)
        {
            let start = range.start.max(visible.start);
            let end = range.end.min(visible.end);
            if start >= end {
                continue;
            }
            let mapped = if visible.len() == source.len() {
                (source.start + start - visible.start)..(source.start + end - visible.start)
            } else {
                source.clone()
            };
            if let Some(last) = ranges.last_mut().filter(|last| last.end == mapped.start) {
                last.end = mapped.end;
            } else {
                ranges.push(mapped);
            }
        }
        ranges
    }
    pub(crate) fn visible_offset(&self, source: usize) -> usize {
        let ix = self
            .fragments
            .partition_point(|(_, original)| original.end <= source);
        self.fragments
            .get(ix)
            .map_or(self.text.len(), |(visible, original)| {
                visible.start
                    + if visible.len() == original.len() {
                        source.saturating_sub(original.start)
                    } else {
                        0
                    }
            })
    }
    pub(crate) fn replacement(
        &self,
        range: Range<usize>,
        replacement: &str,
    ) -> Vec<(Range<usize>, String)> {
        self.source_ranges(range)
            .into_iter()
            .enumerate()
            .map(|(ix, range)| {
                (
                    range,
                    if ix == 0 {
                        replacement.to_owned()
                    } else {
                        String::new()
                    },
                )
            })
            .collect()
    }
}

impl SourcePresentation {
    pub(super) fn tools_changed(&mut self, cx: &mut Context<Self>) {
        self.spelling.surface_generation = self.spelling.surface_generation.wrapping_add(1);
        self.schedule_spelling(cx);
        cx.emit(SearchSurfaceChanged);
    }
    pub(crate) fn search_text(&self, cx: &App) -> SearchText {
        SearchText::new(self.document.read(cx).source(), &self.projection, self.live)
    }
    pub(crate) fn search_surface_key(&self, cx: &App) -> (u64, u64, bool) {
        (
            self.document.read(cx).revision(),
            self.spelling.surface_generation,
            self.live,
        )
    }
}
