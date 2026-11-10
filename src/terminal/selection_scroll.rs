use std::time::Instant;

use gpui_kit::{Modifiers, OperatingSystem, ScrollDelta, ScrollEvent, TouchPhase};
use rio_vt::crosswords::Crosswords;
use rio_vt::crosswords::grid::{Dimensions, Scroll};
use rio_vt::crosswords::pos::{Column, Line, Pos, Side};
use rio_vt::selection::{Selection, SelectionType};

use super::remote::EventProxy;
use super::remote::RemoteTerminal;
use super::smart_select::GridWordSelection;
use crate::core::config::{Config, MouseZoomModifier};

type Grid = Crosswords<EventProxy>;

/// Selection and viewport interaction. Reading owns decoded semantic rows,
/// while scheduled ticks still request the existing live guard lazily.
#[derive(Default)]
pub(super) struct SelectionScroll {
    pub(super) reading: Option<super::reading::ReadingView>,
    selecting: bool,
    word: Option<GridWordSelection>,
    drag: Option<DragScroll>,
    drag_epoch: u64,
    animation: Option<ScrollAnim>,
    animation_epoch: u64,
    gesture_until: Option<Instant>,
    gesture_zoom: Option<bool>,
    /// Kept apart: switching gestures must never turn scroll travel into zoom.
    scroll_debt: f32,
    zoom_debt: f32,
    fraction: f32,
    presented: Option<super::presentation::PresentedSelection>,
    gesture: Option<(usize, usize, bool, usize)>,
    live_anchor: Option<std::ops::Range<usize>>,
}

#[derive(Clone, Copy)]
struct DragScroll {
    overshoot: f32,
    col: usize,
    side: Side,
}

/// Relative travel, not an absolute target: incoming output moves the grid
/// without dragging an animation back toward an obsolete viewport position.
#[derive(Clone, Copy)]
struct ScrollAnim {
    remaining: f32,
    last: Instant,
}

pub(super) enum WheelMotion {
    Zoom(i32),
    Quantized(i32),
    Animate(f32),
    Direct(f32),
}

impl SelectionScroll {
    /// Lifecycle transitions outrank resize relocation and atomic holds. The
    /// generation comes from the same VT guard, not a final-mode observation.
    pub(super) fn invalidate_reading(&mut self, term: &mut Grid, terminal: &RemoteTerminal) {
        let invalid = self.reading.as_ref().is_some_and(|reading| {
            term.mode().contains(rio_vt::crosswords::Mode::ALT_SCREEN)
                || !reading.belongs_to(terminal.presentation_facts().reading_generation)
        });
        if invalid {
            self.clear_presented();
            self.jump_to_prompt(term);
        }
    }

    pub(super) fn sync_reading(&mut self, term: &mut Grid, terminal: &RemoteTerminal) {
        self.invalidate_reading(term, terminal);
        let relocated = self
            .reading
            .as_mut()
            .is_some_and(|reading| reading.finish_resize(term, terminal));
        if relocated {
            self.clear_presented();
        }
        if self
            .reading
            .as_mut()
            .is_some_and(|reading| !reading.reconcile(term))
        {
            self.clear_presented();
            self.jump_to_prompt(term);
        } else if term.display_offset() == 0
            && self.fraction == 0.
            && !self.reading.as_ref().is_some_and(|reading| reading.relocating())
            && self.reading.take().is_some()
        {
            self.clear_presented();
        }
        if let Some(reading) = self.reading.as_mut() {
            reading.maintain(
                term,
                terminal,
                self.presented.as_ref().map(|selection| selection.range()),
            );
        }
    }

