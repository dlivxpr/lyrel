use super::*;

/// #738: which pane a directional move came from is remembered against the
/// pane it landed on, not against the direction alone.
///
/// One slot per direction is enough for a single move and back, but the
/// second step of a walk overwrites the first: left off `3` onto `2` and
/// left again onto `1` would leave only `1 -> 2`, and the second move back
/// right — the one out of `2` — would be handed no origin and fall to the
/// geometry that sent the user to the wrong pane in the first place.
#[test]
fn each_pane_remembers_the_move_that_landed_on_it() {
    fn id(n: u64) -> gpui_kit::EntityId {
        gpui_kit::EntityId::from(n)
    }
    let mut tab = Tab::new(Pane::Empty);
    let live = [id(1), id(2), id(3)];

    tab.remember_focus_origin(id(3), id(2), Dir::Left, &live);
    tab.remember_focus_origin(id(2), id(1), Dir::Left, &live);

    // Walking back retraces both steps rather than only the last one.
    assert_eq!(tab.focus_origin(id(1), Dir::Right), Some(id(2)));
    assert_eq!(tab.focus_origin(id(2), Dir::Right), Some(id(3)));
    // Nothing is claimed about a direction no move went in, or about a pane
    // no move has landed on.
    assert_eq!(tab.focus_origin(id(2), Dir::Left), None);
    assert_eq!(tab.focus_origin(id(3), Dir::Right), None);

    // A pane the tab no longer holds takes its entries with it, on either
    // side: with `3` closed, `2` no longer remembers having come from it,
    // and the move back out of `2` is left to geometry.
    tab.remember_focus_origin(id(1), id(2), Dir::Right, &[id(1), id(2)]);
    assert_eq!(tab.focus_origin(id(2), Dir::Right), None);
    assert_eq!(tab.focus_origin(id(2), Dir::Left), Some(id(1)));
}
