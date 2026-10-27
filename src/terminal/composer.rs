//! The message composer: a real text box for writing a prompt to a coding
//! agent the way a chat box lets you — mouse selection, the platform's own
//! editing keys, an IME with its candidates in place, files attached as chips,
//! `/` and `@` menus — and handing it over whole.
//!
//! **Where it sits.** For an agent whose input area lyrel can find on the grid
//! (omp, Kimi Code, pi, OpenCode) the box is laid *over* that area: the agent's
//! own input line and the status rows under it are covered, so the pane has
//! one input rather than two. Whenever the agent puts something else there —
//! a permission prompt, a model picker, a question — the area stops looking
//! like an input, the box steps aside, and the keyboard goes to the TUI until
//! the input comes back. Any other agent gets the box docked under the grid,
//! taking its rows from it.
//!
//! **What it sends.** Sending is typing, not an API: the text and then Enter,
//! written to the pty — see [`submit_plan`] for why each agent is spoken to
//! slightly differently. The toolbar's controls are the agent's own keys
//! (Shift+Tab, its `/model` and `/effort` commands), and what they show is
//! what the agent said: the mode off the status row the box covers, the model
//! and context off what its hooks report ([`crate::core::cli_agent::AgentReadout`]). Nothing reported
//! means nothing shown.

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{self, InputEvent, RopeExt as _, Textarea, TextareaState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, Size, h_flex};
use gpui_kit::{
    Context, Entity, ExternalPaths, Focusable as _, MouseButton, MouseDownEvent, Pixels,
    SharedString, Subscription, Window, div, prelude::*, px,
};
use rio_vt::config::colors::{AnsiColor, ColorRgb};
use rio_vt::crosswords::grid::Dimensions as _;
use rio_vt::crosswords::pos::{Column, Line, Pos};
use rio_vt::crosswords::square::Wide;
use rio_vt::crosswords::style::StyleFlags;
use rio_vt::crosswords::{Crosswords, Mode};
use rio_vt::event::EventListener;

use super::prompt_input::bytes::types_cleanly;
use super::view::{GRID_PAD_X, GRID_PAD_Y, TerminalView, pasted_paths_text};
use crate::core::cli_agent::{AgentStatus, CLIAgent};
use crate::ui::host_ops::{HostId, HostOps};
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::search::files::{FileIndex, FileList, IndexedFile, rank, walk};

/// How tall the text may grow, in lines, before it scrolls instead.
const MAX_ROWS: usize = 8;

/// The pause between two writes that must not arrive as one read.
///
/// An agent's input layer tells typing from pasting by how bytes arrive, and a
/// CR landing in the same read as the text before it is taken as part of that
/// text — a newline in the message rather than the key that sends it. Long
/// enough to put the two in separate reads on a loaded machine, short enough
/// to be inside the time a key press takes to feel instant.
const SETTLE: Duration = Duration::from_millis(50);

/// Let the native TUI accept pasted attachments before submitting the message.
const SETTLE_AFTER_PASTE_SLOW: Duration = Duration::from_millis(300);

/// How long the agent's input area has to stay gone before the box steps
/// aside. An agent redrawing its screen can pass through a frame without the
/// input on it; a picker or a prompt stays.
pub(super) const STEP_ASIDE_AFTER: Duration = Duration::from_millis(250);

/// Rows the `/` and `@` menu shows at once.
const MENU_ROWS: usize = 8;

/// A file walk or command scan this recent answers the next menu as it is.
const SOURCES_FRESH_FOR: Duration = Duration::from_secs(30);

/// How far up from the bottom of the screen an agent's input area can start.
/// Past this the thing found is transcript, not the input.
const INPUT_AREA_MAX_ROWS: usize = 40;

/// The design's type: 13.5px text in the box. gpui-component sizes an input's
/// text at 7/8 of a custom size, and pads it 8px across and 2px down.
const TEXT_PX: f32 = 13.5;
const INPUT_PAD_X: f32 = 8.;
const INPUT_PAD_Y: f32 = 2.;
/// How far the box stands in from the pane's sides and bottom.
const BOX_INSET: f32 = 16.;

/// The key the toolbar sends: Shift+Tab cycles the permission mode in every
/// agent that has one.
const BACK_TAB: &[u8] = b"\x1b[Z";

// ---------------------------------------------------------------------------
// Sending

/// One write of a submission, and how long to wait before making it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Step {
    pub delay: Duration,
    pub bytes: Vec<u8>,
}

pub(super) fn submit_plan(agent: CLIAgent, text: &str, bracketed: bool) -> Vec<Step> {
    let clean: String = text
        .replace("\r\n", "\n")
        .chars()
        .filter(|&c| c != '\x1b')
        .map(|c| if c == '\r' { '\n' } else { c })
        .collect();
    let mut body = clean.trim_end();
    let mut steps = Vec::new();
    let mut delay = Duration::ZERO;
    if agent == CLIAgent::OpenCode
        && let Some(rest) = body.strip_prefix('!')
        && !rest.is_empty()
    {
        steps.push(Step {
            delay,
            bytes: b"!".to_vec(),
        });
        body = rest;
        delay = SETTLE;
    }
    let spaced;
    if agent == CLIAgent::OhMyPi && body.contains('@') && !body.starts_with('/') {
        spaced = format!("{body} ");
        body = &spaced;
    }
    if !body.is_empty() {
        let pasted = bracketed
            && (matches!(agent, CLIAgent::OpenCode | CLIAgent::Kimi) || !types_cleanly(body));
        let bytes = if pasted {
            lyrel_core::core::paste::bracket(body.as_bytes())
        } else {
            body.as_bytes().to_vec()
        };
        steps.push(Step { delay, bytes });
    }
    let delay = if steps.is_empty() {
        Duration::ZERO
    } else {
        SETTLE
    };
    steps.push(Step {
        delay,
        bytes: b"\r".to_vec(),
    });
    steps
}

pub(super) fn compose_message(
    agent: CLIAgent,
    text: &str,
    attached: &[String],
    shell: Option<&str>,
) -> String {
    let text = text.trim_end();
    if attached.is_empty() {
        return text.to_string();
    }
    let words = match agent {
        // Oh My Pi reads a mentioned file — an image included — into the
        // turn itself; a path is just words to it.
        CLIAgent::OhMyPi => attached
            .iter()
            .map(|p| mention(Some(agent), p, shell))
            .collect::<Vec<_>>()
            .join(" "),
        _ => pasted_paths_text(attached, shell),
    };
    let words = words.trim_end();
    match text.is_empty() {
        true => words.to_string(),
        false => format!("{text} {words}"),
    }
}

fn attaches_pasted_paths(agent: CLIAgent) -> bool {
    agent == CLIAgent::OpenCode
}

/// The writes that send `text` with `attached` — [`compose_message`] handed
/// to [`submit_plan`], except for an agent that takes a file only from a
/// paste of its own ([`attaches_pasted_paths`]). That one gets the text, then
/// each path as a paste by itself after a typed space, and Enter once the
/// last of them has had time to land.
pub(super) fn submit_message(
    agent: CLIAgent,
    text: &str,
    attached: &[String],
    shell: Option<&str>,
    bracketed: bool,
) -> Vec<Step> {
    if attached.is_empty() || !bracketed || !attaches_pasted_paths(agent) {
        return submit_plan(
            agent,
            &compose_message(agent, text, attached, shell),
            bracketed,
        );
    }
    let mut steps = submit_plan(agent, text, bracketed);
    let enter = steps.pop();
    for path in attached {
        if !steps.is_empty() {
            steps.push(Step {
                delay: SETTLE,
                bytes: b" ".to_vec(),
            });
        }
        let delay = match steps.is_empty() {
            true => Duration::ZERO,
            false => SETTLE,
        };
        steps.push(Step {
            delay,
            bytes: lyrel_core::core::paste::bracket(path.as_bytes()),
        });
    }
    steps.extend(enter.map(|enter| Step {
        delay: SETTLE_AFTER_PASTE_SLOW,
        ..enter
    }));
    steps
}

/// How long OpenCode's `@` list takes to search the project for a query.
const OPENCODE_LIST_WAIT: Duration = Duration::from_millis(700);

/// One stretch of a message cut at its mentions — see [`mention_pieces`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Piece<'a> {
    Text(&'a str),
    /// The path of an `@` mention, without the `@`.
    Mention(&'a str),
}

/// `text` cut at its `@` mentions of paths that `exists` — an `@` at the
/// start of a word, followed by the longest run of up to a few words that
/// names one, a closing `.`, `,` and the like left out. An `@` that names
/// nothing is left in the text.
pub(super) fn mention_pieces<'a>(text: &'a str, exists: impl Fn(&str) -> bool) -> Vec<Piece<'a>> {
    const MAX_WORDS: usize = 4;
    let mut pieces = Vec::new();
    let mut from = 0;
    let mut at = 0;
    while let Some(found) = text[at..].find('@') {
        let sign = at + found;
        at = sign + 1;
        let starts_word = text[..sign]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        if !starts_word {
            continue;
        }
        let rest = &text[sign + 1..];
        // Where each of the next few words ends.
        let mut ends = Vec::new();
        let mut in_word = false;
        for (i, c) in rest.char_indices() {
            match (c.is_whitespace(), in_word) {
                (true, true) => {
                    ends.push(i);
                    in_word = false;
                    if ends.len() == MAX_WORDS {
                        break;
                    }
                }
                (false, false) => in_word = true,
                _ => {}
            }
        }
        if in_word && ends.len() < MAX_WORDS {
            ends.push(rest.len());
        }
        let path = ends.iter().rev().find_map(|&end| {
            let path = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '"', '\'']);
            (!path.is_empty() && exists(path)).then_some(path)
        });
        if let Some(path) = path {
            if from < sign {
                pieces.push(Piece::Text(&text[from..sign]));
            }
            pieces.push(Piece::Mention(path));
            from = sign + 1 + path.len();
            at = from;
        }
    }
    if from < text.len() {
        pieces.push(Piece::Text(&text[from..]));
    }
    pieces
}

/// The writes that send OpenCode a message whose `pieces` mention files.
///
/// OpenCode attaches a file to the message only when it was picked from its
/// own `@` list; a mention typed or pasted is words to it, with or without a
/// space in the path, and the model has to go looking for the file. So each
/// mention goes the way a pick does: a typed `@` opens the list, the path —
/// its spaces left out, as the list would end at one — goes in as the query,
/// and Enter, once the list has had time to search, takes its top match.
/// A space after it closes a list that found nothing, which would take the
/// Enter that sends.
///
/// The `attached` paths follow, each a paste of its own as
/// [`submit_message`] hands them over.
///
/// `None` for a message without mentions, a command or a shell line, which
/// go the way [`submit_message`] sends them.
pub(super) fn opencode_mention_plan(
    pieces: &[Piece],
    attached: &[String],
    bracketed: bool,
) -> Option<Vec<Step>> {
    let first = match pieces.first()? {
        Piece::Text(t) => t.trim_start(),
        Piece::Mention(_) => "",
    };
    if !bracketed
        || first.starts_with(['!', '/'])
        || !pieces.iter().any(|p| matches!(p, Piece::Mention(_)))
    {
        return None;
    }
    let mut steps: Vec<Step> = Vec::new();
    let step = |steps: &mut Vec<Step>, delay: Duration, bytes: Vec<u8>| {
        let delay = match steps.is_empty() {
            true => Duration::ZERO,
            false => delay,
        };
        steps.push(Step { delay, bytes });
    };
    for (i, piece) in pieces.iter().enumerate() {
        match piece {
            Piece::Text(text) => {
                let text = match i + 1 == pieces.len() {
                    true => text.trim_end(),
                    false => text,
                };
                // A pick ends in a space of its own.
                let text = match i > 0 {
                    true => text.strip_prefix(' ').unwrap_or(text),
                    false => text,
                };
                if !text.is_empty() {
                    step(
                        &mut steps,
                        SETTLE,
                        lyrel_core::core::paste::bracket(text.as_bytes()),
                    );
                }
            }
            Piece::Mention(path) => {
                let query: String = path.chars().filter(|c| !c.is_whitespace()).collect();
                step(&mut steps, SETTLE, b"@".to_vec());
                step(
                    &mut steps,
                    SETTLE,
                    lyrel_core::core::paste::bracket(query.as_bytes()),
                );
                step(&mut steps, OPENCODE_LIST_WAIT, b"\r".to_vec());
                // What follows closes a list that found nothing when it
                // has a space in it; otherwise a space of its own does.
                let spaced = match pieces.get(i + 1) {
                    Some(Piece::Text(next)) => next
                        .strip_prefix(' ')
                        .unwrap_or(next)
                        .trim_end()
                        .contains(char::is_whitespace),
                    _ => false,
                };
                if !spaced {
                    step(&mut steps, SETTLE, b" ".to_vec());
                }
            }
        }
    }
    for path in attached {
        step(&mut steps, SETTLE, b" ".to_vec());
        step(
            &mut steps,
            SETTLE,
            lyrel_core::core::paste::bracket(path.as_bytes()),
        );
    }
    step(&mut steps, SETTLE_AFTER_PASTE_SLOW, b"\r".to_vec());
    Some(steps)
}

