//! The shared GPUI desktop foundation extracted from the root `lyrel` package.
//!
//! Everything here is desktop-only (Windows/macOS) and free of terminal
//! emulation, document format engines, WebView engines and the application
//! shell: the crate owns pure values, `Global` state and narrow shell-installed
//! hooks so that downstream desktop crates (terminal, workbench editor) can
//! build on one foundation without importing the workbench.
//!
//! Module exports are explicit throughout — nothing here glob-re-exports
//! `lyrel-core`/`lyrel-protocol` modules, so `config`, `session` and `keychain`
//! keep one unambiguous meaning per module.

#![cfg(any(target_os = "macos", target_os = "windows"))]

pub mod actions;
pub mod config;
pub mod desktop_file;
pub mod desktop_lookups;
pub mod file_drag;
pub mod file_search;
pub mod fuzzy;
pub mod host_ops;
pub mod host_registry;
pub mod i18n;
pub mod notification_bridge;
pub mod notification_history;
pub mod palette;
pub mod pane_dim;
pub mod path_display;
pub mod presets;
pub mod presentation;
pub mod route;
pub mod rounding;
pub mod scm_snapshot;
pub mod session;
pub mod ssh_config;
