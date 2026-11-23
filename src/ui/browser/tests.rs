use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;

use super::{BrowserCommand, BrowserEvent, PageStatus, RouteState, layout_active_web};
use crate::ui::app::test_window::harness_with_tabs;

#[gpui_kit::test]
fn browser_tabs_select_independent_surfaces_and_drop_closed_popups(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let (first, second) = app.update_in(&mut vcx, |app, _, cx| {
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
    vcx.update(|window, _| {
        assert!(window.find(format!("web-tab-{first}")).visible());
        assert!(window.find(format!("web-tab-{second}")).visible());
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

use lyrel_core::core::session::{RemoteRef, RemoteTarget, WorkspaceId};
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
