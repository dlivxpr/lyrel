//! Lyrel-owned table insertion and keyboard-accessible table/code command surfaces.
//! Rendering/editing stays in the MIT-derived editor; no GPL application UI is imported.
use super::super::*;
use gpui_kit::component::{
    Rope, RopeExt as _, Selectable as _,
    highlighter::{HighlightTheme, LanguageRegistry, SyntaxHighlighter},
    menu::DropdownMenu as _,
    popover::Popover,
};
use gpui_kit::{App, FocusHandle, HighlightStyle, IntoElement, Render, RenderOnce, WeakEntity};
use lyrel_markdown_editor::EditorState as MarkdownEditor;
use std::{cell::RefCell, ops::Range};

pub(super) fn configure(editor: &mut MarkdownEditor, _: &mut Context<MarkdownEditor>) {
    let mut languages = LanguageRegistry::singleton().languages();
    languages.sort();
    languages.retain(|language| language.as_ref() != "text");
    languages.insert(0, "text".into());
    editor.set_code_languages(languages);
    struct Cached {
        source: String,
        parser: SyntaxHighlighter,
        styles: Vec<(Range<usize>, HighlightStyle)>,
        theme: Option<Arc<HighlightTheme>>,
    }
    let cache = RefCell::new(HashMap::<String, Cached>::new());
    editor.set_code_highlighter(move |language, source, cx| {
        let theme = cx.theme().highlight_theme.clone();
        let Some(grammar) = LanguageRegistry::singleton().language(language) else {
            return Vec::new();
        };
        let mut cache = cache.borrow_mut();
        let cached = cache
            .entry(grammar.name.to_string())
            .or_insert_with(|| Cached {
                source: String::new(),
                parser: SyntaxHighlighter::new(language),
                styles: Vec::new(),
                theme: None,
            });
        if cached.source != source {
            let rope = Rope::from_str(source);
            let old_end_byte = cached.parser.text().len();
            let old_end = cached.parser.text().offset_to_point(old_end_byte);
            let new_end = rope.offset_to_point(source.len());
            let edit = tree_sitter::InputEdit {
                start_byte: 0,
                old_end_byte,
                new_end_byte: source.len(),
                start_position: tree_sitter::Point::new(0, 0),
                old_end_position: tree_sitter::Point::new(old_end.row, old_end.column),
                new_end_position: tree_sitter::Point::new(new_end.row, new_end.column),
            };
            cached.parser.update(Some(edit), &rope, None);
            cached.source.clear();
            cached.source.push_str(source);
            cached.theme = None;
        }
        if cached
            .theme
            .as_ref()
            .is_none_or(|old| !Arc::ptr_eq(old, &theme))
        {
            cached.styles = cached.parser.styles(&(0..source.len()), theme.as_ref());
            cached.theme = Some(theme);
        }
        cached.styles.clone()
    });
}

#[derive(IntoElement)]
struct Controls {
    editor: Entity<MarkdownEditor>,
}
impl RenderOnce for Controls {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let id: SharedString = format!("markdown-table-code-{:?}", self.editor.entity_id()).into();
        let view = window.use_keyed_state(id.clone(), cx, |window, cx| {
            cx.new(|cx| TableCodeControls::new(self.editor, window, cx))
        });
        div().id(id).child(view.read(cx).clone())
    }
}

pub(crate) fn render_controls(editor: &Entity<MarkdownEditor>) -> AnyElement {
    Controls {
        editor: editor.clone(),
    }
    .into_any_element()
}

pub(crate) fn open_table_picker(editor: &Entity<MarkdownEditor>, _: &mut Window, cx: &mut App) {
    editor.update(cx, |_, cx| {
        cx.emit(lyrel_markdown_editor::EditorEvent::InsertTable)
    });
}

struct TableCodeControls {
    editor: WeakEntity<MarkdownEditor>,
    picker: Entity<TablePicker>,
    open: bool,
    _subscriptions: Vec<Subscription>,
}
impl TableCodeControls {
    fn new(editor: Entity<MarkdownEditor>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let picker = cx.new(|cx| TablePicker::new(editor.downgrade(), window, cx));
        let document = editor.read(cx).document().clone();
        let subscriptions = vec![
            cx.observe(&editor, |_, _, cx| cx.notify()),
            cx.observe(&document, |_, _, cx| cx.notify()),
            cx.subscribe(&picker, |this, _, _: &gpui_kit::DismissEvent, cx| {
                this.open = false;
                cx.notify();
            }),
            cx.subscribe_in(&editor, window, |this, _, event, window, cx| {
                if matches!(event, lyrel_markdown_editor::EditorEvent::InsertTable) {
                    this.open = true;
                    this.picker.update(cx, |picker, cx| picker.reset(cx));
                    let picker = this.picker.clone();
                    let focus = picker.read(cx).focus.clone();
                    focus.focus(window, cx);
                    window.on_next_frame(move |window, cx| {
                        let focus = picker.read(cx).focus.clone();
                        focus.focus(window, cx);
                    });
                    cx.notify();
                }
            }),
        ];
        Self {
            editor: editor.downgrade(),
            picker,
            open: false,
            _subscriptions: subscriptions,
        }
    }
}

