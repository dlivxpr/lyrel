use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use lyrel_core::core::config::RightPanelTab;
use lyrel_core::core::session::{
    RemoteRef, RemoteTarget, TabEntryState, TabSelectionState, ToolLayoutState, WebTabState,
    WindowView, WindowViews, WorkspaceId,
};
use lyrel_core::host::local::LocalHost;
use lyrel_core::host::{
    ContentLimits, ContentQuery, ContentResults, Entry, Host, HostId, Meta, Output, SearchHit,
    SharedHost, ShellInventory, WatchSub,
};
use lyrel_ui_base::host_registry::HostRegistry;
use lyrel_ui_base::i18n::{L10nKey, t};
use lyrel_ui_base::session::WorkspaceStore;

use super::html_preview::HtmlPreviewService;
use super::surface::RouteGuard;
use super::{BrowserCommand, BrowserEvent, PageStatus, policy};
use crate::ui::app::LyrelApp;
use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::file_tree::FileTreeEvent;

struct RemotePosixTestHost {
    id: HostId,
    root: PathBuf,
    local: SharedHost,
    connected: AtomicBool,
}

impl RemotePosixTestHost {
    fn shared(id: HostId, root: PathBuf) -> SharedHost {
        let canonical_root = std::fs::canonicalize(&root)
            .map(lyrel_core::core::path_spelling::local_spelling_buf)
            .unwrap_or(root);
        Arc::new(Self {
            id,
            root: canonical_root,
            local: LocalHost::new(),
            connected: AtomicBool::new(true),
        })
    }

    fn new_disconnected(id: HostId, root: PathBuf) -> SharedHost {
        let canonical_root = LocalHost::new().canonicalize(&root).unwrap_or(root);
        Arc::new(Self {
            id,
            root: canonical_root,
            local: LocalHost::new(),
            connected: AtomicBool::new(false),
        })
    }

    fn to_disk(&self, path: &Path) -> PathBuf {
        let raw = path.to_string_lossy();
        let mut out = self.root.clone();
        for seg in raw
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
        {
            out.push(seg);
        }
        out
    }

    fn to_wire(&self, disk: &Path) -> io::Result<PathBuf> {
        let rel = disk
            .strip_prefix(&self.root)
            .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "outside root"))?;
        let mut out = String::from("/");
        for (ix, comp) in rel.components().enumerate() {
            if ix > 0 {
                out.push('/');
            }
            out.push_str(&comp.as_os_str().to_string_lossy());
        }
        Ok(PathBuf::from(out))
    }
}

impl Host for RemotePosixTestHost {
    fn id(&self) -> HostId {
        self.id
    }

    fn separator(&self) -> char {
        '/'
    }

    fn is_absolute(&self, p: &Path) -> bool {
        p.to_string_lossy().starts_with('/')
    }

    fn read_dir(&self, dir: &Path, root: Option<&Path>) -> io::Result<Vec<Entry>> {
        self.local
            .read_dir(&self.to_disk(dir), root.map(|r| self.to_disk(r)).as_deref())
    }

    fn stat(&self, p: &Path) -> io::Result<Meta> {
        self.local.stat(&self.to_disk(p))
    }

    fn read_file(&self, p: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
        self.local.read_file(&self.to_disk(p), max_bytes)
    }

    fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
        let disk = self.local.canonicalize(&self.to_disk(p))?;
        self.to_wire(&disk)
    }

    fn search(
        &self,
        _roots: &[PathBuf],
        _query: &str,
        _limit: usize,
        _max_dirs: usize,
        _show_hidden: bool,
    ) -> io::Result<Vec<SearchHit>> {
        Ok(Vec::new())
    }

    fn search_content(
        &self,
        _roots: &[PathBuf],
        _query: &ContentQuery,
        _limits: &ContentLimits,
    ) -> io::Result<ContentResults> {
        Ok(ContentResults::default())
    }

    fn write_file(&self, p: &Path, bytes: &[u8]) -> io::Result<Meta> {
        self.local.write_file(&self.to_disk(p), bytes)
    }

    fn create_file_new(&self, p: &Path) -> io::Result<()> {
        self.local.create_file_new(&self.to_disk(p))
    }

    fn create_dir(&self, p: &Path, recursive: bool) -> io::Result<()> {
        self.local.create_dir(&self.to_disk(p), recursive)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.local.rename(&self.to_disk(from), &self.to_disk(to))
    }

    fn remove(&self, p: &Path, recursive: bool) -> io::Result<()> {
        self.local.remove(&self.to_disk(p), recursive)
    }

    fn repo_root(&self, _p: &Path) -> io::Result<Option<PathBuf>> {
        Ok(None)
    }

    fn git(&self, _cwd: &Path, _args: &[&str]) -> io::Result<Output> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "no git"))
    }

    fn shells(&self) -> io::Result<ShellInventory> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "no shells"))
    }

    fn watch(&self, _dirs: &[PathBuf]) -> io::Result<WatchSub> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "no watch"))
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
}

fn settle(vcx: &mut VisualTestContext) {
    for _ in 0..4 {
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.render_frame(cx);
        });
    }
    vcx.run_until_parked();
}

