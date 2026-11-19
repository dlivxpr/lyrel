//! Sanitize one chapter body into bounded, virtualizable block fragments.
//!
//! The allowlist, dropped active elements, `img` collection, `id` anchors and
//! depth/output ceilings come from Glim `books/mod.rs::sanitize`. Glim emits
//! one HTML string; Kit's HTML parser puts that whole string in one `Root`
//! block, so a 3000-paragraph chapter lays out at once. Lyrel splits the same
//! sanitized structure into top-level fragments (one `TextView` each), keeping
//! HTML structure rather than converting to Markdown or plain text. Each
//! fragment also retains its normalized text, so search never reads rendered
//! layout and `<mark>` can be injected at exact text offsets.
//!
//! Script/style/frame/form/audio/video subtrees are dropped. Only archive-local
//! `img` sources and archive-local `a` targets are kept; remote, `file:` and
//! data URLs become a visible placeholder image or lose their link.

use std::ops::Range;

use roxmltree::Node;

use super::archive::resolve;
use super::limits::Limits;
use super::{CancelToken, EpubError};

/// The source used for an image the book asked for but the reader will not
/// fetch. The UI resolves it to a visible placeholder and reports it, never a
/// transparent pixel or a network request.
pub const BLOCKED_IMAGE_SRC: &str = "lyrel-epub:blocked-resource";

/// One piece of a sanitized block: already-escaped markup or play text.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Html(String),
    Text(String),
}

/// A named location inside one block.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Anchor {
    pub id: String,
    /// Byte offset into [`BlockFragment::text`].
    pub text_offset: usize,
}

/// One sanitized top-level fragment, the unit a presentation virtualizes.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct BlockFragment {
    parts: Vec<Part>,
    /// The fragment's already-rendered HTML.
    pub html: String,
    /// Normalized visible text, used by full-book search.
    pub text: String,
    pub anchors: Vec<Anchor>,
    /// Archive-local image sources, in document order (duplicates retained).
    pub images: Vec<String>,
}

/// What the reader refused to carry out of a chapter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Omissions {
    pub scripts: usize,
    pub styles: usize,
    pub frames: usize,
    pub remote_images: usize,
    pub blocked_links: usize,
    pub unsupported_elements: usize,
    pub missing_images: usize,
}

impl Omissions {
    pub fn total(&self) -> usize {
        self.scripts
            + self.styles
            + self.frames
            + self.remote_images
            + self.blocked_links
            + self.unsupported_elements
            + self.missing_images
    }
}

/// Renders `block` with `<mark>` around the supplied text ranges.
///
/// Kit's HTML path has no range-highlight API and `range_for_source` is always
/// `None`, so this is the supported way to show a search hit. `color` should
/// be a `#RRGGBB`/`#RRGGBBAA` value; anything else falls back to yellow.
pub fn html_with_marks(block: &BlockFragment, ranges: &[Range<usize>], color: &str) -> String {
    if ranges.is_empty() {
        return block.html.clone();
    }
    let color = mark_color(color);
    let ranges = merge_ranges(block.text.len(), ranges);
    if ranges.is_empty() {
        return block.html.clone();
    }
    let mut output = String::with_capacity(block.html.len() + ranges.len() * 40);
    let mut position = 0usize;
    let mut range_ix = 0usize;
    let mut open_until: Option<usize> = None;
    for part in &block.parts {
        match part {
            Part::Html(html) => output.push_str(html),
            Part::Text(text) => {
                let mut local = 0usize;
                let event_end = position + text.len();
                while local < text.len() {
                    let absolute = position + local;
                    if open_until.is_some_and(|end| absolute >= end) {
                        output.push_str("</mark>");
                        open_until = None;
                    }
                    while open_until.is_none()
                        && range_ix < ranges.len()
                        && ranges[range_ix].start <= absolute
                    {
                        if absolute < ranges[range_ix].end {
                            output.push_str("<mark color=\"");
                            output.push_str(&color);
                            output.push_str("\">");
                            open_until = Some(ranges[range_ix].end);
                            break;
                        }
                        range_ix += 1;
                    }
                    let next = [
                        open_until.unwrap_or(usize::MAX),
                        ranges.get(range_ix).map_or(usize::MAX, |range| range.start),
                        event_end,
                    ]
                    .into_iter()
                    .filter(|boundary| *boundary > absolute)
                    .min()
                    .unwrap_or(event_end);
                    let take = next - absolute;
                    escape_text_into(&text[local..local + take], &mut output);
                    local += take;
                }
                position = event_end;
            }
        }
    }
    if open_until.is_some() {
        output.push_str("</mark>");
    }
    output
}

