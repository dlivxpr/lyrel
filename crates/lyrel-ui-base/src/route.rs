//! Route values shared across the desktop shell: what a pane's workspace is,
//! how a pane reaches it, and the pane/workspace binding a forwarding or SFTP
//! request is assembled from.
//!
//! Everything here is a pure value or pure request assembly: no `TerminalView`,
//! no `RemoteTerminal`, no shell controller. Network submission lives with the
//! consumers — the terminal-side forward client and SFTP client adapters own
//! the daemon round trips these routes name.

use lyrel_protocol::daemon::protocol::{WorkspaceOp, WorkspaceRequest};

/// The workspace a pane belongs to and how its daemon is reached — the value
/// a pane's title, relink and route questions are answered from. Built by the
/// remote-workspace seam, which is the one that may ask the network and the
/// keychain; everything downstream only reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneWorkspace {
    pub workspace: lyrel_protocol::core::session::WorkspaceId,
    pub target: lyrel_protocol::core::session::RemoteTarget,
    pub spec: Option<Box<lyrel_protocol::daemon::protocol::NativeSshSpec>>,
    /// The name the workspace answers to — its own label, or its machine's
    /// when it has none. A pane keeps answering to this name when its link
    /// dies (#438 gave SSH panes their host here; without this, a workspace
    /// pane fell back to the bare app name and the tab read "lyrel —
    /// disconnected").
    pub label: Option<String>,
    /// Whether the daemon serving this workspace's panes echoes a `Size` frame
    /// when it applies a resize — read off the host's control hello by whoever
    /// built this value, because this module may not ask the network itself.
    /// `false` whenever the answer is unknown (link down, server too old to
    /// advertise), which keeps the safe reflow-at-request behavior.
    pub resize_echo: bool,
}

impl PaneWorkspace {
    pub fn shares_localhost(&self) -> bool {
        matches!(
            self.target,
            lyrel_protocol::core::session::RemoteTarget::Wsl { .. }
        )
    }

    pub fn route_header(&self) -> anyhow::Result<lyrel_protocol::daemon::router::RouteHeader> {
        use lyrel_protocol::core::session::RemoteTarget;
        use lyrel_protocol::daemon::router::RouteHeader;
        let header = match (&self.target, &self.spec) {
            (RemoteTarget::Wsl { distro }, _) => RouteHeader::wsl(distro.clone()),
            (RemoteTarget::LocalStdio { program, args }, _) => {
                let mut argv: Vec<&str> = args.iter().map(String::as_str).collect();
                if !argv.contains(&"--pane") {
                    argv.push("--pane");
                }
                RouteHeader::local_stdio(program.clone(), &argv)
            }
            (_, Some(spec)) => RouteHeader::ssh((**spec).clone()),
            (_, None) => {
                // Deliberately not the target: a `Profile` spells itself as
                // its config UUID in `Display` and in `Debug` alike, and a
                // deleted profile is exactly what empties `spec` here. This
                // sentence is not only logged — `land_pane` hands it to the
                // pending pane, which prints the reason verbatim under
                // "could not reach {machine}", so the UUID reached the screen
                // (#485). The workspace's own name is what every other
                // surface calls this thing.
                return Err(match self.label.as_deref() {
                    Some(label) => anyhow::anyhow!(
                        "{label} has no SSH connection details, so its panes cannot be routed"
                    ),
                    None => anyhow::anyhow!(
                        "this workspace has no SSH connection details, so its panes \
                         cannot be routed"
                    ),
                });
            }
        };
        Ok(header.for_pane())
    }

    /// Assemble the workspace-scoped request `op` names, or `None` when this
    /// workspace carries no connection details to send it over — the caller
    /// then falls back to the pane-level request for `view_pane`.
    pub fn workspace_request(&self, view_pane: u64, op: WorkspaceOp) -> Option<WorkspaceRequest> {
        Some(WorkspaceRequest {
            workspace: self.workspace,
            spec: self.spec.clone()?,
            view_pane,
            op,
        })
    }
}

/// How a pane's daemon is reached: not at all (local), over a routed header,
/// or not reachably. The route travels with the pane so a relink — which
/// rebuilds it — re-answers the resize-echo question too.
#[derive(Clone, Debug, Default)]
pub enum PaneRoute {
    #[default]
    Local,
    Remote {
        header: Box<lyrel_protocol::daemon::router::RouteHeader>,
        /// Whether the daemon at the far end of this route echoes a `Size`
        /// frame when it applies a resize, per its host's control hello (see
        /// [`PaneWorkspace::resize_echo`]). Riding on the route puts the
        /// answer everywhere a route is assigned — spawn, attach, relink —
        /// and a relink builds its route fresh, so a reconnect re-answers it.
        resize_echo: bool,
    },
    Unroutable(String),
}

