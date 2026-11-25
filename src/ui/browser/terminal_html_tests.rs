use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use lyrel_core::core::config::RightPanelTab;
use lyrel_core::core::session::{RemoteRef, RemoteTarget, WorkspaceId};
use lyrel_core::daemon::protocol::DaemonMsg;
use lyrel_core::host::local::LocalHost;
use lyrel_core::host::{
    ContentLimits, ContentQuery, ContentResults, Entry, Host, HostId, Meta, Output, SearchHit,
    SharedHost, ShellInventory, WatchSub,
};
use lyrel_terminal::view::{TerminalView, quiet_test_pane};
use lyrel_ui_base::actions::{OpenLinkInBrowser, OpenLinkInSystemBrowser};
use lyrel_ui_base::host_registry::HostRegistry;
use lyrel_ui_base::session::WorkspaceStore;

use super::BrowserCommand;
use crate::ui::app::LyrelApp;
use crate::ui::app::test_window::{harness_on_workspace, harness_with_tabs};
use crate::ui::pane::{Pane, PaneSlot};
use crate::ui::tab::Tab;

struct RemotePosixTestHost {
    id: HostId,
    root: PathBuf,
    local: SharedHost,
}

impl RemotePosixTestHost {
    fn new(id: HostId, root: PathBuf) -> SharedHost {
        let canonical_root = LocalHost::new().canonicalize(&root).unwrap_or(root);
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

fn wait_for_opened_url(cx: &mut TestAppContext, vcx: &mut VisualTestContext) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        settle(vcx);
        if let Some(url) = cx.opened_url() {
            return url;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for system browser URL"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
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

fn feed_pane_output(
    pane: &Entity<TerminalView>,
    vcx: &mut VisualTestContext,
    stream: &mut crate::daemon::transport::Stream,
    cwd: PathBuf,
    output: &[u8],
    needle_col: usize,
    needle_char: char,
) {
    DaemonMsg::Cwd(cwd).encode(stream).unwrap();
    DaemonMsg::Output(output.to_vec()).encode(stream).unwrap();
    wait_until(vcx, "pane cwd and output", |vcx| {
        pane.update(vcx, |view, cx| {
            view.cwd().is_some() && view.hover_link_at(needle_col, 0, true, cx)
        })
    });
    let _ = needle_char;
}

#[gpui_kit::test]
fn local_terminal_html_links_and_context_menu_open_in_browser_and_system_browser(
    cx: &mut TestAppContext,
) {
    lyrel_ui_base::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let site = LocalHost::new().canonicalize(dir.path()).unwrap();
    let index = site.join("index.html");
    let other = site.join("other.htm");
    std::fs::write(&index, "<h1>Local Terminal HTML</h1>").unwrap();
    std::fs::write(&other, "<p>System Browser HTML</p>").unwrap();

    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
    let pane = app.read_with(&vcx, |app, _| {
        app.tabs[0]
            .pane
            .first_leaf()
            .unwrap()
            .terminal()
            .unwrap()
            .clone()
    });
    feed_pane_output(
        &pane,
        &mut vcx,
        &mut streams[0],
        site.clone(),
        b"index.html other.htm\r\n",
        2,
        'n',
    );

    // Click index.html -> opens in Side Panel Browser via loopback mount.
    app.update_in(&mut vcx, |_, window, cx| {
        pane.update(cx, |view, cx| {
            assert!(view.open_link_at_with_modifiers(2, 0, false, window, cx));
        });
    });
    let tab_id = app.read_with(&vcx, |app, _| {
        assert_eq!(app.tool_layout().active, RightPanelTab::Browser);
        app.active_browser_detail()
            .expect("html preview is raised")
            .to_string()
    });
    let cmds = wait_for_commands(&app, &mut vcx, &tab_id, 2);
    let mount_url = match &cmds[1] {
        BrowserCommand::Navigate { url } => url.clone(),
        other => panic!("expected Navigate command, got {other:?}"),
    };
    assert!(mount_url.starts_with("http://127.0.0.1:"));
    assert!(mount_url.ends_with("/index.html"));
    assert_eq!(
        http_get(&mount_url),
        (200, "<h1>Local Terminal HTML</h1>".into())
    );

    // Shift+Click other.htm -> opens loopback URL in System Browser.
    app.update_in(&mut vcx, |_, window, cx| {
        pane.update(cx, |view, cx| {
            assert!(view.open_link_at_with_modifiers(13, 0, true, window, cx));
        });
    });
    let opened = wait_for_opened_url(cx, &mut vcx);
    assert!(opened.starts_with("http://127.0.0.1:"));
    assert!(opened.ends_with("/other.htm"));
    assert_eq!(
        http_get(&opened),
        (200, "<p>System Browser HTML</p>".into())
    );

    // Context menu actions on other.htm open Side Panel Browser and System Browser too.
    app.update_in(&mut vcx, |_, window, cx| {
        pane.update(cx, |view, cx| {
            view.record_menu_link(13, 0, cx);
            view.focus_handle.focus(window, cx);
        });
        window.dispatch_action(Box::new(OpenLinkInBrowser), cx);
    });
    settle(&mut vcx);
    let second_tab_id = app.read_with(&vcx, |app, _| {
        assert_eq!(app.tool_layout().web_tabs.len(), 2);
        app.active_browser_detail().unwrap().to_string()
    });
    let cmds2 = wait_for_commands(&app, &mut vcx, &second_tab_id, 2);
    assert!(matches!(
        &cmds2[1],
        BrowserCommand::Navigate { url } if url.ends_with("/other.htm")
    ));

    app.update_in(&mut vcx, |_, window, cx| {
        pane.update(cx, |view, cx| {
            view.record_menu_link(2, 0, cx);
            view.focus_handle.focus(window, cx);
        });
        window.dispatch_action(Box::new(OpenLinkInSystemBrowser), cx);
    });
    let menu_opened = wait_for_opened_url(cx, &mut vcx);
    assert!(menu_opened.starts_with("http://127.0.0.1:"));
    assert!(menu_opened.ends_with("/index.html"));
    assert_eq!(
        http_get(&menu_opened),
        (200, "<h1>Local Terminal HTML</h1>".into())
    );
}

#[gpui_kit::test]
fn remote_terminal_html_links_and_context_menu_open_in_browser_and_system_browser(
    cx: &mut TestAppContext,
) {
    lyrel_ui_base::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let remote_dir = tempfile::tempdir().unwrap();
    let project = remote_dir.path().join("workspace");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("preview.xhtml"), "<div>Remote HTML</div>").unwrap();

    let target = RemoteTarget::Direct {
        user: "dev".into(),
        host: "remote.test".into(),
        port: 22,
    };
    let remote_ref = RemoteRef::new(target, WorkspaceId::new());
    let host_id = remote_ref.host_id();
    let remote_host = RemotePosixTestHost::new(host_id, remote_dir.path().to_path_buf());

    cx.update(|cx| {
        WorkspaceStore::init(cx);
        HostRegistry::insert(cx, remote_host);
    });
    let client_ws = cx.update(|cx| WorkspaceStore::claim_remote(cx, remote_ref.clone()));
    let (app, mut vcx) = harness_on_workspace(cx, client_ws);
    let (pane, mut stream) = app.update_in(&mut vcx, |app, window, cx| {
        let (view, stream) = quiet_test_pane(1, window, cx);
        let pane_ws = crate::ui::remote_workspace::pane_workspace_for(cx, client_ws);
        view.update(cx, |v, _| v.set_workspace(pane_ws));
        LyrelApp::watch_terminal(&view, window, cx);
        app.tabs
            .push(Tab::new(Pane::leaf(PaneSlot::Ready(view.clone()))));
        app.tabs.select(0);
        cx.notify();
        (view, stream)
    });
    vcx.background_executor.run_until_parked();

    feed_pane_output(
        &pane,
        &mut vcx,
        &mut stream,
        PathBuf::from("/workspace"),
        b"/workspace/preview.xhtml\r\n",
        2,
        'w',
    );

    // Click remote HTML link -> opens in Side Panel Browser with RemoteRef host.
    app.update_in(&mut vcx, |_, window, cx| {
        pane.update(cx, |view, cx| {
            assert!(view.open_link_at_with_modifiers(2, 0, false, window, cx));
        });
    });
    let tab_id = app.read_with(&vcx, |app, _| {
        let tab_id = app
            .active_browser_detail()
            .expect("remote html tab")
            .to_string();
        let tab = app.web_tab(&tab_id).unwrap();
        assert_eq!(tab.host.as_ref(), Some(&remote_ref));
        assert_eq!(tab.html_path.as_deref(), Some("/workspace/preview.xhtml"));
        tab_id
    });
    let cmds = wait_for_commands(&app, &mut vcx, &tab_id, 2);
    let mount_url = match &cmds[1] {
        BrowserCommand::Navigate { url } => url.clone(),
        other => panic!("expected Navigate command, got {other:?}"),
    };
    assert!(mount_url.starts_with("http://127.0.0.1:"));
    assert!(mount_url.ends_with("/preview.xhtml"));
    assert_eq!(http_get(&mount_url), (200, "<div>Remote HTML</div>".into()));

    // Shift+Click & context menu OpenLinkInSystemBrowser -> loopback URL in System Browser.
    app.update_in(&mut vcx, |_, window, cx| {
        pane.update(cx, |view, cx| {
            assert!(view.open_link_at_with_modifiers(2, 0, true, window, cx));
        });
    });
    let opened = wait_for_opened_url(cx, &mut vcx);
    assert!(opened.starts_with("http://127.0.0.1:"));
    assert!(opened.ends_with("/preview.xhtml"));
    assert_eq!(http_get(&opened), (200, "<div>Remote HTML</div>".into()));

    app.update_in(&mut vcx, |_, window, cx| {
        pane.update(cx, |view, cx| {
            view.record_menu_link(2, 0, cx);
            view.focus_handle.focus(window, cx);
        });
        window.dispatch_action(Box::new(OpenLinkInBrowser), cx);
    });
    settle(&mut vcx);
    app.read_with(&vcx, |app, _| {
        assert_eq!(app.tool_layout().web_tabs.len(), 1);
    });
}
