//! 格式无关的结构树行、源码面容器与失败／加载呈现。
//!
//! JSON、TOML 与 YAML 共用这里的视觉与无障碍语义：一行只由显示文本、深度、
//! 展开状态和可选徽标组成，点击、键盘与格式命令仍属于各自的 presentation。
//! 这里不持有投影、文档或可写范围，也不把任何格式的语义塞进共享类型。

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, spinner::Spinner, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClickEvent, ElementId, MouseButton, MouseDownEvent, SharedString,
    TestSupportExt as _, Window, div, relative, rems,
};
use lyrel_markdown_editor::EditorState as SourceEditor;

/// 一行可见结构节点的纯呈现数据。
pub struct StructureRowData {
    pub id: SharedString,
    pub depth: usize,
    pub expandable: bool,
    pub expanded: bool,
    pub selected: bool,
    pub striped: bool,
    pub key: SharedString,
    pub value: SharedString,
    /// 值的颜色由格式按节点种类决定；共享行不解释 kind。
    pub color: gpui_kit::Hsla,
    /// 锚点、tag 或别名目标的简短说明。
    pub badge: Option<SharedString>,
    pub record_line: Option<usize>,
    pub error: Option<SharedString>,
    pub aria_label: SharedString,
}

/// 画一行结构树，并把点击行为交给格式自己的 presentation。
pub fn structure_row(
    data: StructureRowData,
    on_mouse_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &mut App,
) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let key = data.key.clone();
    let value = data.value.clone();
    div()
        .id(data.id)
        .test_support()
        .role(gpui_kit::Role::TreeItem)
        .aria_label(data.aria_label)
        .aria_value(value.clone())
        .aria_selected(data.selected)
        .when_some(data.badge.clone(), |row, badge| row.aria_description(badge))
        .when(data.expandable, |row| row.aria_expanded(data.expanded))
        .flex()
        .items_center()
        .gap_2()
        // Row height, indent and the chevron slot are part of the ordinary
        // zoom scale: `rems` keeps the 30/10/14px ratio at the base 16px font
        // and follows the theme's base font with the virtual list's measured
        // item height.
        .h(rems(1.875))
        .pl(rems(0.625 + data.depth.min(24) as f32 * 0.875))
        .pr_3()
        .text_sm()
        .font_family(cx.theme().mono_font_family.clone())
        .when(data.striped, |row| {
            row.bg(cx.theme().foreground.opacity(0.025))
        })
        .when(data.selected, |row| row.bg(cx.theme().sidebar_accent))
        .cursor_pointer()
        .child(
            div()
                .w(rems(0.875))
                .flex_shrink_0()
                .when(data.expandable, |spacer| {
                    spacer.child(Icon::new(if data.expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    }))
                }),
        )
        .when_some(data.record_line, |row, line| {
            row.child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("L{line}")),
            )
        })
        .when(!key.is_empty(), |row| {
            row.child(
                div()
                    .max_w(relative(0.42))
                    .truncate()
                    .text_color(cx.theme().blue)
                    .child(key),
            )
            .child(div().text_color(muted).child(":"))
        })
        .when_some(data.badge, |row, badge| {
            row.child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(cx.theme().yellow)
                    .child(badge),
            )
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(data.color)
                .child(value),
        )
        .when_some(data.error, |row, error| {
            row.child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(cx.theme().warning)
                    .child(error),
            )
        })
        .test_support()
        .on_mouse_down(MouseButton::Left, on_mouse_down)
        .on_click(on_click)
        .into_any_element()
}

/// 源码面：把实际滚动视口交给共享 Document 的 presentation。
pub fn source_body_for(source: &gpui_kit::Entity<SourceEditor>) -> AnyElement {
    let source = source.clone();
    let measure = source.clone();
    div()
        .on_children_prepainted(move |bounds, _, cx| {
            if let Some(bounds) = bounds.first() {
                measure.update(cx, |source, cx| source.set_scroll_viewport(*bounds, cx));
            }
        })
        .flex_1()
        .min_w_0()
        .min_h_0()
        .child(v_flex().size_full().min_h_0().child(source))
        .into_any_element()
}

/// 结构不可用时的可见说明；`scope` 让各格式保留自己的元素身份。
pub fn structure_error(
    scope: impl Into<ElementId>,
    title: SharedString,
    detail: SharedString,
    cx: &mut App,
) -> AnyElement {
    let (foreground, muted, warning) = {
        let theme = cx.theme();
        (theme.foreground, theme.muted_foreground, theme.warning)
    };
    v_flex()
        .id(scope)
        .test_support()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .items_center()
        .justify_center()
        .gap_2()
        .px_4()
        .child(
            Icon::new(IconName::TriangleAlert)
                .large()
                .text_color(warning),
        )
        .child(div().text_sm().text_color(foreground).child(title))
        .child(
            div()
                .max_w(rems(34.))
                .text_xs()
                .text_color(muted)
                .child(detail),
        )
        .into_any_element()
}

/// 后台解析进行中的可见状态。
pub fn structure_loading(
    scope: impl Into<ElementId>,
    label: SharedString,
    cx: &mut App,
) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    v_flex()
        .id(scope)
        .flex_1()
        .min_w_0()
        .min_h_0()
        .items_center()
        .justify_center()
        .gap_2()
        .text_xs()
        .text_color(muted)
        .child(Spinner::new().small())
        .child(label)
        .into_any_element()
}
