use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
use gpui_kit::{Entity, Focusable as _, MouseButton, TestAppContext, VisualTestContext, px, size};
use lyrel_core::core::config::RightPanelTab;
use lyrel_core::core::session::{RemoteRef, RemoteTarget, WorkspaceId};
use lyrel_core::daemon::protocol::DaemonMsg;
use lyrel_core::host::local::LocalHost;
use lyrel_core::host::{
    ContentLimits, ContentQuery, ContentResults, Entry, Host, HostId, Meta, Output, SearchHit,
    SharedHost, ShellInventory, WatchSub,
};
use lyrel_terminal::view::quiet_test_pane;
use lyrel_ui_base::host_registry::HostRegistry;
use lyrel_ui_base::i18n::{L10nKey, t};
use lyrel_ui_base::session::WorkspaceStore;

use super::BrowserCommand;
use super::html_preview::HtmlPreviewService;
use crate::ui::app::LyrelApp;
use crate::ui::app::test_window::{harness_on_workspace, harness_with_pane, harness_with_tabs};
use crate::ui::pane::{Pane, PaneSlot};
use crate::ui::tab::Tab;

struct RemotePosixTestHost {
    id: HostId,
    root: PathBuf,
    local: SharedHost,
}

impl RemotePosixTestHost {
    fn shared(id: HostId, root: PathBuf) -> SharedHost {
        let canonical_root = lyrel_core::core::path_spelling::local_spelling_buf(
            std::fs::canonicalize(&root).unwrap_or(root),
        );
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

    fn repo_root(&self, p: &Path) -> io::Result<Option<PathBuf>> {
        let Some(disk_root) = self.local.repo_root(&self.to_disk(p))? else {
            return Ok(None);
        };
        Ok(Some(self.to_wire(&disk_root)?))
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> io::Result<Output> {
        let mut out = self.local.git(&self.to_disk(cwd), args)?;
        if args.first() == Some(&"rev-parse") && out.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            let mapped: Vec<String> = text
                .lines()
                .map(|line| {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        return String::new();
                    }
                    let disk = lyrel_core::core::path_spelling::local_spelling_buf(
                        std::fs::canonicalize(trimmed).unwrap_or_else(|_| PathBuf::from(trimmed)),
                    );
                    self.to_wire(&disk)
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| trimmed.to_string())
                })
                .collect();
            let mut joined = mapped.join("\n");
            if text.ends_with('\n') {
                joined.push('\n');
            }
            out.stdout = joined.into_bytes();
        }
        Ok(out)
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
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        settle(vcx);
        if done(vcx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {stage}");
        std::thread::sleep(Duration::from_millis(15));
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
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        settle(vcx);
        if let Some(url) = cx.opened_url() {
            return url;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for opened system browser url"
        );
        std::thread::sleep(Duration::from_millis(15));
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

fn git_init(root: &Path) {
    let out = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(root)
        .output()
        .expect("git init");
    assert!(out.status.success(), "git init failed");
}

#[gpui_kit::test]
fn editor_tab_context_menu_offers_dual_html_browser_actions_for_local_and_remote_buffers(
    cx: &mut TestAppContext,
) {
    lyrel_ui_base::config::pin_test_config_dir();
    cx.executor().allow_parking();

    let dir = tempfile::tempdir().unwrap();
    let site = lyrel_core::core::path_spelling::local_spelling_buf(
        std::fs::canonicalize(dir.path()).unwrap(),
    );
    let html_file = site.join("index.html");
    let svg_file = site.join("icon.svg");
    let rs_file = site.join("main.rs");
    std::fs::write(&html_file, "<h1>Local Tab</h1>").unwrap();
    std::fs::write(&svg_file, "<svg/>").unwrap();
    std::fs::write(&rs_file, "fn main() {}\n").unwrap();

    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.simulate_resize(size(px(1440.), px(900.)));

    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&svg_file, window, cx);
        app.open_file_in_editor(&rs_file, window, cx);
        app.open_file_in_editor(&html_file, window, cx);
    });
    wait_until(&mut vcx, "local editor buffers", |vcx| {
        app.read_with(vcx, |app, cx| {
            app.tab_code().is_some_and(|c| c.files.len() == 3) && app.active_buffer(cx).is_some()
        })
    });

    let (svg_id, rs_id, html_id) = app.read_with(&vcx, |app, cx| {
        let buffers = app.editor.read(cx).buffers();
        let find_id = |target: &Path| {
            app.tab_code()
                .unwrap()
                .files
                .iter()
                .copied()
                .find(|id| buffers.get(*id).is_some_and(|f| f.path == target))
                .expect("buffer opened")
        };
        (find_id(&svg_file), find_id(&rs_file), find_id(&html_file))
    });

    // .svg tab must NOT expose Open in Browser / Open in System Browser (keeps default app action).
    vcx.update(|window, cx| {
        window.right_click(("editor-file-tab", svg_id.as_u64()), cx);
    });
    settle(&mut vcx);
    vcx.update(|window, _cx| {
        let menu = window.within("popup-menu");
        assert!(
            menu.try_find_by_label(t(L10nKey::PanelOpenInBrowser))
                .is_none(),
            ".svg tab must not show Open in Browser"
        );
        assert!(
            menu.try_find_by_label(t(L10nKey::BrowserOpenExternal))
                .is_none(),
            ".svg tab must not show Open in System Browser"
        );
        assert!(
            menu.try_find_by_label(t(L10nKey::AppMenuOpenLinkWithDefaultApp))
                .is_some(),
            ".svg tab keeps Open with Default App"
        );
    });
    vcx.update(|window, cx| window.press("escape", cx));
    settle(&mut vcx);

    // .rs tab must NOT expose HTML browser actions.
    vcx.update(|window, cx| {
        window.right_click(("editor-file-tab", rs_id.as_u64()), cx);
    });
    settle(&mut vcx);
    vcx.update(|window, _cx| {
        let menu = window.within("popup-menu");
        assert!(
            menu.try_find_by_label(t(L10nKey::PanelOpenInBrowser))
                .is_none(),
            ".rs tab must not show Open in Browser"
        );
        assert!(
            menu.try_find_by_label(t(L10nKey::BrowserOpenExternal))
                .is_none(),
            ".rs tab must not show Open in System Browser"
        );
    });
    vcx.update(|window, cx| window.press("escape", cx));
    settle(&mut vcx);

    // .html tab exposes both actions; Open in Browser opens Side Panel Browser preview.
    vcx.update(|window, cx| {
        window.right_click(("editor-file-tab", html_id.as_u64()), cx);
    });
    settle(&mut vcx);
    vcx.update(|window, cx| {
        let menu = window.within("popup-menu");
        menu.find_by_label(t(L10nKey::BrowserOpenExternal));
        let pos = menu
            .find_by_label(t(L10nKey::PanelOpenInBrowser))
            .bounds()
            .center();
        window.pointer_move(pos, None, cx);
        window.pointer_down(pos, MouseButton::Left, cx);
        window.pointer_up(pos, MouseButton::Left, cx);
    });
    wait_until(&mut vcx, "local html preview tab", |vcx| {
        app.read_with(vcx, |app, _| app.active_browser_detail().is_some())
    });
    let local_tab_id = app.read_with(&vcx, |app, _| {
        app.active_browser_detail().unwrap().to_string()
    });
    let cmds = wait_for_commands(&app, &mut vcx, &local_tab_id, 2);
    assert!(matches!(
        &cmds[1],
        BrowserCommand::Navigate { url } if url.ends_with("/index.html")
    ));

    // Return to the editor and trigger Open in System Browser from the tab context menu.
    app.update_in(&mut vcx, |app, window, cx| {
        app.toggle_code_panel(window, cx);
    });
    settle(&mut vcx);
    vcx.update(|window, cx| {
        window.right_click(("editor-file-tab", html_id.as_u64()), cx);
    });
    settle(&mut vcx);
    vcx.update(|window, cx| {
        let pos = window
            .within("popup-menu")
            .find_by_label(t(L10nKey::BrowserOpenExternal))
            .bounds()
            .center();
        window.pointer_move(pos, None, cx);
        window.pointer_down(pos, MouseButton::Left, cx);
        window.pointer_up(pos, MouseButton::Left, cx);
    });
    let local_external = wait_for_opened_url(cx, &mut vcx);
    assert!(local_external.starts_with("http://127.0.0.1:"));
    assert!(local_external.ends_with("/index.html"));
    assert_eq!(
        http_get(&local_external),
        (200, "<h1>Local Tab</h1>".into())
    );

    // Remote .xhtml buffer also exposes both actions and previews through Host loopback mount.
    let remote_dir = tempfile::tempdir().unwrap();
    let project = remote_dir.path().join("workspace");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("remote.xhtml"), "<h1>Remote Tab</h1>").unwrap();

    let target = RemoteTarget::Direct {
        user: "dev".into(),
        host: "remote.test".into(),
        port: 22,
    };
    let remote_ref = RemoteRef::new(target, WorkspaceId::new());
    let host_id = remote_ref.host_id();
    let remote_host = RemotePosixTestHost::shared(host_id, remote_dir.path().to_path_buf());

    let (remote_app, mut remote_vcx, _rstreams) = harness_with_tabs(cx, 1);
    remote_vcx.simulate_resize(size(px(1440.), px(900.)));
    let client_ws = cx.update(|cx| {
        WorkspaceStore::init(cx);
        HostRegistry::insert(cx, remote_host.clone());
        let id = WorkspaceStore::claim_remote(cx, remote_ref.clone());
        WorkspaceStore::claim(cx, Some(id))
    });
    remote_app.update_in(&mut remote_vcx, |app, window, cx| {
        app.workspace = client_ws;
        app.adopt_tool_layout(client_ws, cx);
        app.editor_open_on_host(
            remote_host,
            Path::new("/workspace/remote.xhtml"),
            window,
            cx,
        );
    });
    wait_until(&mut remote_vcx, "remote editor buffer", |vcx| {
        remote_app.read_with(vcx, |app, cx| app.active_buffer(cx).is_some())
    });
    let remote_buf_id =
        remote_app.read_with(&remote_vcx, |app, _| app.tab_code().unwrap().files[0]);

    remote_vcx.update(|window, cx| {
        window.right_click(("editor-file-tab", remote_buf_id.as_u64()), cx);
    });
    settle(&mut remote_vcx);
    remote_vcx.update(|window, cx| {
        let menu = window.within("popup-menu");
        menu.find_by_label(t(L10nKey::BrowserOpenExternal));
        let pos = menu
            .find_by_label(t(L10nKey::PanelOpenInBrowser))
            .bounds()
            .center();
        window.pointer_move(pos, None, cx);
        window.pointer_down(pos, MouseButton::Left, cx);
        window.pointer_up(pos, MouseButton::Left, cx);
    });
    wait_until(&mut remote_vcx, "remote html preview tab", |vcx| {
        remote_app.read_with(vcx, |app, _| app.active_browser_detail().is_some())
    });
    let remote_tab_id = remote_app.read_with(&remote_vcx, |app, _| {
        app.active_browser_detail().unwrap().to_string()
    });
    let rcmds = wait_for_commands(&remote_app, &mut remote_vcx, &remote_tab_id, 2);
    let remote_url = match &rcmds[1] {
        BrowserCommand::Navigate { url } => url.clone(),
        other => panic!("expected Navigate command, got {other:?}"),
    };
    assert!(remote_url.ends_with("/remote.xhtml"));
    assert_eq!(http_get(&remote_url), (200, "<h1>Remote Tab</h1>".into()));
    remote_app.read_with(&remote_vcx, |app, _| {
        let tab = app.web_tab(&remote_tab_id).unwrap();
        assert_eq!(tab.host.as_ref(), Some(&remote_ref));
        assert_eq!(tab.html_path.as_deref(), Some("/workspace/remote.xhtml"));
    });

    // Return to remote editor and trigger Open in System Browser from the tab context menu.
    remote_app.update_in(&mut remote_vcx, |app, window, cx| {
        app.toggle_code_panel(window, cx);
    });
    settle(&mut remote_vcx);
    remote_vcx.update(|window, cx| {
        window.right_click(("editor-file-tab", remote_buf_id.as_u64()), cx);
    });
    settle(&mut remote_vcx);
    remote_vcx.update(|window, cx| {
        let pos = window
            .within("popup-menu")
            .find_by_label(t(L10nKey::BrowserOpenExternal))
            .bounds()
            .center();
        window.pointer_move(pos, None, cx);
        window.pointer_down(pos, MouseButton::Left, cx);
        window.pointer_up(pos, MouseButton::Left, cx);
    });
    let remote_external = wait_for_opened_url(cx, &mut remote_vcx);
    assert!(remote_external.starts_with("http://127.0.0.1:"));
    assert!(remote_external.ends_with("/remote.xhtml"));
    assert_eq!(
        http_get(&remote_external),
        (200, "<h1>Remote Tab</h1>".into())
    );

    app.update_in(&mut vcx, |app, window, cx| {
        app.close_right_panel_web_tab(local_tab_id, window, cx);
    });
    remote_app.update_in(&mut remote_vcx, |app, window, cx| {
        app.close_right_panel_web_tab(remote_tab_id, window, cx);
    });
    cx.update(|cx| {
        cx.global::<HtmlPreviewService>().purge_host(HostId::LOCAL);
        cx.global::<HtmlPreviewService>().purge_host(host_id);
    });
}

