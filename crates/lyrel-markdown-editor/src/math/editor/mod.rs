//! The structural math editor.
//!
//! `model` is the editable tree; `latex::parse_latex_checked` rejects lossy entry.
//! `geometry`, `cursor` and `input` retain the pinned two-dimensional editing behavior.
//! `view` seats the tree through GPUI Kit/Fast; the whole crate is desktop-only, so it
//! does not declare a second GUI feature/backend or leak GUI types into headless packages.

pub mod latex;
pub mod model;

pub mod cursor;
pub mod geometry;
pub mod input;
pub mod view;
