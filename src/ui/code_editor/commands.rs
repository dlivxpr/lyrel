use std::ops::Range;

use gpui_kit::component::input::RopeExt as _;
use gpui_kit::{Context, Div, InteractiveElement as _, Stateful, Window};

use super::super::app::LyrelApp;

// Application-owned IDE commands, not aliases for the old component fork.
gpui_kit::actions!(
    editor,
    [
        ToggleLineComment,
        ToggleBlockComment,
        MoveLineUp,
        MoveLineDown,
        CopyLineUp,
        CopyLineDown,
        DeleteLine,
        InsertLineAbove,
        InsertLineBelow,
        SelectLine,
        MoveToMatchingBracket,
        SelectNextOccurrence,
        SelectAllOccurrences,
        SkipOccurrence,
        TransformToUppercase,
        TransformToLowercase,
        TransformToTitleCase,
        TrimTrailingWhitespace,
        JoinLines,
        RemoveSurroundingBrackets,
    ]
);

#[derive(Clone, Copy)]
pub(crate) enum Command {
    Comment,
    BlockComment,
    MoveUp,
    MoveDown,
    CopyUp,
    CopyDown,
    Delete,
    InsertAbove,
    InsertBelow,
    SelectLine,
    Bracket,
    Next,
    All,
    Skip,
    Upper,
    Lower,
    Title,
    Trim,
    Join,
    Unwrap,
}

pub(super) fn install(mut root: Stateful<Div>, cx: &mut Context<LyrelApp>) -> Stateful<Div> {
    macro_rules! bind {
        ($($action:ty => $command:ident),* $(,)?) => { $(
            root = root.on_action(cx.listener(|app, _: &$action, window, cx| {
                if app.editor_has_focus(window, cx) {
                    app.editor_apply_command(Command::$command, window, cx);
                } else { cx.propagate(); }
            }));
        )* };
    }
    bind!(
        ToggleLineComment => Comment, ToggleBlockComment => BlockComment,
        MoveLineUp => MoveUp, MoveLineDown => MoveDown, CopyLineUp => CopyUp,
        CopyLineDown => CopyDown, DeleteLine => Delete, InsertLineAbove => InsertAbove,
        InsertLineBelow => InsertBelow, SelectLine => SelectLine, MoveToMatchingBracket => Bracket,
        SelectNextOccurrence => Next, SelectAllOccurrences => All, SkipOccurrence => Skip,
        TransformToUppercase => Upper, TransformToLowercase => Lower, TransformToTitleCase => Title,
        TrimTrailingWhitespace => Trim, JoinLines => Join, RemoveSurroundingBrackets => Unwrap,
    );
    root.on_action(
        cx.listener(|app, _: &gpui_kit::component::input::Search, window, cx| {
            if !app.editor_markdown_search(false, window, cx) {
                cx.propagate();
            }
        }),
    )
    .on_action(
        cx.listener(|app, _: &gpui_kit::component::input::Replace, window, cx| {
            if !app.editor_markdown_search(true, window, cx) {
                cx.propagate();
            }
        }),
    )
}

struct Edit {
    range: Range<usize>,
    replacement: String,
    selection: Range<usize>,
}

fn whole_lines(text: &str, selection: &Range<usize>) -> Range<usize> {
    let start = text[..selection.start].rfind('\n').map_or(0, |at| at + 1);
    let end_at = if selection.end > selection.start && text[..selection.end].ends_with('\n') {
        selection.end - 1
    } else {
        selection.end
    };
    let end = text[end_at..]
        .find('\n')
        .map_or(text.len(), |at| end_at + at + 1);
    start..end
}

fn comment_pair(language: &str) -> (&'static str, &'static str) {
    match language {
        "python" | "bash" | "ruby" | "yaml" | "toml" | "elixir" | "cmake" | "make" | "graphql" => {
            ("#", "")
        }
        "lua" | "sql" => ("--", ""),
        "html" | "markdown" | "xml" | "vue" | "svelte" | "astro" => ("<!--", "-->"),
        "erb" | "ejs" => ("<%#", "%>"),
        "css" => ("/*", "*/"),
        _ => ("//", ""),
    }
}

// Commands submit one replacement to Kit so text, caret and Undo stay atomic.
// The changes here are application edits in source-byte coordinates.
fn combine(
    text: &str,
    mut changes: Vec<(Range<usize>, String)>,
    selected: Range<usize>,
) -> Option<Edit> {
    changes.retain(|(range, inserted)| text[range.clone()] != *inserted);
    changes.sort_by_key(|(range, _)| (range.start, range.end));
    let range = changes.first()?.0.start..changes.last()?.0.end;
    let map = |offset: usize, right: bool| {
        let mut mapped = offset;
        for (range, inserted) in &changes {
            if offset < range.start || (offset == range.start && !right) {
                break;
            }
            mapped =
                mapped - (offset.saturating_sub(range.start).min(range.len())) + inserted.len();
            if offset < range.end {
                break;
            }
        }
        mapped
    };
    let selection = map(selected.start, selected.is_empty())..map(selected.end, true);
    let mut replacement = String::with_capacity(range.len());
    let mut at = range.start;
    for (change, inserted) in changes {
        replacement.push_str(&text[at..change.start]);
        replacement.push_str(&inserted);
        at = change.end;
    }
    Some(Edit {
        range,
        replacement,
        selection,
    })
}

