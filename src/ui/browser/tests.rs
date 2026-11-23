use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;

use super::{BrowserCommand, BrowserEvent, PageStatus, RouteState, layout_active_web};
use crate::ui::app::test_window::harness_with_tabs;

#[gpui_kit::test]
fn browser_tabs_select_independent_surfaces_and_drop_closed_popups(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.simulate_resize(gpui_kit::size(gpui_kit::px(1440.), gpui_kit::px(900.)));
    let (first, second) = app.update_in(&mut vcx, |app, _, cx| {
        app.right_panel_width.set(460.);
        app.open_web_page("https://example.test/first", None, cx);
        let first = layout_active_web(&app.tool_layout()).unwrap();
        app.attach_test_browser_surface(&first, cx);
        app.browser_reload(&first, cx);

        app.on_browser_event(
            &first,
            BrowserEvent::NewWindow {
                url: "https://example.test/second".into(),
            },
            cx,
        );
        let second = layout_active_web(&app.tool_layout()).unwrap();
        assert_ne!(first, second);
        app.attach_test_browser_surface(&second, cx);
        app.browser_forward(&second, cx);
        assert_eq!(
            app.browser_surface_commands(&first),
            [BrowserCommand::Reload]
        );
        assert_eq!(
            app.browser_surface_commands(&second),
            [BrowserCommand::Forward]
        );
        (first, second)
    });
    settle(&mut vcx);
    vcx.update(|window, _| {
        let strip = window.find("side-panel-tool-scroll").bounds();
        let first_tab = window.find(format!("web-tab-{first}"));
        let second_tab = window.find(format!("web-tab-{second}"));
        assert!(strip.contains(&first_tab.bounds().center()));
        assert!(strip.contains(&second_tab.bounds().center()));
        assert_eq!(second_tab.selected(), Some(true));
    });

    app.update_in(&mut vcx, |app, window, cx| {
        app.close_right_panel_web_tab(second.clone(), window, cx);
        app.on_browser_event(
            &second,
            BrowserEvent::NewWindow {
                url: "https://example.test/stale".into(),
            },
            cx,
        );
        assert_eq!(
            layout_active_web(&app.tool_layout()).as_deref(),
            Some(first.as_str())
        );
        assert!(app.browser_surface_commands(&second).is_empty());
    });
}

use std::sync::{Arc, Mutex};

use lyrel_core::core::session::{PreviewSourceState, RemoteRef, RemoteTarget, WorkspaceId};
use lyrel_ui_base::route::ForwardRoute;
use lyrel_ui_base::session::WorkspaceStore;

use super::route::{BrowserForwardVerifier, ForwardVerifier};

/// The agreed test seam: the browser's real verification path, with the
/// daemon's forward replaced by a recording answer. Tests switch the answer
/// between an endpoint, a changed endpoint and a loss.
struct RecordingVerifier {
    answer: Mutex<Result<u16, String>>,
    calls: Mutex<Vec<(String, u16)>>,
}

impl RecordingVerifier {
    fn new(answer: Result<u16, String>) -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(answer),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn answer(&self, answer: Result<u16, String>) {
        *self.answer.lock().unwrap() = answer;
    }

    fn calls(&self) -> Vec<(String, u16)> {
        self.calls.lock().unwrap().clone()
    }
}

impl ForwardVerifier for RecordingVerifier {
    fn verify(
        &self,
        _route: &ForwardRoute,
        remote_host: &str,
        remote_port: u16,
    ) -> Result<u16, String> {
        self.calls
            .lock()
            .unwrap()
            .push((remote_host.to_string(), remote_port));
        self.answer.lock().unwrap().clone()
    }
}

fn remote_host(cx: &mut TestAppContext) -> RemoteRef {
    let target = RemoteTarget::Direct {
        user: "dev".into(),
        host: "remote.test".into(),
        port: 22,
    };
    let workspace = cx.update(|cx| {
        WorkspaceStore::init(cx);
        WorkspaceStore::claim_remote(cx, RemoteRef::new(target.clone(), WorkspaceId::new()))
    });
    RemoteRef::new(target, workspace)
}

