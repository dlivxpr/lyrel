//! Find in files for the right panel's Files tab: the "In file contents"
//! half of its search, across the active tab's project, on whichever machine
//! that project lives.
//!
//! The roots are the Files tab's own ([`LyrelApp::project_roots`]), and the
//! search runs through [`Host::search_content`](lyrel_core::host::Host), so a
//! remote workspace is searched on the remote end and only the hits cross
//! the wire. Typing re-searches after a short pause; a generation counter
//! (see [`model::SearchRun`]) makes sure only the answer to the last thing
//! typed is ever drawn.

mod model;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Entity, HighlightStyle, MouseButton, SharedString, StyledText,
    Subscription, Window, div, px, rems,
};
use lyrel_core::host::{ContentHit, ContentLimits, ContentQuery};

use crate::ui::app::LyrelApp;
use crate::ui::host_ops::HostOps;
use crate::ui::host_registry::HostRegistry;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::right_panel::{META, ROW_FILL_RADIUS, ROW_GLYPH, ROW_INSET, TEXT_MONO};

use model::{FileGroup, Outcome, SearchKey, SearchRun};

/// How long typing has to pause before the search goes out. A little longer
/// than the name search's: every keystroke that does get through reads files.
const DEBOUNCE: Duration = Duration::from_millis(250);

const FILE_ROW_H: f32 = 26.;
const HIT_ROW_H: f32 = 22.;
const CHEVRON_W: f32 = 10.;
const ROW_GAP: f32 = 6.;

/// Where a hit's excerpt starts: under its file's name, past the chevron and
/// the icon.
const HIT_LEAD: f32 = ROW_INSET + CHEVRON_W + ROW_GAP + ROW_GLYPH + ROW_GAP;

pub(crate) struct PanelSearchState {
    pub(crate) input: Entity<InputState>,
    pub(crate) case_sensitive: bool,
    pub(crate) whole_word: bool,
    pub(crate) regex: bool,
    pub(crate) run: SearchRun,
    /// Files whose hits are folded away. Cleared by a new search, kept by a
    /// refresh of the same one.
    pub(crate) collapsed: HashSet<PathBuf>,
    /// The field takes focus on the next frame — set whenever the tab is
    /// brought forward, which happens where there is no `Window` to focus with.
    pub(crate) focus_pending: bool,
    _input_sub: Subscription,
    rows: Option<(Body, Rc<Vec<PanelSearchRow>>)>,
    #[cfg(test)]
    pub(crate) read_landings: usize,
    #[cfg(test)]
    pub(crate) read_landing_host: Option<usize>,
}

impl PanelSearchState {
    /// The search over `input` — the Files tab's field, which it shares
    /// with the name search there.
    pub(crate) fn new(
        input: Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
    ) -> Self {
        let sub = cx.subscribe_in(&input, window, |app, _input, ev, _window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                app.panel_search_refresh(cx)
            }
        });
        PanelSearchState {
            input,
            case_sensitive: false,
            whole_word: false,
            regex: false,
            run: SearchRun::default(),
            collapsed: HashSet::new(),
            focus_pending: false,
            _input_sub: sub,
            rows: None,
            #[cfg(test)]
            read_landings: 0,
            #[cfg(test)]
            read_landing_host: None,
        }
    }

    pub(crate) fn host_changed(&mut self) {
        self.run.retarget(None);
        self.collapsed.clear();
        self.rows = None;
    }

    fn query(&self, cx: &App) -> ContentQuery {
        ContentQuery {
            pattern: self.input.read(cx).value().to_string(),
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            regex: self.regex,
            show_hidden: false,
        }
    }
}

/// What the body of the tab says, worked out before anything is drawn.
#[derive(Clone, PartialEq)]
pub(crate) enum Body {
    /// An SSH pane's files are SFTP's, and a search cannot run there.
    SshPane,
    /// The active tab has no directory on its host.
    NoFolder,
    /// Roots are still being asked for.
    Resolving,
    /// Nothing typed yet.
    Idle(String),
    Searching,
    Results,
    NoMatches(String),
    BadPattern(String),
    ServerTooOld,
    Failed(String),
}

#[derive(Clone)]
pub(crate) enum PanelSearchRow {
    Heading(Option<String>),
    Note(Body),
    Searching,
    // Indexes locate current render data; only path and hit coordinates identify
    // a row across refreshes. One shared path per group also serves all its hits.
    File(usize, Rc<Path>),
    Hit(usize, usize, Rc<Path>, u32, u32),
    Truncated,
}

