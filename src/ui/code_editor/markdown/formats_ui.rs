use super::{
    Bold, ClearHighlightColor, CustomHighlightColor, Format, Highlight, InlineCode, Italic,
    Selection, SourcePresentation, Strike, Underline, decorations, eligible, state, trim,
};
use crate::ui::i18n::{L10nKey, t};
use gpui_kit::{Action, App, AppContext as _, Context, Window};

#[derive(Default)]
pub(in super::super) struct State {
    custom: Option<CustomColor>,
    feedback: Option<Feedback>,
    commands: std::collections::VecDeque<(Format, Option<u32>)>,
    lease: Option<CommandLease>,
    generation: u64,
    task: Option<gpui_kit::Task<()>>,
    blur: Option<gpui_kit::Subscription>,
}
// Immutable positioned parse cache, invalidated from the owning document. It
// cannot accept text input, save, or retain independent undo history.
pub(in crate::ui::code_editor::markdown) struct Syntax {
    revision: u64,
    grammar: super::super::super::mdx::Grammar,
    source: String,
    root: markdown::mdast::Node,
    decorations: Vec<super::Decoration>,
    coverage: Vec<(Format, std::ops::Range<usize>)>,
}
impl Syntax {
    pub(in crate::ui::code_editor::markdown) fn source(&self) -> &str {
        &self.source
    }
    pub(in crate::ui::code_editor::markdown) fn root(&self) -> &markdown::mdast::Node {
        &self.root
    }
    pub(in crate::ui::code_editor::markdown) fn new(
        source: &str,
        root: markdown::mdast::Node,
        revision: u64,
        grammar: super::super::super::mdx::Grammar,
    ) -> Self {
        let decorations = decorations(source, &root);
        Self::from_parts(source, root, revision, grammar, decorations)
    }
    pub(in crate::ui::code_editor::markdown) fn from_parts(
        source: &str,
        root: markdown::mdast::Node,
        revision: u64,
        grammar: super::super::super::mdx::Grammar,
        decorations: Vec<super::Decoration>,
    ) -> Self {
        let coverage = super::coverage(&root, &decorations);
        Self {
            revision,
            grammar,
            source: source.into(),
            root,
            decorations,
            coverage,
        }
    }
}
struct Feedback {
    revision: u64,
    selection: Selection,
    enabled: [bool; 7],
    states: [u8; 7],
    color: Option<u32>,
}
struct CustomColor {
    format: Format,
    selection: Selection,
    revision: u64,
    picker: gpui_kit::Entity<gpui_kit::component::color_picker::ColorPickerState>,
}
#[derive(PartialEq, Eq)]
struct CommandLease {
    token: (gpui_kit::EntityId, u64, u64, u64, Option<gpui_kit::EntityId>),
    selection: Selection,
    grammar: super::super::super::mdx::Grammar,
    live: bool,
}
#[derive(Clone, Debug, PartialEq, gpui_kit::Action)]
#[action(namespace = markdown_formats, no_json)]
pub(in super::super) struct SetHighlightColor {
    pub color: u32,
}

const FORMATS: [Format; 7] = [
    Format::Bold,
    Format::Italic,
    Format::Underline,
    Format::Strike,
    Format::Code,
    Format::Highlight,
    Format::HighlightColor,
];
// Document color data from the pinned reference, not UI theme colors.
const HIGHLIGHT_COLORS: [(L10nKey, u32); 7] = [
    (L10nKey::MarkdownColorYellow, 0xffd54f80),
    (L10nKey::MarkdownColorOrange, 0xffab4080),
    (L10nKey::MarkdownColorGreen, 0xa5d6a780),
    (L10nKey::MarkdownColorBlue, 0x90caf980),
    (L10nKey::MarkdownColorPurple, 0xce93d880),
    (L10nKey::MarkdownColorPink, 0xf48fb180),
    (L10nKey::MarkdownColorGray, 0xbdbdbd80),
];

