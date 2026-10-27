use anyhow::{Context as _, Result, bail};
use lyrel_core::daemon::control::{ControlRequest, ReplyOk};
use serde_json::json;

use super::{Outcome, report};
use crate::backend::Backend;

pub(super) fn execute(
    path: Option<std::path::PathBuf>,
    machine: Option<&str>,
    backend: &mut dyn Backend,
) -> Result<Outcome> {
    match path {
        // clap has no subcommand for this word, so it landed in [PATH].
        // Treat a word that is not a path as the typo it almost certainly
        // is: launching the GUI for `lyrel statu` would hide the typo.
        Some(word) if !looks_like_a_path(&word) => bail!(
            "unknown subcommand '{}' — run `lyrel --help` for the list. \
             (A path in this position would open the GUI there, but \
             '{}' does not name one.)",
            word.display(),
            word.display(),
        ),
        path => launch_gui(path, machine, backend),
    }
}

/// Whether a bare word in the `[PATH]` position was meant as a path.
///
/// Anything with a separator, a leading `.`/`~`, or that actually exists on
/// disk counts. A plain word like `tree` or `statu` does not — it is a
/// mistyped subcommand, and saying so beats offering to open the GUI there.
fn looks_like_a_path(path: &std::path::Path) -> bool {
    path.to_str().is_some_and(|s| {
        s.starts_with('/')
            || s.starts_with('.')
            || s.starts_with('~')
            || s.contains('/')
            || s.contains('\\')
    }) || path.exists()
}

fn launch_gui(
    path: Option<std::path::PathBuf>,
    machine: Option<&str>,
    backend: &mut dyn Backend,
) -> Result<Outcome> {
    if let Some(machine) = machine {
        bail!(
            "`lyrel [PATH]` controls the GUI on this machine and cannot be combined with -m {machine}"
        );
    }

    let path = path.map(resolve_gui_path).transpose()?;
    let wire_path = path.as_deref().and_then(gui_wire_path);
    // A live GUI receives the request through the daemon. If the daemon itself
    // is absent, the same fallback as "no GUI registered" starts the app, which
    // will start its daemon during normal initialization.
    let request_path = path
        .is_none()
        .then_some(None)
        .or_else(|| wire_path.clone().map(Some));
    let delivered = match request_path {
        Some(path) => match backend.control(ControlRequest::GuiOpen {
            path,
            workspace: None,
        }) {
            Ok(ReplyOk::Bool(delivered)) => delivered,
            Ok(other) => bail!("the server answered GuiOpen with {other:?}"),
            Err(_) => false,
        },
        // The JSON control protocol cannot preserve a native non-Unicode path.
        // Launching the app does: Command passes the Path as an OsStr, and the
        // app keeps it locally when it cannot forward it to another process.
        None => false,
    };

    if !delivered {
        crate::gui::launch(path.as_deref())?;
    }
    report(
        "",
        json!({
            "path": wire_path,
            "delivered": delivered,
            "launched": !delivered,
        }),
    )
}

fn gui_wire_path(path: &std::path::Path) -> Option<String> {
    path.to_str().map(str::to_owned)
}

fn resolve_gui_path(raw: std::path::PathBuf) -> Result<std::path::PathBuf> {
    let expanded = raw.to_str().and_then(expand_home).unwrap_or(raw);
    // Do not canonicalize here: preserving the caller's junction or symlink
    // spelling keeps shell cwd reporting and tab labels consistent.
    let path = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .context("reading the current directory")?
            .join(expanded)
    };
    let metadata =
        std::fs::metadata(&path).with_context(|| format!("opening {}", path.display()))?;
    if !metadata.is_dir() {
        bail!("{} is not a directory", path.display());
    }
    Ok(path)
}

fn expand_home(raw: &str) -> Option<std::path::PathBuf> {
    let rest = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\"));
    if raw != "~" && rest.is_none() {
        return None;
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)?;
    Some(match rest {
        Some(rest) => home.join(rest),
        None => home,
    })
}

#[cfg(test)]
mod tests;
