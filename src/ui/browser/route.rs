//! Host-bound routing for Side Panel pages (#392).
//!
//! A page opened from a managed remote Workspace keeps the Host's own spelling
//! of its address — `http://localhost:3000/app` — as its editable and
//! persistent identity. What the native surface actually loads is the local
//! end of a verified SSH forward. This module owns that translation:
//!
//! - [`route_plan`] resolves the tab's durable [`RemoteRef`] against the
//!   Workspace Store and proves the record still describes the same Host.
//! - [`BrowserForwardVerifier`] builds (or finds) the automatic loopback
//!   forward. The production verifier is [`ForwardClient`]; the GPUI
//!   whole-workbench fixture installs a recording one, so the browser's
//!   verification and reconnect paths run for real without a network.
//! - [`VerifiedRoute`] maps between the Host's origin and the forwarded
//!   origin, and is the only thing the surface may load from.
//!
//! Site data is named per Host: one durable partition directory on Windows
//! (`browser/host-<id>`), one identified `WKWebsiteDataStore` on macOS derived
//! from the same stable key. Two Hosts therefore never share cookies or
//! localStorage, and a Host's data survives a restart.
//!
//! Changing a forward's local port changes the browser origin and therefore
//! the bucket for localStorage and IndexedDB. Cookies use host/domain and
//! path rules without port isolation inside a Host's partition. The external
//! browser has its own site data.

use lyrel_core::core::session::RemoteTarget;
use lyrel_terminal::loopback::{LoopbackUrl, parse_loopback_url};
use lyrel_ui_base::route::{ForwardRoute, PaneWorkspace};
use std::path::PathBuf;
use std::sync::Arc;

/// The named site-data partition one Host's pages use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserSiteData {
    /// The profile directory on Windows and the partition identity on macOS.
    pub(crate) dir: PathBuf,
}

/// A verified loopback forward for one Host-bound page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedForward {
    /// The Host's own origin spelling, e.g. `http://localhost:3000`.
    pub(crate) original_origin: String,
    /// The local origin the forward listens on, e.g. `http://127.0.0.1:53124`.
    pub(crate) forwarded_origin: String,
    pub(crate) local_port: u16,
}

impl VerifiedForward {
    pub(crate) fn from_loopback(loopback: &LoopbackUrl, local_port: u16) -> Self {
        Self {
            original_origin: loopback.origin(),
            forwarded_origin: loopback.forwarded_origin(local_port),
            local_port,
        }
    }

    /// The surface URL for a URL the address bar shows in the Host's spelling.
    pub(crate) fn forwarded_url(&self, original: &str) -> Option<String> {
        rewrite_origin(original, &self.original_origin, &self.forwarded_origin)
    }

    /// The Host-spelling URL for a URL the surface reports.
    pub(crate) fn original_url(&self, forwarded: &str) -> Option<String> {
        rewrite_origin(forwarded, &self.forwarded_origin, &self.original_origin)
    }
}

/// The route a Host-bound page has at this moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VerifiedRoute {
    /// The Host shares this machine's loopback (WSL, a local stdio daemon), so
    /// the original address loads as written. The approved origin still
    /// gates the surface: another loopback port could be a local service.
    Shared { origin: String },
    /// The address is served by a verified forward.
    Forwarded(VerifiedForward),
}

impl VerifiedRoute {
    pub(crate) fn approved_origin(&self) -> &str {
        match self {
            Self::Shared { origin } => origin,
            Self::Forwarded(forward) => &forward.forwarded_origin,
        }
    }

    /// The surface URL for `url`, which the address bar shows in the Host's
    /// spelling. `None` when the URL is not on this route's original origin
    /// (an external page the tab navigated to, or another loopback port that
    /// needs its own verification).
    pub(crate) fn forwarded_url(&self, url: &str) -> Option<String> {
        match self {
            Self::Shared { origin } => rewrite_origin(url, origin, origin),
            Self::Forwarded(forward) => forward.forwarded_url(url),
        }
    }

    /// The address-bar URL for a URL the surface reported.
    pub(crate) fn original_url(&self, url: &str) -> Option<String> {
        match self {
            Self::Shared { origin } => rewrite_origin(url, origin, origin),
            Self::Forwarded(forward) => forward.original_url(url),
        }
    }
}