// ---------------------------------------------------------------------------
// Finding the agent's input area

/// Where an agent's input area starts on the screen, and what its status rows
/// say about the permission mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InputArea {
    /// Screen row of the area's first line.
    pub top: usize,
}

fn covers(agent: CLIAgent) -> bool {
    matches!(
        agent,
        CLIAgent::Pi | CLIAgent::OhMyPi | CLIAgent::Kimi | CLIAgent::OpenCode
    )
}

fn makes_room(agent: CLIAgent) -> bool {
    agent == CLIAgent::OhMyPi
}

/// Whole rows of `line` it takes to make up what `area` rows of `line`, with
/// `below` under them, fall short of `need` — none when they don't.
pub(super) fn rows_short(need: Pixels, line: Pixels, area: usize, below: Pixels) -> usize {
    let line = line.as_f32();
    if line <= 0. {
        return 0;
    }
    let short = need.as_f32() - line * area as f32 - below.as_f32();
    match short > 0. {
        true => (short / line).ceil() as usize,
        false => 0,
    }
}

pub(super) fn input_area(agent: CLIAgent, rows: &[String], width: usize) -> Option<InputArea> {
    let floor = rows.len().saturating_sub(INPUT_AREA_MAX_ROWS);
    match agent {
        CLIAgent::Pi => pi_input_top(rows, floor, width).map(|top| InputArea { top }),
        CLIAgent::OhMyPi => omp_input_top(rows, floor, width).map(|top| InputArea { top }),
        CLIAgent::Kimi => kimi_input_top(rows, floor).map(|top| InputArea { top }),
        CLIAgent::OpenCode => opencode_input_top(rows, floor).map(|top| InputArea { top }),
        _ => None,
    }
}

/// How many rows Kimi Code draws under its input's frame at most: its model
/// and directory line and the context gauge, which wrap in a narrow pane.
const KIMI_FOOTER_MAX_ROWS: usize = 4;

/// Where Kimi Code's input starts: the top edge of the rounded frame whose
/// first line inside opens with `>`, with its model line and context gauge
/// under it.
///
/// Its `/` and `@` lists open under the frame, between it and the model
/// line; a tool approval takes the frame's place between two rules; the
/// welcome banner is a frame of its own, far up with the transcript under
/// it. The box steps aside for the lists and approvals.
fn kimi_input_top(rows: &[String], floor: usize) -> Option<usize> {
    let edge = |r: &str, open: char, close: char| {
        let t = r.trim();
        t.starts_with(open) && t.ends_with(close) && t.chars().count() > 2
    };
    let bottom = (floor..rows.len())
        .rev()
        .find(|&i| edge(&rows[i], '╰', '╯'))?;
    let footer = rows[bottom + 1..]
        .iter()
        .filter(|r| !r.trim().is_empty())
        .count();
    let top = (floor..bottom)
        .rev()
        .find(|&i| !rows[i].trim_start().starts_with('│'))?;
    let prompt = rows.get(top + 1).is_some_and(|r| {
        r.trim_start()
            .trim_start_matches('│')
            .trim_start()
            .starts_with('>')
    });
    (edge(&rows[top], '╭', '╮') && top + 1 < bottom && prompt && footer <= KIMI_FOOTER_MAX_ROWS)
        .then_some(top)
}

/// How many rows OpenCode draws under its input at most: its key hints, and
/// on its home screen the directory at the bottom of the screen.
const OPENCODE_FOOTER_MAX_ROWS: usize = 3;

/// Where OpenCode's input starts: the top of the block it draws on a `┃`
/// bar, the agent and model on its last line, closed by a `╹▀▀▀` edge with
/// the key hints under it.
///
/// Its `/` and `@` lists open on the same bar right over the input, each row
/// closed by a `┃` at its right; a permission request takes the input's
/// place on a bar with no edge under it. The sent messages in the
/// transcript are on a bar too, without the edge. The box steps aside for
/// the lists and requests.
fn opencode_input_top(rows: &[String], floor: usize) -> Option<usize> {
    let bar = |r: &str| r.trim_start().starts_with('┃');
    let edge = (floor..rows.len()).rev().find(|&i| {
        let t = rows[i].trim();
        t.starts_with('╹') && t.chars().skip(1).all(|c| c == '▀') && t.chars().count() > 2
    })?;
    let footer = rows[edge + 1..]
        .iter()
        .filter(|r| !r.trim().is_empty())
        .count();
    let mut top = edge;
    while top > floor && bar(&rows[top - 1]) {
        top -= 1;
    }
    let list = rows[top..edge].iter().any(|r| {
        let t = r.trim();
        t.chars().count() > 1 && t.ends_with('┃')
    });
    (top + 2 <= edge && footer <= OPENCODE_FOOTER_MAX_ROWS && !list).then_some(top)
}

fn interrupt_keys(_agent: Option<CLIAgent>) -> Vec<Step> {
    vec![Step {
        delay: Duration::ZERO,
        bytes: b"\x1b".to_vec(),
    }]
}

fn clear_input(agent: CLIAgent, lines: usize) -> Option<Vec<Step>> {
    if lines == 0 {
        return None;
    }
    let keys = match agent {
        CLIAgent::Kimi | CLIAgent::OpenCode => b"\x15\x7f".repeat(lines),
        _ => return None,
    };
    Some(vec![Step {
        delay: Duration::ZERO,
        bytes: keys,
    }])
}

fn held_lines(agent: CLIAgent, rows: &[String]) -> usize {
    let floor = rows.len().saturating_sub(INPUT_AREA_MAX_ROWS);
    // Lines `first..` hold text; how many, down to the last that does.
    let count = |lines: Vec<&str>| {
        lines
            .iter()
            .rposition(|l| !l.trim().is_empty())
            .map_or(0, |last| last + 1)
    };
    match agent {
        CLIAgent::Kimi => kimi_input_top(rows, floor).map_or(0, |top| {
            count(
                rows[top + 1..]
                    .iter()
                    .take_while(|r| r.trim_start().starts_with('│'))
                    .enumerate()
                    .map(|(i, r)| {
                        let t = r.trim().trim_matches('│').trim();
                        match i {
                            0 => t.trim_start_matches('>').trim(),
                            _ => t,
                        }
                    })
                    .collect(),
            )
        }),
        CLIAgent::OpenCode => opencode_input_top(rows, floor).map_or(0, |top| {
            let block: Vec<&str> = rows[top..]
                .iter()
                .take_while(|r| r.trim_start().starts_with('┃'))
                .map(|r| r.trim().trim_start_matches('┃').trim())
                .collect();
            // The last line names the agent and model.
            let lines = block[..block.len().saturating_sub(1)]
                .iter()
                .map(|l| match l.starts_with("Ask anything...") {
                    true => "",
                    false => l,
                })
                .collect();
            count(lines)
        }),
        _ => 0,
    }
}

/// Where Oh My Pi's input starts, in the composer shapes that keep it findable.
///
/// - **The status band**, its default, and **the rounded box**: the status
///   line — model, directory, branch, context gauge — on the row above the
///   input, whose first line opens with `╰─ `. Its own lists open below that
///   line. A dialog's frame closes with `╰──…╯` instead, and a tool approval
///   or a picker takes the input's place, so neither reads as the input.
/// - **The Pi and Claude Code shapes** rule the input off the way Pi does —
///   see [`pi_input_top`].
///
/// Its borderless, field and rail shapes leave no frame to find, and the box
/// stays aside for them.
fn omp_input_top(rows: &[String], floor: usize, width: usize) -> Option<usize> {
    let line = (floor + 1..rows.len()).rev().find(|&i| {
        rows[i]
            .trim_start()
            .strip_prefix("╰─")
            .is_some_and(|rest| !rest.starts_with('─'))
    });
    match line {
        Some(line) => (!rows[line - 1].trim().is_empty()).then_some(line - 1),
        None => pi_input_top(rows, floor, width),
    }
}

/// How many rows Pi draws under its input at most: the directory and usage
/// rows, an extension's status, and its own completion list, which opens
/// there.
const PI_FOOTER_MAX_ROWS: usize = 10;

/// Where Pi's input starts: its upper rule.
///
/// Pi draws its editor between two full-width rules, the upper one carrying
/// the turn's spinner (`── ⠹ Working ──`) while it works, with the
/// directory and usage rows under the lower one. Its selectors — `/model`,
/// `/resume`, `/tree`, `/login` — take the editor's place between the same
/// rules, but open on a blank row and run on for several, and `/settings`
/// opens on its `>` search line; the editor is one row when empty, and opens
/// on its text otherwise. Only the lowest pair of rules counts: anything
/// ruled higher up is the transcript's.
fn pi_input_top(rows: &[String], floor: usize, width: usize) -> Option<usize> {
    let rule = |row: &str| {
        let t = row.trim();
        t.starts_with("──")
            && t.ends_with('─')
            && t.chars().filter(|&c| c == '─').count() >= width * 3 / 5
    };
    let rules: Vec<usize> = (floor..rows.len()).filter(|&i| rule(&rows[i])).collect();
    let (&lower, higher) = rules.split_last()?;
    let &upper = higher.last()?;
    let footer = rows[lower + 1..]
        .iter()
        .filter(|r| !r.trim().is_empty())
        .count();
    let inside = &rows[upper + 1..lower];
    let opens_on_text = |row: &String| {
        let t = row.trim_end();
        !t.trim().is_empty() && t != ">" && !t.starts_with("> ")
    };
    let editor = inside.len() == 1 || inside.first().is_some_and(opens_on_text);
    (footer <= PI_FOOTER_MAX_ROWS && editor).then_some(upper)
}

/// The background the agent paints the screen row `line` in: the colour most
/// of its cells have — an inverse cell's being its foreground — or `None` for
/// the terminal's own background.
fn row_backdrop<T: EventListener>(
    term: &Crosswords<T>,
    line: usize,
    palette: &[ColorRgb; 256],
) -> Option<ColorRgb> {
    let grid = &term.grid;
    if line >= grid.screen_lines() {
        return None;
    }
    let row = &grid[Line(line as i32)];
    let mut tally: Vec<(Option<ColorRgb>, usize)> = Vec::new();
    for c in 0..grid.columns() {
        let style = grid.style_of(&row[Column(c)]);
        let color = match style.flags.contains(StyleFlags::INVERSE) {
            true => style.fg,
            false => style.bg,
        };
        let rgb = match color {
            AnsiColor::Spec(rgb) => Some(rgb),
            AnsiColor::Indexed(i) => Some(palette[i as usize]),
            AnsiColor::Named(named) => palette.get(named as usize).copied(),
        };
        match tally.iter_mut().find(|(seen, _)| *seen == rgb) {
            Some((_, n)) => *n += 1,
            None => tally.push((rgb, 1)),
        }
    }
    tally
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .and_then(|(rgb, _)| rgb)
}

