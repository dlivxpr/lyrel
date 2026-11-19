//! The editor's single overlay menu state. The five independent menu fields
//! this replaces (`menu`, `table_menu` + its scroll, `image_menu`,
//! `prop_menu`, `code_lang_menu` + its scroll) could previously be open
//! simultaneously — several dismiss sites forgot one, leaving stale popups.
//! One enum makes overlapping menus unrepresentable at compile time and
//! [`OverlayMenuState::dismiss`] always clears whatever is open.

use gpui_kit::{Pixels, Point, ScrollHandle};

use crate::DiagMenu;

/// The one overlay (right-click / picker) menu that may be open, if any.
/// Opening one variant replaces whatever was open; there is no "second menu".
pub(crate) enum OverlayMenu {
    /// The right-click suggestions / clipboard / turn-into menu for a
    /// diagnostic or the selection.
    Diagnostics(DiagMenu),
    /// The table right-click menu (Word-style row/column editing), anchored
    /// at the click; its actions operate on the caret's table cell. The
    /// scroll handle drives its overflow list + thumb.
    Table {
        anchor: Point<Pixels>,
        scroll: ScrollHandle,
    },
    /// The image right-click menu: the image's logical line + the menu's
    /// anchor (window space). Offers Word-style object actions (Delete).
    Image { line: usize, anchor: Point<Pixels> },
    /// The right-clicked property-panel row: its source line + click position
    /// (anchors the Edit/Delete property menu).
    Property { line: usize, anchor: Point<Pixels> },
    /// The code block's language picker: the opening fence row + the menu's
    /// anchor. The scroll handle drives its overflow list + thumb.
    CodeLang {
        fence_row: usize,
        anchor: Point<Pixels>,
        scroll: ScrollHandle,
    },
}

/// The single open overlay menu, if any. Owned by
/// [`EditorState`](crate::EditorState); the mutually exclusive invariant
/// lives here, not at the call sites.
pub(crate) struct OverlayMenuState {
    menu: Option<OverlayMenu>,
}

impl OverlayMenuState {
    pub(crate) fn new() -> Self {
        Self { menu: None }
    }

    /// Whether the table right-click menu is the open one — the pointer-owner
    /// gate in `on_mouse_move` (hover tracking pauses behind it).
    pub(crate) fn is_table_open(&self) -> bool {
        matches!(self.menu, Some(OverlayMenu::Table { .. }))
    }

    /// Dismiss whatever menu is open. Returns whether one was (so the caller
    /// repaints only on an actual change).
    pub(crate) fn dismiss(&mut self) -> bool {
        self.menu.take().is_some()
    }

    pub(crate) fn open_diagnostics(&mut self, menu: DiagMenu) {
        self.menu = Some(OverlayMenu::Diagnostics(menu));
    }

    pub(crate) fn open_table(&mut self, anchor: Point<Pixels>) {
        self.menu = Some(OverlayMenu::Table {
            anchor,
            scroll: ScrollHandle::new(),
        });
    }

    pub(crate) fn open_image(&mut self, line: usize, anchor: Point<Pixels>) {
        self.menu = Some(OverlayMenu::Image { line, anchor });
    }

    pub(crate) fn open_property(&mut self, line: usize, anchor: Point<Pixels>) {
        self.menu = Some(OverlayMenu::Property { line, anchor });
    }

    pub(crate) fn open_code_lang(&mut self, fence_row: usize, anchor: Point<Pixels>) {
        self.menu = Some(OverlayMenu::CodeLang {
            fence_row,
            anchor,
            scroll: ScrollHandle::new(),
        });
    }

    pub(crate) fn diagnostics(&self) -> Option<&DiagMenu> {
        match &self.menu {
            Some(OverlayMenu::Diagnostics(menu)) => Some(menu),
            _ => None,
        }
    }

    pub(crate) fn diagnostics_mut(&mut self) -> Option<&mut DiagMenu> {
        match &mut self.menu {
            Some(OverlayMenu::Diagnostics(menu)) => Some(menu),
            _ => None,
        }
    }

    /// The table menu's anchor + overflow scroll handle, if it is open.
    pub(crate) fn table(&self) -> Option<(Point<Pixels>, &ScrollHandle)> {
        match &self.menu {
            Some(OverlayMenu::Table { anchor, scroll }) => Some((*anchor, scroll)),
            _ => None,
        }
    }

    /// The property menu's source row + anchor, if it is open.
    pub(crate) fn property(&self) -> Option<(usize, Point<Pixels>)> {
        match &self.menu {
            Some(OverlayMenu::Property { line, anchor }) => Some((*line, *anchor)),
            _ => None,
        }
    }

    /// The image menu's logical line + anchor, if it is open.
    pub(crate) fn image(&self) -> Option<(usize, Point<Pixels>)> {
        match &self.menu {
            Some(OverlayMenu::Image { line, anchor }) => Some((*line, *anchor)),
            _ => None,
        }
    }

    /// The language picker's fence row + anchor + overflow scroll handle, if
    /// it is open.
    pub(crate) fn code_lang(&self) -> Option<(usize, Point<Pixels>, &ScrollHandle)> {
        match &self.menu {
            Some(OverlayMenu::CodeLang {
                fence_row,
                anchor,
                scroll,
            }) => Some((*fence_row, *anchor, scroll)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{point, px};

    /// The five overlay menus are one enum: opening a variant replaces
    /// whatever was open, so overlapping popups are unrepresentable, and one
    /// `dismiss` clears whichever menu is open.
    #[test]
    fn opening_a_menu_replaces_the_open_one() {
        let mut menus = OverlayMenuState::new();
        assert!(!menus.is_table_open());
        menus.open_table(point(px(10.), px(10.)));
        assert!(menus.is_table_open());
        assert!(menus.table().is_some());

        // A second open replaces the first — the table menu is gone.
        menus.open_code_lang(3, point(px(5.), px(5.)));
        assert!(!menus.is_table_open());
        assert!(menus.table().is_none());
        assert_eq!(menus.code_lang().map(|(row, ..)| row), Some(3));

        // Dismiss clears whatever is open, and reports the change once.
        assert!(menus.dismiss());
        assert!(!menus.dismiss());
        assert!(menus.code_lang().is_none());
    }
}
