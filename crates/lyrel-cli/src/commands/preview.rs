use anyhow::{Result, bail};
use lyrel_protocol::core::web;
use lyrel_protocol::daemon::control::{ControlRequest, ReplyOk};
use serde_json::json;

use super::target::fetch_machine;
use super::{Outcome, report};
use crate::address::{self, Context};
use crate::backend::Backend;
use crate::cli::PreviewArgs;
use crate::resolve;

/// One agent's request for a Side Panel preview (#393).
///
/// The command is deliberately a request, not a navigation: the daemon that
/// owns the pane resolves its workspace and Host, and a GUI attached to that
/// workspace reveals the page without moving the terminal keyboard. A build
/// with no such GUI is told so instead of pretending a window consumed it.
/// The screenshot directory is left to the owning daemon, because a request
/// routed with `-m` would otherwise name a directory on the wrong machine.
pub(super) fn preview(
    args: PreviewArgs,
    ctx: &Context,
    backend: &mut dyn Backend,
) -> Result<Outcome> {
    let pane = address::pane_or_context(args.pane.as_deref(), ctx)?;
    let url = args.url.trim().to_string();
    if !web::is_preview_address(&url) {
        bail!(
            "'{}' is not a preview address — pass an absolute http(s) URL (for example \
             http://localhost:5173)",
            args.url
        );
    }

    // Verifying the pane here turns a typo into a clear answer before the
    // request reaches a GUI, and names the workspace in the report.
    let machine = fetch_machine(backend)?;
    let workspace = resolve::workspace_of_pane(&machine, pane)?.id;

    let delivered = match backend.control(ControlRequest::PreviewOpen {
        pane,
        url: url.clone(),
        scratch: String::new(),
    })? {
        ReplyOk::Bool(delivered) => delivered,
        other => bail!("the server answered PreviewOpen with {other:?}"),
    };
    if !delivered {
        bail!(
            "no lyrel window is attached to workspace {workspace} to show the preview — \
             open that workspace and retry"
        );
    }
    report(
        format!("preview requested for %{pane} in workspace {workspace} — {url}"),
        json!({
            "pane": pane,
            "workspace": workspace,
            "url": url,
            "delivered": true,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::execute;
    use crate::commands::test_support::{cli, human, json_of, mock, run_cli};

    #[test]
    fn preview_asks_for_the_panes_own_workspace_and_leaves_the_scratch_to_the_daemon() {
        let mut backend = mock();
        let expected_workspace = backend.machine.workspaces[1].id;
        backend.replies.push_back(ReplyOk::Bool(true));

        let outcome = run_cli(
            &[
                "lyrel",
                "preview",
                "http://localhost:5173/app",
                "--pane",
                "%5",
            ],
            &Context::default(),
            &mut backend,
        );
        let json = json_of(outcome);
        assert_eq!(json["pane"], 5);
        assert_eq!(json["workspace"], expected_workspace.to_string());
        assert_eq!(json["delivered"], true);
        assert_eq!(
            backend.control_calls,
            [
                ControlRequest::MachineGet,
                ControlRequest::PreviewOpen {
                    pane: 5,
                    url: "http://localhost:5173/app".into(),
                    scratch: String::new(),
                },
            ]
        );
    }

    #[test]
    fn a_scheme_the_surface_would_refuse_never_reaches_the_wire() {
        let mut backend = mock();
        let err = execute(
            cli(&["lyrel", "preview", "file:///etc/passwd", "--pane", "%5"]),
            &Context::default(),
            &mut backend,
        )
        .expect_err("a local file is not a preview");
        assert!(err.to_string().contains("absolute http(s) URL"), "{err}");
        assert!(
            backend.control_calls.is_empty(),
            "the request must not be invented for a denied route"
        );
    }

    #[test]
    fn a_preview_nobody_consumed_names_the_missing_window() {
        let mut backend = mock();
        backend.replies.push_back(ReplyOk::Bool(false));
        let err = execute(
            cli(&["lyrel", "preview", "http://localhost:5173", "--pane", "%5"]),
            &Context::default(),
            &mut backend,
        )
        .expect_err("an undelivered preview is not success");
        assert!(err.to_string().contains("no lyrel window"), "{err}");
    }

    #[test]
    fn the_human_report_names_the_pane_and_workspace() {
        let mut backend = mock();
        backend.replies.push_back(ReplyOk::Bool(true));
        let text = human(run_cli(
            &["lyrel", "preview", "https://example.test/x", "--pane", "5"],
            &Context::default(),
            &mut backend,
        ));
        assert!(text.contains("%5"), "{text}");
        assert!(text.contains("https://example.test/x"), "{text}");
    }
}
