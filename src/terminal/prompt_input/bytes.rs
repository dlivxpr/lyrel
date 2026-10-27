pub(in crate::terminal) fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    lyrel_core::core::paste::paste_bytes(text.as_bytes(), bracketed)
}

/// A line the shell can be handed byte for byte, as if it had been typed at its
/// own prompt.
///
/// Control characters are what rule a line out. Under bracketed paste the shell
/// inserts every byte literally; delivered raw, each one runs through the line
/// editor's binding table instead, and a Tab completes, a `^U` kills, a `^C`
/// abandons the line. ESC is already stripped upstream; `is_control` covers the
/// rest, embedded newlines included — a multi-line command still goes as one
/// paste, which is what [`submit_bytes`] was built for.
///
/// Printable keys are deliberately *not* excluded, and cannot be: a line editor
/// binding one is exactly the mechanism this exists to reach. fish binds space
/// to `expand-abbr`; zsh users bind `.` to `rationalise-dot` and quotes to
/// zsh-autopair. Reaching the first means reaching the others, which is why the
/// caller keeps pasted text away from this path entirely.
///
/// The length bound is a cost ceiling, not a correctness one, and it is a
/// policy dial rather than a discontinuity in the data. A paste lands in one
/// go; raw bytes make the shell's line editor redraw as it consumes them, so
/// the added latency is linear in length from the very first byte — measured on
/// a pty it rises perfectly smoothly, with no knee to hang a bound on.
///
/// What 512 buys is a ceiling on that latency. The worst configuration measured
/// is zsh with zsh-syntax-highlighting and zsh-autosuggestions, which both
/// re-run per keystroke: ~0.26 ms per byte, so +15 ms on a typical 60-byte
/// command and +126 ms at the bound. Bare zsh is ~0.012 ms per byte (+6 ms at
/// the bound), bash is free at every length, and fish — the shell #660 is about
/// — shows no penalty this harness can resolve. Halving the bound would halve
/// the worst case; the number is a judgement about how much latency a long
/// typed line may pay, not something the curve picks out.
pub(in crate::terminal) fn types_cleanly(line: &str) -> bool {
    line.len() <= 512 && !line.chars().any(char::is_control)
}

/// Build the byte sequence that submits the local editor's buffer to the shell.
///
/// A plain single-line command the user *typed* goes raw, no paste markers: the
/// shell's own reader then sees the same bytes typing at its prompt would
/// produce, so its input-time expansions run — fish abbreviations (#660), zsh
/// `magic-space`, a readline macro on a printable key. Inside a bracketed paste
/// none of that fires; fish's expand-on-execute only reaches the token under
/// the cursor, so `j` expanded but `j build` ran literally.
///
/// `pasted` is what keeps that from rewriting text the user did not type. A
/// paste's contract is that what went in is what runs, and a fish user with
/// `abbr -a l 'ls -la'` pasting `l /tmp` from their notes must not get
/// `ls -la /tmp`. So a line that has carried clipboard content keeps the paste
/// framing whatever else is true of it — see [`crate::terminal::cmd_editor::CmdEditor::pasted`].
///
/// Multi-line and control characters keep it too. A multi-line command goes in
/// as one paste and one CR, so it costs one prompt cycle instead of one per
/// line — preexec, the user's precmd chain, a syntax-highlight pass over the
/// whole buffer — and zle keeps the embedded newlines in its buffer, so
/// backslash / open-quote continuation and heredocs still parse as one unit.
///
/// ESC is stripped either way (unlike the paste path): clipboard text carrying
/// its own `ESC[201~` could otherwise close the paste early and have the rest
/// run as typed input, and a raw ESC reaching zle is an editor command.
///
/// An empty buffer skips the markers: zsh's `bracketed-paste-magic` (which
/// oh-my-zsh turns on) errors on a paste with nothing between them.
pub(in crate::terminal) fn submit_bytes(line: &str, bracketed: bool, pasted: bool) -> Vec<u8> {
    let clean: String = line
        .replace("\r\n", "\n")
        .chars()
        .filter(|&c| c != '\x1b')
        .map(|c| if c == '\r' { '\n' } else { c })
        .collect();
    let framed = bracketed && !clean.is_empty() && (pasted || !types_cleanly(&clean));
    let mut bytes = paste_bytes(&clean, framed);
    bytes.push(b'\r');
    bytes
}

#[cfg(test)]
#[path = "bytes_tests.rs"]
mod tests;
