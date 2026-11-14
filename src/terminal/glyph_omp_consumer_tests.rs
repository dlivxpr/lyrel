// Real external-client acceptance: the actual released omp drives its pinned
// support -> bundled-outline registration -> coverage-confirmation handshake
// through the same real PTY -> daemon -> production consumer seam as every
// other Glyph test. Nothing here emulates the client: the pane's shell is the
// released omp executable, started with no prompt, an isolated HOME/profile,
// and every discovery surface disabled, then torn down once its icons are
// proven painted by registration.
//
// This is `#[ignore]`d like the other real-client fixtures: it requires the
// released binary located by the repository's existing `LYREL_TEST_OMP`
// contract and is run explicitly. A missing binary is a hard failure, never a
// silent pass — the acceptance claim means a real released omp, not "some
// client".

/// The codepoint omp's own handshake confirms after registering its bundle
/// (the `omp` mark in `glyph-protocol.ts`).
const OMP_CONFIRM_CP: u32 = 0xf0d57;

/// A legal profile name scoped to the isolated HOME — never a filesystem path.
const OMP_PROFILE: &str = "lyrel-glyph-acceptance";

/// Locate the released omp the way `omp_replay_consumer_tests.rs` already does:
/// the runner passes its absolute path through `LYREL_TEST_OMP`. `--version`
/// is verified before the handshake so the acceptance records which release
/// actually negotiated; a non-executable path fails here instead of faking it.
fn omp_binary() -> (String, String) {
    let path = std::env::var_os("LYREL_TEST_OMP")
        .expect("set LYREL_TEST_OMP to an installed released omp executable");
    let path = path.to_string_lossy().into_owned();
    let output = std::process::Command::new(&path)
        .arg("--version")
        .output()
        .unwrap_or_else(|error| panic!("LYREL_TEST_OMP={path} did not execute: {error}"));
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    assert!(
        output.status.success() && !version.is_empty(),
        "LYREL_TEST_OMP={path} is not a working omp release: {version} (stderr {})",
        String::from_utf8_lossy(&output.stderr)
    );
    (path, version)
}

/// The pane shell spec that launches the released client with an isolated
/// HOME, no prompt, no model-facing surfaces and a bounded runtime. Only
/// arguments embedded in the wrapper's shell source use `quote_for_shell`;
/// `ShellSpec::args` contains argv, not pre-quoted shell source. The wrapper
/// supplies the environment because `ShellSpec` has no env field.
fn omp_shell_spec(home: &std::path::Path, binary: &str) -> crate::daemon::protocol::ShellSpec {
    use crate::daemon::protocol::ShellSpec;
    std::fs::create_dir_all(home).unwrap();
    let program = if cfg!(windows) { "cmd.exe" } else { "sh" };
    let quoted = |arg: &str| lyrel_core::core::shell_quote::quote_for_shell(arg, Some(program));
    let omp_args = [
        format!("--profile={OMP_PROFILE}"),
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
    let invocation = std::iter::once(binary)
        .chain(omp_args.iter().map(String::as_str))
        .map(quoted)
        .collect::<Vec<_>>()
        .join(" ");
    if cfg!(windows) {
        let script = home.join("omp-acceptance.cmd");
        let home_escaped = home.display().to_string().replace('/', "\\");
        std::fs::write(
            &script,
            format!(
                "@echo off\r\n\
                 set \"HOME={home_escaped}\"\r\n\
                 set \"USERPROFILE={home_escaped}\"\r\n\
                 set \"APPDATA={home_escaped}\\\\appdata\"\r\n\
                 set \"LOCALAPPDATA={home_escaped}\\\\localappdata\"\r\n\
                 set \"XDG_CONFIG_HOME={home_escaped}\\\\.config\"\r\n\
                 set \"XDG_DATA_HOME={home_escaped}\\\\.local\\\\share\"\r\n\
                 set \"XDG_CACHE_HOME={home_escaped}\\\\.cache\"\r\n\
                 set \"TMUX=\"\r\n\
                 set \"STY=\"\r\n\
                 {invocation}\r\n",
            ),
        )
        .unwrap();
        ShellSpec {
            program: "cmd.exe".into(),
            args: vec!["/d".into(), "/c".into(), script.display().to_string()],
            args_are_lyrel_defaults: false,
        }
    } else {
        let script = home.join("omp-acceptance.sh");
        let home_q = quoted(&home.display().to_string());
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 export HOME={home_q}\n\
                 export XDG_CONFIG_HOME={home_q}/.config\n\
                 export XDG_DATA_HOME={home_q}/.local/share\n\
                 export XDG_CACHE_HOME={home_q}/.cache\n\
                 export TMUX= STY=\n\
                 exec {invocation}\n",
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
/// icons. Reaching a painted registered path proves the whole chain through
/// the real binary. The pane keeps the ordinary primary font — `Config`'s
/// default `font_family` — and the test reads back the family the production
/// view actually resolved, so the supplementary-plane mark is supplied by the
/// registration path and cannot be a Nerd-Font glyph the primary font owns.
#[gpui_kit::test]
#[ignore = "requires LYREL_TEST_OMP; real released omp, never calls a model"]
fn glyph_consumer_real_omp_registers_and_paints_its_bundle(cx: &mut TestAppContext) {
    let (binary, version) = omp_binary();
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
                Some(omp_shell_spec(home.path(), &binary)),
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
    // The fixture convenience helper sends a SEND command. A real client must
    // receive only terminal replies, never an injected command or prompt.
    let (view, window) = graphics_build_rooted_consumer(
        cx,
        pane,
        glyph_support_stream(&daemon, pane, true),
        Vec::new(),
    );
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(80, 24));

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
    // primary font has no glyph for this supplementary-plane codepoint.
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
    let (family, cell_cp) = view.read_with(cx, |view, _| {
        (
            view.font.family.clone(),
            view.grid_buf[row * view.cell_geom.unwrap().cols + col].c as u32,
        )
    });
    // Read back the resolved primary family and record it beside the release
    // version: the registered path, not a system Nerd-Font fallback, is the
    // provenance this test asserts. `font_family`'s default is an ordinary
    // mono font (never a Nerd Font), which is what "ordinary primary" means.
    assert_eq!(
        family.as_str(),
        Config::default().font_family,
        "the pane must keep the ordinary primary font, not a Nerd-Font fallback \
         (client {version})"
    );
    assert!(
        cell_cp >= 0xE000,
        "the painted cell carries a private-use codepoint from the client (got {cell_cp:#x})"
    );
    eprintln!(
        "REAL_OMP_GLYPH version={version:?} family={family} cp={cell_cp:x} paths={}",
        paths.len()
    );

    // The isolated client must still be alive — a rejected handshake exits
    // the pane instead of lingering.
    assert!(
        !view.read_with(cx, |view, _| view.terminal.child_exited()),
        "the real omp exited before its icon was observed: handshake rejected"
    );
    daemon.panes().kill(pane).unwrap();
}