/// What a Host-bound page needs before it can load `loopback`.
#[derive(Debug, Clone)]
pub(crate) enum RoutePlan {
    /// No forward needed; the original origin is approved directly.
    Shared { origin: String },
    /// Build or find the forward through the owning Workspace.
    Forward {
        route: ForwardRoute,
        loopback: LoopbackUrl,
    },
}

/// A failure that keeps the page from loading and asks for an explicit
/// reconnect. The message is shown in the page overlay as-is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouteFailure {
    pub(crate) message: String,
}

impl RouteFailure {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Builds the automatic loopback forward for a Host-bound page.
///
/// Production resolves the real tunnel; automated workbench tests install a
/// recording implementation so the verification, loss and reconnect paths run
/// without a remote fixture. The trait is `Send + Sync` because verification
/// runs on the background executor.
pub(crate) trait ForwardVerifier: Send + Sync {
    fn verify(
        &self,
        route: &ForwardRoute,
        remote_host: &str,
        remote_port: u16,
    ) -> Result<u16, String>;
}

/// The production verifier: the daemon's own automatic loopback forward,
/// which reuses a listening forward and builds a new one when the old one is
/// gone.
struct LiveForwardVerifier;

impl ForwardVerifier for LiveForwardVerifier {
    fn verify(
        &self,
        route: &ForwardRoute,
        remote_host: &str,
        remote_port: u16,
    ) -> Result<u16, String> {
        lyrel_terminal::forward_client::ForwardClient::ensure_loopback(
            route,
            remote_host,
            remote_port,
        )
        .map(|forward| forward.local_port)
        .map_err(|error| error.to_string())
    }
}

/// The process-wide forward verifier. `default_global` installs the live one;
/// tests replace it before opening a page.
pub(crate) struct BrowserForwardVerifier(pub(crate) Arc<dyn ForwardVerifier>);

impl Default for BrowserForwardVerifier {
    fn default() -> Self {
        Self(Arc::new(LiveForwardVerifier))
    }
}

impl gpui_kit::Global for BrowserForwardVerifier {}

/// The named site-data partition for `tab`'s surface. `None` only when the
/// config dir itself is unavailable — the surface then reports Unsupported
/// rather than borrowing a shared profile.
pub(crate) fn browser_site_data(tab: &crate::ui::right_panel::WebTab) -> Option<BrowserSiteData> {
    let root = std::path::absolute(lyrel_core::core::config::config_dir_path()?).ok()?;
    let key = tab
        .host
        .as_ref()
        .map(|host| format!("host-{}", host.host_id().0))
        .unwrap_or_else(|| "local".to_string());
    Some(BrowserSiteData {
        dir: root.join("browser").join(&key),
    })
}

/// Resolves the Workspace that owns `tab`'s Host, refusing a record that now
/// points somewhere else. This is the Host-identity check the surface's
/// approved origin relies on: a reconnect, a repointed profile or a reused
/// workspace id must not silently serve the page from a different machine.
pub(crate) fn owning_workspace(
    tab: &crate::ui::right_panel::WebTab,
    cx: &gpui_kit::App,
) -> Result<PaneWorkspace, RouteFailure> {
    let host = tab
        .host
        .as_ref()
        .ok_or_else(|| RouteFailure::new("this page is not bound to a Host"))?;
    owning_workspace_for_host(host, cx)
}

pub(crate) fn owning_workspace_for_host(
    host: &lyrel_core::core::session::RemoteRef,
    cx: &gpui_kit::App,
) -> Result<PaneWorkspace, RouteFailure> {
    let client_workspace = lyrel_ui_base::session::WorkspaceStore::all(cx)
        .views
        .iter()
        .find(|view| view.open && view.host.as_ref() == Some(host))
        .map(|view| view.id);
    client_workspace
        .and_then(|id| crate::ui::remote_workspace::pane_workspace_for(cx, id))
        .ok_or_else(|| {
            RouteFailure::new(format!(
                "{} is no longer open in this window",
                host.route_label("this Host")
            ))
        })
}

/// What it takes to load `url` for `tab`, or a failure to show.
pub(crate) fn route_plan(
    tab: &crate::ui::right_panel::WebTab,
    url: &str,
    cx: &gpui_kit::App,
) -> Result<RoutePlan, RouteFailure> {
    let host = tab
        .host
        .as_ref()
        .ok_or_else(|| RouteFailure::new("this page is not bound to a Host"))?;
    route_plan_for_host(host, url, cx)
}

/// What it takes to load `url` for a Workspace bound to `host`.
pub(crate) fn route_plan_for_host(
    host: &lyrel_core::core::session::RemoteRef,
    url: &str,
    cx: &gpui_kit::App,
) -> Result<RoutePlan, RouteFailure> {
    let loopback = normalized_loopback_url(url)
        .and_then(|url| parse_loopback_url(&url))
        .ok_or_else(|| RouteFailure::new("this loopback address cannot be forwarded"))?;
    let workspace = owning_workspace_for_host(host, cx)?;
    if workspace.shares_localhost() || matches!(&workspace.target, RemoteTarget::LocalStdio { .. })
    {
        return Ok(RoutePlan::Shared {
            origin: loopback.origin(),
        });
    }
    if workspace.spec.is_none() {
        return Err(RouteFailure::new(format!(
            "{} has no SSH connection details; its loopback ports cannot be forwarded",
            workspace
                .label
                .clone()
                .unwrap_or_else(|| workspace.target.to_string())
        )));
    }
    Ok(RoutePlan::Forward {
        route: ForwardRoute::new(0, Some(workspace)),
        loopback,
    })
}

/// Whether `url` is a loopback address the routing rules apply to.
pub(crate) fn is_loopback(url: &str) -> bool {
    super::policy::loopback_origin(url).is_some()
}

fn normalized_loopback_url(address: &str) -> Option<String> {
    let origin = super::policy::loopback_origin(address)?;
    let url = url::Url::parse(address).ok()?;
    Some(format!("{origin}{}", &url[url::Position::BeforePath..]))
}

/// The Host-spelling address for a loopback URL a page asked to load. When
/// the URL is on a previously verified forwarded origin, it is translated
/// back first, so a stale history entry never asks the Host to forward the
/// desktop's own port number.
pub(crate) fn original_loopback_url(url: &str, last: Option<&VerifiedForward>) -> String {
    if let Some(forward) = last
        && let Some(original) = forward.original_url(url)
    {
        return original;
    }
    url.to_string()
}

/// Replaces one origin prefix with another, only at an origin boundary.
fn rewrite_origin(url: &str, from: &str, to: &str) -> Option<String> {
    let at_origin = |url: &str| {
        url.strip_prefix(from)
            .is_some_and(|rest| rest.is_empty() || matches!(rest.as_bytes()[0], b'/' | b'?' | b'#'))
    };
    if at_origin(url) {
        return Some(format!("{to}{}", &url[from.len()..]));
    }
    let normalized = normalized_loopback_url(url)?;
    at_origin(&normalized).then(|| format!("{to}{}", &normalized[from.len()..]))
}

/// Derive the `WKWebsiteDataStore` identifier from the full profile path.
/// Like the Windows profile directory, this scopes the store to both the
/// Host and the Lyrel configuration root, including isolated test instances.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn site_data_identifier(dir: &std::path::Path) -> [u8; 16] {
    // Two FNV-1a passes fill a stable identifier from the native path bytes.
    let path = dir.as_os_str().as_encoded_bytes();
    let first = fnv1a64(path, 0xcbf2_9ce4_8422_2325);
    let second = fnv1a64(path, 0x9e37_79b9_7f4a_7c15);
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&first.to_be_bytes());
    bytes[8..].copy_from_slice(&second.to_be_bytes());
    // RFC 4122 version 4 and variant bits: a well-formed identifier, even
    // though the bytes are deterministic.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes
}