    pub(super) fn did_scroll(&mut self, term: &Grid, terminal: &RemoteTerminal) {
        if term.synchronized_output() {
            return;
        }
        if self
            .reading
            .as_mut()
            .is_some_and(|reading| reading.relocation_scroll(term))
        {
            return;
        }
        if term.display_offset() == 0 && self.fraction == 0. {
            if self.reading.take().is_some() {
                self.clear_presented();
            }
        } else if let Some(reading) = &mut self.reading {
            reading.maintain(
                term,
                terminal,
                self.presented.as_ref().map(|selection| selection.range()),
            );
        } else if !term.mode().contains(rio_vt::crosswords::Mode::ALT_SCREEN) {
            self.reading = Some(super::reading::ReadingView::capture(
                term,
                &terminal.images(),
                &terminal.presentation_facts(),
            ));
            self.adopt_live_selection(term);
        }
    }

    fn absolute_selection(term: &Grid) -> Option<std::ops::Range<usize>> {
        let range = term.selection.as_ref()?.to_range(term)?;
        let origin = super::reading::live_start(term);
        Some(
            (origin + i64::from(range.start.row.0)) as usize * term.columns() + range.start.col.0
                ..(origin + i64::from(range.end.row.0)) as usize * term.columns()
                    + range.end.col.0
                    + 1,
        )
    }

    fn adopt_live_selection(&mut self, term: &Grid) {
        let current = Self::absolute_selection(term);
        let anchor = if self.selecting {
            self.live_anchor.clone().or_else(|| current.clone())
        } else {
            current.clone()
        };
        if let Some(anchor) = anchor {
            let clicks = self.gesture.map_or(1, |gesture| gesture.3);
            let mut selection =
                super::presentation::PresentedSelection::new(anchor.clone(), clicks);
            if let Some(current) = current {
                let at = if current.start < anchor.start {
                    current.start
                } else {
                    current.end
                };
                selection.update(at..at);
            }
            self.presented = Some(selection);
        }
    }

    fn selection_origin(&self, cols: usize) -> usize {
        self.reading
            .as_ref()
            .map_or(0, |reading| reading.top().max(0) as usize * cols)
    }

    pub(super) fn apply_reading_selection(
        &self,
        cells: &mut [super::element::RenderCell],
        cols: usize,
    ) -> bool {
        self.presented
            .as_ref()
            .is_some_and(|selection| selection.apply_from(cells, self.selection_origin(cols)))
    }

    pub(super) fn apply_reading_sliver_selection(
        &self,
        cells: &mut [super::element::RenderCell],
        cols: usize,
    ) -> bool {
        let origin = self.reading.as_ref().map_or(0, |reading| {
            reading.top().saturating_sub(1).max(0) as usize * cols
        });
        self.presented
            .as_ref()
            .is_some_and(|selection| selection.apply_from(cells, origin))
    }

    pub(super) fn reading_selection_text(&self) -> Option<String> {
        self.reading
            .as_ref()?
            .selected_text(self.presented.as_ref()?)
    }

    pub(super) fn reading_selection_is_lines(&self) -> bool {
        self.presented
            .as_ref()
            .is_some_and(|selection| selection.clicks() >= 3)
    }

    pub(super) fn has_presented(&self) -> bool {
        self.presented.is_some()
    }

    pub(super) fn clear_presented(&mut self) {
        if self.presented.take().is_some() {
            self.live_anchor = None;
            self.selecting = false;
            self.drag = None;
        }
    }

    pub(super) fn adopt_presented(
        &mut self,
        cells: &[super::element::RenderCell],
        cols: usize,
        cfg: &Config,
    ) {
        if self.presented.is_some() {
            return;
        }
        let (col, row, left, clicks) = self.gesture.unwrap_or((0, 0, true, 1));
        let grid = super::presentation::PresentedGrid::new(cells, cols, 0);
        let range = match (
            cells.iter().position(|cell| cell.selected),
            cells.iter().rposition(|cell| cell.selected),
        ) {
            (Some(start), Some(end)) => start..end + 1,
            _ if clicks > 1 => grid.click_range(col, row, clicks, cfg),
            _ => {
                let at = grid.boundary(col, row, left);
                at..at
            }
        };
        let origin = self.selection_origin(cols);
        self.presented = Some(super::presentation::PresentedSelection::new(
            range.start + origin..range.end + origin,
            clicks,
        ));
    }

