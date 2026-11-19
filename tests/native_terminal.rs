#[path = "native_terminal/cases.rs"]
mod cases;

fn main() -> std::process::ExitCode {
    cases::run_fixture().unwrap_or_else(|| lyrel::run_native_terminal_tests(cases::native_cases()))
}
