fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").expect("Cargo provides the target OS");
    assert!(
        matches!(target_os.as_str(), "windows" | "macos"),
        "lyrel desktop supports Windows and macOS only; for Linux use cargo build --workspace --exclude lyrel (lyrel-core, lyrel-server and lyrel-cli remain supported)"
    );
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/favicon.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/favicon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=failed to embed Windows icon: {e}");
        }
    }
}
