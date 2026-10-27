//! Host-aware links and their pointer/probe lifecycle. The retained terminal view
//! schedules Host work and delivers navigation; this owner never receives it.
use std::path::{Path, PathBuf};

use parking_lot::FairMutex;
use rio_vt::crosswords::pos::{Column, Line, Pos};
use rio_vt::crosswords::{Crosswords, grid::Dimensions};

use super::link_probe::{Existence, LinkProbeCache};
use super::remote::EventProxy;
use super::search::{self, LinkRoots, LinkTarget, PathStyle};
use crate::core::config::LinkFileOpen;
use crate::ui::host_ops::HostId;

type Grid = FairMutex<Crosswords<EventProxy>>;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct HoveredLink {
    runs: Vec<(Pos, Pos)>,
    armed: bool,
}

impl HoveredLink {
    pub fn runs(&self) -> &[(Pos, Pos)] {
        &self.runs
    }
    pub fn is_armed(&self) -> bool {
        self.armed
    }
}

pub(super) enum LinkAt {
    Found(LinkTarget, Vec<(Pos, Pos)>),
    Unresolved {
        candidate: search::FileCandidate,
        pending: bool,
    },
    None,
}

enum GridLink {
    Hyperlink(String, Vec<(Pos, Pos)>),
    Text(String, Vec<Pos>, usize),
}

/// Every input that can change the meaning of a printed path. Geometry changes
/// also bump the epoch, even if no pointer event follows the resize.
#[derive(Clone, PartialEq)]
struct Location {
    host: HostId,
    cwd: Option<PathBuf>,
    local_paths: bool,
    files: bool,
}

pub(super) struct CapturedLink {
    host: HostId,
    target: LinkTarget,
}

impl CapturedLink {
    pub fn host(&self) -> HostId {
        self.host
    }
    pub fn target(&self) -> &LinkTarget {
        &self.target
    }
    pub fn path(&self) -> &Path {
        match &self.target {
            LinkTarget::File { path, .. } => path,
            LinkTarget::Url(_) => unreachable!("only file links are captured"),
        }
    }
    pub fn open_mode(&self, configured: LinkFileOpen) -> LinkFileOpen {
        LinkInteraction::file_open_mode(self.host, configured)
    }
}

pub(super) struct LinkInteraction {
    location: Option<Location>,
    epoch: u64,
    probes: LinkProbeCache,
    repo_root: Option<Option<PathBuf>>,
    root_pending: bool,
    root_wanted: bool,
    last_cell: Option<(usize, usize)>,
    modifier_down: bool,
    hovered: Option<HoveredLink>,
    menu: Option<CapturedLink>,
    menu_allowed: bool,
}

impl Default for LinkInteraction {
    fn default() -> Self {
        Self {
            location: None,
            epoch: 0,
            probes: Default::default(),
            repo_root: None,
            root_pending: false,
            last_cell: None,
            root_wanted: false,
            modifier_down: false,
            hovered: None,
            menu: None,
            menu_allowed: true,
        }
    }
}

impl LinkInteraction {
    pub fn retarget(&mut self, host: HostId, cwd: Option<PathBuf>, local_paths: bool, files: bool) {
        let location = Location {
            host,
            cwd,
            local_paths,
            files,
        };
        if self.location.as_ref() == Some(&location) {
            return;
        }
        self.location = Some(location);
        self.invalidate_geometry();
        self.repo_root = None;
        self.root_wanted = false;
        self.probes = LinkProbeCache::default();
        self.probes.retarget(host);
    }

    pub fn invalidate_geometry(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.clear_hover();
        // Retired work cannot leave a path permanently in flight.
        self.probes.cancel_pending();
        self.root_pending = false;
    }

    pub fn cached_hover(&self, col: usize, row: usize, armed: bool) -> Option<bool> {
        (self.last_cell == Some((col, row)) && self.modifier_down == armed)
            .then_some(self.hovered.is_some())
    }

    pub fn file_open_mode(host: HostId, configured: LinkFileOpen) -> LinkFileOpen {
        if host.is_local() {
            configured
        } else {
            LinkFileOpen::Internal
        }
    }

