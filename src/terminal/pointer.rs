//! OSC 22 pointer-shape claims: the application's name for the pointer while
//! it hovers this pane's content, mapped onto the GPUI shapes the platform
//! can actually show.
//!
//! The wire half lives in `rio-vt`'s parser and `ParseBridge`
//! (`Handler::set_mouse_cursor_icon`); the empty `OSC 22 ;` reset the upstream
//! callback never sees is cut out of the stream by the reader's
//! `OscTokenizer`. This module owns the last hop: which `CursorIcon` becomes
//! which `CursorStyle`, and an explicit answer for names GPUI has no shape
//! for rather than pretending they rendered.
//!
//! The mapping is written against `gpui-fast`'s `CursorStyle` enum and its
//! Windows `load_cursor` table: `OpenHand`/`ClosedHand` have no real cursor
//! there and degrade to the system arrow, so both grab hands follow the same
//! substitution `ui::app::composition` already makes for its own drag grip —
//! `PointingHand` — instead of silently dropping the affordance the
//! application asked for. `gpui-fast` has no `wait`/`progress`/`help`
//! shape at all on any backend; those claims degrade to the platform arrow
//! in-band rather than being dropped, so a later reset still has a well
//! defined "the app released its claim" meaning.
//!
//! The claim is scoped by construction: it is stored on the pane's own
//! `ParsePresentation` and only `TerminalElement::paint` reads it for that
//! pane's content hitbox. Menus, the split-drag grip, lyrel's own selection
//! and armed-link gestures are evaluated ahead of it (see
//! `TerminalView::content_cursor`); the sidebar, the editor and other panes
//! never see it.

use cursor_icon::CursorIcon;
use gpui_kit::CursorStyle;