fn mark_color(value: &str) -> String {
    let valid = value.len() == 7 || value.len() == 9;
    if valid && value.starts_with('#') && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit()) {
        value.to_owned()
    } else {
        "yellow".into()
    }
}

fn merge_ranges(length: usize, ranges: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut sorted = ranges
        .iter()
        .filter(|range| range.start < range.end && range.start < length)
        .map(|range| range.start..range.end.min(length))
        .collect::<Vec<_>>();
    sorted.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(sorted.len());
    for range in sorted {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// Sanitizes the first `<body>` of a chapter XML document.
pub(crate) fn sanitize_chapter(
    document: &roxmltree::Document<'_>,
    href: &str,
    limits: &Limits,
    cancel: &CancelToken,
) -> Result<(Vec<BlockFragment>, Omissions), EpubError> {
    let body = descendants(document.root_element(), "body")
        .next()
        .unwrap_or_else(|| document.root_element());
    let mut sanitizer = Sanitizer {
        base: href,
        limits,
        cancel,
        omissions: Omissions::default(),
        blocks: Vec::new(),
        inline: None,
        pending_anchors: Vec::new(),
        total_html: 0,
        total_text: 0,
    };
    sanitizer.flatten_children(body, 0)?;
    sanitizer.flush_inline(true)?;
    Ok((sanitizer.blocks, sanitizer.omissions))
}

struct Sanitizer<'a> {
    base: &'a str,
    limits: &'a Limits,
    cancel: &'a CancelToken,
    omissions: Omissions,
    blocks: Vec<BlockFragment>,
    inline: Option<Builder>,
    pending_anchors: Vec<String>,
    total_html: usize,
    total_text: usize,
}

impl Sanitizer<'_> {
    fn flatten_children(&mut self, parent: Node<'_, '_>, depth: usize) -> Result<(), EpubError> {
        super::check_cancel(self.cancel)?;
        if depth > self.limits.html_depth {
            return Err(EpubError::limit(
                "HTML depth",
                self.limits.html_depth as u64,
            ));
        }
        for child in parent.children() {
            if child.is_text() {
                let text = child.text().unwrap_or_default();
                self.inline_mut().push_text(text, true);
                continue;
            }
            if !child.is_element() {
                continue;
            }
            let tag = child.tag_name().name();
            if is_dropped(tag) {
                self.omit_dropped(tag);
                continue;
            }
            if is_transparent(tag) {
                if let Some(id) = attribute(child, "id") {
                    self.pending_anchors.push(id.to_owned());
                }
                self.flush_inline(true)?;
                self.flatten_children(child, depth + 1)?;
                self.flush_inline(true)?;
                continue;
            }
            if is_block_tag(tag) || tag == "image" || tag == "img" {
                self.flush_inline(true)?;
                self.append_block(child, depth + 1, false)?;
            } else {
                let mut builder = self.inline.take().unwrap_or_else(|| {
                    let mut builder = Builder::default();
                    builder.push_html("<p>");
                    builder
                });
                builder.sanitize_element(self, child, depth + 1, false)?;
                self.inline = Some(builder);
            }
        }
        Ok(())
    }

    fn inline_mut(&mut self) -> &mut Builder {
        self.inline.get_or_insert_with(|| {
            let mut builder = Builder::default();
            builder.push_html("<p>");
            builder
        })
    }

    fn flush_inline(&mut self, wrap: bool) -> Result<(), EpubError> {
        let Some(mut builder) = self.inline.take() else {
            return Ok(());
        };
        if wrap {
            builder.push_html("</p>");
        }
        if builder.is_empty() {
            return Ok(());
        }
        self.finish_block(builder)
    }

    fn append_block(
        &mut self,
        node: Node<'_, '_>,
        depth: usize,
        pre: bool,
    ) -> Result<(), EpubError> {
        super::check_cancel(self.cancel)?;
        let mut builder = Builder::default();
        builder.sanitize_element(self, node, depth, pre)?;
        if builder.is_empty() {
            return Ok(());
        }
        self.finish_block(builder)
    }

    fn finish_block(&mut self, mut builder: Builder) -> Result<(), EpubError> {
        super::check_cancel(self.cancel)?;
        if self.blocks.len() >= self.limits.blocks_per_chapter {
            return Err(EpubError::limit(
                "chapter blocks",
                self.limits.blocks_per_chapter as u64,
            ));
        }
        builder.trim();
        let html_len = builder.rendered_len();
        if html_len > self.limits.block_bytes {
            return Err(EpubError::limit(
                "block bytes",
                self.limits.block_bytes as u64,
            ));
        }
        self.total_html = self.total_html.saturating_add(html_len);
        if self.total_html > self.limits.chapter_html_bytes {
            return Err(EpubError::limit(
                "chapter HTML bytes",
                self.limits.chapter_html_bytes as u64,
            ));
        }
        self.total_text = self.total_text.saturating_add(builder.text.len());
        if self.total_text > self.limits.chapter_html_bytes {
            return Err(EpubError::limit(
                "chapter text bytes",
                self.limits.chapter_html_bytes as u64,
            ));
        }
        let html = builder.render_html();
        let anchors = builder
            .anchors
            .iter()
            .map(|(id, offset)| Anchor {
                id: id.clone(),
                text_offset: *offset,
            })
            .collect::<Vec<_>>();
        let mut fragment = BlockFragment {
            parts: std::mem::take(&mut builder.parts),
            html,
            text: std::mem::take(&mut builder.text),
            anchors,
            images: std::mem::take(&mut builder.images),
        };
        for id in self.pending_anchors.drain(..) {
            fragment.anchors.push(Anchor { id, text_offset: 0 });
        }
        self.blocks.push(fragment);
        Ok(())
    }

    fn omit_dropped(&mut self, tag: &str) {
        match tag {
            "script" => self.omissions.scripts += 1,
            "style" => self.omissions.styles += 1,
            "iframe" | "object" | "embed" => self.omissions.frames += 1,
            _ => self.omissions.unsupported_elements += 1,
        }
    }
}

