//! Shared dialog chrome primitives: the card layout grid, the button tones
//! and the button builder. Owned here so the workbench editor's overlays and
//! the root shell's cards (SSH sheet, worktree prompt, switcher) build their
//! answers from one definition and cannot drift apart. The root package keeps
//! the composed card helpers (header rows, fields, keycaps) in its own
//! `ui::dialog`; anything two surfaces render lives here.

use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::{
    App, ClickEvent, Div, ElementId, Hsla, SharedString, Stateful, TestSupportExt as _, Window,
    prelude::*, px, rems,
};

use crate::presets::Surface;

/// The card's corner — the switcher's, a notch rounder than a menu: a dialog
/// is the one thing on screen while it is up.
pub const CARD_RADIUS: f32 = 12.;

/// The title row, the height of the switcher's search row, so a prompt that
/// opens where the switcher just closed puts its first line in the same place.
pub const HEADER_H: f32 = 48.;

/// The button row. A 28px button in it leaves 6px above and below, the same
/// fit as the switcher's footer and its New workspace button.
pub const FOOTER_H: f32 = 40.;

/// How far the title and the fields sit in from the card's edge — the
/// switcher's search-row inset, so the text columns of the two line up.
pub const INSET: f32 = 18.;

/// A text field: a borderless well on the faint fill, the sidebar search's
/// shape. The field's own `.small()` padding (8px) is what sets the text in.
pub const FIELD_H: f32 = 28.;

/// A button: the field's height and the field's corner — the v5 confirm
/// card's 28px pill-less rectangle, on every card alike.
pub const BUTTON_H: f32 = 28.;
const BUTTON_RADIUS: f32 = 7.;
const BUTTON_PAD: f32 = 14.;
/// Button text: a half step under the body size, the shell tab-text step.
const BUTTON_TEXT: f32 = 12.5 / 16.;

/// A key named in a hint: an 18px cap on the faint fill.
pub const KEYCAP: f32 = 18.;

/// What a button is asking for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// The one answer a card is for: an inverted neutral, the surface's ink as
    /// the fill — the Commit button's paint, not the accent. The accent is
    /// already spent on focus and on the rail; a dialog of blue buttons made
    /// every sheet look like a call to action.
    Primary,
    /// Everything else: the well's faint fill at rest, one rung up under the
    /// pointer. A bare word with no shape read as a link, not as the second
    /// answer to the question.
    Secondary,
    /// The action a card warns about. Red, and only on the one site that has
    /// earned it — overriding a changed host key.
    Danger,
}

/// A dialog button. The click handler is attached only while `enabled`: a
/// disabled button has to be inert, not merely grey.
///
/// Disabled, every tone sinks to the faint fill with secondary ink — the
/// same fall the Commit button takes — so it never advertises an action that
/// cannot run.
pub fn button(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    tone: Tone,
    enabled: bool,
    rungs: Surface,
    cx: &App,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui_kit::base::ObservedElement<Stateful<Div>> {
    let theme = cx.theme();
    let surface: Hsla = gpui_kit::rgb(rungs.base).into();
    let (fill, hover, ink): (Option<Hsla>, Option<Hsla>, Hsla) = match (tone, enabled) {
        // The surface's own next rung, like a well: `muted` is a step off the
        // window, and on a dark popover it was the card's own colour.
        (_, false) => (
            Some(gpui_kit::rgb(rungs.hover).into()),
            None,
            theme.muted_foreground,
        ),
        (Tone::Primary, true) => {
            let fg = theme.foreground;
            (Some(fg), Some(fg.blend(surface.opacity(0.14))), surface)
        }
        (Tone::Danger, true) => {
            let red = theme.danger;
            (
                Some(red),
                Some(red.blend(surface.opacity(0.14))),
                theme.danger_foreground,
            )
        }
        (Tone::Secondary, true) => (
            Some(gpui_kit::rgb(rungs.hover).into()),
            Some(gpui_kit::rgb(rungs.selected).into()),
            theme.foreground,
        ),
    };
    h_flex()
        .id(id)
        .test_support()
        .items_center()
        .justify_center()
        .h(px(BUTTON_H))
        .px(px(BUTTON_PAD))
        .rounded(px(BUTTON_RADIUS))
        .text_size(rems(BUTTON_TEXT))
        .when(tone != Tone::Secondary, |b| {
            b.font_weight(gpui_kit::FontWeight::MEDIUM)
        })
        .text_color(ink)
        .when_some(fill, |b, fill| b.bg(fill))
        .when_some(hover, |b, hover| b.hover(move |s| s.bg(hover)))
        .when(enabled, |b| b.cursor_pointer().on_click(on_click))
        .child(text.into())
}

/// The standard two answers for a destructive or ambiguous prompt: the action
/// on the right where the machine puts it, and the safe answer on the left,
/// marked as the cancel so it also answers Escape, Space and the initial
/// keyboard focus.
///
/// gpui renders answer 0 rightmost, so the action goes first — `Ok(0)` is
/// "they meant it" and everything else, including a dropped channel, is "leave
/// it alone".
pub fn confirm_answers(action: &str, keep: &str) -> [gpui_kit::PromptButton; 2] {
    [
        gpui_kit::PromptButton::ok(action),
        gpui_kit::PromptButton::cancel(keep),
    ]
}
