//! Refuse a silent system-ConPTY fallback when an installed pair is incomplete.
#[path = "../../conpty_files.rs"]
mod files;

pub(crate) fn ensure_prepared() -> std::io::Result<()> {
    static VERIFIED: std::sync::LazyLock<Result<(), String>> = std::sync::LazyLock::new(|| {
        // An architecture with no pinned package is refused outright: silently
        // skipping verification is what used to let a build drift onto the
        // in-box conhost. Only the targets the release publishes are covered;
        // anything else is not a supported Windows build.
        let arch = files::architecture(std::env::consts::ARCH).ok_or_else(|| {
            format!(
                "no pinned ConPTY package exists for this Windows architecture ({}); \
                 supported: x86_64",
                std::env::consts::ARCH
            )
        })?;
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let directory = executable
            .parent()
            .ok_or("ConPTY executable directory is unavailable")?;
        files::read_verified(directory, arch, false).map_err(|e| e.to_string())?;
        Ok(())
    });
    VERIFIED.as_ref().map(|_| ()).map_err(|message| {
        std::io::Error::other(format!(
            "{message}; restore the pinned ConPTY set (conpty.dll, OpenConsole.exe, \
             LICENSE-ConPTY.txt) beside this executable — from the matching lyrel release \
             package, or `pwsh scripts/prepare-conpty.ps1` in a source checkout — and \
             restart lyrel"
        ))
    })
}
