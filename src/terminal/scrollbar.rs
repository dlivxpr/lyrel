//! The scrollback bar down the right edge of a terminal pane (issue #432).
//!
//! A pane's scroll position does not live in a GPUI ScrollHandle: it is
//! Rio's display_offset, counted in rows of scrollback rather than pixels.
//! This handle translates between the two and retains the viewport's actual
//! window bounds for the shared Kit scrollbar.
//! free — a thumb that appears while the view moves and fades out once it
//! stops.
//!
//! The bar never touches the terminal. [`set_offset`](ScrollbarHandle::set_offset)
//! only records the row it wants; `TerminalView::sync_frame_facts` applies that on
//! the next render and reports back where the grid actually ended up.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::scroll::ScrollbarHandle;
use gpui_kit::{Bounds, Pixels, Point, Size, point, px, size};

/// Where the grid stood the last time the pane reported in.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct GridScroll {
    /// Rows of scrollback behind the viewport.
    pub(crate) history: usize,
    /// How many of those rows the viewport has been scrolled back over. Zero is
    /// the live edge.
    pub(crate) display_offset: usize,
    /// Rows the viewport shows.
    pub(crate) screen_lines: usize,
    /// The height of one row, in logical pixels.
    pub(crate) line_height: f32,
}

impl GridScroll {
    /// A row is never zero pixels tall; a zero here would only ever be the
    /// default this starts life with, one render before the first layout.
    fn line_height(&self) -> f32 {
        self.line_height.max(1.)
    }

    /// Rows that have scrolled off the top of the viewport.
    fn above(&self) -> usize {
        self.history.saturating_sub(self.display_offset)
    }
}

#[derive(Clone, Copy, Default)]
struct ScrollState {
    reported: GridScroll,
    current: GridScroll,
}

/// The pane's end of the scrollbar: a snapshot of the grid the bar reads, and a
/// row the bar asks for.
#[derive(Clone, Default)]
pub(crate) struct TerminalScrollHandle {
    grid: Rc<Cell<ScrollState>>,
    viewport: Rc<Cell<Bounds<Pixels>>>,
    /// The `display_offset` the bar wants and the pane has not applied yet.
    pending: Rc<Cell<Option<usize>>>,
}

impl TerminalScrollHandle {
    pub(crate) fn set_viewport(&self, bounds: Bounds<Pixels>) {
        self.viewport.set(bounds);
    }

    /// Report where the grid stands now.
    ///
    /// Scrollback growth at the live edge must not reveal the bar, so only its
    /// reported geometry is held. Input always uses the current grid: freezing
    /// it too would clamp a first scrollbar request against old or empty history
    /// and prevent the viewport from entering stable reading at all.
    pub(crate) fn sync(&self, live: GridScroll) {
        let state = self.grid.get();
        let snap = state.reported;
        let pinned_growth = live.display_offset == 0
            && snap.display_offset == 0
            && live.history >= snap.history
            && live.screen_lines == snap.screen_lines
            && live.line_height == snap.line_height;
        self.grid.set(ScrollState {
            reported: if pinned_growth { snap } else { live },
            current: live,
        });
    }

    /// The row the bar was dragged to, if it was dragged since the last render.
    pub(crate) fn take_pending(&self) -> Option<usize> {
        self.pending.take()
    }

    #[cfg(test)]
    fn snapshot(&self) -> GridScroll {
        self.grid.get().reported
    }
}