impl SourcePresentation {
    fn format_command_lease(&self, cx: &App) -> CommandLease {
        CommandLease {
            token: self.input_token(cx),
            selection: self.state.selection,
            grammar: self.document.read(cx).grammar(),
            live: self.live,
        }
    }
    fn finish_format_commands(&mut self) {
        self.formats.commands.clear();
        self.formats.lease = None;
        self.formats.task = None;
        self.formats.blur = None;
    }
    pub(super) fn dispatch_format_command(
        &mut self,
        format: Format,
        color: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_composing() || !self.extra_selections.is_empty() {
            return;
        }
        self.focus.focus(window, cx);
        if self.state.selection.range().is_empty() {
            return;
        }
        if self.formats.task.is_none() {
            if let Some(command) = self.plan_format(format, color, cx) {
                self.command(command, window, cx);
                return;
            }
            if self.format_syntax(cx).is_some() || self.document.read(cx).source().len() <= 32 * 1024 {
                return;
            }
        }
        let lease = self.format_command_lease(cx);
        if self.formats.lease.as_ref() != Some(&lease) {
            self.formats.commands.clear();
            self.formats.generation = self.formats.generation.wrapping_add(1);
            self.formats.lease = Some(lease);
        }
        self.formats.commands.push_back((format, color));
        if self.formats.task.is_some() {
            return;
        }
        // One worker retains command order. Only its own transactions advance
        // the lease; user edits, another presentation or a focus change cancel it.
        self.formats.blur = Some(cx.on_blur(&self.focus, window, |view, _, _| {
            view.formats.commands.clear();
            view.formats.lease = None;
            view.formats.generation = view.formats.generation.wrapping_add(1);
        }));
        self.formats.task = Some(cx.spawn_in(window, async move |view, cx| {
            loop {
                let request = view.update_in(cx, |view, window, cx| {
                    let current = view.format_command_lease(cx);
                    if view.formats.lease.as_ref() != Some(&current)
                        || !view.focus.is_focused(window)
                        || view.is_composing()
                        || !view.extra_selections.is_empty()
                    {
                        view.finish_format_commands();
                        return None;
                    }
                    let Some((format, color)) = view.formats.commands.pop_front() else {
                        view.finish_format_commands();
                        return None;
                    };
                    Some((view.formats.generation, current.selection,
                        view.document.read(cx).source().clone(), current.grammar, format, color))
                });
                let Ok(Some((generation, selection, source, grammar, format, color))) = request else {
                    break;
                };
                let command = cx.background_executor().spawn(async move {
                    let source = source.to_string();
                    let root = super::super::super::parser::parse(&source, grammar, true).ok()?;
                    let styles = decorations(&source, &root);
                    super::edit(&source, &root, selection, format, color, &styles)
                }).await;
                let next = view.update_in(cx, |view, window, cx| {
                    if view.formats.generation != generation {
                        return true;
                    }
                    if view.formats.lease.as_ref() != Some(&view.format_command_lease(cx))
                        || !view.focus.is_focused(window)
                        || view.is_composing()
                        || !view.extra_selections.is_empty()
                    {
                        view.finish_format_commands();
                        return false;
                    }
                    if let Some(command) = command {
                        view.command(command, window, cx);
                    }
                    view.formats.lease = Some(view.format_command_lease(cx));
                    if view.formats.commands.is_empty() {
                        view.finish_format_commands();
                        false
                    } else {
                        true
                    }
                });
                if !matches!(next, Ok(true)) {
                    break;
                }
            }
        }));
    }
    pub(in crate::ui::code_editor::markdown) fn format_syntax(&self, cx: &App) -> Option<&Syntax> {
        let doc = self.document.read(cx);
        self.projection
            .syntax
            .as_deref()
            .filter(|syntax| syntax.revision == doc.revision() && syntax.grammar == doc.grammar())
    }