impl PaneRoute {
    pub fn for_workspace(workspace: Option<&PaneWorkspace>) -> PaneRoute {
        match workspace {
            None => PaneRoute::Local,
            Some(ws) => match ws.route_header() {
                Ok(header) => PaneRoute::Remote {
                    header: Box::new(header),
                    resize_echo: ws.resize_echo,
                },
                Err(e) => PaneRoute::Unroutable(e.to_string()),
            },
        }
    }

    pub fn header(&self) -> Option<&lyrel_protocol::daemon::router::RouteHeader> {
        match self {
            PaneRoute::Remote { header, .. } => Some(header),
            PaneRoute::Local | PaneRoute::Unroutable(_) => None,
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, PaneRoute::Local)
    }

    pub fn allow_remote_clipboard_write(&self) -> bool {
        match self {
            PaneRoute::Remote { header, .. } => match &header.target {
                lyrel_protocol::daemon::router::RouteTarget::Ssh(spec) => {
                    spec.remote_clipboard_write
                }
                _ => false,
            },
            PaneRoute::Local | PaneRoute::Unroutable(_) => false,
        }
    }
}

/// A forwarding registry target: the pane a request starts from and,
/// for a workspace pane, the workspace whose daemon serves it. Pure binding —
/// the daemon round trips run through the terminal-side forward client.
#[derive(Clone, Debug, Default)]
pub struct ForwardRoute {
    pane_id: u64,
    workspace: Option<PaneWorkspace>,
}

/// Workspace forwards outlive individual panes and share automatic-port claims.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ForwardOwnerKey {
    Pane(u64),
    Workspace(lyrel_protocol::core::session::WorkspaceId),
}

impl ForwardRoute {
    pub fn new(pane_id: u64, workspace: Option<PaneWorkspace>) -> Self {
        Self { pane_id, workspace }
    }

    pub fn owner_key(&self) -> ForwardOwnerKey {
        match &self.workspace {
            Some(ws) => ForwardOwnerKey::Workspace(ws.workspace),
            None => ForwardOwnerKey::Pane(self.pane_id),
        }
    }

    pub fn pane_id(&self) -> u64 {
        self.pane_id
    }

    /// The workspace-scoped request for `op`, or `None` when this is a plain
    /// pane route or the workspace has no connection details — the caller
    /// addresses the pane itself then.
    pub fn workspace_request(&self, op: WorkspaceOp) -> Option<WorkspaceRequest> {
        self.workspace.as_ref()?.workspace_request(self.pane_id, op)
    }
}

/// The SFTP browsing/transfers route for one pane: which pane's connection
/// the ops run on, and — for a workspace pane — the workspace request they
/// can be addressed with instead. Pure binding; the round trips live in the
/// terminal-side SFTP client.
#[derive(Clone, Debug)]
pub struct SftpRoute {
    pane_id: u64,
    workspace: Option<PaneWorkspace>,
}

impl SftpRoute {
    pub fn new(pane_id: u64, workspace: Option<PaneWorkspace>) -> Self {
        Self { pane_id, workspace }
    }

    /// What makes this route *this* route, for a host id. Keyed by pane —
    /// two panes into the same server are two routes, because every op is
    /// addressed to a pane's SSH connection — and by workspace, because a
    /// workspace pane's id was minted by the far daemon and can collide with
    /// a local one.
    pub fn connection_key(&self) -> String {
        match &self.workspace {
            Some(ws) => format!("sftp:{}:{}", ws.workspace, self.pane_id),
            None => format!("sftp:{}", self.pane_id),
        }
    }

    pub fn pane_id(&self) -> u64 {
        self.pane_id
    }

    pub fn workspace(&self) -> Option<&PaneWorkspace> {
        self.workspace.as_ref()
    }

    /// The workspace-scoped request for `op`, or `None` when the op has to be
    /// addressed to the pane itself.
    pub fn workspace_request(&self, op: WorkspaceOp) -> Option<WorkspaceRequest> {
        self.workspace.as_ref()?.workspace_request(self.pane_id, op)
    }
}
