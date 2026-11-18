#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

fn main() {
    #[cfg(target_os = "macos")]
    let result = lyrel_updater::macos::run();
    #[cfg(target_os = "windows")]
    let result = lyrel_updater::windows::run();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let result: Result<(), String> =
        Err("lyrel-updater supports Windows and macOS only; Linux ships the headless server and CLI".to_string());
    if let Err(error) = result {
        eprintln!("lyrel-updater: {error}");
        std::process::exit(1);
    }
}
