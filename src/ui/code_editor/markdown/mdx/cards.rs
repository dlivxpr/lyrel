//! Native, source-backed cards. Reading order remains document order; card
//! navigation is a separate command, never a side effect of placing the caret.
use super::{Field, PropertyKind, view::{self, FieldContext}};
use super::super::{Command, EditKind, Selection, SourcePresentation};
use crate::ui::i18n::{L10nKey, t};
use gpui_kit::*;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, button::{Button, ButtonVariants as _}};
use std::ops::Range;

pub(super) fn matches(name: &str) -> bool {
    matches!(name, "Card" | "CardGroup")
}

pub(super) fn render(
    context: &FieldContext<'_>,
    content: AnyElement,
    _: &mut Window,
    cx: &mut Context<SourcePresentation>,
) -> AnyElement {
    let Some(component) = context.component() else { return content; };
    let group = context.source().slice(component.name_range()).chars().eq("CardGroup".chars());
    let mut surface = div().w_full().min_w_0().px_3()
        .border_l_1().border_r_1().border_color(cx.theme().border);
    match context.field() {
        Some(Field::Name) => {
            let heading = if group { t(L10nKey::EditorCardGroup) } else { t(L10nKey::EditorCard) };
            let title = component.properties().iter().find(|property| {
                property.kind() == PropertyKind::Literal && property.name_range().is_some_and(|range| context.source().slice(range).chars().eq("title".chars()))
            }).map(|property| literal_value(context.source().slice(property.value_range()).to_string()));
            let mut header = div().flex().flex_wrap().items_center().gap_2()
                .child(div().flex_1().min_w_0().font_weight(FontWeight::MEDIUM)
                    .child(title.unwrap_or_else(|| heading.to_string())));
            header = header.child(edit_button(context, "mdx-card-name", L10nKey::EditorMdxName,
                component.name_range(), "Edit card component name", cx));
            if !group {
                if let Some(property) = component.properties().iter().find(|property| {
                    property.kind() == PropertyKind::Literal && property.name_range().is_some_and(|range| context.source().slice(range).chars().eq("href".chars()))
                }) {
                    let target = literal_value(context.source().slice(property.value_range()).to_string());
                    if !target.is_empty() {
                        let revision = context.revision();
                        let grammar = context.grammar();
                        header = header.child(div()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(Button::new("mdx-card-follow").ghost().small()
                                .label(t(L10nKey::EditorCardFollow))
                                .accessibility_label(format!("Open card destination: {target}"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if this.document().read(cx).revision() != revision || this.document().read(cx).grammar() != grammar { return; }
                                    this.command(Command::FinishComposition, window, cx);
                                    this.focus_handle(cx).focus(window, cx);
                                    this.follow_link(target.clone(), cx);
                                }))));
                    }
                }
            }
            // Missing common properties are added locally, without rebuilding
            // the tag or interpreting expressions/spreads as property values.
            for (name, key) in if group {
                &[ ("cols", L10nKey::EditorCardColumns) ][..]
            } else {
                &[ ("title", L10nKey::EditorCardTitle), ("href", L10nKey::EditorCardDestination) ][..]
            } {
                if !component.properties().iter().any(|property| property.name_range().is_some_and(|range| context.source().slice(range).chars().eq(name.chars()))) {
                    let at = component.properties().last().map_or(component.name_range().end, |property| property.source_range().end);
                    let text = format!(" {name}=\"\"");
                    let selected = at + name.len() + 3;
                    let revision = context.revision();
                    let grammar = context.grammar();
                    header = header.child(div().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(Button::new(SharedString::from(format!("mdx-card-add-{name}"))).ghost().small()
                            .label(t(*key)).accessibility_label(format!("Add card property: {name}"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.document().read(cx).revision() != revision || this.document().read(cx).grammar() != grammar { return; }
                                this.command(Command::FinishComposition, window, cx);
                                // This changes tag structure, not the name at
                                // its adjacent insertion boundary. A sparse
                                // batch must not mirror attributes into </name>.
                                this.command(Command::BatchReplace { edits: vec![(at..at, text.clone())], kind: EditKind::Command }, window, cx);
                                this.command(Command::Select(Selection::caret(selected)), window, cx);
                                this.focus_handle(cx).focus(window, cx);
                            }))));
                }
            }
            surface = surface.border_t_1().rounded_tl(cx.theme().radius).rounded_tr(cx.theme().radius).pt_2()
                .child(div().text_xs().text_color(cx.theme().muted_foreground).child(heading))
                .child(header);
            if group {
                surface = surface.child(div().text_xs().text_color(cx.theme().muted_foreground)
                    .child(t(L10nKey::EditorCardGroupOrder)));
            }
            surface.child(content).into_any_element()
        }
        Some(Field::Property(kind)) => {
            let Some(property) = context.property() else { return content; };
            let name = property.name_range().map(|range| context.source().slice(range).to_string());
            let key = match name.as_deref() {
                Some("title") => Some(L10nKey::EditorCardTitle),
                Some("href") => Some(L10nKey::EditorCardDestination),
                Some("icon") => Some(L10nKey::EditorCardIcon),
                Some("description") => Some(L10nKey::EditorCardDescription),
                Some("cols") => Some(L10nKey::EditorCardColumns),
                _ => None,
            };
            // Dynamic values remain visibly source, even for familiar names.
            if matches!(kind, PropertyKind::Expression | PropertyKind::Spread) || key.is_none() {
                return surface.child(view::render_generic(context, content, cx)).into_any_element();
            }
            let name = name.expect("named native property");
            surface.child(edit_button(context, SharedString::from(format!("mdx-card-{name}")),
                key.expect("native property label"), property.value_range(),
                format!("Edit card property: {name}"), cx))
                .child(content).into_any_element()
        }
        Some(Field::Children) => {
            let range = component.children_range();
            let revision = context.revision();
            let grammar = context.grammar();
            let expansion = component.closing_name_range().is_none().then(|| (
                component.source_range().end - 2..component.source_range().end,
                context.source().slice(component.name_range()).to_string(), component.is_flow(),
            ));
            surface.child(div().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(Button::new(if group { "mdx-card-group-children" } else { "mdx-card-children" }).ghost().small()
                    .label(t(if group { L10nKey::EditorCardEditGroup } else { L10nKey::EditorCardEditContent }))
                    .accessibility_label(if group { "Edit card group children" } else { "Edit card content" })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.document().read(cx).revision() != revision || this.document().read(cx).grammar() != grammar { return; }
                        this.command(Command::FinishComposition, window, cx);
                        if let Some((ending, name, block)) = &expansion {
                            this.command(Command::ExpandComponent { ending: ending.clone(), name: name.clone(), block: *block }, window, cx);
                        } else {
                            this.command(Command::Select(Selection::caret(range.start)), window, cx);
                        }
                        this.focus_handle(cx).focus(window, cx);
                    })))).child(content).into_any_element()
        }
        Some(Field::Closing) => surface.border_b_1().rounded_bl(cx.theme().radius).rounded_br(cx.theme().radius).pb_2().child(content).into_any_element(),
        _ => surface.child(content).into_any_element(),
    }
}

