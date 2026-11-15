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
//! - `document`: the page/page-count facts Lyrel reads before rendering;
//! - `bridge`: the session token and message state machine;
//! - `surface`: the native child surface — WebView2 on Windows, WKWebView on
//!   macOS — behind the same allowlist contract;
//! - `session`: session/token identities and the preview theme.
//!
//! The engine is the official draw.io viewer bundle: it renders the graph with
//! draw.io's own shapes, stencils and layout. Lyrel does not implement a
//! reduced renderer.

pub(crate) mod assets;
pub(crate) mod bridge;
pub(crate) mod document;
pub(crate) mod session;
pub(crate) mod surface;

pub(crate) use document::{DrawioError, DrawioSource, looks_like_drawio, parse};

#[cfg(test)]
#[path = "drawio_engine/tests.rs"]
mod tests;
