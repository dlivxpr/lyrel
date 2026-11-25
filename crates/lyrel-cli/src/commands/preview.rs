use anyhow::{Result, bail};
use lyrel_protocol::core::web::{self, PreviewTarget};
use lyrel_protocol::daemon::control::{ControlRequest, ReplyOk, feature};
use serde_json::json;

use super::target::fetch_machine;
use super::{Outcome, report};
use crate::address::{self, Context};
use crate::backend::Backend;
use crate::cli::PreviewArgs;
use crate::resolve;

/// One agent's request for a Side Panel preview (#393, #424).
///
/// The command is deliberately a request, not a navigation: the daemon that
/// owns the pane resolves its workspace, working directory, and Host, and a GUI
/// attached to that workspace reveals the page without moving the terminal
/// keyboard. A build with no such GUI is told so instead of pretending a
/// window consumed it. The screenshot directory is left to the owning daemon,
/// because a request routed with `-m` would otherwise name a directory on the
/// wrong machine.
pub(super) fn preview(
    args: PreviewArgs,
    ctx: &Context,
    backend: &mut dyn Backend,
) -> Result<Outcome> {
    let pane = address::pane_or_context(args.pane.as_deref(), ctx)?;
    let target = web::parse_preview_target(&args.url).map_err(anyhow::Error::msg)?;

    let (req_url, req_file, target_label) = match &target {
        PreviewTarget::Url(url) => (url.clone(), None, url.clone()),
        PreviewTarget::HtmlFile(path) => {
            if !backend.hello()?.has_feature(feature::PREVIEW_HTML) {
                bail!(
                    "the lyrel server does not support HTML file previews yet — \
                     update the server and retry"
                );
            }
            (String::new(), Some(path.clone()), path.clone())
        }
    };

    // Verifying the pane here turns a typo into a clear answer before the
    // request reaches a GUI, and names the workspace in the report.
    let machine = fetch_machine(backend)?;
    let workspace = resolve::workspace_of_pane(&machine, pane)?.id;

    let delivered = match backend.control(ControlRequest::PreviewOpen {
        pane,
        url: req_url,
        scratch: String::new(),
        file: req_file,
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
    let payload = match target {
        PreviewTarget::Url(url) => json!({
            "pane": pane,
            "workspace": workspace,
            "url": url,
            "delivered": true,
        }),
        PreviewTarget::HtmlFile(file) => json!({
            "pane": pane,
            "workspace": workspace,
            "file": file,
            "delivered": true,
        }),
    };
    report(
        format!("preview requested for %{pane} in workspace {workspace} — {target_label}"),
        payload,
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
                    file: None,
                },
            ]
        );
    }

    #[test]
    fn preview_accepts_relative_absolute_and_file_uri_html_paths() {
        for (input, expected_file) in [
            ("dist/index.html", "dist/index.html"),
            ("/workspace/page.HTM", "/workspace/page.HTM"),
            (
                "file:///workspace/My%20Site/index.xhtml?v=1#top",
                "/workspace/My Site/index.xhtml",
            ),
            ("file:///C:/Users/dev/index.html", "C:/Users/dev/index.html"),
        ] {
            let mut backend = mock();
            backend.replies.push_back(ReplyOk::Bool(true));
            let outcome = run_cli(
                &["lyrel", "preview", input, "--pane", "%5"],
                &Context::default(),
                &mut backend,
            );
            let json = json_of(outcome);
            assert_eq!(json["file"], expected_file);
            assert_eq!(
                backend.control_calls,
                [
                    ControlRequest::MachineGet,
                    ControlRequest::PreviewOpen {
                        pane: 5,
                        url: String::new(),
                        scratch: String::new(),
                        file: Some(expected_file.into()),
                    },
                ]
            );
        }
    }

    #[test]
    fn non_html_files_and_unsupported_schemes_never_reach_the_wire() {
        for rejected in [
            "file:///etc/passwd",
            ".env",
            "src/main.rs",
            "icon.svg",
            "ftp://example.com/index.html",
            "javascript:alert(1)",
            "data:text/html,<h1>hi</h1>",
            "about:blank",
        ] {
            let mut backend = mock();
            execute(
                cli(&["lyrel", "preview", rejected, "--pane", "%5"]),
                &Context::default(),
                &mut backend,
            )
            .expect_err(rejected);
            assert!(
                backend.control_calls.is_empty(),
                "the request must not be sent for denied target {rejected}"
            );
        }
    }

    #[test]
    fn html_preview_requires_server_feature() {
        let mut backend = mock();
        backend.preview_html_supported = false;
        let err = execute(
            cli(&["lyrel", "preview", "index.html", "--pane", "%5"]),
            &Context::default(),
            &mut backend,
        )
        .expect_err("older server without preview-html-v1 must be refused");
        assert!(err.to_string().contains("update the server"), "{err}");
        assert!(
            backend.control_calls.is_empty(),
            "PreviewOpen must not cross the wire when preview-html-v1 is missing"
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
