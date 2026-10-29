//! One positioned parse per document grammar. Ox Content owns MDX syntax;
//! mdast is only the published data contract consumed by the existing projection.
//! Neither adapter serializes source or executes JavaScript.
use super::mdx::Grammar;
use markdown::{
    mdast as md,
    unist::{Point, Position},
};
use ox_content_ast as ox;
use std::ops::Range;

pub(super) fn parse(
    source: &str,
    grammar: Grammar,
    document_start: bool,
) -> Result<md::Node, String> {
    if grammar == Grammar::Markdown {
        return markdown::to_mdast(source, &grammar.options(document_start))
            .map_err(|error| error.to_string());
    }
    let mut context = Adapter::new(source);
    let mut children = Vec::new();
    if document_start {
        if let Some((node, end)) = context.metadata() {
            children.push(node);
            context.base = end;
        }
    }
    let allocator = ox_content_allocator::Allocator::new();
    let options = ox_content_parser::ParserOptions {
        mdx: true,
        math: true,
        // Match the established MDX profile: `<...>` is JSX/HTML rather
        // than an autolink. Other GFM constructs remain available.
        autolinks: false,
        ..ox_content_parser::ParserOptions::gfm()
    };
    let document =
        ox_content_parser::Parser::with_options(&allocator, &source[context.base..], options)
            .parse()
            .map_err(|error| error.to_string())?;
    children.extend(context.children(&document.children)?);
    Ok(md::Node::Root(md::Root {
        children,
        position: Some(context.position(0..source.len())),
    }))
}