fn rgb_to_hsla(c: ColorRgb) -> gpui_kit::Hsla {
    gpui_kit::Rgba {
        r: c.r as f32 / 255.,
        g: c.g as f32 / 255.,
        b: c.b as f32 / 255.,
        a: 1.,
    }
    .into()
}

/// The screen as text, one string per line, wide characters' spacer cells
/// left out.
fn screen_rows<T: EventListener>(term: &Crosswords<T>) -> Vec<String> {
    let grid = &term.grid;
    (0..grid.screen_lines())
        .map(|l| {
            let line = Line(l as i32);
            let row = &grid[line];
            let mut text = String::with_capacity(grid.columns());
            for c in 0..grid.columns() {
                if row[Column(c)].wide() != Wide::Spacer {
                    text.extend(
                        grid.cell_text(Pos::new(line, Column(c)))
                            .map(|c| if c == '\0' { ' ' } else { c }),
                    );
                }
            }
            text
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Menus

/// A `/` or `@` being typed at the caret: which one, where it starts, and what
/// follows it so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Trigger {
    pub sigil: char,
    /// Byte offset of the sigil.
    pub start: usize,
    pub query: String,
}

/// The word the caret is at the end of, when it opens with `/` or `@`.
///
/// A `/` counts only as the first thing in the message: that is the only
/// place an agent reads a slash command, and anywhere else it is a path or a
/// fraction. An `@` counts at the start of any word.
pub(super) fn trigger_at(text: &str, cursor: usize) -> Option<Trigger> {
    let before = text.get(..cursor)?;
    let start = before
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let word = &before[start..];
    let sigil = word.chars().next()?;
    let valid = match sigil {
        '/' => before[..start].trim().is_empty(),
        '@' => true,
        _ => false,
    };
    valid.then(|| Trigger {
        sigil,
        start,
        query: word[1..].to_string(),
    })
}

/// One row of the `/` or `@` menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MenuItem {
    pub label: String,
    pub detail: String,
    /// What replaces the trigger word when the row is picked.
    pub insert: String,
    pub is_file: bool,
}

/// The agent's own commands, the ones it ships with. Descriptions stay in the
/// agent's language — they name what its UI will do, in the words it uses.
fn builtin_commands(agent: CLIAgent) -> &'static [(&'static str, &'static str)] {
    match agent {
        CLIAgent::OpenCode => &[
            ("/new", "New session"),
            ("/models", "Switch model"),
            ("/agents", "Switch agent"),
            ("/sessions", "Switch session"),
            ("/compact", "Compact session"),
            ("/undo", "Undo previous message"),
            ("/review", "Review changes"),
            ("/init", "Guided AGENTS.md setup"),
            ("/share", "Share session"),
            ("/status", "View status"),
            ("/mcps", "Toggle MCPs"),
            ("/help", "Help"),
        ],
        CLIAgent::Kimi => &[
            ("/new", "Start a fresh session"),
            ("/compact", "Compact the conversation context"),
            ("/model", "Switch LLM model"),
            ("/effort", "Switch thinking effort"),
            ("/sessions", "Browse and resume sessions"),
            ("/undo", "Withdraw the last prompt"),
            ("/init", "Analyze the codebase and generate AGENTS.md"),
            ("/plan", "Toggle plan mode"),
            ("/permission", "Select permission mode"),
            ("/usage", "Show session tokens and context window"),
            ("/status", "Show session and runtime status"),
            ("/mcp", "Show MCP server status"),
            ("/help", "Show available commands and shortcuts"),
        ],
        CLIAgent::Pi => &[
            ("/model", "Select model"),
            ("/thinking", "Set thinking level"),
            ("/compact", "Compact the session context"),
            ("/new", "Start a new session"),
            ("/resume", "Resume a different session"),
            ("/tree", "Navigate the session tree"),
            ("/fork", "Fork from a previous message"),
            ("/session", "Show session info and stats"),
            ("/copy", "Copy the last agent message"),
            ("/export", "Export the session"),
            ("/settings", "Open settings"),
            ("/reload", "Reload extensions, skills and prompts"),
            ("/hotkeys", "Show keyboard shortcuts"),
            ("/quit", "Quit Pi"),
        ],
        CLIAgent::OhMyPi => &[
            ("/model", "Switch model"),
            ("/plan", "Toggle plan mode"),
            ("/compact", "Compact the session context"),
            ("/new", "Start a new session"),
            ("/resume", "Resume a different session"),
            ("/retry", "Retry the last failed turn"),
            ("/tree", "Navigate the session tree"),
            ("/fork", "Fork from a previous message"),
            ("/context", "Show context usage"),
            ("/usage", "Show provider usage and limits"),
            ("/todo", "View or change the todo list"),
            ("/copy", "Copy text from the conversation"),
            ("/settings", "Open settings"),
            ("/exit", "Exit Oh My Pi"),
        ],
        // Amp and Crush have no typed commands: `/` on an empty input opens
        // their command palette, which a message cannot drive.
        _ => &[],
    }
}

/// The `/` menu for `query`: names starting with it first, in the order the
/// agent lists them, then names that merely contain it.
pub(super) fn command_items(commands: &[(String, String)], query: &str) -> Vec<MenuItem> {
    let q = query.to_lowercase();
    let starts = commands
        .iter()
        .filter(|(n, _)| n[1..].to_lowercase().starts_with(&q));
    let contains = commands.iter().filter(|(n, _)| {
        let n = n[1..].to_lowercase();
        !n.starts_with(&q) && n.contains(&q)
    });
    starts
        .chain(contains)
        .take(MENU_ROWS)
        .map(|(name, detail)| MenuItem {
            label: name.clone(),
            detail: detail.clone(),
            insert: name.clone(),
            is_file: false,
        })
        .collect()
}

pub(super) fn mention(agent: Option<CLIAgent>, path: &str, _shell: Option<&str>) -> String {
    if matches!(
        agent,
        Some(CLIAgent::Kimi | CLIAgent::Pi | CLIAgent::OhMyPi)
    ) && path.chars().any(char::is_whitespace)
    {
        format!("@\"{path}\"")
    } else {
        format!("@{path}")
    }
}

