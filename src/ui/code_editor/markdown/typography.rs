use super::{Kind, RenderContext};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::*;
use gpui_kit::{App, FontWeight, Pixels, SharedString, StyleRefinement, Window, relative, rems};

// Fixed reference 9c09b8c: page_view.rs:77-78 (28px at 16px body),
// zorite-editor/lib.rs:204-224,230-242 and markdown_syntax.rs:1664-1673.
// These are document ratios, not a second application scale or palette.
pub(super) const LINE_HEIGHT: f32 = 1.45;
pub(super) const LIST_GAP: f32 = 0.25;

pub(super) fn inset(live: bool) -> f32 {
    if live { 1.75 } else { 0.75 }
}

pub(super) fn content_width(viewport: Pixels, rem: Pixels, live: bool) -> Pixels {
    // The reference fills its available column, with no fixed-width page or
    // max-width shell. Keep the scrollbar at the viewport edge, not this inset.
    (viewport - rem * (inset(live) * 2.)).max(rem)
}

#[derive(Clone, PartialEq)]
pub(super) struct LayoutKey {
    width: Pixels,
    rem: Pixels,
    family: SharedString,
    mono_family: SharedString,
    font_size: Pixels,
    mono_size: Pixels,
    live: bool,
    wrap: bool,
}

impl LayoutKey {
    pub(super) fn new(width: Pixels, live: bool, wrap: bool, window: &Window, cx: &App) -> Self {
        Self {
            width,
            rem: window.rem_size(),
            family: cx.theme().font_family.clone(),
            mono_family: cx.theme().mono_font_family.clone(),
            font_size: cx.theme().font_size,
            mono_size: cx.theme().mono_font_size,
            live,
            wrap,
        }
    }
}

fn heading_scale(level: u8) -> f32 {
    match level {
        1 => 1.8,
        2 => 1.5,
        3 => 1.3,
        4 => 1.15,
        5 => 1.05,
        _ => 1.,
    }
}

pub(super) fn context(context: RenderContext, live: bool, cx: &App) -> RenderContext {
    if !live {
        return context;
    }
    let foreground = cx.theme().foreground;
    let style = context.style().clone()
        // Editable blank lines already own paragraph spacing. Adding renderer
        // paragraph gaps would turn a single LF into an apparent blank line.
        .with_paragraph_gap(rems(0.))
        .with_heading(move |level| {
            StyleRefinement::default()
                .p_0()
                .text_size(rems(heading_scale(level)))
                .line_height(relative(LINE_HEIGHT))
                .font_weight(FontWeight::BOLD)
                .text_color(foreground)
        })
        .with_code_background(cx.theme().muted)
        .with_code_block(
            StyleRefinement::default()
                .px_3()
                .py_2()
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(rems(1.))
                .line_height(relative(LINE_HEIGHT))
                .rounded(cx.theme().radius),
        )
        .with_list_style(
            StyleRefinement::default()
                // Reference: representative bullet floor 7 + text gap 8.
                // Longer ordered markers retain their intrinsic width.
                .min_w(rems(15. / 16.))
                .pr(rems(0.5)),
        )
        .with_blockquote_style(
            StyleRefinement::default().pl(rems(14. / 16.)),
        );
    context.with_style(style)
}

pub(super) fn list_level(style: super::super::lists::Structure) -> f32 {
    // Reference's per-level step is marker + 8px gap + four 4.5px spaces
    // at 16px body. Native task controls keep their own hit target/focus.
    let marker = if style.task.is_some() {
        0.9
    } else if matches!(style.marker, Some(super::super::lists::Marker::Ordered(..))) {
        0.875
    } else {
        7. / 16.
    };
    marker + 0.5 + 18. / 16.
}

pub(super) fn estimated_height(kind: Kind, lines: f32, live: bool, rem: Pixels) -> Pixels {
    if !live {
        return rem * (lines * 1.5 + 0.5);
    }
    let scale = if let Kind::Heading(level) = kind { heading_scale(level) } else { 1. };
    let padding = match kind {
        Kind::Code(_) | Kind::Mermaid(_) => 1.,
        Kind::List(_) => LIST_GAP,
        _ => 0.,
    };
    rem * (lines * LINE_HEIGHT * scale + padding)
}
