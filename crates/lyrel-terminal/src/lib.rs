//! The desktop terminal client, extracted from the root `lyrel` workbench
//! package: the pane connection and reader/writer tasks, the terminal
//! view/element render path, glyph protocol and Kitty/iTerm2/Sixel client
//! graphics, smart double-click selection, composer and prompt input.
//!
//! Desktop-only, mirroring the root package: on Linux this crate is an empty
//! rlib so headless workspace builds are unaffected. The workbench shell
//! (window chrome, workspace switcher, editor) consumes this crate; this
//! crate never depends on the shell, the editor, or any LSP/WebView stack.

// Desktop-only client: everything below compiles only on Windows/macOS. On
// Linux this crate is an empty rlib so headless workspace builds are
// unaffected, exactly like `lyrel-ui-base`.
#![cfg(any(target_os = "macos", target_os = "windows"))]

pub mod agent_activity;
pub mod boxdraw;
pub mod bridge;
pub mod cmd_editor;
pub mod color_scheme;
pub mod command_cursor;
pub mod completion;
pub mod composer;
pub mod desktop_notification;
pub mod element;
pub mod fps;
pub mod forward_client;
pub mod fuzzy;
pub mod generator;
pub mod glyph;
pub mod highlight;
pub mod history;
pub mod history_session;
pub mod hold;
pub mod images;
pub mod input;
pub mod lifecycle;
pub mod link;
pub mod link_probe;
pub mod loopback;
pub mod palette;
pub mod parked_cursor;
pub mod paste;
pub mod pointer;
pub mod prefill;
pub mod presentation;
pub mod prompt_input;
pub mod prompt_reflow;
pub mod reading;
pub mod remote;
pub mod reverse_search;
pub mod scrollbar;
pub mod search;
pub mod selection_scroll;
pub mod sftp_client;
pub mod signature;
pub mod size;
pub mod smart_select;
pub mod typeahead;
pub mod view;

// Windows toast branding (`aumid`) and the shared debug rate meter, lifted
// out of the root package with the client: their only consumers are here and
// the root shell, which depends on this crate anyway.
#[cfg(target_os = "windows")]
pub mod aumid;
pub mod rate_meter;

// Rooted-consumer graphics test helpers, quiet test panes and the native
// consumer fixture hooks the root package's whole-window and native suites
// build on. Never part of the production interface.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use remote::RemoteTerminal;
pub use remote::SpawnCancellation;
pub use size::TermSize;