    pub(super) fn start_presented(
        &mut self,
        cells: &mut [super::element::RenderCell],
        cols: usize,
        col: usize,
        row: usize,
        left: bool,
        clicks: usize,
        shift: bool,
        cfg: &Config,
    ) -> bool {
        use super::presentation::{PresentedGrid, PresentedSelection};
        let grid = if let Some(reading) = self.reading.as_ref() {
            PresentedGrid::reading(reading)
        } else {
            PresentedGrid::new(cells, cols, 0)
        };
        let target = if clicks > 1 {
            grid.click_range(col, row, clicks, cfg)
        } else {
            let at = grid.boundary(col, row, left);
            at..at
        };
        let origin = grid.absolute_origin();
        let target = target.start + origin..target.end + origin;
        if shift {
            self.adopt_presented(cells, cols, cfg);
        }
        self.gesture = Some((col, row, left, clicks));
        if shift
            && clicks == 1
            && let Some(selection) = self.presented.as_mut()
        {
            selection.update(target);
        } else {
            self.presented = Some(PresentedSelection::new(target, clicks));
        }
        self.selecting = true;
        self.word = None;
        self.drag = None;
        self.cancel_animation();
        self.apply_reading_selection(cells, cols)
    }

    pub(super) fn update_presented(
        &mut self,
        cells: &mut [super::element::RenderCell],
        cols: usize,
        col: usize,
        row: usize,
        left: bool,
        cfg: &Config,
    ) -> bool {
        let origin = self.selection_origin(cols);
        let Some(selection) = self.presented.as_mut() else {
            return false;
        };
        let grid = if let Some(reading) = self.reading.as_ref() {
            super::presentation::PresentedGrid::reading(reading)
        } else {
            super::presentation::PresentedGrid::new(cells, cols, 0)
        };
        let target_origin = grid.absolute_origin();
        let target = if selection.clicks() > 1 {
            grid.click_range(col, row, selection.clicks(), cfg)
        } else {
            let at = grid.boundary(col, row, left);
            at..at
        };
        selection.update(target.start + target_origin..target.end + target_origin);
        selection.apply_from(cells, origin)
    }

    pub(super) fn select_all_presented(
        &mut self,
        cells: &mut [super::element::RenderCell],
    ) -> bool {
        let cols = self.reading.as_ref().map_or(1, |reading| reading.columns());
        let origin = self.selection_origin(cols);
        let selection =
            super::presentation::PresentedSelection::new(origin..origin + cells.len(), 1);
        let selected = selection.apply_from(cells, origin);
        self.presented = Some(selection);
        selected
    }

    pub(super) fn fraction(&self) -> f32 {
        self.fraction
    }

    pub(super) fn is_selecting(&self) -> bool {
        self.selecting
    }

    pub(super) fn is_animating(&self) -> bool {
        self.animation.is_some()
    }

