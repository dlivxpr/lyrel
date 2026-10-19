//! Opening a text box on a value the user is about to replace.
//!
//! A box that comes up with something already in it — a rename, a find bar, a
//! prompt carrying a suggested name — only behaves if the caret treats that
//! text as a *value* rather than as text to write around.
//! `InputState::default_value` leaves the caret at offset 0, so the first
//! keystroke prepends ("beta" over "alpha" gives "betaalpha"); `set_value`
//! leaves it at the end, so it appends. Both are wrong for something you are
//! replacing, and every find bar and rename box on the platform answers the
//! same way: select it.

use gpui_kit::component::input::{InputState, SelectAll};
use gpui_kit::{AppContext as _, Context, Entity, Window};

/// How many frames to keep trying for. One is not enough — see below — and
/// nothing in the app has needed more than two; the rest is headroom, and the
/// loop stops the moment the selection lands.
const ATTEMPTS: u8 = 4;

/// Select everything in `input` as soon as it has been drawn.
///
/// `InputState::select_all` is `pub(super)` in the UI crate, so this goes
/// through the action the input itself binds ⌘A to, aimed at the box's own
/// focus handle rather than at whatever happens to be focused.
///
/// It has to wait for a frame: actions route along the dispatch tree of the
/// last frame *drawn*, and a box created this turn is not in it — and frame
/// callbacks run before that frame is drawn, not after, so one wait is not
/// always enough. Rather than guess, dispatch and then look at whether it
/// took. That is also why this cannot be unit-tested: the test harness never
/// paints.
///
/// One box is deliberately left off this: the file tree's inline rename. The
/// selection lands there like anywhere else — typing does replace the name —
/// but that input paints no selection band, where the SFTP form's does, so the
/// name would go in one keystroke with nothing on screen to warn of it. A
/// selection you cannot see is worse than a caret in the wrong place, so that
/// box keeps its caret at the end until the painting is understood.
pub(crate) fn select_all_when_drawn<T: 'static>(
    input: &Entity<InputState>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    if input.read(cx).value().is_empty() {
        return;
    }
    try_select(input.clone(), ATTEMPTS, window);
}

fn try_select(input: Entity<InputState>, left: u8, window: &mut Window) {
    window.on_next_frame(move |window, cx| {
        let handle = gpui_kit::Focusable::focus_handle(input.read(cx), cx);
        handle.dispatch_action(&SelectAll, window, cx);
        if input.read(cx).selected_range().is_empty() && left > 1 {
            try_select(input, left - 1, window);
        }
    });
    // Registering a callback does not by itself ask for a frame, and a panel
    // that has finished drawing has no other reason to produce one.
    window.refresh();
}

/// A new box holding `value`, with the value selected.
///
/// Focusing is left to the caller: a form opens with one of its boxes focused
/// and the rest merely filled, and tabbing into one of those selects it the
/// same way.
pub(crate) fn filled_box<T: 'static>(
    value: impl Into<String>,
    window: &mut Window,
    cx: &mut Context<T>,
) -> Entity<InputState> {
    let input = cx.new(|cx| InputState::new(window, cx));
    input.update(cx, |state, cx| state.set_value(value.into(), window, cx));
    select_all_when_drawn(&input, window, cx);
    input
}
