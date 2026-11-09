//! Slash authoring is a transient command lease on the existing document.
//! The inventory is fixed Zorite 9c09 `SNIPPETS`; notebook templates are not
//! applicable (reference-object-full F6/O). No template store is created.
use super::*;
use gpui_kit::component::{
    IndexPath, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    list::{List, ListDelegate, ListEvent, ListItem, ListState},
};
struct Bindings;
impl Global for Bindings {}
pub(super) fn init(cx: &mut App) {
    if cx.has_global::<Bindings>() {
        return;
    }
    cx.set_global(Bindings);
    cx.bind_keys([
        KeyBinding::new("up", actions::MoveUp, Some("MarkdownSlashGrid")),
        KeyBinding::new("down", actions::MoveDown, Some("MarkdownSlashGrid")),
        KeyBinding::new("left", actions::MoveLeft, Some("MarkdownSlashGrid")),
        KeyBinding::new("right", actions::MoveRight, Some("MarkdownSlashGrid")),
        KeyBinding::new(
            "enter",
            actions::Enter {
                secondary: false,
                shift: false,
            },
            Some("MarkdownSlashGrid"),
        ),
        KeyBinding::new("escape", actions::Escape, Some("MarkdownSlashPicker")),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Root,
    Markdown,
}
#[derive(Clone, Copy)]
enum Operation {
    Category,
    Insert(&'static str, usize),
    Table,
    Property,
    Date,
    Time,
}
#[derive(Clone)]
struct Entry {
    id: &'static str,
    key: L10nKey,
    alias: &'static str,
    operation: Operation,
}
impl Entry {
    fn allowed_in_cell(&self) -> bool {
        match self.operation {
            Operation::Insert(text, _) => {
                !text.contains(['\n', '|'])
                    && matches!(
                        self.id,
                        "bold"
                            | "italic"
                            | "strike"
                            | "inline-code"
                            | "inline-math"
                            | "highlight"
                            | "underline"
                            | "link"
                            | "wiki"
                            | "image"
                    )
            }
            Operation::Category | Operation::Date | Operation::Time => true,
            Operation::Table | Operation::Property => false,
        }
    }
}
fn entries() -> Vec<Entry> {
    use L10nKey::*;
    let mut entries: Vec<_> = [
        ("h1", MarkdownBlockHeading1, "heading 1 h1", "# ", 2),
        ("h2", MarkdownBlockHeading2, "heading 2 h2", "## ", 3),
        ("h3", MarkdownBlockHeading3, "heading 3 h3", "### ", 4),
        ("bullet", MarkdownBlockBullet, "bullet list", "- ", 2),
        ("numbered", MarkdownBlockNumbered, "numbered list", "1. ", 3),
        ("todo", MarkdownBlockTask, "to-do todo task", "- [ ] ", 6),
        ("quote", MarkdownBlockQuote, "quote", "> ", 2),
        (
            "note",
            MarkdownBlockNote,
            "note alert callout",
            "> [!NOTE] ",
            10,
        ),
        ("tip", MarkdownBlockTip, "tip alert callout", "> [!TIP] ", 9),
        (
            "important",
            MarkdownBlockImportant,
            "important alert callout",
            "> [!IMPORTANT] ",
            15,
        ),
        (
            "warning",
            MarkdownBlockWarning,
            "warning alert callout",
            "> [!WARNING] ",
            13,
        ),
        (
            "caution",
            MarkdownBlockCaution,
            "caution alert callout",
            "> [!CAUTION] ",
            13,
        ),
        ("code", MarkdownBlockCode, "code block", "```\n\n```", 4),
        (
            "mermaid",
            EditorMarkdownMermaidSource,
            "mermaid diagram",
            "```mermaid\n\n```",
            11,
        ),
        ("math", MarkdownBlockMath, "math", "$$\n\n$$", 3),
        ("divider", MarkdownSlashDivider, "divider", "---\n", 4),
        ("bold", MarkdownFormatBold, "bold", "****", 2),
        ("italic", MarkdownFormatItalic, "italic", "**", 1),
        ("strike", MarkdownFormatStrike, "strikethrough", "~~~~", 2),
        ("inline-code", MarkdownFormatCode, "inline code", "``", 1),
        (
            "inline-math",
            MarkdownSlashInlineMath,
            "inline math",
            "$$",
            1,
        ),
        (
            "highlight",
            MarkdownFormatHighlight,
            "highlight",
            "<mark></mark>",
            6,
        ),
        (
            "underline",
            MarkdownFormatUnderline,
            "underline",
            "<u></u>",
            3,
        ),
        ("link", MarkdownSlashLink, "link", "[]()", 1),
        ("wiki", MarkdownSlashWiki, "wiki link", "[[]]", 2),
        ("image", MarkdownSlashImage, "image", "![]()", 4),
    ]
    .into_iter()
    .map(|(id, key, alias, text, caret)| Entry {
        id,
        key,
        alias,
        operation: Operation::Insert(text, caret),
    })
    .collect();
    entries.insert(
        15,
        Entry {
            id: "table",
            key: EditorTableInsert,
            alias: "table",
            operation: Operation::Table,
        },
    );
    entries.push(Entry {
        id: "property",
        key: EditorPropertyAdd,
        alias: "property",
        operation: Operation::Property,
    });
    entries
}
fn date_time(time: bool) -> String {
    chrono::Local::now()
        .format(if time { "%H:%M" } else { "%Y-%m-%d" })
        .to_string()
}
fn filtered(level: Level, query: &str) -> Vec<Entry> {
    let query = query.to_lowercase();
    if level == Level::Root && query.is_empty() {
        return vec![
            Entry {
                id: "markdown",
                key: L10nKey::MarkdownSlashMarkdown,
                alias: "markdown",
                operation: Operation::Category,
            },
            Entry {
                id: "date",
                key: L10nKey::MarkdownSlashDate,
                alias: "date",
                operation: Operation::Date,
            },
            Entry {
                id: "time",
                key: L10nKey::MarkdownSlashTime,
                alias: "time",
                operation: Operation::Time,
            },
        ];
    }
    let mut result = entries();
    if level == Level::Root {
        result.extend([
            Entry {
                id: "date",
                key: L10nKey::MarkdownSlashDate,
                alias: "date",
                operation: Operation::Date,
            },
            Entry {
                id: "time",
                key: L10nKey::MarkdownSlashTime,
                alias: "time",
                operation: Operation::Time,
            },
        ]);
    }
    result.retain(|entry| {
        query.is_empty()
            || entry.alias.contains(&query)
            || t(entry.key).to_lowercase().contains(&query)
    });
    result
}
struct Lease {
    range: Range<usize>,
    revision: u64,
    resources: Option<super::super::resources::ResourceContext>,
    generation: u64,
}
struct Choices {
    entries: Vec<Entry>,
    owner: WeakEntity<SourcePresentation>,
    generation: u64,
    flyout: bool,
    table: bool,
}
impl ListDelegate for Choices {
    type Item = ListItem;
    fn items_count(&self, _: usize, _: &App) -> usize {
        self.entries.len()
    }
    fn set_selected_index(
        &mut self,
        _: Option<IndexPath>,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        cx.notify();
    }
    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let entry = self.entries.get(ix.row)?;
        let label = match entry.operation {
            Operation::Category => format!("{} ›", t(entry.key)),
            Operation::Date => format!("{} ({})", t(entry.key), date_time(false)),
            Operation::Time => format!("{} ({})", t(entry.key), date_time(true)),
            _ => t(entry.key).to_owned(),
        };
        let disabled = self.table && !entry.allowed_in_cell();
        let label = if disabled {
            format!("{label} · {}", t(L10nKey::MarkdownSlashBlockUnavailable))
        } else {
            label
        };
        let hover_owner = self.owner.clone();
        let accept_owner = self.owner.clone();
        let generation = self.generation;
        let flyout = self.flyout;
        Some(
            ListItem::new(SharedString::from(format!(
                "markdown-slash-{}-{}",
                if flyout { "flyout" } else { "entry" },
                entry.id
            )))
            .accessibility_label(label.clone())
            .disabled(disabled)
            .child(label)
            .on_mouse_enter(move |_, window, cx| {
                let _ = hover_owner.update(cx, |view, cx| {
                    view.select_slash(ix.row, flyout, generation, window, cx)
                });
            })
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                if disabled {
                    return;
                }
                cx.stop_propagation();
                let _ = accept_owner.update(cx, |view, cx| {
                    if !view.slash_current(generation, cx) {
                        return;
                    }
                    view.select_slash(ix.row, flyout, generation, window, cx);
                    view.accept_slash(window, cx);
                });
            }),
        )
    }
}
struct Picker {
    rows: usize,
    columns: usize,
    style: usize,
    focus: FocusHandle,
    custom_rows: Entity<InputState>,
    custom_columns: Entity<InputState>,
    error: bool,
}
#[derive(Default)]
pub(super) struct State {
    pub(super) generation: u64,
    lease: Option<Lease>,
    list: Option<Entity<ListState<Choices>>>,
    flyout: Option<Entity<ListState<Choices>>>,
    flyout_active: bool,
    picker: Option<Picker>,
    subscriptions: Vec<Subscription>,
}
impl SourcePresentation {
    pub(super) fn dismiss_slash(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.slash.lease.take().is_some();
        self.slash.generation = self.slash.generation.wrapping_add(1);
        self.slash.list = None;
        self.slash.flyout = None;
        self.slash.picker = None;
        self.slash.flyout_active = false;
        self.slash.subscriptions.clear();
        if open {
            cx.notify();
        }
        open
    }
    pub(super) fn slash_current(&self, generation: u64, cx: &App) -> bool {
        self.live
            && !self.is_composing()
            && self.extra_selections.is_empty()
            && self.slash.lease.as_ref().is_some_and(|lease| {
                let document = self.document.read(cx);
                lease.generation == generation
                    && generation == self.slash.generation
                    && lease.revision == document.revision()
                    && self.state.selection == Selection::caret(lease.range.end)
                    && match &lease.resources {
                        Some(resources) => resources.is_current(document.resources()),
                        None => document.resources().is_none(),
                    }
            })
    }
    fn slash_has_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx)
            || self.slash.picker.as_ref().is_some_and(|picker| {
                picker.focus.contains_focused(window, cx)
                    || picker
                        .custom_rows
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                    || picker
                        .custom_columns
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
            })
    }
    // Empty authoring pairs have no AST text run yet ("****" even parses as
    // a rule). Keep their actual insertion boundary until content exists.
    // This is derived from the sole source, so Undo/Redo needs no extra history.
    pub(super) fn empty_slash_inline(&self, at: usize, cx: &App) -> bool {
        if !self.live || self.is_composing() || !self.state.selection.range().is_empty() {
            return false;
        }
        let document = self.document.read(cx);
        let source = document.source();
        [
            ("**", "**"),
            ("*", "*"),
            ("~~", "~~"),
            ("`", "`"),
            ("$", "$"),
            ("<mark>", "</mark>"),
            ("<u>", "</u>"),
            ("[", "]()"),
            ("[[", "]]"),
            ("![](", ")"),
        ]
        .into_iter()
        .any(|(open, close)| {
            let Some(start) = at.checked_sub(open.len()) else {
                return false;
            };
            let end = at + close.len();
            end <= source.len()
                && document.boundary(start, false) == start
                && document.boundary(end, false) == end
                && source.slice(start..at).chars().eq(open.chars())
                && source.slice(at..end).chars().eq(close.chars())
        })
    }
    fn slash_choices(
        &mut self,
        entries: Vec<Entry>,
        flyout: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ListState<Choices>> {
        let table = self.active_table().is_some();
        let owner = cx.weak_entity();
        let generation = self.slash.generation;
        let list = cx.new(|cx| {
            ListState::new(
                Choices {
                    entries,
                    owner,
                    generation,
                    flyout,
                    table,
                },
                window,
                cx,
            )
        });
        if !list.read(cx).delegate().entries.is_empty() {
            list.update(cx, |list, cx| {
                list.set_selected_index(Some(IndexPath::new(0)), window, cx)
            });
        }
        self.slash.subscriptions.push(cx.subscribe_in(
            &list,
            window,
            move |view, _, event, window, cx| {
                if !view.slash_current(generation, cx) {
                    return;
                }
                match event {
                    ListEvent::Confirm(ix) => {
                        view.select_slash(ix.row, flyout, generation, window, cx);
                        view.accept_slash(window, cx);
                    }
                    ListEvent::Cancel => {
                        view.dismiss_slash(cx);
                        view.focus.focus(window, cx);
                    }
                    _ => {}
                }
            },
        ));
        list
    }
    pub(super) fn request_slash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_slash(cx);
        if !self.live
            || !self.focus.is_focused(window)
            || self.is_composing()
            || !self.extra_selections.is_empty()
            || !self.state.selection.range().is_empty()
        {
            return;
        }
        let document = self.document.read(cx);
        let end = self.state.selection.head();
        if self.formula.is_some()
            || self
                .projection
                .leaf_at(end)
                .is_some_and(|leaf| !leaf.accepts_reference(end))
        {
            return;
        }
        let row = document.source().offset_to_point(end).row;
        let start = document.source().line_start_offset(row);
        let line = document.source().slice(start..end).to_string();
        // Bracket completions remain owned by #222; template placeholders are
        // literal document text, never a route into notebook management.
        for (open, close) in [("[[", "]]"), ("{{", "}}"), ("((", "))")] {
            if let Some(at) = line.rfind(open) {
                let query = &line[at + open.len()..];
                if !query.contains(close) && !query.contains(open) {
                    return;
                }
            }
        }
        let Some(at) = line.rfind('/') else {
            return;
        };
        if at > 0
            && !line[..at]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
        {
            return;
        }
        let query = &line[at + 1..];
        if query.chars().any(|ch| !ch.is_alphanumeric() && ch != '-') {
            return;
        }
        let revision = document.revision();
        let resources = document.resources().cloned();
        self.dismiss_references(cx);
        self.slash.lease = Some(Lease {
            range: start + at..end,
            revision,
            resources,
            generation: self.slash.generation,
        });
        self.slash.subscriptions.push(cx.on_focus_out(
            &self.focus.clone(),
            window,
            |view, _, window, cx| {
                if !view.slash_has_focus(window, cx) {
                    view.dismiss_slash(cx);
                }
            },
        ));
        self.slash.list = Some(self.slash_choices(filtered(Level::Root, query), false, window, cx));
        cx.notify();
    }
    fn select_slash(
        &mut self,
        row: usize,
        flyout: bool,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.slash_current(generation, cx) || !self.slash_has_focus(window, cx) {
            return;
        }
        let list = if flyout {
            self.slash.flyout.clone()
        } else {
            self.slash.list.clone()
        };
        let Some(list) = list else {
            return;
        };
        let category = list
            .read(cx)
            .delegate()
            .entries
            .get(row)
            .is_some_and(|entry| matches!(entry.operation, Operation::Category));
        list.update(cx, |list, cx| {
            list.set_selected_index(Some(IndexPath::new(row)), window, cx);
            list.scroll_to_selected_item(window, cx);
        });
        self.slash.flyout_active = flyout;
        if !flyout {
            if category {
                if self.slash.flyout.is_none() {
                    self.slash.flyout =
                        Some(self.slash_choices(filtered(Level::Markdown, ""), true, window, cx));
                }
            } else {
                self.slash.flyout = None;
            }
        }
        cx.notify();
    }
    pub(super) fn move_slash(
        &mut self,
        down: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.slash.lease.is_none() {
            return false;
        }
        if !self.slash_current(self.slash.generation, cx) {
            self.dismiss_slash(cx);
            return false;
        }
        if self.slash.picker.is_some() {
            return self.slash_grid(0, if down { 1 } else { -1 }, window, cx);
        }
        let flyout = self.slash.flyout_active;
        if let Some(list) = if flyout {
            &self.slash.flyout
        } else {
            &self.slash.list
        } {
            let count = list.read(cx).delegate().entries.len();
            if count > 0 {
                let current = list.read(cx).selected_index().map_or(0, |ix| ix.row);
                let row = if down {
                    (current + 1) % count
                } else {
                    (current + count - 1) % count
                };
                self.select_slash(row, flyout, self.slash.generation, window, cx);
            }
        }
        true
    }
    pub(super) fn horizontal_slash(
        &mut self,
        right: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.slash_current(self.slash.generation, cx) {
            return false;
        }
        if self.slash.picker.is_some() {
            return self.slash_grid(if right { 1 } else { -1 }, 0, window, cx);
        }
        if right {
            if self.slash.flyout.is_some() {
                self.slash.flyout_active = true;
                cx.notify();
                return true;
            }
        } else if self.slash.flyout_active {
            self.slash.flyout_active = false;
            self.slash.flyout = None;
            cx.notify();
            return true;
        }
        false
    }
    fn slash_grid(&mut self, x: i32, y: i32, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.focus.is_focused(window)
            && !self
                .slash
                .picker
                .as_ref()
                .is_some_and(|picker| picker.focus.is_focused(window))
        {
            return false;
        }
        let Some(picker) = &mut self.slash.picker else {
            return false;
        };
        picker.rows = (picker.rows as i32 + y).clamp(1, 8) as usize;
        picker.columns = (picker.columns as i32 + x).clamp(1, 8) as usize;
        cx.notify();
        true
    }
    pub(super) fn accept_slash(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.slash.lease.is_none() {
            return false;
        }
        if !self.slash_current(self.slash.generation, cx) {
            self.dismiss_slash(cx);
            return false;
        }
        if !self.slash_has_focus(window, cx) {
            self.dismiss_slash(cx);
            return false;
        }
        if let Some(picker) = &self.slash.picker {
            let custom = picker
                .custom_rows
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
                || picker
                    .custom_columns
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window);
            if !custom && !self.focus.is_focused(window) && !picker.focus.is_focused(window) {
                return false;
            }
            self.insert_slash_table(custom, window, cx);
            return true;
        }
        let list = if self.slash.flyout_active {
            &self.slash.flyout
        } else {
            &self.slash.list
        };
        let entry = list.as_ref().and_then(|list| {
            list.read(cx)
                .selected_index()
                .and_then(|ix| list.read(cx).delegate().entries.get(ix.row))
                .cloned()
        });
        let Some(entry) = entry else {
            return true;
        };
        if self.active_table().is_some() && !entry.allowed_in_cell() {
            return true;
        }
        match entry.operation {
            Operation::Category => {
                if self.slash.flyout.is_none() {
                    self.slash.flyout =
                        Some(self.slash_choices(filtered(Level::Markdown, ""), true, window, cx));
                }
                self.slash.flyout_active = true;
                cx.notify();
            }
            Operation::Table => {
                let custom_rows = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(t(L10nKey::MarkdownSlashRows))
                        .default_value("2")
                });
                let custom_columns = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(t(L10nKey::MarkdownSlashColumns))
                        .default_value("2")
                });
                let focus = cx.focus_handle().tab_stop(true);
                self.slash.picker = Some(Picker {
                    rows: 1,
                    columns: 1,
                    style: 0,
                    focus: focus.clone(),
                    custom_rows,
                    custom_columns,
                    error: false,
                });
                focus.focus(window, cx);
                cx.notify();
            }
            Operation::Insert(text, caret) => {
                self.commit_slash(text.into(), caret, false, window, cx)
            }
            Operation::Date | Operation::Time => {
                let text = date_time(matches!(entry.operation, Operation::Time));
                let caret = text.len();
                self.commit_slash(text, caret, false, window, cx);
            }
            Operation::Property => self.commit_slash("key:: ".into(), 0, true, window, cx),
        }
        true
    }
    fn commit_slash(
        &mut self,
        text: String,
        caret: usize,
        property: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.slash_current(self.slash.generation, cx) || !self.slash_has_focus(window, cx) {
            self.dismiss_slash(cx);
            return;
        }
        let range = self.slash.lease.as_ref().unwrap().range.clone();
        let selection = Selection::caret(range.start + caret);
        let formula = text == "$$\n\n$$";
        self.dismiss_slash(cx);
        self.command(
            Command::BatchReplaceSelected {
                kind: EditKind::Command,
                edits: vec![(range, text)],
                selection,
            },
            window,
            cx,
        );
        self.focus.focus(window, cx);
        if formula {
            self.begin_formula(selection.head(), false, window, cx);
        }
        if property {
            let source = self.document.read(cx).source().clone();
            let grammar = self.document.read(cx).grammar();
            let token = self.input_token(cx);
            cx.spawn_in(window, async move |view, cx| {
                let index = cx
                    .background_executor()
                    .spawn(async move {
                        let source = source.to_string();
                        super::super::parser::parse(&source, grammar, true)
                            .ok()
                            .map(|root| properties::Index::from_ast(&source, &root))
                    })
                    .await;
                let _ = view.update_in(cx, |view, window, cx| {
                    if view.input_token(cx) == token
                        && view.focus.is_focused(window)
                        && view.document.read(cx).grammar() == grammar
                        && view.live
                        && let Some(index) = index
                    {
                        view.projection.properties = index;
                        view.enter_document_property(window, cx);
                    }
                });
            })
            .detach();
        }
    }
    fn insert_slash_table(&mut self, custom: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &self.slash.picker else {
            return;
        };
        let (rows, columns) = if custom {
            let rows = picker
                .custom_rows
                .read(cx)
                .value()
                .trim()
                .parse::<usize>()
                .ok();
            let columns = picker
                .custom_columns
                .read(cx)
                .value()
                .trim()
                .parse::<usize>()
                .ok();
            match (rows, columns) {
                (Some(rows), Some(columns)) if rows > 0 && columns > 0 => {
                    (rows.min(100), columns.min(100))
                }
                _ => {
                    self.slash.picker.as_mut().unwrap().error = true;
                    cx.notify();
                    return;
                }
            }
        } else {
            (picker.rows, picker.columns)
        };
        let mut text = String::new();
        if picker.style > 0 {
            text.push_str(
                [
                    "",
                    "<!-- table:striped -->\n",
                    "<!-- table:header -->\n",
                    "<!-- table:minimal -->\n",
                ][picker.style],
            );
        }
        let caret = text.len() + 2;
        text.push('|');
        for _ in 0..columns {
            text.push_str("  |");
        }
        text.push_str("\n|");
        for _ in 0..columns {
            text.push_str(" --- |");
        }
        text.push('\n');
        for _ in 1..rows {
            text.push('|');
            for _ in 0..columns {
                text.push_str("  |");
            }
            text.push('\n');
        }
        self.commit_slash(text, caret, false, window, cx);
    }
    pub(super) fn render_slash(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        if !self.slash_current(self.slash.generation, cx) || !self.slash_has_focus(window, cx) {
            return Empty.into_any_element();
        }
        let mut surface = div()
            .id("markdown-slash-menu")
            .test_support()
            .w_80()
            .max_h_96()
            .bg(cx.theme().popover)
            .text_color(cx.theme().popover_foreground)
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius)
            .shadow_lg()
            .p_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        if let Some(picker) = &self.slash.picker {
            surface = surface.child(self.render_slash_picker(picker, window, cx));
        } else if let Some(list) = &self.slash.list {
            if list.read(cx).delegate().entries.is_empty() {
                surface = surface.child(
                    div()
                        .id("markdown-slash-empty")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(t(L10nKey::MarkdownSlashEmpty))
                        .p_2()
                        .text_sm()
                        .child(t(L10nKey::MarkdownSlashEmpty)),
                );
            } else if let Some(flyout) = &self.slash.flyout {
                surface = surface
                    .w(rems(40.))
                    .max_w(window.viewport_size().width - window.rem_size())
                    .child(
                        gpui_kit::component::h_flex()
                            .items_start()
                            .gap_1()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(List::new(list).small().max_h_64()),
                            )
                            .child(
                                div()
                                    .id("markdown-slash-flyout")
                                    .test_support()
                                    .flex_1()
                                    .min_w_0()
                                    .border_l_1()
                                    .border_color(cx.theme().border)
                                    .child(List::new(flyout).small().max_h_64()),
                            ),
                    );
            } else {
                surface = surface.child(List::new(list).small().max_h_64());
            }
        }
        let position = self
            .caret_bounds(self.state.selection.head(), cx)
            .map(|bounds| bounds.bottom_left())
            .or_else(|| self.viewport.bounds.map(|bounds| bounds.origin))
            .unwrap_or_default();
        deferred(
            anchored()
                .position(position)
                .snap_to_window_with_margin(window.rem_size() * 0.5)
                .child(surface),
        )
        .into_any_element()
    }
    fn render_slash_picker(
        &self,
        picker: &Picker,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let generation = self.slash.generation;
        let mut grid = div()
            .id("markdown-slash-table-picker")
            .test_support()
            .role(Role::Group)
            .aria_label(t(L10nKey::EditorTableInsert))
            .key_context(if picker.focus.is_focused(window) {
                "MarkdownSlashPicker MarkdownSlashGrid"
            } else {
                "MarkdownSlashPicker"
            })
            .track_focus(&picker.focus)
            .border_1()
            .border_color(cx.theme().border)
            .focus_visible(|style| style.border_color(cx.theme().primary))
            .max_h_96()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .child(div().text_sm().child(t(L10nKey::EditorTableInsert)))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(L10nKey::MarkdownSlashTableHelp)),
            );
        for row in 1..=8 {
            let mut line = gpui_kit::component::h_flex().gap_1();
            for column in 1..=8 {
                let selected = row <= picker.rows && column <= picker.columns;
                line = line.child(
                    Button::new(SharedString::from(format!(
                        "markdown-slash-cell-{row}-{column}"
                    )))
                    .small()
                    .ghost()
                    .w_7()
                    .h_7()
                    .p_0()
                    .tab_stop(false)
                    .label(format!("{row}×{column}"))
                    .when(selected, |button| {
                        button.bg(cx.theme().primary.opacity(0.2))
                    })
                    .on_mouse_move(cx.listener(move |view, _, window, cx| {
                        if !view.slash_current(generation, cx) || !view.slash_has_focus(window, cx)
                        {
                            return;
                        }
                        if let Some(picker) = &mut view.slash.picker {
                            if picker.rows != row || picker.columns != column {
                                picker.rows = row;
                                picker.columns = column;
                                cx.notify();
                            }
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, window, cx| {
                            cx.stop_propagation();
                            if !view.slash_current(generation, cx)
                                || !view.slash_has_focus(window, cx)
                            {
                                return;
                            }
                            if let Some(picker) = &mut view.slash.picker {
                                picker.rows = row;
                                picker.columns = column;
                            }
                            view.insert_slash_table(false, window, cx);
                        }),
                    )
                    .on_click(cx.listener(move |view, _, window, cx| {
                        if !view.slash_current(generation, cx) || !view.slash_has_focus(window, cx)
                        {
                            return;
                        }
                        if let Some(picker) = &mut view.slash.picker {
                            picker.rows = row;
                            picker.columns = column;
                        }
                        view.insert_slash_table(false, window, cx);
                    })),
                );
            }
            grid = grid.child(line);
        }
        for (style, key) in [
            L10nKey::EditorTableStyleGrid,
            L10nKey::EditorTableStyleStriped,
            L10nKey::EditorTableStyleHeader,
            L10nKey::EditorTableStyleMinimal,
        ]
        .into_iter()
        .enumerate()
        {
            grid = grid.child(
                Button::new(("markdown-slash-table-style", style))
                    .small()
                    .ghost()
                    .label(t(key))
                    .when(style == picker.style, |button| {
                        button.bg(cx.theme().primary.opacity(0.2))
                    })
                    .on_click(cx.listener(move |view, _, window, cx| {
                        if !view.slash_current(generation, cx) || !view.slash_has_focus(window, cx)
                        {
                            return;
                        }
                        if let Some(picker) = &mut view.slash.picker {
                            picker.style = style;
                            picker.focus.focus(window, cx);
                        }
                        cx.notify();
                    })),
            );
        }
        grid = grid
            .child(
                gpui_kit::component::h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_xs().child(t(L10nKey::MarkdownSlashRows)))
                            .child(
                                Input::new(&picker.custom_rows)
                                    .id("markdown-slash-table-rows")
                                    .small()
                                    .aria_label(t(L10nKey::MarkdownSlashRows)),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_xs().child(t(L10nKey::MarkdownSlashColumns)))
                            .child(
                                Input::new(&picker.custom_columns)
                                    .id("markdown-slash-table-columns")
                                    .small()
                                    .aria_label(t(L10nKey::MarkdownSlashColumns)),
                            ),
                    ),
            )
            .child(
                Button::new("markdown-slash-table-custom")
                    .small()
                    .label(t(L10nKey::MarkdownSlashCustomTable))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        if view.slash_current(generation, cx) && view.slash_has_focus(window, cx) {
                            view.insert_slash_table(true, window, cx);
                        }
                    })),
            )
            .when(picker.error, |grid| {
                grid.child(
                    div()
                        .id("markdown-slash-table-error")
                        .role(Role::Status)
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(t(L10nKey::MarkdownSlashTableInvalid)),
                )
            });
        grid.into_any_element()
    }
}
