use super::*;
use gpui_kit::component::{Sizable as _, button::{Button, ButtonVariants as _}, highlighter::Language, menu::DropdownMenu as _};

#[derive(Clone, Debug, PartialEq, gpui_kit::Action)]
#[action(namespace = markdown_code, no_json)]
struct ChooseLanguage {
    id: u64,
    revision: u64,
    language: String,
}
#[derive(Clone, Debug, PartialEq, gpui_kit::Action)]
#[action(namespace = markdown_code, no_json)]
struct CopyBody { id: u64, revision: u64 }
#[derive(Clone, Debug, PartialEq, gpui_kit::Action)]
#[action(namespace = markdown_code, no_json)]
struct ToggleSource { id: u64, revision: u64 }

gpui_kit::actions!(markdown_code, [FocusControls, ReturnFocus]);
struct Bindings;
impl Global for Bindings {}
pub(super) fn init(cx: &mut App) {
    if !cx.has_global::<Bindings>() {
        cx.set_global(Bindings);
        cx.bind_keys([
            KeyBinding::new("f6", FocusControls, Some("Markdown")),
            KeyBinding::new("escape", ReturnFocus, Some("MarkdownCodeControls")),
        ]);
    }
}

#[derive(Default)]
pub(super) struct State {
    source: Option<u64>,
    revision: Option<u64>,
    fences: HashMap<u64, Fence>,
    controls: HashMap<u64, FocusHandle>,
    terminal_preedit: Option<(usize, String, bool, bool)>,
}

/// Derived source slices only. Fence spelling and container prefixes never become
/// another buffer; every mutation is submitted to MarkdownDocument's history.
struct Fence {
    language: Range<usize>,
    opening: Range<usize>,
    closing: Option<Range<usize>>,
    body: Range<usize>,
    hidden: Vec<Range<usize>>,
    prefix: String,
    ending: String,
}
impl Fence {
    fn read(raw: &str, start: usize, kind: Kind) -> Option<Self> {
        let Kind::Code(super::super::regions::Code::Fenced { marker, width }) = kind else { return None; };
        let first_end = raw.find('\n').unwrap_or(raw.len());
        let first = &raw[..first_end];
        let fence = first.bytes().position(|byte| byte == marker)?;
        if first[fence..].bytes().take_while(|byte| *byte == marker).count() < width { return None; }
        let info = fence + first[fence..].bytes().take_while(|byte| *byte == marker).count();
        let language_start = info + first[info..].len() - first[info..].trim_start().len();
        let language_end = language_start + first[language_start..].find(char::is_whitespace).unwrap_or(first.len() - language_start);
        let prefix: String = first[..fence].chars().map(|ch| if matches!(ch, '>' | '\t' | ' ') { ch } else { ' ' }).collect();
        let body_start = (first_end + usize::from(first_end < raw.len())).min(raw.len());
        let mut closing = None;
        let mut hidden = vec![start..start + body_start];
        let mut offset = body_start;
        for line in raw[body_start..].split_inclusive('\n') {
            let content = line.strip_prefix(&prefix).unwrap_or(line);
            let trimmed = content.trim_start_matches(' ');
            let indentation = content.len() - trimmed.len();
            let count = trimmed.bytes().take_while(|byte| *byte == marker).count();
            if indentation <= 3 && count >= width && trimmed[count..].trim().is_empty() {
                closing = Some(start + offset..start + offset + line.trim_end_matches('\n').len());
                hidden.push(start + offset..start + raw.len());
                break;
            }
            if content.len() != line.len() {
                hidden.push(start + offset..start + offset + prefix.len());
            }
            offset += line.len();
        }
        let body_end = closing.as_ref().map_or(start + raw.len(), |range| range.start);
        Some(Self {
            language: start + language_start..start + language_end,
            opening: start..start + first_end,
            closing,
            body: start + body_start..body_end,
            hidden,
            ending: format!("{prefix}{}", String::from_utf8(vec![marker; width]).expect("ASCII fence")),
            prefix,
        })
    }
    fn on_fence(&self, selection: Selection) -> bool {
        let range = selection.range();
        if range.is_empty() && self.body.is_empty() && range.start == self.body.start { return false; }
        [&self.opening].into_iter().chain(self.closing.iter()).any(|fence| {
            if range.is_empty() { fence.start <= range.start && range.start <= fence.end }
            else { range.start < fence.end && fence.start < range.end }
        })
    }
    fn copy(&self, raw: &str, start: usize) -> String {
        raw[self.body.start - start..self.body.end - start].split_inclusive('\n')
            .map(|line| line.strip_prefix(&self.prefix).unwrap_or(line)).collect()
    }
}