    pub fn hovered(&self) -> Option<&HoveredLink> {
        self.hovered.as_ref()
    }
    pub fn modifier_down(&self) -> bool {
        self.modifier_down
    }
    #[cfg(test)]
    pub fn last_cell(&self) -> Option<(usize, usize)> {
        self.last_cell
    }
    pub fn menu(&self) -> Option<&CapturedLink> {
        self.menu.as_ref()
    }
    pub fn menu_allowed(&self) -> bool {
        self.menu_allowed
    }
    pub fn record_menu_permission(&mut self, mouse_mode: bool, shift: bool) {
        self.menu_allowed = should_show_context_menu(mouse_mode, shift);
    }
    pub fn forget_menu(&mut self) {
        self.menu = None;
    }
    pub fn capture_menu(&mut self, result: LinkAt) {
        self.menu = match result {
            LinkAt::Found(target @ LinkTarget::File { .. }, _) => Some(CapturedLink {
                host: self.location.as_ref().expect("link location").host,
                target,
            }),
            _ => None,
        };
    }
    pub fn clear_hover(&mut self) -> bool {
        self.last_cell = None;
        self.hovered.take().is_some()
    }
    pub fn refresh_cell(&mut self, armed: bool, recompute: bool) -> Option<(usize, usize)> {
        if self.last_cell.is_none() {
            self.modifier_down = armed;
        }
        if recompute {
            self.last_cell.take()
        } else {
            self.last_cell
        }
    }

    /// Handles cheap hover cases before the adapter obtains a cwd snapshot or
    /// asks the Host registry. Returns (link present, painted hover changed).
    pub fn prepare_hover(
        &mut self,
        grid: &Grid,
        col: usize,
        row: usize,
        armed: bool,
        enabled: bool,
        alt_screen: bool,
    ) -> Option<(bool, bool)> {
        if let Some(found) = self.cached_hover(col, row, armed) {
            return Some((found, false));
        }
        if !enabled || (!armed && alt_screen) || cell_is_blank(grid, col, row) {
            self.last_cell = enabled.then_some((col, row));
            self.modifier_down = armed;
            return Some((false, self.hovered.take().is_some()));
        }
        None
    }

    pub fn resolve_hover(
        &mut self,
        grid: &Grid,
        col: usize,
        row: usize,
        armed: bool,
        include_loopback: bool,
        cached_root: impl FnOnce(HostId, &Path) -> Option<PathBuf>,
    ) -> (bool, bool) {
        self.last_cell = Some((col, row));
        self.modifier_down = armed;
        let next = match self.resolve(grid, col, row, true, include_loopback, cached_root) {
            LinkAt::Found(_, runs) => Some(HoveredLink { runs, armed }),
            _ => None,
        };
        let changed = next != self.hovered;
        self.hovered = next;
        (self.hovered.is_some(), changed)
    }

    pub fn roots(&self) -> LinkRoots {
        let Some(location) = &self.location else {
            return LinkRoots::default();
        };
        let mut dirs = Vec::new();
        if let Some(cwd) = &location.cwd {
            dirs.push(cwd.clone());
            if let Some(Some(root)) = &self.repo_root
                && root != cwd
            {
                dirs.push(root.clone());
            }
        }
        LinkRoots {
            dirs,
            local_home: location.host.is_local(),
            style: link_path_style(location.local_paths, location.cwd.as_deref()),
        }
    }

    pub fn unresolved_message(
        &self,
        candidate: &search::FileCandidate,
        pending: bool,
    ) -> Option<String> {
        use crate::ui::i18n::{L10nKey, t_fmt};
        let roots = self.roots();
        // Ordinary words and unanswered remote probes must remain silent.
        if pending || !candidate.looks_like_a_path(roots.style) {
            return None;
        }
        let rooted = candidate.is_rooted(roots.style);
        let root = if rooted {
            None
        } else {
            roots.dirs.into_iter().next()
        };
        Some(match root {
            Some(root) => t_fmt(
                L10nKey::LinkFileNotUnder,
                &[
                    ("path", &candidate.path),
                    ("dir", &root.display().to_string()),
                ],
            ),
            None if rooted => t_fmt(L10nKey::LinkFileMissing, &[("path", &candidate.path)]),
            None => t_fmt(L10nKey::LinkFileNoDirectory, &[("path", &candidate.path)]),
        })
    }

    pub fn wants_root(&self) -> bool {
        self.root_wanted && !self.root_pending && self.repo_root.is_none()
    }

    pub fn request_root(&mut self) -> Option<(u64, PathBuf)> {
        if !self.wants_root() {
            return None;
        }
        let cwd = self.location.as_ref()?.cwd.clone()?;
        self.root_pending = true;
        Some((self.epoch, cwd))
    }
    pub fn land_root(&mut self, epoch: u64, root: Option<PathBuf>) -> bool {
        if epoch != self.epoch {
            return false;
        }
        self.root_pending = false;
        self.repo_root = Some(root);
        true
    }
    pub fn take_probes(&mut self) -> (u64, Vec<PathBuf>) {
        (self.epoch, self.probes.take_wanted())
    }
    pub fn land_probes(&mut self, epoch: u64, answers: Vec<(PathBuf, Existence)>) -> bool {
        epoch == self.epoch && self.probes.land(answers)
    }

