//! Native disclosure controls decorate the existing source-mapped leaves.
//! Collapsing removes layout only; document text, children and history stay owned
//! by the shared Markdown document and its single input handler.
use super::super::{Selection, SourcePresentation};
use super::{
    Component, Field, PropertyKind, ViewState,
    view::{self, FieldContext},
};
use crate::ui::i18n::{L10nKey, t};
use gpui_kit::base::input::Rope;
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
};
use gpui_kit::{base::StyledExt, prelude::*, *};

pub(super) fn matches(name: &str) -> bool {
    matches!(name, "Accordion" | "AccordionGroup")
}

/// Check every ancestor, not only the nearest component. Hidden leaves remain
/// in Projection and in the source map; the layout owner filters them out before
/// measuring or virtualizing, without reserving a spacer for their old height.
pub(super) fn visible(context: &FieldContext<'_>) -> bool {
    let range = context.range();
    !context.components().iter().any(|component| {
        context.state().collapsed.contains(&component.id())
            && context
                .source()
                .slice(component.name_range())
                .chars()
                .eq("Accordion".chars())
            && component.children_range().start <= range.start
            && range.end <= component.source_range().end
    })
}

/// Explicit selection/navigation/undo into a hidden child reveals every
/// collapsed ancestor. The unified SourcePresentation reveal hook owns when to
/// invoke this rule, including source-to-live and asynchronous parse landing.
pub(in super::super) fn reveal_selection(
    components: &[Component],
    source: &Rope,
    state: &mut ViewState,
    selection: Selection,
) -> bool {
    let mut changed = false;
    for component in components {
        if !state.collapsed.contains(&component.id()) {
            continue;
        }
        let children = component.children_range();
        if [selection.anchor, selection.head]
            .into_iter()
            .any(|at| children.start <= at && at < component.source_range().end)
            && source
                .slice(component.name_range())
                .chars()
                .eq("Accordion".chars())
        {
            changed |= state.collapsed.remove(&component.id());
        }
    }
    changed
}

pub(super) fn render(
    context: &FieldContext<'_>,
    content: AnyElement,
    _: &mut Window,
    cx: &mut Context<SourcePresentation>,
) -> AnyElement {
    let Some(component) = context.component() else {
        return content;
    };
    let group = context
        .source()
        .slice(component.name_range())
        .chars()
        .eq("AccordionGroup".chars());
    let id = component.id();
    match context.field() {
        Some(Field::Name) if group => {
            gpui_kit::base::Accordion::new(("mdx-accordion-group", id.value()))
                .w_full()
                .min_w_0()
                .py_1()
                .child(
                    div()
                        .text_sm()
                        .font_medium()
                        .child(t(L10nKey::EditorMdxAccordionsGroup)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(t(L10nKey::EditorMdxName)),
                )
                .child(content)
                .into_any_element()
        }
        Some(Field::Name) => {
            // Only literal labels are presented as values. Expressions/spreads
            // remain labeled source fields and are never evaluated.
            let title = component
                .properties()
                .iter()
                .find(|property| {
                    property.kind() == PropertyKind::Literal
                        && property.name_range().is_some_and(|range| {
                            context.source().slice(range).chars().eq("title".chars())
                        })
                })
                .map(|property| context.source().slice(property.value_range()).to_string())
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| t(L10nKey::EditorMdxAccordionsSection).to_string());
            let open = !context.state().collapsed.contains(&id);
            let accessibility = format!(
                "{}: {title}",
                if open {
                    "Collapse accordion"
                } else {
                    "Expand accordion"
                }
            );
            div()
                .id(("mdx-accordion", id.value()))
                .w_full()
                .min_w_0()
                .py_1()
                .child(
                    div()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            gpui_kit::base::Button::new("mdx-accordion-disclosure")
                                .flex()
                                .items_center()
                                .gap_1()
                                .px_2()
                                .py_1()
                                .text_sm()
                                .rounded_md()
                                .focus_visible(|style| style.border_1().border_color(cx.theme().primary))
                                .aria_expanded(open)
                                .child(
                                    gpui_kit::component::Icon::new(if open {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .small(),
                                )
                                .child(title)
                                .accessibility_label(accessibility)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.toggle_mdx_accordion(id, window, cx);
                                    cx.stop_propagation();
                                })),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(t(L10nKey::EditorMdxName)),
                )
                .child(content)
                .into_any_element()
        }
        Some(Field::Property(_)) | Some(Field::Children) => {
            view::render_generic(context, content, cx)
        }
        Some(Field::Closing) => div()
            .w_full()
            .text_color(cx.theme().muted_foreground)
            .child(content)
            .into_any_element(),
        // Nested Markdown keeps its native leaf, text marks, geometry and input.
        None => div()
            .w_full()
            .min_w_0()
            .pl_3()
            .border_l_1()
            .border_color(cx.theme().border)
            .child(content)
            .into_any_element(),
        _ => content,
    }
}
