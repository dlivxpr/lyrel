//! The daemon round trips a [`ForwardRoute`] names. The route value itself —
//! which pane, which workspace — lives in `lyrel_ui_base::route` with the other
//! shared values; this adapter is where the network submission lives, keeping
//! the "workspace request when there is a workspace, pane request otherwise"
//! fallback and the reply interpretation in one terminal-side place.

use lyrel_core::daemon::protocol::{DaemonMsg, LoopbackForward, ManagedForward, SshForwardRule};
use crate::RemoteTerminal;
use lyrel_ui_base::route::ForwardRoute;

/// Submit forward-registry requests for a route. Every method keeps the
/// contract the forwards UI was built on: an unanswered request is not an
/// authoritative empty registry (`None` where a list is the answer), and the
/// daemon's own refusal text travels to the caller unchanged.
pub struct ForwardClient;

impl ForwardClient {
    pub fn list(route: &ForwardRoute) -> Option<Vec<ManagedForward>> {
        let Some(req) = route.workspace_request(lyrel_core::daemon::protocol::WorkspaceOp::ListForwards)
        else {
            return RemoteTerminal::list_forwards(route.pane_id());
        };
        Self::forwards(RemoteTerminal::on_workspace(req))
    }

    pub fn add(route: &ForwardRoute, rule: SshForwardRule) -> Option<Vec<ManagedForward>> {
        let Some(req) = route.workspace_request(lyrel_core::daemon::protocol::WorkspaceOp::AddForward {
            rule: rule.clone(),
        }) else {
            return RemoteTerminal::add_forward(route.pane_id(), rule);
        };
        Self::forwards(RemoteTerminal::on_workspace(req))
    }

    pub fn teardown(route: &ForwardRoute) -> Vec<ManagedForward> {
        let Some(req) =
            route.workspace_request(lyrel_core::daemon::protocol::WorkspaceOp::TeardownForwards)
        else {
            return Vec::new();
        };
        Self::forwards(RemoteTerminal::on_workspace(req)).unwrap_or_default()
    }

    /// Resolve or build the automatic loopback forward for an endpoint.
    pub fn ensure_loopback(
        route: &ForwardRoute,
        remote_host: &str,
        remote_port: u16,
    ) -> anyhow::Result<LoopbackForward> {
        let Some(req) =
            route.workspace_request(lyrel_core::daemon::protocol::WorkspaceOp::EnsureLoopback {
                remote_host: remote_host.to_string(),
                remote_port,
            })
        else {
            return RemoteTerminal::ensure_loopback_forward(
                route.pane_id(),
                remote_host,
                remote_port,
            );
        };
        match RemoteTerminal::on_workspace(req)? {
            DaemonMsg::LoopbackForward(f) => Ok(f),
            other => Err(anyhow::anyhow!(
                "unexpected reply to EnsureLoopback: {other:?}"
            )),
        }
    }

    pub fn set_enabled(
        route: &ForwardRoute,
        forward_id: u64,
        enabled: bool,
    ) -> anyhow::Result<Vec<ManagedForward>> {
        let Some(req) =
            route.workspace_request(lyrel_core::daemon::protocol::WorkspaceOp::SetForwardEnabled {
                forward_id,
                enabled,
            })
        else {
            return RemoteTerminal::set_forward_enabled(route.pane_id(), forward_id, enabled);
        };
        match RemoteTerminal::on_workspace(req)? {
            DaemonMsg::ForwardList(list) => Ok(list),
            other => Err(anyhow::anyhow!(
                "unexpected reply to SetForwardEnabled: {other:?}"
            )),
        }
    }

    pub fn remove(route: &ForwardRoute, forward_id: u64) -> Option<Vec<ManagedForward>> {
        let Some(req) = route
            .workspace_request(lyrel_core::daemon::protocol::WorkspaceOp::RemoveForward { forward_id })
        else {
            return RemoteTerminal::remove_forward(route.pane_id(), forward_id);
        };
        Self::forwards(RemoteTerminal::on_workspace(req))
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
}
