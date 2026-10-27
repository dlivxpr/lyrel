use std::path::PathBuf;

use crate::core::osc::OscTokenizer;

#[derive(Default, Clone, PartialEq, Eq)]
pub(super) struct ShellState {
    pub(super) active: bool,
    pub(super) at_prompt: bool,
    /// `at_prompt` as the marks themselves read it, kept out of the
    /// suppression the reader applies when a foreground program is running.
    ///
    /// Two different questions share one pair of marks. "Should the local line
    /// editor engage?" must say no while `ssh` owns the pty, whoever drew the
    /// prompt — that is `at_prompt`. "Is anything running in this pane?" wants
    /// the opposite: the prompt an `ssh` is relaying belongs to the shell that
    /// is actually driving the pane, and it is the only thing this side can
    /// read about the far one. Keeping both means neither answer has to be
    /// derived from the other's.
    pub(super) mark_at_prompt: bool,
    pub(super) last_exit_code: Option<i32>,
    pub(super) command: Option<String>,
}

#[derive(Default)]
pub(super) struct SniffSignals {
    pub(super) cwd: Option<PathBuf>,
    /// The last title the pane set in this read, already capped. `Some("")` is
    /// a reset — an empty OSC 0/2 clears the title rather than setting a blank
    /// one, the same way the GUI's terminal treats it.
    pub(super) title: Option<String>,
    pub(super) shell: Vec<ShellState>,
    pub(super) agent_events: Vec<crate::core::cli_agent::AgentEvent>,
    pub(super) notification: Option<String>,
}

pub(super) struct OscSniffer {
    tok: OscTokenizer,
    pub(super) shell: ShellState,
    /// Whether the title the pane is showing still belongs to something that
    /// is running — see [`crate::core::osc::TitleLifetime`] (#889).
    title_life: crate::core::osc::TitleLifetime,
    notes: crate::core::osc::Notifications,
}

impl OscSniffer {
    pub(super) fn new() -> Self {
        Self {
            tok: OscTokenizer::new(&[b"0", b"2", b"7", b"133", b"9", b"99", b"777"]),
            shell: ShellState::default(),
            title_life: crate::core::osc::TitleLifetime::default(),
            notes: crate::core::osc::Notifications::default(),
        }
    }

    pub(super) fn feed(&mut self, bytes: &[u8]) -> SniffSignals {
        let mut signals = SniffSignals::default();
        let shell = &mut self.shell;
        let title_life = &mut self.title_life;
        let notes = &mut self.notes;
        self.tok.feed(bytes, |payload| {
            // In stream order, so that a shell which re-titles itself right
            // after the `D` mark gets the last word over the retirement.
            if title_life.saw(payload) == crate::core::osc::TitleEffect::Retire {
                signals.title = Some(String::new());
            }
            if let Some(path) = parse_osc7(payload) {
                signals.cwd = Some(path);
            } else if let Some(title) = parse_osc_title(payload) {
                signals.title = Some(title);
            } else if let Some(rest) = payload.strip_prefix(b"133;") {
                if handle_osc133(shell, rest) {
                    match signals.shell.last_mut() {
                        Some(last) if last.at_prompt == shell.at_prompt => {
                            *last = shell.clone();
                        }
                        _ => signals.shell.push(shell.clone()),
                    }
                }
            } else if let Some(event) = crate::core::cli_agent::parse_agent_event(payload) {
                signals.agent_events.push(event);
            } else if let Some((title, body)) = notes.parse(payload) {
                if title.as_deref() != Some(crate::core::cli_agent::AGENT_EVENT_SENTINEL) {
                    signals.notification = Some(body);
                }
            }
        });
        signals
    }
}

fn handle_osc133(shell: &mut ShellState, rest: &[u8]) -> bool {
    // Generic OSC 133 blocks may be emitted by the foreground TUI, including
    // when replaying a transcript. Only our shell integration owns its argv.
    let command_mark = rest.strip_suffix(b";aid=lyrel");
    let rest = command_mark.unwrap_or(rest);
    shell.active = true;
    match rest.first() {
        Some(b'A') | Some(b'B') => shell.at_prompt = true,
        Some(b'C') => {
            shell.at_prompt = false;
            if command_mark.is_some() {
                shell.command = rest
                    .strip_prefix(b"C;")
                    .map(|c| String::from_utf8_lossy(&percent_decode(c)).into_owned())
                    .filter(|s| !s.trim().is_empty());
            }
        }
        Some(b'D') => {
            shell.at_prompt = true;
            if command_mark.is_some() {
                shell.command = None;
            }
            shell.last_exit_code = rest
                .strip_prefix(b"D;")
                .and_then(|c| std::str::from_utf8(c).ok())
                .and_then(|s| s.trim().parse::<i32>().ok());
        }
        _ => return false,
    }
    // The unsuppressed twin, set here so every arm gets it and no future arm
    // can forget to. Suppression happens later, on the reader thread, and only
    // ever touches `at_prompt`.
    shell.mark_at_prompt = shell.at_prompt;
    true
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    let s = String::from_utf8_lossy(bytes);
    PathBuf::from(strip_uri_drive_slash(s.as_ref()))
}

#[cfg_attr(unix, allow(dead_code))]
pub(super) fn strip_uri_drive_slash(path: &str) -> &str {
    let b = path.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        &path[1..]
    } else {
        path
    }
}

pub(crate) fn parse_osc7(payload: &[u8]) -> Option<PathBuf> {
    let rest = payload.strip_prefix(b"7;")?;
    let path_bytes: &[u8] = if let Some(after) = rest.strip_prefix(b"file://") {
        let idx = after.iter().position(|&c| c == b'/')?;
        &after[idx..]
    } else if rest.first() == Some(&b'/') {
        rest
    } else {
        return None;
    };
    let decoded = percent_decode(path_bytes);
    if decoded.is_empty() {
        return None;
    }
    Some(path_from_bytes(&decoded))
}

/// Longest title the tree will keep. The window that owns the pane shows about
/// forty columns of it; the rest is only ever paid for — in `machine.json`, and
/// in a `PaneFacts` delta to every attached client — so it is cut here rather
/// than at each place that renders one.
pub(super) const MAX_OSC_TITLE: usize = 256;

/// The title from an OSC 0 (icon *and* window) or OSC 2 (window) payload,
/// capped. An empty payload comes back as `Some("")`: that is a reset, which
/// the caller has to tell apart from "no title in this read".
///
/// OSC 1 is deliberately not read. It sets the icon name alone, which the GUI's
/// terminal ignores — and the point of recording a title at all is to agree
/// with what the GUI shows.
pub(crate) fn parse_osc_title(payload: &[u8]) -> Option<String> {
    let rest = payload
        .strip_prefix(b"0;")
        .or_else(|| payload.strip_prefix(b"2;"))?;
    let title = String::from_utf8_lossy(rest);
    let title = title.trim();
    Some(match title.chars().count() > MAX_OSC_TITLE {
        true => title.chars().take(MAX_OSC_TITLE).collect(),
        false => title.to_string(),
    })
}

pub(super) fn percent_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' && i + 2 < input.len() {
            if let (Some(h), Some(l)) = (hex_val(input[i + 1]), hex_val(input[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

pub(super) fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
