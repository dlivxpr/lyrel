//! The daemon round trips an [`SftpRoute`] names. The route value lives in
//! `lyrel_ui_base::route` with the other shared route values; this adapter owns
//! the network submission — workspace request when the workspace carries
//! connection details, pane request otherwise — and the wire-error wording
//! the SFTP panel surfaces unchanged.

use lyrel_core::daemon::protocol::{
    DaemonMsg, SftpEntry, SftpJobProgress, SftpOp, SftpOpResult, SftpTransferSpec, WorkspaceOp,
};
use crate::RemoteTerminal;
use lyrel_ui_base::i18n::{L10nKey, t_fmt};
use lyrel_ui_base::route::SftpRoute;

/// Submit SFTP requests for a route.
pub struct SftpClient;

impl SftpClient {
    pub fn list(route: &SftpRoute, path: &str) -> Result<Vec<SftpEntry>, String> {
        let Some(req) = route.workspace_request(WorkspaceOp::SftpList {
            path: path.to_string(),
        }) else {
            return RemoteTerminal::sftp_list(route.pane_id(), path);
        };
        match RemoteTerminal::on_workspace(req) {
            Ok(DaemonMsg::SftpEntries(e)) => Ok(e),
            Ok(other) => Err(t_fmt(
                L10nKey::SftpErrorUnexpectedReply,
                &[("reply", &format!("{other:?}"))],
            )),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn op(route: &SftpRoute, op: SftpOp) -> SftpOpResult {
        let Some(req) = route.workspace_request(WorkspaceOp::SftpOp { op: op.clone() }) else {
            return RemoteTerminal::sftp_op(route.pane_id(), op);
        };
        match RemoteTerminal::on_workspace(req) {
            Ok(DaemonMsg::SftpOpResult(r)) => r,
            Ok(other) => SftpOpResult::Error(t_fmt(
                L10nKey::SftpErrorUnexpectedReply,
                &[("reply", &format!("{other:?}"))],
            )),
            Err(e) => SftpOpResult::Error(e.to_string()),
        }
    }

    pub fn transfer_start(route: &SftpRoute, spec: SftpTransferSpec) -> Result<u64, String> {
        let Some(req) =
            route.workspace_request(WorkspaceOp::SftpTransferStart { spec: spec.clone() })
        else {
            return RemoteTerminal::sftp_transfer_start(spec);
        };
        match RemoteTerminal::on_workspace(req) {
            Ok(DaemonMsg::SftpTransferStarted { job_id }) => Ok(job_id),
            Ok(other) => Err(t_fmt(
                L10nKey::SftpErrorUnexpectedReply,
                &[("reply", &format!("{other:?}"))],
            )),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn transfer_list(route: &SftpRoute) -> Result<Vec<SftpJobProgress>, String> {
        let Some(req) = route.workspace_request(WorkspaceOp::SftpTransferList) else {
            return RemoteTerminal::sftp_transfer_list(route.pane_id());
        };
        match RemoteTerminal::on_workspace(req) {
            Ok(DaemonMsg::SftpTransferProgress(jobs)) => Ok(jobs),
            Ok(other) => Err(t_fmt(
                L10nKey::SftpErrorUnexpectedReply,
                &[("reply", &format!("{other:?}"))],
            )),
            Err(e) => Err(e.to_string()),
        }
    }
}