fn settle(vcx: &mut gpui_kit::VisualTestContext) {
    for _ in 0..4 {
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.render_frame(cx);
        });
    }
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn a_host_bound_page_verifies_before_load_and_keeps_the_host_address(cx: &mut TestAppContext) {
    let host = remote_host(cx);
    let verifier = RecordingVerifier::new(Ok(53124));
    cx.update(|cx| {
        cx.set_global(BrowserForwardVerifier(verifier.clone()));
    });
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let tab_id = app.update_in(&mut vcx, |app, _, cx| {
        app.open_web_page("http://localhost:3000/app", Some(host.clone()), cx);
        layout_active_web(&app.tool_layout()).expect("the page is selected")
    });
    settle(&mut vcx);

    assert_eq!(
        verifier.calls(),
        [("localhost".to_string(), 3000)],
        "the route is verified through the Host before anything loads"
    );
    assert_eq!(
        app.read_with(&vcx, |app, _| app.browser_surface_commands(&tab_id)),
        [
            BrowserCommand::SetRouteGuard {
                guard: super::surface::RouteGuard::Origin("http://127.0.0.1:53124".into()),
            },
            BrowserCommand::Navigate {
                url: "http://127.0.0.1:53124/app".into(),
            },
        ],
        "the surface only ever sees the verified endpoint"
    );
    app.read_with(&vcx, |app, _| {
        let page = app.right_panel.web_pages.get(&tab_id).expect("a live page");
        assert_eq!(page.display_url, "http://localhost:3000/app");
        assert!(matches!(page.route, RouteState::Verified(_)));
        assert_eq!(
            app.web_tab(&tab_id).expect("the tab").address,
            "http://localhost:3000/app",
            "the original address is the tab's identity"
        );
    });
}

#[gpui_kit::test]
fn a_lost_forward_blocks_the_stale_port_and_reconnects_explicitly(cx: &mut TestAppContext) {
    let host = remote_host(cx);
    let verifier = RecordingVerifier::new(Ok(53124));
    cx.update(|cx| {
        cx.set_global(BrowserForwardVerifier(verifier.clone()));
    });
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let tab_id = app.update_in(&mut vcx, |app, _, cx| {
        app.open_web_page("http://localhost:3000/app", Some(host.clone()), cx);
        layout_active_web(&app.tool_layout()).expect("the page is selected")
    });
    settle(&mut vcx);

    // The forward dies. A reload must not send the old local port to the
    // surface: verification runs first and fails visibly.
    verifier.answer(Err("the workspace is not connected".into()));
    app.update_in(&mut vcx, |app, _, cx| {
        app.on_browser_event(
            &tab_id,
            super::BrowserEvent::LoadFailed {
                url: "http://127.0.0.1:53124/app".into(),
                message: "the forward is gone".into(),
            },
            cx,
        );
        app.browser_reload(&tab_id, cx);
    });
    settle(&mut vcx);
    app.read_with(&vcx, |app, _| {
        let page = app.right_panel.web_pages.get(&tab_id).expect("a live page");
        assert!(
            matches!(page.status, PageStatus::RouteUnverified(_)),
            "a lost forward is an explicit state, not a blank page"
        );
        assert!(matches!(page.route, RouteState::Lost { .. }));
        assert_eq!(page.display_url, "http://localhost:3000/app");
        assert_eq!(
            app.browser_surface_commands(&tab_id)
                .iter()
                .filter(|command| matches!(command, BrowserCommand::Navigate { url } if url.contains("53124")))
                .count(),
            1,
            "the only navigate to the old endpoint is the one made before it was lost"
        );
    });

    // Reconnect is explicit and lands on the new endpoint; the address bar
    // keeps the Host's spelling.
    verifier.answer(Ok(54001));
    app.update_in(&mut vcx, |app, _, cx| app.browser_reconnect(&tab_id, cx));
    settle(&mut vcx);
    let commands = app.read_with(&vcx, |app, _| app.browser_surface_commands(&tab_id));
    assert_eq!(
        commands.last(),
        Some(&BrowserCommand::Navigate {
            url: "http://127.0.0.1:54001/app".into(),
        })
    );
    app.read_with(&vcx, |app, _| {
        let page = app.right_panel.web_pages.get(&tab_id).expect("a live page");
        assert_eq!(page.display_url, "http://localhost:3000/app");
        assert!(matches!(page.route, RouteState::Verified(_)));
    });
    assert_eq!(
        verifier.calls(),
        [
            ("localhost".to_string(), 3000),
            ("localhost".to_string(), 3000),
            ("localhost".to_string(), 3000),
        ],
        "every load and the reconnect verify the route again"
    );
}

// --- Agent previews and explicit feedback (#393) ---

use std::time::{Duration, Instant};

use crate::daemon::protocol::ClientMsg;

