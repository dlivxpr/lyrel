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

use std::collections::HashMap;

use gpui_kit::component::menu::PopupMenu;
use gpui_kit::{
    App, DismissEvent, EntityId, Focusable as _, Global, Subscription, WeakEntity, WindowId,
};

/// One overlay that takes visual and input precedence over the window's
/// native surfaces while it is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoverSource {
    /// Search Everywhere, the workbench's own modal palette.
    SearchEverywhere,
    /// The Side Panel `+` add menu (the browser-relevant menu of #391).
    ToolAddMenu,
    /// The Park fan, its overflow list or its preview card.
    ParkFan,
    /// The warm drop target a tiled or picture-in-picture pane drag reveals.
    ParkDropTarget,
    /// The notification bell's inbox popover.
    InboxPopover,
    /// The right Side Panel's open/close slide.
    SidePanelTransition,
    /// The right panel's incoming-page entrance after a tool switch.
    ToolSwitch,
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

/// The open `PopupMenu` entities of this `App`, one list for every window.
///
/// A menu is an in-window deferred draw, so it paints *under* a native child
/// surface just like any other overlay. The menu's own focus handle is the
/// reliable open signal: the library focuses a top-level menu when it opens
/// and restores the previous focus when it dismisses, while a submenu is
/// created without taking focus. A menu entity can outlive its dismissal
/// inside a trigger's element state, so counting creations would keep the
/// page hidden forever; asking focus per window cannot.
#[derive(Default)]
pub(crate) struct PopupMenus {
    menus: Vec<WeakEntity<PopupMenu>>,
    subscriptions: HashMap<EntityId, Subscription>,
}

impl Global for PopupMenus {}

/// Installs the one `PopupMenu` observer for this `App`. Idempotent so every
/// window opened into the same process shares one list.
pub(crate) fn install_popup_menu_presence(cx: &mut App) {
    if cx.has_global::<PopupMenus>() {
        return;
    }
    cx.set_global(PopupMenus::default());
    cx.observe_new::<PopupMenu>(|_menu, _window, cx| {
        let entity_id = cx.entity().entity_id();
        let menu = cx.entity();
        // A dismissal is the common close path; the release hook below also
        // covers a menu replaced or dropped without one.
        let dismiss = cx.subscribe_self(|_, _: &DismissEvent, cx| notify_live_apps(cx));
        let release = cx.on_release(move |_, cx| {
            if cx.has_global::<PopupMenus>() {
                let state = cx.global_mut::<PopupMenus>();
                state.subscriptions.remove(&entity_id);
                state.menus.retain(|weak| weak.entity_id() != entity_id);
            }
            notify_live_apps(cx);
        });
        let state = cx.global_mut::<PopupMenus>();
        state.menus.push(menu.downgrade());
        state.subscriptions.insert(entity_id, dismiss);
        release.detach();
    })
    .detach();
}

/// Whether a menu in `window` currently holds focus. Submenus never do, so
/// they do not keep the page concealed after the top-level menu closes.
pub(crate) fn popup_menu_open(window: WindowId, cx: &mut App) -> bool {
    let Some(state) = cx.try_global::<PopupMenus>() else {
        return false;
    };
    let menus: Vec<gpui_kit::FocusHandle> = state
        .menus
        .iter()
        .filter_map(|menu| menu.upgrade())
        .map(|menu| menu.read(cx).focus_handle(cx))
        .collect();
    if menus.is_empty() {
        return false;
    }
    let Some(handle) = cx.windows().into_iter().find(|h| h.window_id() == window) else {
        return false;
    };
    menus.iter().any(|focus| {
        handle
            .update(cx, |_, window, cx| {
                focus.is_focused(window) || focus.contains_focused(window, cx)
            })
            .unwrap_or(false)
    })
}

/// The whole workbench's surfaces re-read the cover set after a menu opens,
/// closes or is dropped.
fn notify_live_apps(cx: &mut App) {
    let apps: Vec<_> = crate::ui::windows::WindowRegistry::live_apps(cx).collect();
    for app in apps {
        let _ = app.update(cx, |app, cx| app.sync_browser_surfaces(cx));
    }
}

#[derive(Default)]
struct NativeSurfaceCoverHooks;

impl Global for NativeSurfaceCoverHooks {}

/// Installs the bridge from the shared dialog/sheet cover registry to each
/// window's workbench, once per `App`. A cover raised by a crate that cannot
/// name `LyrelApp` (the workbench editor's dialogs) lands here and the owning
/// window hides its native surfaces on the same frame.
pub(crate) fn install_native_surface_cover_hooks(cx: &mut App) {
    if cx.has_global::<NativeSurfaceCoverHooks>() {
        return;
    }
    cx.set_global(NativeSurfaceCoverHooks);
    lyrel_ui_base::native_surface::on_change(cx, |window, cx| {
        if let Some(app) = crate::ui::windows::WindowRegistry::app_for_window(cx, window) {
            let _ = app.update(cx, |app, cx| app.sync_browser_surfaces(cx));
        }
    });
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
        assert!(coordinator.is_covered(), "one cover hides the surfaces");
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
