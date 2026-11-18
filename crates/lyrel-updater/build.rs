fn main() {
    // The executable relocates with the platform package, so it carries the
    // same identity resources the GUI binary does: the application icon and
    // the PE version metadata the release verifiers read back (see
    // `verify-windows-package.ps1` and `fixed_file_version` in this crate).
    #[cfg(windows)]
    {
        // Absolute so the resource compiler resolves it from its own working
        // directory regardless of where Cargo invokes the build script from;
        // the icon lives at the repository root, two levels up from here.
        let icon =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/favicon.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        let mut res = winresource::WindowsResource::new();
        res.set_icon(icon.to_str().expect("the icon path is valid UTF-8"));
        // 产品／发布者元数据不等同于 Authenticode 证书身份。
        res.set("ProductName", "lyrel");
        res.set("FileDescription", "lyrel");
        res.set("CompanyName", "dlivxpr");
        if let Err(e) = res.compile() {
            println!("cargo:warning=failed to embed Windows icon: {e}");
        }
    }
    if std::env::var("CARGO_CFG_TARGET_OS").expect("Cargo provides the target OS") == "macos" {
        // Keep the desktop runtime floor independent of the SDK selected for
        // Fast, exactly as the GUI binary's build script does.
        println!("cargo:rustc-link-arg-bin=lyrel-updater=-mmacosx-version-min=15.0");
    }
}
