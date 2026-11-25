//! A pane asking the workbench to preview a URL in the Side Panel browser
//! (#392).
//!
//! The terminal knows which Host it is bound to and whether a loopback URL
//! needs its forward; the app owns the tab, the Host binding and the verified
//! route. Emitting this event instead of handing the URL to the system
//! browser is what keeps a remote `localhost:3000` page on its own Host.

/// Where a terminal URL or Info panel port should open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WebOpenDestination {
    #[default]
    Browser,
    SystemBrowser,
}

/// Open `address` in the Side Panel browser or System Browser, bound to the
/// originating pane's Workspace Host when applicable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenWebPageRequested {
    /// The Host's own spelling of the address, e.g. `http://localhost:3000/`.
    pub address: String,
    pub destination: WebOpenDestination,
}

/// Open a workspace HTML file in the Side Panel Browser or System Browser (#422).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenHtmlPreviewRequested {
    pub host_id: crate::host_ops::HostId,
    pub path: std::path::PathBuf,
    pub destination: WebOpenDestination,
}
