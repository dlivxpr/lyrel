//! Native editorial callouts. Only presentation is family-specific: source,
//! nested Markdown, property selection, composition and undo keep one owner.
use super::{Field, view::{FieldContext, render_generic}};
use super::super::SourcePresentation;
use crate::ui::i18n::{t, L10nKey};
use gpui_kit::{*, prelude::*};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, button::{Button, ButtonVariants as _}};

pub(super) fn matches(context: &FieldContext<'_>) -> bool {
    callout_name(context).is_some()
}

fn callout_name(context: &FieldContext<'_>) -> Option<&'static str> {
    let component = context.component()?;
    ["Note", "Tip", "Info", "Warning"].into_iter().find(|name| {
        context.source().slice(component.name_range()).chars().eq(name.chars())
    })
}

pub(super) fn render(
    context: &FieldContext<'_>,
    content: AnyElement,
    _: &mut Window,
    cx: &mut Context<SourcePresentation>,
) -> AnyElement {
    let Some(component) = context.component() else { return content; };
    let (label, color) = match callout_name(context) {
        Some("Note") => (L10nKey::EditorCalloutNote, cx.theme().muted_foreground),
        Some("Tip") => (L10nKey::EditorCalloutTip, cx.theme().success),
        Some("Info") => (L10nKey::EditorCalloutInfo, cx.theme().info),
        Some("Warning") => (L10nKey::EditorCalloutWarning, cx.theme().warning),
        _ => return content,
    };
    let id = component.id();
    let source = context.state().source_components.contains(&id);
    let header = context.field() == Some(Field::Name) || source;
    let mut callout = div().w_full().min_w_0().border_l_2().border_color(color)
        .pl_3().pr_2().text_color(cx.theme().foreground);
    if header {
        let revision = context.revision();
        let grammar = context.grammar();
        let action = if source { L10nKey::EditorCalloutReturnContent } else { L10nKey::EditorCalloutEditSource };
        // The perimeter consumes pointer-down, not Button itself: the standard
        // control still owns press tracking, keyboard activation and AX naming.
        let controls = div().flex().items_center().flex_wrap().gap_2()
            .child(div().text_sm().text_color(color).child(t(label)))
            .child(div().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(Button::new(if source { "callout-return-content" } else { "callout-edit-source" })
                    .ghost().small().label(t(action)).accessibility_label(t(action))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.document().read(cx).revision() != revision || this.document().read(cx).grammar() != grammar { return; }
                        this.toggle_component_source(id, window, cx);
                        cx.stop_propagation();
                    }))));
        callout = callout.pt_2().child(controls);
    }
    let content = match context.field() {
        Some(Field::Property(_)) | Some(Field::Children) => render_generic(context, content, cx),
        _ => content,
    };
    if source {
        callout = callout.font_family(cx.theme().mono_font_family.clone()).child(content).pb_2();
    } else if context.field() == Some(Field::Closing) {
        callout = callout.pb_2().child(div().text_xs().text_color(cx.theme().muted_foreground).child(content));
    } else {
        callout = callout.child(content);
    }
    callout.into_any_element()
}