#[derive(Default)]
struct Builder {
    parts: Vec<Part>,
    text: String,
    anchors: Vec<(String, usize)>,
    images: Vec<String>,
    last_space: bool,
    /// A `<pre>` fragment keeps its own whitespace instead of collapsing it.
    preserve: bool,
}

impl Builder {
    fn push_html(&mut self, html: &str) {
        match self.parts.last_mut() {
            Some(Part::Html(last)) => last.push_str(html),
            _ => self.parts.push(Part::Html(html.to_owned())),
        }
    }

    fn push_text(&mut self, raw: &str, collapse: bool) {
        let mut normalized = String::with_capacity(raw.len());
        let mut has_text = !self.text.trim().is_empty();
        for character in raw.chars() {
            if collapse && character.is_whitespace() {
                if !self.last_space && has_text {
                    normalized.push(' ');
                    self.last_space = true;
                }
            } else {
                normalized.push(character);
                has_text = true;
                self.last_space = character.is_whitespace();
            }
        }
        if normalized.is_empty() {
            return;
        }
        self.text.push_str(&normalized);
        match self.parts.last_mut() {
            Some(Part::Text(last)) => last.push_str(&normalized),
            _ => self.parts.push(Part::Text(normalized)),
        }
    }

    fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.images.is_empty() && self.rendered_len() == 0
    }

    fn trim(&mut self) {
        if self.preserve {
            self.parts
                .retain(|part| !matches!(part, Part::Text(text) if text.is_empty()));
            self.last_space = self.text.ends_with(' ');
            return;
        }
        while self.text.ends_with(' ') {
            self.text.pop();
            if !trim_last_text(&mut self.parts) {
                break;
            }
        }
        while self.text.starts_with(' ') {
            self.text.remove(0);
            if !trim_first_text(&mut self.parts) {
                break;
            }
        }
        if self.text.is_empty() {
            self.parts
                .retain(|part| !matches!(part, Part::Text(text) if text.is_empty()));
        }
        self.last_space = self.text.ends_with(' ');
    }

    fn rendered_len(&self) -> usize {
        self.parts
            .iter()
            .map(|part| match part {
                Part::Html(html) => html.len(),
                Part::Text(text) => escaped_len(text),
            })
            .sum()
    }

    fn render_html(&self) -> String {
        let mut output = String::with_capacity(self.rendered_len());
        for part in &self.parts {
            match part {
                Part::Html(html) => output.push_str(html),
                Part::Text(text) => escape_text_into(text, &mut output),
            }
        }
        output
    }

    fn sanitize_element(
        &mut self,
        owner: &mut Sanitizer<'_>,
        node: Node<'_, '_>,
        depth: usize,
        pre: bool,
    ) -> Result<(), EpubError> {
        super::check_cancel(owner.cancel)?;
        if depth > owner.limits.html_depth {
            return Err(EpubError::limit(
                "HTML depth",
                owner.limits.html_depth as u64,
            ));
        }
        if node.is_text() {
            self.push_text(node.text().unwrap_or_default(), !pre);
            return Ok(());
        }
        if !node.is_element() {
            return Ok(());
        }
        let raw_tag = node.tag_name().name();
        if is_dropped(raw_tag) {
            owner.omit_dropped(raw_tag);
            return Ok(());
        }
        let tag = allowed_tag(raw_tag);
        if tag == "div" && !is_allowed_tag(raw_tag) {
            owner.omissions.unsupported_elements += 1;
        }
        self.push_html("<");
        self.push_html(tag);
        if let Some(id) = attribute(node, "id") {
            self.push_html(" id=\"");
            push_attribute(id, &mut self.parts);
            self.push_html("\"");
            self.anchors.push((id.to_owned(), self.text.len()));
        }
        if tag == "a" {
            if let Some(href) = attribute(node, "href") {
                match resolve(owner.base, href) {
                    Some(resolved) => {
                        self.push_html(" href=\"");
                        push_attribute(&resolved, &mut self.parts);
                        self.push_html("\"");
                    }
                    None => owner.omissions.blocked_links += 1,
                }
            }
        }
        if tag == "img" {
            let reference = attribute(node, "src").or_else(|| attribute(node, "href"));
            let resolved = reference.and_then(|href| resolve(owner.base, href));
            let (source, blocked) = match resolved {
                Some(source) => (source, false),
                None => (BLOCKED_IMAGE_SRC.to_owned(), true),
            };
            if blocked {
                owner.omissions.remote_images += 1;
            }
            if self.images.len() >= owner.limits.images_per_chapter {
                return Err(EpubError::limit(
                    "chapter images",
                    owner.limits.images_per_chapter as u64,
                ));
            }
            self.images.push(source.clone());
            self.push_html(" src=\"");
            push_attribute(&source, &mut self.parts);
            self.push_html("\"");
            if let Some(alt) = attribute(node, "alt") {
                self.push_html(" alt=\"");
                push_attribute(alt, &mut self.parts);
                self.push_html("\"");
            }
        }
        if matches!(tag, "td" | "th") {
            for (name, max) in [("colspan", 1000), ("rowspan", 65534)] {
                if let Some(value) = attribute(node, name)
                    && value
                        .parse::<u32>()
                        .is_ok_and(|number| number >= 1 && number <= max)
                {
                    self.push_html(" ");
                    self.push_html(name);
                    self.push_html("=\"");
                    self.push_html(value);
                    self.push_html("\"");
                }
            }
        }
        self.push_html(">");
        if matches!(tag, "br" | "hr" | "img") {
            return Ok(());
        }
        let pre = pre || raw_tag == "pre";
        if raw_tag == "pre" {
            self.preserve = true;
        }
        for child in node.children() {
            self.sanitize_element(owner, child, depth + 1, pre)?;
        }
        self.push_html("</");
        self.push_html(tag);
        self.push_html(">");
        Ok(())
    }
}

