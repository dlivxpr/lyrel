fn main() -> std::process::ExitCode {
    lyrel::run_native_terminal_tests(lyrel::NativeTerminalFixtureSupport::new(
        lyrel_test_support::pty::pty_sender,
    ))
}