fn wait_until(
    vcx: &mut VisualTestContext,
    stage: &str,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        settle(vcx);
        if done(vcx) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {stage}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn wait_for_commands(
    app: &Entity<LyrelApp>,
    vcx: &mut VisualTestContext,
    tab_id: &str,
    min_len: usize,
) -> Vec<BrowserCommand> {
    let mut commands = Vec::new();
    wait_until(vcx, "browser surface commands", |vcx| {
        commands = app.read_with(vcx, |app, _| app.browser_surface_commands(tab_id));
        commands.len() >= min_len
    });
    commands
}

fn wait_for_reload_count(
    app: &Entity<LyrelApp>,
    vcx: &mut VisualTestContext,
    tab_id: &str,
    min_reloads: usize,
) -> Vec<BrowserCommand> {
    let mut commands = Vec::new();
    wait_until(vcx, "browser reload commands", |vcx| {
        commands = app.read_with(vcx, |app, _| app.browser_surface_commands(tab_id));
        commands
            .iter()
            .filter(|cmd| matches!(cmd, BrowserCommand::Reload))
            .count()
            >= min_reloads
    });
    commands
}

fn attach_quiet_tab(
    app: &Entity<LyrelApp>,
    vcx: &mut VisualTestContext,
) -> crate::daemon::transport::Stream {
    use crate::ui::pane::{Pane, PaneSlot};
    use lyrel_terminal::view::quiet_test_pane;

    app.update_in(vcx, |app, window, cx| {
        let (view, stream) = quiet_test_pane(1, window, cx);
        LyrelApp::watch_terminal(&view, window, cx);
        app.tabs
            .push(crate::ui::tab::Tab::new(Pane::leaf(PaneSlot::Ready(view))));
        app.tabs.select(0);
        cx.notify();
        stream
    })
}

fn http_get(url: &str) -> (u16, String) {
    let rest = url.strip_prefix("http://").expect("http url");
    let (host_port, path) = rest.split_once('/').expect("url path");
    let addr: SocketAddr = host_port.parse().expect("socket addr");
    let mut stream = TcpStream::connect(addr).expect("connect");
    let req = format!("GET /{path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    stream.flush().unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..split]);
    let status: u16 = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let body = String::from_utf8_lossy(&raw[split + 4..]).into_owned();
    (status, body)
}

#[gpui_kit::test]
fn file_scheme_remains_denied_while_local_html_preview_renders_via_loopback(
    cx: &mut TestAppContext,
) {
    assert_eq!(policy::classify("file:///etc/passwd"), policy::Route::Deny);
    assert_eq!(
        policy::classify("file:///C:/Users/dev/index.html"),
        policy::Route::Deny
    );

    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let site = lyrel_core::core::path_spelling::local_spelling_buf(
        std::fs::canonicalize(dir.path()).unwrap(),
    );
    let html_file = site.join("index.html");
    let sub_dir = site.join("sub");
    std::fs::create_dir_all(&sub_dir).unwrap();
    let sibling_file = sub_dir.join("about.html");
    std::fs::write(&html_file, "<h1>Local Preview</h1>").unwrap();
    std::fs::write(&sibling_file, "<h1>About</h1>").unwrap();

    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let clean_path = lyrel_core::core::path_spelling::spelling_on(HostId::LOCAL, &html_file)
        .to_string_lossy()
        .into_owned();

    let tab_id = app.update_in(&mut vcx, |app, _window, cx| {
        app.open_html_preview(HostId::LOCAL, &html_file, cx);
        app.active_browser_detail()
            .expect("html preview is raised")
            .to_string()
    });

    let cmds = wait_for_commands(&app, &mut vcx, &tab_id, 2);
    let (mount_origin, mount_url) = match &cmds[..] {
        [
            BrowserCommand::SetRouteGuard {
                guard: RouteGuard::Origin(origin),
            },
            BrowserCommand::Navigate { url },
        ] => (origin.clone(), url.clone()),
        other => panic!("unexpected surface commands: {other:?}"),
    };
    assert!(mount_url.starts_with(&format!("{mount_origin}/p/")));
    assert!(mount_url.ends_with("/index.html"));

    // Clean path stays in address bar and title; never leaks 127.0.0.1:<port>/p/<token>.
    app.read_with(&vcx, |app, cx| {
        let tab = app.web_tab(&tab_id).unwrap();
        assert_eq!(tab.html_path.as_deref(), Some(clean_path.as_str()));
        let page = &app.right_panel.web_pages[&tab_id];
        assert_eq!(page.display_url, clean_path);
        assert!(!page.display_title(&tab).contains("127.0.0.1"));
        assert!(!page.display_title(&tab).contains("/p/"));
        let service = cx.global::<HtmlPreviewService>();
        assert!(service.is_running());
        assert_eq!(service.active_mount_count(), 1);
    });

    // Navigating inside the mount (e.g. relative link to sub/about.html) maps cleanly back to path.
    let prefix = mount_url.rsplit_once('/').unwrap().0;
    let sibling_mount_url = format!("{prefix}/sub/about.html");
    app.update_in(&mut vcx, |app, _window, cx| {
        app.on_browser_event(
            &tab_id,
            BrowserEvent::Navigated {
                url: sibling_mount_url,
                can_back: true,
                can_forward: false,
            },
            cx,
        );
    });
    let expected_sibling =
        lyrel_core::core::path_spelling::spelling_on(HostId::LOCAL, &sibling_file)
            .to_string_lossy()
            .into_owned();
    app.read_with(&vcx, |app, _| {
        let page = &app.right_panel.web_pages[&tab_id];
        assert_eq!(page.display_url, expected_sibling);
        assert_eq!(page.status, PageStatus::Ready);
    });

    // Reloading stays on the linked document, including its native history.
    let reload_commands = app.update_in(&mut vcx, |app, _, cx| {
        app.browser_reload(&tab_id, cx);
        app.browser_surface_commands(&tab_id)
    });
    assert_eq!(reload_commands.last(), Some(&BrowserCommand::Reload));

    // Opening the same HTML file again focuses and reloads the existing tab (no duplicate tab).
    app.update_in(&mut vcx, |app, _window, cx| {
        app.open_html_preview(HostId::LOCAL, &html_file, cx);
    });
    let cmds_after_reopen = wait_for_commands(&app, &mut vcx, &tab_id, reload_commands.len() + 3);
    assert!(matches!(
        &cmds_after_reopen[cmds_after_reopen.len() - 1],
        BrowserCommand::Navigate { url } if url.ends_with("/index.html")
    ));
    app.read_with(&vcx, |app, _| {
        assert_eq!(app.tool_layout().web_tabs.len(), 1);
        assert_eq!(app.right_panel.web_pages[&tab_id].display_url, clean_path);
    });

    // Navigating away via address bar to an http(s) URL drops html_mount and shuts down server.
    app.update_in(&mut vcx, |app, window, cx| {
        app.focus_browser_address(window, cx);
        let input = app.browser_chrome.as_ref().unwrap().input.clone();
        input.update(cx, |state, cx| {
            state.set_value("https://example.com", window, cx);
        });
    });
    vcx.update(|window, cx| window.press("enter", cx));
    settle(&mut vcx);

    app.read_with(&vcx, |app, cx| {
        let tab = app.web_tab(&tab_id).unwrap();
        assert_eq!(tab.html_path, None);
        assert_eq!(tab.address, "https://example.com");
        let page = &app.right_panel.web_pages[&tab_id];
        assert!(page.html_mount.is_none());
        assert_eq!(page.display_url, "https://example.com");
        assert!(!cx.global::<HtmlPreviewService>().is_running());
    });
}

#[gpui_kit::test]
fn closing_html_preview_tab_releases_mount_and_stops_server(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let html_file = dir.path().join("demo.htm");
    std::fs::write(&html_file, "<p>hello</p>").unwrap();

    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let tab_id = app.update_in(&mut vcx, |app, _window, cx| {
        app.open_html_preview(HostId::LOCAL, &html_file, cx);
        app.active_browser_detail().unwrap().to_string()
    });
    wait_for_commands(&app, &mut vcx, &tab_id, 2);

    assert!(cx.update(|cx| cx.global::<HtmlPreviewService>().is_running()));

    app.update_in(&mut vcx, |app, window, cx| {
        app.close_right_panel_web_tab(tab_id.clone(), window, cx);
    });
    settle(&mut vcx);

    cx.update(|cx| {
        let service = cx.global::<HtmlPreviewService>();
        assert!(!service.is_running());
        assert_eq!(service.active_mount_count(), 0);
    });
}

#[gpui_kit::test]
fn remote_html_preview_and_system_browser_actions_work_and_purge_on_disconnect(
    cx: &mut TestAppContext,
) {
    lyrel_ui_base::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let remote_dir = tempfile::tempdir().unwrap();
    let project = remote_dir.path().join("workspace");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("preview.xhtml"), "<html/>").unwrap();

    let target = RemoteTarget::Direct {
        user: "dev".into(),
        host: "remote.test".into(),
        port: 22,
    };
    let remote_ref = RemoteRef::new(target.clone(), WorkspaceId::new());
    let host_id = remote_ref.host_id();
    let remote_host = RemotePosixTestHost::shared(host_id, remote_dir.path().to_path_buf());

    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let client_ws = cx.update(|cx| {
        WorkspaceStore::init(cx);
        HostRegistry::insert(cx, remote_host);
        let id = WorkspaceStore::claim_remote(cx, remote_ref.clone());
        WorkspaceStore::claim(cx, Some(id))
    });
    app.update_in(&mut vcx, |app, _window, cx| {
        app.workspace = client_ws;
        app.adopt_tool_layout(client_ws, cx);
    });
    let remote_path = PathBuf::from("/workspace/preview.xhtml");

    let tab_id = app.update_in(&mut vcx, |app, _window, cx| {
        app.open_html_preview(host_id, &remote_path, cx);
        app.active_browser_detail().unwrap().to_string()
    });

    let cmds = wait_for_commands(&app, &mut vcx, &tab_id, 2);
    assert!(matches!(
        &cmds[1],
        BrowserCommand::Navigate { url } if url.ends_with("/preview.xhtml")
    ));
    app.read_with(&vcx, |app, _| {
        let tab = app.web_tab(&tab_id).unwrap();
        assert_eq!(tab.host.as_ref(), Some(&remote_ref));
        assert_eq!(tab.html_path.as_deref(), Some("/workspace/preview.xhtml"));
        assert_eq!(
            app.right_panel.web_pages[&tab_id].display_url,
            "/workspace/preview.xhtml"
        );
        assert_eq!(app.tool_layout().active, RightPanelTab::Browser);
    });

    // Browser toolbar Open in System Browser mounts an external URL and invokes cx.open_url.
    app.update_in(&mut vcx, |app, _window, cx| {
        app.browser_open_external(&tab_id, cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let opened = loop {
        settle(&mut vcx);
        if let Some(url) = cx.opened_url() {
            break url;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for external browser URL"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert!(opened.starts_with("http://127.0.0.1:"));
    assert!(opened.ends_with("/preview.xhtml"));
    let (status, body) = http_get(&opened);
    assert_eq!(status, 200);
    assert_eq!(body, "<html/>");

    // HostLinks::remove purges remote mounts immediately (and stops the server when none remain).
    cx.update(|cx| {
        crate::ui::desktop_lookups::HostLinks::remove(cx, host_id);
        let service = cx.global::<HtmlPreviewService>();
        assert_eq!(service.active_mount_count(), 0);
        assert!(!service.is_running());
    });
}

#[gpui_kit::test]
fn persisted_html_preview_layout_stores_only_html_path_and_host_never_loopback(
    cx: &mut TestAppContext,
) {
    lyrel_ui_base::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let local_dir = tempfile::tempdir().unwrap();
    let local_html = LocalHost::new()
        .canonicalize(local_dir.path())
        .unwrap()
        .join("local.html");
    std::fs::write(&local_html, "<h1>Local</h1>").unwrap();

    let remote_dir = tempfile::tempdir().unwrap();
    let remote_proj = remote_dir.path().join("site");
    std::fs::create_dir_all(&remote_proj).unwrap();
    std::fs::write(remote_proj.join("remote.html"), "<h1>Remote</h1>").unwrap();

    let target = RemoteTarget::Direct {
        user: "dev".into(),
        host: "persist.test".into(),
        port: 22,
    };
    let remote_ref = RemoteRef::new(target, WorkspaceId::new());
    let host_id = remote_ref.host_id();
    let remote_host = RemotePosixTestHost::shared(host_id, remote_dir.path().to_path_buf());

    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    cx.update(|cx| {
        WorkspaceStore::init(cx);
        HostRegistry::insert(cx, remote_host);
    });
    let (local_id, remote_id) = app.update_in(&mut vcx, |app, _window, cx| {
        app.open_html_preview(HostId::LOCAL, &local_html, cx);
        let local_id = app.active_browser_detail().unwrap().to_string();
        let remote_ws = WorkspaceStore::claim_remote(cx, remote_ref.clone());
        let client_ws = WorkspaceStore::claim(cx, Some(remote_ws));
        app.workspace = client_ws;
        app.adopt_tool_layout(client_ws, cx);
        app.open_html_preview(host_id, Path::new("/site/remote.html"), cx);
        let remote_id = app.active_browser_detail().unwrap().to_string();
        (local_id, remote_id)
    });
    wait_for_commands(&app, &mut vcx, &remote_id, 2);

    let json = app.read_with(&vcx, |app, cx| {
        let state = app.tool_layout().to_state();
        let serialized = serde_json::to_string(&state).unwrap();
        assert!(!serialized.contains("127.0.0.1"));
        assert!(!serialized.contains("/p/"));
        assert!(serialized.contains("/site/remote.html"));
        assert!(serialized.contains("persist.test"));
        let view = WorkspaceStore::all(cx)
            .get(app.workspace)
            .and_then(|v| v.tool_layout.as_ref())
            .cloned()
            .expect("persisted layout");
        let view_json = serde_json::to_string(&view).unwrap();
        assert!(!view_json.contains("127.0.0.1"));
        assert!(!view_json.contains("/p/"));
        let _ = local_id;
        serialized
    });
    assert!(json.contains("html_path"));
}

#[gpui_kit::test]
fn restored_html_preview_lazily_mounts_and_recovers_missing_file_via_retry(
    cx: &mut TestAppContext,
) {
    lyrel_ui_base::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let site = LocalHost::new().canonicalize(dir.path()).unwrap();
    let html_file = site.join("restored.html");
    std::fs::write(&html_file, "<h1>Restored</h1>").unwrap();
    let clean_path = lyrel_core::core::path_spelling::spelling_on(HostId::LOCAL, &html_file)
        .to_string_lossy()
        .into_owned();

    let workspace = WorkspaceId::new();
    let tab_id = "web-restored-html".to_string();
    let state = ToolLayoutState {
        version: ToolLayoutState::CURRENT_VERSION,
        order: vec![
            TabEntryState::Tool(RightPanelTab::Info),
            TabEntryState::Tool(RightPanelTab::Browser),
            TabEntryState::Web(WebTabState {
                id: tab_id.clone(),
                address: clean_path.clone(),
                host: None,
                title: "restored.html".into(),
                html_path: Some(clean_path.clone()),
            }),
        ],
        active: TabSelectionState::Tool(RightPanelTab::Browser),
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
    let _stream = attach_quiet_tab(&app, &mut vcx);
    settle(&mut vcx);

    // Restoring does not eagerly start the server or mount anything until the tab is prepared.
    cx.update(|cx| {
        let service = cx.default_global::<HtmlPreviewService>();
        assert!(!service.is_running());
        assert_eq!(service.active_mount_count(), 0);
    });

    // Remove the file before selecting the restored tab -> missing-file overlay + Retry button.
    std::fs::remove_file(&html_file).unwrap();
    app.update_in(&mut vcx, |app, window, cx| {
        app.reveal_browser_page(&tab_id, window, cx);
    });
    wait_until(&mut vcx, "missing html overlay", |vcx| {
        app.read_with(vcx, |app, _| {
            app.right_panel
                .web_pages
                .get(&tab_id)
                .is_some_and(|page| matches!(page.status, PageStatus::Failed(_)))
        })
    });
    app.read_with(&vcx, |app, _| {
        let page = &app.right_panel.web_pages[&tab_id];
        assert_eq!(
            page.status,
            PageStatus::Failed(t(L10nKey::BrowserHtmlMissing).to_string())
        );
        assert_eq!(page.display_url, clean_path);
        assert!(page.html_mount.is_none());
    });
    vcx.update(|window, _| {
        assert!(window.find(format!("browser-overlay-{tab_id}")).visible());
        assert!(
            window
                .find(format!("browser-overlay-retry-{tab_id}"))
                .visible()
        );
        assert!(
            window
                .try_find(format!("browser-overlay-open-{tab_id}"))
                .is_none()
        );
    });

    // Restore the file and click Retry -> lazily mounts and navigates without leaking loopback URL.
    std::fs::write(&html_file, "<h1>Back Online</h1>").unwrap();
    vcx.update(|window, cx| {
        window.click(format!("browser-overlay-retry-{tab_id}"), cx);
    });
    let cmds = wait_for_commands(&app, &mut vcx, &tab_id, 2);
    let mount_url = match &cmds[1] {
        BrowserCommand::Navigate { url } => url.clone(),
        other => panic!("expected Navigate, got {other:?}"),
    };
    assert!(mount_url.ends_with("/restored.html"));
    let (status, body) = http_get(&mount_url);
    assert_eq!(status, 200);
    assert_eq!(body, "<h1>Back Online</h1>");
    app.read_with(&vcx, |app, _| {
        let page = &app.right_panel.web_pages[&tab_id];
        assert_eq!(page.display_url, clean_path);
        assert!(page.html_mount.is_some());
    });
}

#[gpui_kit::test]
fn restored_remote_html_preview_shows_disconnected_overlay_and_recovers_on_reconnect(
    cx: &mut TestAppContext,
) {
    lyrel_ui_base::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let remote_dir = tempfile::tempdir().unwrap();
    let proj = remote_dir.path().join("app");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("index.html"), "<h1>Remote Restored</h1>").unwrap();

    let target = RemoteTarget::Direct {
        user: "dev".into(),
        host: "restore-remote.test".into(),
        port: 22,
    };
    let remote_ref = RemoteRef::new(target, WorkspaceId::new());
    let host_id = remote_ref.host_id();
    let workspace = WorkspaceId::new();
    let tab_id = "web-restored-remote".to_string();
    let state = ToolLayoutState {
        version: ToolLayoutState::CURRENT_VERSION,
        order: vec![
            TabEntryState::Tool(RightPanelTab::Info),
            TabEntryState::Tool(RightPanelTab::Browser),
            TabEntryState::Web(WebTabState {
                id: tab_id.clone(),
                address: "/app/index.html".into(),
                host: Some(remote_ref.clone()),
                title: "index.html".into(),
                html_path: Some("/app/index.html".into()),
            }),
        ],
        active: TabSelectionState::Tool(RightPanelTab::Browser),
        last_used: RightPanelTab::Info,
        open: true,
    };
    cx.update(|cx| {
        WorkspaceStore::install_for_test(
            cx,
            WindowViews {
                views: vec![WindowView {
                    id: workspace,
                    host: Some(remote_ref),
                    tool_layout: Some(state),
                    ..WindowView::default()
                }],
                active: Some(workspace),
            },
        );
    });
    let (app, mut vcx) = crate::ui::app::test_window::harness_on_workspace(cx, workspace);
    let _stream = attach_quiet_tab(&app, &mut vcx);
    settle(&mut vcx);

    // First select with Host absent from HostRegistry -> disconnected overlay + Reconnect button.
    app.update_in(&mut vcx, |app, window, cx| {
        app.reveal_browser_page(&tab_id, window, cx);
    });
    settle(&mut vcx);
    app.read_with(&vcx, |app, _| {
        let page = &app.right_panel.web_pages[&tab_id];
        assert_eq!(
            page.status,
            PageStatus::RouteUnverified(t(L10nKey::BrowserHtmlHostDisconnected).to_string())
        );
        assert_eq!(page.display_url, "/app/index.html");
        assert!(page.html_mount.is_none());
    });
    vcx.update(|window, _| {
        assert!(
            window
                .find(format!("browser-overlay-reconnect-{tab_id}"))
                .visible()
        );
        assert!(
            window
                .try_find(format!("browser-overlay-open-{tab_id}"))
                .is_none()
        );
    });

    // Also test a Host registered in HostRegistry whose is_connected() == false.
    let disconnected_host =
        RemotePosixTestHost::new_disconnected(host_id, remote_dir.path().to_path_buf());
    cx.update(|cx| HostRegistry::insert(cx, disconnected_host));
    vcx.update(|window, cx| {
        window.click(format!("browser-overlay-reconnect-{tab_id}"), cx);
    });
    settle(&mut vcx);
    app.read_with(&vcx, |app, _| {
        assert_eq!(
            app.right_panel.web_pages[&tab_id].status,
            PageStatus::RouteUnverified(t(L10nKey::BrowserHtmlHostDisconnected).to_string())
        );
    });

    // Now connect the real Host and click Reconnect -> mounts and loads cleanly.
    let live_host = RemotePosixTestHost::shared(host_id, remote_dir.path().to_path_buf());
    cx.update(|cx| HostRegistry::insert(cx, live_host));
    vcx.update(|window, cx| {
        window.click(format!("browser-overlay-reconnect-{tab_id}"), cx);
    });
    let cmds = wait_for_commands(&app, &mut vcx, &tab_id, 2);
    let mount_url = match &cmds[1] {
        BrowserCommand::Navigate { url } => url.clone(),
        other => panic!("expected Navigate, got {other:?}"),
    };
    let (status, body) = http_get(&mount_url);
    assert_eq!(status, 200);
    assert_eq!(body, "<h1>Remote Restored</h1>");
    app.read_with(&vcx, |app, _| {
        let page = &app.right_panel.web_pages[&tab_id];
        assert_eq!(page.display_url, "/app/index.html");
        assert!(page.html_mount.is_some());
    });
}

#[gpui_kit::test]
fn remote_host_disconnect_marks_html_preview_disconnected_and_reconnect_remounts_on_new_host(
    cx: &mut TestAppContext,
) {
    lyrel_ui_base::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let remote_dir_v1 = tempfile::tempdir().unwrap();
    let proj_v1 = remote_dir_v1.path().join("workspace");
    std::fs::create_dir_all(&proj_v1).unwrap();
    std::fs::write(proj_v1.join("live.html"), "<p>v1</p>").unwrap();

    let remote_dir_v2 = tempfile::tempdir().unwrap();
    let proj_v2 = remote_dir_v2.path().join("workspace");
    std::fs::create_dir_all(&proj_v2).unwrap();
    std::fs::write(proj_v2.join("live.html"), "<p>v2</p>").unwrap();

    let target = RemoteTarget::Direct {
        user: "dev".into(),
        host: "reconnect.test".into(),
        port: 22,
    };
    let remote_ref = RemoteRef::new(target, WorkspaceId::new());
    let host_id = remote_ref.host_id();
    let host_v1 = RemotePosixTestHost::shared(host_id, remote_dir_v1.path().to_path_buf());

    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let client_ws = cx.update(|cx| {
        WorkspaceStore::init(cx);
        HostRegistry::insert(cx, host_v1);
        let id = WorkspaceStore::claim_remote(cx, remote_ref.clone());
        WorkspaceStore::claim(cx, Some(id))
    });
    app.update_in(&mut vcx, |app, window, cx| {
        app.workspace = client_ws;
        app.adopt_tool_layout(client_ws, cx);
        let weak = cx.entity().downgrade();
        crate::ui::windows::WindowRegistry::register(cx, client_ws, window.window_handle(), weak);
    });

    let remote_path = PathBuf::from("/workspace/live.html");
    let tab_id = app.update_in(&mut vcx, |app, _window, cx| {
        app.open_html_preview(host_id, &remote_path, cx);
        app.active_browser_detail().unwrap().to_string()
    });
    let cmds = wait_for_commands(&app, &mut vcx, &tab_id, 2);
    let v1_url = match &cmds[1] {
        BrowserCommand::Navigate { url } => url.clone(),
        other => panic!("expected Navigate, got {other:?}"),
    };
    assert_eq!(http_get(&v1_url).1, "<p>v1</p>");

    // Also create an external mount to confirm HostLinks::remove purges both.
    app.update_in(&mut vcx, |app, _window, cx| {
        app.browser_open_external(&tab_id, cx);
    });
    wait_until(&mut vcx, "external mount", |vcx| {
        vcx.update(|_, cx| cx.global::<HtmlPreviewService>().active_mount_count() >= 2)
    });

    // Disconnect the remote Host via HostLinks::remove.
    cx.update(|cx| {
        crate::ui::desktop_lookups::HostLinks::remove(cx, host_id);
        let service = cx.global::<HtmlPreviewService>();
        assert_eq!(service.active_mount_count(), 0);
        assert!(!service.is_running());
    });
    settle(&mut vcx);

    app.read_with(&vcx, |app, _| {
        let page = &app.right_panel.web_pages[&tab_id];
        assert!(page.html_mount.is_none());
        assert_eq!(
            page.status,
            PageStatus::RouteUnverified(t(L10nKey::BrowserHtmlHostDisconnected).to_string())
        );
        assert_eq!(page.display_url, "/workspace/live.html");
        let cmds = app.browser_surface_commands(&tab_id);
        assert_eq!(
            cmds.last(),
            Some(&BrowserCommand::SetRouteGuard {
                guard: RouteGuard::DenyLoopback,
            })
        );
    });
    vcx.update(|window, _| {
        assert!(
            window
                .find(format!("browser-overlay-reconnect-{tab_id}"))
                .visible()
        );
    });

    // Reconnect with a new SharedHost instance -> clicking Reconnect mounts fresh token on v2.
    let host_v2 = RemotePosixTestHost::shared(host_id, remote_dir_v2.path().to_path_buf());
    cx.update(|cx| HostRegistry::insert(cx, host_v2));
    vcx.update(|window, cx| {
        window.click(format!("browser-overlay-reconnect-{tab_id}"), cx);
    });
    let cmds_after = wait_for_commands(&app, &mut vcx, &tab_id, 5);
    let v2_url = match &cmds_after[cmds_after.len() - 1] {
        BrowserCommand::Navigate { url } => url.clone(),
        other => panic!("expected Navigate, got {other:?}"),
    };
    assert_ne!(v1_url, v2_url);
    assert_eq!(http_get(&v2_url).1, "<p>v2</p>");
    app.read_with(&vcx, |app, _| {
        let page = &app.right_panel.web_pages[&tab_id];
        assert!(page.html_mount.is_some());
        assert_eq!(page.display_url, "/workspace/live.html");
    });
}

#[gpui_kit::test]
fn remote_html_preview_departure_keeps_webpage_live_and_restores_saved_file(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = Path::new("/index.html");
    std::fs::write(dir.path().join("index.html"), "<h1>Starting preview</h1>").unwrap();
    let remote = RemoteRef::new(
        RemoteTarget::Direct {
            user: "dev".into(),
            host: "departure.test".into(),
            port: 22,
        },
        WorkspaceId::new(),
    );
    let host_id = remote.host_id();
    let host = RemotePosixTestHost::shared(host_id, dir.path().to_path_buf());
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let workspace = cx.update(|cx| {
        WorkspaceStore::init(cx);
        HostRegistry::insert(cx, host.clone());
        let id = WorkspaceStore::claim_remote(cx, remote);
        WorkspaceStore::claim(cx, Some(id))
    });
    let tab_id = app.update_in(&mut vcx, |app, window, cx| {
        app.workspace = workspace;
        app.adopt_tool_layout(workspace, cx);
        let weak = cx.entity().downgrade();
        crate::ui::windows::WindowRegistry::register(cx, workspace, window.window_handle(), weak);
        app.open_html_preview(host_id, path, cx);
        app.active_browser_detail().unwrap().to_string()
    });
    let commands = wait_for_commands(&app, &mut vcx, &tab_id, 2);
    let initial_url = match &commands[1] {
        BrowserCommand::Navigate { url } => url,
        other => panic!("expected HTML preview navigation, got {other:?}"),
    };
    assert_eq!(http_get(initial_url).1, "<h1>Starting preview</h1>");

    let site = "https://example.test/redirect-target";
    app.update_in(&mut vcx, |app, _, cx| {
        app.on_browser_event(
            &tab_id,
            BrowserEvent::Navigated {
                url: "about:blank".into(),
                can_back: false,
                can_forward: false,
            },
            cx,
        );
        assert_eq!(
            app.right_panel.web_pages[&tab_id].display_url,
            "/index.html"
        );
        assert_eq!(
            app.right_panel.web_pages[&tab_id].status,
            PageStatus::Starting
        );
        app.on_browser_event(
            &tab_id,
            BrowserEvent::LoadFailed {
                url: "https://example.test/unavailable".into(),
                message: "server unavailable".into(),
            },
            cx,
        );
        assert_eq!(
            app.right_panel.web_pages[&tab_id].display_url,
            "/index.html"
        );
        assert_eq!(cx.global::<HtmlPreviewService>().active_mount_count(), 1);
        app.on_browser_event(
            &tab_id,
            BrowserEvent::Navigated {
                url: site.into(),
                can_back: true,
                can_forward: false,
            },
            cx,
        );
    });
    settle(&mut vcx);
    app.read_with(&vcx, |app, cx| {
        let tab = app.web_tab(&tab_id).unwrap();
        assert_eq!(tab.address, "/index.html");
        assert_eq!(tab.html_path.as_deref(), Some("/index.html"));
        assert_eq!(
            app.browser_chrome
                .as_ref()
                .unwrap()
                .input
                .read(cx)
                .value()
                .as_str(),
            site
        );
        assert!(!cx.global::<HtmlPreviewService>().is_running());
        assert_eq!(
            app.browser_surface_commands(&tab_id).last(),
            Some(&BrowserCommand::SetRouteGuard {
                guard: RouteGuard::DenyLoopback,
            })
        );
    });

    cx.update(|cx| crate::ui::desktop_lookups::HostLinks::remove(cx, host_id));
    app.update_in(&mut vcx, |app, _, cx| {
        assert_eq!(app.right_panel.web_pages[&tab_id].status, PageStatus::Ready);
        app.browser_reload(&tab_id, cx);
        assert_eq!(
            app.browser_surface_commands(&tab_id).last(),
            Some(&BrowserCommand::Reload)
        );
        app.browser_open_external(&tab_id, cx);
    });
    assert_eq!(cx.opened_url().as_deref(), Some(site));

    cx.update(|cx| HostRegistry::insert(cx, host));
    let (restored, mut restored_vcx) =
        crate::ui::app::test_window::open_window_on(cx, Some(workspace));
    let _stream = attach_quiet_tab(&restored, &mut restored_vcx);
    restored.update_in(&mut restored_vcx, |app, window, cx| {
        app.reveal_browser_page(&tab_id, window, cx);
    });
    let commands = wait_for_commands(&restored, &mut restored_vcx, &tab_id, 2);
    let restored_url = match &commands[1] {
        BrowserCommand::Navigate { url } => url,
        other => panic!("expected restored HTML preview navigation, got {other:?}"),
    };
    assert_eq!(http_get(restored_url).1, "<h1>Starting preview</h1>");

    let min_commands = app.update_in(&mut vcx, |app, _, cx| {
        let commands = app.browser_surface_commands(&tab_id).len();
        app.open_html_preview(host_id, path, cx);
        assert_eq!(app.active_browser_detail(), Some(tab_id.as_str()));
        assert_eq!(app.tool_layout().web_tabs.len(), 1);
        commands + 3
    });
    let commands = wait_for_commands(&app, &mut vcx, &tab_id, min_commands);
    let reopened_url = match commands.last().unwrap() {
        BrowserCommand::Navigate { url } => url,
        other => panic!("expected reopened HTML preview navigation, got {other:?}"),
    };
    assert_eq!(http_get(reopened_url).1, "<h1>Starting preview</h1>");
}

#[gpui_kit::test]
fn html_preview_auto_reloads_on_editor_save_and_fs_events(cx: &mut TestAppContext) {
    lyrel_ui_base::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let site = LocalHost::new().canonicalize(dir.path()).unwrap();
    let html_file = site.join("auto.html");
    let other_file = site.join("other.html");
    std::fs::write(&html_file, "<p>initial</p>").unwrap();
    std::fs::write(&other_file, "<p>other</p>").unwrap();

    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let tab_id = app.update_in(&mut vcx, |app, _window, cx| {
        app.open_html_preview(HostId::LOCAL, &html_file, cx);
        app.active_browser_detail().unwrap().to_string()
    });
    let cmds = wait_for_commands(&app, &mut vcx, &tab_id, 2);
    let mount_url = match &cmds[1] {
        BrowserCommand::Navigate { url } => url.clone(),
        other => panic!("expected Navigate, got {other:?}"),
    };
    app.update_in(&mut vcx, |app, _window, cx| {
        app.on_browser_event(
            &tab_id,
            BrowserEvent::Navigated {
                url: mount_url.clone(),
                can_back: false,
                can_forward: false,
            },
            cx,
        );
    });
    settle(&mut vcx);

    // Open the same HTML file in the built-in editor, modify it, and save it.
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&html_file, window, cx);
    });
    let editor_input = {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            vcx.run_until_parked();
            if let Some(file) = app.read_with(&vcx, |app, cx| app.editor_active_local_file(cx)) {
                break file.into_input();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "editor file load timeout"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    app.update_in(&mut vcx, |app, window, cx| {
        editor_input.update(cx, |input, cx| {
            input.set_value("<p>edited-in-editor</p>", window, cx);
        });
        app.editor_save_active(window, cx);
    });
    let cmds_after_save = wait_for_reload_count(&app, &mut vcx, &tab_id, 1);
    assert_eq!(cmds_after_save.last(), Some(&BrowserCommand::Reload));
    assert_eq!(http_get(&mount_url).1, "<p>edited-in-editor</p>");

    // Unrelated file or directory FsEvents must NOT trigger reload.
    let reload_count_before_unrelated = cmds_after_save
        .iter()
        .filter(|c| matches!(c, BrowserCommand::Reload))
        .count();
    app.update_in(&mut vcx, |app, window, cx| {
        app.on_file_tree_event(
            &FileTreeEvent::FsEvents {
                host: HostId::LOCAL,
                paths: HashSet::from([other_file.clone(), site.clone()]),
            },
            window,
            cx,
        );
    });
    settle(&mut vcx);
    assert_eq!(
        app.read_with(&vcx, |app, _| {
            app.browser_surface_commands(&tab_id)
                .iter()
                .filter(|c| matches!(c, BrowserCommand::Reload))
                .count()
        }),
        reload_count_before_unrelated
    );

    let webpage = "https://example.test/current";
    app.update_in(&mut vcx, |app, window, cx| {
        app.on_browser_event(
            &tab_id,
            BrowserEvent::Navigated {
                url: webpage.into(),
                can_back: true,
                can_forward: false,
            },
            cx,
        );
        assert_eq!(
            app.browser_surface_commands(&tab_id).last(),
            Some(&BrowserCommand::SetRouteGuard {
                guard: RouteGuard::Open
            })
        );
        editor_input.update(cx, |input, cx| {
            input.set_value("<p>saved after leaving preview</p>", window, cx);
        });
        app.editor_save_active(window, cx);
        app.on_file_tree_event(
            &FileTreeEvent::FsEvents {
                host: HostId::LOCAL,
                paths: HashSet::from([html_file.clone()]),
            },
            window,
            cx,
        );
    });
    wait_until(&mut vcx, "editor save after preview departure", |vcx| {
        app.read_with(vcx, |app, cx| app.editor_unsaved(cx).is_empty())
            && std::fs::read_to_string(&html_file)
                .is_ok_and(|body| body == "<p>saved after leaving preview</p>")
    });
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.right_panel.web_pages[&tab_id].display_url, webpage);
        assert_eq!(
            app.browser_surface_commands(&tab_id)
                .iter()
                .filter(|cmd| matches!(cmd, BrowserCommand::Reload))
                .count(),
            reload_count_before_unrelated
        );
        assert!(!cx.global::<HtmlPreviewService>().is_running());
    });

    let min_commands = app.update_in(&mut vcx, |app, _, cx| {
        let commands = app.browser_surface_commands(&tab_id).len();
        app.open_html_preview(HostId::LOCAL, &html_file, cx);
        commands + 3
    });
    let commands = wait_for_commands(&app, &mut vcx, &tab_id, min_commands);
    let mount_url = match commands.last().unwrap() {
        BrowserCommand::Navigate { url } => url,
        other => panic!("expected reopened HTML preview navigation, got {other:?}"),
    };

    // External modification + FileTreeEvent::FsEvents for html_file triggers Reload & serves fresh bytes.
    std::fs::write(&html_file, "<p>external-edit</p>").unwrap();
    app.update_in(&mut vcx, |app, window, cx| {
        app.on_file_tree_event(
            &FileTreeEvent::FsEvents {
                host: HostId::LOCAL,
                paths: HashSet::from([html_file.clone()]),
            },
            window,
            cx,
        );
    });
    let cmds_after_fs = wait_for_reload_count(&app, &mut vcx, &tab_id, 2);
    assert_eq!(cmds_after_fs.last(), Some(&BrowserCommand::Reload));
    assert_eq!(http_get(mount_url).1, "<p>external-edit</p>");
}
