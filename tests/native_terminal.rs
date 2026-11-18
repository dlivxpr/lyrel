fn main() -> std::process::ExitCode {
    lyrel::run_native_terminal_tests(lyrel::NativeTerminalFixtureSupport::new(
        lyrel_test_support::fixture_identity::fixture_source_identity,
        lyrel_test_support::hex::hex,
        lyrel_test_support::hex::unhex,
        lyrel_test_support::pty::pty_sender,
    ))
}
