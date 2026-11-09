//! Source-preserving clipboard payloads. HTML is an inert, allowlisted projection
//! of positioned syntax, never a browser/MDX renderer or a resource loader.
use super::super::{Command, EditKind, Selection, parser};
use super::SourcePresentation;
use super::formats::{self, Decoration, Format};
use crate::ui::i18n::{L10nKey, t};
use gpui_kit::base::input::RopeExt as _;
use gpui_kit::{ClipboardEntry, ClipboardItem, Context, Window};
use markdown::mdast::Node;
use std::ops::Range;

gpui_kit::actions!(
    markdown_clipboard,
    [CopyMarkdownOnly, CopyContents, CopyContentsMarkdown]
);

fn escaped(text: &str, out: &mut String) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
}

/// The original line endings belong to the document, not its LF coordinates.
fn append_source(
    source: &super::Rope,
    endings: &[crate::ui::editor_text::LineEnding],
    range: Range<usize>,
    out: &mut String,
) {
    let row = source.offset_to_point(range.start).row;
    let mut endings = endings[row..].iter();
    for chunk in source.slice(range).chunks() {
        for part in chunk.split_inclusive('\n') {
            if let Some(line) = part.strip_suffix('\n') {
                out.push_str(line);
                if endings.next() == Some(&crate::ui::editor_text::LineEnding::CrLf) {
                    out.push('\r');
                }
                out.push('\n');
            } else {
                out.push_str(part);
            }
        }
    }
}