#[gpui_kit::test]
fn editor_header_preview_button_shows_only_for_html_and_opens_or_reloads_preview(
    cx: &mut TestAppContext,
) {
    lyrel_ui_base::config::pin_test_config_dir();
    cx.executor().allow_parking();

    let dir = tempfile::tempdir().unwrap();
    let site = lyrel_core::core::path_spelling::local_spelling_buf(
        std::fs::canonicalize(dir.path()).unwrap(),
    );
    let rs_file = site.join("lib.rs");
    let svg_file = site.join("icon.svg");
    let htm_file = site.join("page.htm");
    std::fs::write(&rs_file, "pub fn ok() {}\n").unwrap();
    std::fs::write(&svg_file, "<svg/>").unwrap();
    std::fs::write(&htm_file, "<h1>Header Preview</h1>").unwrap();

    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.simulate_resize(size(px(1440.), px(900.)));

    // Untitled buffer: no preview button.
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_new_file(window, cx);
    });
    settle(&mut vcx);
    vcx.update(|window, _cx| {
        assert!(
            window.try_find("editor-html-preview").is_none(),
            "untitled buffer must not show HTML preview button"
        );
    });

    // .rs buffer: no preview button.
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&rs_file, window, cx);
    });
    wait_until(&mut vcx, "rs buffer active", |vcx| {
        app.read_with(vcx, |app, cx| {
            app.active_buffer(cx).is_some_and(|b| b.path == rs_file)
        })
    });
    vcx.update(|window, _cx| {
        assert!(
            window.try_find("editor-html-preview").is_none(),
            ".rs buffer must not show HTML preview button"
        );
    });

    // .svg buffer: no preview button.
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&svg_file, window, cx);
    });
    wait_until(&mut vcx, "svg buffer active", |vcx| {
        app.read_with(vcx, |app, cx| {
            app.active_buffer(cx).is_some_and(|b| b.path == svg_file)
        })
    });
    vcx.update(|window, _cx| {
        assert!(
            window.try_find("editor-html-preview").is_none(),
            ".svg buffer must not show HTML preview button"
        );
    });

    // .htm buffer: shows editor-html-preview button and opens preview on click.
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&htm_file, window, cx);
    });
    wait_until(&mut vcx, "htm buffer active", |vcx| {
        app.read_with(vcx, |app, cx| {
            app.active_buffer(cx).is_some_and(|b| b.path == htm_file)
        })
    });

    vcx.update(|window, cx| {
        let pos = window.find("editor-html-preview").bounds().center();
        window.pointer_move(pos, None, cx);
        window.pointer_down(pos, MouseButton::Left, cx);
        window.pointer_up(pos, MouseButton::Left, cx);
    });
    wait_until(&mut vcx, "htm preview opened from header", |vcx| {
        app.read_with(vcx, |app, _| app.active_browser_detail().is_some())
    });
    let tab_id = app.read_with(&vcx, |app, _| {
        app.active_browser_detail().unwrap().to_string()
    });
    let cmds = wait_for_commands(&app, &mut vcx, &tab_id, 2);
    assert!(matches!(
        &cmds[1],
        BrowserCommand::Navigate { url } if url.ends_with("/page.htm")
    ));

    // Clicking the header preview button again focuses & reloads the existing preview tab (no duplicate).
    app.update_in(&mut vcx, |app, window, cx| {
        app.toggle_code_panel(window, cx);
    });
    settle(&mut vcx);
    vcx.update(|window, cx| {
        let pos = window.find("editor-html-preview").bounds().center();
        window.pointer_move(pos, None, cx);
        window.pointer_down(pos, MouseButton::Left, cx);
        window.pointer_up(pos, MouseButton::Left, cx);
    });
    let cmds_after = wait_for_commands(&app, &mut vcx, &tab_id, 4);
    assert!(matches!(
        &cmds_after[cmds_after.len() - 1],
        BrowserCommand::Navigate { url } if url.ends_with("/page.htm")
    ));
    app.read_with(&vcx, |app, _| {
        assert_eq!(app.tool_layout().web_tabs.len(), 1);
        assert_eq!(app.active_browser_detail(), Some(tab_id.as_str()));
    });

    app.update_in(&mut vcx, |app, window, cx| {
        app.close_right_panel_web_tab(tab_id, window, cx);
    });
    cx.update(|cx| {
        cx.global::<HtmlPreviewService>().purge_host(HostId::LOCAL);
    });
}