fn block_pair(language: &str) -> Option<(&'static str, &'static str)> {
    match language {
        "html" | "markdown" | "xml" | "vue" | "svelte" | "astro" => Some(("<!--", "-->")),
        "erb" | "ejs" => Some(("<%#", "%>")),
        "lua" => Some(("--[[", "]]")),
        "cmake" => Some(("#[[", "]]")),
        "python" | "bash" | "ruby" | "yaml" | "toml" | "make" | "graphql" | "elixir" => None,
        _ => Some(("/*", "*/")),
    }
}

fn indent_columns(line: &str, tab_size: usize) -> usize {
    let mut column = 0;
    for ch in line.chars().take_while(|ch| matches!(ch, ' ' | '\t')) {
        column += if ch == '\t' {
            tab_size - column % tab_size
        } else {
            1
        };
    }
    column
}

fn plan(
    text: &str,
    selected: Range<usize>,
    language: &str,
    tab_size: usize,
    command: Command,
) -> Option<Edit> {
    let mut range = selected.clone();
    let replacement;
    let selection;
    match command {
        Command::Upper | Command::Lower | Command::Title => {
            if range.is_empty() {
                use unicode_segmentation::UnicodeSegmentation as _;
                let (start, word) = text.unicode_word_indices().find(|(start, word)| {
                    *start <= selected.start && selected.start <= *start + word.len()
                })?;
                range = start..start + word.len();
            }
            let original = &text[range.clone()];
            replacement = match command {
                Command::Upper => original.to_uppercase(),
                Command::Lower => original.to_lowercase(),
                _ => {
                    use unicode_segmentation::UnicodeSegmentation as _;
                    let mut titled = String::with_capacity(original.len());
                    for word in original.split_word_bounds() {
                        let mut chars = word.chars();
                        if let Some(first) = chars.next() {
                            titled.extend(first.to_uppercase());
                        }
                        titled.push_str(chars.as_str());
                    }
                    titled
                }
            };
            selection = Some(if selected.is_empty() {
                let mut at = (selected.start - range.start).min(replacement.len());
                while !replacement.is_char_boundary(at) {
                    at -= 1;
                }
                range.start + at..range.start + at
            } else {
                range.start..range.start + replacement.len()
            });
        }
        Command::Trim => {
            range = if selected.is_empty() {
                0..text.len()
            } else {
                whole_lines(text, &selected)
            };
            let mut changes = Vec::new();
            let mut offset = range.start;
            for line in text[range].split_inclusive('\n') {
                let body = line.strip_suffix('\n').unwrap_or(line);
                let body = body.strip_suffix('\r').unwrap_or(body);
                let keep = body.trim_end_matches([' ', '\t']).len();
                if keep < body.len() {
                    changes.push((offset + keep..offset + body.len(), String::new()));
                }
                offset += line.len();
            }
            return combine(text, changes, selected);
        }
        Command::Join => {
            let lines = whole_lines(text, &selected);
            let mut breaks: Vec<_> = text[lines.clone()]
                .match_indices('\n')
                .map(|(at, _)| lines.start + at)
                .filter(|at| at + 1 < lines.end)
                .collect();
            if breaks.is_empty() && lines.end > lines.start && text[..lines.end].ends_with('\n') {
                breaks.push(lines.end - 1);
            }
            let mut changes: Vec<(Range<usize>, String)> = Vec::with_capacity(breaks.len());
            for newline in breaks {
                let start = text[..newline].trim_end_matches([' ', '\t', '\r']).len();
                let next = &text[newline + 1..];
                let end = newline + 1 + next.len() - next.trim_start_matches([' ', '\t']).len();
                let left_empty = start == 0 || text[..start].ends_with('\n');
                let next_char = text[end..].chars().next();
                let space = if left_empty
                    || matches!(next_char, None | Some('\n' | '\r' | ')' | ']' | '}'))
                {
                    ""
                } else {
                    " "
                };
                if let Some((previous, inserted)) = changes.last_mut()
                    && previous.end >= start
                {
                    previous.end = end;
                    if !space.is_empty() {
                        *inserted = space.to_owned();
                    }
                } else {
                    changes.push((start..end, space.to_owned()));
                }
            }
            let join = changes.first()?.0.start;
            let mut edit = combine(text, changes, selected.clone())?;
            if selected.is_empty() {
                edit.selection = join..join;
            }
            return Some(edit);
        }
        Command::Comment => {
            range = whole_lines(text, &selected);
            let (prefix, suffix) = comment_pair(language);
            let original = &text[range.clone()];
            let significant = |line: &str| !line.trim_matches([' ', '\t', '\r', '\n']).is_empty();
            let all_blank = !original.lines().any(significant);
            let indent = original
                .lines()
                .filter(|line| significant(line))
                .map(|line| indent_columns(line, tab_size))
                .min()
                .unwrap_or(0);
            let remove = !all_blank
                && original
                    .lines()
                    .filter(|line| significant(line))
                    .all(|line| {
                        let body = line.trim_start_matches([' ', '\t']);
                        body.starts_with(prefix) && (suffix.is_empty() || body.ends_with(suffix))
                    });
            if original.is_empty() {
                let inserted = if suffix.is_empty() {
                    format!("{prefix} ")
                } else {
                    format!("{prefix}  {suffix}")
                };
                let caret = range.start + prefix.len() + 1;
                return Some(Edit {
                    range,
                    replacement: inserted,
                    selection: caret..caret,
                });
            }
            let mut changes = Vec::new();
            let mut offset = range.start;
            for line in original.split_inclusive('\n') {
                let body = line.trim_end_matches(['\r', '\n']);
                if significant(body) || all_blank {
                    if remove {
                        let at = body.len() - body.trim_start_matches([' ', '\t']).len();
                        let prefix_end = at
                            + prefix.len()
                            + usize::from(body[at + prefix.len()..].starts_with(' '));
                        changes.push((offset + at..offset + prefix_end, String::new()));
                        if !suffix.is_empty() {
                            let end = body.len();
                            let mut start = end - suffix.len();
                            if body[..start].ends_with(' ') {
                                start -= 1;
                            }
                            start = start.max(prefix_end);
                            changes.push((offset + start..offset + end, String::new()));
                        }
                    } else {
                        let mut column = 0;
                        let mut at = 0;
                        for (byte, ch) in body.char_indices() {
                            if !matches!(ch, ' ' | '\t') || (!all_blank && column >= indent) {
                                break;
                            }
                            column += if ch == '\t' {
                                tab_size - column % tab_size
                            } else {
                                1
                            };
                            at = byte + ch.len_utf8();
                        }
                        changes.push((offset + at..offset + at, format!("{prefix} ")));
                        if !suffix.is_empty() {
                            changes.push((
                                offset + body.len()..offset + body.len(),
                                format!(" {suffix}"),
                            ));
                        }
                    }
                }
                offset += line.len();
            }
            return combine(text, changes, selected);
        }
        Command::BlockComment => {
            let Some((open, close)) = block_pair(language) else {
                return plan(text, selected, language, tab_size, Command::Comment);
            };
            let before = text[..range.start].trim_end_matches([' ', '\t']);
            let after = text[range.end..].trim_start_matches([' ', '\t']);
            if before.ends_with(open) && after.starts_with(close) {
                let start = before.len() - open.len();
                let end = text.len() - after.len() + close.len();
                return combine(
                    text,
                    vec![
                        (start..range.start, String::new()),
                        (range.end..end, String::new()),
                    ],
                    selected,
                );
            }
            if range.is_empty() {
                let line = whole_lines(text, &range);
                let body = text[line.clone()].trim_end_matches(['\r', '\n', ' ', '\t']);
                let indent = body.len() - body.trim_start_matches([' ', '\t']).len();
                range = line.start + indent..line.start + body.len();
            }
            let original = &text[range.clone()];
            if original.starts_with(open)
                && original.ends_with(close)
                && original.len() >= open.len() + close.len()
            {
                let first = open.len() + usize::from(original[open.len()..].starts_with(' '));
                let mut last = original.len() - close.len();
                if last > first && original[..last].ends_with(' ') {
                    last -= 1;
                }
                return combine(
                    text,
                    vec![
                        (range.start..range.start + first, String::new()),
                        (range.start + last..range.end, String::new()),
                    ],
                    selected,
                );
            }
            replacement = format!("{open} {original} {close}");
            let start = range.start + open.len() + 1;
            selection = Some(if selected.is_empty() {
                let caret = start
                    + selected
                        .start
                        .saturating_sub(range.start)
                        .min(original.len());
                caret..caret
            } else {
                start..start + original.len()
            });
        }
        Command::Unwrap => {
            let (open, close) = bracket_pairs(text, language)
                .into_iter()
                .filter(|(open, close)| *open <= selected.start && close + 1 >= selected.end)
                .max_by_key(|(open, _)| *open)?;
            return combine(
                text,
                vec![
                    (open..open + 1, String::new()),
                    (close..close + 1, String::new()),
                ],
                selected,
            );
        }
        Command::Delete => {
            range = whole_lines(text, &range);
            let column = text[..selected.end]
                .rsplit('\n')
                .next()
                .unwrap_or("")
                .chars()
                .count();
            if range.end == text.len() && range.start > 0 && !text[range.clone()].ends_with('\n') {
                range.start -= 1;
            }
            replacement = String::new();
            let (line_start, line) = if range.end < text.len() {
                (
                    range.start,
                    text[range.end..].split('\n').next().unwrap_or(""),
                )
            } else {
                let start = text[..range.start].rfind('\n').map_or(0, |at| at + 1);
                (start, &text[start..range.start])
            };
            let at = line
                .char_indices()
                .nth(column)
                .map_or(line.len(), |(at, _)| at);
            selection = Some(line_start + at..line_start + at);
        }
        Command::InsertAbove | Command::InsertBelow => {
            let lines = whole_lines(text, &range);
            let body = &text[lines.clone()];
            let indent: String = body
                .chars()
                .take_while(|ch| *ch == ' ' || *ch == '\t')
                .collect();
            let at = if matches!(command, Command::InsertAbove) {
                lines.start
            } else {
                lines.end
            };
            let after_eof = at == text.len() && !text.ends_with('\n') && !text.is_empty();
            range = at..at;
            replacement = if after_eof {
                format!("\n{indent}")
            } else {
                format!("{indent}\n")
            };
            let caret = at + indent.len() + usize::from(after_eof);
            selection = Some(caret..caret);
        }
        Command::CopyUp | Command::CopyDown => {
            let lines = whole_lines(text, &range);
            let body = &text[lines.clone()];
            let before = matches!(command, Command::CopyUp);
            let at = if before { lines.start } else { lines.end };
            range = at..at;
            replacement = if body.ends_with('\n') {
                body.to_owned()
            } else if before {
                format!("{body}\n")
            } else {
                format!("\n{body}")
            };
            let shift = if before { 0 } else { replacement.len() };
            selection = Some(selected.start + shift..selected.end + shift);
        }
        Command::MoveUp | Command::MoveDown => {
            range = whole_lines(text, &range);
            let body = &text[range.clone()];
            if matches!(command, Command::MoveUp) {
                if range.start == 0 {
                    return None;
                }
                let previous = whole_lines(text, &(range.start - 1..range.start - 1));
                let above = &text[previous.clone()];
                replacement = if body.ends_with('\n') {
                    format!("{body}{above}")
                } else {
                    format!("{body}\n{}", above.trim_end_matches('\n'))
                };
                selection = Some(selected.start - above.len()..selected.end - above.len());
                range.start = previous.start;
            } else {
                if range.end == text.len() && (!text.ends_with('\n') || range.is_empty()) {
                    return None;
                }
                let next = whole_lines(text, &(range.end..range.end));
                let below = &text[next.clone()];
                replacement = if below.ends_with('\n') {
                    format!("{below}{body}")
                } else {
                    format!("{below}\n{}", body.trim_end_matches('\n'))
                };
                let shift = below.len() + usize::from(!below.ends_with('\n'));
                selection = Some(selected.start + shift..selected.end + shift);
                range.end = next.end;
            }
        }
        _ => return None,
    }
    if text[range.clone()] == replacement {
        return None;
    }
    let end = range.start + replacement.len();
    let document_end = text.len() - range.len() + replacement.len();
    let selected = selection.unwrap_or(end..end);
    let selected = selected.start.min(document_end)..selected.end.min(document_end);
    Some(Edit {
        range,
        replacement,
        selection: selected,
    })
}

