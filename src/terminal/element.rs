//! One ordered terminal frame pipeline over buffers retained by TerminalView.
//! Private stages borrow the same cells; they do not own a second frame lifecycle.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    App, Bounds, ContentMask, Corners, CursorStyle, Element, ElementId, GlobalElementId, Hitbox,
    HitboxBehavior, HitboxId, Hsla, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Rgba, SharedString, StrikethroughStyle, Style,
    TextAlign, TextRun, Window, fill, point, px, relative, size,
};
use rio_vt::config::colors::ColorRgb;

use super::link::should_show_context_menu;
use super::view::TerminalView;
use crate::core::config::Config;
use crate::ui::i18n::{L10nKey, t};

mod colors;
mod decorations;
mod osc8_underline;
mod snapshot;
mod text;

#[cfg(test)]
mod test_support;

use colors::dim_under;
use decorations::{
    invert_cursor_cell, paint_backgrounds, paint_cell_runs, paint_cursor, paint_marked,
    paint_special_underlines,
};
use text::paint_glyphs;

const DIM_OPACITY: f32 = 0.66;

/// How much of the text's own colour a link's underline keeps before the
/// modifier is down. Low enough to read as a hint rather than as markup, high
/// enough to survive a light theme.
const HELD_BACK_LINK_ALPHA: f32 = 0.45;

#[derive(Clone, Copy, PartialEq, Default, Debug)]
enum UnderlineKind {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Clone)]
pub(super) struct RenderCell {
    c: char,
    marks: Option<Box<[char]>>,
    fg: Hsla,
    bg: Hsla,
    draw_bg: bool,
    bold: bool,
    italic: bool,
    strikeout: bool,
    underline: UnderlineKind,
    underline_color: Option<Hsla>,
    spacer: bool,
    selected: bool,
    match_hit: bool,
    match_current: bool,
    link_hover: bool,
    osc8_dots: bool,
}

impl Default for RenderCell {
    fn default() -> Self {
        Self {
            c: ' ',
            marks: None,
            fg: Hsla::default(),
            bg: Hsla::default(),
            draw_bg: false,
            bold: false,
            italic: false,
            strikeout: false,
            underline: UnderlineKind::None,
            underline_color: None,
            spacer: false,
            selected: false,
            match_hit: false,
            match_current: false,
            link_hover: false,
            osc8_dots: false,
        }
    }
}

pub struct TerminalElement {
    view: gpui_kit::Entity<TerminalView>,
}

impl TerminalElement {
    pub fn new(view: gpui_kit::Entity<TerminalView>) -> Self {
        Self { view }
    }
}

pub struct TermLayout {
    cell_width: Pixels,
    line_height: Pixels,
    cols: usize,
    rows: usize,
    hitbox: Hitbox,
}

pub(super) struct PaintColors {
    default_fg: Hsla,
    default_bg: Hsla,
    caret: Hsla,
    selection_bg: Hsla,
    match_bg: Hsla,
    current_match_bg: Hsla,
    fg_rgb: ColorRgb,
    bg_rgb: ColorRgb,
    /// Whether faint text on a light cell is held at the text floor (see
    /// `dim_fg`). Mirrors `theme_legible_palette`.
    legible_dim: bool,
}

#[derive(Clone, Copy, PartialEq)]
struct GlyphStyle {
    fg: Hsla,
    bold: bool,
    italic: bool,
    strikeout: bool,
    underline: UnderlineKind,
    underline_color: Option<Hsla>,
    link_hover: bool,
}

impl GlyphStyle {
    fn of(cell: &RenderCell) -> Self {
        Self {
            fg: cell.fg,
            bold: cell.bold,
            italic: cell.italic,
            strikeout: cell.strikeout,
            underline: cell.underline,
            underline_color: cell.underline_color,
            link_hover: cell.link_hover,
        }
    }

    fn draws_on_blanks(&self) -> bool {
        self.underline != UnderlineKind::None || self.strikeout || self.link_hover
    }

    fn underline_style(&self) -> Option<gpui_kit::UnderlineStyle> {
        (matches!(self.underline, UnderlineKind::Single | UnderlineKind::Curly)
            || (self.link_hover && self.underline == UnderlineKind::None))
            .then(|| {
                let wavy = self.underline == UnderlineKind::Curly;
                gpui_kit::UnderlineStyle {
                    thickness: px(1.),
                    color: Some(self.underline_color.unwrap_or(self.fg)),
                    wavy,
                }
            })
    }

