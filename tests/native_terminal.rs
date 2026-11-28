#[path = "native_terminal/cases.rs"]
mod cases;
#[path = "native_terminal/plaintext.rs"]
mod plaintext;

fn main() -> std::process::ExitCode {
    cases::run_fixture().unwrap_or_else(|| lyrel::run_native_terminal_tests(cases::native_cases()))
}
