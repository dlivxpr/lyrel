//! The desktop document and code workbench: `EditorPanelState` (emitting
//! typed panel events), buffer/save lifecycles, every document format view,
//! the Markdown workbench host providers, the offline draw.io WebView
//! surfaces and the LSP client.
//!
//! Desktop-only (Windows/macOS) and free of terminal code: the crate talks to
//! the window shell exclusively through typed event emissions, and never
//! imports `lyrel-terminal` or the root `lyrel` package.

#![cfg(any(target_os = "macos", target_os = "windows"))]

pub mod code_editor;
pub mod editor_session;
pub mod lsp;