#[gpui_kit::test]
fn scm_changes_context_menu_offers_open_and_dual_html_browser_actions_for_local_and_remote(
    cx: &mut TestAppContext,
) {
    lyrel_ui_base::config::pin_test_config_dir();
    cx.executor().allow_parking();

    // 1. Local git repository with HTML, SVG, and Rust files.
    let dir = tempfile::tempdir().unwrap();
    let root = lyrel_core::core::path_spelling::local_spelling_buf(
        std::fs::canonicalize(dir.path()).unwrap(),
    );
    git_init(&root);
    let html_file = root.join("index.html");
    std::fs::write(&html_file, "<h1>SCM Local HTML</h1>").unwrap();
    std::fs::write(root.join("icon.svg"), "<svg/>").unwrap();
    std::fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();

    let (app, mut vcx, mut pane) = harness_with_pane(cx);
    vcx.simulate_resize(size(px(1440.), px(900.)));
    DaemonMsg::Cwd(root.clone()).encode(&mut pane).unwrap();
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut pane)
    .unwrap();
    app.update_in(&mut vcx, |app, _, cx| {
        app.right_panel_visible = true;
        app.right_panel_tab = RightPanelTab::Scm;
        cx.notify();
    });
    wait_until(&mut vcx, "local scm rows rendered", |vcx| {
        vcx.debug_bounds("scm-row-Untracked-index.html").is_some()
            && vcx.debug_bounds("scm-row-Untracked-icon.svg").is_some()
            && vcx.debug_bounds("scm-row-Untracked-main.rs").is_some()
    });

    // Non-HTML rows (.svg and .rs) offer Open, without HTML browser actions.
    for selector in ["scm-row-Untracked-icon.svg", "scm-row-Untracked-main.rs"] {
        let pos = vcx.debug_bounds(selector).unwrap().center();
        vcx.update(|window, cx| {
            window.pointer_move(pos, None, cx);
            window.pointer_down(pos, MouseButton::Right, cx);
            window.pointer_up(pos, MouseButton::Right, cx);
        });
        settle(&mut vcx);
        vcx.update(|window, _cx| {
            let menu = window.within("popup-menu");
            menu.find_by_label(t(L10nKey::FileTreeContextOpen));
            assert!(
                menu.try_find_by_label(t(L10nKey::PanelOpenInBrowser))
                    .is_none(),
                "{selector} must not show Open in Browser"
            );
            assert!(
                menu.try_find_by_label(t(L10nKey::BrowserOpenExternal))
                    .is_none(),
                "{selector} must not show Open in System Browser"
            );
        });
        vcx.update(|window, cx| window.press("escape", cx));
        settle(&mut vcx);
    }

    // HTML row offers Open, Open in Browser, and Open in System Browser.
    let html_pos = vcx
        .debug_bounds("scm-row-Untracked-index.html")
        .unwrap()
        .center();
    vcx.update(|window, cx| {
        window.pointer_move(html_pos, None, cx);
        window.pointer_down(html_pos, MouseButton::Right, cx);
        window.pointer_up(html_pos, MouseButton::Right, cx);
    });
    settle(&mut vcx);
    vcx.update(|window, cx| {
        let menu = window.within("popup-menu");
        menu.find_by_label(t(L10nKey::FileTreeContextOpen));
        menu.find_by_label(t(L10nKey::BrowserOpenExternal));
        let pos = menu
            .find_by_label(t(L10nKey::PanelOpenInBrowser))
            .bounds()
            .center();
        window.pointer_move(pos, None, cx);
        window.pointer_down(pos, MouseButton::Left, cx);
        window.pointer_up(pos, MouseButton::Left, cx);
    });
    wait_until(&mut vcx, "local scm html preview opened", |vcx| {
        app.read_with(vcx, |app, _| app.active_browser_detail().is_some())
    });
    let local_tab_id = app.read_with(&vcx, |app, _| {
        app.active_browser_detail().unwrap().to_string()
    });
    let cmds = wait_for_commands(&app, &mut vcx, &local_tab_id, 2);
    assert!(matches!(
        &cmds[1],
        BrowserCommand::Navigate { url } if url.ends_with("/index.html")
    ));

    // Switch back to Changes (Scm) panel, then right-click again -> Open in System Browser.
    app.update_in(&mut vcx, |app, window, cx| {
        app.set_right_panel_tab(RightPanelTab::Scm, window, cx);
    });
    wait_until(&mut vcx, "local scm row visible again", |vcx| {
        vcx.debug_bounds("scm-row-Untracked-index.html").is_some()
    });
    let html_pos = vcx
        .debug_bounds("scm-row-Untracked-index.html")
        .unwrap()
        .center();
    vcx.update(|window, cx| {
        window.pointer_move(html_pos, None, cx);
        window.pointer_down(html_pos, MouseButton::Right, cx);
        window.pointer_up(html_pos, MouseButton::Right, cx);
    });
    settle(&mut vcx);
    vcx.update(|window, cx| {
        let pos = window
            .within("popup-menu")
            .find_by_label(t(L10nKey::BrowserOpenExternal))
            .bounds()
            .center();
        window.pointer_move(pos, None, cx);
        window.pointer_down(pos, MouseButton::Left, cx);
        window.pointer_up(pos, MouseButton::Left, cx);
    });
    let local_external = wait_for_opened_url(cx, &mut vcx);
    assert!(local_external.starts_with("http://127.0.0.1:"));
    assert!(local_external.ends_with("/index.html"));
    assert_eq!(
        http_get(&local_external),
        (200, "<h1>SCM Local HTML</h1>".into())
    );

    // Right-click again -> Open opens the file in the built-in editor.
    let html_pos = vcx
        .debug_bounds("scm-row-Untracked-index.html")
        .unwrap()
        .center();
    vcx.update(|window, cx| {
        window.pointer_move(html_pos, None, cx);
        window.pointer_down(html_pos, MouseButton::Right, cx);
        window.pointer_up(html_pos, MouseButton::Right, cx);
    });
    settle(&mut vcx);
    vcx.update(|window, cx| {
        let pos = window
            .within("popup-menu")
            .find_by_label(t(L10nKey::FileTreeContextOpen))
            .bounds()
            .center();
        window.pointer_move(pos, None, cx);
        window.pointer_down(pos, MouseButton::Left, cx);
        window.pointer_up(pos, MouseButton::Left, cx);
    });
    wait_until(&mut vcx, "local scm file opened in editor", |vcx| {
        app.read_with(vcx, |app, cx| {
            app.code_panel_visible() && app.active_buffer(cx).is_some_and(|b| b.path == html_file)
        })
    });

    // 2. Remote git repository via RemotePosixTestHost.
    let remote_dir = tempfile::tempdir().unwrap();
    let project = remote_dir.path().join("workspace");
    std::fs::create_dir_all(&project).unwrap();
    git_init(&project);
    std::fs::write(project.join("remote.xhtml"), "<div>Remote SCM HTML</div>").unwrap();
    std::fs::write(project.join("notes.rs"), "pub fn r() {}\n").unwrap();

    let target = RemoteTarget::Direct {
        user: "dev".into(),
        host: "remote-scm.test".into(),
        port: 22,
    };
    let remote_ref = RemoteRef::new(target, WorkspaceId::new());
    let host_id = remote_ref.host_id();
    let remote_host = RemotePosixTestHost::shared(host_id, remote_dir.path().to_path_buf());
    cx.update(|cx| {
        WorkspaceStore::init(cx);
        HostRegistry::insert(cx, remote_host);
    });
    let client_ws = cx.update(|cx| WorkspaceStore::claim_remote(cx, remote_ref.clone()));
    let (remote_app, mut remote_vcx) = harness_on_workspace(cx, client_ws);
    remote_vcx.simulate_resize(size(px(1440.), px(900.)));
    let (_rpane, mut rstream) = remote_app.update_in(&mut remote_vcx, |app, window, cx| {
        let (view, stream) = quiet_test_pane(1, window, cx);
        let pane_ws = crate::ui::remote_workspace::pane_workspace_for(cx, client_ws);
        view.update(cx, |v, _| v.set_workspace(pane_ws));
        LyrelApp::watch_terminal(&view, window, cx);
        LyrelApp::watch_pane_focus(&view.focus_handle(cx), view.entity_id(), window, cx);
        LyrelApp::watch_terminal_scm(&view, cx);
        app.tabs
            .push(Tab::new(Pane::leaf(PaneSlot::Ready(view.clone()))));
        app.tabs.select(0);
        view.focus_handle(cx).focus(window, cx);
        app.right_panel_visible = true;
        app.right_panel_tab = RightPanelTab::Scm;
        cx.notify();
        (view, stream)
    });
    DaemonMsg::Cwd(PathBuf::from("/workspace"))
        .encode(&mut rstream)
        .unwrap();
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut rstream)
    .unwrap();
    wait_until(&mut remote_vcx, "remote scm rows rendered", |vcx| {
        vcx.debug_bounds("scm-row-Untracked-remote.xhtml").is_some()
            && vcx.debug_bounds("scm-row-Untracked-notes.rs").is_some()
    });

    let remote_html_pos = remote_vcx
        .debug_bounds("scm-row-Untracked-remote.xhtml")
        .unwrap()
        .center();
    remote_vcx.update(|window, cx| {
        window.pointer_move(remote_html_pos, None, cx);
        window.pointer_down(remote_html_pos, MouseButton::Right, cx);
        window.pointer_up(remote_html_pos, MouseButton::Right, cx);
    });
    settle(&mut remote_vcx);
    remote_vcx.update(|window, cx| {
        let menu = window.within("popup-menu");
        menu.find_by_label(t(L10nKey::FileTreeContextOpen));
        menu.find_by_label(t(L10nKey::BrowserOpenExternal));
        let pos = menu
            .find_by_label(t(L10nKey::PanelOpenInBrowser))
            .bounds()
            .center();
        window.pointer_move(pos, None, cx);
        window.pointer_down(pos, MouseButton::Left, cx);
        window.pointer_up(pos, MouseButton::Left, cx);
    });
    wait_until(&mut remote_vcx, "remote scm html preview opened", |vcx| {
        remote_app.read_with(vcx, |app, _| app.active_browser_detail().is_some())
    });
    let remote_tab_id = remote_app.read_with(&remote_vcx, |app, _| {
        app.active_browser_detail().unwrap().to_string()
    });
    let rcmds = wait_for_commands(&remote_app, &mut remote_vcx, &remote_tab_id, 2);
    let remote_mount_url = match &rcmds[1] {
        BrowserCommand::Navigate { url } => url.clone(),
        other => panic!("expected Navigate command, got {other:?}"),
    };
    assert!(remote_mount_url.ends_with("/remote.xhtml"));
    assert_eq!(
        http_get(&remote_mount_url),
        (200, "<div>Remote SCM HTML</div>".into())
    );

    remote_app.update_in(&mut remote_vcx, |app, window, cx| {
        app.set_right_panel_tab(RightPanelTab::Scm, window, cx);
    });
    wait_until(&mut remote_vcx, "remote scm row visible again", |vcx| {
        vcx.debug_bounds("scm-row-Untracked-remote.xhtml").is_some()
    });
    let remote_html_pos = remote_vcx
        .debug_bounds("scm-row-Untracked-remote.xhtml")
        .unwrap()
        .center();
    remote_vcx.update(|window, cx| {
        window.pointer_move(remote_html_pos, None, cx);
        window.pointer_down(remote_html_pos, MouseButton::Right, cx);
        window.pointer_up(remote_html_pos, MouseButton::Right, cx);
    });
    settle(&mut remote_vcx);
    remote_vcx.update(|window, cx| {
        let pos = window
            .within("popup-menu")
            .find_by_label(t(L10nKey::BrowserOpenExternal))
            .bounds()
            .center();
        window.pointer_move(pos, None, cx);
        window.pointer_down(pos, MouseButton::Left, cx);
        window.pointer_up(pos, MouseButton::Left, cx);
    });
    let remote_external = wait_for_opened_url(cx, &mut remote_vcx);
    assert!(remote_external.starts_with("http://127.0.0.1:"));
    assert!(remote_external.ends_with("/remote.xhtml"));
    assert_eq!(
        http_get(&remote_external),
        (200, "<div>Remote SCM HTML</div>".into())
    );

    app.update_in(&mut vcx, |app, window, cx| {
        app.close_right_panel_web_tab(local_tab_id, window, cx);
    });
    remote_app.update_in(&mut remote_vcx, |app, window, cx| {
        app.close_right_panel_web_tab(remote_tab_id, window, cx);
    });
    cx.update(|cx| {
        cx.global::<HtmlPreviewService>().purge_host(HostId::LOCAL);
        cx.global::<HtmlPreviewService>().purge_host(host_id);
    });
}
