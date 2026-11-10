//! Host-facing resource intents. This module performs no file or network I/O.
use super::*;
pub use zorite_markdown::syntax::LinkHit;

// Insert a local image through the owning workbench Host.
actions!(lyrel_markdown_resources, [InsertImage]);

impl EditorState {
    /// A Host bitmap changed without changing source/history/revision.
    pub fn refresh_resource_layout(&mut self, cx: &mut Context<Self>) {
        self.invalidate_geometry();
        self.shape_caches.line_heights.borrow_mut().clear();
        cx.notify();
    }
    /// Image references using the same grammar as the native image renderer.
    /// Fenced code is not a resource request. Duplicate sources share a load.
    pub fn image_sources(&self) -> Vec<String> {
        let scan = self.scan_data();
        let mut sources = Vec::new();
        for (row, line) in self.content.lines().enumerate() {
            if scan.fence_odd.get(row).copied().unwrap_or(false) {
                continue;
            }
            for (_, range) in markdown_syntax::inline_image_spans(line) {
                let src = &line[range];
                if !sources.iter().any(|s| s == src) {
                    sources.push(src.to_owned());
                }
            }
        }
        sources
    }

    /// Current painted bounds of a block image, for Host previews and native hit-testing.
    pub fn image_bounds(&self, source: &str) -> Option<Bounds<Pixels>> {
        self.image_rects.iter().find_map(|(row, bounds)| {
            let start = *self.line_starts().get(*row)?;
            let (src, ..) = markdown_syntax::image_row(&self.content[start..self.line_end(*row)])?;
            (src == source).then_some(*bounds)
        })
    }

    pub(super) fn image_source_at(&self, position: Point<Pixels>) -> Option<SharedString> {
        self.inline_image_at(position).or_else(|| {
            let row = self
                .image_rects
                .iter()
                .find_map(|(row, rect)| rect.contains(&position).then_some(*row))?;
            let start = *self.line_starts().get(row)?;
            let (src, ..) = markdown_syntax::image_row(&self.content[start..self.line_end(row)])?;
            Some(src.to_owned().into())
        })
    }
}