impl PartialEq for PanelSearchRow {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Heading(a), Self::Heading(b)) => a == b,
            (Self::Note(a), Self::Note(b)) => a == b,
            (Self::Searching, Self::Searching) | (Self::Truncated, Self::Truncated) => true,
            (Self::File(_, a), Self::File(_, b)) => a == b,
            (Self::Hit(_, _, a, line_a, col_a), Self::Hit(_, _, b, line_b, col_b)) => {
                a == b && line_a == line_b && col_a == col_b
            }
            _ => false,
        }
    }
}

impl LyrelApp {
    /// Enter in the field: search again now, even for the same query — the
    /// files may have changed under it.
    fn panel_search_refresh(&mut self, cx: &mut Context<Self>) {
        let Some((generation, key)) = self.panel_search.run.refresh() else {
            return;
        };
        self.panel_search.rows = None;
        self.panel_search_spawn(generation, key, Duration::ZERO, cx);
        cx.notify();
    }

    /// Bring the run in line with what the tab now asks for.
    fn panel_search_sync(&mut self, key: Option<SearchKey>, cx: &mut Context<Self>) {
        if self.panel_search.run.wanted() != key.as_ref() {
            self.panel_search.collapsed.clear();
            self.panel_search.rows = None;
        }
        if let Some(generation) = self.panel_search.run.retarget(key.clone())
            && let Some(key) = key
        {
            self.panel_search_spawn(generation, key, DEBOUNCE, cx);
        }
    }

