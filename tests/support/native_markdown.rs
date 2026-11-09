//! Real native font consumers, executed on the process main thread on every desktop OS.
use gpui_kit::{PlatformTextSystem, TestAppContext};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::process::ExitCode;
use std::sync::Arc;

pub(crate) struct Case {
    name: &'static str,
    run: fn(&mut TestAppContext),
}

impl Case {
    pub(crate) const fn new(name: &'static str, run: fn(&mut TestAppContext)) -> Self {
        Self { name, run }
    }

    fn execute(&self, text_system: Arc<dyn PlatformTextSystem>) {
        let name = self.name;
        let run = self.run;
        let text_system = AssertUnwindSafe(move || text_system);
        gpui_kit::run_test_once(
            0,
            Box::new(move |dispatcher: gpui_kit::TestDispatcher| {
                let mut cx = TestAppContext::build_with_text_system(
                    dispatcher.clone(),
                    Some(name),
                    text_system(),
                );
                let result = catch_unwind(AssertUnwindSafe(|| run(&mut cx)));
                cx.run_until_parked();
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
                drop(cx);
                dispatcher.drain_tasks();
                if let Err(error) = result {
                    resume_unwind(error);
                }
            }),
        );
    }
}

fn usage() {
    println!(
        "native_markdown [FILTER ...] [--exact] [--list] [--nocapture] [--test-threads=1]\n\
         Runs real native-font Markdown consumers on the process main thread.\n\
         FILTER matches the original unit-test selector (substring, or full name with --exact).\n\
         Unknown exact selectors and unsupported options fail with exit code 2."
    );
}

pub(crate) fn main() -> ExitCode {
    let mut filters = Vec::new();
    let mut exact = false;
    let mut list = false;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--list" => list = true,
            "--exact" => exact = true,
            "--nocapture" | "--test-threads=1" => {}
            "--help" | "-h" => {
                usage();
                return ExitCode::SUCCESS;
            }
            _ if argument.starts_with('-') => {
                eprintln!("native_markdown: unsupported option {argument}");
                usage();
                return ExitCode::from(2);
            }
            _ => filters.push(argument),
        }
    }
    let cases: Vec<_> = crate::ui::code_editor::markdown::native_cases().collect();
    if exact {
        for filter in &filters {
            if !cases.iter().any(|case| case.name == filter) {
                eprintln!("native_markdown: unknown exact test selector {filter:?}");
                return ExitCode::from(2);
            }
        }
    }
    let selected: Vec<_> = cases
        .iter()
        .filter(|case| {
            filters.is_empty()
                || filters.iter().any(|filter| {
                    if exact {
                        case.name == filter
                    } else {
                        case.name.contains(filter)
                    }
                })
        })
        .collect();
    if list {
        for case in &selected {
            println!("{}: test", case.name);
        }
        println!("\n{} tests, 0 benchmarks", selected.len());
        return ExitCode::SUCCESS;
    }
    if selected.is_empty() {
        println!("running 0 tests");
        println!(
            "\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; {} filtered out",
            cases.len(),
        );
        return ExitCode::SUCCESS;
    }

    crate::core::config::pin_test_config_dir();

    // Keep the native platform alive until all independent test contexts are gone.
    // Only its Send + Sync text system crosses into the deterministic test platform;
    // no native window or AppKit object is constructed on a libtest worker.
    // A permanently headless Windows platform deliberately supplies NoopTextSystem.
    // The window-capable platform supplies real fonts without opening a test window.
    let platform = gpui_kit::platform::current_platform(false);
    let text_system = platform.text_system();
    assert!(
        !text_system.all_font_names().is_empty(),
        "native_markdown requires real platform fonts, not NoopTextSystem"
    );
    println!("running {} tests", selected.len());
    let mut failures = Vec::new();
    for case in &selected {
        println!("test {} ...", case.name);
        match catch_unwind(AssertUnwindSafe(|| case.execute(text_system.clone()))) {
            Ok(()) => println!("test {} ... ok", case.name),
            Err(_) => {
                println!("test {} ... FAILED", case.name);
                failures.push(case.name);
            }
        }
    }
    drop(text_system);
    drop(platform);
    let passed = selected.len() - failures.len();
    if !failures.is_empty() {
        println!("\nfailures:");
        for name in &failures {
            println!("    {name}");
        }
    }
    println!(
        "\ntest result: {}. {passed} passed; {} failed; 0 ignored; 0 measured; {} filtered out",
        if failures.is_empty() { "ok" } else { "FAILED" },
        failures.len(),
        cases.len() - selected.len(),
    );
    if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
