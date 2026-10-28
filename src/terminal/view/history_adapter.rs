//! Host-aware history loading and reverse-search presentation.
//! TerminalHistory owns scope/cache/navigation/search and persistence decisions;
//! this adapter schedules Host reads and applies only accepted load results.

use super::*;
use super::prompt::menu_layout;
use crate::terminal::history_session::{PromptHistory, unix_now};

const MAX_HISTORY_BYTES: u64 = 4 << 20;

impl TerminalView {
    pub(super) fn desired_history_scope(&self) -> crate::terminal::history::Scope {
        if let Some(ctx) = self.remote_context() {
            return crate::terminal::history::Scope::remote(&ctx.target);
        }
        if !self.host_id.is_local() {
            return crate::terminal::history::Scope::remote(&format!("host-{:016x}", self.host_id.0));
        }
        crate::terminal::history::Scope::Local
    }

    pub(super) fn history_prompt(&self) -> PromptHistory {
        PromptHistory::new(
            self.terminal.prompt_seq(),
            self.terminal.at_prompt(),
            self.terminal.last_exit_code(),
        )
    }

    pub(super) fn follow_history_scope(&mut self, cx: &mut Context<Self>) {
        let scope = self.desired_history_scope();
        let Some(load) = self
            .history
            .change_scope(scope, self.host_id, self.history_prompt())
        else {
            return;
        };
        cx.notify();
        let shell_files = self.remote_shell_history_sources(cx);
        let loading = load.scope().clone();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let files = shell_files
                        .into_iter()
                        .filter_map(|(host, path)| {
                            // The name comes along: it is what tells the loader
                            // which shell's format the bytes are in.
                            let name = path.file_name()?.to_string_lossy().into_owned();
                            Some((name, host.read_file(&path, MAX_HISTORY_BYTES).ok()?))
                        })
                        .collect();
                    crate::terminal::history::load_with_shell_files(&loading, files)
                })
                .await;
            this.update(cx, |view, cx| {
                if view.history.accept_load(load, view.host_id, loaded) {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn remote_shell_history_sources(
        &self,
        cx: &mut Context<Self>,
    ) -> Vec<(crate::ui::host_ops::SharedHost, std::path::PathBuf)> {
        if self.history.scope().is_local() || self.host_id.is_local() {
            return Vec::new();
        }
        // The Host reaches the workspace machine's home directory and nothing
        // beyond it. A pane that has ssh'ed onward from there (remote_context)
        // is scoped to the *inner* target, and seeding that scope from the
        // workspace host's ~/.zsh_history would offer commands from the wrong
        // box — the exact confusion scoping exists to prevent. Those panes
        // start from what lyrel recorded for the inner target, like bare ssh.
        if self.remote_context().is_some() {
            return Vec::new();
        }
        let Some(host) = self.host(cx) else {
            return Vec::new();
        };
        if !host.is_connected() {
            return Vec::new();
        }
        let Some(home) = crate::ui::remote_connect::HostLinks::home(cx, self.host_id) else {
            return Vec::new();
        };
        crate::terminal::history::shell_history_names()
            .into_iter()
            .map(|name| (std::sync::Arc::clone(&host), host.join(&home, name)))
            .collect()
    }

    pub(super) fn flush_pending_history(&mut self) {
        self.history.flush_pending(self.history_prompt());
    }

    pub(super) fn render_reverse_search_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let rs = self.history.search()?;
        let matches = rs.matches();
        if matches.is_empty() {
            return None;
        }
        let (srow, _) = self.cursor_cell()?;

        const MAX_ROWS: usize = 10;
        let (total_rows, total_cols) = {
            let term = self.terminal.term.lock();
            (term.screen_lines(), term.columns())
        };
        let (place_above, visible, first) =
            menu_layout(total_rows, srow, matches.len(), rs.selected(), MAX_ROWS);
        let hidden_above = first;
        let hidden_below = matches.len() - first - visible;

        let theme = cx.theme();
        let lh = self.line_height;
        let now = unix_now();
        let row = |i: usize| {
            let m = &matches[i];
            let line = self.history.entries()[m.index].as_str();
            let selected = rs.selected() == i;
            let base = if selected {
                theme.foreground
            } else {
                theme.muted_foreground
            };

            let spans: Vec<gpui_kit::AnyElement> = highlight_runs(line, &m.positions)
                .into_iter()
                .map(|(run, hit)| {
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .text_color(if hit { theme.blue } else { base })
                        .child(run)
                        .into_any_element()
                })
                .collect();

            let meta = self.history.metadata(line);
            let failed = meta.and_then(|em| em.exit).filter(|&e| e != 0);
            let ago = meta
                .and_then(|em| em.ts)
                .map(|ts| crate::terminal::history::format_ago(now, ts));

            div()
                .h(lh)
                .flex()
                .items_center()
                .gap_1p5()
                .px_2()
                .whitespace_nowrap()
                .when(selected, |d| d.bg(theme.list_active))
                .child(div().flex_1().flex().overflow_hidden().children(spans))
                .when_some(failed, |d, code| {
                    d.child(
                        div()
                            .flex_none()
                            .text_color(theme.red)
                            .child(format!("✗ {code}")),
                    )
                })
                .when_some(ago, |d, ago| {
                    d.child(
                        div()
                            .flex_none()
                            .text_color(theme.muted_foreground)
                            .child(ago),
                    )
                })
                .into_any_element()
        };
        let rows: Vec<gpui_kit::AnyElement> = (first..first + visible).map(row).collect();

        let footer = |n: usize, label: String| {
            (n > 0).then(|| {
                div()
                    .h(lh)
                    .flex()
                    .items_center()
                    .px_2()
                    .text_color(theme.muted_foreground)
                    .child(label)
                    .into_any_element()
            })
        };
        let footer_lines = (hidden_above > 0) as usize + (hidden_below > 0) as usize;
        let line_count = visible + footer_lines;
        let menu_h = lh * (line_count as f32) + px(10.);

        let gap = px(6.);
        let grid_w = self.cell_width * (total_cols as f32);
        let menu_w = if grid_w < px(720.) { grid_w } else { px(720.) };
        let y = if place_above {
            px(GRID_PAD_Y) + lh * (srow as f32) - menu_h - gap
        } else {
            px(GRID_PAD_Y) + lh * ((srow + 1) as f32) + gap
        };

        Some(
            div()
                .absolute()
                .left(px(GRID_PAD_X))
                .top(y)
                // Same fall-through as the completion menu (#541): a bare div
                // inserts no hitbox, so a click on a history row landed on the
                // grid beneath it.
                .occlude()
                .flex()
                .flex_col()
                .py_1()
                .w(menu_w)
                .overflow_hidden()
                .bg(theme.popover)
                .border_1()
                .border_color(theme.border)
                .rounded(px(6.))
                .font_family(self.font.family.clone())
                .text_size(self.font_size)
                .text_color(theme.muted_foreground)
                .children(footer(hidden_above, format!("↑ {hidden_above} more")))
                .children(rows)
                .children(footer(hidden_below, format!("↓ {hidden_below} more"))),
        )
    }
}

#[cfg(test)]
mod consumer_tests {
    use super::*;
    use crate::terminal::view::test_support::*;
    use crate::daemon::protocol::DaemonMsg;
    use gpui_kit::{TestAppContext, point};
    mod history_scope_consumer_tests {
        include!("../history_scope_consumer_tests.rs");
    }
    mod history_search_consumer_tests {
        include!("../history_search_consumer_tests.rs");
    }
    mod history_navigation_consumer_tests {
        include!("../history_navigation_consumer_tests.rs");
    }
    mod history_last_word_consumer_tests {
        include!("../history_last_word_consumer_tests.rs");
    }
    mod history_ghost_consumer_tests {
        include!("../history_ghost_consumer_tests.rs");
    }
}
/// A history entry drawn on a one-line row, with its line breaks folded to a
/// visible stand-in.
///
/// gpui breaks text on `\n` whatever `white_space` says, so an entry carrying
/// one paints its tail over whatever sits below it. Entries do carry them: a
/// command composed in the inline editor keeps its newlines in the in-session
/// list, even though `history::append` refuses to write them to disk. Folding
/// is one char for one char so a fuzzy matcher's positions still address the
/// same characters.
pub(super) fn one_line_char(c: char) -> char {
    match c {
        '\n' => '↵',
        c if c.is_control() => ' ',
        c => c,
    }
}

pub(super) fn one_line(text: &str) -> String {
    text.chars().map(one_line_char).collect()
}

/// Split `line` into consecutive runs of matched / unmatched characters, each
/// one folded onto a single line by `one_line_char`. `positions` is ascending.
fn highlight_runs(line: &str, positions: &[usize]) -> Vec<(String, bool)> {
    let mut runs: Vec<(String, bool)> = Vec::new();
    let mut pos = positions.iter().copied().peekable();
    for (ci, ch) in line.chars().enumerate() {
        let hit = pos.next_if_eq(&ci).is_some();
        let ch = one_line_char(ch);
        match runs.last_mut() {
            Some((run, run_hit)) if *run_hit == hit => run.push(ch),
            _ => runs.push((ch.to_string(), hit)),
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_multiline_entry_draws_on_one_menu_row() {
        // A command composed in the inline editor keeps its newlines in the
        // in-session history. Left alone, gpui breaks the row's text on them
        // and the tail paints over every row below it.
        let line = "curl 'http://x' \\\n  -H 'accept: */*' \\\n  -H 'pragma: no-cache'";
        let runs = highlight_runs(line, &[0, 1, 2, 3]);
        let drawn: String = runs.iter().map(|(run, _)| run.as_str()).collect();
        assert!(
            !drawn.contains('\n'),
            "no run carries a line break: {drawn}"
        );
        assert_eq!(drawn.chars().count(), line.chars().count());
        assert_eq!(drawn.matches('↵').count(), 2);
        assert_eq!(
            runs.first().map(|(r, hit)| (r.as_str(), *hit)),
            Some(("curl", true))
        );
        assert!(runs[1..].iter().all(|(_, hit)| !hit));
    }

    #[test]
    fn highlight_runs_alternate_on_the_matched_characters() {
        let runs = highlight_runs("git status", &[0, 4, 5]);
        let shape: Vec<(&str, bool)> = runs.iter().map(|(r, h)| (r.as_str(), *h)).collect();
        assert_eq!(
            shape,
            [("g", true), ("it ", false), ("st", true), ("atus", false)]
        );
        assert!(highlight_runs("", &[]).is_empty());
    }
}