fn body_input(text: &str, prefix: &str, first: bool, terminal: bool, selected: Option<Range<usize>>) -> (String, Option<Range<usize>>, usize) {
    let text = text.replace("\r\n", "\n");
    let mut output = if first { prefix.to_owned() } else { String::new() };
    let mut output16 = output.encode_utf16().count();
    let prefix16 = prefix.encode_utf16().count();
    let mut input16 = 0;
    let mut mapped = (output16, output16);
    for (ix, ch) in text.char_indices() {
        if let Some(range) = &selected {
            if input16 == range.start { mapped.0 = output16; }
            if input16 == range.end { mapped.1 = output16; }
        }
        output.push(ch);
        input16 += ch.len_utf16();
        output16 += ch.len_utf16();
        if ch == '\n' && !(terminal && ix + 1 == text.len()) {
            output.push_str(prefix);
            output16 += prefix16;
        }
    }
    if let Some(range) = &selected {
        if range.start >= input16 { mapped.0 = output16; }
        if range.end >= input16 { mapped.1 = output16; }
    }
    let caret = output.len();
    if terminal && !output.ends_with('\n') { output.push('\n'); }
    (output, selected.map(|_| mapped.0..mapped.1), caret)
}

impl SourcePresentation {
    fn code_fence(&self, id: u64, revision: u64, cx: &App) -> Option<&Fence> {
        if self.document.read(cx).revision() != revision || self.projection.revision != revision || self.code.revision != Some(revision) { return None; }
        self.code.fences.get(&id)
    }
    pub(super) fn sync_code(&mut self, cx: &mut Context<Self>) {
        let revision = self.document.read(cx).revision();
        if !self.live || self.projection.revision != revision { return; }
        let mut count = 0;
        let mut changed_projection = false;
        for leaf in &self.projection.leaves {
            if matches!(leaf.kind, Kind::Code(super::super::regions::Code::Fenced { .. })) {
                count += 1;
                changed_projection |= !self.code.fences.contains_key(&leaf.id);
            }
        }
        if self.code.revision != Some(revision) || changed_projection || count != self.code.fences.len() {
            self.code.fences.clear();
            for leaf in &self.projection.leaves {
                if !matches!(leaf.kind, Kind::Code(super::super::regions::Code::Fenced { .. })) { continue; }
                let raw = self.document.read(cx).source().slice(leaf.range.clone()).to_string();
                if let Some(fence) = Fence::read(&raw, leaf.range.start, leaf.kind) {
                    self.code.fences.insert(leaf.id, fence);
                    self.code.controls.entry(leaf.id).or_insert_with(|| cx.focus_handle().tab_stop(true));
                }
            }
            self.code.controls.retain(|id, _| self.code.fences.contains_key(id));
            self.code.revision = Some(revision);
        }
        let marked = self.state.marked_range();
        for leaf in &mut self.projection.leaves {
            let Some(fence) = self.code.fences.get(&leaf.id) else { continue; };
            leaf.materialize(self.document.read(cx).source());
            let reveal = self.code.source == Some(leaf.id) || fence.on_fence(self.state.selection)
                || self.extra_selections.iter().any(|selection| fence.on_fence(*selection));
            let hidden = if reveal { &[][..] } else { fence.hidden.as_slice() };
            let changed = leaf.hide_ranges(hidden, marked.as_ref());
            let insertion_changed = leaf.set_inline_insertions(if reveal { &[] } else { std::slice::from_ref(&fence.body.start) });
            if changed || insertion_changed {
                if let Some(cached) = self.blocks.get_mut(&leaf.id) { cached.version = u64::MAX; }
                self.geometry = None;
                self.viewport.heights.remove(&leaf.id);
            }
        }
    }
    pub(super) fn code_language(&self, leaf: &super::super::projection::Leaf, cx: &App) -> Option<SharedString> {
        let fence = self.code_fence(leaf.id, self.projection.revision, cx)?;
        Some(self.document.read(cx).source().slice(fence.language.clone()).to_string().into())
    }
    fn choose_code_language(&mut self, action: &ChooseLanguage, window: &mut Window, cx: &mut Context<Self>) {
        self.command(Command::FinishComposition, window, cx);
        let Some(fence) = self.code_fence(action.id, action.revision, cx) else { return; };
        let metadata = self.document.read(cx).source().slice(fence.language.end..fence.opening.end).to_string();
        let language = if action.language == "text" && metadata.trim().is_empty() { "" } else { action.language.as_str() };
        let old = self.document.read(cx).source().slice(fence.language.clone()).to_string();
        if old != language {
            let at = fence.body.start - fence.language.len() + language.len();
            self.command(Command::BatchReplaceSelected {
                edits: vec![(fence.language.clone(), language.to_owned())],
                selection: Selection::caret(at),
            }, window, cx);
        }
        self.focus.focus(window, cx);
    }
    fn copy_code_body(&mut self, action: &CopyBody, window: &mut Window, cx: &mut Context<Self>) {
        let Some(fence) = self.code_fence(action.id, action.revision, cx) else { return; };
        let leaf = self.projection.leaves.iter().find(|leaf| leaf.id == action.id).expect("current code leaf");
        let raw = self.document.read(cx).source().slice(leaf.range.clone()).to_string();
        let code = fence.copy(&raw, leaf.range.start);
        cx.write_to_clipboard(ClipboardItem::new_string(code));
        self.focus.focus(window, cx);
    }
    fn toggle_code_source(&mut self, action: &ToggleSource, window: &mut Window, cx: &mut Context<Self>) {
        self.command(Command::FinishComposition, window, cx);
        let Some(fence) = self.code_fence(action.id, action.revision, cx) else { return; };
        let showing = self.code.source == Some(action.id);
        let at = if showing { fence.body.start } else { fence.language.start };
        self.code.source = (!showing).then_some(action.id);
        self.sync_code(cx);
        self.command(Command::Select(Selection::caret(at)), window, cx);
        self.focus.focus(window, cx);
    }
    pub(super) fn code_input(&mut self, command: Command, cx: &App) -> (Command, Option<usize>) {
        if matches!(command, Command::CancelComposition | Command::FinishComposition) {
            self.code.terminal_preedit = None;
            return (command, None);
        }
        if !self.live || !self.extra_selections.is_empty() { return (command, None); }
        let range = match &command {
            Command::Compose { range, .. } => range.clone().or_else(|| self.state.marked_range()).unwrap_or_else(|| self.state.selection.range()),
            Command::Commit(_) => self.state.marked_range().unwrap_or_else(|| self.state.selection.range()),
            Command::Insert(_) => self.state.selection.range(),
            Command::Replace { range, kind: EditKind::Typing | EditKind::Paste, .. } => range.clone(),
            _ => return (command, None),
        };
        let (prefix, first, terminal) = if let Some((start, prefix, first, terminal)) = self.code.terminal_preedit.as_ref().filter(|(start, _, _, _)| self.state.marked_range().is_some_and(|marked| marked.start == *start)) {
            if *start != range.start { return (command, None); }
            (prefix.clone(), *first, *terminal)
        } else {
            let Some(leaf) = self.projection.leaf_at(range.start) else { return (command, None); };
            let Some(fence) = self.code_fence(leaf.id, self.projection.revision, cx) else { return (command, None); };
            if self.code.source == Some(leaf.id) || range.start < fence.body.start || range.end > fence.body.end { return (command, None); }
            let terminal = fence.closing.is_some() && range.end == fence.body.end;
            let text = match &command { Command::Compose { text, .. } | Command::Commit(text) | Command::Insert(text) | Command::Replace { text, .. } => text, _ => unreachable!() };
            if !terminal && (fence.prefix.is_empty() || !text.contains('\n')) { return (command, None); }
            (fence.prefix.clone(), terminal && range.start == fence.body.end, terminal)
        };
        match command {
            Command::Compose { range: replacement, text, selected } if !text.is_empty() => {
                let length16 = text.encode_utf16().count();
                let (text, selected, _) = body_input(&text, &prefix, first, terminal, Some(selected.unwrap_or(length16..length16)));
                self.code.terminal_preedit = Some((range.start, prefix, first, terminal));
                (Command::Compose { range: replacement, text, selected }, None)
            }
            Command::Commit(text) if self.state.composition.is_some() => {
                self.code.terminal_preedit = None;
                let (text, _, caret) = body_input(&text, &prefix, first, terminal, None);
                (Command::Commit(text), Some(range.start + caret))
            }
            Command::Commit(text) | Command::Insert(text) | Command::Replace { text, .. } if !text.is_empty() => {
                let (text, _, caret) = body_input(&text, &prefix, first, terminal, None);
                (Command::BatchReplaceSelected { edits: vec![(range.clone(), text)], selection: Selection::caret(range.start + caret) }, None)
            }
            command => (command, None),
        }
    }
    pub(super) fn code_indent(&mut self, outdent: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.live || !self.extra_selections.is_empty() { return false; }
        self.sync_code(cx);
        let Some(leaf) = self.projection.leaf_at(self.state.selection.head) else { return false; };
        let Some(fence) = self.code_fence(leaf.id, self.projection.revision, cx) else { return false; };
        let selection = self.state.selection;
        let selected = selection.range();
        if selected.start < fence.body.start || selected.end > fence.body.end { return false; }
        let unit = if self.indent.hard_tabs { "\t".to_owned() } else { " ".repeat(self.indent.size) };
        if !outdent && selected.is_empty() {
            self.command(Command::Insert(unit), window, cx);
            return true;
        }
        let document = self.document.read(cx);
        let first = document.source().offset_to_point(selected.start).row;
        let last = document.source().offset_to_point(selected.end).row;
        let mut edits = Vec::new();
        for row in first..=last {
            let line = document.source().line_start_offset(row);
            if line >= fence.body.end || row > first && line == selected.end { break; }
            let end = document.source().line_end_offset(row);
            let raw = document.source().slice(line..end).to_string();
            let prefix = if raw.starts_with(&fence.prefix) { fence.prefix.len() } else { 0 };
            let at = line + prefix;
            if outdent {
                let code = &raw[prefix..];
                let width = if code.starts_with('\t') { 1 } else { code.bytes().take(self.indent.size).take_while(|byte| *byte == b' ').count() };
                if width > 0 { edits.push((at..at + width, String::new())); }
            } else {
                edits.push((at..at, unit.clone()));
            }
        }
        let mut mapped = selection;
        for (range, text) in edits.iter().rev() {
            let map = |at: usize| if at <= range.start { at } else if at >= range.end { at - range.len() + text.len() } else { range.start + text.len() };
            mapped = Selection::new(map(mapped.anchor), map(mapped.head));
        }
        if !edits.is_empty() {
            self.command(Command::BatchReplaceSelected { edits, selection: mapped }, window, cx);
        }
        true
    }
    pub(super) fn code_delete(&mut self, backward: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.live || !self.extra_selections.is_empty() { return false; }
        self.sync_code(cx);
        let selected = self.state.selection.range();
        let at = self.state.selection.head;
        let Some(leaf) = self.projection.leaf_at(at) else { return false; };
        let Some(fence) = self.code_fence(leaf.id, self.projection.revision, cx) else {
            if !selected.is_empty() { return false; }
            let edge = if backward { leaf.content_start() } else { leaf.content_end() };
            if at != edge { return false; }
            let adjacent = if backward {
                self.projection.leaves.iter().rev().find(|other| other.range.end < at && (!other.text.is_empty() || matches!(other.kind, Kind::Code(_))))
            } else {
                self.projection.leaves.iter().find(|other| other.range.start > at && (!other.text.is_empty() || matches!(other.kind, Kind::Code(_))))
            };
            let target = adjacent.and_then(|other| self.code_fence(other.id, self.projection.revision, cx)).map(|fence| {
                if backward { fence.body.end.saturating_sub(usize::from(fence.body.end > fence.body.start)) } else { fence.body.start }
            });
            if let Some(at) = target {
                self.command(Command::Select(Selection::caret(at)), window, cx);
                return true;
            }
            return false;
        };
        if !selected.is_empty() {
            if selected.start >= fence.body.start && selected.end == fence.body.end && self.code.source != Some(leaf.id) {
                let text = if selected.start == fence.body.start { String::new() } else { "\n".into() };
                self.command(Command::Replace { range: selected, text, kind: EditKind::Command }, window, cx);
                return true;
            }
            return false;
        }
        if self.code.source == Some(leaf.id) || at < fence.body.start || at > fence.body.end { return false; }
        let document = self.document.read(cx);
        let row = document.source().offset_to_point(at).row;
        let start = document.source().line_start_offset(row);
        let end = document.source().line_end_offset(row);
        let raw = document.source().slice(start..end).to_string();
        let prefix = if raw.starts_with(&fence.prefix) { fence.prefix.len() } else { 0 };
        let first = fence.body.start + prefix;
        let last = fence.body.end.saturating_sub(usize::from(fence.closing.is_some() && fence.body.end > fence.body.start));
        if backward && at <= first || !backward && at >= last {
            return true; // Invisible delimiters are not Backspace/Delete targets.
        }
        let range = if backward && at == start + prefix {
            start - 1..at
        } else if !backward && at == end {
            let next_end = document.source().line_end_offset(row + 1);
            let next = document.source().slice(end + 1..next_end).to_string();
            at..end + 1 + if next.starts_with(&fence.prefix) { fence.prefix.len() } else { 0 }
        } else {
            return false;
        };
        self.command(Command::Replace { range, text: String::new(), kind: EditKind::Command }, window, cx);
        true
    }
    pub(super) fn code_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.live { return false; }
        self.sync_code(cx);
        let Some(leaf) = self.projection.leaf_at(self.state.selection.head) else { return false; };
        let Some(fence) = self.code_fence(leaf.id, self.projection.revision, cx) else { return false; };
        let range = self.state.selection.range();
        if fence.body.start <= range.start && range.end <= fence.body.end {
            // Code Enter is deliberately not Markdown continuation or smart indent.
            let text = if fence.body.is_empty() { format!("{}\n", fence.prefix) } else { format!("\n{}", fence.prefix) };
            self.command(Command::Replace { range, text, kind: EditKind::Command }, window, cx);
            return true;
        }
        false
    }
    pub(super) fn exit_code(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.live { return false; }
        self.sync_code(cx);
        let Some(leaf) = self.projection.leaf_at(self.state.selection.head) else { return false; };
        let Some(fence) = self.code_fence(leaf.id, self.projection.revision, cx) else { return false; };
        let at = fence.body.start;
        let end = leaf.range.end;
        let return_to_body = self.code.source.is_some() || fence.on_fence(self.state.selection);
        let ending = fence.closing.is_none().then(|| format!("\n{}\n\n", fence.ending));
        self.code.source = None;
        if return_to_body {
            self.command(Command::Select(Selection::caret(at)), window, cx);
        } else if let Some(text) = ending {
            self.command(Command::Replace { range: end..end, text, kind: EditKind::Command }, window, cx);
        } else if end == self.document.read(cx).source().len() {
            self.command(Command::Replace { range: end..end, text: "\n\n".into(), kind: EditKind::Command }, window, cx);
        } else {
            self.command(Command::Select(Selection::caret(end + 1)), window, cx);
        }
        self.focus.focus(window, cx);
        true
    }
    pub(super) fn code_controls(&self, leaf: &super::super::projection::Leaf, content: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let revision = self.projection.revision;
        let Some(fence) = self.code_fence(leaf.id, revision, cx) else { return content; };
        let id = leaf.id;
        let language = self.document.read(cx).source().slice(fence.language.clone()).to_string();
        let focus = self.focus.clone();
        let copy_view = cx.entity().downgrade();
        let source_view = cx.entity().downgrade();
        let selected = if language.is_empty() { "text".to_owned() } else { language };
        div().w_full().min_w_0().flex().flex_col()
            .child(gpui_kit::component::h_flex().gap_1().flex_wrap()
                .id(("markdown-code-controls", id))
                .key_context("MarkdownCodeControls")
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(Button::new(("markdown-code-language", id)).small().ghost()
                    .track_focus(self.code.controls.get(&id).expect("code language focus"))
                    .label(if selected == "text" { t(L10nKey::MarkdownCodePlain) } else { selected.clone().into() }).tooltip(t(L10nKey::MarkdownCodeLanguage))
                    .dropdown_menu(move |menu, _, _| {
                        let mut menu = menu.action_context(focus.clone());
                        let mut languages: Vec<_> = Language::all().map(|language| language.name()).filter(|name| *name != "text").collect();
                        languages.sort_unstable();
                        languages.dedup();
                        for language in std::iter::once("text").chain(languages) {
                            let label = if language == "text" { t(L10nKey::MarkdownCodePlain) } else { language.into() };
                            menu = menu.menu_with_check(label, selected == language, Box::new(ChooseLanguage { id, revision, language: language.to_owned() }));
                        }
                        menu
                    }))
                .child(Button::new(("markdown-code-copy", id)).small().ghost().label(t(L10nKey::MarkdownCodeCopy))
                    .on_click(move |_, window, cx| { let _ = copy_view.update(cx, |this, cx| this.copy_code_body(&CopyBody { id, revision }, window, cx)); }))
                .child(Button::new(("markdown-code-source", id)).small().ghost()
                    .label(t(if self.code.source == Some(id) { L10nKey::MarkdownCodeLive } else { L10nKey::MarkdownCodeSource }))
                    .on_click(move |_, window, cx| { let _ = source_view.update(cx, |this, cx| this.toggle_code_source(&ToggleSource { id, revision }, window, cx)); })))
            .child(content).into_any_element()
    }
    pub(super) fn code_actions(&self, root: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        root.on_action(cx.listener(|this, action: &ChooseLanguage, window, cx| this.choose_code_language(action, window, cx)))
            .on_action(cx.listener(|this, action: &CopyBody, window, cx| this.copy_code_body(action, window, cx)))
            .on_action(cx.listener(|this, action: &ToggleSource, window, cx| this.toggle_code_source(action, window, cx)))
            .on_action(cx.listener(|this, _: &FocusControls, window, cx| {
                if let Some(focus) = this.projection.leaf_at(this.state.selection.head).and_then(|leaf| this.code.controls.get(&leaf.id)) {
                    focus.focus(window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &ReturnFocus, window, cx| this.focus.focus(window, cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::{Fence, body_input};
    use crate::ui::code_editor::markdown::{projection::Kind, regions::Code};
    #[test]
    fn nested_fence_copy_has_no_container_or_fence_syntax() {
        let raw = "> ~~~~rust title=x\n> - literal\n> \n> ~~~~";
        let fence = Fence::read(raw, 7, Kind::Code(Code::Fenced { marker: b'~', width: 4 })).unwrap();
        assert_eq!(fence.copy(raw, 7), "- literal\n\n");
        assert_eq!(&raw[fence.language.start - 7..fence.language.end - 7], "rust");
    }
    #[test]
    fn empty_nested_code_input_preserves_fence_and_utf16_caret() {
        let (text, selected, caret) = body_input("中🎉", "> ", true, true, Some(3..3));
        assert_eq!(text, "> 中🎉\n");
        assert_eq!(selected, Some(5..5));
        assert_eq!(caret, "> 中🎉".len());
        let (text, _, _) = body_input("one\ntwo\n", "> ", true, true, None);
        assert_eq!(text, "> one\n> two\n");
    }
}