    pub(super) fn start(
        &mut self,
        term: &mut Grid,
        col: usize,
        row: usize,
        left: bool,
        clicks: usize,
        shift: bool,
        cfg: &Config,
    ) {
        self.gesture = Some((col, row, left, clicks));
        let point = Pos::new(
            Line(row as i32 - term.grid.display_offset() as i32),
            Column(col),
        );
        let side = if left { Side::Left } else { Side::Right };
        let extend_existing = shift && clicks == 1 && term.selection.is_some();
        if extend_existing {
            self.extend(term, point, side, &cfg.word_separators);
        } else {
            self.word = None;
            if clicks == 2 {
                self.word = Some(GridWordSelection::start(
                    term,
                    point,
                    &cfg.word_separators,
                    cfg.smart_select,
                ));
            } else {
                let ty = if clicks >= 3 {
                    SelectionType::Lines
                } else {
                    SelectionType::Simple
                };
                term.selection = Some(Selection::new(ty, point, side));
            }
        }
        if !(extend_existing && self.live_anchor.is_some()) {
            let absolute_row = (super::reading::live_start(term) + i64::from(point.row.0)) as usize;
            let wide = term.grid[point].wide();
            let col = if wide == rio_vt::crosswords::square::Wide::Spacer {
                col.saturating_sub(1)
            } else {
                col
            };
            let at = absolute_row * term.columns()
                + col
                + if left {
                    0
                } else if matches!(
                    wide,
                    rio_vt::crosswords::square::Wide::Wide
                        | rio_vt::crosswords::square::Wide::Spacer
                ) {
                    2
                } else {
                    1
                };
            self.live_anchor = Some(Self::absolute_selection(term).unwrap_or(at..at));
        }
        self.selecting = true;
    }

    pub(super) fn update(
        &mut self,
        term: &mut Grid,
        col: usize,
        row: usize,
        left: bool,
        separators: &str,
    ) {
        let point = Pos::new(
            Line(row as i32 - term.grid.display_offset() as i32),
            Column(col),
        );
        self.extend(
            term,
            point,
            if left { Side::Left } else { Side::Right },
            separators,
        );
    }

    fn extend(&mut self, term: &mut Grid, point: Pos, side: Side, separators: &str) {
        if let Some(word) = self.word.as_mut() {
            word.update(term, point, separators);
        } else if let Some(selection) = term.selection.as_mut() {
            selection.update(point, side);
        }
    }

    pub(super) fn select_all(&mut self, term: &mut Grid) {
        let grid = &term.grid;
        let start = Pos::new(grid.topmost_line(), Column(0));
        let end = Pos::new(grid.bottommost_line(), grid.last_column());
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.update(end, Side::Right);
        term.selection = Some(selection);
        self.word = None;
    }

    pub(super) fn end(&mut self, copy: bool, editor_gesture: bool) -> SelectEndCopy {
        let result = select_end_copy(copy, self.selecting, editor_gesture);
        self.selecting = false;
        self.drag = None;
        result
    }

    /// Only a transition out of idle starts a timer. Updating overshoot leaves
    /// the existing loop in charge, and the epoch fences a retired loop.
    pub(super) fn autoscroll(&mut self, overshoot: f32, col: usize, left: bool) -> Option<u64> {
        if !self.selecting || overshoot == 0. {
            self.drag = None;
            return None;
        }
        let idle = self.drag.is_none();
        self.drag = Some(DragScroll {
            overshoot,
            col,
            side: if left { Side::Left } else { Side::Right },
        });
        if !idle {
            return None;
        }
        self.drag_epoch += 1;
        Some(self.drag_epoch)
    }

    pub(super) fn drag_tick<G: std::ops::DerefMut<Target = Grid>>(
        &mut self,
        epoch: u64,
        grid: impl FnOnce() -> G,
        cfg: &Config,
        terminal: &RemoteTerminal,
    ) -> Option<bool> {
        if epoch != self.drag_epoch {
            return None;
        }
        if !self.selecting {
            self.drag = None;
        }
        let drag = self.drag?;
        self.cancel_animation();
        let mut term = grid();
        if term.synchronized_output() {
            return Some(false);
        }
        self.sync_reading(&mut term, terminal);
        let before = term.grid.display_offset();
        term.scroll_display(Scroll::Delta(drag_scroll_step(drag.overshoot)));
        self.did_scroll(&term, terminal);
        let offset = term.grid.display_offset();
        let row = if drag.overshoot > 0. {
            0
        } else {
            term.screen_lines().saturating_sub(1)
        };
        if let Some(selection) = self.presented.as_mut() {
            if let Some(reading) = self.reading.as_ref() {
                let grid = super::presentation::PresentedGrid::reading(reading);
                let origin = grid.absolute_origin();
                let target = if selection.clicks() > 1 {
                    grid.click_range(drag.col, row, selection.clicks(), cfg)
                } else {
                    let at = grid.boundary(drag.col, row, drag.side == Side::Left);
                    at..at
                };
                selection.update(target.start + origin..target.end + origin);
            }
        } else {
            let point = Pos::new(Line(row as i32 - offset as i32), Column(drag.col));
            self.extend(&mut term, point, drag.side, &cfg.word_separators);
        }
        if offset != before {
            self.fraction = 0.;
        }
        Some(offset != before)
    }

