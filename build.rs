fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").expect("Cargo provides the target OS");
    assert!(
        matches!(target_os.as_str(), "windows" | "macos"),
        "lyrel desktop supports Windows and macOS only; for Linux use cargo build --workspace --exclude lyrel (lyrel-core, lyrel-server and lyrel-cli remain supported)"
    );
    if target_os == "macos" {
        // Keep the desktop runtime floor independent of the SDK selected for Fast.
        println!("cargo:rustc-link-arg-bin=lyrel-app=-mmacosx-version-min=15.0");
    }
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/favicon.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/favicon.ico");
        // 产品／发布者元数据不等同于 Authenticode 证书身份。
        res.set("ProductName", "lyrel");
        res.set("FileDescription", "lyrel");
        res.set("CompanyName", "dlivxpr");
        if let Err(e) = res.compile() {
            println!("cargo:warning=failed to embed Windows icon: {e}");
        }
    }
}
