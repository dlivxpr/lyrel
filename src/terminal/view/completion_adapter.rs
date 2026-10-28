//! Completion scheduling and presentation for the retained TerminalView.
//! TerminalCompletion alone owns generations, ranking, and cancellation.
//! Async results are applied against the current input and Host context.

use super::prompt::menu_layout;
use super::*;
use crate::terminal::completion::CandidateKind;
use crate::terminal::completion::state::{CompletionContext, CompletionWork, RemoteRequest};
use crate::terminal::prompt_input::layout::input_start;

impl TerminalView {
    pub(super) fn ghost_suggestion(&self) -> Option<String> {
        self.completion
            .ghost_suggestion(self.input.editor(), &self.history)
    }

    pub(super) fn completion_context(&self) -> CompletionContext {
        CompletionContext::new(self.host_id, self.cwd(), self.remote_context())
    }

    pub(super) fn complete_tab(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.history.search().is_some() {
            return;
        }
        let cwd = self
            .paths_are_local()
            .then(|| self.local_cwd().or_else(|| std::env::current_dir().ok()))
            .flatten();
        let share = cwd.is_none().then(|| self.wsl_share_cwd()).flatten();
        let remote = self.remote_ssh_cwd();
        let shell = self.shell_program();
        let context = self.completion_context();
        let work = self.completion.tab(
            self.input.editor_mut(),
            context,
            cwd.as_deref(),
            share.as_deref(),
            remote.as_deref(),
            shell.as_deref(),
            cx.global::<Config>().tab_completion,
            forward,
        );
        match work {
            CompletionWork::Handoff => self.handoff_tab_to_shell(!forward, cx),
            CompletionWork::Updated => {}
            CompletionWork::Generators {
                generation,
                scripts,
                cwd,
            } => {
                for pending in scripts {
                    let cwd = cwd.clone();
                    cx.spawn(async move |this, cx| {
                        let results =
                            cx.background_executor()
                                .spawn(async move {
                                    crate::terminal::generator::run(&pending.script, &cwd)
                                })
                                .await;
                        let _ = this.update(cx, |view, cx| {
                            let context = view.completion_context();
                            if view.completion.merge(
                                generation,
                                context,
                                view.input.editor(),
                                results,
                            ) {
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                }
            }
            CompletionWork::Remote(request) => {
                let route = crate::ui::sftp::SftpRoute::new(self.pane_id, self.workspace.clone());
                let dir = request.directory().to_string();
                cx.spawn(async move |this, cx| {
                    let listed = cx.background_spawn(async move { route.list(&dir) }).await;
                    let _ =
                        this.update(cx, |view, cx| view.remote_path_results(request, listed, cx));
                })
                .detach();
            }
        }
        self.lifecycle.show_cursor();
        cx.notify();
    }

    /// The cwd to list over the distro's `\\wsl$` share, for a pane whose
    /// filesystem is a WSL distro's: the local wsl.exe pane (tagged by its
    /// remote context) and the WSL-workspace pane (tagged by its workspace
    /// target) both report a POSIX cwd this process cannot read natively.
    pub(super) fn wsl_share_cwd(&self) -> Option<std::path::PathBuf> {
        let distro = wsl_share_distro(
            self.terminal.remote_context().as_ref(),
            self.workspace.as_ref(),
            self.host_id.is_local(),
        )?;
        let cwd = self.cwd()?;
        wsl_share_path(&distro, &cwd.to_string_lossy())
    }

    pub(super) fn remote_ssh_cwd(&self) -> Option<String> {
        let owned = match self.terminal.remote_context() {
            Some(remote) => remote.kind == crate::daemon::protocol::RemoteKind::NativeSsh,
            // A WSL workspace carries no SSH spec: there is no connection to
            // list over, and its panes complete through the `\\wsl$` share
            // instead — so only a spec-carrying (SSH) workspace claims the
            // remote-listing path.
            None => self.workspace.as_ref().is_some_and(|w| w.spec.is_some()),
        };
        if !owned {
            return None;
        }
        let cwd = self.cwd()?.to_string_lossy().into_owned();
        cwd.starts_with('/').then_some(cwd)
    }

    pub(super) fn remote_path_results(
        &mut self,
        request: RemoteRequest,
        listed: Result<Vec<crate::daemon::protocol::SftpEntry>, String>,
        cx: &mut Context<Self>,
    ) {
        let context = self.completion_context();
        let active =
            self.link_inactive_reason(cx).is_none() && self.input_inactive_reason().is_none();
        let shell = self.shell_program();
        if let Some(shift) = self.completion.remote_results(
            request,
            context,
            self.input.editor_mut(),
            active,
            listed,
            shell.as_deref(),
        ) {
            self.handoff_tab_to_shell(shift, cx);
        }
        cx.notify();
    }

    pub(super) fn close_completion(&mut self) {
        self.completion.close();
    }

    pub(super) fn completion_accept(&mut self, cx: &mut Context<Self>) {
        let shell = self.shell_program();
        self.completion
            .accept(self.input.editor_mut(), shell.as_deref());
        self.lifecycle.show_cursor();
        cx.notify();
    }

    pub(super) fn render_completion_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let items = self.completion.items();
        if items.is_empty() {
            return None;
        }
        let (srow, scol) = self.cursor_cell()?;

        const MAX_ROWS: usize = 10;
        let (total_rows, total_cols) = {
            let term = self.terminal.term.lock();
            (term.screen_lines(), term.columns())
        };
        // Anchored to where the input starts, which is a row below the prompt
        // when the prompt left it too little room.
        let (start_row, scol) = input_start(scol, total_cols.max(1));
        let srow = (srow + start_row).saturating_sub(self.input_scroll_rows());
        // How wide the menu ends up, decided before the rows so their
        // descriptions can be elided against it. A menu wider than the pane
        // has its right-hand column clipped by the pane, and the clip takes
        // the ellipsis with it — which is exactly the mid-word cut the
        // ellipsis exists to prevent. The history menu below already caps
        // itself to the grid this way.
        let grid_w = self.cell_width * (total_cols as f32);
        let menu_w = px(COMPLETION_MENU_MAX_W).min(grid_w);
        let (place_above, visible, first) = menu_layout(
            total_rows,
            srow,
            items.len(),
            self.completion.selected_index().unwrap_or(0),
            MAX_ROWS,
        );
        let hidden_above = first;
        let hidden_below = items.len() - first - visible;

        let theme = cx.theme();
        let lh = self.line_height;
        let cell = self.cell_width.as_f32();
        let row = |i: usize| {
            let cand = items[i];
            let selected = self.completion.selected_index() == Some(i);
            let icon_color = if selected {
                theme.foreground
            } else {
                theme.muted_foreground
            };
            let icon = completion_row_icon(cand.icon.as_deref(), cand.kind, icon_color);
            let label = if cand.is_dir() && !cand.text.ends_with('/') {
                format!("{}/", cand.text)
            } else {
                cand.text.clone()
            };
            let budget = description_budget(cell, label.chars().count(), menu_w.as_f32());
            div()
                .h(lh)
                .flex()
                .items_center()
                .gap_1p5()
                .px_2()
                .whitespace_nowrap()
                .when(selected, |d| {
                    d.bg(theme.list_active).text_color(theme.foreground)
                })
                .child(icon)
                .child(div().flex_shrink_0().child(label))
                .when_some(cand.description.clone(), |d, desc| {
                    d.child(
                        div()
                            .ml_2()
                            .text_color(theme.muted_foreground)
                            .child(elide(&desc, budget)),
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
        let menu_h = self.line_height * (line_count as f32) + px(10.);

        let gap = px(6.);
        // Anchored under the word being completed, then pulled back inside the
        // pane if that would hang it over the right edge.
        let anchor = px(GRID_PAD_X) + self.cell_width * (scol as f32);
        let x = anchor
            .min(px(GRID_PAD_X) + grid_w - menu_w)
            .max(px(GRID_PAD_X));
        let y = if place_above {
            px(GRID_PAD_Y) + self.line_height * (srow as f32) - menu_h - gap
        } else {
            px(GRID_PAD_Y) + self.line_height * ((srow + 1) as f32) + gap
        };

        Some(
            div()
                .absolute()
                .left(x)
                .top(y)
                // The menu has no click handlers of its own, and a handlerless
                // element inserts no hitbox — so without this the press falls
                // through to the grid: the selection there is cleared, or a
                // modified click opens whatever link lies under the row (#541).
                .occlude()
                .flex()
                .flex_col()
                .py_1()
                .min_w(px(120.).min(menu_w))
                .max_w(menu_w)
                .overflow_hidden()
                .bg(theme.popover)
                .border_1()
                .border_color(theme.border)
                .rounded(px(6.))
                .font_family(self.font.family.clone())
                .text_size(self.font_size)
                // Resting rows sit back so the selected row, which paints
                // itself in the full foreground, is the brightest line in
                // the menu — matching what the row icons already do.
                .text_color(theme.muted_foreground)
                .children(footer(hidden_above, format!("↑ {hidden_above} more")))
                .children(rows)
                .children(footer(hidden_below, format!("↓ {hidden_below} more"))),
        )
    }

    /// What the remote-completion pill should say right now, if anything:
    /// the listing's failure once it has one, "listing…" while it runs, and
    /// nothing at all the rest of the time (#585).
    pub(super) fn remote_completion_notice_text(&self) -> Option<String> {
        self.completion.notice_text()
    }

    pub(super) fn render_remote_completion_notice(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let text = self.remote_completion_notice_text()?;
        Some(Self::notice_pill(text, cx))
    }
}

#[cfg(test)]
mod consumer_tests {
    use super::*;
    use crate::daemon::protocol::DaemonMsg;
    use crate::terminal::view::test_support::*;
    use gpui_kit::TestAppContext;
    mod completion_consumer_fixture {
        include!("../completion_consumer_fixture.rs");
    }
    mod completion_keyboard_consumer_tests {
        include!("../completion_keyboard_consumer_tests.rs");
    }
    mod completion_async_consumer_tests {
        include!("../completion_async_consumer_tests.rs");
    }
}
fn completion_row_icon(
    raw: Option<&str>,
    kind: CandidateKind,
    color: gpui_kit::Hsla,
) -> gpui_kit::AnyElement {
    let slot = |child: gpui_kit::AnyElement| {
        div()
            .w(px(16.))
            .flex()
            .justify_center()
            .items_center()
            .child(child)
            .into_any_element()
    };

    if let Some(raw) = raw {
        if let Some(emoji) = fig_icon_emoji(raw) {
            return slot(
                div()
                    .text_size(px(13.))
                    .child(emoji.to_string())
                    .into_any_element(),
            );
        }
        if let Some(name) = fig_icon_glyph(raw) {
            return slot(
                Icon::new(name)
                    .size(px(15.))
                    .text_color(color)
                    .into_any_element(),
            );
        }
    }

    let name = match kind {
        CandidateKind::Command | CandidateKind::Value => IconName::SquareTerminal,
        CandidateKind::Flag => IconName::Dash,
        CandidateKind::Dir => IconName::Folder,
        CandidateKind::File => IconName::File,
    };
    slot(
        Icon::new(name)
            .size(px(15.))
            .text_color(color)
            .into_any_element(),
    )
}

fn fig_icon_emoji(raw: &str) -> Option<&str> {
    if raw.is_empty() {
        None
    } else if !raw.starts_with("fig://") {
        Some(raw)
    } else if raw.starts_with("fig://template") {
        fig_query_param(raw, "badge")
    } else {
        None
    }
}

fn fig_icon_glyph(raw: &str) -> Option<IconName> {
    let ty = raw
        .strip_prefix("fig://icon")
        .and_then(|r| fig_query_param(r, "type"))?;
    match ty {
        "folder" => Some(IconName::Folder),
        "file" => Some(IconName::File),
        "git" => Some(IconName::Github),
        "asterisk" => Some(IconName::Asterisk),
        _ => None,
    }
}

fn fig_query_param<'a>(raw: &'a str, key: &str) -> Option<&'a str> {
    raw.split_once('?')?.1.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then_some(v)
    })
}

/// Where the completion menu stops growing.
const COMPLETION_MENU_MAX_W: f32 = 480.;

/// How many characters of a candidate's description fit beside its name.
///
/// The menu is monospaced and clips whatever runs past its edge, so a long
/// description used to end mid-word — "…or a symli" — with nothing to say it
/// had been cut. The row spends its width on `px_2` either side, the kind
/// icon, the gap after it and the margin before the description; whatever is
/// left, in cells, is the description's.
///
/// `menu_w` is the width the menu actually got, not the width it would like:
/// in a pane narrower than [`COMPLETION_MENU_MAX_W`] the menu is capped to the
/// grid, and a budget measured against the larger number puts the ellipsis
/// past the edge — which is the same mid-word cut, only harder to see.
fn description_budget(cell_width: f32, label_cells: usize, menu_w: f32) -> usize {
    const ROW_CHROME: f32 = 8. + 8. + 16. + 6. + 8.;
    if cell_width <= 0. {
        return 0;
    }
    let free = menu_w - ROW_CHROME - label_cells as f32 * cell_width;
    (free / cell_width).floor().max(0.) as usize
}

/// `text` cut to `budget` characters, with the last one spent on an ellipsis.
/// A budget too small to say anything with returns nothing rather than a bare
/// "…", which reads as a description that is there but unreadable.
fn elide(text: &str, budget: usize) -> String {
    if text.chars().count() <= budget {
        return text.to_string();
    }
    if budget < 2 {
        return String::new();
    }
    let mut out: String = text.chars().take(budget - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fig_icon_emoji_takes_bare_emoji_and_template_badge_only() {
        assert_eq!(fig_icon_emoji("⚙️"), Some("⚙️"));
        assert_eq!(
            fig_icon_emoji("fig://template?color=2ecc71&badge=🔥"),
            Some("🔥")
        );
        assert_eq!(fig_icon_emoji("fig://icon?type=git"), None);
        assert_eq!(fig_icon_emoji("fig://template?color=2ecc71"), None);
        assert_eq!(fig_icon_emoji(""), None);
    }

    #[test]
    fn fig_icon_glyph_maps_known_types_and_falls_back_otherwise() {
        assert!(matches!(
            fig_icon_glyph("fig://icon?type=folder"),
            Some(IconName::Folder)
        ));
        assert!(matches!(
            fig_icon_glyph("fig://icon?type=file"),
            Some(IconName::File)
        ));
        assert!(matches!(
            fig_icon_glyph("fig://icon?type=git"),
            Some(IconName::Github)
        ));
        assert!(fig_icon_glyph("fig://icon?type=docker").is_none());
        assert!(fig_icon_glyph("⚙️").is_none());
    }
    #[test]
    fn a_description_that_fits_is_left_alone() {
        assert_eq!(elide("Show commit logs", 40), "Show commit logs");
        // Exactly the budget is still a fit; nothing is spent on an ellipsis.
        assert_eq!(elide("abcd", 4), "abcd");
    }

    #[test]
    fn an_overlong_description_ends_in_an_ellipsis_inside_its_budget() {
        let out = elide("Move or rename a file, a directory, or a symlink", 12);
        assert_eq!(out.chars().count(), 12, "the ellipsis is inside the budget");
        assert!(out.ends_with('…'));
        assert!(out.starts_with("Move or ren"));
    }

    #[test]
    fn a_budget_with_no_room_to_say_anything_says_nothing() {
        // A lone "…" reads as a description that is there but unreadable.
        assert_eq!(elide("Show commit logs", 1), "");
        assert_eq!(elide("Show commit logs", 0), "");
    }

    #[test]
    fn the_description_budget_is_what_the_name_leaves_of_the_menu() {
        const W: f32 = COMPLETION_MENU_MAX_W;
        // A 9px cell: 46px of row chrome, so a bare row leaves (480-46)/9 cells.
        assert_eq!(description_budget(9., 0, W), 48);
        // Every cell the name takes is one the description does not get.
        assert_eq!(description_budget(9., 10, W), 38);
        // A name that fills the menu on its own leaves nothing, and never
        // underflows into a huge budget.
        assert_eq!(description_budget(9., 200, W), 0);
        assert_eq!(description_budget(0., 10, W), 0);
        // A pane too narrow for the full menu shrinks the budget with it,
        // instead of eliding to a width the menu never got.
        assert!(description_budget(9., 10, 240.) < description_budget(9., 10, W));
        assert_eq!(description_budget(9., 10, 240.), 11);
    }
}
