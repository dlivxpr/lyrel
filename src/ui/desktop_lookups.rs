//! Read-only host/mirror/route/display-label lookups live in `lyrel-ui-base`;
//! this facade keeps `crate::ui::desktop_lookups::` working.

pub use lyrel_ui_base::desktop_lookups::{
    HostChoice, HostLinks, WorkspaceMirror, accepts_input, available_hosts, display_name_for,
    filter_hosts, home_for_host, install_accepts_input, install_pane_route_for, label_from_hosts,
    local_home, pane_ids, pane_route_for, publish_host_mirror, route_label, sweep_wsl,
    target_label,
};
