//! Window-level coordination between GPUI overlays and native child surfaces
//! (#391's first seam, extensible by #403).
//!
//! A native child window (the browser's WebView, today; the diagram surface
//! is the other client this coordinator is written for) renders above the
//! GPUI canvas inside the same OS window: any overlay the workbench paints —
//! Search Everywhere, a menu, a dialog, the Park fan, the inbox popover —
//! would slide *under* it. GPUI cannot clip a sibling HWND/NSView, so the
//! coordination has to happen at the visibility layer: while any covering
//! source is active, every native surface in the window hides, and the panel
//! paints its bounded static frame instead. The page behind it is never
//! touched — scroll position, an unsubmitted form, keyboard focus all come
//! back with the surface when the last cover lifts.
//!
//! Sources register themselves (`cover`/`uncover`); the surfaces read
//! [`SurfaceCoordinator::is_covered`] from their paint and watchdog paths.
//! #403 adds the remaining overlays — Park fan, inbox popover, dialogs,
//! context menus — as more `CoverSource` variants on this one choke point.

/// One overlay that takes visual and input precedence over the window's
/// native surfaces while it is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoverSource {
    /// Search Everywhere, the workbench's own modal palette.
    SearchEverywhere,
    /// The Side Panel `+` add menu (the browser-relevant menu of #391).
    ToolAddMenu,
}

/// The set of covers currently active in this window. Counted per source,
/// not boolean: two overlays can be up at once (a menu over Search
/// Everywhere), and the surfaces stay hidden until the *last* one lifts.
#[derive(Debug, Default)]
pub(crate) struct SurfaceCoordinator {
    covers: Vec<CoverSource>,
}

impl SurfaceCoordinator {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Registers `source` as covering. Idempotent per source: the same
    /// overlay re-announcing itself (an open callback firing twice) must not
    /// stack.
    pub(crate) fn cover(&mut self, source: CoverSource) {
        if !self.covers.contains(&source) {
            self.covers.push(source);
        }
    }

    pub(crate) fn uncover(&mut self, source: CoverSource) {
        self.covers.retain(|open| *open != source);
    }

    /// Whether any covering overlay is up: the surfaces hide, the bounded
    /// static frames paint.
    pub(crate) fn is_covered(&self) -> bool {
        !self.covers.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn covers(&self) -> &[CoverSource] {
        &self.covers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_are_idempotent_and_lift_one_at_a_time() {
        let mut coordinator = SurfaceCoordinator::new();
        assert!(!coordinator.is_covered());

        coordinator.cover(CoverSource::SearchEverywhere);
        coordinator.cover(CoverSource::SearchEverywhere);
        assert!(
            coordinator.is_covered(),
            "one cover hides the surfaces"
        );
        assert_eq!(
            coordinator.covers(),
            &[CoverSource::SearchEverywhere],
            "re-announcing the same overlay does not stack"
        );

        coordinator.cover(CoverSource::ToolAddMenu);
        assert_eq!(coordinator.covers().len(), 2);

        coordinator.uncover(CoverSource::SearchEverywhere);
        assert!(
            coordinator.is_covered(),
            "the menu still covers while the palette lifted"
        );

        coordinator.uncover(CoverSource::ToolAddMenu);
        assert!(!coordinator.is_covered());

        // Lifting something that never covered is a no-op, not a panic.
        coordinator.uncover(CoverSource::ToolAddMenu);
        assert!(!coordinator.is_covered());
    }
}