    fn panel_search_spawn(
        &mut self,
        generation: u64,
        key: SearchKey,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |app, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let _ = app.update(cx, |app, cx| {
                if !app.panel_search.run.is_current(generation) {
                    return;
                }
                let Some(host) = HostRegistry::lookup(cx, key.host) else {
                    app.panel_search.run.accept(
                        generation,
                        Outcome::Failed(t(L10nKey::PanelSearchHostGone).to_string()),
                    );
                    app.panel_search.rows = None;
                    cx.notify();
                    return;
                };
                app.file_tree_sync_host(&host, cx);
                if !app.panel_search.run.is_current(generation) {
                    return;
                }
                #[cfg(test)]
                let landing_host = std::sync::Arc::as_ptr(&host) as *const () as usize;
                let authority = app.file_tree.read_authority(&host);
                let SearchKey { roots, query, .. } = key;
                HostOps::run(
                    host,
                    cx,
                    move |h| {
                        Outcome::from_result(h.search_content(
                            &roots,
                            &query,
                            &ContentLimits::default(),
                        ))
                    },
                    move |app, outcome, cx| {
                        #[cfg(test)]
                        {
                            app.panel_search.read_landings += 1;
                            app.panel_search.read_landing_host = Some(landing_host);
                        }
                        if !authority.is_current(&app.file_tree, cx) {
                            return;
                        }
                        if app.panel_search.run.accept(generation, outcome) {
                            app.panel_search.rows = None;
                            cx.notify();
                        }
                    },
                );
            });
        })
        .detach();
    }

    fn panel_search_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Body {
        // The Files tab hands an SSH pane to the SFTP browser; there is no
        // host here that could run a search over there.
        if self.panel_search_ssh_pane(window, cx) {
            self.panel_search_sync(None, cx);
            return Body::SshPane;
        }
        let Some((host, roots)) = self.project_roots(cx) else {
            return Body::Resolving;
        };
        if roots.is_empty() {
            self.panel_search_sync(None, cx);
            return Body::NoFolder;
        }
        let query = self.panel_search.query(cx);
        if query.pattern.is_empty() {
            self.panel_search_sync(None, cx);
            // `~/…`, the way the Info tab spells the working directory: a
            // full `/Users/…` path broke across the narrow column mid-name.
            let home = self
                .tabs
                .get(self.tabs.active_index())
                .and_then(|tab| tab.detail_pane(window, cx))
                .and_then(|leaf| leaf.read(cx).display_home(cx));
            let root = roots
                .iter()
                .map(|r| {
                    crate::ui::path_display::abbreviate_home(&r.to_string_lossy(), home.as_deref())
                        .into_owned()
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Body::Idle(root);
        }
        let pattern = query.pattern.clone();
        self.panel_search_sync(
            Some(SearchKey {
                host: host.id(),
                roots,
                query,
            }),
            cx,
        );
        let run = &self.panel_search.run;
        // Until the answer for this query arrives, the last one stays on
        // screen under a "Searching…" summary rather than blinking out on
        // every keystroke. A state with no hits to keep says "Searching…" on
        // its own. Only hits from the same host are kept: a row opens its
        // path on the active host, so another machine's hits would open the
        // wrong file (or none) there.
        let Some(landed) = run.landed().filter(|l| Some(&l.key) == run.wanted()) else {
            return match run.landed() {
                Some(l)
                    if matches!(l.outcome, Outcome::Found(_))
                        && !l.groups.is_empty()
                        && run.wanted().is_some_and(|w| w.host == l.key.host) =>
                {
                    Body::Results
                }
                _ => Body::Searching,
            };
        };
        match &landed.outcome {
            Outcome::Found(_) if landed.groups.is_empty() => Body::NoMatches(pattern),
            Outcome::Found(_) => Body::Results,
            Outcome::BadPattern(e) => Body::BadPattern(e.clone()),
            Outcome::ServerTooOld => Body::ServerTooOld,
            Outcome::Failed(e) => Body::Failed(e.clone()),
        }
    }

    fn panel_search_ssh_pane(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        use crate::daemon::protocol::RemoteKind;
        let Some(leaf) = self
            .tabs
            .get(self.tabs.active_index())
            .and_then(|tab| tab.detail_pane(window, cx))
        else {
            return false;
        };
        leaf.read(cx)
            .remote_context()
            .is_some_and(|remote| remote.kind == RemoteKind::NativeSsh)
    }

    /// The Files tab's "In file contents" section, for what its field holds:
    /// a heading with the tally, then either the hits grouped by file or the
    /// one line that says why there are none. Empty while nothing is typed —
    /// the tree is showing then.
    pub(crate) fn panel_search_section(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Rc<Vec<PanelSearchRow>> {
        let body = self.panel_search_body(window, cx);
        if let Some((held, rows)) = &self.panel_search.rows
            && *held == body
        {
            return Rc::clone(rows);
        }
        let mut rows = Vec::new();
        if !matches!(body, Body::Idle(_)) {
            rows.push(PanelSearchRow::Heading(self.panel_search_tally(&body)));
            if matches!(body, Body::Results) {
                if self.panel_search.run.running() {
                    rows.push(PanelSearchRow::Searching);
                }
                if let Some(landed) = self.panel_search.run.landed() {
                    for (file, group) in landed.groups.iter().enumerate() {
                        let path: Rc<Path> = Rc::from(group.path.as_path());
                        rows.push(PanelSearchRow::File(file, Rc::clone(&path)));
                        if !self.panel_search.collapsed.contains(&group.path) {
                            rows.extend(group.hits.iter().enumerate().map(|(n, hit)| {
                                PanelSearchRow::Hit(file, n, Rc::clone(&path), hit.line, hit.column)
                            }));
                        }
                    }
                    if matches!(&landed.outcome, Outcome::Found(found) if found.truncated) {
                        rows.push(PanelSearchRow::Truncated);
                    }
                }
            } else {
                rows.push(PanelSearchRow::Note(body.clone()));
            }
        }
        let rows = Rc::new(rows);
        self.panel_search.rows = Some((body, Rc::clone(&rows)));
        rows
    }

    pub(crate) fn render_panel_search_row(
        &self,
        row: &PanelSearchRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match row {
            PanelSearchRow::Heading(tally) => crate::ui::file_tree::search_section_heading(
                t(L10nKey::PanelSearchInContents),
                tally.clone(),
                cx,
            ),
            PanelSearchRow::Note(body) => v_flex()
                .children(self.panel_search_rows(body.clone(), cx))
                .into_any_element(),
            PanelSearchRow::Searching => div()
                .px(px(ROW_INSET))
                .pb(px(4.))
                .text_size(rems(META))
                .text_color(cx.theme().muted_foreground)
                .child(t(L10nKey::PanelSearchSearching))
                .into_any_element(),
            PanelSearchRow::Truncated => div()
                .px(px(ROW_INSET))
                .pt(px(6.))
                .text_size(rems(META))
                .italic()
                .text_color(cx.theme().muted_foreground)
                .child(t(L10nKey::PanelSearchTruncated))
                .into_any_element(),
            PanelSearchRow::File(index, _) => {
                let group = &self
                    .panel_search
                    .run
                    .landed()
                    .expect("presented search result")
                    .groups[*index];
                self.panel_search_file_row(
                    group,
                    self.panel_search.collapsed.contains(&group.path),
                    cx,
                )
            }
            PanelSearchRow::Hit(file, n, ..) => {
                let hit = &self
                    .panel_search
                    .run
                    .landed()
                    .expect("presented search result")
                    .groups[*file]
                    .hits[*n];
                self.panel_search_hit_row(hit, cx)
            }
        }
    }

    /// The heading's trailing count: hits, once there are some to count.
    fn panel_search_tally(&self, body: &Body) -> Option<String> {
        let landed = self.panel_search.run.landed()?;
        match (body, &landed.outcome) {
            (Body::Results, Outcome::Found(_)) => {
                Some(model::hit_count(&landed.groups).to_string())
            }
            _ => None,
        }
    }

    pub(crate) fn panel_search_toggles(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.panel_search;
        let muted = cx.theme().muted_foreground;
        let toggle = |id: &'static str,
                      label: &'static str,
                      tip: L10nKey,
                      on: bool,
                      flip: fn(&mut PanelSearchState)| {
            Button::new(id)
                .label(label)
                .ghost()
                .xsmall()
                .selected(on)
                // Off reads as a hint of what can be turned on, not as three
                // words in body ink beside the query.
                .when(!on, |b| b.text_color(muted))
                // Read out by name, not as the glyphs drawn ("A a", "dot star").
                .accessibility_label(t(tip))
                .tooltip(t(tip))
                .on_click(cx.listener(move |this, _, _window, cx| {
                    flip(&mut this.panel_search);
                    cx.notify();
                }))
        };
        h_flex()
            .flex_none()
            .gap(px(2.))
            .child(toggle(
                "panel-search-case",
                "Aa",
                L10nKey::SearchMatchCase,
                state.case_sensitive,
                |s| s.case_sensitive = !s.case_sensitive,
            ))
            .child(toggle(
                "panel-search-word",
                "ab",
                L10nKey::PanelSearchWholeWord,
                state.whole_word,
                |s| s.whole_word = !s.whole_word,
            ))
            .child(toggle(
                "panel-search-regex",
                ".*",
                L10nKey::SearchUseRegex,
                state.regex,
                |s| s.regex = !s.regex,
            ))
            .into_any_element()
    }

    fn panel_search_rows(&self, body: Body, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let danger = cx.theme().danger;
        let note = |text: String, hint: Option<String>, ink: Option<gpui_kit::Hsla>, cx: &App| {
            let muted = cx.theme().muted_foreground;
            v_flex()
                .flex_none()
                .px(px(ROW_INSET))
                .py(px(4.))
                .gap(px(3.))
                .text_size(rems(crate::ui::right_panel::TEXT))
                .text_color(ink.unwrap_or(muted))
                .child(text)
                .children(hint.map(|h| div().text_size(rems(META)).text_color(muted).child(h)))
                .into_any_element()
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        match body {
            Body::SshPane => rows.push(note(
                t(L10nKey::PanelSearchSshPane).into(),
                Some(t(L10nKey::PanelSearchSshPaneHint).into()),
                None,
                cx,
            )),
            Body::NoFolder => rows.push(note(
                t(L10nKey::PanelSearchNoFolder).into(),
                Some(t(L10nKey::PanelSearchNoFolderHint).into()),
                None,
                cx,
            )),
            Body::Resolving | Body::Searching => rows.push(note(
                t(L10nKey::PanelSearchSearching).into(),
                None,
                None,
                cx,
            )),
            // The path on a line of its own: run into the sentence, a narrow
            // column broke it at a slash, mid-name.
            Body::Idle(root) => rows.push(note(
                t(L10nKey::PanelSearchIdle).into(),
                Some(root),
                None,
                cx,
            )),
            Body::NoMatches(query) => rows.push(note(
                t_fmt(L10nKey::PanelSearchNoMatches, &[("query", &query)]),
                None,
                None,
                cx,
            )),
            Body::BadPattern(e) => rows.push(note(
                t_fmt(L10nKey::PanelSearchBadPattern, &[("e", first_line(&e))]),
                None,
                Some(danger),
                cx,
            )),
            Body::ServerTooOld => rows.push(note(
                t(L10nKey::PanelSearchServerTooOld).into(),
                Some(t(L10nKey::PanelSearchServerTooOldHint).into()),
                Some(danger),
                cx,
            )),
            Body::Failed(e) => rows.push(note(
                t_fmt(L10nKey::PanelSearchFailed, &[("e", &e)]),
                None,
                Some(danger),
                cx,
            )),
            Body::Results => unreachable!("result rows use the viewport renderer"),
        }
        rows
    }

    fn panel_search_file_row(
        &self,
        group: &FileGroup,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sf = cx.global::<crate::ui::presets::Surfaces>().popover;
        let muted = cx.theme().muted_foreground;
        let path = group.path.clone();
        h_flex()
            .id(SharedString::from(format!(
                "panel-search-file-{}",
                group.path.display()
            )))
            .test_support()
            .flex_none()
            .items_center()
            .gap(px(ROW_GAP))
            .h(px(FILE_ROW_H))
            .px(px(ROW_INSET))
            .rounded(ROW_FILL_RADIUS)
            .cursor_pointer()
            .hover(|s| s.bg(gpui_kit::rgb(sf.hover)))
            .child(
                div()
                    .flex_none()
                    .w(px(CHEVRON_W))
                    .flex()
                    .justify_center()
                    .child(
                        Icon::new(match collapsed {
                            true => IconName::ChevronRight,
                            false => IconName::ChevronDown,
                        })
                        .size(px(CHEVRON_W))
                        .text_color(muted),
                    ),
            )
            .child(
                Icon::new(IconName::File)
                    .size(px(ROW_GLYPH))
                    .text_color(muted),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_baseline()
                    .gap(px(6.))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_none()
                            .text_sm()
                            .text_color(cx.theme().sidebar_foreground)
                            .child(SharedString::from(group.name.clone())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(rems(META))
                            .text_color(muted)
                            .child(SharedString::from(group.dir.clone())),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(5.))
                    .rounded(px(4.))
                    .bg(cx.theme().muted)
                    .text_size(rems(crate::ui::right_panel::META_MONO))
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(muted)
                    .child(group.hits.len().to_string()),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    let folded = &mut this.panel_search.collapsed;
                    if !folded.remove(&path) {
                        folded.insert(path.clone());
                    }
                    this.panel_search.rows = None;
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    fn panel_search_hit_row(&self, hit: &ContentHit, cx: &mut Context<Self>) -> AnyElement {
        let sf = cx.global::<crate::ui::presets::Surfaces>().popover;
        let mark = HighlightStyle {
            background_color: Some(cx.theme().yellow.opacity(0.35)),
            ..Default::default()
        };
        let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = hit
            .ranges
            .iter()
            .filter(|r| {
                (r.end as usize) <= hit.text.len()
                    && hit.text.is_char_boundary(r.start as usize)
                    && hit.text.is_char_boundary(r.end as usize)
                    && r.start < r.end
            })
            .map(|r| (r.start as usize..r.end as usize, mark))
            .collect();
        let text =
            StyledText::new(SharedString::from(hit.text.clone())).with_highlights(highlights);
        let (path, line, column) = (hit.path.clone(), hit.line, hit.column);
        let tip = t_fmt(
            L10nKey::PanelSearchLineTooltip,
            &[("line", &line.to_string()), ("column", &column.to_string())],
        );
        div()
            .id(SharedString::from(format!(
                "panel-search-hit-{}-{}-{}",
                hit.path.display(),
                hit.line,
                hit.column
            )))
            .test_support()
            .flex_none()
            .flex()
            .items_center()
            .h(px(HIT_ROW_H))
            .pl(px(HIT_LEAD))
            .pr(px(ROW_INSET))
            .rounded(ROW_FILL_RADIUS)
            .cursor_pointer()
            .hover(|s| s.bg(gpui_kit::rgb(sf.hover)))
            .overflow_hidden()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(rems(TEXT_MONO))
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(cx.theme().sidebar_foreground)
                    .child(text),
            )
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.open_linked_file(&path, Some(line), Some(column), false, window, cx);
                }),
            )
            .into_any_element()
    }
}

/// A regex parser's error is a small diagram over several lines; the panel
/// has room for its verdict, which is the last one.
fn first_line(e: &str) -> &str {
    e.lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(e)
}

#[cfg(test)]
mod gpui_tests {
    use super::*;
    use crate::core::config::RightPanelTab;
    use crate::daemon::protocol::DaemonMsg;
    use crate::ui::app::{render_probe, test_window};
    use gpui_kit::{Focusable as _, TestAppContext, VisualTestContext};
    use std::path::Path;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lyrel-psearch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    fn search_panel_on(
        cx: &mut TestAppContext,
        root: &Path,
    ) -> (
        Entity<LyrelApp>,
        VisualTestContext,
        crate::daemon::transport::Stream,
    ) {
        let (app, mut vcx, mut pane) = test_window::harness_with_pane(cx);
        DaemonMsg::Cwd(root.to_path_buf())
            .encode(&mut pane)
            .expect("the pane's socket takes the cwd");
        app.update_in(&mut vcx, |app, _, cx| {
            app.set_right_panel_tab(RightPanelTab::Search, cx);
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            vcx.background_executor.run_until_parked();
            let rooted = app.update_in(&mut vcx, |app, _, cx| {
                app.project_roots(cx).map(|(_, roots)| roots) == Some(vec![root.to_path_buf()])
            });
            if rooted {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the pane never reported its cwd"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        (app, vcx, pane)
    }

    fn type_query(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext, text: &str) {
        app.update_in(vcx, |app, window, cx| {
            app.panel_search
                .input
                .update(cx, |st, cx| st.set_value(text, window, cx));
            cx.notify();
        });
    }

    /// Drives frames and the debounce until the search asked for last has an
    /// answer on screen. The host call runs on a real thread, so real time
    /// has to pass too.
    fn landed(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> Outcome {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            app.update_in(vcx, |_, _, cx| cx.notify());
            vcx.executor().advance_clock(DEBOUNCE);
            vcx.background_executor.run_until_parked();
            let got = app.update_in(vcx, |app, _, _| {
                let run = &app.panel_search.run;
                let wanted = run.wanted().cloned();
                run.landed()
                    .filter(|l| !run.running() && Some(&l.key) == wanted.as_ref())
                    .map(|l| l.outcome.clone())
            });
            if let Some(outcome) = got {
                return outcome;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the search never landed"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn groups(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> Vec<(String, Vec<u32>)> {
        app.update_in(vcx, |app, _, _| {
            app.panel_search
                .run
                .landed()
                .map(|l| {
                    l.groups
                        .iter()
                        .map(|g| {
                            let rel = match g.dir.is_empty() {
                                true => g.name.clone(),
                                false => format!("{}/{}", g.dir, g.name),
                            };
                            (rel, g.hits.iter().map(|h| h.line).collect())
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    #[gpui_kit::test]
    fn opening_the_tab_puts_the_cursor_in_the_field(cx: &mut TestAppContext) {
        let _serial = serial();
        let root = scratch("focus");
        let (app, mut vcx, _pane) = search_panel_on(cx, &root);
        app.update_in(&mut vcx, |_, _, cx| cx.notify());
        vcx.background_executor.run_until_parked();
        let focused = app.update_in(&mut vcx, |app, window, cx| {
            app.panel_search
                .input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        });
        assert!(focused, "the Search tab opened without focusing its field");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[gpui_kit::test]
    fn scrolled_content_groups_fold_and_open_the_intended_file_below_name_matches(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::test::TestWindowExt;
        let _serial = serial();
        let temp =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let root = temp.path();
        for n in 0..120 {
            std::fs::write(
                root.join(format!("needle-{n:03}.txt")),
                "needle in content\n",
            )
            .unwrap();
        }
        let target = root.join("needle-119.txt");
        // LocalHost stores canonical paths in the normal local spelling, not
        // std::fs::canonicalize's Windows verbatim-prefix spelling.
        let expected_target = lyrel_core::core::path_spelling::local_spelling_buf(
            std::fs::canonicalize(&target).unwrap(),
        );
        let (app, mut vcx, _pane) = search_panel_on(cx, root);
        vcx.update(|window, cx| window.input("needle", cx));
        assert!(matches!(landed(&app, &mut vcx), Outcome::Found(_)));
        assert_eq!(groups(&app, &mut vcx).len(), 120);
        let first = target.with_file_name("needle-000.txt");
        let first: SharedString = format!("tree-{}", first.display()).into();
        let last: SharedString = format!("tree-{}", target.display()).into();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            vcx.background_executor.run_until_parked();
            let names_visible = vcx.update(|window, cx| {
                window.render_frame(cx);
                window.try_find(first.clone()).is_some() || window.try_find(last.clone()).is_some()
            });
            if names_visible {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "file-name results never appeared"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        vcx.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find(first).is_some_and(|row| row.visible()),
                "the first file-name match must stay visible when asynchronous results arrive"
            );
        });
        let group: SharedString = format!("panel-search-file-{}", target.display()).into();
        let hit: SharedString = format!("panel-search-hit-{}-1-1", target.display()).into();
        vcx.update(|window, cx| {
            window.render_frame(cx);
            window.scroll(
                "right-panel-tree-rows",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-100_000.))),
                cx,
            );
            window.render_frame(cx);
            assert!(window.find(hit.clone()).visible());
            window.click(group.clone(), cx);
            assert!(window.try_find(hit.clone()).is_none());
            window.click(group, cx);
            window.click(hit, cx);
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            vcx.background_executor.run_until_parked();
            if app.read_with(&vcx, |app, _| {
                app.editor_active_location().is_some_and(|(host, path)| {
                    host == crate::ui::host_ops::HostId::LOCAL && path == expected_target
                })
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the search hit opened the wrong file: expected {expected_target:?}, got {:?}",
                app.read_with(&vcx, |app, _| {
                    app.editor_active_location()
                        .map(|(host, path)| (host, path.to_path_buf()))
                })
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[gpui_kit::test]
    fn same_query_refresh_preserves_scrolled_content_identity_after_preceding_insertions(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::test::TestWindowExt;
        let _serial = serial();
        let temp =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let root = temp.path();
        for n in 0..100 {
            std::fs::write(
                root.join(format!("file-{n:03}.txt")),
                "plain first line\n  needle original\nneedle trailing\n",
            )
            .unwrap();
        }
        let target = root.join("file-060.txt");
        let expected_target = lyrel_core::core::path_spelling::local_spelling_buf(
            std::fs::canonicalize(&target).unwrap(),
        );
        let hit: SharedString = format!("panel-search-hit-{}-2-3", target.display()).into();
        let group: SharedString = format!("panel-search-file-{}", target.display()).into();
        let (app, mut vcx, _pane) = search_panel_on(cx, root);
        vcx.update(|window, cx| window.input("needle", cx));
        assert!(matches!(landed(&app, &mut vcx), Outcome::Found(_)));
        assert_eq!(groups(&app, &mut vcx).len(), 100);

        // Reach a real, non-terminal viewport through the list's wheel handler.
        // Its first row must be the domain object under test, not a surviving
        // header or neighbour that could accidentally preserve its position.
        let align_at_top = |vcx: &mut VisualTestContext, row: SharedString| {
            for _ in 0..100 {
                let visible = vcx.update(|window, cx| {
                    window.render_frame(cx);
                    if window
                        .try_find(row.clone())
                        .is_some_and(|row| row.visible())
                    {
                        return true;
                    }
                    window.scroll(
                        "right-panel-tree-rows",
                        gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-200.))),
                        cx,
                    );
                    false
                });
                if visible {
                    break;
                }
            }
            vcx.update(|window, cx| {
                window.render_frame(cx);
                let top = window.find("right-panel-tree-rows").bounds().origin.y;
                let before = window
                    .try_find(row.clone())
                    .expect("initial wheel scrolling must reach the intended content row")
                    .bounds()
                    .origin
                    .y;
                window.scroll(
                    "right-panel-tree-rows",
                    gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), top - before)),
                    cx,
                );
                window.render_frame(cx);
                let target = window
                    .try_find(row)
                    .expect("initial alignment must retain the intended content row")
                    .bounds();
                assert_eq!(
                    target.origin.y, top,
                    "the intended row owns the scroll anchor before refresh"
                );
                target.origin.y
            })
        };
        let hit_y = align_at_top(&mut vcx, hit.clone());

        // Add a hit before line 2 without moving its path/line/column. Its
        // mutable hit index changes, but the reader must stay at the same hit.
        let refreshed = "needle inserted\n  needle refreshed\nneedle trailing\n";
        std::fs::write(&target, refreshed).unwrap();
        vcx.update(|window, cx| window.press("enter", cx));
        assert!(matches!(landed(&app, &mut vcx), Outcome::Found(_)));
        assert_eq!(groups(&app, &mut vcx)[60].1, vec![1, 2, 3]);
        vcx.update(|window, cx| {
            window.render_frame(cx);
            let row = window
                .try_find(hit.clone())
                .expect("hit insertion refresh lost the previously visible path/line/column");
            assert!(row.visible(), "hit insertion refresh hid the scroll anchor");
            assert_eq!(
                row.bounds().origin.y,
                hit_y,
                "hit insertion moved the scroll anchor"
            );
        });

        // A new preceding file changes file indexes too. Exercise the file
        // header anchor separately before opening its existing line-2 hit.
        vcx.update(|window, cx| {
            window.scroll(
                "right-panel-tree-rows",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(200.))),
                cx,
            );
        });
        let group_y = align_at_top(&mut vcx, group.clone());
        std::fs::write(root.join("before-all.txt"), "needle new file\n").unwrap();
        vcx.update(|window, cx| window.press("enter", cx));
        assert!(matches!(landed(&app, &mut vcx), Outcome::Found(_)));
        assert_eq!(groups(&app, &mut vcx).len(), 101);
        vcx.update(|window, cx| {
            window.render_frame(cx);
            let row = window
                .try_find(group)
                .expect("file insertion refresh lost the previously visible file header");
            assert!(
                row.visible(),
                "file insertion refresh hid the scroll anchor"
            );
            assert_eq!(
                row.bounds().origin.y,
                group_y,
                "file insertion moved the scroll anchor"
            );
            window.click(hit, cx);
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            vcx.background_executor.run_until_parked();
            if app.read_with(&vcx, |app, _| {
                app.editor_active_location().is_some_and(|(host, path)| {
                    host == crate::ui::host_ops::HostId::LOCAL && path == expected_target
                })
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the refreshed hit opened the wrong file"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        app.read_with(&vcx, |app, cx| {
            let file = app
                .editor
                .local_file(app.tab_code().unwrap().active_id().unwrap())
                .expect("the hit opened a local code editor");
            let input = file.input.read(cx);
            let cursor = input.cursor_position();
            assert_eq!((cursor.line, cursor.character), (1, 2));
            assert_eq!(input.text().to_string(), refreshed);
        });
    }

    #[gpui_kit::test]
    fn a_query_lands_as_hits_grouped_by_file(cx: &mut TestAppContext) {
        let _serial = serial();
        let root = scratch("grouped");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "fn needle() {}\nlet x = 1;\n// Needle\n",
        )
        .unwrap();
        std::fs::write(root.join("notes.md"), "a needle here\n").unwrap();
        std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::write(root.join("build/out.rs"), "needle\n").unwrap();
        let (app, mut vcx, _pane) = search_panel_on(cx, &root);

        type_query(&app, &mut vcx, "needle");
        assert!(matches!(landed(&app, &mut vcx), Outcome::Found(_)));
        assert_eq!(
            groups(&app, &mut vcx),
            vec![
                ("notes.md".to_string(), vec![1]),
                ("src/lib.rs".to_string(), vec![1, 3]),
            ],
            "sorted walk, gitignored build/ left out, case ignored"
        );

        // A toggle is part of the query: flipping it searches again.
        app.update_in(&mut vcx, |app, _, cx| {
            app.panel_search.case_sensitive = true;
            cx.notify();
        });
        landed(&app, &mut vcx);
        assert_eq!(
            groups(&app, &mut vcx),
            vec![
                ("notes.md".to_string(), vec![1]),
                ("src/lib.rs".to_string(), vec![1]),
            ]
        );

        type_query(&app, &mut vcx, "nothing-like-this");
        match landed(&app, &mut vcx) {
            Outcome::Found(f) => assert!(f.hits.is_empty()),
            other => panic!("{other:?}"),
        }

        app.update_in(&mut vcx, |app, _, cx| {
            app.panel_search.regex = true;
            cx.notify();
        });
        type_query(&app, &mut vcx, "(");
        assert!(matches!(landed(&app, &mut vcx), Outcome::BadPattern(_)));

        type_query(&app, &mut vcx, "");
        app.update_in(&mut vcx, |_, _, cx| cx.notify());
        vcx.background_executor.run_until_parked();
        assert!(
            app.update_in(&mut vcx, |app, _, _| app
                .panel_search
                .run
                .landed()
                .is_none()),
            "an emptied field clears the results"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[gpui_kit::test]
    fn a_settled_search_panel_reaches_render_idle(cx: &mut TestAppContext) {
        let _serial = serial();
        let root = scratch("idle");
        for n in 0..8 {
            std::fs::write(root.join(format!("f{n}.rs")), "needle\n").unwrap();
        }
        let (app, mut vcx, _pane) = search_panel_on(cx, &root);
        type_query(&app, &mut vcx, "needle");
        landed(&app, &mut vcx);
        assert_eq!(groups(&app, &mut vcx).len(), 8);

        // A focused field blinks its caret, which is a frame every half
        // second by design; what is measured is the panel, so the pane gets
        // focus back first.
        app.update_in(&mut vcx, |app, window, cx| app.focus_active(window, cx));
        test_window::quiesce(&mut vcx, Some(&root));
        render_probe::arm(200);
        vcx.executor()
            .advance_clock(std::time::Duration::from_secs(9));
        vcx.background_executor.run_until_parked();
        assert_eq!(render_probe::draws(), 0, "a finished search kept drawing");
        let _ = std::fs::remove_dir_all(&root);
    }
}
