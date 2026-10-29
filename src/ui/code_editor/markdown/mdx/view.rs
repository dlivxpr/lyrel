//! One field-rendering hook for generic and dedicated native component views.
//! Adapters decorate the existing leaf element, never replace its document,
//! input handler, source mapping, geometry key, or history.
use super::{Component, Field, Grammar, Property, PropertyKind, ViewState};
use super::super::{Command, Selection, SourcePresentation, projection::{Kind, Leaf, Projection}};
use gpui_kit::{*, prelude::*};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, button::{Button, ButtonVariants as _}};
use crate::ui::i18n::{t, L10nKey};

pub(in super::super) struct FieldContext<'a> {
    component: Option<&'a Component>,
    property: Option<&'a Property>,
    field: Option<Field>,
    source: &'a gpui_kit::base::input::Rope,
    state: &'a ViewState,
    revision: u64,
    grammar: Grammar,
}
impl<'a> FieldContext<'a> {
    pub(in super::super) fn new(projection: &'a Projection, leaf: &Leaf, source: &'a gpui_kit::base::input::Rope, state: &'a ViewState) -> Self {
        let field = if let Kind::Mdx(field) = leaf.kind { Some(field) } else { None };
        let own = projection.mdx.field(&leaf.range);
        let component = own.map(|(component, _)| component).or_else(|| projection.mdx.components.iter()
            .filter(|component| component.source_range().start <= leaf.range.start && leaf.range.end <= component.source_range().end)
            .min_by_key(|component| component.source_range().len()));
        Self { component, property: own.and_then(|(_, property)| property), field,
            source, state, revision: projection.revision, grammar: projection.grammar }
    }
    pub(in super::super) fn component(&self) -> Option<&Component> { self.component }
    pub(in super::super) fn property(&self) -> Option<&Property> { self.property }
    pub(in super::super) fn field(&self) -> Option<Field> { self.field }
    pub(in super::super) fn source(&self) -> &gpui_kit::base::input::Rope { self.source }
    pub(in super::super) fn state(&self) -> &ViewState { self.state }
}

/// Dedicated family modules attach here: choose the family from the positioned
/// component name, then decorate `content` using the same FieldContext. A normal
/// Markdown child has field=None; metadata-only Children marks the child boundary.
/// Returning content unchanged preserves its native continuous surface.
pub(in super::super) fn render_field(context: &FieldContext<'_>, content: AnyElement, _: &mut Window, cx: &mut Context<SourcePresentation>) -> AnyElement {
    render_generic(context, content, cx)
}

fn render_generic(context: &FieldContext<'_>, content: AnyElement, cx: &mut Context<SourcePresentation>) -> AnyElement {
    let Some(field) = context.field else { return content; };
    let mut header = div().flex().flex_col().gap_1().text_xs().text_color(cx.theme().muted_foreground);
    if field == Field::Name { header = header.child(t(L10nKey::EditorMdxGeneric)); }
    let (key, target, id, accessibility) = match (field, context.component, context.property) {
        (Field::Name, Some(component), _) => (
            L10nKey::EditorMdxName, Some(component.name_range()), "mdx-name".to_owned(), "Edit component name".to_owned(),
        ),
        (Field::Property(kind), _, Some(property)) => {
            let name = property.name_range().map(|range| context.source.slice(range).to_string());
            let key = if matches!(kind, PropertyKind::Expression | PropertyKind::Spread) { L10nKey::EditorMdxDynamic } else { L10nKey::EditorMdxProperty };
            (key, Some(property.value_range()), format!("mdx-property-{}", name.as_deref().unwrap_or("spread")), format!("Edit property: {}", name.as_deref().unwrap_or("spread")))
        }
        (Field::Children, Some(component), _) => (
            L10nKey::EditorMdxChildren, Some(component.children_range().start..component.children_range().start), "mdx-children".to_owned(), "Edit component children".to_owned(),
        ),
        (Field::Closing, _, _) => (L10nKey::EditorMdxClosing, None, String::new(), String::new()),
        _ => (L10nKey::EditorMdxDynamic, None, String::new(), String::new()),
    };
    if let Some(range) = target {
        let revision = context.revision;
        let grammar = context.grammar;
        let expansion = context.component.filter(|component| field == Field::Children && component.closing_name_range().is_none()).map(|component| {
            (component.source_range().end - 2..component.source_range().end, context.source.slice(component.name_range()).to_string())
        });
        header = header.child(Button::new(SharedString::from(id)).ghost().xsmall()
            .label(t(key)).accessibility_label(accessibility)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.document().read(cx).revision() != revision || this.document().read(cx).grammar() != grammar { return; }
                if let Some((ending, name)) = &expansion {
                    this.command(Command::ExpandComponent { ending: ending.clone(), name: name.clone() }, window, cx);
                } else { this.command(Command::Select(Selection::new(range.start, range.end)), window, cx); }
                this.focus_handle(cx).focus(window, cx);
                cx.stop_propagation();
            })));
    } else { header = header.child(t(key)); }
    div().w_full().child(header).child(content).into_any_element()
}