/// The desktop shape an OSC 22 name resolves to on this build.
///
/// Exhaustive over the parser's vocabulary — every `CursorIcon` the upstream
/// OSC parser can produce — so an unknown or future name lands on the same
/// honest fallback instead of a missing-arm `unreachable!`. `Arrow` here is
/// "the claim took, the platform has nothing closer", not "no claim": reset
/// still has to release it.
pub(super) fn cursor_style(icon: CursorIcon) -> CursorStyle {
    match icon {
        CursorIcon::Default => CursorStyle::Arrow,
        CursorIcon::ContextMenu => CursorStyle::ContextualMenu,
        CursorIcon::Pointer => CursorStyle::PointingHand,
        CursorIcon::Text => CursorStyle::IBeam,
        CursorIcon::VerticalText => CursorStyle::IBeamCursorForVerticalLayout,
        CursorIcon::Cell | CursorIcon::Crosshair => CursorStyle::Crosshair,
        CursorIcon::Alias => CursorStyle::DragLink,
        CursorIcon::Copy => CursorStyle::DragCopy,
        CursorIcon::NoDrop | CursorIcon::NotAllowed => CursorStyle::OperationNotAllowed,
        CursorIcon::EResize => CursorStyle::ResizeRight,
        CursorIcon::WResize => CursorStyle::ResizeLeft,
        CursorIcon::EwResize | CursorIcon::ColResize => CursorStyle::ResizeLeftRight,
        CursorIcon::NResize => CursorStyle::ResizeUp,
        CursorIcon::SResize => CursorStyle::ResizeDown,
        CursorIcon::NsResize | CursorIcon::RowResize => CursorStyle::ResizeUpDown,
        // Follow the native tables, not platform.rs's reversed CSS comments:
        // UpLeftDownRight loads NW/SE; UpRightDownLeft loads NE/SW.
        CursorIcon::NeswResize | CursorIcon::NeResize | CursorIcon::SwResize => {
            CursorStyle::ResizeUpRightDownLeft
        }
        CursorIcon::NwseResize | CursorIcon::NwResize | CursorIcon::SeResize => {
            CursorStyle::ResizeUpLeftDownRight
        }
        // Windows ships no open/closed hand; gpui's `load_cursor` answers
        // both with the arrow there. The drag affordance survives as the
        // pointing hand — the same call `composition` makes for its own
        // grip — elsewhere the real hand exists.
        #[cfg(target_os = "windows")]
        CursorIcon::Grab | CursorIcon::Grabbing => CursorStyle::PointingHand,
        #[cfg(not(target_os = "windows"))]
        CursorIcon::Grab => CursorStyle::OpenHand,
        #[cfg(not(target_os = "windows"))]
        CursorIcon::Grabbing => CursorStyle::ClosedHand,
        // `help`, `wait`, `progress`, `move`, `no-drop`-adjacent dnd shapes
        // and scroll/zoom claims have no gpui shape on any platform. Keeping
        // the claim at the arrow is what "honest fallback" means here; it is
        // not silently treated as unset, so the pane still reports the app
        // owns the pointer until an actual release arrives.
        _ => CursorStyle::Arrow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The producers in the wild — kitty's `pointer_shape`, codex's
    /// `link_pointer`, ghostty-compatible TUIs — only ever emit these names;
    /// each must land on the shape the platform really shows.
    #[test]
    fn shapes_the_real_producers_emit() {
        assert_eq!(cursor_style(CursorIcon::Pointer), CursorStyle::PointingHand);
        assert_eq!(cursor_style(CursorIcon::Default), CursorStyle::Arrow);
        assert_eq!(cursor_style(CursorIcon::Text), CursorStyle::IBeam);
    }

    /// Resize claims map direction for direction; the two diagonal corners
    /// gpui names only by their full diagonal are the NE/SW and NW/SE pairs.
    #[test]
    fn resize_names_keep_their_direction() {
        assert_eq!(
            cursor_style(CursorIcon::EwResize),
            CursorStyle::ResizeLeftRight
        );
        assert_eq!(
            cursor_style(CursorIcon::NsResize),
            CursorStyle::ResizeUpDown
        );
        assert_eq!(cursor_style(CursorIcon::WResize), CursorStyle::ResizeLeft);
        assert_eq!(
            cursor_style(CursorIcon::NeResize),
            CursorStyle::ResizeUpRightDownLeft
        );
        assert_eq!(
            cursor_style(CursorIcon::NwResize),
            CursorStyle::ResizeUpLeftDownRight
        );
        assert_eq!(
            cursor_style(CursorIcon::SeResize),
            CursorStyle::ResizeUpLeftDownRight
        );
        assert_eq!(
            cursor_style(CursorIcon::SwResize),
            CursorStyle::ResizeUpRightDownLeft
        );
    }

    /// Names with no gpui shape degrade to the platform arrow — still an
    /// owned claim, not silence — on every platform.
    #[test]
    fn names_without_a_shape_fall_back_in_band() {
        for icon in [
            CursorIcon::Wait,
            CursorIcon::Progress,
            CursorIcon::Help,
            CursorIcon::Move,
            CursorIcon::AllScroll,
            CursorIcon::ZoomIn,
            CursorIcon::ZoomOut,
            CursorIcon::DndAsk,
            CursorIcon::AllResize,
        ] {
            assert_eq!(cursor_style(icon), CursorStyle::Arrow, "{}", icon.name());
        }
    }

    /// The grab hands keep a real affordance per platform instead of the
    /// bare arrow Windows' cursor table would answer.
    #[test]
    fn grab_hands_match_the_platforms_real_cursors() {
        #[cfg(target_os = "windows")]
        {
            assert_eq!(cursor_style(CursorIcon::Grab), CursorStyle::PointingHand);
            assert_eq!(
                cursor_style(CursorIcon::Grabbing),
                CursorStyle::PointingHand
            );
        }
        #[cfg(not(target_os = "windows"))]
        {
            assert_eq!(cursor_style(CursorIcon::Grab), CursorStyle::OpenHand);
            assert_eq!(cursor_style(CursorIcon::Grabbing), CursorStyle::ClosedHand);
        }
    }
}
