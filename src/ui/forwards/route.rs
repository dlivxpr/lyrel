use crate::daemon::protocol::{
    DaemonMsg, ManagedForward, SshForwardRule, WorkspaceOp, WorkspaceRequest,
};
use crate::terminal::{PaneWorkspace, RemoteTerminal};

/// A forwarding registry target, supplied by the workbench or terminal consumer.
#[derive(Clone, Debug, Default)]
pub(crate) struct ForwardRoute {
    pane_id: u64,
    workspace: Option<PaneWorkspace>,
}

/// Workspace forwards outlive individual panes and share automatic-port claims.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ForwardOwnerKey {
    Pane(u64),
    Workspace(crate::core::session::WorkspaceId),
}

impl ForwardRoute {
    pub(crate) fn new(pane_id: u64, workspace: Option<PaneWorkspace>) -> Self {
        Self { pane_id, workspace }
    }

    pub(crate) fn owner_key(&self) -> ForwardOwnerKey {
        match &self.workspace {
            Some(ws) => ForwardOwnerKey::Workspace(ws.workspace),
            None => ForwardOwnerKey::Pane(self.pane_id),
        }
    }

    fn workspace_op(&self, op: WorkspaceOp) -> Option<WorkspaceRequest> {
        RemoteTerminal::workspace_request(self.workspace.as_ref()?, self.pane_id, op)
    }

    /// An unanswered request is not an authoritative empty registry.
    fn forwards(reply: anyhow::Result<DaemonMsg>) -> Option<Vec<ManagedForward>> {
        match reply {
            Ok(DaemonMsg::ForwardList(list)) => Some(list),
            Ok(other) => {
                log::warn!("unexpected reply to a workspace forward request: {other:?}");
                None
            }
            Err(e) => {
                log::warn!("a workspace forward request failed: {e}");
                None
            }
        }
    }

    pub(crate) fn list(&self) -> Vec<ManagedForward> {
        let Some(req) = self.workspace_op(WorkspaceOp::ListForwards) else {
            return RemoteTerminal::list_forwards(self.pane_id);
        };
        Self::forwards(RemoteTerminal::on_workspace(req)).unwrap_or_default()
    }

    pub(crate) fn add(&self, rule: SshForwardRule) -> Option<Vec<ManagedForward>> {
        let Some(req) = self.workspace_op(WorkspaceOp::AddForward { rule: rule.clone() }) else {
            return RemoteTerminal::add_forward(self.pane_id, rule);
        };
        Self::forwards(RemoteTerminal::on_workspace(req))
    }

    pub(crate) fn teardown(&self) -> Vec<ManagedForward> {
        let Some(req) = self.workspace_op(WorkspaceOp::TeardownForwards) else {
            return Vec::new();
        };
        Self::forwards(RemoteTerminal::on_workspace(req)).unwrap_or_default()
    }

    /// Resolve or build the automatic loopback forward for an endpoint.
    pub(crate) fn ensure_loopback(
        &self,
        remote_host: &str,
        remote_port: u16,
    ) -> anyhow::Result<crate::daemon::protocol::LoopbackForward> {
        let Some(req) = self.workspace_op(WorkspaceOp::EnsureLoopback {
            remote_host: remote_host.to_string(),
            remote_port,
        }) else {
            return RemoteTerminal::ensure_loopback_forward(self.pane_id, remote_host, remote_port);
        };
        match RemoteTerminal::on_workspace(req)? {
            DaemonMsg::LoopbackForward(f) => Ok(f),
            other => Err(anyhow::anyhow!(
                "unexpected reply to EnsureLoopback: {other:?}"
            )),
        }
    }

    pub(crate) fn set_enabled(
        &self,
        forward_id: u64,
        enabled: bool,
    ) -> anyhow::Result<Vec<ManagedForward>> {
        let Some(req) = self.workspace_op(WorkspaceOp::SetForwardEnabled {
            forward_id,
            enabled,
        }) else {
            return RemoteTerminal::set_forward_enabled(self.pane_id, forward_id, enabled);
        };
        match RemoteTerminal::on_workspace(req)? {
            DaemonMsg::ForwardList(list) => Ok(list),
            other => Err(anyhow::anyhow!(
                "unexpected reply to SetForwardEnabled: {other:?}"
            )),
        }
    }

    pub(crate) fn remove(&self, forward_id: u64) -> Option<Vec<ManagedForward>> {
        let Some(req) = self.workspace_op(WorkspaceOp::RemoveForward { forward_id }) else {
            return RemoteTerminal::remove_forward(self.pane_id, forward_id);
        };
        Self::forwards(RemoteTerminal::on_workspace(req))
    }
}
