//! Cohesive private subsystems owned by [`EditorState`](crate::EditorState).
//!
//! Spec #333 wave 1: the editor's flat per-presentation fields live here as
//! four `pub(crate)` structs — the layout frame + shaping caches
//! ([`FrameState`]), the hit map + affordance/drag state
//! ([`AffordanceState`]), the single mutually exclusive overlay menu
//! ([`OverlayMenuState`]), and selection + presentation-local IME state
//! ([`SelectionState`]). [`EditorState`](crate::EditorState) keeps core
//! identity, style/config, and the optional host capability callbacks;
//! `Document` remains the sole authority for source text, history, and the
//! single active IME `Composition` lease.

mod affordances;
mod frame;
mod overlay_menu;
mod selection;

pub(crate) use affordances::AffordanceState;
pub(crate) use frame::FrameState;
pub(crate) use overlay_menu::OverlayMenuState;
pub(crate) use selection::SelectionState;
