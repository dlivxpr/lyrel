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
        if !leaf.accepts_pair(&range) && !self.code_accepts_pair(&range, cx) {
            return command;
        }
        let document = self.document.read(cx);
        let source = document.source();
        let start = document
            .boundary(range.start.saturating_sub(4), false)
            .max(leaf.range.start);
        let prev = source.slice(start..range.start).chars().last();
        let next = source.slice(range.end..leaf.range.end).chars().next();
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
            return Command::Select(Selection::caret(range.start + ch.len_utf8()));
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
        let inner = source.slice(range.clone()).to_string();
        let caret = range.start + ch.len_utf8() + inner.len();
        Command::BatchReplaceSelected {
            edits: vec![(range, format!("{ch}{inner}{close}"))],
            selection: Selection::caret(caret),
        }
    }
}