fn bracket_pairs(text: &str, language: &str) -> Vec<(usize, usize)> {
    // Parse through the published language grammar, not the editor's private
    // tree. Commands must ignore delimiters in strings and comments.
    let tree = gpui_kit::component::highlighter::LanguageRegistry::singleton()
        .language(language)
        .and_then(|config| config.language.clone())
        .and_then(|language| {
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(&language).ok()?;
            parser.parse(text, None)
        });
    let mut excluded = Vec::new();
    if let Some(tree) = &tree {
        let mut cursor = tree.walk();
        loop {
            let node = cursor.node();
            let kind = node.kind();
            let skip =
                kind.contains("string") || kind.contains("comment") || kind == "char_literal";
            if skip {
                excluded.push(node.byte_range());
            }
            if !skip && cursor.goto_first_child() {
                continue;
            }
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    break;
                }
            }
            if cursor.node().id() == tree.root_node().id() {
                break;
            }
        }
    }
    let mut stack = Vec::new();
    let mut pairs = Vec::new();
    let mut excluded_ix = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut skip_until = 0;
    for (offset, ch) in text.char_indices() {
        while excluded
            .get(excluded_ix)
            .is_some_and(|range: &Range<usize>| range.end <= offset)
        {
            excluded_ix += 1;
        }
        if offset < skip_until
            || excluded
                .get(excluded_ix)
                .is_some_and(|range| range.contains(&offset))
        {
            continue;
        }
        if tree.is_none() {
            if let Some(delimiter) = quote {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == delimiter {
                    quote = None;
                }
                continue;
            }
            if matches!(ch, '\'' | '"' | '`') {
                quote = Some(ch);
                continue;
            }
            let (comment, suffix) = comment_pair(language);
            if suffix.is_empty() && text[offset..].starts_with(comment) {
                skip_until = text[offset..]
                    .find('\n')
                    .map_or(text.len(), |end| offset + end);
                continue;
            }
            if let Some((open, close)) = block_pair(language)
                && text[offset..].starts_with(open)
            {
                skip_until = text[offset + open.len()..]
                    .find(close)
                    .map_or(text.len(), |end| offset + open.len() + end + close.len());
                continue;
            }
        }
        match ch {
            '(' | '[' | '{' => stack.push((offset, ch)),
            ')' | ']' | '}' => {
                if let Some((start, open)) = stack.last().copied()
                    && matches!((open, ch), ('(', ')') | ('[', ']') | ('{', '}'))
                {
                    stack.pop();
                    pairs.push((start, offset));
                }
            }
            _ => {}
        }
    }
    pairs
}

