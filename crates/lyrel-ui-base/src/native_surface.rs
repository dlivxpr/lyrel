//! Window-scoped covers for native child surfaces (the browser's WebView and
//! the diagram surface): a dialog, sheet or any other surface the shell opens
//! above the GPUI canvas registers here, and the native children hide for as
//! long as one is up. The count is per window, so a dialog in one window does
//! not conceal another window's page.
//!
//! The root workbench installs one change listener per `App` (through
//! [`on_change`]) and pushes the new state into its surfaces. Crates that
//! cannot reach `LyrelApp` — the workbench editor's Markdown dialogs, for
//! instance — only need the window and an `App` to raise and release a cover.
//!
//! A guard releases on `Drop` as a safety net; callers that can still reach
//! `cx` should call [`NativeSurfaceCoverGuard::release`] so the listener runs
//! on the same frame the dialog closes.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::{App, Global, Window, WindowId};

type Listener = Rc<dyn Fn(WindowId, &mut App)>;

/// The covers currently raised across this `App`, and the listeners the shell
/// installed to react to them.
#[derive(Default)]
pub struct NativeSurfaceCovers {
    windows: HashMap<WindowId, Rc<Cell<usize>>>,
    listeners: RefCell<Vec<Listener>>,
}

impl Global for NativeSurfaceCovers {}

fn ensure(cx: &mut App) {
    if !cx.has_global::<NativeSurfaceCovers>() {
        cx.set_global(NativeSurfaceCovers::default());
    }
}

fn notify(window: WindowId, cx: &mut App) {
    let listeners: Vec<Listener> = cx
        .try_global::<NativeSurfaceCovers>()
        .map(|covers| covers.listeners.borrow().clone())
        .unwrap_or_default();
    for listener in listeners {
        listener(window, cx);
    }
}

/// Registers one reaction to a window's cover count changing. The workbench
/// shell installs its surface sync here once per `App`; later registrations
/// are additive, and a listener whose window is gone simply finds nothing to
/// update.
pub fn on_change(cx: &mut App, listener: impl Fn(WindowId, &mut App) + 'static) {
    ensure(cx);
    cx.global_mut::<NativeSurfaceCovers>()
        .listeners
        .borrow_mut()
        .push(Rc::new(listener));
}

/// Raises one cover on `window`. Keep the returned guard for as long as the
/// covering surface is up, and call [`NativeSurfaceCoverGuard::release`] when
/// it closes.
pub fn cover(window: &Window, cx: &mut App) -> NativeSurfaceCoverGuard {
    ensure(cx);
    let window_id = window.window_handle().window_id();
    let count = {
        let covers = cx.global_mut::<NativeSurfaceCovers>();
        let count = covers
            .windows
            .entry(window_id)
            .or_insert_with(|| Rc::new(Cell::new(0)))
            .clone();
        count.set(count.get() + 1);
        count
    };
    notify(window_id, cx);
    NativeSurfaceCoverGuard {
        window: window_id,
        count,
        released: Rc::new(Cell::new(false)),
    }
}

/// Whether any covering surface is up in `window`.
pub fn is_covered(window: WindowId, cx: &App) -> bool {
    cx.try_global::<NativeSurfaceCovers>()
        .and_then(|covers| covers.windows.get(&window))
        .is_some_and(|count| count.get() > 0)
}

/// One raised cover. Clone it into the dialog's `on_close`; the clone and the
/// original share one release latch, so the count is decremented once no
/// matter which copy sees the close first.
#[derive(Clone)]
pub struct NativeSurfaceCoverGuard {
    window: WindowId,
    count: Rc<Cell<usize>>,
    released: Rc<Cell<bool>>,
}

impl NativeSurfaceCoverGuard {
    /// Releases this guard's cover and notifies listeners. Idempotent.
    pub fn release(&self, cx: &mut App) {
        if !self.take() {
            return;
        }
        notify(self.window, cx);
    }

    fn take(&self) -> bool {
        if self.released.replace(true) {
            return false;
        }
        self.count.set(self.count.get().saturating_sub(1));
        true
    }
}

impl Drop for NativeSurfaceCoverGuard {
    fn drop(&mut self) {
        // A dialog torn down without its close callback still gives the cover
        // back. There is no `App` here to notify listeners; the next cover
        // change or the shell's watchdog pushes the released state out.
        let _ = self.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EmptyView;

    impl gpui_kit::Render for EmptyView {
        fn render(
            &mut self,
            _: &mut gpui_kit::Window,
            _: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            gpui_kit::div()
        }
    }

    #[gpui_kit::test]
    fn covers_count_per_window_and_release_once(cx: &mut gpui_kit::TestAppContext) {
        let (_, mut window) = cx.add_window_view(|_, _| EmptyView);
        window.update(|window, cx| {
            let id = window.window_handle().window_id();
            assert!(!is_covered(id, cx));
            let guard = cover(window, cx);
            assert!(is_covered(id, cx));
            let clone = guard.clone();
            clone.release(cx);
            assert!(!is_covered(id, cx), "one release lifts the cover");
            guard.release(cx);
            assert!(!is_covered(id, cx), "release is idempotent");
        });
    }

    #[gpui_kit::test]
    fn a_change_listener_sees_the_window(cx: &mut gpui_kit::TestAppContext) {
        use std::cell::RefCell;
        use std::rc::Rc;

        let seen = Rc::new(RefCell::new(Vec::new()));
        let (_, mut window) = cx.add_window_view(|_, _| EmptyView);
        window.update(|window, cx| {
            let seen = seen.clone();
            on_change(cx, move |id, _| seen.borrow_mut().push(id));
            let id = window.window_handle().window_id();
            let guard = cover(window, cx);
            guard.release(cx);
            assert_eq!(&*seen.borrow(), &[id, id]);
        });
    }
}
