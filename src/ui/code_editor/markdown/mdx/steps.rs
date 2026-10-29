//! Native procedural hierarchy over the existing source-mapped editing leaves.
//! Numbers are derived from direct Step siblings, never stored in the document.
use super::super::{Command, Selection, SourcePresentation};
use super::{
    Component, Field,
    view::{FieldContext, render_generic},
};
use crate::ui::i18n::{L10nKey, t, t_fmt};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
};
use gpui_kit::*;

pub(super) fn matches(context: &FieldContext<'_>) -> bool {
    context.component().is_some_and(|component| {
        named(context, component, "Steps") || named(context, component, "Step")
    })
}

fn named(context: &FieldContext<'_>, component: &Component, name: &str) -> bool {
    context
        .source()
        .slice(component.name_range())
        .chars()
        .eq(name.chars())
}

fn ordinal(context: &FieldContext<'_>, step: &Component) -> usize {
    let Some(parent) = context.components().iter().find(|component| {
        named(context, component, "Steps") && component.child_ids().contains(&step.id())
    }) else {
        return 1;
    };
    parent
        .child_ids()
        .iter()
        .filter(|id| {
            context
                .components()
                .iter()
                .any(|component| component.id() == **id && named(context, component, "Step"))
        })
        .position(|id| *id == step.id())
        .map_or(1, |ix| ix + 1)
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
    let is_step = named(context, component, "Step");
    let mut body = div().w_full().min_w_0();
    if is_step {
        body = body.border_l_1().border_color(cx.theme().border).pl_3();
    }
    match context.field() {
        Some(Field::Name) => {
            let label = if is_step {
                t_fmt(
                    L10nKey::EditorMdxStepsNumber,
                    &[("number", &ordinal(context, component).to_string())],
                )
            } else {
                let count = component
                    .child_ids()
                    .iter()
                    .filter(|id| {
                        context
                            .components()
                            .iter()
                            .any(|child| child.id() == **id && named(context, child, "Step"))
                    })
                    .count();
                t_fmt(
                    if count == 1 {
                        L10nKey::EditorMdxStepsSequenceSingular
                    } else {
                        L10nKey::EditorMdxStepsSequence
                    },
                    &[("count", &count.to_string())],
                )
            };
            let mut header = div()
                .id(("mdx-steps-header", component.id().value()))
                .test_support()
                .role(if is_step { Role::ListItem } else { Role::List })
                .aria_label(label.clone())
                .flex()
                .flex_col()
                .gap_1()
                .text_color(cx.theme().foreground)
                .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label));
            if is_step {
                if let Some(title) = component.properties().iter().find(|property| {
                    property.name_range().is_some_and(|range| {
                        context.source().slice(range).chars().eq("title".chars())
                    })
                }) {
                    header = header
                        .child(
                            div()
                                .text_sm()
                                .child(context.source().slice(title.value_range()).to_string()),
                        )
                        .child(select_control(
                            context,
                            "mdx-step-title",
                            L10nKey::EditorMdxStepsEditTitle,
                            title.value_range(),
                            false,
                            cx,
                        ));
                }
            }
            header = header.child(select_control(
                context,
                "mdx-steps-source",
                if is_step {
                    L10nKey::EditorMdxStepsEditSource
                } else {
                    L10nKey::EditorMdxStepsEditContainerSource
                },
                component.source_range(),
                true,
                cx,
            ));
            body.child(header).child(content).into_any_element()
        }
        // Property values and the empty/self-closing insertion boundary retain
        // the common structured editor, including expressions and spread props.
        Some(Field::Property(_) | Field::Children) => body
            .child(render_generic(context, content, cx))
            .into_any_element(),
        Some(Field::Closing) => body
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(if is_step {
                        L10nKey::EditorMdxStepsEndStep
                    } else {
                        L10nKey::EditorMdxStepsEndSequence
                    })),
            )
            .child(content)
            .into_any_element(),
        _ => body.child(content).into_any_element(),
    }
}

fn select_control(
    context: &FieldContext<'_>,
    id: &'static str,
    key: L10nKey,
    range: std::ops::Range<usize>,
    source: bool,
    cx: &mut Context<SourcePresentation>,
) -> AnyElement {
    let revision = context.revision();
    let grammar = context.grammar();
    let component = context.component().expect("procedural component").id();
    // The outer boundary isolates document pointer placement without stopping
    // the Button's own press tracking, focus or keyboard activation.
    div()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            Button::new((id, component.value()))
                .ghost()
                .small()
                .label(t(key))
                .accessibility_label(t(key))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.document().read(cx).revision() != revision
                        || this.document().read(cx).grammar() != grammar
                    {
                        return;
                    }
                    this.command(Command::FinishComposition, window, cx);
                    if source && this.is_live() {
                        this.toggle_mode(window, cx);
                    }
                    this.command(
                        Command::Select(Selection::new(range.start, range.end)),
                        window,
                        cx,
                    );
                    this.focus_handle(cx).focus(window, cx);
                    cx.stop_propagation();
                })),
        )
        .into_any_element()
}