fn matching_bracket(text: &str, cursor: usize, language: &str) -> Option<usize> {
    let adjacent = text[cursor..]
        .chars()
        .next()
        .filter(|ch| matches!(ch, '(' | ')' | '[' | ']' | '{' | '}'))
        .map(|_| cursor)
        .or_else(|| {
            text[..cursor]
                .char_indices()
                .next_back()
                .filter(|(_, ch)| matches!(ch, '(' | ')' | '[' | ']' | '{' | '}'))
                .map(|(at, _)| at)
        });
    let pairs = bracket_pairs(text, language);
    let (open, close) = pairs
        .iter()
        .copied()
        .find(|(open, close)| adjacent == Some(*open) || adjacent == Some(*close))
        .or_else(|| {
            pairs
                .into_iter()
                .filter(|(open, close)| *open <= cursor && cursor <= *close)
                .max_by_key(|(open, _)| *open)
        })?;
    Some(if adjacent == Some(close) { open } else { close })
}

impl LyrelApp {
    pub(crate) fn editor_apply_command(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(file) = self.active_buffer() else {
            return false;
        };
        let Some(input) = self.active_editor_input() else {
            return false;
        };
        let state = input.read(cx);
        let selected = if matches!(command, Command::Next | Command::Skip) {
            state.occurrence_range()
        } else {
            state.selected_range()
        };
        let language = file.language();
        let tab_size = file.indent.size.max(1);
        if matches!(command, Command::Next | Command::All | Command::Skip) {
            let text = state.text();
            let select_word = selected.is_empty() && matches!(command, Command::Next);
            let selected = if selected.is_empty() {
                text.word_range(state.cursor()).unwrap_or(selected)
            } else {
                selected
            };
            if selected.is_empty() {
                return true;
            }
            if select_word {
                input.set_selected_range(selected, cx);
                return true;
            }
            let needle = text.slice(selected.clone()).to_string();
            let source = text.to_string();
            let mut matches: Vec<_> = source
                .match_indices(&needle)
                .map(|(at, _)| at..at + needle.len())
                .collect();
            if matches!(command, Command::All) {
                if let Some(first) = matches.first().cloned() {
                    input.set_selected_range(first, cx);
                    for range in matches.drain(1..) {
                        input.add_selection(range, cx);
                    }
                }
            } else {
                let next = matches
                    .into_iter()
                    .find(|range| range.start >= selected.end)
                    .or_else(|| source.find(&needle).map(|at| at..at + needle.len()));
                if let Some(next) = next {
                    if matches!(command, Command::Skip) {
                        input.set_selected_range(next, cx);
                    } else {
                        input.add_selection(next, cx);
                    }
                }
            }
            return true;
        }
        let text = state.text().to_string();
        if matches!(command, Command::SelectLine) {
            let mut range = whole_lines(&text, &selected);
            if !selected.is_empty() && selected == range && range.end < text.len() {
                range.end = whole_lines(&text, &(range.end..range.end)).end;
            }
            input.set_selected_range(range, cx);
            return true;
        }
        if matches!(command, Command::Bracket) {
            if let Some(offset) = matching_bracket(&text, state.cursor(), language) {
                input.set_selected_range(offset..offset, cx);
            }
            return true;
        }
        let selected = if matches!(command, Command::InsertAbove | Command::InsertBelow) {
            state.cursor()..state.cursor()
        } else {
            selected
        };
        let Some(edit) = plan(&text, selected.clone(), language, tab_size, command) else {
            return true;
        };
        // IDE commands deliberately affect the active selection only; typing
        // continues to honor all occurrence selections in either presentation.
        if let Some(code) = input.code() {
            code.update(cx, |state, cx| {
                state.set_selected_range(edit.range, cx);
                state.replace(edit.replacement, window, cx);
            });
        } else if let Some(view) = input.markdown().filter(|_| {
            matches!(
                command,
                Command::MoveUp | Command::MoveDown | Command::CopyUp | Command::CopyDown
            )
        }) {
            let selected_lines = whole_lines(&text, &selected);
            let lines = match command {
                Command::MoveUp => vec![
                    selected_lines.clone(),
                    edit.range.start..selected_lines.start,
                ],
                Command::MoveDown => {
                    vec![selected_lines.end..edit.range.end, selected_lines.clone()]
                }
                _ => vec![selected_lines.clone()],
            };
            view.update(cx, |view, cx| {
                view.command(
                    super::markdown::Command::ReplaceLines {
                        range: edit.range,
                        lines,
                        leading_newline: matches!(command, Command::CopyDown)
                            && !text[selected_lines].ends_with('\n'),
                        trailing_newline: edit.replacement.ends_with('\n'),
                        selection: super::markdown::Selection::new(
                            edit.selection.start,
                            edit.selection.end,
                        ),
                    },
                    window,
                    cx,
                )
            });
        } else {
            input.replace(edit.range, &edit.replacement, window, cx);
        }
        input.set_selected_range(edit.selection, cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edited(
        source: &str,
        range: Range<usize>,
        language: &str,
        command: Command,
    ) -> (String, Range<usize>) {
        let Some(edit) = plan(source, range.clone(), language, 4, command) else {
            return (source.to_owned(), range);
        };
        let mut text = source.to_owned();
        text.replace_range(edit.range, &edit.replacement);
        assert!(text.is_char_boundary(edit.selection.start));
        assert!(text.is_char_boundary(edit.selection.end));
        (text, edit.selection)
    }

    #[test]
    fn line_commands_preserve_unicode_and_the_unterminated_last_line() {
        let (text, selection) = edited("a\n中", 0..2, "rust", Command::MoveDown);
        assert_eq!(text, "中\na");
        assert_eq!(&text[selection], "a");
        assert_eq!(edited(&text, 4..5, "rust", Command::MoveUp).0, "a\n中");
        assert_eq!(edited("中", 0..3, "rust", Command::CopyDown).0, "中\n中");
        assert_eq!(edited("a\n中", 2..5, "rust", Command::Delete).0, "a");
    }

    #[test]
    fn comments_round_trip_without_changing_indentation_or_the_next_line() {
        let text = "  中();\n  x();\nkeep();";
        let selected = 0..text.find("keep").unwrap();
        let (commented, selected) = edited(text, selected, "rust", Command::Comment);
        assert_eq!(commented, "  // 中();\n  // x();\nkeep();");
        assert_eq!(
            edited(&commented, selected, "rust", Command::Comment).0,
            text
        );
        let (wrapped, selected) = edited("中", 0..3, "rust", Command::BlockComment);
        assert_eq!(wrapped, "/* 中 */");
        assert_eq!(
            edited(&wrapped, selected, "rust", Command::BlockComment).0,
            "中"
        );
    }

    #[test]
    fn trimming_a_selection_leaves_other_lines_and_crlf_intact() {
        let source = "keep \t\r\n中  \r\nlast\t";
        let start = source.find('中').unwrap();
        let end = source.find("last").unwrap();
        let (text, selection) = edited(source, start..end, "rust", Command::Trim);
        assert_eq!(text, "keep \t\r\n中\r\nlast\t");
        assert_eq!(&text[selection], "中\r\n");
    }

    #[test]
    fn word_transforms_and_whitespace_edits_keep_the_caret() {
        let source = "let delta = 1;";
        assert_eq!(
            edited(source, 6..6, "rust", Command::Upper),
            ("let DELTA = 1;".into(), 6..6)
        );
        assert_eq!(
            edited("once mORE can't foo_bar", 0..22, "rust", Command::Title).0,
            "Once MORE Can't Foo_bar"
        );
        assert_eq!(
            edited("中文  \nvalue\t", 8..8, "rust", Command::Trim),
            ("中文\nvalue".into(), 6..6)
        );
        assert_eq!(
            edited("    aa  \n \t bb\ncc", 5..5, "rust", Command::Join),
            ("    aa bb\ncc".into(), 6..6)
        );
    }

    #[test]
    fn comments_keep_bare_carets_and_respect_tab_columns() {
        assert_eq!(
            edited("  emit();", 5..5, "rust", Command::Comment),
            ("  // emit();".into(), 8..8)
        );
        let source = "\tfirst\n    second";
        let edit = plan(source, 0..source.len(), "rust", 4, Command::Comment).unwrap();
        let mut actual = source.to_owned();
        actual.replace_range(edit.range, &edit.replacement);
        assert_eq!(actual, "\t// first\n    // second");
        assert_eq!(
            edited("", 0..0, "rust", Command::Comment),
            ("// ".into(), 3..3)
        );
        assert_eq!(edited("<!-- -->", 0..8, "html", Command::Comment).0, "");
    }

    #[test]
    fn unwrap_uses_the_enclosing_code_pair_and_ignores_quoted_brackets() {
        let source = "call(\")\", 中)";
        assert_eq!(matching_bracket(source, 4, "rust"), Some(source.len() - 1));
        let caret = source.find('中').unwrap();
        assert_eq!(
            edited(source, caret..caret, "rust", Command::Unwrap),
            ("call\")\", 中".into(), caret - 1..caret - 1)
        );
        assert_eq!(
            edited("left\nright\n", 7..7, "rust", Command::MoveDown).0,
            "left\n\nright"
        );
        assert_eq!(
            edited("before\n中文x\nafter", 13..13, "rust", Command::Delete),
            ("before\nafter".into(), 9..9)
        );
    }

    #[test]
    fn bracket_navigation_handles_a_multibyte_character_after_the_caret() {
        let text = "(中[文])";
        assert_eq!(matching_bracket(text, 0, "rust"), Some(9));
        assert_eq!(matching_bracket(text, text.len(), "rust"), Some(0));
        assert_eq!(matching_bracket(text, 4, "rust"), Some(8));
        assert_eq!(edited("\"", 0..1, "rust", Command::Unwrap).0, "\"");
    }
}

#[cfg(test)]
mod gpui_tests {
    use super::*;
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::code_editor::nav::gpui_tests::open_rs;
    use gpui_kit::{TestAppContext, VisualTestContext};

    fn select_two(
        input: &gpui_kit::Entity<gpui_kit::component::input::EditorState>,
        vcx: &mut VisualTestContext,
    ) {
        input.update_in(vcx, |state, window, cx| {
            state.set_value("foo foo", window, cx);
            state.set_selected_range(0..3, cx);
            state.add_selection(4..7, cx);
            state.focus(window, cx);
        });
        vcx.run_until_parked();
    }

    #[gpui_kit::test]
    fn ide_commands_change_only_the_active_selection_but_typing_keeps_all_cursors(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        open_rs(&app, &mut vcx, "/commands/active.rs");
        vcx.run_until_parked();
        let input = app.read_with(&vcx, |app, _| {
            app.active_buffer()
                .unwrap()
                .input
                .code()
                .expect("ordinary code editor")
                .clone()
        });
        select_two(&input, &mut vcx);
        vcx.dispatch_action(TransformToUppercase);
        vcx.run_until_parked();
        assert_eq!(
            input.read_with(&vcx, |state, _| state.text().to_string()),
            "FOO foo"
        );
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        vcx.run_until_parked();
        assert_eq!(
            input.read_with(&vcx, |state, _| state.text().to_string()),
            "foo foo"
        );

        select_two(&input, &mut vcx);
        vcx.simulate_keystrokes("x");
        vcx.run_until_parked();
        assert_eq!(
            input.read_with(&vcx, |state, _| state.text().to_string()),
            "x x"
        );
    }

    #[gpui_kit::test]
    fn editor_shortcuts_edit_the_focused_document_instead_of_running_workspace_commands(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        open_rs(&app, &mut vcx, "/commands/keyboard.rs");
        vcx.run_until_parked();
        let input = app.read_with(&vcx, |app, _| {
            app.active_buffer()
                .unwrap()
                .input
                .code()
                .expect("ordinary code editor")
                .clone()
        });
        input.update_in(&mut vcx, |state, window, cx| {
            state.set_value("one\ntwo\nthree", window, cx);
            state.set_selected_range(5..5, cx);
            state.focus(window, cx);
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("secondary-/");
        vcx.run_until_parked();
        assert_eq!(
            input.read_with(&vcx, |state, _| state.text().to_string()),
            "one\n// two\nthree"
        );
        vcx.simulate_keystrokes("secondary-z");
        vcx.run_until_parked();
        assert_eq!(
            input.read_with(&vcx, |state, _| state.text().to_string()),
            "one\ntwo\nthree"
        );
        vcx.simulate_keystrokes("alt-up");
        vcx.run_until_parked();
        assert_eq!(
            input.read_with(&vcx, |state, _| state.text().to_string()),
            "two\none\nthree"
        );
    }
    #[gpui_kit::test]
    fn markdown_workbench_shortcuts_keep_commands_selection_and_shared_undo(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("commands.md");
        std::fs::write(&path, "one\ntwo\nthree").unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.read_with(&vcx, |app, _| app.active_buffer().is_some()) {
            vcx.run_until_parked();
            assert!(
                std::time::Instant::now() < deadline,
                "Markdown open did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.update_in(&mut vcx, |app, window, cx| {
            app.toggle_document_preview(window, cx)
        });
        app.update_in(&mut vcx, |app, window, cx| {
            let input = app.active_buffer().unwrap().input.clone();
            input.set_selected_range(5..5, cx);
            input.focus(window, cx);
        });
        vcx.simulate_keystrokes("secondary-/");
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer()
                    .unwrap()
                    .input
                    .read(cx)
                    .text()
                    .to_string(),
                "one\n<!-- two -->\nthree"
            );
            assert!(app.active_buffer().unwrap().dirty);
        });
        vcx.simulate_keystrokes("secondary-z");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("alt-up");
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer()
                    .unwrap()
                    .input
                    .read(cx)
                    .text()
                    .to_string(),
                "two\none\nthree"
            );
        });
        vcx.simulate_keystrokes("secondary-a");
        vcx.simulate_keystrokes("f o o space f o o");
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, cx| {
            let input = app.active_editor_input().unwrap();
            input.set_selected_range(0..3, cx);
            input.focus(window, cx);
        });
        vcx.simulate_keystrokes("secondary-shift-l");
        vcx.simulate_keystrokes("x");
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer()
                    .unwrap()
                    .input
                    .read(cx)
                    .text()
                    .to_string(),
                "x x"
            );
        });
        vcx.simulate_keystrokes("secondary-z");
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer()
                    .unwrap()
                    .input
                    .read(cx)
                    .text()
                    .to_string(),
                "foo foo"
            );
        });
    }
    #[gpui_kit::test]
    fn markdown_line_commands_preserve_mixed_endings_and_undo(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed-commands.md");
        std::fs::write(&path, b"one\r\ntwo\nthree\r\n").unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.read_with(&vcx, |app, _| app.active_buffer().is_some()) {
            vcx.run_until_parked();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.update_in(&mut vcx, |app, window, cx| {
            app.toggle_document_preview(window, cx);
            let input = app.active_editor_input().unwrap();
            input.set_selected_range(4..7, cx);
            input.focus(window, cx);
        });
        vcx.dispatch_action(MoveLineUp);
        app.read_with(&vcx, |app, cx| {
            let input = app.active_editor_input().unwrap();
            assert_eq!(
                input
                    .markdown()
                    .unwrap()
                    .read(cx)
                    .document()
                    .read(cx)
                    .encode()
                    .unwrap(),
                b"two\none\r\nthree\r\n"
            );
        });
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        vcx.dispatch_action(CopyLineDown);
        app.read_with(&vcx, |app, cx| {
            let input = app.active_editor_input().unwrap();
            assert_eq!(
                input
                    .markdown()
                    .unwrap()
                    .read(cx)
                    .document()
                    .read(cx)
                    .encode()
                    .unwrap(),
                b"one\r\ntwo\ntwo\nthree\r\n"
            );
        });
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        app.read_with(&vcx, |app, cx| {
            let input = app.active_editor_input().unwrap();
            assert_eq!(
                input
                    .markdown()
                    .unwrap()
                    .read(cx)
                    .document()
                    .read(cx)
                    .encode()
                    .unwrap(),
                b"one\r\ntwo\nthree\r\n"
            );
            assert_eq!(input.read(cx).selected_range(), 4..7);
        });
    }

    #[gpui_kit::test]
    fn markdown_repeated_multiline_indent_preserves_selection_and_undo(cx: &mut TestAppContext) {
        use gpui_kit::test::TestWindowExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("indent.md");
        std::fs::write(&path, "one\ntwo\nthree").unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.read_with(&vcx, |app, _| app.active_buffer().is_some()) {
            vcx.run_until_parked();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.update_in(&mut vcx, |app, window, cx| {
            app.toggle_document_preview(window, cx);
            let input = app.active_editor_input().unwrap();
            input.set_selected_range(0..7, cx);
            input.focus(window, cx);
        });
        vcx.update(|window, cx| {
            window.render_frame(cx);
            window.press("tab", cx);
            window.press("tab", cx);
        });
        app.read_with(&vcx, |app, cx| {
            let input = app.active_editor_input().unwrap();
            assert_eq!(
                input.read(cx).text().to_string(),
                "        one\n        two\nthree"
            );
            assert_eq!(input.read(cx).selected_range(), 8..23);
        });
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        app.read_with(&vcx, |app, cx| {
            let input = app.active_editor_input().unwrap();
            assert_eq!(input.read(cx).text().to_string(), "one\ntwo\nthree");
            assert_eq!(input.read(cx).selected_range(), 0..7);
        });
    }

    #[gpui_kit::test]
    fn markdown_explicit_commands_and_repeated_occurrences_keep_their_selection_scope(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("primary-command.md");
        std::fs::write(&path, "foo foo foo").unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.read_with(&vcx, |app, _| app.active_buffer().is_some()) {
            vcx.run_until_parked();
            assert!(
                std::time::Instant::now() < deadline,
                "Markdown open did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.update_in(&mut vcx, |app, window, cx| {
            app.toggle_document_preview(window, cx)
        });
        app.update_in(&mut vcx, |app, window, cx| {
            let input = app.active_editor_input().unwrap();
            input.set_selected_range(0..3, cx);
            input.focus(window, cx);
            assert!(app.editor_apply_command(Command::All, window, cx));
            assert!(app.editor_apply_command(Command::Upper, window, cx));
        });
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            let input = app.active_editor_input().unwrap();
            assert_eq!(input.read(cx).text().to_string(), "FOO foo foo");
            assert_eq!(input.read(cx).selected_range(), 0..3);
        });
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_editor_input()
                    .unwrap()
                    .read(cx)
                    .text()
                    .to_string(),
                "foo foo foo"
            )
        });
        app.update_in(&mut vcx, |app, window, cx| {
            let input = app.active_editor_input().unwrap();
            input.set_selected_range(0..3, cx);
            input.focus(window, cx);
            assert!(app.editor_apply_command(Command::Next, window, cx));
            assert!(app.editor_apply_command(Command::Next, window, cx));
        });
        vcx.simulate_keystrokes("x");
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_editor_input()
                    .unwrap()
                    .read(cx)
                    .text()
                    .to_string(),
                "x x x"
            )
        });
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_editor_input()
                    .unwrap()
                    .read(cx)
                    .text()
                    .to_string(),
                "foo foo foo"
            )
        });
    }
    #[gpui_kit::test]
    fn markdown_source_commands_apply_and_undo_through_the_workbench(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inventory.md");
        std::fs::write(&path, "initial").unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.read_with(&vcx, |app, _| app.active_buffer().is_some()) {
            vcx.run_until_parked();
            assert!(
                std::time::Instant::now() < deadline,
                "Markdown open did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.update_in(&mut vcx, |app, window, cx| {
            app.toggle_document_preview(window, cx)
        });
        for (command, before, selection, expected) in [
            (Command::Comment, "中", 0..3, "<!-- 中 -->"),
            (Command::BlockComment, "中", 0..3, "<!-- 中 -->"),
            (Command::MoveUp, "a\n中", 2..5, "中\na"),
            (Command::MoveDown, "a\n中", 0..1, "中\na"),
            (Command::CopyUp, "中", 0..3, "中\n中"),
            (Command::CopyDown, "中", 0..3, "中\n中"),
            (Command::Delete, "a\n中", 2..5, "a"),
            (Command::InsertAbove, "a\nb", 2..2, "a\n\nb"),
            (Command::InsertBelow, "a\nb", 2..2, "a\nb\n"),
            (Command::Upper, "aBc", 0..3, "ABC"),
            (Command::Lower, "aBc", 0..3, "abc"),
            (Command::Title, "hello world", 0..11, "Hello World"),
            (Command::Trim, "a  \nb\t", 0..0, "a\nb"),
            (Command::Join, "a\n b", 0..0, "a b"),
            (Command::Unwrap, "(中)", 1..4, "中"),
        ] {
            app.update_in(&mut vcx, |app, window, cx| {
                let input = app.active_editor_input().unwrap();
                let len = input.read(cx).text().len();
                input.replace(0..len, before, window, cx);
                input.set_selected_range(selection, cx);
                input.focus(window, cx);
                assert!(app.editor_apply_command(command, window, cx));
            });
            vcx.run_until_parked();
            app.read_with(&vcx, |app, cx| {
                assert_eq!(
                    app.active_buffer()
                        .unwrap()
                        .input
                        .read(cx)
                        .text()
                        .to_string(),
                    expected
                )
            });
            vcx.dispatch_action(gpui_kit::component::input::Undo);
            vcx.run_until_parked();
            app.read_with(&vcx, |app, cx| {
                assert_eq!(
                    app.active_buffer()
                        .unwrap()
                        .input
                        .read(cx)
                        .text()
                        .to_string(),
                    before
                )
            });
        }
        app.update_in(&mut vcx, |app, window, cx| {
            let input = app.active_editor_input().unwrap();
            let len = input.read(cx).text().len();
            input.replace(0..len, "(中)\nfoo foo foo\n", window, cx);
            input.set_selected_range(0..0, cx);
            assert!(app.editor_apply_command(Command::Bracket, window, cx));
            assert_eq!(input.read(cx).selected_range(), 4..4);
            input.set_selected_range(7..7, cx);
            assert!(app.editor_apply_command(Command::SelectLine, window, cx));
            assert_eq!(input.read(cx).selected_range(), 6..18);
            input.set_selected_range(7..7, cx);
            assert!(app.editor_apply_command(Command::Next, window, cx));
            assert_eq!(input.read(cx).selected_range(), 6..9);
            assert!(app.editor_apply_command(Command::Skip, window, cx));
            assert_eq!(input.read(cx).selected_range(), 10..13);
        });
    }
}
