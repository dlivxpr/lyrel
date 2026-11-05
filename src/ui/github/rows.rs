//! Stack connector presentation, semantically ported from tty7
//! 6e3d2a414d2ea0de3ea1628480c5bce6e932b85e (Apache-2.0).

use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::{AnyElement, Div, Hsla, SharedString, TestSupportExt as _, div, prelude::*, rems};
use lyrel_core::core::github::stack::Cell;

use crate::ui::right_panel::{META, ROW_INSET};

pub(crate) const ROW_H: f32 = 26.;
pub(crate) const GLYPH: f32 = 14.;

fn connector(lane: usize, height: f32, color: Hsla) -> Div {
    div()
        .absolute()
        .left(rems((lane as f32 * GLYPH + GLYPH / 2. - 0.5) / 16.))
        .w(rems(1. / 16.))
        .h(rems(height / 16.))
        .bg(color)
}

pub(crate) fn stack_glyph(
    number: u64,
    glyph: AnyElement,
    cells: &[Cell],
    cx: &gpui_kit::App,
) -> AnyElement {
    let color = cx.theme().muted_foreground;
    let gap = (ROW_H - GLYPH) / 2.;
    let x = |lane: usize| lane as f32 * GLYPH + GLYPH / 2.;
    let node = cells
        .iter()
        .position(|c| matches!(c, Cell::Node { .. }))
        .unwrap_or(0);
    let mut column = div()
        .id(SharedString::from(format!("panel-github-stack-{number}")))
        .test_support()
        .flex_none()
        .relative()
        .w(rems(cells.len().max(1) as f32 * GLYPH / 16.))
        .h(rems(ROW_H / 16.))
        .child(
            div()
                .absolute()
                .top(rems(gap / 16.))
                .left(rems(node as f32 * GLYPH / 16.))
                .child(glyph),
        );
    for (lane, &cell) in cells.iter().enumerate() {
        column = match cell {
            Cell::Empty => column,
            Cell::Pipe => column.child(connector(lane, ROW_H, color).top_0()),
            Cell::Node { up, down } => column
                .when(up, |d| d.child(connector(lane, gap, color).top_0()))
                .when(down, |d| d.child(connector(lane, gap, color).bottom_0())),
            Cell::Join => column
                .child(connector(lane, gap / 2., color).top_0())
                .child(
                    div()
                        .absolute()
                        .top(rems(gap / 2. / 16.))
                        .left(rems((x(node) - 0.5) / 16.))
                        .w(rems((x(lane) - x(node) + 1.) / 16.))
                        .h(rems(1. / 16.))
                        .bg(color),
                ),
        };
    }
    column.into_any_element()
}

pub(crate) fn stack_trunk(number: u64, base: &str, cx: &gpui_kit::App) -> AnyElement {
    let theme = cx.theme();
    let color = theme.muted_foreground;
    const HEIGHT: f32 = 18.;
    const DOT: f32 = 5.;
    h_flex()
        .id(SharedString::from(format!("panel-github-trunk-{number}")))
        .test_support()
        .role(gpui_kit::Role::StaticText)
        .aria_label(base.to_string())
        .h(rems(HEIGHT / 16.))
        .px(rems(ROW_INSET / 16.))
        .gap_2()
        .child(
            div()
                .flex_none()
                .relative()
                .w(rems(GLYPH / 16.))
                .h(rems(HEIGHT / 16.))
                .child(connector(0, HEIGHT / 2., color).top_0())
                .child(
                    div()
                        .absolute()
                        .top(rems((HEIGHT - DOT) / 2. / 16.))
                        .left(rems((GLYPH - DOT) / 2. / 16.))
                        .size(rems(DOT / 16.))
                        .rounded_full()
                        .bg(color),
                ),
        )
        .child(
            div()
                .text_size(rems(META))
                .font_family(theme.mono_font_family.clone())
                .text_color(theme.muted_foreground)
                .child(base.to_string()),
        )
        .into_any_element()
}
