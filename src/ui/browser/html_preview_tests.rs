use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use lyrel_core::core::config::RightPanelTab;
use lyrel_core::core::session::{RemoteRef, RemoteTarget, WorkspaceId};
use lyrel_core::host::local::LocalHost;
use lyrel_core::host::{
    ContentLimits, ContentQuery, ContentResults, Entry, Host, HostId, Meta, Output, SearchHit,
    SharedHost, ShellInventory, WatchSub,
};
use lyrel_ui_base::host_registry::HostRegistry;
use lyrel_ui_base::session::WorkspaceStore;

use super::html_preview::HtmlPreviewService;
use super::surface::RouteGuard;
use super::{BrowserCommand, BrowserEvent, PageStatus, policy};
use crate::ui::app::LyrelApp;
use crate::ui::app::test_window::harness_with_tabs;

struct RemotePosixTestHost {
    id: HostId,
    root: PathBuf,
    local: SharedHost,
}

impl RemotePosixTestHost {
    fn new(id: HostId, root: PathBuf) -> SharedHost {
        let canonical_root = std::fs::canonicalize(&root)
            .map(lyrel_core::core::path_spelling::local_spelling_buf)
            .unwrap_or(root);
        Arc::new(Self {
            id,
            root: canonical_root,
            local: LocalHost::new(),
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
        true
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

    // Opening the same HTML file again focuses and reloads the existing tab (no duplicate tab).
    app.update_in(&mut vcx, |app, _window, cx| {
        app.open_html_preview(HostId::LOCAL, &html_file, cx);
    });
    let cmds_after_reopen = wait_for_commands(&app, &mut vcx, &tab_id, 4);
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
    let remote_host = RemotePosixTestHost::new(host_id, remote_dir.path().to_path_buf());

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
