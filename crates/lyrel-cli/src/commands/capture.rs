use anyhow::Result;
use serde_json::json;

use super::{Outcome, report};
use crate::address::{self, Context};
use crate::backend::Backend;
use crate::cli::CaptureArgs;
use crate::{output, screen};

pub(super) fn capture(args: CaptureArgs, ctx: &Context, backend: &mut dyn Backend) -> Result<Outcome> {
    let pane = address::pane_or_context(args.target.as_deref(), ctx)?;
    let segments = backend.capture(pane, args.scrollback)?;
    // How much the replay actually carried, counted before anything renders or
    // trims it. An empty answer used to be one fact — "nothing came back" — and
    // a caller could not tell a blank pane from a capture that lost its bytes
    // on the way (#841). With this beside it the two read differently: zero
    // bytes is a pane that printed nothing, and bytes with no text is a screen
    // whose content did not survive the grid.
    let replayed: usize = segments.iter().map(|segment| segment.bytes.len()).sum();
    // Raw is the default and stays byte-for-byte what the daemon stored, joined
    // in replay order; `--plain` hands the same bytes to a grid instead. Either
    // way `--json` carries whatever was printed, so a caller reads one field.
    let rendered = if args.plain {
        screen::render(&segments)
    } else {
        let bytes: Vec<u8> = segments.into_iter().flat_map(|s| s.bytes).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    // Said once, on stderr, where it cannot corrupt stdout or the JSON line —
    // and reported as the observation it is, not as a diagnosis. Only `--plain`
    // can reach it: lossy UTF-8 never turns bytes into an empty string, so the
    // raw form's text is empty exactly when the replay was.
    if rendered.is_empty() && replayed > 0 {
        eprintln!(
            "lyrel: capture %{pane}: {replayed} bytes replayed and the grid they \
             drive is blank — this empty result is the pane's screen, not a \
             capture that came back short"
        );
    }
    // After the note, not before: a tail is a view of the answer, and whether
    // the answer itself was blank is a fact about the pane either way.
    let text = match args.tail {
        Some(keep) => last_lines(&rendered, keep as usize),
        None => rendered,
    };
    report(
        text.clone(),
        json!({ "pane": pane, "text": text, "bytes": replayed }),
    )
}

/// The last `keep` lines of `text`, counted the way `tail -n` counts them.
///
/// A trailing newline terminates the last line rather than opening an empty
/// one, so `tail -n 1` of `"a\nb\n"` is `"b\n"` and not `""`. Splitting on
/// `\n` alone leaves the `\r` of a CRLF attached to the line it ended, which
/// is what the raw form is supposed to hand back byte-for-byte.
fn last_lines(text: &str, keep: usize) -> String {
    let (body, trailer) = match text.strip_suffix('\n') {
        Some(body) => (body, "\n"),
        None => (text, ""),
    };
    let start = body
        .rmatch_indices('\n')
        .nth(keep.saturating_sub(1))
        .map(|(at, _)| at + 1)
        .unwrap_or(0);
    format!("{}{trailer}", &body[start..])
}

pub(super) fn procs(target: Option<&str>, ctx: &Context, backend: &mut dyn Backend) -> Result<Outcome> {
    let pane = address::pane_or_context(target, ctx)?;
    let procs = backend.procs(pane)?;
    report(output::procs_tables(&procs), serde_json::to_value(&procs)?)
}

#[cfg(test)]
mod tests;
