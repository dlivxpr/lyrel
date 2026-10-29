use super::{Component, ComponentId, Field, ViewState, view::{self, FieldContext}};
use super::super::{Command, Selection, SourcePresentation};
use crate::ui::i18n::{L10nKey, t};
use gpui_kit::{*, prelude::*};
use gpui_kit::base::input::Rope;
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _, tab::Tab};
use gpui_kit::test::TestSupportExt as _;

pub(in super::super) fn matches(name: &str) -> bool { matches!(name, "Tabs" | "Tab") }

fn named(component: &Component, source: &Rope, name: &str) -> bool {
    source.slice(component.name_range()).chars().eq(name.chars())
}
fn children<'a>(components: &'a [Component], source: &Rope, group: &'a Component) -> impl Iterator<Item = &'a Component> {
    group.child_ids().iter().filter_map(move |id| components.iter().find(|component| component.id() == *id))
        .filter(move |component| named(component, source, "Tab"))
}
fn active(components: &[Component], source: &Rope, state: &ViewState, group: &Component) -> Option<ComponentId> {
    let requested = state.active_child.get(&group.id()).copied();
    children(components, source, group).find(|child| Some(child.id()) == requested)
        .or_else(|| children(components, source, group).next()).map(Component::id)
}

/// Layout only: every ancestor Tabs imposes its own active-child constraint.
/// Never remove hidden leaves from the source projection or document history.
pub(in super::super) fn visible(context: &FieldContext<'_>) -> bool {
    let range = context.range();
    context.components().iter().filter(|group| named(group, context.source(), "Tabs")).all(|group| {
        let selected = active(context.components(), context.source(), context.state(), group);
        children(context.components(), context.source(), group).all(|child| {
            let child_range = child.source_range();
            !(child_range.start <= range.start && range.end <= child_range.end) || Some(child.id()) == selected
        })
    })
}

/// A source/undo selection reveals its own ancestors; it does not change text.
/// The caret is authoritative here, so a cross-child range follows its head.
pub(in super::super) fn reveal_selection(components: &[Component], source: &Rope, state: &mut ViewState, selection: Selection) -> bool {
    let mut changed = false;
    for group in components.iter().filter(|group| named(group, source, "Tabs")) {
        if let Some(child) = children(components, source, group).find(|child| {
            let range = child.source_range();
            range.start <= selection.head() && selection.head() < range.end
        }) {
            if active(components, source, state, group) != Some(child.id()) {
                state.active_child.insert(group.id(), child.id());
                changed = true;
            }
        }
    }
    changed
}

fn label(component: &Component, source: &Rope) -> SharedString {
    component.properties().iter().find(|property| {
        property.name_range().is_some_and(|range| source.slice(range).chars().eq("title".chars()))
            && property.kind() == super::PropertyKind::Literal
    }).map(|property| source.slice(property.value_range()).to_string().into())
        .unwrap_or_else(|| t(L10nKey::EditorMdxTabsUntitled).into())
}

pub(in super::super) fn render(context: &FieldContext<'_>, content: AnyElement, _: &mut Window, cx: &mut Context<SourcePresentation>) -> AnyElement {
    let Some(component) = context.component() else { return content; };
    let is_group = named(component, context.source(), "Tabs");
    if context.field().is_none() {
        if is_group { return content; }
        let title = label(component, context.source());
        return div().id(("mdx-tabs-panel", component.id().value())).test_support().aria_label(title)
            .w_full().pl_3().border_l_1().border_color(cx.theme().border)
            .child(content).into_any_element();
    }
    if context.field() != Some(Field::Name) {
        return view::render_generic(context, content, cx);
    }
    let title: SharedString = if is_group { t(L10nKey::EditorMdxTabsGroup).into() } else { label(component, context.source()) };
    let mut result = div().w_full().flex().flex_col().gap_1()
        .child(div().text_sm().text_color(cx.theme().foreground).child(title));
    if is_group {
        let selected = active(context.components(), context.source(), context.state(), component);
        let group = component.id();
        let mut strip = div().id(("mdx-tabs-strip", group.value())).flex().flex_wrap().gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        let choices: Vec<_> = children(context.components(), context.source(), component).collect();
        let ids: Vec<_> = choices.iter().map(|child| child.id()).collect();
        for (ix, child) in choices.iter().enumerate() {
            let id = child.id();
            let title = label(child, context.source());
            let selector = title.clone();
            let keys = ids.clone();
            // Each published Tab is scoped by a stable component ID rather
            // than TabBar's positional child identity. Base owns activation,
            // focus-visible and AX tab role; the document owns the selection.
            strip = strip.child(div().id(("mdx-tabs-choice", id.value()))
                .test_support().aria_label(selector)
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    let next = match event.keystroke.key.as_str() {
                        "left" => Some((ix + keys.len() - 1) % keys.len()),
                        "right" => Some((ix + 1) % keys.len()),
                        "home" => Some(0), "end" => Some(keys.len() - 1),
                        _ => None,
                    };
                    if let Some(next) = next {
                        this.activate_mdx_tab(group, keys[next], window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(Tab::new().segmented().small().selected(selected == Some(id))
                    .label(title.clone()).aria_label(title)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_mdx_tab(group, id, window, cx);
                        cx.stop_propagation();
                    }))));
        }
        if choices.is_empty() {
            result = result.child(div().text_xs().text_color(cx.theme().muted_foreground).child(t(L10nKey::EditorMdxTabsEmpty)));
        } else { result = result.child(strip); }
    }
    // Existing name/property/children commands always target the one input.
    result.child(view::render_generic(context, content, cx)).into_any_element()
}