/// The `@` menu for `query`. The mention is spelled relative to `cwd` — which
/// is what the agent resolves it against — and in full for a file outside it.
fn file_items(
    index: &FileIndex,
    query: &str,
    cwd: Option<&Path>,
    agent: Option<CLIAgent>,
    shell: Option<&str>,
) -> Vec<MenuItem> {
    let spell = |path: &Path| -> String {
        cwd.and_then(|c| path.strip_prefix(c).ok())
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    };
    let item = |f: &IndexedFile| MenuItem {
        label: f.name().to_string(),
        detail: f.dir().to_string(),
        insert: mention(agent, &spell(&f.path), shell),
        is_file: true,
    };
    match query.is_empty() {
        // The walk is breadth-first, so its head is the top of the project.
        true => index.files.iter().take(MENU_ROWS).map(item).collect(),
        false => rank(index, query, MENU_ROWS)
            .into_iter()
            .map(|(_, f)| item(f))
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// State

/// What a pane's composer holds that outlives the view drawing it.
///
/// A view is rebuilt over the same daemon pane whenever its workspace is
/// switched out and back, and a half-written prompt must not be the price of
/// looking at another workspace. Kept by pane, for the life of the app.
#[derive(Default)]
struct ComposerMemory(HashMap<(HostId, u64), Remembered>);

impl gpui_kit::Global for ComposerMemory {}

#[derive(Default, Clone)]
struct Remembered {
    draft: String,
    attached: Vec<String>,
    open: bool,
}

enum Files {
    Unwalked,
    Walking,
    Ready(Arc<FileIndex>, Instant),
    Failed(Instant),
}

/// How the box is on screen this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Presence {
    /// Not wanted, or no agent to talk to.
    Hidden,
    /// Under the grid, taking its rows.
    Docked,
    /// Over the agent's input area, which starts this many rows up from the
    /// bottom of the screen.
    Covering(usize),
    /// Wanted, but the agent is showing something else where its input goes —
    /// the box waits for it to come back.
    SteppedAside,
}

pub(super) struct Composer {
    pub(super) input: Entity<TextareaState>,
    /// Whether the user wants the box on this pane.
    open: bool,
    presence: Presence,
    /// The input area as last found, and since when it has been missing.
    area: Option<InputArea>,
    /// How many rows up from the bottom of the live screen that area starts,
    /// however far the view is scrolled back — what the grid's reserved rows
    /// are worked out from, so scrolling never resizes the pane.
    area_rows: usize,
    missing_since: Option<Instant>,
    /// The box had the keyboard when it stepped aside, and gets it back when
    /// it returns.
    refocus: bool,
    /// Files to send with the message, spelled the way the pane's host reads
    /// them — uploaded already, for a remote pane.
    attached: Vec<String>,
    /// Text typed at the grid while the box covers the input, and whether the
    /// box should take the keyboard with it. Taken in at the next draw, the
    /// first place with a window to edit and focus in.
    typed: String,
    grab: bool,
    /// Writes waiting their turn. Submissions queue rather than interleave, so
    /// a second message sent inside the first one's settle time cannot land
    /// its text between the first one's text and its Enter.
    queue: VecDeque<Step>,
    pumping: bool,
    /// Whose name the placeholder carries.
    named: Option<CLIAgent>,
    /// The row of the `/` or `@` menu the keyboard is on.
    highlighted: usize,
    /// The text as it stood when Esc closed the menu. The menu stays shut
    /// until the text moves on from there.
    dismissed: Option<String>,
    files: Files,
    commands: Option<(Vec<(String, String)>, Instant)>,
    /// The box's height as last drawn empty — nothing typed, nothing
    /// attached — which is what it needs of the pane at the least.
    frame_height: Rc<Cell<Option<Pixels>>>,
    /// The colour the agent paints the screen in around its input, when it
    /// paints one of its own — the layer covering the input takes it, so it
    /// is not a strip of another colour around the box — and how wide the
    /// grid's columns paint it.
    backdrop: Option<(ColorRgb, Pixels)>,
    _subs: Vec<Subscription>,
}

impl TerminalView {
    fn composer_key(&self) -> (HostId, u64) {
        (self.host_id(), self.pane_id)
    }

    fn remember_composer(&self, cx: &mut Context<Self>) {
        let Some(c) = self.composer.as_ref() else {
            return;
        };
        let entry = Remembered {
            draft: c.input.read(cx).value().to_string(),
            attached: c.attached.clone(),
            open: c.open,
        };
        let key = self.composer_key();
        let memory = cx.default_global::<ComposerMemory>();
        match entry.draft.is_empty() && entry.attached.is_empty() && !entry.open {
            true => memory.0.remove(&key),
            false => memory.0.insert(key, entry),
        };
    }

    fn ensure_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.composer.is_some() {
            return;
        }
        let remembered = cx
            .try_global::<ComposerMemory>()
            .and_then(|m| m.0.get(&self.composer_key()))
            .cloned()
            .unwrap_or_default();
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, MAX_ROWS)
                .submit_on_enter(true)
                .default_value(remembered.draft)
        });
        let subs = vec![cx.subscribe_in(&input, window, Self::on_composer_event)];
        self.composer = Some(Composer {
            input,
            open: remembered.open,
            presence: Presence::Hidden,
            area: None,
            area_rows: 0,
            missing_since: None,
            refocus: false,
            attached: remembered.attached,
            typed: String::new(),
            grab: false,
            queue: VecDeque::new(),
            pumping: false,
            named: None,
            highlighted: 0,
            dismissed: None,
            files: Files::Unwalked,
            commands: None,
            frame_height: Rc::new(Cell::new(None)),
            backdrop: None,
            _subs: subs,
        });
    }

    /// Rows the grid leaves empty at its bottom so that the box, covering an
    /// input shorter than itself, covers nothing above it: Oh My Pi's input
    /// is a status line and a prompt line, where the box is some five rows
    /// tall, and would otherwise hide the last lines of the reply. The agent
    /// is told the pane is that much shorter and draws its input higher.
    ///
    /// Only for an agent whose input is that short ([`makes_room`]): the
    /// others' inputs, with their rules and status rows, about hold the box,
    /// and resizing their pane each time the box comes and goes would have
    /// them redraw for a row of padding.
    pub(super) fn composer_reserved_rows(&self) -> usize {
        let Some(c) = self
            .composer
            .as_ref()
            .filter(|c| matches!(c.presence, Presence::Covering(_)))
        else {
            return 0;
        };
        if !self.agent().is_some_and(makes_room) {
            return 0;
        }
        let Some(frame) = c.frame_height.get() else {
            return 0;
        };
        // The input's rows on the live screen, not the ones left in view: a
        // view scrolled back shows fewer of them, and taking that for an
        // input shrinking under the box would resize the pane per line
        // scrolled.
        rows_short(
            frame + px(BOX_INSET),
            self.line_height,
            c.area_rows,
            self.grid_slack + px(GRID_PAD_Y),
        )
    }

    pub(super) fn presence(&self) -> Presence {
        self.composer
            .as_ref()
            .map_or(Presence::Hidden, |c| c.presence)
    }

    /// Whether the box is on screen.
    pub(super) fn composer_shown(&self) -> bool {
        matches!(self.presence(), Presence::Docked | Presence::Covering(_))
    }

    fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.composer.as_ref() else {
            return;
        };
        let focus = c.input.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        // Said here as well as by the input's own Focus event, which only
        // comes round on the next frame: what is about to land — a drop, a
        // picked file — has to find the box already holding the keyboard.
        self.composer_focused = true;
    }

    /// Open the box and put the caret in it; from inside it, close it; with
    /// it open but the terminal focused, go back into it.
    ///
    /// A pane with no agent in the foreground has nobody to compose for, so
    /// the chord does nothing there.
    pub fn toggle_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent().is_none() {
            return;
        }
        self.ensure_composer(window, cx);
        let Some(c) = self.composer.as_mut() else {
            return;
        };
        let focused = c.input.read(cx).focus_handle(cx).is_focused(window);
        match (c.open, focused) {
            (true, true) => {
                c.open = false;
                c.refocus = false;
                window.focus(&self.focus_handle, cx);
            }
            _ => {
                // Focused here and now. `refocus` is for a box coming back
                // from stepping aside; set here it would outlive this focus
                // and pull the keyboard back from the next click elsewhere.
                c.open = true;
                c.refocus = false;
                self.focus_composer(window, cx);
            }
        }
        self.remember_composer(cx);
        cx.notify();
    }

    /// Esc in the box. Over the agent's input it is the agent's Esc — the one
    /// that interrupts a turn — since the box is standing in for that input;
    /// for an agent whose Esc does not, its own interrupt key
    /// ([`interrupt_keys`]).
    /// Docked, it hands the keyboard back to the terminal and leaves the box
    /// where it is, so the next Esc is the agent's.
    pub(super) fn composer_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.presence() {
            Presence::Covering(_) => {
                let Some(agent) = self.agent() else { return };
                if let Some(c) = self.composer.as_mut() {
                    c.queue.extend(interrupt_keys(Some(agent)));
                }
                self.pump_composer(agent, cx);
            }
            _ => {
                window.focus(&self.focus_handle, cx);
                cx.notify();
            }
        }
    }

    /// Ctrl+C in the box: clear what is written, or — with nothing written —
    /// the agent's Ctrl+C.
    pub(super) fn composer_interrupt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.composer.as_mut() else {
            return;
        };
        let empty = c.input.read(cx).value().is_empty() && c.attached.is_empty();
        if empty {
            self.send_to_pty(b"\x03", cx);
            return;
        }
        c.attached.clear();
        c.input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.remember_composer(cx);
    }

    /// Typing at the grid while the box covers the agent's input: the input
    /// the keys were meant for is under the box, so they go into the box.
    ///
    /// So does text handed to the terminal while the box has the keyboard —
    /// a file tree's "attach to agent", say. The box's own typing never comes
    /// this way, and written to the pty it would land in the agent's input
    /// under the box, unseen, and run into the next message.
    pub(super) fn composer_takes_typing(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if !matches!(self.presence(), Presence::Covering(_)) {
            return false;
        }
        let focused = self.composer_focused;
        let Some(c) = self.composer.as_mut() else {
            return false;
        };
        c.typed.push_str(text);
        c.grab |= !focused;
        cx.notify();
        true
    }

    /// Whether files pasted now become the composer's attachments: it has
    /// the keyboard, or covers the input they would be pasted into.
    pub(super) fn composer_takes_files(&self) -> bool {
        let covering = matches!(self.presence(), Presence::Covering(_));
        self.composer_focused && self.composer_shown() || covering
    }

    /// Files arriving by way of the terminal's paste path — dropped on the
    /// box, a copied file or a screenshot pasted into it, an upload to a
    /// remote pane landing — become attachments while the box has the
    /// keyboard, or covers the input they would otherwise be pasted into
    /// (taking the keyboard with them, as typing there does). Hands them
    /// back otherwise, for the terminal to paste.
    pub(super) fn composer_takes_paths(
        &mut self,
        spelled: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Option<Vec<String>> {
        if !self.composer_takes_files() {
            return Some(spelled);
        }
        let focused = self.composer_focused;
        let c = self.composer.as_mut()?;
        c.grab |= !focused;
        for path in spelled {
            if !c.attached.contains(&path) {
                c.attached.push(path);
            }
        }
        self.remember_composer(cx);
        cx.notify();
        None
    }

    fn detach(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(c) = self.composer.as_mut()
            && index < c.attached.len()
        {
            c.attached.remove(index);
        }
        self.remember_composer(cx);
        cx.notify();
    }

    /// The attach button: pick files on this computer, which then go the way
    /// dropped files do — uploaded first for a remote pane.
    fn pick_attachments(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_composer(window, cx);
        let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update_in(cx, |view, window, cx| {
                    view.focus_composer(window, cx);
                    view.paste_local_paths(paths, cx);
                });
            }
        })
        .detach();
    }

    /// Per-frame upkeep: restore a box the pane had open before this view
    /// existed, find the agent's input area, decide where the box goes, and
    /// move the keyboard with it.
    pub(super) fn sync_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let agent = self.agent();
        if self.composer.is_none()
            && agent.is_some()
            && cx
                .try_global::<ComposerMemory>()
                .is_some_and(|m| m.0.contains_key(&self.composer_key()))
        {
            self.ensure_composer(window, cx);
        }
        // A pane whose box was never asked for, or is shut, has no input area
        // to look for: the scan below reads the whole screen, every frame.
        let Some(open) = self.composer.as_ref().map(|c| c.open) else {
            return;
        };
        let asking = self.agent_is_asking();
        let (found, backdrop, screen_lines, offset) = if !open {
            (None, None, 0, 0)
        } else {
            let mut palette = self.terminal.palette;
            if let Some(active) = cx.try_global::<super::palette::ActivePalette>() {
                palette[..16].copy_from_slice(&active.ansi16);
            }
            let term = self.terminal.term.lock();
            let found = agent
                .filter(|a| covers(*a))
                .and_then(|a| input_area(a, &screen_rows(&term), term.columns()));
            // The row over the input is the screen around it: the input's own
            // rows can be shaded apart from it.
            let backdrop = found
                .as_ref()
                .and_then(|area| row_backdrop(&term, area.top.saturating_sub(1), &palette))
                .map(|rgb| (rgb, self.cell_width * term.columns() as f32));
            (
                found,
                backdrop,
                term.screen_lines(),
                term.grid.display_offset(),
            )
        };
        let Some(c) = self.composer.as_mut() else {
            return;
        };

        // The area, with a grace period before it counts as gone.
        let now = Instant::now();
        let mut recheck = None;
        match found {
            // Shut, nothing was looked for: what was found before is stale.
            _ if !open => {
                c.area = None;
                c.missing_since = None;
            }
            Some(area) => {
                c.area = Some(area);
                c.backdrop = backdrop;
                c.missing_since = None;
            }
            None => {
                let since = *c.missing_since.get_or_insert(now);
                if now.duration_since(since) >= STEP_ASIDE_AFTER {
                    c.area = None;
                } else {
                    recheck = Some(STEP_ASIDE_AFTER - now.duration_since(since));
                }
            }
        }

        c.presence = match agent {
            _ if !c.open => Presence::Hidden,
            None => Presence::Hidden,
            Some(a) if !covers(a) => Presence::Docked,
            Some(_) if asking => Presence::SteppedAside,
            Some(_) => match &c.area {
                Some(area) => {
                    c.area_rows = screen_lines.saturating_sub(area.top);
                    Presence::Covering(c.area_rows.saturating_sub(offset))
                }
                None => Presence::SteppedAside,
            },
        };

        if let Some(agent) = agent
            && c.named != Some(agent)
        {
            c.named = Some(agent);
            let key = match builtin_commands(agent).is_empty() {
                true => L10nKey::ComposerPlaceholderFiles,
                false => L10nKey::ComposerPlaceholder,
            };
            let placeholder = t_fmt(key, &[("agent", agent.display_name())]);
            c.input.update(cx, |state, cx| {
                state.set_placeholder(placeholder, window, cx)
            });
        }

        let shown = matches!(c.presence, Presence::Docked | Presence::Covering(_));
        if !shown && self.composer_focused {
            c.refocus = c.presence == Presence::SteppedAside;
            self.composer_focused = false;
            window.focus(&self.focus_handle, cx);
        } else if shown {
            let take_focus = (c.refocus || c.grab) && !self.composer_focused;
            if take_focus {
                c.refocus = false;
                c.grab = false;
            }
            let typed = std::mem::take(&mut c.typed);
            let input = c.input.clone();
            if !typed.is_empty() {
                input.update(cx, |state, cx| state.insert(typed, window, cx));
            }
            if take_focus {
                self.focus_composer(window, cx);
            }
        }

        if let Some(wait) = recheck {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
    }

    fn on_composer_event(
        &mut self,
        _input: &Entity<TextareaState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                if let Some(c) = self.composer.as_mut() {
                    c.highlighted = 0;
                }
                self.remember_composer(cx);
                self.warm_menu_sources(cx);
            }
            InputEvent::PressEnter { shift: false, .. } => self.submit_composer(window, cx),
            InputEvent::PressEnter { .. } => {}
            InputEvent::Focus => {
                self.composer_focused = true;
                cx.notify();
            }
            InputEvent::Blur => {
                self.composer_focused = false;
                cx.notify();
            }
        }
    }

    fn composer_trigger(&self, cx: &gpui_kit::App) -> Option<Trigger> {
        let c = self.composer.as_ref()?;
        let state = c.input.read(cx);
        let text = state.value();
        if c.dismissed.as_deref() == Some(text.as_ref()) {
            return None;
        }
        trigger_at(&text, state.cursor())
    }

    /// Start what the menu about to open needs: the file walk for `@`, the
    /// custom command list for `/`. Both are kept a while, so typing through
    /// a query does not walk the project once per keystroke.
    fn warm_menu_sources(&mut self, cx: &mut Context<Self>) {
        let Some(trigger) = self.composer_trigger(cx) else {
            return;
        };
        let host_id = self.host_id();
        let local = host_id.is_local();
        let cwd = self.files_cwd();
        let Some(c) = self.composer.as_mut() else {
            return;
        };
        match trigger.sigil {
            '/' => {
                if c.commands
                    .as_ref()
                    .is_some_and(|(_, at)| at.elapsed() < SOURCES_FRESH_FOR)
                {
                    return;
                }
                c.commands = Some((Vec::new(), Instant::now()));
            }
            '@' => {
                let stale = match &c.files {
                    Files::Unwalked => true,
                    Files::Walking => false,
                    Files::Ready(_, at) | Files::Failed(at) => at.elapsed() > SOURCES_FRESH_FOR,
                };
                let Some(cwd) = cwd.filter(|_| stale) else {
                    return;
                };
                let Some(host) = crate::ui::host_registry::HostRegistry::lookup(cx, host_id) else {
                    return;
                };
                if !matches!(c.files, Files::Ready(..)) {
                    c.files = Files::Walking;
                }
                let home = local
                    .then(|| std::env::var_os("HOME").map(PathBuf::from))
                    .flatten();
                HostOps::run(
                    host,
                    cx,
                    move |h| walk(h, &[cwd], home.as_deref()),
                    |view: &mut TerminalView, list, cx| {
                        let Some(c) = view.composer.as_mut() else {
                            return;
                        };
                        c.files = match list {
                            FileList::Ready(index) => Files::Ready(index, Instant::now()),
                            _ => Files::Failed(Instant::now()),
                        };
                        cx.notify();
                    },
                );
            }
            _ => {}
        }
    }

    /// The `/` or `@` menu as it stands at the caret, if one is open.
    fn composer_menu(&self, cx: &gpui_kit::App) -> Option<(Trigger, Vec<MenuItem>)> {
        let trigger = self.composer_trigger(cx)?;
        let c = self.composer.as_ref()?;
        let items = match trigger.sigil {
            '/' => {
                let agent = self.agent()?;
                let mut all: Vec<(String, String)> = builtin_commands(agent)
                    .iter()
                    .map(|(n, d)| (n.to_string(), d.to_string()))
                    .collect();
                if let Some((custom, _)) = &c.commands {
                    for (name, detail) in custom {
                        if !all.iter().any(|(n, _)| n == name) {
                            all.push((name.clone(), detail.clone()));
                        }
                    }
                }
                command_items(&all, &trigger.query)
            }
            _ => match &c.files {
                Files::Ready(index, _) => file_items(
                    index,
                    &trigger.query,
                    self.files_cwd().as_deref(),
                    self.agent(),
                    self.shell_program().as_deref(),
                ),
                _ => Vec::new(),
            },
        };
        (!items.is_empty()).then_some((trigger, items))
    }

    /// Replace the word being typed with the picked row, and a space after it
    /// so the next word starts clean.
    fn pick_menu_item(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((trigger, items)) = self.composer_menu(cx) else {
            return;
        };
        let Some(item) = items.get(index) else {
            return;
        };
        let Some(c) = self.composer.as_ref() else {
            return;
        };
        let input = c.input.clone();
        input.update(cx, |state, cx| {
            let text = state.value().to_string();
            let cursor = state.cursor().min(text.len());
            let inserted = format!("{} ", item.insert);
            let next = format!("{}{inserted}{}", &text[..trigger.start], &text[cursor..]);
            let caret = trigger.start + inserted.len();
            state.set_value(next, window, cx);
            let position = state.text().offset_to_position(caret);
            state.set_cursor_position(position, window, cx);
        });
    }

    fn step_menu(&mut self, forward: bool, cx: &mut Context<Self>) -> bool {
        let Some((_, items)) = self.composer_menu(cx) else {
            return false;
        };
        let Some(c) = self.composer.as_mut() else {
            return false;
        };
        let n = items.len();
        let at = c.highlighted.min(n - 1);
        c.highlighted = match forward {
            true => (at + 1) % n,
            false => (at + n - 1) % n,
        };
        cx.notify();
        true
    }

    /// The agent is asking a question only its TUI can put — a permission
    /// prompt, a choice. Whatever the box sent would be taken as the answer,
    /// so nothing is sent until the question is gone.
    ///
    /// Only a hook can say so. An agent without them — Amp — is marked
    /// waiting by any desktop notification it raises, "Agent is ready" when
    /// a turn ends included, and stays so; read as a question, that kept the
    /// box aside, with nothing sendable, from the first finished turn on.
    /// Its own dialogs it puts in its input's place, and the box steps aside
    /// for those off the screen.
    fn agent_is_asking(&self) -> bool {
        self.agent_session()
            .is_some_and(|s| s.rich && s.status == AgentStatus::Waiting)
    }

    fn composer_can_send(&self, cx: &gpui_kit::App) -> bool {
        self.composer
            .as_ref()
            .is_some_and(|c| !c.attached.is_empty() || !c.input.read(cx).value().trim().is_empty())
            && !self.agent_is_asking()
    }

    pub(super) fn submit_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.agent() else {
            return;
        };
        if !self.composer_can_send(cx) {
            return;
        }
        let shell = self.shell_program();
        // Where OpenCode's mentions are looked up, to be picked from its list.
        let mentions_from = match agent == CLIAgent::OpenCode && self.host_id().is_local() {
            true => self.files_cwd(),
            false => None,
        };
        let Some(c) = self.composer.as_mut() else {
            return;
        };
        let bracketed = self
            .terminal
            .term
            .lock()
            .mode()
            .contains(Mode::BRACKETED_PASTE);
        let held = match c.presence {
            Presence::Covering(_) => {
                let term = self.terminal.term.lock();
                held_lines(agent, &screen_rows(&term))
            }
            _ => 0,
        };
        let typed = c.input.read(cx).value().to_string();
        let message = &typed;
        let picked = mentions_from.as_deref().and_then(|cwd| {
            let exists = |p: &str| Path::new(p).is_relative() && cwd.join(p).exists();
            opencode_mention_plan(&mention_pieces(message, exists), &c.attached, bracketed)
        });
        let mut steps = match picked {
            Some(steps) => steps,
            None => submit_message(agent, message, &c.attached, shell.as_deref(), bracketed),
        };
        let mut before = Vec::new();
        // Text left in the agent's own input under the box would run into the
        // message: clear it first.
        if let Some(clear) = clear_input(agent, held) {
            before.extend(clear);
        }
        if !before.is_empty()
            && let Some(first) = steps.first_mut()
        {
            first.delay = first.delay.max(SETTLE);
            steps.splice(0..0, before);
        }
        c.attached.clear();
        c.dismissed = None;
        c.queue.extend(steps);
        c.input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.remember_composer(cx);
        self.pump_composer(agent, cx);
    }

    /// Write queued steps one at a time, each after its own delay.
    ///
    /// Every write after a wait checks that `agent` is still what the pane is
    /// running: an agent that quit inside the wait has handed the pty back to
    /// the shell, and the rest of the message — its Enter above all — would
    /// run there as a command.
    fn pump_composer(&mut self, agent: CLIAgent, cx: &mut Context<Self>) {
        let Some(c) = self.composer.as_mut() else {
            return;
        };
        if c.pumping {
            return;
        }
        c.pumping = true;
        cx.spawn(async move |this, cx| {
            loop {
                // Popping the last step and standing the pump down are one
                // update, so a submission can never find it still "pumping"
                // after it has stopped looking at the queue.
                let next = this
                    .update(cx, |view, _| {
                        let c = view.composer.as_mut()?;
                        let step = c.queue.pop_front();
                        c.pumping = step.is_some();
                        step
                    })
                    .ok()
                    .flatten();
                let Some(step) = next else { return };
                if !step.delay.is_zero() {
                    cx.background_executor().timer(step.delay).await;
                }
                let sent = this.update(cx, |view, cx| {
                    if view.agent() != Some(agent) {
                        if let Some(c) = view.composer.as_mut() {
                            c.queue.clear();
                        }
                        return;
                    }
                    view.send_to_pty(&step.bytes, cx);
                });
                if sent.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    // -----------------------------------------------------------------------
    // Drawing

    /// The box, and whether it is docked (laid out under the grid) rather
    /// than laid over it.
    pub(super) fn render_composer(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<(gpui_kit::AnyElement, bool)> {
        let presence = self.presence();
        let covering = match presence {
            Presence::Covering(rows) => Some(rows),
            Presence::Docked => None,
            Presence::Hidden | Presence::SteppedAside => return None,
        };
        let frame = self.render_composer_frame(window, cx)?;
        // Measured while empty: the least the box needs of the pane, for
        // [`Self::composer_reserved_rows`].
        let measure = self.composer.as_ref().and_then(|c| {
            (covering.is_some() && c.attached.is_empty() && c.input.read(cx).value().is_empty())
                .then(|| c.frame_height.clone())
        });
        let frame = div()
            .w_full()
            .when_some(measure, |d, cell| {
                d.on_children_prepainted(move |bounds, window, _| {
                    let Some(b) = bounds.first() else { return };
                    if cell.get() != Some(b.size.height) {
                        cell.set(Some(b.size.height));
                        window.refresh();
                    }
                })
            })
            .child(frame);
        let reserved = self.composer_reserved_rows();
        let element = match covering {
            // Over the input area, painted in the background around it — the
            // grid's own, or the one the agent paints — so what is under it
            // is gone rather than showing through, and the layer is not seen. At least
            // as tall as the area; taller when the box needs it. The box sits
            // at the bottom of the pane, where a chat's input sits.
            Some(rows) => div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                // Measured up from the pane's bottom edge: its padding, the
                // part of a row the grid's height left over, the rows the
                // grid gave up for the box, then the input's rows.
                .min_h(
                    self.line_height * (rows + reserved) as f32 + self.grid_slack + px(GRID_PAD_Y),
                )
                .flex()
                .flex_col()
                .justify_end()
                .px(px(BOX_INSET))
                .pb(px(BOX_INSET))
                .bg(cx.theme().background)
                // Over the grid's own cells, the colour the agent paints
                // them in; the pane's padding around them keeps the theme's.
                .when_some(
                    self.composer.as_ref().and_then(|c| c.backdrop),
                    |d, (backdrop, width)| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .left(px(GRID_PAD_X))
                                .w(width)
                                .bottom(
                                    self.line_height * reserved as f32
                                        + self.grid_slack
                                        + px(GRID_PAD_Y),
                                )
                                .bg(rgb_to_hsla(backdrop)),
                        )
                    },
                )
                .occlude()
                // Around the box is the terminal as far as a click goes: it
                // takes the keyboard out of the box, as a click on the rows
                // above does. The layer only hides what is under it.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        window.focus(&this.focus_handle, cx);
                    }),
                )
                .child(frame)
                .into_any_element(),
            None => div()
                .flex_none()
                .w_full()
                .pt(px(8.))
                // The pane's own padding is already between it and the edges.
                .px(px(BOX_INSET - GRID_PAD_X))
                .pb(px(BOX_INSET - GRID_PAD_Y))
                .child(frame)
                .into_any_element(),
        };
        Some((element, covering.is_none()))
    }

    fn render_composer_frame(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::AnyElement> {
        let c = self.composer.as_ref()?;
        let agent = self.agent()?;
        let theme = cx.theme();
        let focused = c.input.read(cx).focus_handle(cx).is_focused(window);
        let can_send = self.composer_can_send(cx);
        let ink = theme.foreground;
        let muted = theme.muted_foreground;
        let amber = theme.warning;
        let hairline = crate::ui::theme::hairline(window);
        let menu = self.composer_menu(cx);

        let chips = (!c.attached.is_empty()).then(|| {
            h_flex()
                .flex_wrap()
                .gap(px(6.))
                .px(px(10.))
                .pt(px(10.))
                .children(c.attached.iter().enumerate().map(|(i, path)| {
                    let name = path
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or(path)
                        .trim_matches('\'')
                        .to_string();
                    let image = ["png", "jpg", "jpeg", "gif", "webp"]
                        .iter()
                        .any(|ext| name.to_lowercase().ends_with(&format!(".{ext}")));
                    let glyph = match image {
                        true => div()
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .bg(ink.opacity(0.08))
                            .child(
                                gpui_kit::svg()
                                    .path("icons/image.svg")
                                    .size(px(11.))
                                    .text_color(muted),
                            )
                            .into_any_element(),
                        false => gpui_kit::svg()
                            .path("icons/file.svg")
                            .mx(px(2.))
                            .size(px(12.))
                            .text_color(muted)
                            .into_any_element(),
                    };
                    h_flex()
                        .id(("composer-chip", i))
                        .h(px(28.))
                        .max_w(px(260.))
                        .gap(px(7.))
                        .pl(px(6.))
                        .pr(px(4.))
                        .rounded(px(7.))
                        .bg(ink.opacity(0.05))
                        .text_size(px(12.))
                        .tooltip({
                            let path: SharedString = path.clone().into();
                            move |window, cx| Tooltip::new(path.clone()).build(window, cx)
                        })
                        .child(glyph)
                        .child(div().min_w_0().truncate().child(name))
                        .child(
                            div()
                                .id(("composer-chip-remove", i))
                                .size(px(18.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .hover(|s| s.bg(ink.opacity(0.07)))
                                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(move |this, _, _w, cx| this.detach(i, cx)))
                                .child(Icon::new(IconName::Close).size(px(9.)).text_color(muted)),
                        )
                }))
        });

        // A toolbar button: 28px tall, the box's own hover tint.
        let tool = |id: &'static str| {
            h_flex()
                .id(id)
                .flex_none()
                .h(px(28.))
                .gap(px(6.))
                .px(px(9.))
                .rounded(px(7.))
                .text_size(px(12.))
                .text_color(ink.opacity(0.5))
                .hover(move |s| s.bg(ink.opacity(0.05)))
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
        };

        let attach = tool("composer-attach")
            .w(px(28.))
            .px_0()
            .justify_center()
            .tooltip(|window, cx| Tooltip::new(t(L10nKey::ComposerAttach)).build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| this.pick_attachments(window, cx)))
            .child(
                Icon::new(IconName::Plus)
                    .size(px(12.))
                    .text_color(ink.opacity(0.5)),
            );

        let send = div()
            .id("composer-send")
            .flex_none()
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(match can_send {
                true => ink,
                false => ink.opacity(0.07),
            })
            .tooltip(|window, cx| Tooltip::new(t(L10nKey::ComposerSendTip)).build(window, cx))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(cx.listener(|this, _, window, cx| this.submit_composer(window, cx)))
            .child(
                Icon::new(IconName::ArrowUp)
                    .size(px(12.))
                    .text_color(match can_send {
                        true => theme.background,
                        false => ink.opacity(0.35),
                    }),
            );

        let asking = self.agent_is_asking().then(|| {
            div()
                .min_w_0()
                .truncate()
                .px(px(6.))
                .text_size(px(12.))
                .text_color(amber)
                .child(t_fmt(
                    L10nKey::ComposerAgentAsking,
                    &[("agent", agent.display_name())],
                ))
        });
        let result = self
            .agent_session()
            .filter(|session| session.status == AgentStatus::Done)
            .and_then(|session| session.readout.result)
            .map(|result| {
                let text: SharedString = crate::ui::i18n::agent_result_text(&result).into();
                let color = match result.outcome {
                    crate::core::cli_agent::AgentOutcome::Succeeded => muted,
                    crate::core::cli_agent::AgentOutcome::Failed => theme.danger,
                    _ => theme.warning,
                };
                div()
                    .id("composer-result")
                    .min_w_0()
                    .truncate()
                    .px_2()
                    .text_xs()
                    .text_color(color)
                    .child(text.clone())
                    .tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
            });

        let popup = menu.as_ref().map(|(trigger, items)| {
            let highlighted = c.highlighted.min(items.len() - 1);
            let title = match trigger.sigil {
                '/' => t(L10nKey::ComposerMenuCommands),
                _ => t(L10nKey::ComposerMenuFiles),
            };
            crate::ui::theme::floating_surface(div(), cx)
                .rounded(px(10.))
                .absolute()
                .left_0()
                .bottom_full()
                .mb(px(6.))
                .w(px(380.))
                .max_w_full()
                .p(px(5.))
                .flex()
                .flex_col()
                .gap(px(1.))
                .text_size(px(13.))
                .child(
                    div()
                        .h(px(24.))
                        .px(px(9.))
                        .flex()
                        .items_center()
                        .text_size(px(11.5))
                        .text_color(ink.opacity(0.4))
                        .child(title),
                )
                .children(items.iter().enumerate().map(|(i, item)| {
                    h_flex()
                        .id(("composer-menu", i))
                        .h(px(30.))
                        .px(px(9.))
                        .gap(px(10.))
                        .rounded(px(6.))
                        .when(i == highlighted, |s| s.bg(ink.opacity(0.05)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                window.prevent_default();
                                cx.stop_propagation();
                                this.pick_menu_item(i, window, cx);
                            }),
                        )
                        .when(item.is_file, |s| {
                            s.child(
                                gpui_kit::svg()
                                    .path("icons/file.svg")
                                    .size(px(12.))
                                    .text_color(ink.opacity(0.4)),
                            )
                        })
                        .child(
                            div()
                                .flex_none()
                                .font_family(self.font.family.clone())
                                .text_size(px(12.))
                                .child(item.label.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_right()
                                .text_size(px(12.))
                                .text_color(ink.opacity(0.4))
                                .child(item.detail.clone()),
                        )
                }))
        });

        let menu_open = menu.is_some();
        Some(
            div()
                .id("composer")
                .relative()
                .w_full()
                // The terminal surface this sits in focuses the grid on any
                // click and opens its own context menu on a right one. Neither
                // is right for a click on the box.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        window.prevent_default();
                        // Not the layer under it, which takes a click as one
                        // on the terminal.
                        cx.stop_propagation();
                        // A click on the frame around the text, not only on
                        // the text, is a click on the box.
                        if !this.composer_focused {
                            this.focus_composer(window, cx);
                        }
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _: &MouseDownEvent, _w, cx| {
                        this.links.record_menu_permission(true, false);
                        cx.stop_propagation();
                    }),
                )
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                // Files dropped on the box are attachments, not words for the
                // terminal's line; the surface's own drop handler would focus
                // the grid first.
                .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                    cx.stop_propagation();
                    this.focus_composer(window, cx);
                    this.drop_files(paths, cx);
                }))
                .on_drop(cx.listener(
                    |this, drag: &crate::ui::file_tree::RemotePathDrag, window, cx| {
                        cx.stop_propagation();
                        this.focus_composer(window, cx);
                        this.drop_remote_path(drag, cx);
                    },
                ))
                .capture_action(cx.listener(|this, _: &input::Paste, _w, cx| {
                    // Text pastes are the box's own. A copied file or a
                    // screenshot has no text to paste, and the terminal
                    // already knows how to turn those into a path.
                    let Some(item) = cx.read_from_clipboard() else {
                        return;
                    };
                    if item.text().is_some() && !super::view::clipboard_has_paths(&item) {
                        return;
                    }
                    cx.stop_propagation();
                    this.paste_from_clipboard(cx);
                }))
                // Shift+Tab is the agent's: it cycles the permission mode.
                .capture_action(cx.listener(|this, _: &input::OutdentInline, _w, cx| {
                    cx.stop_propagation();
                    this.send_to_pty(BACK_TAB, cx);
                }))
                .capture_action(cx.listener(|this, _: &input::Backspace, _w, cx| {
                    let Some(c) = this.composer.as_ref() else {
                        return;
                    };
                    let state = c.input.read(cx);
                    if state.cursor() != 0 || !state.selected_range().is_empty() {
                        return;
                    }
                    if let Some(last) = c.attached.len().checked_sub(1) {
                        cx.stop_propagation();
                        this.detach(last, cx);
                    }
                }))
                .when(menu_open, |el| {
                    el.capture_action(cx.listener(|this, _: &input::MoveUp, _w, cx| {
                        if this.step_menu(false, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .capture_action(cx.listener(|this, _: &input::MoveDown, _w, cx| {
                        if this.step_menu(true, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .capture_action(cx.listener(|this, action: &input::Enter, window, cx| {
                        if action.shift {
                            return;
                        }
                        cx.stop_propagation();
                        let at = this.composer.as_ref().map_or(0, |c| c.highlighted);
                        this.pick_menu_item(at, window, cx);
                    }))
                    .capture_action(cx.listener(|this, _: &input::IndentInline, window, cx| {
                        cx.stop_propagation();
                        let at = this.composer.as_ref().map_or(0, |c| c.highlighted);
                        this.pick_menu_item(at, window, cx);
                    }))
                    .capture_action(cx.listener(
                        |this, _: &input::Escape, _w, cx| {
                            cx.stop_propagation();
                            if let Some(c) = this.composer.as_mut() {
                                c.dismissed = Some(c.input.read(cx).value().to_string());
                            }
                            cx.notify();
                        },
                    ))
                })
                .children(popup.map(|p| gpui_kit::deferred(p).with_priority(1)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .rounded(px(12.))
                        // Opaque: under it can be whatever colour the agent
                        // paints its screen in.
                        .bg(theme.background.blend(ink.opacity(0.035)))
                        .relative()
                        .border_t(hairline)
                        .border_b(hairline)
                        .border_l(hairline)
                        .border_r(hairline)
                        .border_color(ink.opacity(0.15))
                        .children(chips)
                        .child(
                            div()
                                .min_h(px(44.))
                                .px(px(14. - INPUT_PAD_X))
                                .pt(px(12. - INPUT_PAD_Y))
                                .pb(px(4. - INPUT_PAD_Y))
                                .child(
                                    Textarea::new(&c.input)
                                        .appearance(false)
                                        .with_size(Size::Size(px(TEXT_PX / 0.875))),
                                ),
                        )
                        .child(
                            h_flex()
                                .h(px(40.))
                                .px(px(6.))
                                .gap(px(2.))
                                .child(attach)
                                .children(asking)
                                .children(result)
                                .child(div().flex_1())
                                .child(send),
                        )
                        // Focus and a drag over the box thicken the ring. Drawn
                        // as a layer over the hairline, not as a wider border,
                        // so the box's contents stay where they are.
                        .child(
                            // Laid out inside the hairline, so pulled out over it.
                            div()
                                .absolute()
                                .top(-hairline)
                                .bottom(-hairline)
                                .left(-hairline)
                                .right(-hairline)
                                .rounded(px(12.))
                                .border_1()
                                .border_color(match focused {
                                    true => ink.opacity(0.22),
                                    false => gpui_kit::transparent_black(),
                                })
                                .drag_over::<ExternalPaths>(move |s, _, _, _| {
                                    s.border_color(ink.opacity(0.48))
                                }),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(steps: &[Step]) -> Vec<&[u8]> {
        steps.iter().map(|s| s.bytes.as_slice()).collect()
    }

    #[test]
    fn a_plain_line_is_typed_then_entered_separately() {
        let steps = submit_plan(CLIAgent::Pi, "fix the tests", true);
        assert_eq!(bytes(&steps), [&b"fix the tests"[..], b"\r"]);
        assert_eq!(steps[0].delay, Duration::ZERO);
        assert_eq!(steps[1].delay, SETTLE);
    }

    #[test]
    fn several_lines_go_as_one_paste() {
        let steps = submit_plan(CLIAgent::Pi, "one\r\ntwo\n", true);
        assert_eq!(bytes(&steps), [&b"\x1b[200~one\ntwo\x1b[201~"[..], b"\r"]);
    }

    #[test]
    fn without_bracketed_paste_line_breaks_stay_newlines_not_enters() {
        let steps = submit_plan(CLIAgent::Pi, "one\ntwo", false);
        assert_eq!(bytes(&steps), [&b"one\ntwo"[..], b"\r"]);
    }

    #[test]
    fn kimi_is_always_pasted() {
        // Typed, the Enter behind the line is one of its newlines.
        for text in ["fix it", "?why", "!ls", "/usage"] {
            let steps = submit_plan(CLIAgent::Kimi, text, true);
            let pasted = format!("\x1b[200~{text}\x1b[201~");
            assert_eq!(bytes(&steps), [pasted.as_bytes(), b"\r"], "{text}");
            assert_eq!(steps[1].delay, SETTLE, "{text}");
        }
    }

    #[test]
    fn opencode_is_always_pasted() {
        let steps = submit_plan(CLIAgent::OpenCode, "PONG! ok?", true);
        assert_eq!(bytes(&steps), [&b"\x1b[200~PONG! ok?\x1b[201~"[..], b"\r"]);
        let steps = submit_plan(CLIAgent::OpenCode, "!ls", true);
        assert_eq!(bytes(&steps), [&b"!"[..], b"\x1b[200~ls\x1b[201~", b"\r"]);
    }

    #[test]
    fn escapes_cannot_close_the_paste_early() {
        let steps = submit_plan(CLIAgent::Pi, "a\n\x1b[201~b", true);
        assert_eq!(bytes(&steps)[0], b"\x1b[200~a\n[201~b\x1b[201~");
    }

    #[test]
    fn attachments_follow_the_text_as_shell_words() {
        let attached = vec!["/tmp/a b.png".to_string(), "/src/x.rs".to_string()];
        let claude = CLIAgent::Pi;
        assert_eq!(
            compose_message(claude, "look at these\n", &attached, Some("zsh")),
            "look at these '/tmp/a b.png' /src/x.rs"
        );
        assert_eq!(
            compose_message(claude, "", &attached[1..], Some("zsh")),
            "/src/x.rs"
        );
        assert_eq!(
            compose_message(claude, "just text  ", &[], Some("zsh")),
            "just text"
        );
    }

    #[test]
    fn opencode_gets_each_attachment_as_a_paste_of_its_own() {
        let attached = vec!["/tmp/a b.png".to_string(), "/src/x.rs".to_string()];
        for agent in [CLIAgent::OpenCode] {
            let steps = submit_message(agent, "look\n", &attached, Some("zsh"), true);
            assert_eq!(
                bytes(&steps),
                [
                    &b"\x1b[200~look\x1b[201~"[..],
                    b" ",
                    b"\x1b[200~/tmp/a b.png\x1b[201~",
                    b" ",
                    b"\x1b[200~/src/x.rs\x1b[201~",
                    b"\r",
                ],
                "{agent:?}"
            );
            // Attachments alone: nothing to space them from.
            let steps = submit_message(agent, "", &attached[..1], None, true);
            assert_eq!(
                bytes(&steps),
                [&b"\x1b[200~/tmp/a b.png\x1b[201~"[..], b"\r"]
            );
        }
        // Everyone else still gets them as words after the text.
        let steps = submit_message(CLIAgent::Pi, "look", &attached, Some("zsh"), true);
        assert_eq!(
            bytes(&steps),
            [&b"look '/tmp/a b.png' /src/x.rs"[..], b"\r"]
        );
    }

    #[test]
    fn mentions_close_before_enter_for_oh_my_pi() {
        let steps = submit_plan(CLIAgent::OhMyPi, "read @plain.txt", true);
        assert_eq!(bytes(&steps), [&b"read @plain.txt "[..], b"\r"]);
    }

    /// Oh My Pi reads a mentioned file, an image too, into the turn; a
    /// mention with a space goes in its quotes.
    #[test]
    fn oh_my_pi_gets_its_attachments_as_mentions() {
        let attached = vec!["/tmp/red.png".to_string(), "/tmp/a b.png".to_string()];
        assert_eq!(
            compose_message(CLIAgent::OhMyPi, "look", &attached, Some("zsh")),
            "look @/tmp/red.png @\"/tmp/a b.png\""
        );
        assert_eq!(
            compose_message(CLIAgent::Pi, "look", &attached, Some("zsh")),
            "look /tmp/red.png '/tmp/a b.png'"
        );
    }

    #[test]
    fn mentions_are_cut_out_where_they_name_a_path() {
        let exists = |p: &str| ["my notes.md", "plain.md", "src/a b/c.rs"].contains(&p);
        assert_eq!(
            mention_pieces("read @my notes.md and @plain.md.", exists),
            [
                Piece::Text("read "),
                Piece::Mention("my notes.md"),
                Piece::Text(" and "),
                Piece::Mention("plain.md"),
                Piece::Text("."),
            ]
        );
        assert_eq!(
            mention_pieces("@src/a b/c.rs", exists),
            [Piece::Mention("src/a b/c.rs")]
        );
        // An `@` inside a word, or naming nothing, stays words.
        for text in ["mail me@plain.md", "ask @someone about it", "@ plain.md"] {
            assert_eq!(mention_pieces(text, exists), [Piece::Text(text)]);
        }
        assert_eq!(mention_pieces("", exists), []);
    }

    /// OpenCode attaches only what is picked from its `@` list, so each
    /// mention is picked there: `@`, the path as the query, Enter.
    #[test]
    fn opencode_picks_each_mention_from_its_own_list() {
        let pieces = [
            Piece::Text("read "),
            Piece::Mention("my notes.md"),
            Piece::Text(" now, please"),
        ];
        let steps = opencode_mention_plan(&pieces, &[], true).unwrap();
        assert_eq!(
            bytes(&steps),
            [
                &b"\x1b[200~read \x1b[201~"[..],
                b"@",
                b"\x1b[200~mynotes.md\x1b[201~",
                b"\r",
                b"\x1b[200~now, please\x1b[201~",
                b"\r",
            ]
        );
        // With no space in what follows, one of its own closes the list.
        let steps =
            opencode_mention_plan(&[Piece::Mention("a.md"), Piece::Text(".")], &[], true).unwrap();
        assert_eq!(
            bytes(&steps),
            [
                &b"@"[..],
                b"\x1b[200~a.md\x1b[201~",
                b"\r",
                b" ",
                b"\x1b[200~.\x1b[201~",
                b"\r",
            ]
        );
        // Ending on one, a space closes a list that found nothing, and
        // attachments still follow as pastes of their own.
        let attached = vec!["/tmp/a.png".to_string()];
        let steps = opencode_mention_plan(&[Piece::Mention("plain.md")], &attached, true).unwrap();
        assert_eq!(
            bytes(&steps),
            [
                &b"@"[..],
                b"\x1b[200~plain.md\x1b[201~",
                b"\r",
                b" ",
                b" ",
                b"\x1b[200~/tmp/a.png\x1b[201~",
                b"\r",
            ]
        );
        // Without a mention, or as a command or shell line, it goes as usual.
        assert_eq!(opencode_mention_plan(&[Piece::Text("hi")], &[], true), None);
        assert_eq!(opencode_mention_plan(&pieces, &[], false), None);
        let command = [Piece::Text("/review "), Piece::Mention("plain.md")];
        assert_eq!(opencode_mention_plan(&command, &[], true), None);
    }

    #[test]
    fn a_mention_with_a_space_is_spelled_the_way_each_agent_reads_it() {
        for agent in [CLIAgent::Kimi, CLIAgent::Pi, CLIAgent::OhMyPi] {
            assert_eq!(
                mention(Some(agent), "my notes.md", Some("zsh")),
                "@\"my notes.md\""
            );
            assert_eq!(mention(Some(agent), "src/a.rs", Some("zsh")), "@src/a.rs");
        }
    }

    fn screen(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    const RULE: &str = "────────────────────────────────────────";

    const PI_FOOTER: [&str; 2] = [
        "/private/tmp/demo/proj (main)",
        "↑29k ↓52 1.9%/200k (auto)          claude-sonnet-4-6",
    ];

    fn pi_screen(lines: &[&str]) -> Vec<String> {
        let mut rows = screen(lines);
        rows.extend(PI_FOOTER.iter().map(|r| r.to_string()));
        rows
    }

    #[test]
    fn pis_input_is_its_lowest_ruled_editor() {
        // Empty, idle.
        let rows = pi_screen(&[" 只回复两个字：好的", "", " 好的", "", RULE, "", RULE]);
        assert_eq!(
            input_area(CLIAgent::Pi, &rows, 40),
            Some(InputArea { top: 4 })
        );
        // Several lines in it, while a turn runs and the spinner rides the
        // upper rule.
        let working = "── ⠼ Working ──────────────────────────────";
        let rows = pi_screen(&["", working, "line one", "line two", RULE]);
        assert_eq!(
            input_area(CLIAgent::Pi, &rows, 40),
            Some(InputArea { top: 1 })
        );
        // Its completion list opens between the editor and the status rows.
        let rows = pi_screen(&[
            RULE,
            "/mod",
            RULE,
            "→ model          <provider/model> — Select model",
            "  scoped-models  Enable/disable models for Ctrl+P cycling",
        ]);
        assert_eq!(
            input_area(CLIAgent::Pi, &rows, 40),
            Some(InputArea { top: 0 })
        );
    }

    /// Pi's selectors take the editor's place between its rules: none of
    /// them is the input.
    #[test]
    fn pis_selectors_are_not_its_input() {
        let model = pi_screen(&[
            " 好的",
            "",
            RULE,
            "",
            "Only showing models from configured providers. Use /login to add providers.",
            "",
            ">",
            "",
            "→ ✓ claude-sonnet-4-6 [relay] · default",
            "    gpt-5.5 [relay]",
            "",
            "  Enter to select · Ctrl+S to set as default · Escape/Ctrl+C to cancel",
            RULE,
        ]);
        assert_eq!(input_area(CLIAgent::Pi, &model, 40), None);
        let settings = pi_screen(&[
            " Operation aborted",
            "",
            RULE,
            ">",
            "",
            "→ Auto-compact                      true",
            "  Auto-resize images                true",
            "  (1/32)",
            "",
            "  Type to search · Enter/Space to change · Esc to cancel",
            RULE,
        ]);
        assert_eq!(input_area(CLIAgent::Pi, &settings, 40), None);
        let resume = pi_screen(&[
            RULE,
            "",
            "Resume Session (Current Folder)     ◉ Current Folder | ○ All",
            "tab scope · re:<pattern> regex · \"phrase\" exact",
            "",
            ">",
            "",
            "› Reply with just: ok1                         18 now",
            RULE,
        ]);
        assert_eq!(input_area(CLIAgent::Pi, &resume, 40), None);
        // A ruled block far above the bottom is the transcript's.
        let mut scrolled = screen(&[RULE, "quoted", RULE]);
        scrolled.extend((0..12).map(|i| format!(" reply line {i}")));
        assert_eq!(input_area(CLIAgent::Pi, &scrolled, 40), None);
    }

    const OMP_BAR: &str =
        " π > ◒ claude-sonnet-4-6 (relay) > 🗑 demo/proj > ⑂ main ?15 ▶─4%─────┃────200K─";

    #[test]
    fn omps_input_hangs_under_its_status_line() {
        let rows = screen(&[
            " DONE",
            "",
            OMP_BAR,
            "╰─                                  ⇧⇥ to change thinking effort",
        ]);
        assert_eq!(
            input_area(CLIAgent::OhMyPi, &rows, 40),
            Some(InputArea { top: 2 })
        );
        // Several lines, and its command list open under them.
        let rows = screen(&[
            RULE,
            "",
            OMP_BAR,
            "╰─ /model",
            "   second line",
            "❯ ⬢  model         Model: relay/claude-sonnet-4-6                █",
            "  ⬢  modelpreset   Presets: none saved                           █",
        ]);
        assert_eq!(
            input_area(CLIAgent::OhMyPi, &rows, 40),
            Some(InputArea { top: 2 })
        );
        // The rounded box shape.
        let rows = screen(&[
            "╭── π > ⬢ Sonnet 4.5 · ◒ high > 🗺 Plan ▶────────┃──◀ ◫ 62.0%/200K ⟲ ──╮",
            "╰─ Ask anything, edit files, run tools                               ─╯",
        ]);
        assert_eq!(
            input_area(CLIAgent::OhMyPi, &rows, 40),
            Some(InputArea { top: 0 })
        );
        // The Claude Code shape, ruled like Pi's.
        let rows = screen(&[
            "───────────────────────────── gallery · ◫ 62.0%/200K ⟲ ─",
            "❯ Ask anything, edit files, run tools",
            RULE,
            " π · ⬢ Sonnet 4.5 · ◒ high · 🗺 Plan",
        ]);
        assert_eq!(
            input_area(CLIAgent::OhMyPi, &rows, 40),
            Some(InputArea { top: 0 })
        );
    }

    /// A tool approval and the model picker take the input's place, framed;
    /// a frame's bottom edge is not the input's first line.
    #[test]
    fn omps_dialogs_are_not_its_input() {
        let approval = screen(&[
            "╭──────────────────────────────────────╮",
            "│ $ touch omp2.txt                     │",
            "╰──────────────────────────────────────╯",
            "  ⎋ Creating omp2.txt",
            "╭─ Allow tool: bash ───────────────────╮",
            "│ Command: touch omp2.txt              │",
            "│  ❯ Approve                           │",
            "│    Deny                              │",
            "│ ↑/↓ navigate  ⏎ select  ⎋ cancel     │",
            "╰──────────────────────────────────────╯",
        ]);
        assert_eq!(input_area(CLIAgent::OhMyPi, &approval, 40), None);
        let picker = screen(&[
            "│   ○ anthropic   │   local/qwen2.5-1.5b          free█ │",
            "│   ○ gmi-cloud   │   ● default ◒ · ○ smol · ○ slow   │",
            "├─────────────────┴──────────────────────────────────┤",
            "│ ⏎/→ models · ↑/↓ providers · type to search · ⎋ close │",
            "╰────────────────────────────────────────────────────╯",
        ]);
        assert_eq!(input_area(CLIAgent::OhMyPi, &picker, 40), None);
    }

    const KIMI_FOOTER: [&str; 2] = [
        " claude-sonnet-4-6 thinking: high  …/local/agents/smoke",
        "                                   context: 0% (0/195k)",
    ];

    #[test]
    fn kimis_input_is_the_frame_over_its_model_line() {
        let at = |inside: &[&str]| {
            let mut rows = vec![
                " ╭──────────────────────────────────────╮".to_string(),
                " │  ▐█▛█▛█▌  Welcome to Kimi Code!       │".to_string(),
                " ╰──────────────────────────────────────╯".to_string(),
                String::new(),
                "   No session yet — one will be created.".to_string(),
                String::new(),
                " ╭──────────────────────────────────────╮".to_string(),
            ];
            rows.extend(inside.iter().map(|l| l.to_string()));
            rows.push(" ╰──────────────────────────────────────╯".to_string());
            rows.extend(KIMI_FOOTER.map(String::from));
            rows.extend(std::iter::repeat_n(String::new(), 5));
            rows
        };
        for (inside, held) in [
            (&[" │ >                                    │"][..], 0),
            (
                &[
                    " │ > line one                           │",
                    " │   line two                           │",
                    " │   line three                         │",
                ][..],
                3,
            ),
        ] {
            let rows = at(inside);
            assert_eq!(
                input_area(CLIAgent::Kimi, &rows, 40),
                Some(InputArea { top: 6 })
            );
            assert_eq!(held_lines(CLIAgent::Kimi, &rows), held);
        }
    }

    /// Kimi Code's lists open under its input's frame; a tool approval takes
    /// the frame's place, with only its welcome banner framed far above.
    #[test]
    fn kimis_lists_and_approvals_are_not_its_input() {
        let slash = screen(&[
            " ╭──────────────────────────────────────╮",
            " │ > /                                  │",
            " ╰──────────────────────────────────────╯",
            " │   → yolo           Ask When Needed    │",
            " │     model          Switch LLM model   │",
            " │     permission     Select permission  │",
            " │     plan           Toggle plan mode   │",
            " │     (1/51)                            │",
            KIMI_FOOTER[0],
            KIMI_FOOTER[1],
        ]);
        let approval = screen(&[
            " ╰──────────────────────────────────────╯",
            "",
            " ✨ Use the shell to run: touch kimi.txt",
            "",
            " ● Running a command · $ touch kimi.txt",
            " ──────────────────────────────────────",
            "   ▶ Run this command?",
            "",
            "   ▶ 1. Approve once",
            "     2. Approve for this session",
            "     3. Reject",
            "",
            "   ↑/↓ select · 1/2/3/4 choose · ↵ confirm",
            " ──────────────────────────────────────",
            KIMI_FOOTER[0],
            KIMI_FOOTER[1],
        ]);
        for rows in [slash, approval] {
            assert_eq!(input_area(CLIAgent::Kimi, &rows, 40), None, "{rows:#?}");
        }
    }

    const OPENCODE_EDGE: &str = "  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀";

    #[test]
    fn opencodes_input_is_the_bar_over_its_key_hints() {
        // Its home screen, the input in the middle over the directory.
        let rows = screen(&[
            "        █▀▀█ █▀▀█ █▀▀█ █▀▀▄ █▀▀▀ █▀▀█",
            "",
            "    ┃",
            "    ┃  Ask anything... \"Fix broken tests\"",
            "    ┃",
            "    ┃  Build · claude-sonnet-4-6 Relay",
            "    ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀",
            "    tab agents  ctrl+p commands",
            "",
            "",
            "  /private/tmp/local/agents/smoke            1.18.4",
        ]);
        assert_eq!(
            input_area(CLIAgent::OpenCode, &rows, 40),
            Some(InputArea { top: 2 })
        );
        assert_eq!(held_lines(CLIAgent::OpenCode, &rows), 0);
        // A session, a turn running, two lines typed.
        let rows = screen(&[
            "  ┃  Use the shell to run: touch oc.txt",
            "  ┃",
            "",
            "     ▣  Build · claude-sonnet-4-6",
            "",
            "  ┃",
            "  ┃  line one",
            "  ┃  line two",
            "  ┃",
            "  ┃  Build · claude-sonnet-4-6 Relay",
            OPENCODE_EDGE,
            "   ⬝⬝⬝⬝⬝⬝⬝⬝  esc interrupt         ctrl+p commands",
            "",
        ]);
        assert_eq!(
            input_area(CLIAgent::OpenCode, &rows, 40),
            Some(InputArea { top: 5 })
        );
        assert_eq!(held_lines(CLIAgent::OpenCode, &rows), 3);
    }

    /// OpenCode's lists open on its input's bar, closed by a bar at the
    /// right; a permission request takes the input's place, with no edge.
    #[test]
    fn opencodes_lists_and_requests_are_not_its_input() {
        let slash = screen(&[
            "  ┃ /agents     Switch agent                ┃",
            "  ┃ /connect    Connect provider            ┃",
            "  ┃",
            "  ┃  /",
            "  ┃",
            "  ┃  Build · claude-sonnet-4-6 Relay",
            OPENCODE_EDGE,
            "   tab agents  ctrl+p commands",
        ]);
        let request = screen(&[
            "     $ touch /tmp/local/agents/smoke/oc.txt",
            "",
            "  ┃",
            "  ┃  △ Permission required",
            "  ┃    ← Access external directory",
            "  ┃",
            "  ┃   Allow once   Allow always   Reject",
            "  ┃",
            "",
        ]);
        let sent = screen(&["  ┃", "  ┃  Reply with just PONG", "  ┃", "", "     PONG"]);
        for rows in [slash, request, sent] {
            assert_eq!(input_area(CLIAgent::OpenCode, &rows, 40), None, "{rows:#?}");
        }
    }

    /// The grid gives up rows only for the part of the box an input does not
    /// already hold, and whole rows of it.
    #[test]
    fn the_grid_makes_room_only_for_what_the_input_does_not_hold() {
        let (line, below) = (px(21.), px(9.));
        // An existing five-row input holds the box.
        assert_eq!(rows_short(px(101.), line, 5, below), 0);
        // Oh My Pi's two do not: 101 - 42 - 9 = 50, three rows' worth.
        assert_eq!(rows_short(px(101.), line, 2, below), 3);
        assert_eq!(rows_short(px(101.), line, 4, below), 1);
        assert_eq!(rows_short(px(101.), px(0.), 2, below), 0);
    }

    #[test]
    fn each_agents_builtin_commands_are_distinct_slash_words() {
        for agent in CLIAgent::ALL {
            let names: Vec<&str> = builtin_commands(agent).iter().map(|(n, _)| *n).collect();
            for (i, name) in names.iter().enumerate() {
                assert!(name.starts_with('/') && !name.contains(' '), "{name}");
                assert!(!names[..i].contains(name), "{agent:?} lists {name} twice");
            }
        }
        // Kimi exposes effort as an interactive slash command.
        assert!(
            builtin_commands(CLIAgent::Kimi)
                .iter()
                .any(|(n, _)| *n == "/effort")
        );
    }

    #[test]
    fn a_slash_opens_the_menu_only_at_the_start_of_the_message() {
        let t = |text: &str| trigger_at(text, text.len());
        assert_eq!(
            t("/comp"),
            Some(Trigger {
                sigil: '/',
                start: 0,
                query: "comp".into()
            })
        );
        assert_eq!(t("  /").map(|t| t.start), Some(2));
        assert_eq!(t("see src/main.rs"), None);
        assert_eq!(t("fix /tmp"), None);
    }

    #[test]
    fn an_at_opens_the_menu_at_the_start_of_any_word() {
        let text = "look at @src/ma and";
        let cursor = "look at @src/ma".len();
        assert_eq!(
            trigger_at(text, cursor),
            Some(Trigger {
                sigil: '@',
                start: 8,
                query: "src/ma".into()
            })
        );
        assert_eq!(trigger_at("mail me@host", 12), None);
        assert_eq!(
            trigger_at("done @x ", 8),
            None,
            "a finished word is not a query"
        );
    }

    #[test]
    fn commands_that_start_with_the_query_come_before_ones_that_contain_it() {
        let all: Vec<(String, String)> = [("/compact", ""), ("/clear", ""), ("/memory", "")]
            .iter()
            .map(|(n, d)| (n.to_string(), d.to_string()))
            .collect();
        let labels = |q| {
            command_items(&all, q)
                .into_iter()
                .map(|i| i.label)
                .collect::<Vec<_>>()
        };
        assert_eq!(labels("c"), ["/compact", "/clear"]);
        assert_eq!(labels("m"), ["/memory", "/compact"]);
        assert_eq!(labels(""), ["/compact", "/clear", "/memory"]);
    }

    #[test]
    fn a_file_mention_is_spelled_from_the_panes_directory() {
        let index = crate::ui::search::files::build_index(
            &[PathBuf::from("/repo")],
            vec![
                PathBuf::from("/repo/app/src/main.rs"),
                PathBuf::from("/repo/README.md"),
            ],
            false,
        );
        let items = file_items(&index, "main", Some(Path::new("/repo/app")), None, None);
        assert_eq!(items[0].insert, "@src/main.rs");
        let items = file_items(&index, "readme", Some(Path::new("/repo/app")), None, None);
        assert_eq!(
            items[0].insert, "@/repo/README.md",
            "outside the cwd: in full"
        );
    }
}
