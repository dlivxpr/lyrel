use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

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

use super::BrowserCommand;
use super::html_preview::HtmlPreviewService;
use super::surface::RouteGuard;
use crate::daemon::protocol::ClientMsg;
use crate::ui::app::LyrelApp;
use crate::ui::app::test_window::harness_with_tabs;

struct RemotePosixTestHost {
    id: HostId,
    root: PathBuf,
    local: SharedHost,
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
        })
    }

    fn to_disk(&self, path: &Path) -> PathBuf {
        let raw = path.to_string_lossy();
        let mut out = self.root.clone();
        for seg in raw
            .trim_start_matches(['/', '\\'])
            .split(['/', '\\'])
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
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        settle(vcx);
        if done(vcx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {stage}");
        std::thread::sleep(Duration::from_millis(10));
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

fn active_web(app: &LyrelApp) -> Option<String> {
    app.active_browser_detail().map(str::to_string)
}

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
    vcx: &mut VisualTestContext,
    stream: &mut crate::daemon::transport::Stream,
    stage: &str,
) -> Vec<Vec<u8>> {
    let mut inputs = Vec::new();
    wait_until(vcx, stage, |_| {
        inputs.extend(take_inputs(stream));
        !inputs.is_empty()
    });
    inputs
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
fn local_agent_html_preview_opens_without_stealing_focus_reuses_tab_and_shares_screenshot(
    cx: &mut TestAppContext,
) {
    lyrel_ui_base::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let site = lyrel_core::core::path_spelling::local_spelling_buf(
        std::fs::canonicalize(dir.path()).unwrap(),
    );
    let index_file = site.join("index.html");
    let next_file = site.join("next.htm");
    let scratch = site.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(&index_file, "<h1>Agent HTML v1</h1>").unwrap();
    std::fs::write(&next_file, "<h1>Agent HTML v2</h1>").unwrap();

    let clean_index = lyrel_core::core::path_spelling::spelling_on(HostId::LOCAL, &index_file)
        .to_string_lossy()
        .into_owned();
    let clean_next = lyrel_core::core::path_spelling::spelling_on(HostId::LOCAL, &next_file)
        .to_string_lossy()
        .into_owned();
    let scratch_str = scratch.to_string_lossy().into_owned();

    let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
    settle(&mut vcx);

    let (focused, tab_id) = app.update_in(&mut vcx, |app, window, cx| {
        app.focus_active(window, cx);
        let focused = window.focused(cx).expect("terminal keeps focus");
        app.reveal_agent_preview(app.workspace, 1, "", Some(&clean_index), &scratch_str, cx)
            .expect("local agent HTML preview opens");
        let tab_id = active_web(app).expect("HTML preview is active");
        assert!(app.right_panel_visible, "right panel expands");
        (focused, tab_id)
    });
    assert_eq!(
        vcx.update(|window, cx| window.focused(cx)),
        Some(focused),
        "revealing an agent HTML preview never steals terminal focus"
    );

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
    let (status, body) = http_get(&mount_url);
    assert_eq!(status, 200);
    assert_eq!(body, "<h1>Agent HTML v1</h1>");

    app.read_with(&vcx, |app, cx| {
        let tab = app.web_tab(&tab_id).unwrap();
        assert_eq!(tab.html_path.as_deref(), Some(clean_index.as_str()));
        assert_eq!(tab.address, clean_index);
        let page = &app.right_panel.web_pages[&tab_id];
        assert_eq!(page.display_url, clean_index);
        assert!(page.html_mount.is_some());
        assert!(!page.display_title(&tab).contains("127.0.0.1"));
        let badge = app
            .agent_preview_source_label(&tab, cx)
            .expect("agent preview shows source badge");
        assert!(badge.contains("%1"), "{badge}");
    });

    // Share writes screenshot through Host and delivers clean HTML path + PNG path to %1.
    app.update_in(&mut vcx, |app, window, cx| {
        app.browser_preview_feedback_open(&tab_id, window, cx);
        let input = app.right_panel.web_pages[&tab_id]
            .feedback
            .as_ref()
            .expect("composer exists")
            .input
            .clone();
        input.update(cx, |state, cx| {
            state.set_value("fix button spacing", window, cx);
        });
        app.browser_preview_feedback_send(&tab_id, window, cx);
        let feedback = app.right_panel.web_pages[&tab_id]
            .feedback
            .as_ref()
            .expect("composer exists");
        assert_eq!(feedback.error, None, "send failed early");
        assert!(feedback.pending, "capture must be pending");
    });
    let inputs = wait_for_inputs(&mut vcx, &mut streams[0], "agent html share");
    let prompt = String::from_utf8(inputs.concat()).expect("prompt is UTF-8");
    assert!(prompt.contains("fix button spacing"), "{prompt}");
    assert!(prompt.contains(&clean_index), "{prompt}");
    assert!(prompt.contains("Screenshot (PNG)"), "{prompt}");
    assert!(!prompt.contains("127.0.0.1"), "{prompt}");

    let written = std::fs::read_dir(&scratch)
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
        .expect("screenshot PNG was written")
        .path();
    assert!(
        prompt.contains(&written.to_string_lossy().into_owned()),
        "{prompt}"
    );

    // Move away to Info tab, then have the same pane open a second HTML file:
    // reuses the same tab and switches back to Browser without stealing focus.
    app.update_in(&mut vcx, |app, window, cx| {
        app.set_right_panel_tab(RightPanelTab::Info, window, cx);
    });
    app.update_in(&mut vcx, |app, _window, cx| {
        app.reveal_agent_preview(app.workspace, 1, "", Some(&clean_next), &scratch_str, cx)
            .expect("reuses pane tab for next HTML file");
    });
    let cmds2 = wait_for_commands(&app, &mut vcx, &tab_id, 4);
    assert!(matches!(
        &cmds2[cmds2.len() - 1],
        BrowserCommand::Navigate { url } if url.ends_with("/next.htm")
    ));
    app.read_with(&vcx, |app, _| {
        assert_eq!(app.tool_layout().web_tabs.len(), 1);
        assert_eq!(app.tool_layout().active, RightPanelTab::Browser);
        let tab = app.web_tab(&tab_id).unwrap();
        assert_eq!(tab.html_path.as_deref(), Some(clean_next.as_str()));
        assert_eq!(app.right_panel.web_pages[&tab_id].display_url, clean_next);
    });

    // Switching the same pane from HTML file to an http(s) URL clears html_path & html_mount.
    app.update_in(&mut vcx, |app, _window, cx| {
        app.reveal_agent_preview(
            app.workspace,
            1,
            "http://localhost:5173/app",
            None,
            &scratch_str,
            cx,
        )
        .expect("switching back to URL clears HTML mount");
    });
    settle(&mut vcx);
    app.read_with(&vcx, |app, cx| {
        let tab = app.web_tab(&tab_id).unwrap();
        assert_eq!(tab.html_path, None);
        assert_eq!(tab.address, "http://localhost:5173/app");
        let page = &app.right_panel.web_pages[&tab_id];
        assert!(page.html_mount.is_none());
        assert_eq!(page.display_url, "http://localhost:5173/app");
        assert_eq!(cx.global::<HtmlPreviewService>().active_mount_count(), 0);
    });
}

#[gpui_kit::test]
fn remote_agent_html_preview_uses_verified_host_shares_through_remote_host_and_fails_closed(
    cx: &mut TestAppContext,
) {
    lyrel_ui_base::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let remote_dir = tempfile::tempdir().unwrap();
    let project = remote_dir.path().join("workspace");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("preview.xhtml"), "<p>Remote HTML</p>").unwrap();

    let target = RemoteTarget::Direct {
        user: "dev".into(),
        host: "remote.test".into(),
        port: 22,
    };
    let remote_ref = RemoteRef::new(target.clone(), WorkspaceId::new());
    let host_id = remote_ref.host_id();
    let remote_host = RemotePosixTestHost::shared(host_id, remote_dir.path().to_path_buf());

    cx.update(|cx| {
        WorkspaceStore::init(cx);
        HostRegistry::insert(cx, remote_host);
        crate::ui::remote_workspace::RemoteLinks::set_attached_for_test(cx, host_id);
    });
    let client_ws = cx.update(|cx| WorkspaceStore::claim_remote(cx, remote_ref.clone()));
    let (app, mut vcx) = crate::ui::app::test_window::harness_on_workspace(cx, client_ws);
    let mut stream = app.update_in(&mut vcx, |app, window, cx| {
        let (view, stream) = lyrel_terminal::view::quiet_test_pane(7, window, cx);
        let pane_ws = crate::ui::remote_workspace::pane_workspace_for(cx, client_ws);
        view.update(cx, |v, _| v.set_workspace(pane_ws));
        LyrelApp::watch_terminal(&view, window, cx);
        app.tabs
            .push(crate::ui::tab::Tab::new(crate::ui::pane::Pane::leaf(
                crate::ui::pane::PaneSlot::Ready(view),
            )));
        app.tabs.select(0);
        stream
    });
    settle(&mut vcx);

    let tab_id = app.update_in(&mut vcx, |app, _window, cx| {
        // Non-HTML file and wrong workspace are refused cleanly.
        assert!(
            app.reveal_agent_preview(
                remote_ref.workspace,
                7,
                "",
                Some("/etc/passwd"),
                "/tmp/scratch",
                cx,
            )
            .is_err()
        );
        assert!(
            app.reveal_agent_preview(
                WorkspaceId::new(),
                7,
                "",
                Some("/workspace/preview.xhtml"),
                "/tmp/scratch",
                cx,
            )
            .is_err()
        );

        app.reveal_agent_preview(
            remote_ref.workspace,
            7,
            "",
            Some("/workspace/preview.xhtml"),
            "/tmp/scratch",
            cx,
        )
        .expect("remote agent HTML preview opens on verified Host");
        app.active_browser_detail().unwrap().to_string()
    });

    let cmds = wait_for_commands(&app, &mut vcx, &tab_id, 2);
    let mount_url = match &cmds[1] {
        BrowserCommand::Navigate { url } => url.clone(),
        other => panic!("expected Navigate, got {other:?}"),
    };
    let (status, body) = http_get(&mount_url);
    assert_eq!(status, 200);
    assert_eq!(body, "<p>Remote HTML</p>");

    app.read_with(&vcx, |app, cx| {
        let tab = app.web_tab(&tab_id).unwrap();
        assert_eq!(tab.host.as_ref(), Some(&remote_ref));
        assert_eq!(tab.html_path.as_deref(), Some("/workspace/preview.xhtml"));
        assert_eq!(
            app.right_panel.web_pages[&tab_id].display_url,
            "/workspace/preview.xhtml"
        );
        let badge = app.agent_preview_source_label(&tab, cx).unwrap();
        assert!(badge.contains("%7"), "{badge}");
    });

    // Share writes screenshot to RemotePosixTestHost and sends Host-scoped path back to %7.
    app.update_in(&mut vcx, |app, window, cx| {
        app.browser_preview_feedback_open(&tab_id, window, cx);
        let input = app.right_panel.web_pages[&tab_id]
            .feedback
            .as_ref()
            .unwrap()
            .input
            .clone();
        input.update(cx, |state, cx| {
            state.set_value("check alignment", window, cx);
        });
        app.browser_preview_feedback_send(&tab_id, window, cx);
        let feedback = app.right_panel.web_pages[&tab_id]
            .feedback
            .as_ref()
            .expect("composer exists");
        assert_eq!(feedback.error, None, "send failed early");
        assert!(feedback.pending, "capture must be pending");
    });
    let inputs = wait_for_inputs(&mut vcx, &mut stream, "remote agent html share");
    let prompt = String::from_utf8(inputs.concat()).expect("prompt is UTF-8");
    assert!(prompt.contains("check alignment"), "{prompt}");
    assert!(prompt.contains("/workspace/preview.xhtml"), "{prompt}");
    assert!(prompt.contains("/tmp/scratch/preview-7-"), "{prompt}");

    let remote_scratch = remote_dir.path().join("tmp").join("scratch");
    assert!(
        std::fs::read_dir(&remote_scratch)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.path().extension().is_some_and(|ext| ext == "png")),
        "screenshot must be written through remote Host"
    );

    // Replacing the Host connection invalidates the PreviewBinding and fails closed.
    let replacement_host = RemotePosixTestHost::shared(host_id, remote_dir.path().to_path_buf());
    cx.update(|cx| {
        HostRegistry::insert(cx, replacement_host);
    });
    app.update_in(&mut vcx, |app, window, cx| {
        let input = app.right_panel.web_pages[&tab_id]
            .feedback
            .as_ref()
            .unwrap()
            .input
            .clone();
        input.update(cx, |state, cx| {
            state.set_value("after host reconnect", window, cx);
        });
        app.browser_preview_feedback_send(&tab_id, window, cx);
        let feedback = app.right_panel.web_pages[&tab_id]
            .feedback
            .as_ref()
            .unwrap();
        assert!(
            feedback.error.is_some(),
            "must fail closed when Host changed"
        );
        assert!(!feedback.pending);
    });
    settle(&mut vcx);
    assert!(
        take_inputs(&mut stream).is_empty(),
        "nothing may reach pane after Host replacement"
    );
}
