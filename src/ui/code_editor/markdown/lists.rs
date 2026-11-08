use super::{EditKind, MarkdownDocument, PresentationState, Selection};
use gpui_kit::base::input::RopeExt as _;
use std::ops::Range;

/// Presentation-only facts from a container's AST ancestry. Source spelling is
/// always read from the document when a structural command is executed.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct Structure {
    pub depth: usize,
    pub quotes: usize,
    pub start: usize,
    pub marker: Option<Marker>,
    pub task: Option<bool>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Marker {
    Bullet,
    Ordered(u32, u8),
}

struct Prefix {
    content: usize,
    indent: Range<usize>,
    marker: Option<Range<usize>>,
    task: Option<usize>,
    quotes: smallvec::SmallVec<[Range<usize>; 2]>,
}
impl Prefix {
    // This only reads a line's exact source spelling after the AST identified
    // the container. It is not a second Markdown parser or a source-position search.
    fn read(line: &str) -> Self {
        let bytes = line.as_bytes();
        let mut at = 0;
        let mut quotes = smallvec::SmallVec::new();
        loop {
            let start = at;
            while matches!(bytes.get(at), Some(b' ' | b'\t')) {
                at += 1;
            }
            if bytes.get(at) != Some(&b'>') {
                at = start;
                break;
            }
            at += 1;
            if matches!(bytes.get(at), Some(b' ' | b'\t')) {
                at += 1;
            }
            quotes.push(start..at);
        }
        let indent_start = at;
        while matches!(bytes.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        let indent = indent_start..at;
        let marker_start = at;
        let marker_end = match bytes.get(at) {
            Some(b'-' | b'+' | b'*') => Some(at + 1),
            Some(b'0'..=b'9') => {
                while bytes.get(at).is_some_and(u8::is_ascii_digit) {
                    at += 1;
                }
                (at - marker_start <= 9 && matches!(bytes.get(at), Some(b'.' | b')')))
                    .then_some(at + 1)
            }
            _ => None,
        };
        let marker = marker_end
            .filter(|end| *end == bytes.len() || matches!(bytes.get(*end), Some(b' ' | b'\t')))
            .map(|end| {
                at = end;
                while matches!(bytes.get(at), Some(b' ' | b'\t')) {
                    at += 1;
                }
                marker_start..at
            });
        if marker.is_none() {
            at = indent.end;
        }
        let task = (marker.is_some()
            && bytes.get(at) == Some(&b'[')
            && matches!(bytes.get(at + 1), Some(b' ' | b'x' | b'X'))
            && bytes.get(at + 2) == Some(&b']')
            && (at + 3 == bytes.len() || matches!(bytes.get(at + 3), Some(b' ' | b'\t'))))
        .then_some(at + 1);
        if task.is_some() {
            at += 3;
            while matches!(bytes.get(at), Some(b' ' | b'\t')) {
                at += 1;
            }
        }
        // Quotes can also be children of a list item (`- > prose`). Their
        // prefix belongs to the same item, not to a new sibling on Enter.
        if marker.is_some() {
            while bytes.get(at) == Some(&b'>') {
                let start = at;
                at += 1;
                if matches!(bytes.get(at), Some(b' ' | b'\t')) {
                    at += 1;
                }
                quotes.push(start..at);
            }
        }
        Self {
            content: at,
            indent,
            marker,
            task,
            quotes,
        }
    }
    fn is_structure(&self) -> bool {
        self.marker.is_some() || !self.quotes.is_empty()
    }
    fn outer_quotes(&self) -> usize {
        self.quotes
            .iter()
            .take_while(|quote| {
                self.marker
                    .as_ref()
                    .is_none_or(|marker| quote.start < marker.start)
            })
            .count()
    }
    fn list_indent(&self, text: &str, quotes: usize) -> Range<usize> {
        // A child quote consumes its leading indentation in Prefix::read.
        // Move that indentation, not whitespace inside the quoted text.
        self.quotes
            .get(quotes)
            .filter(|quote| {
                self.marker
                    .as_ref()
                    .is_none_or(|marker| quote.start < marker.start)
            })
            .and_then(|quote| {
                text[quote.clone()]
                    .find('>')
                    .map(|at| quote.start..quote.start + at)
            })
            .unwrap_or_else(|| self.indent.clone())
    }
    fn marker_style(&self, line: &str) -> Option<Marker> {
        let range = self.marker.as_ref()?;
        let token = line[range.clone()].trim_end();
        let first = token.as_bytes()[0];
        Some(if first.is_ascii_digit() {
            Marker::Ordered(
                token[..token.len() - 1].parse().ok()?,
                *token.as_bytes().last()?,
            )
        } else {
            Marker::Bullet
        })
    }
}

pub(super) fn style(
    source: &str,
    at: usize,
    depth: usize,
    quotes: usize,
    owner: usize,
) -> Structure {
    let start = source[..at].rfind('\n').map_or(0, |ix| ix + 1);
    let end = source[start..]
        .find('\n')
        .map_or(source.len(), |ix| start + ix);
    let line = &source[start..end];
    let prefix = Prefix::read(line);
    Structure {
        depth,
        quotes,
        start: owner,
        marker: prefix.marker_style(line),
        task: prefix.task.map(|at| line.as_bytes()[at] != b' '),
    }
}

pub(super) fn prefix_len(line: &str) -> usize {
    Prefix::read(line.split('\n').next().unwrap_or_default()).content
}

fn line(document: &MarkdownDocument, at: usize) -> (usize, String, Prefix) {
    let row = document.source().offset_to_point(at).row;
    let start = document.source().line_start_offset(row);
    let end = document.source().line_end_offset(row);
    let text = document.source().slice(start..end).to_string();
    let prefix = Prefix::read(&text);
    (start, text, prefix)
}

pub(super) fn line_break(
    document: &mut MarkdownDocument,
    state: &mut PresentationState,
    hard: bool,
    at: Option<usize>,
) {
    let selected = state.selection().range();
    let (start, text, prefix) = line(document, selected.start);
    let content = start + prefix.content;
    let (text, prefix) = if prefix.marker.is_none() && !text[prefix.content..].trim().is_empty() {
        at.filter(|at| *at < start)
            .map(|at| {
                let (_, text, prefix) = line(document, at);
                (text, prefix)
            })
            .unwrap_or((text, prefix))
    } else {
        (text, prefix)
    };
    if !prefix.is_structure() || selected.start < content {
        document.edit(state, selected, "\n".into(), EditKind::Command);
        return;
    }
    let inner_quote = prefix.marker.as_ref().is_some_and(|marker| {
        prefix
            .quotes
            .last()
            .is_some_and(|quote| quote.start > marker.start)
    });
    if !hard && selected.is_empty() && text[prefix.content..].trim().is_empty() {
        if prefix.marker.is_some() && !inner_quote && !prefix.indent.is_empty() {
            indent(document, state, true, 2, None);
            return;
        }
        let remove = if inner_quote {
            prefix.quotes.last().unwrap().clone()
        } else if let Some(marker) = &prefix.marker {
            marker.start..prefix.content
        } else {
            prefix.quotes.last().unwrap().clone()
        };
        document.edit(
            state,
            start + remove.start..start + remove.end,
            String::new(),
            EditKind::Command,
        );
        return;
    }
    let mut continuation = text[..prefix.content].to_owned();
    if hard || inner_quote {
        if let Some(marker) = &prefix.marker {
            let end = if inner_quote {
                prefix
                    .quotes
                    .iter()
                    .find(|quote| quote.start > marker.start)
                    .unwrap()
                    .start
            } else {
                prefix.content
            };
            continuation.replace_range(
                marker.start..end,
                &" ".repeat(column(&text, end) - column(&text, marker.start)),
            );
        }
    } else {
        if let Some(at) = prefix.task {
            continuation.replace_range(at..at + 1, " ");
        }
        if let Some(Marker::Ordered(number, delimiter)) = prefix.marker_style(&text) {
            let marker = prefix.marker.as_ref().unwrap();
            let digits = text[marker.clone()]
                .bytes()
                .take_while(u8::is_ascii_digit)
                .count();
            let next = if text.as_bytes()[marker.start] == b'0' && digits > 1 {
                format!("{:0digits$}", number.saturating_add(1))
            } else {
                number.saturating_add(1).to_string()
            };
            continuation.replace_range(
                marker.start..marker.start + digits + 1,
                &format!("{next}{}", delimiter as char),
            );
        }
    }
    document.edit(
        state,
        selected,
        format!("{}\n{continuation}", if hard { "  " } else { "" }),
        EditKind::Command,
    );
}

fn column(text: &str, end: usize) -> usize {
    text.as_bytes()[..end].iter().fold(0, |column, byte| {
        if *byte == b'\t' {
            column + 4 - column % 4
        } else {
            column + 1
        }
    })
}

fn outdent_edit(text: &str, indent: Range<usize>, width: usize) -> Option<(Range<usize>, String)> {
    let mut at_column = column(text, indent.start);
    let target = column(text, indent.end)
        .saturating_sub(width)
        .max(at_column);
    for at in indent.clone() {
        let next = if text.as_bytes()[at] == b'\t' {
            at_column + 4 - at_column % 4
        } else {
            at_column + 1
        };
        if next > target {
            return Some((at..indent.end, " ".repeat(target - at_column)));
        }
        at_column = next;
    }
    None
}

fn list_outdent_step(
    document: &MarkdownDocument,
    row: usize,
    text: &str,
    prefix: &Prefix,
    width: usize,
) -> usize {
    if prefix.indent.is_empty() {
        return 0;
    }
    let quotes = prefix.outer_quotes();
    let current_column = column(text, prefix.indent.end);
    let indentation_width = current_column - column(text, prefix.indent.start);
    for previous_row in (0..row).rev() {
        let (_, text, previous) = line(document, document.source().line_start_offset(previous_row));
        if text.trim().is_empty() {
            continue;
        }
        if previous.outer_quotes() < quotes {
            break;
        }
        let indentation = previous.list_indent(&text, quotes);
        let previous_column = column(&text, indentation.end);
        if previous_column < current_column {
            if previous.marker.is_some() && previous.outer_quotes() == quotes {
                return (current_column - previous_column).min(indentation_width);
            }
            break;
        }
    }
    width.max(1).min(indentation_width)
}

pub(super) fn backspace(document: &mut MarkdownDocument, state: &mut PresentationState) {
    let selected = state.selection().range();
    let (start, _, prefix) = line(document, selected.start);
    if selected.is_empty() && selected.start == start + prefix.content && prefix.is_structure() {
        if !prefix.indent.is_empty() && prefix.marker.is_some() {
            indent(document, state, true, 2, None);
            return;
        }
        let previous = start.checked_sub(1).map(|at| line(document, at).2);
        let compatible = previous.as_ref().is_some_and(|previous| {
            previous.is_structure() && previous.quotes.len() == prefix.quotes.len()
        });
        if compatible {
            document.edit(
                state,
                start - 1..start + prefix.content,
                String::new(),
                EditKind::Command,
            );
        } else {
            let inner_quote = prefix.quotes.last().filter(|quote| {
                prefix
                    .marker
                    .as_ref()
                    .is_none_or(|marker| quote.start > marker.start)
            });
            let remove = if let Some(quote) = inner_quote {
                quote.clone()
            } else if let Some(marker) = &prefix.marker {
                marker.start..prefix.content
            } else {
                return;
            };
            document.edit(
                state,
                start + remove.start..start + remove.end,
                String::new(),
                EditKind::Command,
            );
        }
    } else {
        let range = if selected.is_empty() {
            document.previous(selected.start)..selected.end
        } else {
            selected
        };
        document.edit(state, range, String::new(), EditKind::Command);
    }
}

pub(super) fn indent(
    document: &mut MarkdownDocument,
    state: &mut PresentationState,
    outdent: bool,
    width: usize,
    at: Option<usize>,
) {
    let selected = state.selection().range();
    let first_row = document
        .source()
        .offset_to_point(at.unwrap_or(selected.start).min(selected.start))
        .row;
    let mut last_row = document.source().offset_to_point(selected.end).row;
    if last_row > first_row && document.source().line_start_offset(last_row) == selected.end {
        last_row -= 1;
    }
    let (_, first_text, first) = line(document, document.source().line_start_offset(first_row));
    if !first.is_structure() {
        return;
    }
    let quotes = first.outer_quotes();
    let first_column = column(&first_text, first.indent.end);
    let mut step = width.max(1);
    if first.marker.is_some() && outdent {
        step = list_outdent_step(document, first_row, &first_text, &first, width);
    } else if first.marker.is_some() {
        // Match the preceding item's source convention, not EditorConfig's
        // unrelated indentation default. Existing markers are never rewritten.
        let mut parent = false;
        for row in (0..first_row).rev() {
            let (_, text, previous) = line(document, document.source().line_start_offset(row));
            if text.trim().is_empty() {
                continue;
            }
            if previous.outer_quotes() < quotes {
                break;
            }
            let previous_indent = previous.list_indent(&text, quotes);
            let previous_column = column(&text, previous_indent.end);
            if let Some(marker) = previous
                .marker
                .as_ref()
                .filter(|_| previous.outer_quotes() == quotes)
            {
                if previous_column == first_column {
                    step = column(&text, marker.end) - column(&text, marker.start);
                    parent = true;
                    break;
                }
            }
            // Do not reach through prose to an unrelated list farther above.
            if (previous.marker.is_none() && previous_column <= first_column)
                || previous_column < first_column
            {
                break;
            }
        }
        if !parent {
            return;
        }
    }
    // The last explicitly selected item owns its descendants. Move that
    // subtree with it, stopping before the next sibling's untouched marker.
    let (_, last_text, last) = line(document, document.source().line_start_offset(last_row));
    let (tail_text, tail) = if last.marker.is_none() && first.marker.is_some() {
        (first_row..last_row)
            .rev()
            .find_map(|row| {
                let (_, text, prefix) = line(document, document.source().line_start_offset(row));
                prefix.marker.is_some().then_some((text, prefix))
            })
            .unwrap_or((last_text, last))
    } else {
        (last_text, last)
    };
    let tail_column = column(&tail_text, tail.indent.end);
    let mut row = last_row + 1;
    loop {
        let start = document.source().line_start_offset(row);
        if start >= document.source().len() {
            break;
        }
        let (_, text, candidate) = line(document, start);
        if text.trim().is_empty() {
            row += 1;
            continue;
        }
        let descendant = if first.marker.is_some() {
            // Prefix::read includes whitespace before `>` in the quote range.
            // Compare the actual next container's source column, not just its
            // quote count: an unindented quote after a list is not its child.
            candidate.outer_quotes() >= tail.outer_quotes()
                && column(&text, candidate.list_indent(&text, tail.outer_quotes()).end)
                    > tail_column
        } else {
            candidate.quotes.len() > tail.quotes.len()
                || (candidate.quotes.len() == tail.quotes.len()
                    && candidate.indent.len() > tail.indent.len())
        };
        if !descendant {
            break;
        }
        last_row = row;
        row += 1;
    }
    let mut edits = Vec::new();
    let mut group_indent = first_column;
    for row in first_row..=last_row {
        let start = document.source().line_start_offset(row);
        let (_, text, prefix) = line(document, start);
        if text.trim().is_empty() {
            continue;
        }
        let indentation = if first.marker.is_some() {
            prefix.list_indent(&text, quotes)
        } else {
            prefix.indent.clone()
        };
        let indentation_column = column(&text, indentation.end);
        if outdent
            && first.marker.is_some()
            && prefix.marker.is_some()
            && indentation_column < group_indent
        {
            // A shallower selected item begins a new subtree. A root item is
            // already outdented; leave its descendants at their existing level.
            group_indent = indentation_column;
            step = list_outdent_step(document, row, &text, &prefix, width);
        }
        if outdent {
            if first.marker.is_some() {
                if let Some((range, replacement)) = outdent_edit(&text, indentation, step) {
                    edits.push((start + range.start..start + range.end, replacement));
                }
            } else if let Some(quote) = prefix.quotes.get(first.quotes.len().saturating_sub(1)) {
                edits.push((start + quote.start..start + quote.end, String::new()));
            }
        } else {
            let insertion = start
                + if first.marker.is_some() {
                    indentation.end
                } else {
                    indentation.start
                };
            edits.push((
                insertion..insertion,
                if first.marker.is_some() {
                    " ".repeat(step)
                } else {
                    "> ".into()
                },
            ));
        }
    }
    let mut after = state.selection();
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    for (range, text) in &edits {
        let map = |at: usize| {
            if at < range.start {
                at
            } else if at >= range.end {
                at - range.len() + text.len()
            } else {
                range.start + text.len()
            }
        };
        after = Selection::new(map(after.anchor), map(after.head));
    }
    document.batch_edit_selected(state, edits, EditKind::Command, Some(after));
}

pub(super) fn toggle(document: &mut MarkdownDocument, state: &mut PresentationState, at: usize) {
    let (start, text, prefix) = line(document, at);
    if let Some(task) = prefix.task {
        document.batch_edit_selected(
            state,
            vec![(
                start + task..start + task + 1,
                if text.as_bytes()[task] == b' ' {
                    "x"
                } else {
                    " "
                }
                .into(),
            )],
            EditKind::Command,
            Some(state.selection()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Command, MarkdownDocument, PresentationState, Selection, TextFormat};

    #[test]
    fn list_indentation_stops_at_prose_and_an_unrelated_quote() {
        let original = "- earlier\n\nparagraph\n\n007) first\n009) target\n     > child\n> unrelated\n\n42. untouched";
        let mut document = MarkdownDocument::new(original.into(), TextFormat::default());
        let first = original.find("first").unwrap();
        let mut state = PresentationState::new(Selection::caret(first));
        document.command(
            &mut state,
            Command::ListIndent {
                outdent: false,
                width: 4,
                at: None,
            },
        );
        assert_eq!(document.source().to_string(), original);
        assert_eq!(state.selection(), Selection::caret(first));

        let target = original.find("target").unwrap();
        let selection = Selection::new(original.find("     > child").unwrap(), target);
        document.command(&mut state, Command::Select(selection));
        document.command(
            &mut state,
            Command::ListIndent {
                outdent: false,
                width: 4,
                at: None,
            },
        );
        assert_eq!(
            document.source().to_string(),
            "- earlier\n\nparagraph\n\n007) first\n     009) target\n          > child\n> unrelated\n\n42. untouched"
        );
        assert_eq!(
            state.selection(),
            Selection::new(selection.anchor + 5, selection.head + 5)
        );
        document.command(&mut state, Command::Undo);
        assert_eq!(document.source().to_string(), original);
        assert_eq!(state.selection(), selection);
        document.command(&mut state, Command::Select(Selection::caret(first + 5)));
        document.command(
            &mut state,
            Command::ListBreak {
                hard: false,
                at: None,
            },
        );
        assert_eq!(
            document.source().to_string(),
            "- earlier\n\nparagraph\n\n007) first\n008) \n009) target\n     > child\n> unrelated\n\n42. untouched"
        );
        document.command(&mut state, Command::Undo);
        assert_eq!(document.source().to_string(), original);
    }

    #[test]
    fn mixed_tab_indentation_moves_one_structural_level_without_rewriting_markers() {
        let original = "- root\n\t+ sibling\n\n\t009) target\n\t     * child\n- untouched";
        let mut document = MarkdownDocument::new(original.into(), TextFormat::default());
        let at = original.find("target").unwrap();
        let mut state = PresentationState::new(Selection::caret(at));
        document.command(
            &mut state,
            Command::ListIndent {
                outdent: false,
                width: 4,
                at: None,
            },
        );
        assert_eq!(
            document.source().to_string(),
            "- root\n\t+ sibling\n\n\t  009) target\n\t       * child\n- untouched"
        );
        document.command(
            &mut state,
            Command::ListIndent {
                outdent: true,
                width: 4,
                at: None,
            },
        );
        assert_eq!(document.source().to_string(), original);
        assert_eq!(state.selection(), Selection::caret(at));
        let mut document = MarkdownDocument::new("-\ttext".into(), TextFormat::default());
        let mut state = PresentationState::new(Selection::caret(6));
        document.command(
            &mut state,
            Command::ListBreak {
                hard: true,
                at: None,
            },
        );
        assert_eq!(document.source().to_string(), "-\ttext  \n    ");
        document.command(&mut state, Command::Undo);
        assert_eq!(document.source().to_string(), "-\ttext");
    }

    #[test]
    fn reverse_selection_moves_the_last_selected_item_with_its_descendants() {
        let original = "- parent\n  + before\n    * selected\n  + later\n    continued\n    * child\n- final\n  * retained\n- untouched";
        let mut document = MarkdownDocument::new(original.into(), TextFormat::default());
        let selection = Selection::new(
            original.find("continued").unwrap() + "continued".len(),
            original.find("selected").unwrap(),
        );
        let mut state = PresentationState::new(selection);
        document.command(
            &mut state,
            Command::ListIndent {
                outdent: true,
                width: 4,
                at: None,
            },
        );
        assert_eq!(
            document.source().to_string(),
            "- parent\n  + before\n  * selected\n+ later\n  continued\n  * child\n- final\n  * retained\n- untouched"
        );
        document.command(&mut state, Command::Undo);
        assert_eq!(document.source().to_string(), original);
        assert_eq!(state.selection(), selection);
        // A selected root item cannot move out farther. Its children must not
        // be detached just because the selection started in a nested item.
        document.command(
            &mut state,
            Command::Select(Selection::new(
                original.find("final").unwrap() + "final".len(),
                selection.head,
            )),
        );
        document.command(
            &mut state,
            Command::ListIndent {
                outdent: true,
                width: 4,
                at: None,
            },
        );
        assert_eq!(
            document.source().to_string(),
            "- parent\n  + before\n  * selected\n+ later\n  continued\n  * child\n- final\n  * retained\n- untouched"
        );
    }
}