    pub fn resolve(
        &mut self,
        grid: &Grid,
        col: usize,
        row: usize,
        include_files: bool,
        include_loopback: bool,
        cached_root: impl FnOnce(HostId, &Path) -> Option<PathBuf>,
    ) -> LinkAt {
        let Some(line) = link_line_at(grid, col, row) else {
            return LinkAt::None;
        };
        let (text, points, click_idx) = match line {
            GridLink::Hyperlink(uri, runs) => return LinkAt::Found(LinkTarget::Url(uri), runs),
            GridLink::Text(text, points, click_idx) => (text, points, click_idx),
        };
        let Some(location) = &self.location else {
            return LinkAt::None;
        };
        let files = include_files && location.files;
        if files && let Some(cwd) = location.cwd.as_deref() {
            self.root_wanted = true;
            if self.repo_root.is_none() && !self.root_pending {
                if let Some(root) = cached_root(location.host, cwd) {
                    self.repo_root = Some(Some(root));
                }
            }
        }
        let roots = if files {
            self.roots()
        } else {
            LinkRoots::default()
        };
        let local = location.host.is_local();
        let mut pending = false;
        let mut probe = |path: &Path, require_file: bool| {
            let answer = if local {
                search::local_probe(path, require_file)
            } else {
                self.probes.probe(path, require_file)
            };
            pending |= matches!(answer, search::Probe::Unknown);
            answer
        };
        let link = search::link_at(&text, click_idx, &roots, files, &mut probe);
        let link = link.or_else(|| {
            include_loopback.then(|| {
                super::loopback::loopback_url_span_at(&text, click_idx).map(|(start, end, url)| {
                    search::LinkMatch {
                        start,
                        end,
                        target: LinkTarget::Url(url),
                    }
                })
            })?
        });
        match link {
            Some(link) => {
                let term = grid.lock();
                let runs = super::smart_select::row_runs(&term, &points[link.start..=link.end]);
                LinkAt::Found(link.target, runs)
            }
            None => match files
                .then(|| search::unresolved_candidate(&text, click_idx, roots.style))
                .flatten()
            {
                Some(candidate) => LinkAt::Unresolved { candidate, pending },
                None => LinkAt::None,
            },
        }
    }
}

/// Local output uses the native dialect; remote output follows the Host cwd,
/// defaulting to POSIX when the Host has not reported one. Home is never local
/// merely because the desktop process happens to run here.
pub(super) fn link_path_style(local: bool, cwd: Option<&Path>) -> PathStyle {
    if local {
        PathStyle::NATIVE
    } else {
        cwd.map_or(PathStyle::Posix, PathStyle::of_dir)
    }
}

fn grid_line(term: &Crosswords<EventProxy>, row: usize) -> Option<Line> {
    let line = Line(row as i32 - term.grid.display_offset() as i32);
    (line >= term.topmost_line() && line <= term.bottommost_line()).then_some(line)
}

pub(super) fn cell_is_blank(grid: &Grid, col: usize, row: usize) -> bool {
    let term = grid.lock();
    let Some(line) = grid_line(&term, row) else {
        return true;
    };
    if col >= term.columns() {
        return true;
    }
    let cell = &term.grid[line][Column(col)];
    cell.wide() == rio_vt::crosswords::square::Wide::Narrow && cell.c().is_whitespace()
}

fn link_line_at(grid: &Grid, col: usize, row: usize) -> Option<GridLink> {
    let term = grid.lock();
    let line = grid_line(&term, row)?;
    if col >= term.columns() {
        return None;
    }
    let click = Pos::new(line, Column(col));
    if let Some(hl) = term.cell_hyperlink(line, Column(col)) {
        let uri = hl.uri().to_string();
        if let Some((start, end)) = super::smart_select::hyperlink_run(&term, click) {
            return Some(GridLink::Hyperlink(
                uri,
                super::smart_select::grid_runs(start, end, term.columns()),
            ));
        }
    }
    let (text, points, click_idx) = super::smart_select::logical_line_at(&term, click, true)?;
    Some(GridLink::Text(text, points, click_idx))
}

/// Mouse-reporting applications own right-click unless Shift overrides them.
pub(super) fn should_show_context_menu(mouse_mode: bool, shift: bool) -> bool {
    !mouse_mode || shift
}

#[cfg(test)]
mod tests;
