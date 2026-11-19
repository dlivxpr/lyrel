//! Lightweight process host for the desktop terminal consumer tests.
//!
//! The tests need a real Session Daemon and real raw-mode PTY producers. Before
//! this binary existed they re-launched the multi-hundred-megabyte GUI libtest
//! binary, paying GPUI dynamic loading and a 3,500-test libtest scan for every
//! spawn. This binary links only `lyrel-core`, reuses the shared fixture
//! library from `lyrel-test-support`, and dispatches on one mode argument.

fn main() {
    // `identity` exists so the consumer test support can refuse a fixture
    // binary built from other sources instead of silently trusting it.
    let mode = std::env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "daemon" => run_daemon(),
        "kitty" => lyrel_test_support::pty::pty_sender(),
        "theme" => lyrel_test_support::theme_pty::theme_query_sender(),
        "mouse" => lyrel_test_support::mouse_pty::mouse_reporting_fixture_recorder(),
        "mouse-utf8" => lyrel_test_support::mouse_pty::mouse_utf8_fixture_recorder(),
        "glyph" => lyrel_test_support::glyph_pty::glyph_consumer_fixture_sender(),
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
