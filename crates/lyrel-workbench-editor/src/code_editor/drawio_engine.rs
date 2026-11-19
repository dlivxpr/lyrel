//! The fixed offline draw.io engine exception.
//!
//! Only this module knows how to turn a draw.io document into a native child
//! surface. It is deliberately separate from the text editor, the Markdown
//! preview, and every other reader: no other format gains JavaScript
//! execution, and the shared core/server/CLI never see a WebView dependency.
//!
//! The pieces are:
//!
//! - `assets`: the exact bytes the WebView may load, their CSP and hashes;
//! - `bridge`: the session token and message state machine;
//! - `surface`: the native child surface — WebView2 on Windows, WKWebView on
//!   macOS — behind the same allowlist contract;
//! - `session`: session/token identities and the preview theme.
//!
//! The page/page-count facts Lyrel reads before rendering live in the
//! GUI-free `lyrel_document::drawio` engine.
//!
//! The engine is the official draw.io viewer bundle: it renders the graph
//! with draw.io's own shapes, stencils and layout, and its own Graph/Editor
//! edits the same model. Lyrel does not implement a reduced renderer:
//! editing, clipboard and save all ride the same genuine engine, while the
//! workbench keeps the one document authority — commits land on its bytes
//! and its own shared history drives undo and redo.

pub mod assets;
pub mod bridge;
pub mod session;
pub mod surface;

#[cfg(test)]
#[path = "drawio_engine/tests.rs"]
mod tests;