impl ScrollbarHandle for TerminalScrollHandle {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.viewport.get()
    }

    fn offset(&self) -> Point<Pixels> {
        let grid = self.grid.get().reported;
        // Scroll offsets run negative as the content moves up past the top of
        // the viewport, which is what the rows above it have done.
        point(px(0.), px(-(grid.above() as f32) * grid.line_height()))
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        let grid = self.grid.get().current;
        let above = (-offset.y.as_f32() / grid.line_height())
            .round()
            .clamp(0., grid.history as f32) as usize;
        let target = grid.history - above;
        if target == grid.display_offset {
            return;
        }
        // Move the snapshot with the thumb rather than waiting for the pane to
        // confirm: the bar reads the offset back on the very next mouse move to
        // decide where the thumb sits, and a snapshot still showing the old row
        // would drag it back under the cursor.
        let grid = GridScroll {
            display_offset: target,
            ..grid
        };
        self.grid.set(ScrollState {
            reported: grid,
            current: grid,
        });
        self.pending.set(Some(target));
    }

    fn content_size(&self) -> Size<Pixels> {
        let grid = self.grid.get().reported;
        // Width is never read for a vertical-only bar, and the pane has no
        // horizontal scroll to describe.
        size(
            px(0.),
            px((grid.history + grid.screen_lines) as f32 * grid.line_height()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(history: usize, display_offset: usize) -> GridScroll {
        GridScroll {
            history,
            display_offset,
            screen_lines: 24,
            line_height: 10.,
        }
    }

    #[test]
    fn the_thumb_sits_at_the_bottom_at_the_live_edge_and_at_the_top_of_the_scrollback() {
        let handle = TerminalScrollHandle::default();

        handle.sync(grid(100, 0));
        assert_eq!(handle.offset().y, px(-1000.));
        assert_eq!(handle.content_size().height, px(1240.));

        handle.sync(grid(100, 100));
        assert_eq!(
            handle.offset().y,
            px(0.),
            "scrolled all the way back is the top of the content"
        );
    }

    #[test]
    fn dragging_the_thumb_asks_the_pane_for_a_row() {
        let handle = TerminalScrollHandle::default();
        handle.sync(grid(100, 0));

        handle.set_offset(point(px(0.), px(-250.)));
        assert_eq!(
            handle.take_pending(),
            Some(75),
            "25 rows down from the top of a 100-row scrollback"
        );
        assert_eq!(
            handle.offset().y,
            px(-250.),
            "and the thumb stays where the drag put it until the pane renders"
        );
        assert_eq!(
            handle.take_pending(),
            None,
            "asked for once, not every frame"
        );
    }

    #[test]
    fn a_drag_past_either_end_lands_on_it() {
        let handle = TerminalScrollHandle::default();
        handle.sync(grid(100, 50));

        handle.set_offset(point(px(0.), px(400.)));
        assert_eq!(
            handle.take_pending(),
            Some(100),
            "no further back than the top"
        );

        handle.sync(grid(100, 50));
        handle.set_offset(point(px(0.), px(-9000.)));
        assert_eq!(
            handle.take_pending(),
            Some(0),
            "no further forward than the live edge"
        );
    }

    #[test]
    fn a_drag_that_lands_on_the_row_it_started_on_asks_for_nothing() {
        let handle = TerminalScrollHandle::default();
        handle.sync(grid(100, 40));

        // Half a row's worth of travel, which rounds back to where it was.
        handle.set_offset(point(px(0.), px(-604.)));
        assert_eq!(handle.take_pending(), None);
    }

    #[test]
    fn output_at_the_live_edge_does_not_move_the_bar() {
        let handle = TerminalScrollHandle::default();
        handle.sync(grid(100, 0));
        let before = handle.offset();

        // A screenful of new output, all of it pushing history under a viewport
        // that is already at the bottom.
        handle.sync(grid(124, 0));
        assert_eq!(
            handle.offset(),
            before,
            "a streaming pane would otherwise hold the thumb on screen the whole time"
        );
        assert_eq!(handle.snapshot().history, 100);
    }

    #[test]
    fn growth_hidden_from_the_bar_still_updates_the_rows_a_drag_can_reach() {
        let handle = TerminalScrollHandle::default();
        handle.sync(grid(0, 0));
        let hidden = handle.offset();
        handle.sync(grid(124, 0));
        assert_eq!(
            handle.offset(),
            hidden,
            "output alone must not reveal the bar"
        );

        handle.set_offset(point(px(0.), px(-1000.)));
        assert_eq!(handle.take_pending(), Some(24));
        assert_eq!(handle.offset().y, px(-1000.));
        assert_eq!(handle.content_size().height, px(1480.));
    }

    #[test]
    fn everything_other_than_growth_at_the_live_edge_is_reported() {
        let handle = TerminalScrollHandle::default();

        handle.sync(grid(100, 0));
        handle.sync(grid(100, 3));
        assert_eq!(handle.snapshot().display_offset, 3, "the viewport moved");

        handle.sync(grid(140, 5));
        assert_eq!(
            handle.snapshot().history,
            140,
            "output arriving while scrolled back moves the rows under the thumb"
        );

        handle.sync(grid(140, 0));
        handle.sync(grid(0, 0));
        assert_eq!(
            handle.snapshot().history,
            0,
            "a cleared scrollback is not growth, and leaves nothing to scroll"
        );

        handle.sync(grid(0, 0));
        handle.sync(GridScroll {
            screen_lines: 40,
            ..grid(0, 0)
        });
        assert_eq!(
            handle.snapshot().screen_lines,
            40,
            "a resized pane changes how much of the content is on screen"
        );

        handle.sync(GridScroll {
            line_height: 18.,
            ..grid(0, 0)
        });
        assert_eq!(
            handle.snapshot().line_height,
            18.,
            "and so does a font-size change"
        );
    }
}
