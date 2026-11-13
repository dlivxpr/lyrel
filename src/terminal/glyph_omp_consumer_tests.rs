// Real external-client acceptance: the actual installed omp drives its
// pinned support -> bundled-outline registration -> coverage-confirmation
// handshake through the same real PTY -> daemon -> production consumer seam
// as every other Glyph test. Nothing here emulates the client: the pane's
// shell is the real `omp.exe`, started with no prompt, an isolated profile,
// and every discovery surface disabled, then torn down once its icons are
// proven painted by registration.
//
// This is `#[ignore]`d like the native fixtures: it requires the real binary
// and is run explicitly. A missing binary is a hard failure, never a silent
// pass — the acceptance claim means omp 18.20.0, not "some client".

/// The exact installed client under test. `where omp` resolves to the Scoop
/// shim for this binary, verified separately with `omp --version` ==
/// `omp/18.20.0`.
const OMP_EXE: &str = r"C:\Users\dlivx\scoop\apps\oh-my-pi\current\omp.exe";

/// The codepoint omp's own handshake confirms after registering its bundle
/// (the `omp` mark in `glyph-protocol.ts`).
const OMP_CONFIRM_CP: u32 = 0xf0d57;

/// The pane shell spec that launches the real client with an isolated HOME,
/// no prompt, no model-facing surfaces and a bounded runtime. Environment is
/// applied through a wrapper script because `ShellSpec` has no env field: the
/// daemon spawns the wrapper, the wrapper execs omp with the isolation set.
fn omp_shell_spec(home: &std::path::Path) -> crate::daemon::protocol::ShellSpec {
    use crate::daemon::protocol::ShellSpec;
    std::fs::create_dir_all(home).unwrap();
    let profile = home.join("profile");
    std::fs::create_dir_all(&profile).unwrap();
    let omp_args = [
        format!("--profile={}", profile.display()),
        "--no-session".into(),
        "--no-tools".into(),
        "--no-lsp".into(),
        "--no-extensions".into(),
        "--no-skills".into(),
        "--no-rules".into(),
        "--no-context-files".into(),
        "--no-title".into(),
        // The probe happens during TUI startup; the cap only bounds a failed
        // run so the pane never outlives its assertions.
        "--max-time=90".into(),
    ];
    if cfg!(windows) {
        let script = home.join("omp-acceptance.cmd");
        let home = home.display().to_string().replace('/', "\\");
        std::fs::write(
            &script,
            format!(
                "@echo off\r\n\
                 set \"HOME={home}\"\r\n\
                 set \"USERPROFILE={home}\"\r\n\
                 set \"APPDATA={home}\\\\appdata\"\r\n\
                 set \"LOCALAPPDATA={home}\\\\localappdata\"\r\n\
                 set \"XDG_CONFIG_HOME={home}\\\\.config\"\r\n\
                 set \"XDG_DATA_HOME={home}\\\\.local\\\\share\"\r\n\
                 set \"XDG_CACHE_HOME={home}\\\\.cache\"\r\n\
                 set \"TMUX=\"\r\n\
                 set \"STY=\"\r\n\
                 \"{OMP_EXE}\" {}\r\n",
                omp_args.join(" ")
            ),
        )
        .unwrap();
        ShellSpec {
            program: "cmd.exe".into(),
            args: vec!["/c".into(), script.display().to_string()],
            args_are_lyrel_defaults: false,
        }
    } else {
        let script = home.join("omp-acceptance.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 HOME='{0}' XDG_CONFIG_HOME='{0}/.config' \\\n\
                 XDG_DATA_HOME='{0}/.local/share' XDG_CACHE_HOME='{0}/.cache' \\\n\
                 TMUX= STY= exec '{OMP_EXE}' {1}\n",
                home.display(),
                omp_args.join(" ")
            ),
        )
        .unwrap();
        ShellSpec {
            program: "sh".into(),
            args: vec![script.display().to_string()],
            args_are_lyrel_defaults: false,
        }
    }
}

/// The client's real startup handshake: `s`+DA1 must be answered so it
/// registers its bundled outlines with `reply=0`, the confirmation `q`+DA1
/// must report `glossary`, and only then does its TUI draw the registered
/// icons. Reaching a painted registered cell proves the whole chain through
/// the real binary; the registration — not an installed Nerd Font — is what
/// carries the supplementary-plane mark the fallback fonts cannot cover.
#[gpui_kit::test]
#[ignore = "requires the real installed omp 18.20.0 binary at OMP_EXE"]
fn glyph_consumer_real_omp_registers_and_paints_its_bundle(cx: &mut TestAppContext) {
    assert!(
        std::path::Path::new(OMP_EXE).is_file(),
        "real-client acceptance requires {OMP_EXE}; refusing to fake the handshake"
    );
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let home = tempfile::TempDir::new().unwrap();
    let pane = {
        use crate::daemon::protocol::WinSize;
        daemon
            .panes()
            .spawn(
                Some(home.path().to_path_buf()),
                WinSize {
                    cols: 80,
                    rows: 24,
                    cell_w: 10,
                    cell_h: 20,
                },
                Some(omp_shell_spec(home.path())),
                None,
                None,
            )
            .unwrap()
            .pane_id()
    };
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (view, window) =
        graphics_rooted_consumer(cx, pane, glyph_support_stream(&daemon, pane, true));

    // `s`+DA1 answered => the real client wrote its bundled registrations.
    // The store only fills from daemon-accepted bodies, so entries here are
    // the client's actual `r` traffic, never a fixture's.
    graphics_recovery_wait(cx, "real omp registered its bundled outlines", |cx| {
        view.read_with(cx, |view, _| {
            let store = view.terminal.glyphs();
            store.len() > 0 && store.contains(OMP_CONFIRM_CP)
        })
    });

    // The confirmation `q`+DA1 must have reported the glossary: only then
    // does the TUI draw the registered codepoint into a real cell. A painted
    // path inside that cell is the registered outline — the pane's ordinary
    // primary font has no glyph for this codepoint.
    graphics_recovery_wait(cx, "real omp draws a registered icon cell", |cx| {
        view.read_with(cx, |view, _| {
            view.cell_geom.is_some() && view.grid_buf.iter().any(|cell| cell.glyph.is_some())
        })
    });
    let (row, col) = view.read_with(cx, |view, _| {
        view.grid_buf
            .iter()
            .position(|cell| cell.glyph.is_some())
            .map(|index| {
                let cols = view.cell_geom.unwrap().cols;
                (index / cols, index % cols)
            })
            .expect("a registered icon cell exists")
    });
    let cell = glyph_cell_bounds(window, &view, cx, row, col);
    let paths = glyph_paths_in_cell(window, &view, cx, cell);
    assert_eq!(
        paths.len(),
        1,
        "the client's bundled icon paints exactly one registered path: {paths:?}"
    );
    assert!(
        view.read_with(cx, |view, _| {
            view.grid_buf[row * view.cell_geom.unwrap().cols + col].c as u32 >= 0xE000
        }),
        "the painted cell carries a private-use codepoint from the client"
    );

    // The isolated client must still be alive — a rejected handshake exits
    // the pane instead of lingering.
    assert!(
        !view.read_with(cx, |view, _| view.terminal.child_exited()),
        "the real omp exited before its icon was observed: handshake rejected"
    );
    daemon.panes().kill(pane).unwrap();
}