fn trim_last_text(parts: &mut [Part]) -> bool {
    for part in parts.iter_mut().rev() {
        if let Part::Text(text) = part {
            if text.ends_with(' ') {
                text.pop();
                return true;
            }
            if !text.is_empty() {
                return false;
            }
        }
    }
    false
}

fn trim_first_text(parts: &mut [Part]) -> bool {
    for part in parts.iter_mut() {
        if let Part::Text(text) = part {
            if text.starts_with(' ') {
                text.remove(0);
                return true;
            }
            if !text.is_empty() {
                return false;
            }
        }
    }
    false
}

fn push_attribute(value: &str, parts: &mut Vec<Part>) {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            other => escaped.push(other),
        }
    }
    match parts.last_mut() {
        Some(Part::Html(last)) => last.push_str(&escaped),
        _ => parts.push(Part::Html(escaped)),
    }
}

fn escape_text_into(text: &str, output: &mut String) {
    for character in text.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            other => output.push(other),
        }
    }
}

fn escaped_len(text: &str) -> usize {
    text.chars()
        .map(|character| match character {
            '&' | '<' | '>' => 5,
            _ => character.len_utf8(),
        })
        .sum()
}

fn attribute<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|attribute| attribute.name() == name)
        .map(|attribute| attribute.value())
}

fn descendants<'a, 'input>(
    node: Node<'a, 'input>,
    name: &str,
) -> impl Iterator<Item = Node<'a, 'input>> {
    let name = name.to_owned();
    node.descendants()
        .filter(move |node| node.is_element() && node.tag_name().name() == name)
}

