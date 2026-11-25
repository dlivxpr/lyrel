//! Shell-owned remote links and read-only foundation lookups.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{App, Global};
use lyrel_core::host::{Host as _, HostId, remote::RemoteHost};
use lyrel_ui_base::desktop_lookups::publish_remote_home;
use lyrel_ui_base::host_registry::HostRegistry;

pub use lyrel_ui_base::desktop_lookups::{
    HostChoice, WorkspaceMirror, available_hosts, filter_hosts, home_for_host,
    install_accepts_input, install_pane_route_for, label_from_hosts, local_home, pane_ids,
    pane_route_for, publish_host_mirror, route_label, sweep_wsl, target_label,
};
// Consumed by this package's own tests (the machine mirror suite and the
// whole-window terminal consumers), never by production code.
#[cfg(test)]
pub use lyrel_ui_base::desktop_lookups::display_name_for;

/// A live control link to a remote machine, and the home that machine
/// reported when the link came up.
///
/// The connection logic — establishing, dropping and supervising links —
/// stays in `crate::ui::remote_connect`; this table is only what those
/// transitions publish. Read through `try_global` (never `default_global`)
/// on render paths: an absent table and an empty one are the same answer,
/// and installing a global as a side effect of reading one is not.
#[derive(Default)]
pub struct HostLinks {
    hosts: HashMap<HostId, Arc<RemoteHost>>,
}

impl Global for HostLinks {}

impl HostLinks {
    pub fn get(cx: &mut App, id: HostId) -> Option<Arc<RemoteHost>> {
        cx.default_global::<HostLinks>().hosts.get(&id).cloned()
    }

    /// The home directory the host reported when the link came up — the only
    /// thing that can say what a `~` in one of its paths means (#580). Read
    /// through `try_global` rather than `default_global` so a caller holding
    /// nothing but `&App` (every renderer that draws a path) can ask; an
    /// absent table and an empty one are the same answer here.
    pub fn home(cx: &App, id: HostId) -> Option<PathBuf> {
        if id.is_local() {
            None
        } else {
            home_for_host(cx, id)
        }
    }

    /// Whether the daemon behind this host's link advertised `feature` on its
    /// control hello — the per-host sibling of `local_daemon_supports`. `false`
    /// while no link is up: a caller that cannot see the hello must assume
    /// nothing of the far end, and every feature is gated so that "assume
    /// nothing" degrades to the old behavior rather than breaking.
    pub fn peer_supports(cx: &App, id: HostId, feature: &str) -> bool {
        cx.try_global::<HostLinks>()
            .and_then(|table| table.hosts.get(&id))
            .is_some_and(|host| host.peer().has_feature(feature))
    }

    pub fn insert(cx: &mut App, host: Arc<RemoteHost>, home: PathBuf) {
        let id = host.id();
        HostRegistry::insert(cx, Arc::clone(&host).into_shared());
        cx.default_global::<HostLinks>().hosts.insert(id, host);
        publish_remote_home(cx, id, Some(home));
    }

    pub fn remove(cx: &mut App, id: HostId) {
        cx.default_global::<HostLinks>().hosts.remove(&id);
        publish_remote_home(cx, id, None);
        HostRegistry::remove(cx, id);
        if let Some(service) =
            cx.try_global::<crate::ui::browser::html_preview::HtmlPreviewService>()
        {
            service.purge_host(id);
        }
        if !id.is_local() && cx.has_global::<crate::ui::windows::WindowRegistry>() {
            for (_, weak_app) in crate::ui::windows::WindowRegistry::open_windows(cx) {
                if let Some(app) = weak_app.upgrade() {
                    app.update(cx, |app, cx| {
                        app.on_html_host_disconnected(id, cx);
                    });
                }
            }
        }
    }

    pub fn len(cx: &mut App) -> usize {
        cx.default_global::<HostLinks>().hosts.len()
    }
}
