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

use crate::host_ops::HostId;

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

/// The spelling the OS opener expects. POSIX paths are unchanged; on Windows
/// the shell expects native separators and upper-case drive letters.
fn spelled_for_shell(path: &std::path::Path) -> std::path::PathBuf {
    #[cfg(windows)]
    if let Some(text) = path.to_str() {
        return windows_spelling(text).into();
    }
    // A Windows path can contain unpaired surrogates. Normalize separators
    // without replacing any code units; a lossy conversion names another file.
    lyrel_protocol::core::path_spelling::native_separators(path).into_owned()
}

/// A drive letter and everything after its colon, only for rooted paths.
pub fn drive_root(path: &str) -> Option<(char, &str)> {
    let drive = path.chars().next().filter(char::is_ascii_alphabetic)?;
    let rest = path.strip_prefix(drive)?.strip_prefix(':')?;
    rest.starts_with(['\\', '/']).then_some((drive, rest))
}

/// Native Windows spelling, also used to construct links for remote Windows panes.
pub fn windows_spelling(path: &str) -> String {
    let mut out = path.replace('/', "\\");
    if let Some((drive, _)) = drive_root(&out)
        && drive.is_ascii_lowercase()
    {
        out.replace_range(..1, &drive.to_ascii_uppercase().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_path_handed_to_the_os_is_spelled_its_way() {
        #[cfg(windows)]
        {
            assert_eq!(
                spelled_for_shell(Path::new("c:/Users/me/a b.png")).as_os_str(),
                r"C:\Users\me\a b.png"
            );
            assert_eq!(
                spelled_for_shell(Path::new(r"\\srv/share/x")).as_os_str(),
                r"\\srv\share\x"
            );
        }
        #[cfg(not(windows))]
        assert_eq!(
            spelled_for_shell(Path::new(r"/Users/me/a\b.png")).as_os_str(),
            r"/Users/me/a\b.png"
        );
    }
}