fn take_inputs(stream: &mut crate::daemon::transport::Stream) -> Vec<Vec<u8>> {
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let mut inputs = Vec::new();
    loop {
        match ClientMsg::read(stream) {
            Ok(ClientMsg::Input(bytes)) => inputs.push(bytes),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    inputs
}

fn wait_for_inputs(
    vcx: &mut gpui_kit::VisualTestContext,
    stream: &mut crate::daemon::transport::Stream,
    what: &str,
) -> Vec<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        vcx.background_executor.run_until_parked();
        let inputs = take_inputs(stream);
        if !inputs.is_empty() {
            return inputs;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: no prompt reached the pane"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui_kit::test]
fn an_agent_preview_reveals_and_selects_without_moving_terminal_focus(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    settle(&mut vcx);
    let scratch = tempfile::tempdir().unwrap();
    let scratch = scratch.path().to_string_lossy().into_owned();
    let (focused, first) = app.update_in(&mut vcx, |app, window, cx| {
        app.focus_active(window, cx);
        let focused = window.focused(cx).expect("terminal content owns focus");
        app.reveal_agent_preview(app.workspace, 1, "http://localhost:5173/app", &scratch, cx)
            .expect("a local preview is eligible");
        let first = layout_active_web(&app.tool_layout()).expect("the preview is selected");
        assert!(app.right_panel_visible, "the panel reveals for the preview");
        (focused, first)
    });
    assert_eq!(
        vcx.update(|window, cx| window.focused(cx)),
        Some(focused),
        "revealing a preview never takes the terminal keyboard"
    );
    app.read_with(&vcx, |app, _| {
        let tab = app.web_tab(&first).expect("the tab exists");
        assert_eq!(tab.address, "http://localhost:5173/app");
        assert_eq!(
            tab.source,
            Some(PreviewSourceState {
                workspace: app.workspace,
                pane: 1,
                scratch: scratch.clone(),
            })
        );
    });

    // Move the Side Panel to a tool, then have the same agent pane request
    // again: the existing preview is selected, not merely rewritten in place.
    app.update_in(&mut vcx, |app, window, cx| {
        app.select_panel_tab(
            &crate::ui::right_panel::PanelTab::Tool(lyrel_core::core::config::RightPanelTab::Info),
            window,
            cx,
        );
    });
    app.read_with(&vcx, |app, _| {
        assert!(
            matches!(
                app.tool_layout().active,
                crate::ui::right_panel::PanelTab::Tool(_)
            ),
            "the test really left the preview"
        );
    });

    // A second request from the same agent pane reuses its preview tab and
    // moves that page rather than piling up tabs.
    app.update_in(&mut vcx, |app, _window, cx| {
        app.reveal_agent_preview(app.workspace, 1, "http://localhost:5173/next", &scratch, cx)
            .expect("the same pane may move its preview");
    });
    app.read_with(&vcx, |app, _| {
        let layout = app.tool_layout();
        let web: Vec<_> = layout
            .order
            .iter()
            .filter_map(|entry| match entry {
                crate::ui::right_panel::PanelTab::Web(tab) => Some(tab.id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(web, [first.clone()], "one preview tab per agent pane");
        assert!(
            matches!(&layout.active, crate::ui::right_panel::PanelTab::Web(tab) if tab.id == first),
            "a repeated request selects the existing preview"
        );
        assert!(layout.open, "the panel stays revealed");
        assert_eq!(
            app.web_tab(&first).expect("the tab").address,
            "http://localhost:5173/next"
        );
    });
}

#[gpui_kit::test]
fn an_explicit_share_writes_the_screenshot_and_prompts_the_original_pane(cx: &mut TestAppContext) {
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
    settle(&mut vcx);
    let scratch = tempfile::tempdir().unwrap();
    let tab = app.update_in(&mut vcx, |app, _window, cx| {
        app.reveal_agent_preview(
            app.workspace,
            1,
            "http://localhost:5173/app",
            &scratch.path().to_string_lossy(),
            cx,
        )
        .expect("the preview opens");
        let tab = layout_active_web(&app.tool_layout()).expect("selected");
        app.attach_test_browser_surface(&tab, cx);
        tab
    });
    let request = app.update_in(&mut vcx, |app, window, cx| {
        app.browser_preview_feedback_open(&tab, window, cx);
        let input = app.right_panel.web_pages[&tab]
            .feedback
            .as_ref()
            .expect("the composer exists")
            .input
            .clone();
        input.update(cx, |state, cx| {
            state.set_value("make the hero blue", window, cx);
        });
        app.browser_preview_feedback_send(&tab, window, cx);
        app.right_panel.web_pages[&tab]
            .feedback
            .as_ref()
            .expect("the composer exists")
            .pending_capture
            .as_ref()
            .expect("the explicit send asked for a screenshot")
            .request
    });
    assert!(
        app.read_with(&vcx, |app, _| app.browser_surface_commands(&tab))
            .contains(&BrowserCommand::Capture { request }),
        "the share captures through the surface command path"
    );
    let inputs = wait_for_inputs(&mut vcx, &mut streams[0], "the share");
    let prompt = String::from_utf8(inputs.concat()).expect("the prompt is UTF-8");
    assert!(prompt.contains("make the hero blue"), "{prompt}");
    assert!(prompt.contains("Screenshot (PNG)"), "{prompt}");
    assert!(prompt.contains("preview-1-"), "{prompt}");

    // The bytes the native surface answers with are what reach the Host.
    let written = std::fs::read_dir(scratch.path())
        .expect("the scratch directory exists")
        .filter_map(Result::ok)
        .find(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
        .expect("the screenshot was written")
        .path();
    assert_eq!(
        std::fs::read(&written).unwrap(),
        super::surface::test_support::CAPTURED_PNG,
        "the file is the captured PNG, not a re-encoded copy"
    );
    let written = written.to_string_lossy().into_owned();
    assert!(prompt.contains(&written), "the pane is told the exact path");
    app.read_with(&vcx, |app, _| {
        let feedback = app.right_panel.web_pages[&tab]
            .feedback
            .as_ref()
            .expect("the composer stays open");
        assert!(feedback.error.is_none(), "{:?}", feedback.error);
        assert!(feedback.notice.is_some(), "the sent state is visible");
    });
}

#[gpui_kit::test]
fn a_stale_capture_answer_never_consumes_a_newer_share(cx: &mut TestAppContext) {
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
    settle(&mut vcx);
    let scratch = tempfile::tempdir().unwrap();
    let scratch_path = scratch.path().to_string_lossy().into_owned();
    let (tab, first, second) = app.update_in(&mut vcx, |app, window, cx| {
        app.reveal_agent_preview(
            app.workspace,
            1,
            "http://localhost:5173/app",
            &scratch_path,
            cx,
        )
        .expect("the preview opens");
        let tab = layout_active_web(&app.tool_layout()).expect("selected");
        app.attach_test_browser_surface(&tab, cx);
        app.browser_preview_feedback_open(&tab, window, cx);
        let input = app.right_panel.web_pages[&tab]
            .feedback
            .as_ref()
            .expect("the composer exists")
            .input
            .clone();
        input.update(cx, |state, cx| state.set_value("first", window, cx));
        app.browser_preview_feedback_send(&tab, window, cx);
        let first = app.right_panel.web_pages[&tab]
            .feedback
            .as_ref()
            .expect("the composer exists")
            .pending_capture
            .as_ref()
            .expect("the first share is waiting")
            .request;
        app.browser_preview_feedback_cancel(&tab, cx);
        app.browser_preview_feedback_open(&tab, window, cx);
        input.update(cx, |state, cx| state.set_value("second", window, cx));
        app.browser_preview_feedback_send(&tab, window, cx);
        let second = app.right_panel.web_pages[&tab]
            .feedback
            .as_ref()
            .expect("the composer exists")
            .pending_capture
            .as_ref()
            .expect("the second share is waiting")
            .request;
        assert_ne!(first, second);
        (tab, first, second)
    });

    app.update_in(&mut vcx, |app, _window, cx| {
        app.preview_capture_ready(
            &tab,
            first,
            super::surface::test_support::CAPTURED_PNG.to_vec(),
            cx,
        );
        let feedback = app.right_panel.web_pages[&tab]
            .feedback
            .as_ref()
            .expect("the composer exists");
        assert!(feedback.pending, "the newer share is still waiting");
        assert_eq!(
            feedback.pending_capture.as_ref().map(|job| job.request),
            Some(second),
            "a stale answer must not consume the newer pending request"
        );
        assert!(feedback.error.is_none(), "{:?}", feedback.error);
    });

    // Nothing was delivered by the stale or the cancelled request.
    app.update_in(&mut vcx, |app, _window, cx| {
        app.browser_preview_feedback_cancel(&tab, cx);
    });
    settle(&mut vcx);
    assert!(
        take_inputs(&mut streams[0]).is_empty(),
        "neither cancelled request reached the pane"
    );
}

#[gpui_kit::test]
fn a_cancelled_share_never_reaches_the_pane_after_its_write(cx: &mut TestAppContext) {
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
    settle(&mut vcx);
    let scratch = tempfile::tempdir().unwrap();
    let scratch_path = scratch.path().to_string_lossy().into_owned();
    let (tab, request) = app.update_in(&mut vcx, |app, window, cx| {
        app.reveal_agent_preview(
            app.workspace,
            1,
            "http://localhost:5173/app",
            &scratch_path,
            cx,
        )
        .expect("the preview opens");
        let tab = layout_active_web(&app.tool_layout()).expect("selected");
        app.attach_test_browser_surface(&tab, cx);
        app.browser_preview_feedback_open(&tab, window, cx);
        let input = app.right_panel.web_pages[&tab]
            .feedback
            .as_ref()
            .expect("the composer exists")
            .input
            .clone();
        input.update(cx, |state, cx| state.set_value("cancel me", window, cx));
        app.browser_preview_feedback_send(&tab, window, cx);
        let request = app.right_panel.web_pages[&tab]
            .feedback
            .as_ref()
            .expect("the composer exists")
            .pending_capture
            .as_ref()
            .expect("the share is waiting")
            .request;
        app.preview_capture_ready(
            &tab,
            request,
            super::surface::test_support::CAPTURED_PNG.to_vec(),
            cx,
        );
        assert!(
            app.right_panel.web_pages[&tab]
                .feedback
                .as_ref()
                .expect("the composer exists")
                .pending,
            "the request stays pending through the Host write"
        );
        app.browser_preview_feedback_cancel(&tab, cx);
        (tab, request)
    });
    assert!(request > 0);

    // Wait for the write itself, then give the landing callback a chance to
    // run: it must find the share cancelled and deliver nothing.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        vcx.background_executor.run_until_parked();
        let wrote = std::fs::read_dir(scratch.path())
            .map(|entries| {
                entries.filter_map(Result::ok).any(|entry| {
                    entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension == "png")
                })
            })
            .unwrap_or(false);
        if wrote {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the screenshot write never finished"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..20 {
        vcx.background_executor.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        take_inputs(&mut streams[0]).is_empty(),
        "a cancelled share never reaches the pane, even after its screenshot was written"
    );
    app.read_with(&vcx, |app, _| {
        let feedback = app.right_panel.web_pages[&tab]
            .feedback
            .as_ref()
            .expect("the composer stays open");
        assert!(!feedback.pending, "nothing is left waiting");
        assert!(
            feedback.notice.is_none(),
            "a cancelled share never reports success"
        );
        assert!(
            feedback.error.is_none(),
            "a cancelled share is not a failure: {:?}",
            feedback.error
        );
    });
}

#[gpui_kit::test]
fn a_restored_preview_has_no_sendable_source(cx: &mut TestAppContext) {
    use crate::ui::right_panel::PanelTab;
    use lyrel_core::core::config::RightPanelTab;
    use lyrel_core::core::config::RightPanelTab::Info;
    use lyrel_core::core::session::{
        TabEntryState, TabSelectionState, ToolLayoutState, WebTabState, WindowView, WindowViews,
    };

    let workspace = WorkspaceId::new();
    let state = ToolLayoutState {
        version: ToolLayoutState::CURRENT_VERSION,
        order: vec![
            TabEntryState::Tool(Info),
            TabEntryState::Web(WebTabState {
                id: "web-preview".into(),
                address: "http://localhost:5173/app".into(),
                host: None,
                title: String::new(),
            }),
        ],
        active: TabSelectionState::Web("web-preview".into()),
        last_used: RightPanelTab::Info,
        open: true,
    };
    cx.update(|cx| {
        WorkspaceStore::install_for_test(
            cx,
            WindowViews {
                views: vec![WindowView {
                    id: workspace,
                    tool_layout: Some(state),
                    ..WindowView::default()
                }],
                active: Some(workspace),
            },
        );
    });
    let (app, mut vcx) = crate::ui::app::test_window::harness_on_workspace(cx, workspace);
    settle(&mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        let layout = app.tool_layout();
        assert!(
            matches!(&layout.active, PanelTab::Web(tab) if tab.id == "web-preview"),
            "the tab and its address restore"
        );
        let tab = app.web_tab("web-preview").expect("the restored preview");
        assert_eq!(tab.address, "http://localhost:5173/app");
        assert!(
            tab.source.is_none(),
            "a restored preview has no remembered pane to send to"
        );
        assert!(
            app.agent_preview_source_label(&tab, cx).is_none(),
            "no source chip is shown without a live request"
        );
        app.browser_preview_feedback_open("web-preview", window, cx);
        assert!(
            app.right_panel
                .web_pages
                .get("web-preview")
                .and_then(|page| page.feedback.as_ref())
                .is_none(),
            "a restored preview offers no share composer"
        );
        app.browser_preview_feedback_send("web-preview", window, cx);
        assert!(
            app.browser_surface_commands("web-preview").is_empty(),
            "a restored preview never captures anything"
        );
    });
}

#[gpui_kit::test]
fn a_missing_source_pane_refuses_the_share_visibly(cx: &mut TestAppContext) {
    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
    settle(&mut vcx);
    let scratch = tempfile::tempdir().unwrap();
    let tab = app.update_in(&mut vcx, |app, _window, cx| {
        app.reveal_agent_preview(
            app.workspace,
            9,
            "http://localhost:5173/app",
            &scratch.path().to_string_lossy(),
            cx,
        )
        .expect("the preview itself may open");
        let tab = layout_active_web(&app.tool_layout()).expect("selected");
        app.attach_test_browser_surface(&tab, cx);
        tab
    });
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(
            app.agent_preview_source_label(&app.web_tab(&tab).unwrap(), cx)
                .is_some_and(|label| label.contains("closed") || label.contains("已关闭")),
            "the closed source is visible before any click"
        );
        app.browser_preview_feedback_open(&tab, window, cx);
        app.browser_preview_feedback_send(&tab, window, cx);
        let feedback = app.right_panel.web_pages[&tab]
            .feedback
            .as_ref()
            .expect("the composer exists");
        assert!(feedback.error.is_some(), "the refusal is visible");
        assert!(!feedback.pending, "nothing is left waiting");
        assert!(
            app.browser_surface_commands(&tab).is_empty(),
            "a share with no live source never reaches the capture command"
        );
    });
    assert!(
        take_inputs(&mut streams[0]).is_empty(),
        "the missing pane receives nothing"
    );
}

#[gpui_kit::test]
fn a_remote_preview_inherits_the_windows_verified_host_binding(cx: &mut TestAppContext) {
    lyrel_ui_base::config::pin_test_config_dir();
    cx.update(|cx| WorkspaceStore::init(cx));
    let target = RemoteTarget::Direct {
        user: "dev".into(),
        host: "remote.test".into(),
        port: 22,
    };
    let view = cx.update(|cx| {
        WorkspaceStore::claim_remote(cx, RemoteRef::new(target.clone(), WorkspaceId::new()))
    });
    let host = cx
        .update(|cx| WorkspaceStore::remote_ref(cx, view))
        .expect("the claimed view keeps its route");
    let (app, mut vcx) = crate::ui::app::test_window::harness_on_workspace(cx, view);
    let scratch = tempfile::tempdir().unwrap();
    let tab = app.update_in(&mut vcx, |app, _window, cx| {
        app.reveal_agent_preview(
            host.workspace,
            7,
            "http://localhost:3000/app",
            &scratch.path().to_string_lossy(),
            cx,
        )
        .expect("a remote preview resolves through the window's own route");
        layout_active_web(&app.tool_layout()).expect("selected")
    });
    app.read_with(&vcx, |app, _| {
        let tab = app.web_tab(&tab).expect("the preview tab");
        assert_eq!(
            tab.host.as_ref(),
            Some(&host),
            "the event can never substitute another Host"
        );
        assert_eq!(
            tab.source.expect("the source identity").workspace,
            host.workspace
        );
    });
}

#[gpui_kit::test]
fn a_preview_url_that_breaks_the_surface_policy_never_opens(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let err = app
        .update_in(&mut vcx, |app, _window, cx| {
            app.reveal_agent_preview(
                app.workspace,
                1,
                "file:///etc/passwd",
                "/tmp/lyrel/previews",
                cx,
            )
        })
        .expect_err("a local file is not a preview");
    assert!(err.contains("http(s)"), "{err}");
    app.read_with(&vcx, |app, _| {
        assert_eq!(
            app.tool_layout()
                .order
                .iter()
                .filter(|entry| matches!(entry, crate::ui::right_panel::PanelTab::Web(_)))
                .count(),
            0,
            "the denied route never grew a tab"
        );
    });
}