#[cfg(any(target_os = "macos", test))]
fn fnv1a64(bytes: &[u8], seed: u64) -> u64 {
    let mut hash = seed;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::right_panel::WebTab;
    use lyrel_core::core::session::{RemoteRef, RemoteTarget, WorkspaceId};

    fn tab(host: Option<RemoteRef>) -> WebTab {
        WebTab {
            id: "web-test".into(),
            address: "http://localhost:3000/app".into(),
            title: String::new(),
            host,
            source: None,
        }
    }

    fn direct(host: &str) -> RemoteRef {
        RemoteRef::new(
            RemoteTarget::Direct {
                user: "dev".into(),
                host: host.into(),
                port: 22,
            },
            WorkspaceId::new(),
        )
    }

    #[test]
    fn a_forward_maps_both_ways_only_at_an_origin_boundary() {
        let loopback = parse_loopback_url("http://localhost:3000/app?x=1").unwrap();
        let forward = VerifiedForward::from_loopback(&loopback, 53124);
        assert_eq!(forward.original_origin, "http://localhost:3000");
        assert_eq!(forward.forwarded_origin, "http://127.0.0.1:53124");
        assert_eq!(
            forward
                .forwarded_url("http://localhost:3000/app?x=1")
                .as_deref(),
            Some("http://127.0.0.1:53124/app?x=1")
        );
        assert_eq!(
            forward
                .original_url("http://127.0.0.1:53124/app")
                .as_deref(),
            Some("http://localhost:3000/app")
        );
        assert_eq!(
            forward.forwarded_url("http://localhost:30001/app"),
            None,
            "a port is not a prefix"
        );
        assert_eq!(
            forward.forwarded_url("https://localhost:3000/app"),
            None,
            "the scheme is part of the origin"
        );
        assert_eq!(
            forward.forwarded_url("https://example.test/app"),
            None,
            "an external page is not forwarded"
        );
    }

    #[test]
    fn a_changed_port_is_a_new_origin_and_keeps_the_host_spelling() {
        let loopback = parse_loopback_url("http://localhost:3000/").unwrap();
        let first = VerifiedForward::from_loopback(&loopback, 53124);
        let second = VerifiedForward::from_loopback(&loopback, 54001);
        assert_eq!(
            first.original_url("http://127.0.0.1:53124/").as_deref(),
            Some("http://localhost:3000/")
        );
        assert_eq!(
            second.original_url("http://127.0.0.1:54001/").as_deref(),
            Some("http://localhost:3000/")
        );
        assert_eq!(
            second.original_url("http://127.0.0.1:53124/"),
            None,
            "the old origin is not the new route"
        );
        assert_ne!(first.forwarded_origin, second.forwarded_origin);
    }

    #[test]
    fn two_hosts_and_the_local_machine_name_separate_site_data() {
        let local = browser_site_data(&tab(None)).expect("a config dir");
        let alpha = browser_site_data(&tab(Some(direct("alpha.test")))).expect("a config dir");
        let beta = browser_site_data(&tab(Some(direct("beta.test")))).expect("a config dir");
        let same_host = browser_site_data(&tab(Some(direct("alpha.test")))).expect("a config dir");
        assert!(local.dir.ends_with("browser/local"));
        assert_ne!(alpha.dir, local.dir);
        assert_ne!(alpha.dir, beta.dir);
        assert_eq!(alpha.dir, same_host.dir);
        assert_ne!(
            site_data_identifier(&alpha.dir),
            site_data_identifier(&beta.dir)
        );
        assert_eq!(
            site_data_identifier(&alpha.dir),
            site_data_identifier(&same_host.dir)
        );
        let separate_config = std::path::Path::new("other-config")
            .join("browser")
            .join(alpha.dir.file_name().unwrap());
        assert_ne!(
            site_data_identifier(&alpha.dir),
            site_data_identifier(&separate_config)
        );
        let identifier = site_data_identifier(&alpha.dir);
        assert_eq!(identifier[6] >> 4, 4, "a well-formed UUID version");
        assert_eq!(identifier[8] >> 6, 2, "a well-formed UUID variant");
    }

    #[test]
    fn a_stale_forwarded_history_entry_is_translated_back_before_forwarding() {
        let loopback = parse_loopback_url("http://localhost:3000/docs").unwrap();
        let forward = VerifiedForward::from_loopback(&loopback, 53124);
        assert_eq!(
            original_loopback_url("http://127.0.0.1:53124/docs", Some(&forward)),
            "http://localhost:3000/docs"
        );
        assert_eq!(
            original_loopback_url("http://localhost:8080/other", Some(&forward)),
            "http://localhost:8080/other",
            "an ordinary loopback link is its own Host-spelling address"
        );
    }
}