fn is_dropped(tag: &str) -> bool {
    matches!(
        tag,
        "script"
            | "style"
            | "iframe"
            | "object"
            | "embed"
            | "form"
            | "input"
            | "head"
            | "link"
            | "audio"
            | "video"
    )
}

fn is_transparent(tag: &str) -> bool {
    matches!(
        tag,
        "div" | "section" | "article" | "main" | "header" | "footer" | "aside" | "nav"
    )
}

fn is_block_tag(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "blockquote"
            | "ul"
            | "ol"
            | "li"
            | "table"
            | "pre"
            | "hr"
            | "br"
            | "figure"
            | "figcaption"
            | "dl"
            | "dt"
            | "dd"
    )
}

fn is_allowed_tag(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "div"
            | "span"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "em"
            | "i"
            | "strong"
            | "b"
            | "blockquote"
            | "ul"
            | "ol"
            | "li"
            | "table"
            | "thead"
            | "tbody"
            | "tr"
            | "td"
            | "th"
            | "pre"
            | "code"
            | "a"
            | "img"
            | "br"
            | "hr"
            | "sup"
            | "sub"
            | "s"
            | "u"
            | "del"
            | "mark"
            | "small"
            | "figure"
            | "figcaption"
            | "dl"
            | "dt"
            | "dd"
    )
}

fn allowed_tag(tag: &str) -> &str {
    match tag {
        "image" => "img",
        tag if is_allowed_tag(tag) => tag,
        _ => "div",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(body: &str) -> String {
        format!("<html><body>{body}</body></html>")
    }

    fn sanitize(body: &str) -> (Vec<BlockFragment>, Omissions) {
        let text = document(body);
        let document = roxmltree::Document::parse(&text).unwrap();
        sanitize_chapter(
            &document,
            "OPS/Text/one.xhtml",
            &Limits::default(),
            &CancelToken::new(),
        )
        .unwrap()
    }

    #[test]
    fn paragraphs_become_blocks_and_resources_are_archive_local() {
        let (blocks, omissions) = sanitize(
            r#"<div><h1 id="start">One</h1><p>Hello <em>reader</em>.</p><img src="../Images/a.png"/><img src="https://example.com/x.png"/><a href="two.xhtml#x">next</a><a href="file:///etc/passwd">bad</a><script>bad()</script></div>"#,
        );
        assert_eq!(blocks.len(), 5);
        assert_eq!(blocks[0].text, "One");
        assert_eq!(blocks[1].text, "Hello reader.");
        assert!(blocks[1].html.contains("<em>reader</em>"));
        assert_eq!(blocks[0].anchors[0].id, "start");
        assert_eq!(blocks[2].images, ["OPS/Images/a.png"]);
        assert_eq!(blocks[3].images, [BLOCKED_IMAGE_SRC]);
        assert!(blocks[4].html.contains("OPS/Text/two.xhtml#x"));
        assert!(!blocks[4].html.contains("file:"));
        assert_eq!(omissions.scripts, 1);
        assert_eq!(omissions.remote_images, 1);
        assert_eq!(omissions.blocked_links, 1);
    }

    #[test]
    fn search_marks_span_inline_tags_and_use_only_safe_colors() {
        let (blocks, _) = sanitize("<p>alpha <em>beta</em> gamma</p>");
        let block = &blocks[0];
        let ranges = vec![6..10];
        let marked = html_with_marks(block, &ranges, "#123456");
        assert!(marked.contains("<mark color=\"#123456\">"));
        assert!(marked.contains("alpha "));
        assert!(marked.contains("</em>"));
        let invalid = html_with_marks(block, &ranges, "\"/><script>");
        assert!(!invalid.contains("<script>"));
        assert!(invalid.contains("yellow"));
    }

    #[test]
    fn a_single_block_over_the_limit_fails_explicitly() {
        let mut limits = Limits::default();
        limits.block_bytes = 64;
        let text = format!("<html><body><p>{}</p></body></html>", "word ".repeat(100));
        let document = roxmltree::Document::parse(&text).unwrap();
        let error =
            sanitize_chapter(&document, "c.xhtml", &limits, &CancelToken::new()).unwrap_err();
        assert!(matches!(
            error,
            EpubError::Limit {
                resource: "block bytes",
                ..
            }
        ));
    }

    #[test]
    fn whitespace_collapses_outside_pre_and_is_preserved_inside_it() {
        let (blocks, _) = sanitize("<p>a\n   b</p><pre>a\n  b</pre>");
        assert_eq!(blocks[0].text, "a b");
        assert_eq!(blocks[1].text, "a\n  b");
    }
}