struct Html<'a> {
    source: &'a str,
    decorations: Vec<Decoration>,
    definitions: super::super::images::Definitions,
    images: &'a std::collections::HashMap<String, super::super::images::State>,
    math: &'a std::collections::HashMap<super::super::math::Formula, super::super::math::Output>,
    properties: super::properties::Index,
}
impl Html<'_> {
    fn image(&self, image: &gpui_kit::RenderImage, label: &str, out: &mut String) -> bool {
        use base64::Engine as _;
        use image::ImageEncoder as _;
        let Some(bytes) = image.as_bytes(0) else {
            return false;
        };
        let size = image.size(0);
        let (Ok(width), Ok(height)) = (u32::try_from(size.width.0), u32::try_from(size.height.0))
        else {
            return false;
        };
        let mut rgba = bytes.to_vec();
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let mut png = Vec::new();
        if image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&rgba, width, height, image::ExtendedColorType::Rgba8)
            .is_err()
        {
            return false;
        }
        out.push_str("<img alt=\"");
        escaped(label, out);
        out.push_str("\" src=\"data:image/png;base64,");
        base64::engine::general_purpose::STANDARD.encode_string(png, out);
        out.push_str("\">");
        true
    }
    fn literal(&self, range: Range<usize>, out: &mut String) {
        escaped(&self.source[range], out);
    }
    fn text(&self, range: Range<usize>, out: &mut String) {
        // Split at the existing accepted format spans, not arbitrary HTML/CSS.
        let mut cuts = vec![range.start, range.end];
        for d in &self.decorations {
            cuts.extend(
                [d.open.start, d.open.end, d.close.start, d.close.end]
                    .into_iter()
                    .filter(|at| range.start < *at && *at < range.end),
            );
        }
        cuts.sort_unstable();
        cuts.dedup();
        for pair in cuts.windows(2) {
            let part = pair[0]..pair[1];
            if self.decorations.iter().any(|d| {
                (d.open.start <= part.start && part.end <= d.open.end)
                    || (d.close.start <= part.start && part.end <= d.close.end)
            }) {
                continue;
            }
            let active: Vec<_> = self
                .decorations
                .iter()
                .filter(|d| d.open.end <= part.start && part.end <= d.close.start)
                .collect();
            for d in &active {
                out.push_str(&style_tag(d));
            }
            self.literal(part, out);
            for _ in active {
                out.push_str("</span>");
            }
        }
    }
    fn node(&self, node: &Node, selected: &Range<usize>, out: &mut String) {
        let Some(position) = node.position() else {
            return;
        };
        let owner = position.start.offset..position.end.offset;
        let range = owner.start.max(selected.start)..owner.end.min(selected.end);
        if range.is_empty() {
            return;
        }
        if range == owner {
            if let Some(image) = super::super::images::image(node, &self.definitions) {
                if let Some(super::super::images::State::Ready(pixels)) =
                    self.images.get(&image.url)
                {
                    if self.image(pixels, &image.alt, out) {
                        return;
                    }
                }
            }
            if let Some(formula) =
                super::super::math::Formula::from_node(node, &self.source[owner.clone()])
            {
                if let Some(super::super::math::Output::Ready { image, .. }) =
                    self.math.get(&formula)
                {
                    if self.image(image, &formula.tex, out) {
                        return;
                    }
                }
            }
        }
        if matches!(node, Node::Paragraph(_)) {
            let rows: Vec<_> = self
                .properties
                .clipboard_rows()
                .filter(|(line, _, _)| owner.start <= line.start && line.start < owner.end)
                .collect();
            if !rows.is_empty()
                && rows[0].0.start == owner.start
                && rows
                    .last()
                    .is_some_and(|(line, _, _)| line.end >= owner.end)
            {
                out.push_str("<dl>");
                for (line, key, value) in rows {
                    if line.start >= range.end || range.start >= line.end {
                        continue;
                    }
                    out.push_str("<dt>");
                    let key = key.start.max(range.start)..key.end.min(range.end);
                    if key.start < key.end {
                        self.text(key, out);
                    }
                    out.push_str("</dt><dd>");
                    let value = value.start.max(range.start)..value.end.min(range.end);
                    if value.start < value.end {
                        self.text(value, out);
                    }
                    out.push_str("</dd>");
                }
                out.push_str("</dl>");
                return;
            }
        }
        let tag = match node {
            Node::Root(_) => "",
            Node::Paragraph(_) => "p",
            Node::Blockquote(_) => "blockquote",
            Node::Heading(h) => match h.depth {
                1 => "h1",
                2 => "h2",
                3 => "h3",
                4 => "h4",
                5 => "h5",
                _ => "h6",
            },
            Node::Strong(_) => "strong",
            Node::Emphasis(_) => "em",
            Node::Delete(_) => "del",
            Node::List(list) => {
                if list.ordered {
                    "ol"
                } else {
                    "ul"
                }
            }
            Node::ListItem(_) => "li",
            Node::Table(_) => "table",
            Node::TableRow(_) => "tr",
            Node::TableCell(_) => "td",
            Node::Link(_) | Node::LinkReference(_) => "span", // Never attach an executable/resource URL.
            Node::Text(text) => {
                let has_delimiter = self.decorations.iter().any(|d| {
                    (d.open.start < range.end && range.start < d.open.end)
                        || (d.close.start < range.end && range.start < d.close.end)
                });
                if range == owner && !has_delimiter {
                    let active: Vec<_> = self
                        .decorations
                        .iter()
                        .filter(|d| d.open.end <= range.start && range.end <= d.close.start)
                        .collect();
                    for d in &active {
                        out.push_str(&style_tag(d));
                    }
                    escaped(&text.value, out);
                    for _ in active {
                        out.push_str("</span>");
                    }
                } else {
                    self.text(range, out);
                }
                return;
            }
            Node::Break(_) => {
                out.push_str("<br>");
                return;
            }
            Node::ThematicBreak(_) => {
                out.push_str("<hr>");
                return;
            }
            Node::Definition(_) => return,
            Node::MdxJsxTextElement(_) | Node::MdxJsxFlowElement(_)
                if formats::wrapper(self.source, node).is_some() =>
            {
                ""
            }
            Node::Html(_) => {
                if self.source[owner].trim_start().starts_with("<!--") {
                    return;
                }
                self.text(range, out);
                return;
            }
            Node::InlineCode(_) | Node::InlineMath(_) => {
                out.push_str("<code>");
                // Whole atoms can use their decoded body; partial atoms retain only selected source.
                if range == owner {
                    escaped(&node.to_string(), out);
                } else {
                    self.literal(range, out);
                }
                out.push_str("</code>");
                return;
            }
            Node::Image(image) => {
                if range != owner {
                    self.literal(range, out);
                    return;
                }
                out.push_str("<span role=\"img\" aria-label=\"");
                escaped(&image.alt, out);
                out.push_str("\">[");
                escaped(&image.alt, out);
                out.push_str("] ");
                escaped(&image.url, out);
                out.push_str("</span>");
                return;
            }
            Node::Code(_) | Node::Math(_) | Node::Yaml(_) | Node::Toml(_) => {
                out.push_str("<pre><code>");
                if range == owner {
                    escaped(&node.to_string(), out);
                } else {
                    self.literal(range, out);
                }
                out.push_str("</code></pre>");
                return;
            }
            // Unknown components/expressions/embeds remain visibly inert source.
            _ => {
                out.push_str("<code>");
                self.literal(range, out);
                out.push_str("</code>");
                return;
            }
        };
        if !tag.is_empty() {
            out.push('<');
            out.push_str(tag);
            if let Node::List(list) = node {
                if let Some(start) = list.start {
                    out.push_str(&format!(" start=\"{start}\""));
                }
            }
            // Each list item retains its own original ordered marker, including nonsequential numbers.
            if matches!(node, Node::ListItem(_)) {
                let prefix = &self.source[owner.start..owner.end];
                let digits = prefix.bytes().take_while(u8::is_ascii_digit).count();
                if digits > 0 && matches!(prefix.as_bytes().get(digits), Some(b'.' | b')')) {
                    out.push_str(" value=\"");
                    out.push_str(&prefix[..digits]);
                    out.push('"');
                }
            }
            out.push('>');
        }
        if let Some(children) = node.children() {
            for child in children {
                self.node(child, selected, out);
            }
        }
        if !tag.is_empty() {
            out.push_str("</");
            out.push_str(tag);
            out.push('>');
        }
    }
}
fn style_tag(d: &Decoration) -> String {
    let css = match d.format {
        Format::Underline => "text-decoration:underline".into(),
        Format::Highlight => "background-color:yellow".into(),
        Format::TextColor if d.color.is_none() => "color:inherit".into(),
        Format::TextColor | Format::HighlightColor => {
            let color = d.color.unwrap_or(0);
            format!(
                "{}:rgba({},{},{},{})",
                if d.format == Format::TextColor {
                    "color"
                } else {
                    "background-color"
                },
                color >> 24,
                color >> 16 & 255,
                color >> 8 & 255,
                (color & 255) as f32 / 255.
            )
        }
        _ => String::new(),
    };
    format!("<span style=\"{css}\">")
}