    /// One trackpad gesture chooses scrolling or zooming once, including its
    /// momentum tail. A fresh Started event or idle timeout asks again. Wheel
    /// detents never latch; their modifiers are considered notch by notch.
    /// The quantized-route check is lazy too: a zoom never needs the grid lock.
    pub(super) fn wheel(
        &mut self,
        ev: &ScrollEvent,
        line_height: f32,
        cfg: &Config,
        os: OperatingSystem,
        quantized: impl FnOnce() -> bool,
    ) -> WheelMotion {
        let now = Instant::now();
        let gesturing = matches!(ev.touch_phase, TouchPhase::Started)
            || self.gesture_until.is_some_and(|until| now < until);
        self.gesture_until = gesturing.then(|| now + SCROLL_GESTURE_IDLE);
        if !gesturing || matches!(ev.touch_phase, TouchPhase::Started) {
            self.gesture_zoom = None;
            self.zoom_debt = 0.;
        }
        let wants_zoom = zoom_wheel(cfg.mouse_zoom_modifier, &ev.modifiers, os);
        let zoom = if gesturing {
            *self.gesture_zoom.get_or_insert(wants_zoom)
        } else {
            wants_zoom
        };
        let raw = match ev.delta {
            ScrollDelta::Lines(point) => point.y,
            ScrollDelta::Pixels(point) => point.y.as_f32() / line_height,
        };
        if zoom {
            self.cancel_animation();
            let (steps, debt) = zoom_scroll_steps(raw, self.zoom_debt, gesturing);
            self.zoom_debt = debt;
            return WheelMotion::Zoom(steps);
        }
        let delta = raw * cfg.mouse_scroll_multiplier;
        if quantized() {
            self.cancel_animation();
            let total = self.scroll_debt + delta;
            let lines = total.trunc() as i32;
            self.scroll_debt = total - lines as f32;
            WheelMotion::Quantized(lines)
        } else if !gesturing
            && cfg.smooth_scroll
            && (self.is_animating() || delta.abs() >= SCROLL_ANIM_MIN_JUMP)
        {
            WheelMotion::Animate(delta)
        } else {
            self.cancel_animation();
            WheelMotion::Direct(delta)
        }
    }

    pub(super) fn queue_animation(&mut self, delta: f32) -> Option<u64> {
        if let Some(animation) = self.animation.as_mut() {
            animation.remaining += delta;
            return None;
        }
        self.animation = Some(ScrollAnim {
            remaining: delta,
            last: Instant::now(),
        });
        self.animation_epoch += 1;
        Some(self.animation_epoch)
    }

    /// Every non-animation viewport move retires the pending frame chain.
    pub(super) fn cancel_animation(&mut self) {
        if self.animation.take().is_some() {
            self.animation_epoch += 1;
        }
    }

    pub(super) fn animation_frame<G: std::ops::DerefMut<Target = Grid>>(
        &mut self,
        epoch: u64,
        grid: impl FnOnce() -> G,
        terminal: &RemoteTerminal,
    ) -> Option<(bool, bool)> {
        if epoch != self.animation_epoch {
            return None;
        }
        let animation = self.animation.as_mut()?;
        let now = Instant::now();
        let dt = now.duration_since(animation.last);
        animation.last = now;
        let (step, last) = scroll_anim_step(animation.remaining, dt);
        animation.remaining -= step;
        if last {
            self.animation = None;
        }
        let moved = self.smooth_scroll(&mut grid(), step, terminal);
        if !moved {
            self.cancel_animation();
        }
        Some((moved, moved && !last))
    }

