#[cfg(windows)]
mod conpty_files;

fn main() {
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        stage_conpty().unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1);
        });
    }
}

#[cfg(windows)]
fn stage_conpty() -> std::io::Result<()> {
    use std::path::{Path, PathBuf};

    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    // A Windows target with no pinned package is an error, not a skip: quietly
    // staging nothing would ship a build that fails every pane at runtime.
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
    let files = conpty_files::read_verified(&source, arch, true)?;
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    let target = out.ancestors().nth(3).expect("Cargo build output layout");
    // CLI, server, GUI and their test executables all depend on this crate.
    // Keep the fixed pair beside each executable, including isolated -p builds.
    for directory in [target.to_path_buf(), target.join("deps")] {
        std::fs::create_dir_all(&directory)?;
        for (name, bytes) in &files {
            let destination = directory.join(name);
            println!("cargo:rerun-if-changed={}", destination.display());
            if std::fs::read(&destination).ok().as_deref() == Some(bytes.as_slice()) {
                continue;
            }
            std::fs::write(&destination, bytes).map_err(|e| {
                std::io::Error::new(
                    e.kind(),
                    format!(
                        "cannot stage ConPTY {}: {e}; {}",
                        destination.display(),
                        conpty_files::PREPARE_HINT
                    ),
                )
            })?;
        }
    }
    Ok(())
}