fn command_item(
    editor: &Entity<MarkdownEditor>,
    label: L10nKey,
    command: impl Fn(&mut MarkdownEditor, &mut Window, &mut Context<MarkdownEditor>) + 'static,
) -> PopupMenuItem {
    let editor = editor.downgrade();
    PopupMenuItem::new(t(label)).on_click(move |_, window, cx| {
        if let Some(editor) = editor.upgrade() {
            editor.update(cx, |editor, cx| {
                command(editor, window, cx);
                editor.focus_handle(cx).focus(window, cx);
            });
        }
    })
}

impl Render for TableCodeControls {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(editor) = self.editor.upgrade() else {
            return h_flex();
        };
        if editor.read(cx).is_source_only() {
            return h_flex();
        }
        let code = editor.read(cx).caret_code_language();
        let code_editor = editor.clone();
        let copy_editor = editor.clone();
        let menu_editor = editor.clone();
        h_flex()
            .flex_none()
            .justify_end()
            .gap_1()
            .px_2()
            .when(self.open, |row| {
                row.child(
                    Popover::new("markdown-table-insertion-picker")
                        .open(self.open)
                        .track_focus(&self.picker.read(cx).focus)
                        .on_open_change(cx.listener(move |this, open, window, cx| {
                            this.open = *open;
                            if *open {
                                this.picker.update(cx, |picker, cx| {
                                    picker.reset(cx);
                                    picker.focus.focus(window, cx);
                                });
                            } else {
                                menu_editor.read(cx).focus_handle(cx).focus(window, cx);
                            }
                            cx.notify();
                        }))
                        // Command-only picker: a zero-size selectable anchor preserves
                        // Popover positioning/focus without another visible toolbar button.
                        .trigger(Button::new("markdown-table-picker-anchor").size_0())
                        .child(self.picker.clone()),
                )
            })
            .when_some(code, |row, language| {
                row.child(
                    Button::new("markdown-code-language")
                        .ghost()
                        .xsmall()
                        .label(t(L10nKey::MarkdownCodeLanguage))
                        .dropdown_menu(move |mut menu, _, cx| {
                            menu = menu
                                .action_context(code_editor.read(cx).focus_handle(cx))
                                .scrollable(true);
                            let mut languages = LanguageRegistry::singleton().languages();
                            languages.sort();
                            for value in languages {
                                let checked = language == value.as_ref()
                                    || (language.is_empty() && value.as_ref() == "text");
                                let editor = code_editor.downgrade();
                                menu = menu.item(
                                    PopupMenuItem::new(value.clone()).checked(checked).on_click(
                                        move |_, window, cx| {
                                            if let Some(editor) = editor.upgrade() {
                                                editor.update(cx, |editor, cx| {
                                                    editor.set_caret_code_language(&value, cx);
                                                    editor.focus_handle(cx).focus(window, cx);
                                                });
                                            }
                                        },
                                    ),
                                );
                            }
                            menu.separator().item(command_item(
                                &code_editor,
                                L10nKey::MarkdownCodeSource,
                                |editor, _, cx| editor.reveal_caret_code_fence(cx),
                            ))
                        }),
                )
                .child(
                    Button::new("markdown-code-copy")
                        .ghost()
                        .xsmall()
                        .label(t(L10nKey::MarkdownCodeCopy))
                        .on_click(move |_, _, cx| {
                            copy_editor.update(cx, |editor, cx| editor.copy_caret_code(cx))
                        }),
                )
            })
    }
}

