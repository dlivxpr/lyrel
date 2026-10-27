use anyhow::{Context as _, Result, bail};
use serde_json::json;
use std::time::Duration;

use super::{Outcome, report};
use crate::address::{self, Context};
use crate::backend::Backend;
use crate::cli::SendArgs;

pub(super) fn send(args: SendArgs, ctx: &Context, backend: &mut dyn Backend) -> Result<Outcome> {
    // `--enter` is the same thing as `--key enter`, and predates it. Keeping it
    // as sugar rather than deprecating it: it reads better for the overwhelming
    // case, which is typing one command and running it. Going through the same
    // parser leaves one definition of what Enter puts on the wire — and the list
    // is built here, before the dispatch, because the dispatch has to count it:
    // `send %42 --enter` used to report "needs TEXT … or a --key to press" while
    // the docs called `--enter` shorthand for exactly such a key (#581).
    let mut pressed = args.keys.clone();
    if args.enter {
        pressed.push(crate::keys::parse("enter").expect("enter is in the vocabulary"));
    }

    // Text from stdin or a file has no positional of its own, so the one
    // positional there is can only be an address — and a word that is not one
    // is a mistake, not text to type into the caller's own pane.
    if args.stdin || args.from_file.is_some() {
        let source = if args.stdin { "--stdin" } else { "--from-file" };
        if let Some(first) = &args.first {
            address::parse_pane(first).with_context(|| {
                format!(
                    "with {source} the text comes from there, so '{first}' has to be the \
                     pane to send it to"
                )
            })?;
        }
        let pane = address::pane_or_context(args.first.as_deref(), ctx)?;
        let payload = match &args.from_file {
            Some(path) => {
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?
            }
            None => {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut std::io::stdin(), &mut bytes)
                    .context("reading stdin")?;
                bytes
            }
        };
        return deliver(
            pane,
            Payload::Piped {
                source: if args.stdin { "stdin" } else { "file" },
                bytes: payload,
            },
            &args,
            pressed,
            backend,
        );
    }

    // Three shapes reach here, and only the address is ever ambiguous:
    // `send %3 "text"`, `send "text"` (this pane), and — new with --key —
    // `send %3 --key C-c`, where there is no text at all and the lone
    // positional is therefore an address rather than the missing-text error it
    // has to stay in every other case. `send %3 --enter` is that third shape
    // too, with the one carve-out below: only a marked address is promoted by
    // `--enter` alone.
    //
    // The address-shaped-but-broken case must not fall through to "type it":
    // `send %3x --key C-c` used to type `%3x` into the *caller's own* pane and
    // then interrupt whatever was in front of them (#538). The guard is kept
    // narrow on purpose — `%` followed by a digit means "tried to write an
    // address", so the parse error propagates; `%` followed by anything else
    // (`%s/foo/bar/` driving vim's ex, `%!sort`) stays text, because refusing
    // it would break a use that was never ambiguous.
    let (target, text) = match (&args.first, &args.second) {
        (Some(first), Some(text)) => (Some(first.as_str()), Some(text.as_str())),
        (Some(first), None) => match address::parse_pane(first) {
            Ok(_) => {
                if pressed.is_empty() {
                    bail!(
                        "send needs TEXT after the pane address, or a --key to press \
                         — to type '{first}' literally, name the pane too: send %PANE {first}"
                    );
                }
                // A `--key` is always an explicit "press this", so it promotes
                // either spelling of the address. `--enter` is not, for an
                // *unmarked* id: `send 2 --enter` reads as "type 2 and run it"
                // far more often than "press Enter in pane 2", and #538 was
                // about never quietly retargeting a keystroke. The `%` is what
                // says which was meant, so it stays the loud error it is today.
                if args.keys.is_empty() && !first.starts_with('%') {
                    bail!(
                        "'{first}' is a bare pane id and --enter has nothing to type \
                         — to press Enter in pane {first}: send %{first} --enter; \
                         to type '{first}' and press Enter, name the pane too: \
                         send %PANE {first} --enter"
                    );
                }
                (Some(first.as_str()), None)
            }
            // `%` then a digit is someone writing an address, so the parse
            // error is the answer. Anything else is text and always was.
            Err(error) if tried_to_write_an_address(first) => return Err(error),
            Err(_) => (None, Some(first.as_str())),
        },
        (None, _) => {
            if pressed.is_empty() {
                bail!("send needs TEXT to type or a --key to press");
            }
            (None, None)
        }
    };

    let pane = address::pane_or_context(target, ctx)?;
    let payload = match text {
        Some(text) => Payload::Typed(text.to_string()),
        None => Payload::None,
    };
    deliver(pane, payload, &args, pressed, backend)
}