struct Adapter<'a> {
    source: &'a str,
    base: usize,
    lines: Vec<usize>,
}
impl<'a> Adapter<'a> {
    fn new(source: &'a str) -> Self {
        let mut lines = vec![0];
        lines.extend(source.match_indices('\n').map(|(at, _)| at + 1));
        Self {
            source,
            base: 0,
            lines,
        }
    }
    fn point(&self, offset: usize) -> Point {
        let row = self.lines.partition_point(|start| *start <= offset) - 1;
        Point::new(row + 1, offset - self.lines[row] + 1, offset)
    }
    fn position(&self, range: Range<usize>) -> Position {
        Position {
            start: self.point(range.start),
            end: self.point(range.end),
        }
    }
    fn range(&self, span: ox::Span, block: bool) -> Range<usize> {
        let start = self.base + span.start as usize;
        let mut end = self.base + span.end as usize;
        // Ox block spans include their terminating line ending; editor leaves
        // own content, while the projection's gap owns the following newline.
        if block {
            while end > start && matches!(self.source.as_bytes()[end - 1], b'\n' | b'\r') {
                end -= 1;
            }
        }
        start..end
    }
    fn metadata(&self) -> Option<(md::Node, usize)> {
        let mut lines = self.source.split_inclusive('\n');
        let opening = lines.next()?;
        let marker = opening.trim_end_matches(['\r', '\n', ' ', '\t']);
        if !matches!(marker, "---" | "+++") {
            return None;
        }
        let mut offset = opening.len();
        for line in lines {
            let closing = line.trim_end_matches(['\r', '\n', ' ', '\t']);
            if closing == marker || (marker == "---" && closing == "...") {
                let value = self.source[opening.len()..offset]
                    .trim_end_matches(['\r', '\n'])
                    .into();
                let end = offset + line.trim_end_matches(['\r', '\n']).len();
                let position = Some(self.position(0..end));
                let node = if marker == "---" {
                    md::Node::Yaml(md::Yaml { value, position })
                } else {
                    md::Node::Toml(md::Toml { value, position })
                };
                return Some((node, offset + line.len()));
            }
            offset += line.len();
        }
        None
    }
    fn children(&self, nodes: &[ox::Node<'_>]) -> Result<Vec<md::Node>, String> {
        nodes.iter().map(|node| self.node(node)).collect()
    }
    fn item(&self, item: &ox::ListItem<'_>) -> Result<md::Node, String> {
        Ok(md::Node::ListItem(md::ListItem {
            children: self.children(&item.children)?,
            position: Some(self.position(self.range(item.span, true))),
            spread: item.spread,
            checked: item.checked,
        }))
    }
    fn attributes(&self, attributes: &[ox::MdxJsxAttributeEntry<'_>]) -> Vec<md::AttributeContent> {
        attributes
            .iter()
            .map(|attribute| match attribute {
                ox::MdxJsxAttributeEntry::Attribute(attribute) => {
                    md::AttributeContent::Property(md::MdxJsxAttribute {
                        name: attribute.name.into(),
                        value: attribute.value.as_ref().map(|value| match value {
                            ox::MdxJsxAttributeValue::Literal(value) => {
                                md::AttributeValue::Literal((*value).into())
                            }
                            ox::MdxJsxAttributeValue::Expression(value) => {
                                md::AttributeValue::Expression(md::AttributeValueExpression {
                                    value: value.value.into(),
                                    stops: vec![(0, self.base + value.span.start as usize + 1)],
                                })
                            }
                        }),
                    })
                }
                ox::MdxJsxAttributeEntry::Expression(value) => {
                    md::AttributeContent::Expression(md::MdxJsxExpressionAttribute {
                        value: value.value.into(),
                        stops: vec![(0, self.base + value.span.start as usize + 1)],
                    })
                }
            })
            .collect()
    }
    fn reference(
        &self,
        range: Range<usize>,
        label_end: Option<usize>,
    ) -> Option<(md::ReferenceKind, String)> {
        let raw = &self.source[range.clone()];
        let opening = range.start + usize::from(raw.starts_with('!'));
        if self.source.as_bytes().get(opening) != Some(&b'[') || !raw.ends_with(']') {
            return None;
        }
        let end = label_end
            .filter(|at| self.source.as_bytes().get(*at) == Some(&b']'))
            .or_else(|| {
                let mut depth = 1usize;
                let mut escaped = false;
                for (at, byte) in self.source.as_bytes()[opening + 1..range.end]
                    .iter()
                    .enumerate()
                {
                    if escaped {
                        escaped = false;
                        continue;
                    }
                    if *byte == b'\\' {
                        escaped = true;
                        continue;
                    }
                    match byte {
                        b'[' => depth += 1,
                        b']' => {
                            depth -= 1;
                            if depth == 0 {
                                return Some(opening + 1 + at);
                            }
                        }
                        _ => {}
                    }
                }
                None
            })?;
        let tail = &self.source[end + 1..range.end];
        let (kind, label) = if tail.is_empty() {
            (md::ReferenceKind::Shortcut, &self.source[opening + 1..end])
        } else if tail == "[]" {
            (md::ReferenceKind::Collapsed, &self.source[opening + 1..end])
        } else if tail.starts_with('[') && tail.ends_with(']') {
            (md::ReferenceKind::Full, &tail[1..tail.len() - 1])
        } else {
            return None;
        };
        Some((kind, identifier(label)))
    }
    fn node(&self, node: &ox::Node<'_>) -> Result<md::Node, String> {
        use ox::Node as N;
        let block = matches!(
            node,
            N::Paragraph(_)
                | N::Heading(_)
                | N::ThematicBreak(_)
                | N::BlockQuote(_)
                | N::List(_)
                | N::ListItem(_)
                | N::CodeBlock(_)
                | N::MathBlock(_)
                | N::Table(_)
                | N::Definition(_)
                | N::FootnoteDefinition(_)
                | N::MdxJsxFlowElement(_)
                | N::MdxjsEsm(_)
                | N::MdxFlowExpression(_)
        );
        let range = self.range(node.span(), block);
        let position = Some(self.position(range.clone()));
        Ok(match node {
            N::Paragraph(value) => md::Node::Paragraph(md::Paragraph {
                children: self.children(&value.children)?,
                position,
            }),
            N::Heading(value) => md::Node::Heading(md::Heading {
                children: self.children(&value.children)?,
                position,
                depth: value.depth,
            }),
            N::ThematicBreak(_) => md::Node::ThematicBreak(md::ThematicBreak { position }),
            N::BlockQuote(value) => md::Node::Blockquote(md::Blockquote {
                children: self.children(&value.children)?,
                position,
            }),
            N::List(value) => md::Node::List(md::List {
                children: value
                    .children
                    .iter()
                    .map(|item| self.item(item))
                    .collect::<Result<_, _>>()?,
                position,
                ordered: value.ordered,
                start: value.start,
                spread: value.spread,
            }),
            N::ListItem(value) => return self.item(value),
            N::CodeBlock(value) => md::Node::Code(md::Code {
                value: value.value.into(),
                position,
                lang: value.lang.map(str::to_owned),
                meta: value.meta.map(str::to_owned),
            }),
            N::MathBlock(value) => md::Node::Math(md::Math {
                value: value.value.into(),
                position,
                meta: None,
            }),
            N::Html(value) => md::Node::Html(md::Html {
                value: value.value.into(),
                position,
            }),
            N::Table(value) => md::Node::Table(md::Table {
                children: value
                    .children
                    .iter()
                    .map(|row| {
                        Ok(md::Node::TableRow(md::TableRow {
                            children: row
                                .children
                                .iter()
                                .map(|cell| {
                                    Ok(md::Node::TableCell(md::TableCell {
                                        children: self.children(&cell.children)?,
                                        position: Some(self.position(self.range(cell.span, false))),
                                    }))
                                })
                                .collect::<Result<_, String>>()?,
                            position: Some(self.position(self.range(row.span, true))),
                        }))
                    })
                    .collect::<Result<_, String>>()?,
                position,
                align: value
                    .align
                    .iter()
                    .map(|align| match align {
                        ox::AlignKind::None => md::AlignKind::None,
                        ox::AlignKind::Left => md::AlignKind::Left,
                        ox::AlignKind::Center => md::AlignKind::Center,
                        ox::AlignKind::Right => md::AlignKind::Right,
                    })
                    .collect(),
            }),
            N::Text(value) => md::Node::Text(md::Text {
                value: value.value.into(),
                position,
            }),
            N::Emphasis(value) => md::Node::Emphasis(md::Emphasis {
                children: self.children(&value.children)?,
                position,
            }),
            N::Strong(value) => md::Node::Strong(md::Strong {
                children: self.children(&value.children)?,
                position,
            }),
            N::InlineCode(value) => md::Node::InlineCode(md::InlineCode {
                value: value.value.into(),
                position,
            }),
            N::InlineMath(value) => md::Node::InlineMath(md::InlineMath {
                value: value.value.into(),
                position,
            }),
            N::Break(_) => md::Node::Break(md::Break { position }),
            N::Link(value) => {
                let children = self.children(&value.children)?;
                if let Some((reference_kind, identifier)) = self.reference(
                    range,
                    children
                        .last()
                        .and_then(md::Node::position)
                        .map(|position| position.end.offset),
                ) {
                    md::Node::LinkReference(md::LinkReference {
                        children,
                        position,
                        reference_kind,
                        label: Some(identifier.clone()),
                        identifier,
                    })
                } else {
                    md::Node::Link(md::Link {
                        children,
                        position,
                        url: value.url.into(),
                        title: value.title.map(str::to_owned),
                    })
                }
            }
            N::Image(value) => {
                if let Some((reference_kind, identifier)) = self.reference(range, None) {
                    md::Node::ImageReference(md::ImageReference {
                        position,
                        alt: value.alt.into(),
                        reference_kind,
                        label: Some(identifier.clone()),
                        identifier,
                    })
                } else {
                    md::Node::Image(md::Image {
                        position,
                        alt: value.alt.into(),
                        url: value.url.into(),
                        title: value.title.map(str::to_owned),
                    })
                }
            }
            N::Delete(value) => md::Node::Delete(md::Delete {
                children: self.children(&value.children)?,
                position,
            }),
            N::FootnoteReference(value) => md::Node::FootnoteReference(md::FootnoteReference {
                position,
                identifier: identifier(value.identifier),
                label: value.label.map(str::to_owned),
            }),
            N::Definition(value) => md::Node::Definition(md::Definition {
                position,
                url: value.url.into(),
                title: value.title.map(str::to_owned),
                identifier: identifier(value.identifier),
                label: value.label.map(str::to_owned),
            }),
            N::FootnoteDefinition(value) => md::Node::FootnoteDefinition(md::FootnoteDefinition {
                children: self.children(&value.children)?,
                position,
                identifier: identifier(value.identifier),
                label: value.label.map(str::to_owned),
            }),
            N::MdxJsxFlowElement(value) => md::Node::MdxJsxFlowElement(md::MdxJsxFlowElement {
                children: self.children(&value.children)?,
                position,
                name: value.name.map(str::to_owned),
                attributes: self.attributes(&value.attributes),
            }),
            N::MdxJsxTextElement(value) => md::Node::MdxJsxTextElement(md::MdxJsxTextElement {
                children: self.children(&value.children)?,
                position,
                name: value.name.map(str::to_owned),
                attributes: self.attributes(&value.attributes),
            }),
            N::MdxjsEsm(value) => md::Node::MdxjsEsm(md::MdxjsEsm {
                value: value.value.into(),
                position,
                stops: vec![(0, range.start)],
            }),
            N::MdxFlowExpression(value) => md::Node::MdxFlowExpression(md::MdxFlowExpression {
                value: value.value.into(),
                position,
                stops: vec![(0, range.start + 1)],
            }),
            N::MdxTextExpression(value) => md::Node::MdxTextExpression(md::MdxTextExpression {
                value: value.value.into(),
                position,
                stops: vec![(0, range.start + 1)],
            }),
            N::DefinitionList(_)
            | N::DefinitionListTerm(_)
            | N::DefinitionListDefinition(_)
            | N::Superscript(_)
            | N::Subscript(_) => {
                return Err("Parser returned an extension disabled by the document grammar".into());
            }
        })
    }
}

fn identifier(label: &str) -> String {
    let mut value = String::with_capacity(label.len());
    let mut space = false;
    for character in label.chars() {
        if matches!(character, ' ' | '\t' | '\r' | '\n') {
            space = !value.is_empty();
        } else {
            if space {
                value.push(' ');
                space = false;
            }
            value.extend(character.to_uppercase());
        }
    }
    value
}