    pub(super) fn smooth_scroll(
        &mut self,
        term: &mut Grid,
        delta: f32,
        terminal: &RemoteTerminal,
    ) -> bool {
        if term.synchronized_output() {
            return false;
        }
        self.sync_reading(term, terminal);
        let (jump, fraction) = smooth_scroll_step(
            term.grid.display_offset(),
            self.fraction,
            delta,
            term.grid.history_size(),
        );
        if jump != 0 {
            term.scroll_display(Scroll::Delta(jump));
        }
        let moved = jump != 0 || fraction != self.fraction;
        self.fraction = fraction;
        self.did_scroll(term, terminal);
        moved
    }

    pub(super) fn scroll_lines(&mut self, term: &mut Grid, lines: i32, terminal: &RemoteTerminal) {
        if term.synchronized_output() {
            return;
        }
        self.sync_reading(term, terminal);
        self.fraction = 0.;
        term.scroll_display(Scroll::Delta(lines));
        self.did_scroll(term, terminal);
    }

    pub(super) fn jump_to_prompt(&mut self, term: &mut Grid) {
        self.cancel_animation();
        self.reading = None;
        self.live_anchor = None;
        term.selection = None;
        term.scroll_display(Scroll::Bottom);
        self.fraction = 0.;
    }

    pub(super) fn scroll_to(&mut self, term: &mut Grid, target: usize, terminal: &RemoteTerminal) {
        self.cancel_animation();
        if term.synchronized_output() {
            return;
        }
        self.sync_reading(term, terminal);
        let delta = target as i32 - term.grid.display_offset() as i32;
        if delta != 0 {
            term.scroll_display(Scroll::Delta(delta));
        }
        self.fraction = 0.;
        self.did_scroll(term, terminal);
    }

    pub(super) fn clear_scrollback(&mut self) {
        self.cancel_animation();
        self.clear_presented();
        self.reading = None;
        self.live_anchor = None;
        self.fraction = 0.;
    }

