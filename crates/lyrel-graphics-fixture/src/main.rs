//! Lightweight process host for the desktop terminal consumer tests.
//!
//! The tests need a real Session Daemon and real raw-mode PTY producers. Before
//! this crate existed they re-launched the multi-hundred-megabyte GUI libtest
//! binary, paying GPUI dynamic loading and a 3,500-test libtest scan for every
//! spawn. This binary links only `lyrel-core`, reuses the exact shared fixture
//! sources under `tests/support`, and dispatches on one mode argument.

mod pty {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/kitty_pty.rs"
    ));
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/theme_pty.rs"
    ));
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/mouse_pty.rs"
    ));
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/glyph_pty.rs"
    ));
}

fn main() {
    // `identity` exists so the consumer test support can refuse a fixture
    // binary built from other sources instead of silently trusting it.
    let mode = std::env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "daemon" => run_daemon(),
        "kitty" => pty::pty_sender(),
        "theme" => pty::theme_query_sender(),
        "mouse" => pty::mouse_reporting_fixture_recorder(),
        "mouse-utf8" => pty::mouse_utf8_fixture_recorder(),
        "glyph" => pty::glyph_consumer_fixture_sender(),
        "identity" => println!("{}", env!("LYREL_GRAPHICS_FIXTURE_ID")),
        other => {
            eprintln!(
                "lyrel-graphics-fixture: expected one of daemon, kitty, theme, mouse, \
                 mouse-utf8, glyph, identity; got {other:?}"
            );
            std::process::exit(2);
        }
    }
}

fn run_daemon() {
    let dir = std::env::var_os("LYREL_GRAPHICS_TEST_DAEMON")
        .expect("the graphics fixture daemon needs an isolated LYREL_GRAPHICS_TEST_DAEMON dir");
    lyrel_core::core::config::set_config_dir(dir.into());
    lyrel_core::daemon::server::run_daemon().expect("graphics fixture daemon");
}
