//! The shared file-open seam: the typed request a pane's file link emits and
//! the external launchers that hand a path to the OS. Owned here — not by the
//! terminal — because the editor and the file tree open files the same ways
//! the terminal's links do, and none of them should reach into `TerminalView`
//! state for it.
//!
//! Everything in this module is a value or a process spawn; no pane, no shell
//! controller. The terminal resolves a link's path (it alone knows the pane's
//! directory and host) and emits [`OpenFileRequested`]; the app owns opening
//! it, because the editor and the file tree are its to drive.

use crate::ui::host_ops::HostId;

/// A file link the user clicked, on its way to whoever can show it. The
/// terminal resolves the path — it is the only thing that knows the pane's
/// directory and host — and the app opens it, because the editor and the file
/// tree are its to drive.
pub struct OpenFileRequested {
    pub host_id: HostId,
    pub path: std::path::PathBuf,
    pub line: Option<u32>,
    pub column: Option<u32>,
    /// A directory link asks "where is this?", not "what does it say?" — the
    /// app answers it in the file tree instead of the editor.
    pub is_dir: bool,
}

/// Hands a path to whatever the OS has it associated with. Also the fallback
/// for a directory the file tree cannot reach.
///
/// The path is spelled this OS's way first: Explorer takes `c:/a.png` as a
/// request to show Documents — the separators get rewritten and a lowercase
/// drive letter upper-cased before the spawn.
pub fn open_file_path(path: &std::path::Path) -> std::io::Result<()> {
    let path = &spelled_for_shell(path);
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener).arg(path).spawn()?;
    Ok(())
}

/// Shows a path where it lives, rather than opening it.
///
/// `open -R` and `explorer /select,` both select the file inside its folder;
/// no desktop-neutral Linux equivalent exists, so there the folder is opened
/// and the file is left for the eye to find.
pub fn reveal_file_path(path: &std::path::Path) -> std::io::Result<()> {
    let path = &spelled_for_shell(path);
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(path);
        c
    };
    // Explorer wants `/select,` bare and the path quoted behind it. `arg`
    // quotes the whole thing the moment the path holds a space, and Explorer
    // answers a quoted switch by opening Documents and reporting success —
    // so the command line is written out by hand.
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("explorer");
        c.raw_arg(format!("/select,\"{}\"", path.display()));
        c
    };
    #[cfg(all(not(target_os = "macos"), not(windows)))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    command.spawn()?;
    Ok(())
}

/// The spelling the OS opener expects: this machine's separators with a
/// lowercase drive letter upper-cased. `IShellFolder::ParseDisplayName`
/// rejects mixed-separator paths, and Explorer answers a lowercase drive by
/// showing Documents — so the rewrite happens here, once, for every launcher
/// above. The separator half lives in `lyrel-core` next to the storage
/// spelling; only the drive-casing half is the shell's rule, kept here.
fn spelled_for_shell(path: &std::path::Path) -> std::path::PathBuf {
    let native = lyrel_core::core::path_spelling::native_separators(path);
    #[cfg(windows)]
    {
        use std::path::PathBuf;
        let text = native.to_string_lossy().replace('/', "\\");
        let mut chars = text.chars();
        let drive = chars.next().filter(char::is_ascii_alphabetic);
        let rooted = drive.is_some_and(|d| text[1..].starts_with(':') && {
            text[2..].starts_with('\\') && d.is_ascii_lowercase()
        });
        match rooted {
            true => {
                let mut out = text;
                out.replace_range(..1, &drive.unwrap().to_ascii_uppercase().to_string());
                PathBuf::from(out)
            }
            false => PathBuf::from(text),
        }
    }
    #[cfg(not(windows))]
    {
        native.into_owned()
    }
}
