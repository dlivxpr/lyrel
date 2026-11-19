//! Chrome for the small modal cards the app draws itself — the SSH sheet, the
//! worktree prompt — and the buttons and key hints the home page shares with
//! them.
//!
//! The workspace switcher is the reference: the same 12px card, 48px header
//! row with an `esc` cap, hairline-divided footer and faint keycaps. Each of
//! these cards used to spell its own padding, title weight and button variants
//! out a builder call at a time, and they had drifted — a 20px inset here, a
//! semibold title there, the accent-filled primary everywhere. One place now,
//! so two sheets on screen at once cannot disagree about what a dialog is.

use gpui_kit::component::input::Input;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{App, Div, ElementId, Hsla, SharedString, Stateful, div, prelude::*, px, rems};

use crate::ui::presets::Surface;
use crate::ui::right_panel::{HEADING, TEXT};

// The shared card grid and button live in `lyrel-ui-base` so the workbench
// editor's overlays and this shell chrome cannot drift apart; the composed
// card helpers below build on them.
pub(crate) use lyrel_ui_base::dialog::{
    CARD_RADIUS, FIELD_H, FOOTER_H, HEADER_H, INSET, KEYCAP, Tone, button,
};

/// Space between a field's label and its well, and between one field and the
/// next. Four-pixel grid: tight inside a pair, a step wider between pairs.
const LABEL_GAP: f32 = 6.;
const FIELD_GAP: f32 = 14.;

/// The ladder a dialog's hovers are taken from. Cards sit on the popover
/// surface; the home page passes the window's own.
pub(crate) fn popover_rungs(cx: &App) -> Surface {
    cx.global::<crate::ui::presets::Surfaces>().popover
}

/// The card itself, opaque and occluding. Callers add the header, body and
/// footer as children, in that order.
///
/// `title` is what a screen reader announces the card as — a dialog, named
/// by the question it asks.
pub(crate) fn card(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    width: f32,
    cx: &App,
) -> Stateful<Div> {
    v_flex()
        .id(id)
        .role(gpui_kit::Role::Dialog)
        .aria_label(title)
        .occlude()
        .w(px(width))
        .map(|panel| crate::ui::theme::floating_surface(panel, cx))
        .rounded(px(CARD_RADIUS))
        .overflow_hidden()
}

/// The title row: the title at body size in medium, and the `esc` cap on the
/// right that says how to back out. Every card here answers Escape.
pub(crate) fn header(title: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .h(px(HEADER_H))
        .pl(px(INSET))
        .pr(px(14.))
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(rems(TEXT))
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .child(title),
        )
        .child(keycap("esc", cx))
}

/// The card's content column.
pub(crate) fn body() -> Div {
    v_flex()
        .gap(px(FIELD_GAP))
        .px(px(INSET))
        .pt(px(16.))
        .pb(px(18.))
}

/// The button row: answers on the right, the action last, over a hairline.
pub(crate) fn footer(cx: &App) -> Div {
    h_flex()
        .flex_none()
        .items_center()
        .justify_end()
        .gap(px(6.))
        .h(px(FOOTER_H))
        .px(px(6.))
        .border_t_1()
        .border_color(cx.theme().border)
}

/// A field's name: the compact section-heading step, medium, secondary ink.
pub(crate) fn label(text: impl IntoElement, cx: &App) -> Div {
    div()
        .text_size(rems(HEADING))
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

/// The fill of a well on a card: one rung up the card's own ladder.
///
/// Not the theme's `muted`. That one is a step off the *window*, and a card
/// sits on the popover surface, which a dark theme lifts to about the same
/// value — so in the dark every field on a dialog lost its shape and the
/// labels floated over nothing.
pub(crate) fn well_fill(cx: &App) -> Hsla {
    gpui_kit::rgb(popover_rungs(cx).hover).into()
}

/// A text field in its well. The input loses its own border and fill — the
/// well is the shape — and keeps its padding, which is the text's inset.
pub(crate) fn field(input: Input, cx: &App) -> Div {
    h_flex()
        .items_center()
        .h(px(FIELD_H))
        .rounded(crate::ui::rounding::ROW_RADIUS)
        .bg(well_fill(cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(input.appearance(false).small()),
        )
}

/// A label over its field.
pub(crate) fn labelled(name: impl IntoElement, input: Input, cx: &App) -> Div {
    v_flex()
        .gap(px(LABEL_GAP))
        .child(label(name, cx))
        .child(field(input, cx))
}

/// A label over a control that is not a text field.
pub(crate) fn labelled_control(name: impl IntoElement, control: impl IntoElement, cx: &App) -> Div {
    v_flex()
        .gap(px(LABEL_GAP))
        .child(label(name, cx))
        .child(control)
}

/// A dropdown's closed face, in the same well as a text field, so a form of
/// fields and choices still reads as one column.
pub(crate) fn select_well(
    id: impl Into<gpui_kit::ElementId>,
    value: impl Into<gpui_kit::SharedString>,
    cx: &App,
) -> gpui_kit::Stateful<Div> {
    let tk = crate::ui::settings::kit::Tk::of(cx);
    h_flex()
        .id(id)
        .h(px(FIELD_H))
        .pl(px(10.))
        .pr(px(9.))
        .gap(px(8.))
        .items_center()
        .rounded(crate::ui::rounding::ROW_RADIUS)
        .bg(well_fill(cx))
        .cursor_pointer()
        .child(div().flex_1().min_w_0().truncate().child(value.into()))
        .child(crate::ui::settings::kit::chevron_down(&tk))
}

/// A block of read-only detail — a host and its key — set on the field's
/// fill, so it reads as a value to check rather than a line of prose.
pub(crate) fn well(cx: &App) -> Div {
    v_flex()
        .gap(px(4.))
        .px(px(10.))
        .py(px(8.))
        .rounded(crate::ui::rounding::ROW_RADIUS)
        .bg(well_fill(cx))
}

/// A key named in a hint: a small faint cap, never a button outline. The same
/// cap the switcher's header and footer draw.
pub(crate) fn keycap(label: impl Into<SharedString>, cx: &App) -> gpui_kit::AnyElement {
    let theme = cx.theme();
    div()
        .flex_shrink_0()
        .min_w(px(KEYCAP))
        .h(px(KEYCAP))
        .px(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .bg(well_fill(cx))
        .text_size(rems(11. / 16.))
        .text_color(theme.muted_foreground)
        .child(label.into())
        .into_any_element()
}

/// A key combination as one cap per key — `⌘` `T`, never `⌘T` in one. The
/// way Search Everywhere and the shortcut editor spell a chord; the home page
/// and the switcher's footer had each packed theirs into a single cap.
pub(crate) fn chord(tokens: impl IntoIterator<Item = impl Into<SharedString>>, cx: &App) -> Div {
    h_flex()
        .flex_shrink_0()
        .gap(px(3.))
        .children(tokens.into_iter().map(|t| keycap(t, cx)))
}