    pub(in super::super) fn plan_format(
        &mut self,
        format: Format,
        color: Option<u32>,
        cx: &App,
    ) -> Option<super::Command> {
        if self.state.selection.range().start == self.state.selection.range().end {
            return None;
        }
        let bounded;
        let syntax = if let Some(syntax) = self.format_syntax(cx) {
            syntax
        } else {
            let doc = self.document.read(cx);
            // Explicit commands may inspect a bounded document; render never parses.
            if doc.source().len() > 32 * 1024 {
                return None;
            }
            let source = doc.source().to_string();
            let root = super::super::super::parser::parse(&source, doc.grammar(), true).ok()?;
            bounded = Syntax::new(&source, root, doc.revision(), doc.grammar());
            &bounded
        };
        super::edit(
            &syntax.source,
            &syntax.root,
            self.state.selection,
            format,
            color,
            &syntax.decorations,
        )
    }

    fn refresh_format_feedback(&mut self, cx: &App) {
        let selection = self.state.selection;
        if self.is_composing() || !self.extra_selections.is_empty() {
            self.formats.feedback = None;
            return;
        }
        if selection.range().start == selection.range().end {
            self.formats.feedback = Some(Feedback {
                revision: self.document.read(cx).revision(),
                selection,
                enabled: [false; 7],
                states: [0; 7],
                color: None,
            });
            return;
        }
        let Some(syntax) = self.format_syntax(cx) else {
            self.formats.feedback = None;
            return;
        };
        if self
            .formats
            .feedback
            .as_ref()
            .is_some_and(|f| f.revision == syntax.revision && f.selection == selection)
        {
            return;
        }
        let mut feedback = Feedback {
            revision: syntax.revision,
            selection,
            enabled: [false; 7],
            states: [0; 7],
            color: None,
        };
        {
            let root = &syntax.root;
            let range = trim(&syntax.source, selection.range());
            let prose = eligible(&syntax.source, root, &range, Format::Bold);
            feedback.enabled.fill(prose);
            feedback.enabled[4] = prose || eligible(&syntax.source, root, &range, Format::Code);
            for (ix, format) in FORMATS.iter().copied().enumerate() {
                feedback.states[ix] =
                    state(&syntax.source, selection.range(), format, &syntax.coverage);
            }
            feedback.color = syntax
                .decorations
                .iter()
                .rev()
                .find(|d| {
                    d.format == Format::HighlightColor
                        && d.open.start <= selection.range().start
                        && selection.range().end <= d.close.end
                })
                .and_then(|d| d.color);
        }
        self.formats.feedback = Some(feedback);
    }

    pub(in super::super) fn format_menu(
        &self,
        mut menu: gpui_kit::component::menu::PopupMenu,
    ) -> gpui_kit::component::menu::PopupMenu {
        let actions: [Box<dyn Action>; 7] = [
            Box::new(Bold),
            Box::new(Italic),
            Box::new(Underline),
            Box::new(Strike),
            Box::new(InlineCode),
            Box::new(Highlight),
            Box::new(CustomHighlightColor),
        ];
        menu = menu.separator().label(t(L10nKey::MarkdownFormatMenu));
        for (ix, (format, action)) in FORMATS.into_iter().zip(actions).enumerate() {
            let state = self.formats.feedback.as_ref().map_or(0, |f| f.states[ix]);
            let label = if state == 1 {
                t(L10nKey::MarkdownFormatMixed).replace("{format}", t(format.label()))
            } else {
                t(format.label()).to_string()
            };
            let disabled = self.is_composing()
                || !self.extra_selections.is_empty()
                || self
                    .formats
                    .feedback
                    .as_ref()
                    .is_none_or(|f| !f.enabled[ix]);
            menu = menu.menu_with_check_and_disabled(label, state == 2, action, disabled);
        }
        let disabled = self.is_composing()
            || !self.extra_selections.is_empty()
            || self.formats.feedback.as_ref().is_none_or(|f| !f.enabled[6]);
        for (key, color) in HIGHLIGHT_COLORS {
            let checked = self
                .formats
                .feedback
                .as_ref()
                .is_some_and(|f| f.color == Some(color));
            menu = menu.menu_with_check_and_disabled(
                format!("{} · #{color:08x}", t(key)),
                checked,
                Box::new(SetHighlightColor { color }),
                disabled,
            );
        }
        menu.menu_with_disabled(
            t(L10nKey::MarkdownClearHighlightColor),
            Box::new(ClearHighlightColor),
            disabled,
        )
    }