    #[cfg(test)]
    pub(super) fn animation_epoch(&self) -> u64 {
        self.animation_epoch
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum SelectEndCopy {
    None,
    Grid,
    Editor,
}

fn select_end_copy(enabled: bool, grid: bool, editor: bool) -> SelectEndCopy {
    match (enabled, grid, editor) {
        (false, ..) => SelectEndCopy::None,
        (true, true, _) => SelectEndCopy::Grid,
        (true, false, true) => SelectEndCopy::Editor,
        (true, false, false) => SelectEndCopy::None,
    }
}
/// Fraction of the remaining distance consumed per [`SCROLL_ANIM_FRAME`].
const SCROLL_ANIM_SMOOTH: f32 = 0.4;
/// The frame `SCROLL_ANIM_SMOOTH` is calibrated against: one nominal 60 Hz
/// frame. Decay is scaled by real elapsed time, so the same feel holds at any
/// refresh rate.
const SCROLL_ANIM_FRAME: std::time::Duration = std::time::Duration::from_millis(16);
/// Below this much left to travel, land instead of asymptoting toward it. A
/// twentieth of a line is around a pixel — the tail of an exponential decay is
/// invisible long before it ends, and every frame of it costs a full repaint.
const SCROLL_ANIM_MIN: f32 = 0.05;
/// A jump smaller than this reads as continuous already; spreading it would
/// only put lag between the hand and the grid. Inching a wheel one detent at a
/// time lands here, and so does every event a trackpad sends.
const SCROLL_ANIM_MIN_JUMP: f32 = 1.0;
/// How long a trackpad gesture stays "live" after its last event. Long enough
/// to bridge the gaps in a momentum tail, short enough that reaching for the
/// wheel right after a swipe is not mistaken for more of the swipe.
const SCROLL_GESTURE_IDLE: std::time::Duration = std::time::Duration::from_millis(150);
/// How far a trackpad has to travel, in lines, to earn one font-size step while
/// the platform modifier is held. A wheel detent is one step on its own, so
/// this only ever applies to the continuous stream a gesture produces.
const ZOOM_SCROLL_LINES: f32 = 3.0;
/// Whether this wheel event is a zoom rather than a scroll.
///
/// Exactly one modifier, and it has to be the configured one: anything
/// alongside it is somebody else's gesture — shift in particular is the escape
/// hatch that scrolls the scrollback out from under a mouse-reporting program.
///
/// Off macOS the platform modifier *is* Ctrl, so `Platform` and `Ctrl` describe
/// the same key there; the setting still round-trips, so a config file shared
/// with a Mac keeps meaning what it meant.
fn zoom_wheel(modifier: MouseZoomModifier, mods: &Modifiers, os: OperatingSystem) -> bool {
    if mods.number_of_modifiers() != 1 {
        return false;
    }
    match modifier {
        MouseZoomModifier::Platform => {
            if os == OperatingSystem::Mac {
                mods.platform
            } else {
                mods.control
            }
        }
        MouseZoomModifier::Ctrl => mods.control,
        MouseZoomModifier::Alt => mods.alt,
        MouseZoomModifier::None => false,
    }
}

/// How many font-size steps a zoom event is worth, and what is left over for
/// the next one.
///
/// A wheel detent is a discrete click of intent, so it is one step whatever the
/// platform says it covers — macOS calls a single notch five lines, and five
/// points of font per notch would be unusable. A trackpad has no detents and
/// spends a flick over dozens of events, so those accumulate and only pay out
/// once the fingers have travelled [`ZOOM_SCROLL_LINES`].
fn zoom_scroll_steps(lines: f32, debt: f32, gesturing: bool) -> (i32, f32) {
    if !gesturing {
        let step = if lines > 0. {
            1
        } else if lines < 0. {
            -1
        } else {
            0
        };
        return (step, 0.);
    }
    let total = debt + lines;
    let steps = (total / ZOOM_SCROLL_LINES).trunc();
    (steps as i32, total - steps * ZOOM_SCROLL_LINES)
}

fn smooth_scroll_step(offset: usize, frac: f32, delta: f32, max: usize) -> (i32, f32) {
    let pos = (offset as f32 + frac + delta).clamp(0., max as f32);
    let new_offset = pos.floor();
    (new_offset as i32 - offset as i32, pos - new_offset)
}
/// How much of `remaining` to consume this frame, and whether this is the last
/// one. Decay is scaled by the real elapsed time so a dropped frame covers the
/// ground it missed instead of stretching the animation out.
fn scroll_anim_step(remaining: f32, dt: std::time::Duration) -> (f32, bool) {
    if remaining.abs() <= SCROLL_ANIM_MIN {
        return (remaining, true);
    }
    let frames = (dt.as_secs_f32() / SCROLL_ANIM_FRAME.as_secs_f32()).clamp(0.1, 8.);
    let consumed = 1. - (1. - SCROLL_ANIM_SMOOTH).powf(frames);
    let step = remaining * consumed;
    if (remaining - step).abs() <= SCROLL_ANIM_MIN {
        (remaining, true)
    } else {
        (step, false)
    }
}

fn drag_scroll_step(overshoot: f32) -> i32 {
    let lines = overshoot.abs().ceil().clamp(1., 8.) as i32;
    if overshoot < 0. { -lines } else { lines }
}

#[cfg(test)]
#[path = "selection_scroll_tests.rs"]
mod tests;
