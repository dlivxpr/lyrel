#[cfg(windows)]
mod conpty_files;

fn main() {
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        verify_conpty().unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1);
        });
    }
}

#[cfg(windows)]
fn verify_conpty() -> std::io::Result<()> {
    use std::path::{Path, PathBuf};

    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    // A Windows target with no pinned package is an error, not a skip.
    let Some(arch) = conpty_files::architecture(&arch) else {
        return Err(std::io::Error::other(format!(
            "no pinned ConPTY package exists for the Windows target architecture `{arch}` \
             (supported: x86_64)"
        )));
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest = root.join("assets/windows/conpty/manifest.json");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-changed=conpty_files.rs");
    println!("cargo:rerun-if-env-changed=LYREL_CONPTY_DIR");
    let source = std::env::var_os("LYREL_CONPTY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".conpty"));
    let source = if source.is_absolute() {
        source
    } else {
        root.join(source)
    };
    for path in [
        source.join(arch).join("conpty.dll"),
        source.join(arch).join("OpenConsole.exe"),
        source.join("LICENSE-ConPTY.txt"),
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    conpty_files::read_verified(&source, arch, true)?;
    Ok(())
}