struct TablePicker {
    editor: WeakEntity<MarkdownEditor>,
    focus: FocusHandle,
    rows: Entity<InputState>,
    columns: Entity<InputState>,
    selected: (usize, usize),
    style: Option<&'static str>,
    revision: u64,
    selection: Range<usize>,
    _subscriptions: Vec<Subscription>,
}
impl gpui_kit::EventEmitter<gpui_kit::DismissEvent> for TablePicker {}
impl TablePicker {
    fn new(
        editor: WeakEntity<MarkdownEditor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let rows = cx.new(|cx| InputState::new(window, cx).default_value("3"));
        let columns = cx.new(|cx| InputState::new(window, cx).default_value("3"));
        let subscriptions = vec![
            cx.observe(&rows, |_, _, cx| cx.notify()),
            cx.observe(&columns, |_, _, cx| cx.notify()),
        ];
        Self {
            editor,
            focus: cx.focus_handle(),
            rows,
            columns,
            selected: (3, 3),
            style: None,
            revision: 0,
            selection: 0..0,
            _subscriptions: subscriptions,
        }
    }
    fn reset(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = self.editor.upgrade() {
            self.revision = editor.read(cx).document().read(cx).revision();
            self.selection = editor.read(cx).selected_range();
        }
        cx.notify();
    }
    fn dimensions(&self, cx: &App) -> Option<(usize, usize)> {
        let rows = self.rows.read(cx).value().parse::<usize>().ok()?;
        let columns = self.columns.read(cx).value().parse::<usize>().ok()?;
        ((1..=100).contains(&rows) && (1..=100).contains(&columns)).then_some((rows, columns))
    }
    fn insert(&mut self, rows: usize, columns: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.editor.upgrade() {
            if editor.read(cx).document().read(cx).revision() != self.revision {
                return;
            }
            let style = self.style;
            editor.update(cx, |editor, cx| {
                editor.select_range(self.selection.clone(), cx);
                editor.insert_table_with_style(rows, columns, style, cx);
                editor.focus_handle(cx).focus(window, cx);
            });
            cx.emit(gpui_kit::DismissEvent);
        }
    }
}
impl Render for TablePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dimensions = self.dimensions(cx);
        let stale = self
            .editor
            .upgrade()
            .is_none_or(|editor| editor.read(cx).document().read(cx).revision() != self.revision);
        let selected = self.selected;
        let focus = self.focus.clone();
        let mut grid = v_flex().gap_1();
        for row in 1..=8 {
            let mut line = h_flex().gap_1();
            for column in 1..=8 {
                line =
                    line.child(
                        Button::new(SharedString::from(format!(
                            "markdown-table-size-{row}-{column}"
                        )))
                        .ghost()
                        .xsmall()
                        .label(format!("{row}×{column}"))
                        .selected(row <= selected.0 && column <= selected.1)
                        .disabled(stale)
                        .on_click(cx.listener(
                            move |this, _, window, cx| this.insert(row, column, window, cx),
                        )),
                    );
            }
            grid = grid.child(line);
        }
        v_flex()
            .id("markdown-table-picker")
            .test_support()
            .track_focus(&focus)
            .gap_2()
            .p_2()
            .w_96()
            .on_key_down(
                cx.listener(move |this, event: &gpui_kit::KeyDownEvent, window, cx| {
                    if !this.focus.is_focused(window) {
                        return;
                    }
                    match event.keystroke.key.as_str() {
                        "left" => this.selected.1 = this.selected.1.saturating_sub(1).max(1),
                        "right" => this.selected.1 = (this.selected.1 + 1).min(8),
                        "up" => this.selected.0 = this.selected.0.saturating_sub(1).max(1),
                        "down" => this.selected.0 = (this.selected.0 + 1).min(8),
                        "enter" => {
                            this.insert(this.selected.0, this.selected.1, window, cx);
                        }
                        "escape" => cx.emit(gpui_kit::DismissEvent),
                        _ => return,
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(t(L10nKey::MarkdownSlashTableHelp))
            .child(grid)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        v_flex()
                            .flex_1()
                            .child(t(L10nKey::MarkdownSlashRows))
                            .child(Input::new(&self.rows).id("markdown-table-rows").small()),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .child(t(L10nKey::MarkdownSlashColumns))
                            .child(
                                Input::new(&self.columns)
                                    .id("markdown-table-columns")
                                    .small(),
                            ),
                    ),
            )
            .child(
                h_flex().gap_1().flex_wrap().children(
                    [
                        (L10nKey::EditorTableStyleGrid, None),
                        (L10nKey::EditorTableStyleStriped, Some("striped")),
                        (L10nKey::EditorTableStyleHeader, Some("header")),
                        (L10nKey::EditorTableStyleMinimal, Some("minimal")),
                    ]
                    .into_iter()
                    .map(|(label, style)| {
                        Button::new(SharedString::from(format!(
                            "markdown-table-style-{}",
                            style.unwrap_or("grid")
                        )))
                        .ghost()
                        .xsmall()
                        .label(t(label))
                        .selected(self.style == style)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.style = style;
                            cx.notify();
                        }))
                    }),
                ),
            )
            .when(dimensions.is_none(), |view| {
                view.child(
                    div()
                        .text_color(cx.theme().danger)
                        .child(t(L10nKey::MarkdownSlashTableInvalid)),
                )
            })
            .when(stale, |view| {
                view.child(
                    div()
                        .text_color(cx.theme().warning)
                        .child(t(L10nKey::EditorTableChanged)),
                )
            })
            .child(
                Button::new("markdown-table-insert-custom")
                    .small()
                    .label(t(L10nKey::MarkdownSlashCustomTable))
                    .disabled(dimensions.is_none() || stale)
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some((rows, columns)) = this.dimensions(cx) {
                            this.insert(rows, columns, window, cx);
                        }
                    })),
            )
    }
}
