// Explicit native acceptance of the production Root/LyrelApp GitHub panel.
// The existing API boundary is controlled; the pane socket is not a PTY.
struct NativeStackTransport(PathBuf);

impl Transport for NativeStackTransport {
    fn get(&self, path: &str) -> Result<Reply, ApiError> {
        let bytes = std::fs::read(&self.0).map_err(|e| ApiError::Decode(e.to_string()))?;
        let pulls = serde_json::from_slice(&bytes).map_err(|e| ApiError::Decode(e.to_string()))?;
        StackFake {
            pulls: Mutex::new(pulls),
        }
        .get(path)
    }

    fn authenticated(&self) -> bool {
        false
    }
}

struct NativeStackSocket {
    _socket: crate::daemon::transport::Stream,
}
impl gpui_kit::Global for NativeStackSocket {}

/// Requires an explicit owned directory. The JSON can be edited between
/// refreshes; no token lookup, HTTP, daemon, shell, or global registration runs.
#[test]
#[ignore = "opens a native window; set LYREL_GITHUB_NATIVE_DIR to an isolated fixture directory"]
fn native_stacked_pull_panel() {
    use gpui_kit::{AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};

    let directory = PathBuf::from(
        std::env::var_os("LYREL_GITHUB_NATIVE_DIR")
            .expect("set LYREL_GITHUB_NATIVE_DIR to an owned fixture directory"),
    );
    std::fs::create_dir_all(&directory).unwrap();
    let root = directory.join("repository");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "--quiet"]);
    git(
        &root,
        &[
            "config",
            "remote.origin.url",
            "https://github.com/acme/widgets",
        ],
    );
    let root =
        lyrel_core::core::path_spelling::local_spelling_buf(std::fs::canonicalize(&root).unwrap());
    let config = directory.join("config");
    std::fs::create_dir_all(&config).unwrap();
    crate::core::config::set_config_dir(config);
    let data = directory.join("pulls.json");
    if !data.exists() {
        let pulls = vec![
            stack_pull(1, "foundation", "main", "acme/widgets"),
            stack_pull(9, "solo", "main", "acme/widgets"),
            stack_pull(3, "top", "middle", "acme/widgets"),
            stack_pull(2, "middle", "foundation", "acme/widgets"),
            stack_pull(4, "sibling", "foundation", "acme/widgets"),
            stack_pull(8, "foundation", "main", "someone/widgets"),
            stack_pull(7, "cycle-a", "cycle-b", "acme/widgets"),
            stack_pull(6, "cycle-b", "cycle-a", "acme/widgets"),
            stack_pull(5, "missing-base", "not-listed", "acme/widgets"),
        ];
        std::fs::write(&data, serde_json::to_vec_pretty(&pulls).unwrap()).unwrap();
    }
    let transport: Arc<dyn Transport> = Arc::new(NativeStackTransport(data));
    gpui_kit::platform::application()
        .with_assets(crate::ui::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let mut config = crate::core::config::Config::default();
            config.install_cli_on_path = false;
            config.check_for_updates = false;
            config.auto_download_updates = false;
            config.cursor_blink = false;
            cx.set_global(config);
            crate::register_bundled_fonts(cx);
            crate::ui::presets::load_registry(cx);
            crate::ui::theme::apply_theme(None, cx);
            crate::ui::keymap::init(cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(1200.), px(800.)),
                    ))),
                    ..Default::default()
                },
                move |window, cx| {
                    let app = cx.new(|cx| {
                        let mut app = LyrelApp::with_session(
                            None,
                            Some(crate::core::session::Session::default()),
                            window,
                            cx,
                        );
                        let (view, mut socket) =
                            crate::terminal::view::quiet_test_pane(1, window, cx);
                        app.tabs
                            .push(crate::ui::tab::Tab::new(crate::ui::pane::Pane::leaf(
                                crate::ui::pane::PaneSlot::Ready(view),
                            )));
                        app.tabs.select(0);
                        app.github.connector = Some(Arc::new(move || Connection {
                            transport: transport.clone(),
                        }));
                        app.github.kind = Kind::Pulls;
                        app.right_panel_visible = true;
                        app.right_panel_tab = RightPanelTab::GitHub;
                        app.right_panel_width.set(460.);
                        DaemonMsg::Cwd(root).encode(&mut socket).unwrap();
                        cx.set_global(NativeStackSocket { _socket: socket });
                        app
                    });
                    window.set_window_title("lyrel #177 stacked PR fixture");
                    cx.new(|cx| gpui_kit::component::Root::new(app, window, cx))
                },
            )
            .expect("open native GitHub fixture window");
            cx.activate(true);
        });
}