fn edit_button(
    context: &FieldContext<'_>,
    id: impl Into<ElementId>,
    key: L10nKey,
    range: Range<usize>,
    accessibility: impl Into<SharedString>,
    cx: &mut Context<SourcePresentation>,
) -> AnyElement {
    let revision = context.revision();
    let grammar = context.grammar();
    div().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(Button::new(id).ghost().small().label(t(key)).accessibility_label(accessibility)
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.document().read(cx).revision() != revision || this.document().read(cx).grammar() != grammar { return; }
                this.command(Command::FinishComposition, window, cx);
                this.command(Command::Select(Selection::new(range.start, range.end)), window, cx);
                this.focus_handle(cx).focus(window, cx);
            }))).into_any_element()
}

// Decode the literal's display/navigation value only. Its exact source and
// quoting remain authoritative for all edits and saves.
fn literal_value(value: String) -> String {
    if !value.contains('&') { return value; }
    let mut result = String::with_capacity(value.len());
    let mut rest = value.as_str();
    while let Some(at) = rest.find('&') {
        result.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest.bytes().take(34).position(|byte| byte == b';').and_then(|end| {
            let reference = &rest[1..end];
            let decoded = if let Some(number) = reference.strip_prefix('#') {
                let (number, radix) = number.strip_prefix(['x', 'X']).map_or((number, 10), |number| (number, 16));
                if number.is_empty() || u32::from_str_radix(number, radix).is_err() { None }
                else { Some(markdown::decode_numeric(number, radix)) }
            } else { markdown::decode_named(reference, true) };
            decoded.map(|decoded| (end + 1, decoded))
        });
        if let Some((end, decoded)) = decoded {
            result.push_str(&decoded);
            rest = &rest[end..];
        } else {
            result.push('&');
            rest = &rest[1..];
        }
    }
    result.push_str(rest);
    result
}