    pub(in super::super) fn custom_format_color(
        &mut self,
        format: Format,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::Focusable as _;
        use gpui_kit::component::{ActiveTheme as _, color_picker::ColorPickerState};
        self.refresh_format_feedback(cx);
        if self.formats.feedback.as_ref().is_none_or(|f| !f.enabled[6]) || self.is_composing() {
            return;
        }
        let initial = cx.theme().primary.opacity(0.5);
        let picker = cx.new(|cx| ColorPickerState::new(window, cx).default_value(initial));
        picker.read(cx).focus_handle(cx).focus(window, cx);
        self.formats.custom = Some(CustomColor {
            format,
            picker,
            selection: self.state.selection,
            revision: self.document.read(cx).revision(),
        });
        cx.notify();
    }

    pub(in super::super) fn cancel_format_color(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.formats.custom.take().is_none() {
            return false;
        }
        self.focus.focus(window, cx);
        cx.notify();
        true
    }

    pub(in super::super) fn apply_format_color(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(custom) = self.formats.custom.take() else {
            return;
        };
        if custom.revision == self.document.read(cx).revision()
            && custom.selection == self.state.selection
        {
            let color = custom.picker.read(cx).value().map(|color| {
                let rgba: gpui_kit::Rgba = color.into();
                ((rgba.r * 255.).round() as u32) << 24
                    | ((rgba.g * 255.).round() as u32) << 16
                    | ((rgba.b * 255.).round() as u32) << 8
                    | (rgba.a * 255.).round() as u32
            });
            self.format_command(custom.format, color, window, cx);
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(in super::super) fn format_toolbar(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::{
            Sizable as _,
            button::{Button, ButtonVariants as _},
            color_picker::ColorPicker,
            h_flex,
            menu::DropdownMenu as _,
        };
        use gpui_kit::{
            InteractiveElement as _, IntoElement as _, MouseButton, ParentElement as _, Styled as _,
        };
        self.refresh_format_feedback(cx);
        let view = cx.entity().downgrade();
        let focus = self.focus.clone();
        let mut bar = h_flex()
            .flex_none()
            .gap_2()
            .px_3()
            .py_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                Button::new("markdown-format-menu")
                    .small()
                    .ghost()
                    .label(t(L10nKey::MarkdownFormatMenu))
                    .dropdown_menu(move |menu, _, cx| {
                        let menu = menu.action_context(focus.clone());
                        match view.upgrade() {
                            Some(view) => view.read(cx).format_menu(menu),
                            None => menu,
                        }
                    }),
            );
        if let Some(custom) = &self.formats.custom {
            bar = bar
                .child(
                    ColorPicker::new(&custom.picker)
                        .small()
                        .label(t(custom.format.label())),
                )
                .child(
                    Button::new("markdown-format-apply-color")
                        .small()
                        .ghost()
                        .label(t(L10nKey::MarkdownApplyColor))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.apply_format_color(window, cx)),
                        ),
                )
                .child(
                    Button::new("markdown-format-cancel-color")
                        .small()
                        .ghost()
                        .label(t(L10nKey::Cancel))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.cancel_format_color(window, cx);
                        })),
                );
        }
        bar.into_any_element()
    }
}
