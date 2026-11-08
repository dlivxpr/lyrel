//! Visible body pairing changes the owning source once and shares its normal history.
use super::*;

fn closer(open: char) -> Option<char> {
    Some(match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '<' => '>',
        '\'' => '\'',
        '"' => '"',
        _ => return None,
    })
}
fn word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

impl SourcePresentation {
    fn accepts_body_pair(&self, range: &Range<usize>, cx: &App) -> bool {
        let Some(first) = self.projection.leaf_at(range.start) else {
            return false;
        };
        if range.end <= first.range.end {
            return first.accepts_pair(range);
        }
        let source = self.document.read(cx).source();
        let mut covered = range.start;
        for leaf in self
            .projection
            .leaves
            .iter()
            .filter(|leaf| leaf.range.start < range.end && range.start < leaf.range.end)
        {
            if !matches!(leaf.kind, Kind::Prose)
                || !source
                    .slice(covered..leaf.range.start.max(covered))
                    .chars()
                    .all(char::is_whitespace)
                || !leaf.accepts_pair(
                    &(range.start.max(leaf.range.start)..range.end.min(leaf.range.end)),
                )
            {
                return false;
            }
            covered = range.end.min(leaf.range.end);
        }
        covered == range.end
    }
    pub(super) fn auto_pair(&self, command: Command, cx: &App) -> Command {
        if !matches!(
            command,
            Command::Insert(_)
                | Command::Commit(_)
                | Command::Backspace
                | Command::Replace {
                    kind: EditKind::Typing,
                    ..
                }
        ) {
            return command;
        }
        if !self.live
            || self.is_composing()
            || !self.extra_selections.is_empty()
            || self.projection.revision != self.document.read(cx).revision()
        {
            return command;
        }
        let range = self.state.selection.range();
        let Some(leaf) = self.projection.leaf_at(range.start) else {
            return command;
        };
        if !self.accepts_body_pair(&range, cx) && !self.code_accepts_pair(&range, cx) {
            return command;
        }
        let document = self.document.read(cx);
        let source = document.source();
        let start = document
            .boundary(range.start.saturating_sub(4), false)
            .max(leaf.range.start);
        let prev = source.slice(start..range.start).chars().last();
        let next = source.chars_at(range.end).next();
        if matches!(command, Command::Backspace) && range.is_empty() {
            if let (Some(open), Some(close)) = (prev, next)
                && closer(open) == Some(close)
                && (leaf
                    .accepts_pair(&(range.start - open.len_utf8()..range.end + close.len_utf8()))
                    || self.code_accepts_pair(
                        &(range.start - open.len_utf8()..range.end + close.len_utf8()),
                        cx,
                    ))
            {
                return Command::BatchReplaceSelected {
                    kind: EditKind::Command,
                    edits: vec![(
                        range.start - open.len_utf8()..range.end + close.len_utf8(),
                        String::new(),
                    )],
                    selection: Selection::caret(range.start - open.len_utf8()),
                };
            }
            return command;
        }
        let text = match &command {
            Command::Insert(text) | Command::Commit(text) => text,
            Command::Replace {
                range: target,
                text,
                kind: EditKind::Typing,
            } if *target == range => text,
            _ => return command,
        };
        let mut chars = text.chars();
        let Some(ch) = chars.next() else {
            return command;
        };
        if chars.next().is_some() {
            return command;
        }
        if range.is_empty() && matches!(ch, ')' | ']' | '}' | '>' | '\'' | '"') && next == Some(ch)
        {
            return Command::SkipPairedCloser(range.start + ch.len_utf8());
        }
        let Some(close) = closer(ch) else {
            return command;
        };
        if range.is_empty()
            && (next.is_some_and(word)
                || matches!(ch, '\'' | '"') && prev.is_some_and(word)
                || ch == '<' && !prev.is_some_and(word))
        {
            return command;
        }
        let caret = range.end + ch.len_utf8();
        let (edits, kind) = if range.is_empty() {
            (vec![(range, format!("{ch}{close}"))], EditKind::Typing)
        } else {
            // Insert at the edges: selected source and each disk line ending stay untouched.
            (
                vec![
                    (range.start..range.start, ch.to_string()),
                    (range.end..range.end, close.to_string()),
                ],
                EditKind::Command,
            )
        };
        Command::BatchReplaceSelected {
            edits,
            kind,
            selection: Selection::caret(caret),
        }
    }
}