    fn strikethrough_style(&self) -> Option<StrikethroughStyle> {
        self.strikeout.then_some(StrikethroughStyle {
            thickness: px(1.),
            color: Some(self.fg),
        })
    }
}

fn is_blank(cell: &RenderCell) -> bool {
    (cell.c == '\0' || cell.c == ' ') && cell.marks.is_none()
}

#[derive(Clone, Copy)]
pub(super) struct GridCursor {
    row: usize,
    col: usize,
    // Where the IME candidate window should anchor: the fake caret drawn by
    // cursor-hiding TUIs when one is identifiable, else `col`.
    ime_col: usize,
    hidden: bool,
    style: crate::core::config::CursorStyle,
}

#[derive(Clone)]
pub(super) struct GridSnapshot {
    cursor: Option<GridCursor>,
    sliver: Option<Vec<RenderCell>>,
    any_selected: bool,
    any_match: bool,
    any_current: bool,
    /// Scrollback state at snapshot time, so the paint pass can map a kitty
    /// image's absolute anchor row back to a screen row: screen_row =
    /// anchor_row - history_size + display_offset.
    display_offset: i32,
    history_size: usize,
    placeholders: Vec<(i32, Vec<lyrel_core::core::kitty_graphics::PlaceholderSlice>)>,
}

impl GridSnapshot {
    /// The terminal caret this frame paints, if any. None while the program
    /// has the cursor hidden: the emulator reports a DECTCEM-reset cursor
    /// (`?25l`) as `CursorShape::Hidden`, and a TUI that hides it is usually
    /// drawing a caret of its own that ours must not cover (#844).
    pub(super) fn painted_cursor(
        &self,
    ) -> Option<(usize, usize, crate::core::config::CursorStyle)> {
        self.cursor
            .filter(|c| !c.hidden)
            .map(|c| (c.row, c.col, c.style))
    }
}

