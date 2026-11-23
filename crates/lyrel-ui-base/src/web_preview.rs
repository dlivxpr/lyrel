//! A pane asking the workbench to preview a URL in the Side Panel browser
//! (#392).
//!
//! The terminal knows which Host it is bound to and whether a loopback URL
//! needs its forward; the app owns the tab, the Host binding and the verified
//! route. Emitting this event instead of handing the URL to the system
//! browser is what keeps a remote `localhost:3000` page on its own Host.

/// Open `address` in the Side Panel browser, bound to the originating pane's
/// Workspace Host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenWebPageRequested {
    /// The Host's own spelling of the address, e.g. `http://localhost:3000/`.
    pub address: String,
}
