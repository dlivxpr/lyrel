use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};

pub fn launch(path: Option<&Path>) -> Result<()> {
    let executable = find_executable()?;
    let mut command = Command::new(&executable);
    if let Some(path) = path {
        command.arg("--open-path").arg(path);
    }

    // The CLI must not keep a caller's redirected pipes alive after it exits.
    // The GUI owns its own logging and never needs this console's standard IO.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("launching {}", executable.display()))?;
    Ok(())
}

/// The bundled `lyrel-app` on a supported desktop: `LYREL_APP`, then the file
/// next to this executable, then `PATH`. Headless hook diagnosis uses the server.
pub fn find_executable() -> Result<PathBuf> {
    if !cfg!(any(target_os = "macos", target_os = "windows")) {
        bail!(
            "lyrel desktop supports Windows and macOS only; use lyrel-server and CLI subcommands on this platform (`lyrel --help`)"
        );
    }

    if let Some(explicit) = std::env::var_os("LYREL_APP") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Ok(path);
        }
        bail!(
            "LYREL_APP points to {}, but that file does not exist",
            path.display()
        );
    }

    let name = executable_name();
    if let Ok(own) = std::env::current_exe()
        && let Some(dir) = own.parent()
    {
        let sibling = dir.join(name);
        if sibling.is_file() {
            return Ok(sibling);
        }
    }

    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    bail!("could not find {name} next to this CLI or on PATH — install lyrel-app, or set LYREL_APP")
}

fn executable_name() -> &'static str {
    if cfg!(windows) {
        "lyrel-app.exe"
    } else {
        "lyrel-app"
    }
}