impl TerminalElement {
    fn register_mouse_handlers(
        &self,
        geom: CellGeom,
        bounds: Bounds<Pixels>,
        hitbox: HitboxId,
        window: &mut Window,
    ) {
        let view = self.view.clone();
        window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
            if !phase.bubble() || !hitbox.is_hovered(window) {
                return;
            }
            let (col, row, left) = geom.pos_to_cell(ev.position);
            let raw_row = geom.pos_to_row_raw(ev.position);
            let mods = ev.modifiers;
            let button = ev.button;
            let clicks = ev.click_count;
            view.update(cx, |v, cx| {
                if button == MouseButton::Right {
                    // Before anything else: gpui-component builds the popup
                    // from a deferred callback, and by then the pointer is
                    // only a memory. Reading the grid now is what lets the
                    // menu name the file that was actually under the click.
                    match should_show_context_menu(v.mouse_mode(), mods.shift) {
                        true => v.record_menu_link(col, row, cx),
                        false => v.forget_menu_link(),
                    }
                }
                let link_modifier = mods.secondary(cx) || v.link_modifier_down();
                if link_modifier
                    && button == MouseButton::Left
                    && v.open_link_at(col, row, window, cx)
                {
                    return;
                }
                // The mirror image of the context-menu gate in
                // `TerminalView::render`: a click the application gets is never
                // also a click lyrel acts on, and vice versa.
                if !should_show_context_menu(v.mouse_mode(), mods.shift) {
                    v.mouse_press(button, col, row, &mods);
                    return;
                }
                if button == MouseButton::Left
                    && v.editor_click(col, raw_row, clicks, mods.shift, cx)
                {
                    return;
                }
                if button == MouseButton::Left {
                    v.on_select_start(col, row, left, clicks, mods.shift, cx);
                }
            });
        });

        let view = self.view.clone();
        window.on_mouse_event(move |ev: &MouseMoveEvent, _phase, window, cx| {
            let (col, row, left) = geom.pos_to_cell(ev.position);
            let raw_row = geom.pos_to_row_raw(ev.position);
            let mods = ev.modifiers;
            let Some(button) = ev.pressed_button else {
                let inside = hitbox.is_hovered(window);
                if inside && cx.global::<Config>().focus_follows_mouse {
                    let handle = view.read(cx).focus_handle.clone();
                    if !handle.is_focused(window) {
                        window.focus(&handle, cx);
                    }
                }
                view.update(cx, |v, cx| {
                    if inside {
                        if !mods.shift {
                            v.mouse_motion(col, row, &mods);
                        }
                        let armed = mods.secondary(cx) || v.link_modifier_down();
                        v.hover_link_at(col, row, armed, cx);
                    } else {
                        v.clear_hovered_link(cx);
                    }
                });
                return;
            };
            view.update(cx, |v, cx| {
                if v.mouse_mode() && !mods.shift {
                    v.mouse_drag(button, col, row, &mods);
                    return;
                }
                if button == MouseButton::Left && v.editor_drag(col, raw_row, cx) {
                    return;
                }
                if button == MouseButton::Left {
                    v.on_select_update(col, row, left, cx);
                    let overshoot = drag_overshoot(ev.position.y, bounds, geom.line_height);
                    v.select_autoscroll(overshoot, col, left, cx);
                }
            });
        });

        let view = self.view.clone();
        window.on_mouse_event(move |ev: &MouseUpEvent, phase, _window, cx| {
            if !phase.bubble() {
                return;
            }
            let (col, row, _left) = geom.pos_to_cell(ev.position);
            let mods = ev.modifiers;
            let button = ev.button;
            view.update(cx, |v, cx| {
                if v.mouse_mode() && !mods.shift {
                    v.mouse_release(button, col, row, &mods);
                    return;
                }
                v.on_select_end(cx);
            });
        });
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = TermLayout;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        style.flex_grow = 1.0;
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let font_size = self.view.read(cx).font_size;
        let base_font = self.view.read(cx).font.clone();

        let sample = window.text_system().shape_line(
            SharedString::new_static("M"),
            font_size,
            &[TextRun {
                len: 1,
                font: base_font.clone(),
                color: Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        let cell_width = sample.width.max(px(1.));
        let line_height_mul = self.view.read(cx).line_height_mul;
        let line_height = px((font_size.as_f32() * line_height_mul).round()).max(px(1.));

        let cols = (bounds.size.width.as_f32() / cell_width.as_f32())
            .floor()
            .max(1.0) as usize;
        let fits = (bounds.size.height.as_f32() / line_height.as_f32())
            .floor()
            .max(1.0) as usize;
        // The composer covering an input shorter than itself takes rows off
        // the bottom (see `composer_reserved_rows`); they stay part of the
        // pane's leftover space as far as the layer laid over it goes.
        let reserved = self.view.read(cx).composer_reserved_rows();
        let rows = fits.saturating_sub(reserved).max(1);

        self.view.update(cx, |view, cx| {
            view.scroll_handle.set_viewport(bounds);
            view.set_grid_size(
                cols,
                rows,
                cell_width,
                line_height,
                window.scale_factor(),
                cx,
            );
            // Whatever lays itself over the bottom rows (the composer)
            // measures up from the bottom, past this.
            let slack = bounds.size.height - line_height * fits as f32;
            if view.grid_slack != slack {
                view.grid_slack = slack;
                cx.notify();
            }
        });

        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);

        TermLayout {
            cell_width,
            line_height,
            cols,
            rows,
            hitbox,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let fps_start = super::fps::enabled().then(std::time::Instant::now);

        let frac = self.view.read(cx).selection_scroll.fraction().clamp(0., 1.);
        let input_shift = self.view.read(cx).input_scroll_rows();
        let geom = CellGeom {
            origin: point(
                bounds.origin.x,
                bounds.origin.y + prepaint.line_height * (frac - input_shift as f32),
            ),
            cell_width: prepaint.cell_width,
            line_height: prepaint.line_height,
            cols: prepaint.cols,
            rows: prepaint.rows,
        };
        let colors = PaintColors::resolve(cx.theme(), cx);

        let font_size = self.view.read(cx).font_size;
        let base_font = self.view.read(cx).font.clone();
        let bold_font = self.view.read(cx).font_bold.clone();
        let italic_font = self.view.read(cx).font_italic.clone();
        let focused = self.view.read(cx).focus_handle.is_focused(window);
        let cursor_visible = self.view.read(cx).cursor_visible();
        let bell_flash = self.view.read(cx).bell_flash;
        let editor_active = self.view.read(cx).input_active();
        // The pane leaf stores its per-frame dim here (see `TerminalView::dim`).
        // Blending the palette and paint colours toward the window background is
        // what actually dims the pane; the pane no longer wraps the terminal in
        // an element-opacity style, whose per-primitive alpha multiplication
        // would leave stacked decorations with a seam against the segments
        // below them.
        let dim = self.view.read(cx).dim.clamp(0., 1.);
        let (colors, dim, under) = if dim < 1. {
            let under = dim_under(cx);
            (colors.dimmed(dim, under), dim, under)
        } else {
            // `under` is only read while `dim < 1.` (per-cell in the grid and
            // over bitmaps), so a default keeps the common rest frame from
            // paying for the preset lookup and colour math.
            (colors, 1., Rgba::default())
        };

        // This pane's previous frame, borrowed for the duration of this one.
        // `build_grid` overwrites it when it gets the terminal lock, and leaves
        // it exactly as it is when it does not.
        let mut buf = self
            .view
            .update(cx, |view, _| std::mem::take(&mut view.grid_buf));
        let previous = self.view.read(cx).grid_snap.clone();
        // The two frames with nothing to fall back on: the first one this pane
        // ever paints, and the one after a resize, whose previous grid is the
        // wrong shape to paint into these bounds. Those wait for the lock.
        let must_block = previous.is_none() || buf.len() != geom.rows * geom.cols;
        let built = self.build_grid(
            &colors,
            &mut buf,
            geom.rows,
            geom.cols,
            frac > 0.,
            cx,
            dim,
            under,
            must_block,
        );
        if let Some(snap) = &built {
            let snap = snap.clone();
            self.view.update(cx, |view, _| view.grid_snap = Some(snap));
        }
        // `must_block` above is exactly the condition under which `build_grid`
        // is not allowed to come back empty, so one of the two is always here.
        let Some(snap) = built.or(previous) else {
            return;
        };
        let cursor = snap.cursor;
        let sliver = snap.sliver.as_ref();

        let cursor_cell = cursor.map(|c| (c.row, c.ime_col));
        let render_cursor = snap.painted_cursor();

        // Reverse-video the block cursor's cell up front, so it rides the
        // normal background-then-glyph path instead of being tinted on top of
        // the finished frame. `paint_cursor` skips the focused block for the
        // same reason.
        if !editor_active
            && focused
            && cursor_visible
            && let Some((row, col, crate::core::config::CursorStyle::Block)) = render_cursor
        {
            invert_cursor_cell(&mut buf, geom.cols, row, col, &colors);
        }

        // With the input bar up, the IME composes into the bar, which starts a
        // row below the prompt when the prompt left it too little room.
        let ime_cell = cursor.map(|c| {
            self.view
                .read(cx)
                .input_ime_cell(c.row, c.col, geom.cols)
                .unwrap_or((c.row, c.ime_col))
        });
        let cursor_bounds = ime_cell.map(|(row, col)| geom.cell_rect(row, col, 1));
        // Unfocused, the anchor is forgotten so the next focus pushes it again.
        let ime_anchor = cursor_bounds.filter(|_| focused);
        if self
            .view
            .update(cx, |view, _| view.ime_anchor_moved(ime_anchor))
        {
            window.invalidate_character_coordinates();
        }
        let focus_handle = self.view.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            super::view::input::TerminalInputHandler::new(self.view.clone(), cursor_bounds),
            cx,
        );
        let marked = self.view.read(cx).input.marked().to_owned();

        // Kitty-graphics placements for this pane. Each image anchors to an
        // absolute scrollback row (recorded when its command arrived in-stream);
        // map that back to a screen row with the snapshot's scroll state and
        // drop anything scrolled out of the viewport.
        let image_store = self.view.read(cx).terminal.images();
        let displayed = self.view.read(cx).lifecycle.paint_visibility().clone();
        let images = image_store.snapshot_with_placeholders(&snap.placeholders);
        let notice = images.iter().any(|image| image.evicted).then(|| {
            let text = SharedString::new_static(t(L10nKey::TerminalImageEvicted));
            window.text_system().shape_line(
                text.clone(),
                font_size,
                &[TextRun {
                    len: text.len(),
                    font: base_font.clone(),
                    color: cx.theme().muted_foreground,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            )
        });
        let paint_images = |window: &mut Window, cx: &mut App, z: std::ops::Range<i64>| {
            // Separate crop/full-texture bounds select atlas UVs; no placement
            // owns a copied pixel buffer. The enclosing pane mask still applies.
            for img in images
                .iter()
                .filter(|image| z.contains(&i64::from(image.placement.z)))
            {
                let (bounds, image_bounds) = img.paint_bounds(
                    geom.origin,
                    geom.cell_width,
                    geom.line_height,
                    snap.history_size,
                    snap.display_offset,
                    window.scale_factor(),
                );
                let visible = bounds.intersect(&window.content_mask().bounds);
                if visible.size.width <= px(0.)
                    || visible.size.height <= px(0.)
                    || !image_store.claim_for_paint(img)
                {
                    continue;
                }
                image_store.register_for_paint(img, window, &displayed);
                window.paint_layer(visible, |window| {
                    if let Some(data) = &img.data {
                        let _ = window.paint_image(
                            bounds,
                            image_bounds,
                            Corners::default(),
                            data.clone(),
                            0,
                            false,
                        );
                    } else if let Some(notice) = notice.as_ref().filter(|_| img.evicted) {
                        window.with_content_mask(Some(ContentMask { bounds: visible }), |window| {
                            let _ = notice.paint(
                                visible.origin,
                                geom.line_height,
                                TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                        });
                    }
                    if dim < 1. {
                        let mut c: Hsla = under.into();
                        c.a = 1. - dim;
                        window.paint_quad(fill(bounds, c));
                    }
                });
            }
        };
        let below_background = -(1_i64 << 30);
        let sliver = sliver.map(|row| {
            (
                CellGeom {
                    origin: point(geom.origin.x, geom.origin.y - geom.line_height),
                    rows: 1,
                    ..geom
                },
                row,
            )
        });

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            paint_images(window, cx, i64::from(i32::MIN)..below_background);
            paint_backgrounds(window, &geom, &buf);
            if snap.any_selected {
                paint_cell_runs(window, &geom, &buf, colors.selection_bg, |c| c.selected);
            }
            if snap.any_match {
                paint_cell_runs(window, &geom, &buf, colors.match_bg, |c| {
                    c.match_hit && !c.match_current
                });
            }
            if snap.any_current {
                // Fill only. The current match used to carry a caret-coloured
                // outline as well, because a wash it could only be 2.1:1 off the
                // background had no way to say "this one" on its own. Now that
                // it is the accent at the top of the theme's budget, the outline
                // is the same colour saying the same thing a second time.
                paint_cell_runs(window, &geom, &buf, colors.current_match_bg, |c| {
                    c.match_current
                });
            }
            if let Some((sg, row)) = &sliver {
                paint_backgrounds(window, sg, row);
                if snap.any_selected {
                    paint_cell_runs(window, sg, row, colors.selection_bg, |c| c.selected);
                }
            }
            paint_images(window, cx, below_background..0);
            paint_glyphs(
                window,
                cx,
                &geom,
                &buf,
                font_size,
                &base_font,
                bold_font.as_ref(),
                italic_font.as_ref(),
            );
            let scale = window.scale_factor();
            paint_special_underlines(window, &geom, &buf, scale);
            if let Some((sg, row)) = &sliver {
                paint_glyphs(
                    window,
                    cx,
                    sg,
                    row,
                    font_size,
                    &base_font,
                    bold_font.as_ref(),
                    italic_font.as_ref(),
                );
                paint_special_underlines(window, sg, row, scale);
            }
            paint_images(window, cx, 0..i64::from(i32::MAX) + 1);
            if !editor_active {
                paint_cursor(
                    window,
                    &geom,
                    render_cursor,
                    focused,
                    cursor_visible,
                    colors.caret,
                );
                paint_marked(
                    window,
                    cx,
                    &geom,
                    cursor_cell,
                    &marked,
                    font_size,
                    &base_font,
                    colors.default_fg,
                    colors.default_bg,
                );
            }

            if bell_flash {
                let mut c = colors.default_fg;
                c.a = 0.12;
                window.paint_quad(fill(bounds, c));
            }
        });

        self.view.update(cx, |view, _| {
            view.grid_buf = buf;
            view.cell_geom = Some(geom);
        });

        self.register_mouse_handlers(geom, bounds, prepaint.hitbox.id, window);

        let view = self.view.read(cx);
        if view.links.hovered().is_some_and(|link| link.is_armed()) {
            window.set_cursor_style(CursorStyle::PointingHand, &prepaint.hitbox);
        } else if !view.mouse_mode() {
            window.set_cursor_style(CursorStyle::IBeam, &prepaint.hitbox);
        }

        if let Some(start) = fps_start {
            super::fps::record(start.elapsed());
        }

        // The dim is a per-frame hand-off from the pane leaf to this paint
        // (see `TerminalView::set_dim`): the pane writes it while rendering,
        // this paint reads it above. Reset it here so the hand-off is
        // structural — any frame that ends without a render site having set
        // the dim paints at full brightness next, instead of carrying a stale
        // value. A render site that forgets to set it (a maximized pane has
        // no chrome to set it) therefore degrades to a single stale frame at
        // worst, never a permanently dimmed terminal.
        self.view.update(cx, |v, _cx| v.set_dim(1.));
    }
}

#[derive(Clone, Copy)]
pub(super) struct CellGeom {
    pub(super) origin: Point<Pixels>,
    pub(super) cell_width: Pixels,
    pub(super) line_height: Pixels,
    pub(super) cols: usize,
    pub(super) rows: usize,
}

impl CellGeom {
    fn cell_rect(&self, row: usize, col: usize, span: usize) -> Bounds<Pixels> {
        let x = self.origin.x + self.cell_width * (col as f32);
        let y = self.origin.y + self.line_height * (row as f32);
        Bounds::new(
            point(x, y),
            size(self.cell_width * (span as f32), self.line_height),
        )
    }

    pub(super) fn pos_to_cell(&self, pos: Point<Pixels>) -> (usize, usize, bool) {
        let lx = (pos.x - self.origin.x).as_f32().max(0.);
        let ly = (pos.y - self.origin.y).as_f32().max(0.);
        let colf = lx / self.cell_width.as_f32();
        let col = (colf.floor() as usize).min(self.cols.saturating_sub(1));
        let row =
            ((ly / self.line_height.as_f32()).floor() as usize).min(self.rows.saturating_sub(1));
        let left = (colf - colf.floor()) <= 0.5;
        (col, row, left)
    }

    fn pos_to_row_raw(&self, pos: Point<Pixels>) -> usize {
        let ly = (pos.y - self.origin.y).as_f32().max(0.);
        (ly / self.line_height.as_f32()).floor() as usize
    }
}

fn drag_overshoot(y: Pixels, bounds: Bounds<Pixels>, line_height: Pixels) -> f32 {
    let lh = line_height.as_f32().max(1.);
    if y < bounds.top() {
        (bounds.top() - y).as_f32() / lh
    } else if y > bounds.bottom() {
        -((y - bounds.bottom()).as_f32() / lh)
    } else {
        0.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_rect_maps_grid_to_pixels() {
        let geom = CellGeom {
            origin: point(px(10.), px(20.)),
            cell_width: px(8.),
            line_height: px(16.),
            cols: 80,
            rows: 24,
        };
        let r = geom.cell_rect(2, 3, 1);
        assert_eq!(r.origin.x, px(10. + 8. * 3.));
        assert_eq!(r.origin.y, px(20. + 16. * 2.));
        assert_eq!(r.size.width, px(8.));
        assert_eq!(r.size.height, px(16.));
        assert_eq!(geom.cell_rect(0, 0, 4).size.width, px(32.));
    }

    #[test]
    fn pos_to_cell_clamps_and_detects_halves() {
        let geom = CellGeom {
            origin: point(px(0.), px(0.)),
            cell_width: px(10.),
            line_height: px(20.),
            cols: 5,
            rows: 3,
        };
        let (c, r, left) = geom.pos_to_cell(point(px(2.), px(5.)));
        assert_eq!((c, r), (0, 0));
        assert!(left);
        let (_, _, left) = geom.pos_to_cell(point(px(8.), px(5.)));
        assert!(!left);
        let (c, r, _) = geom.pos_to_cell(point(px(-100.), px(-100.)));
        assert_eq!((c, r), (0, 0));
        let (c, r, _) = geom.pos_to_cell(point(px(9999.), px(9999.)));
        assert_eq!((c, r), (4, 2));
    }

    #[test]
    fn drag_overshoot_signed_by_edge_and_zero_inside() {
        let bounds = Bounds::new(point(px(0.), px(100.)), size(px(200.), px(100.)));
        assert_eq!(drag_overshoot(px(150.), bounds, px(10.)), 0.);
        assert_eq!(drag_overshoot(px(100.), bounds, px(10.)), 0.);
        assert_eq!(drag_overshoot(px(200.), bounds, px(10.)), 0.);
        assert_eq!(drag_overshoot(px(80.), bounds, px(10.)), 2.);
        assert_eq!(drag_overshoot(px(230.), bounds, px(10.)), -3.);
    }
}