/// What `send` has to put in front of the keys.
enum Payload {
    None,
    /// From the command line — echoed back in `--json`, as it always was.
    Typed(String),
    /// From stdin or a file. Never echoed: the reason to use these is to keep
    /// the bytes out of places other people can read, and a JSON line in a
    /// log is one of them.
    Piped {
        source: &'static str,
        bytes: Vec<u8>,
    },
}

fn deliver(
    pane: u64,
    payload: Payload,
    args: &SendArgs,
    pressed: Vec<crate::keys::Key>,
    backend: &mut dyn Backend,
) -> Result<Outcome> {
    const KEY_GAP: Duration = Duration::from_millis(200);

    let (source, bytes) = match &payload {
        Payload::None => ("none", &[][..]),
        Payload::Typed(text) => ("argument", text.as_bytes()),
        Payload::Piped { source, bytes } => (*source, &bytes[..]),
    };
    if args.paste && bytes.is_empty() {
        bail!("--paste needs text to paste — TEXT, --stdin or --from-file");
    }
    // `--paste` frames the way the GUI's paste does, which is to ask the pane
    // first: brackets for a pane that switched mode 2004 on, and for one that
    // did not, the line breaks a keyboard would send. Forcing the brackets on
    // a pane that never asked for them would put `ESC[200~` in front of a
    // program that reads it as keystrokes. `None` is a server that cannot say.
    let mode = if args.paste {
        backend.procs(pane)?.context.and_then(|c| c.bracketed_paste)
    } else {
        None
    };
    let bracketed = mode == Some(true);
    if args.paste && !bracketed {
        eprintln!(
            "lyrel: pane %{pane} {} bracketed paste, so the text went unframed — each line \
             break was sent as Enter",
            if mode.is_none() {
                "is on a server too old to say whether it has"
            } else {
                "has not switched on"
            }
        );
    }
    let wire = if args.paste {
        lyrel_core::core::paste::paste_bytes(bytes, bracketed)
    } else {
        bytes.to_vec()
    };

    let mut already_wrote = false;
    if !wire.is_empty() {
        backend.send_input(pane, wire)?;
        already_wrote = true;
    }
    for key in &pressed {
        // Raw-mode TUIs detect a fast stream as pasted input and intentionally
        // absorb Enter as a newline — and a menu being driven by arrow keys has
        // the same problem. Let each keystroke leave the burst window on its
        // own, which is what makes a sequence land as a sequence. Nothing
        // precedes the first write, though, so an interrupt stays immediate.
        if already_wrote {
            std::thread::sleep(KEY_GAP);
        }
        backend.send_input(pane, key.bytes.clone())?;
        already_wrote = true;
    }
    report(
        "",
        json!({
            "pane": pane,
            "sent": match &payload {
                Payload::Typed(text) => Some(text.as_str()),
                Payload::None => Some(""),
                Payload::Piped { .. } => None,
            },
            "source": source,
            "bytes": bytes.len(),
            "paste": args.paste,
            // Whether the paste went framed, and what the pane said about the
            // mode — `null` when nothing was asked or the server cannot say.
            "bracketed": bracketed,
            "bracketed_paste_mode": mode,
            "enter": args.enter,
            "keys": pressed.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(),
        }),
    )
}

/// Whether a lone positional that failed to parse was reaching for an address
/// rather than being text. Only `%` followed by a digit qualifies: it is the
/// shape every pane address has, so `%3x` is a typo worth refusing, while
/// `%s/foo/bar/` and `%!sort` are the ex commands they look like. A bare `3x`
/// is not included — nothing marks it as an address, and it has always typed.
fn tried_to_write_an_address(s: &str) -> bool {
    s.strip_prefix('%')
        .and_then(|rest| rest.chars().next())
        .is_some_and(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests;