impl SourcePresentation {
    fn clipboard_ranges(&self, cx: &gpui_kit::App) -> Vec<Range<usize>> {
        let doc = self.document.read(cx);
        let mut ranges: Vec<_> = std::iter::once(self.state.selection)
            .chain(self.extra_selections.iter().copied())
            .map(Selection::range)
            .filter(|range| !range.is_empty())
            .collect();
        for range in &mut ranges {
            if !self.live
                || self.projection.revision != doc.revision()
                || !doc
                    .source()
                    .slice(range.clone())
                    .chars()
                    .any(|ch| ch == '\n')
            {
                continue;
            }
            if let Some(leaf) = self.projection.leaf_at(range.start).filter(|leaf| {
                matches!(leaf.kind, super::Kind::List(_) | super::Kind::Quote(_))
                    && leaf.content_start() == range.start
            }) {
                range.start = doc
                    .source()
                    .line_start_offset(doc.source().offset_to_point(leaf.range.start).row);
            }
        }
        ranges.sort_by_key(|range| range.start);
        let mut merged: Vec<Range<usize>> = Vec::new();
        for range in ranges {
            if let Some(last) = merged.last_mut().filter(|last| range.start <= last.end) {
                last.end = last.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        merged
    }
    pub(super) fn copy_contents(
        &mut self,
        plain: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_composing() {
            self.command(Command::FinishComposition, window, cx);
        }
        let end = self.document.read(cx).source().len();
        self.copy_markdown_ranges(vec![0..end], false, plain, window, cx);
    }
    pub(super) fn copy_markdown(
        &mut self,
        cut: bool,
        plain: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.formula_copy(cut, window, cx) {
            return;
        }
        let ranges = self.clipboard_ranges(cx);
        self.copy_markdown_ranges(ranges, cut, plain, window, cx);
    }
    pub(super) fn copy_table_markdown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cell) = self.active_table() else {
            return;
        };
        if self.projection.revision != self.document.read(cx).revision() {
            return;
        }
        let mut range = cell.table();
        if let Some(marker) =
            super::super::tables::metadata::read(self.document.read(cx).source(), range.start)
        {
            range.start = marker.range().start;
        }
        self.copy_markdown_ranges(vec![range], false, false, window, cx);
    }
    fn copy_markdown_ranges(
        &mut self,
        ranges: Vec<Range<usize>>,
        cut: bool,
        plain: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ranges.is_empty() {
            return;
        }
        let doc = self.document.read(cx);
        let source = doc.source().clone();
        let endings = doc.endings.clone();
        let grammar = doc.grammar();
        let resources = doc.resources().cloned();
        let images = doc.images.states.clone();
        let math = self.math.outputs.clone();
        let token = self.input_token(cx);
        let selection = self.state.selection;
        let extra = self.extra_selections.clone();
        self.clipboard_generation = self.clipboard_generation.wrapping_add(1);
        let generation = self.clipboard_generation;
        let selected_ranges = ranges.clone();
        self.clipboard_task = Some(cx.spawn_in(window, async move |view, cx| {
            let (text, html) = cx
                .background_executor()
                .spawn(async move {
                    let mut text = String::new();
                    for (ix, range) in selected_ranges.iter().enumerate() {
                        if ix != 0 {
                            text.push('\n');
                        }
                        append_source(&source, &endings, range.clone(), &mut text);
                    }
                    let html = if plain {
                        None
                    } else {
                        let source = source.to_string();
                        let mut out = String::new();
                        match parser::parse(&source, grammar, true) {
                            Ok(root) => {
                                let renderer = Html {
                                    decorations: formats::decorations(&source, &root),
                                    definitions: super::super::images::definitions(&root),
                                    images: &images,
                                    math: &math,
                                    properties: super::properties::Index::from_ast(&source, &root),
                                    source: &source,
                                };
                                for (ix, range) in selected_ranges.iter().enumerate() {
                                    if ix != 0 {
                                        out.push_str("<br>");
                                    }
                                    renderer.node(&root, range, &mut out);
                                }
                            }
                            Err(_) => {
                                out.push_str("<pre>");
                                escaped(&text, &mut out);
                                out.push_str("</pre>");
                            }
                        }
                        Some(out)
                    };
                    (text, html)
                })
                .await;
            let _ = view.update_in(cx, |view, window, cx| {
                if view.clipboard_generation != generation {
                    return;
                }
                let current_resources = view.document.read(cx).resources();
                let same_resources = resources
                    .as_ref()
                    .map_or(current_resources.is_none(), |expected| {
                        expected.is_current(current_resources)
                    });
                if view.input_token(cx) != token
                    || view.document.read(cx).grammar() != grammar
                    || view.state.selection != selection
                    || view.extra_selections != extra
                    || !same_resources
                    || !view.focus.contains_focused(window, cx)
                {
                    view.clipboard_task = None;
                    return;
                }
                view.publish_clipboard(text, html, ranges, cut, window, cx);
                view.clipboard_task = None;
            });
        }));
    }
    fn publish_clipboard(
        &mut self,
        text: String,
        html: Option<String>,
        ranges: Vec<Range<usize>>,
        cut: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Both platform adapters publish the same completed payload.
        // metadata exposes HTML to consumers without touching the user's OS clipboard.
        #[cfg(test)]
        cx.write_to_clipboard(html.as_ref().map_or_else(
            || ClipboardItem::new_string(text.clone()),
            |html| ClipboardItem::new_string_with_metadata(text.clone(), html.clone()),
        ));
        self.clipboard_status = None;
        #[cfg(not(test))]
        {
            let fallback = t(L10nKey::MarkdownClipboardPlainFallback);
            if let Some(html) = html {
                // arboard's contract permits plain-only Ok. Windows can inspect
                // availability without reading or rewriting the clipboard.
                let result = arboard::Clipboard::new().and_then(|mut clipboard| {
                    clipboard.set_html(html.as_str(), Some(text.as_str()))
                });
                if let Err(error) = result {
                    log::warn!("Rich Markdown clipboard publication failed: {error}");
                    if let Err(error) = arboard::Clipboard::new()
                        .and_then(|mut clipboard| clipboard.set_text(text.as_str()))
                    {
                        log::warn!("Plain Markdown clipboard publication failed: {error}");
                        self.clipboard_status = Some(t(L10nKey::MarkdownClipboardFailed).into());
                        cx.notify();
                        return; // Cut must never delete text whose publication failed.
                    }
                    self.clipboard_status = Some(fallback.into());
                }
                #[cfg(target_os = "windows")]
                if self.clipboard_status.is_none() {
                    use windows::Win32::System::DataExchange::{
                        IsClipboardFormatAvailable, RegisterClipboardFormatW,
                    };
                    let format =
                        unsafe { RegisterClipboardFormatW(windows::core::w!("HTML Format")) };
                    if format == 0 || unsafe { IsClipboardFormatAvailable(format) }.is_err() {
                        // Another app may have changed it. Do not overwrite its clipboard.
                        self.clipboard_status = Some(fallback.into());
                    }
                }
                #[cfg(target_os = "macos")]
                if self.clipboard_status.is_none() {
                    self.clipboard_status =
                        Some(t(L10nKey::MarkdownClipboardRichUnconfirmed).into());
                }
            } else if let Err(error) = arboard::Clipboard::new()
                .and_then(|mut clipboard| clipboard.set_text(text.as_str()))
            {
                log::warn!("Plain Markdown clipboard publication failed: {error}");
                self.clipboard_status = Some(t(L10nKey::MarkdownClipboardFailed).into());
                cx.notify();
                return;
            }
        }
        if cut {
            let at = ranges[0].start;
            self.command(
                Command::BatchReplaceSelected {
                    kind: EditKind::Command,
                    edits: ranges
                        .into_iter()
                        .map(|range| (range, String::new()))
                        .collect(),
                    selection: Selection::caret(at),
                },
                window,
                cx,
            );
            self.extra_selections.clear();
        }
        cx.notify();
    }
    pub(super) fn paste_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        // Keep CRLF in the input: SourceText records its original line spelling.
        self.command(
            Command::Replace {
                range: self.state.selection.range(),
                text,
                kind: EditKind::Paste,
            },
            window,
            cx,
        );
    }
    pub(super) fn clipboard_menu(
        &self,
        menu: gpui_kit::component::menu::PopupMenu,
    ) -> gpui_kit::component::menu::PopupMenu {
        menu.menu_with_disabled(
            t(L10nKey::MarkdownCopyMarkdownOnly),
            Box::new(CopyMarkdownOnly),
            self.state.selection.range().is_empty() && self.extra_selections.is_empty(),
        )
        .separator()
        .menu(t(L10nKey::MarkdownCopyContents), Box::new(CopyContents))
        .menu(
            t(L10nKey::MarkdownCopyContentsMarkdown),
            Box::new(CopyContentsMarkdown),
        )
    }
}

/// External path text is not a text flavor. Screenshots/files are considered
/// only when no actual Markdown/plain string entry is present.
pub(super) fn plain_text(item: &ClipboardItem) -> Option<String> {
    let mut text = String::new();
    let mut found = false;
    for entry in item.entries() {
        if let ClipboardEntry::String(value) = entry {
            found = true;
            text.push_str(&value.text);
        }
    }
    found.then_some(text)
}
