//! Stable semantic rows and row-scoped image leases, separate from atomic holds.
//! Absolute rows belong to the GUI mirror; the daemon remains authoritative.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use rio_vt::crosswords::Crosswords;
use rio_vt::crosswords::grid::{Dimensions, Scroll};
use rio_vt::crosswords::pos::{Column, Line};
use rio_vt::crosswords::square::{CellFlags, Hyperlink, Wide};
use rio_vt::crosswords::style::Style;
use rio_vt::event::EventListener;

use super::images::{CommittedImages, ImageStore};

mod relocation;

/// Decoded values, never Square side-table IDs or theme-resolved colours.
#[derive(Clone)]
pub(super) struct ReadingCell {
    pub(super) c: char,
    pub(super) style: Style,
    pub(super) marks: Option<Box<[char]>>,
    pub(super) hyperlink: Option<Hyperlink>,
    pub(super) wide: Wide,
    pub(super) wrapped: bool,
    pub(super) tab_stop: bool,
}

struct ReadingRow {
    cells: Vec<ReadingCell>,
    images: CommittedImages,
    captured_live_start: i64,
}

#[derive(PartialEq, Eq)]
struct ReadingNeeds {
    top: i64,
    oldest: i64,
    rows: usize,
    cols: usize,
    selected: Option<std::ops::Range<usize>>,
}

pub(super) struct ReadingView {
    identity: u64,
    top: i64,
    captured_live_start: i64,
    cols: usize,
    captured_rows: usize,
    rows: BTreeMap<i64, ReadingRow>,
    needs: Option<ReadingNeeds>,
    visible: std::ops::Range<i64>,
    revision: u64,
    relocation: Option<relocation::Relocation>,
}

pub(super) fn live_start<T: EventListener>(term: &Crosswords<T>) -> i64 {
    term.grid.lines_evicted() as i64 + term.grid.history_size() as i64
}

impl ReadingView {
    pub(super) fn capture<T: EventListener>(
        term: &Crosswords<T>,
        images: &ImageStore,
        presentation: &super::presentation::ParsePresentation,
    ) -> Self {
        let live = live_start(term);
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let mut reading = Self {
            identity: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            top: live - term.display_offset() as i64,
            captured_live_start: live,
            cols: term.columns(),
            captured_rows: term.screen_lines(),
            rows: BTreeMap::new(),
            needs: None,
            visible: live - term.display_offset() as i64
                ..live - term.display_offset() as i64 + term.screen_lines() as i64,
            revision: 0,
            relocation: None,
        };
        // Even a one-row scroll exposes mutable active-screen rows. Capture
        // the entire active screen once, not only the currently visible part.
        reading.retain_rows(
            term,
            images,
            presentation,
            live..live + term.screen_lines() as i64,
        );
        reading.retain_visible(term, images, presentation);
        reading
    }

    pub(super) fn prepare_resize<T: EventListener>(
        &mut self,
        term: &Crosswords<T>,
        terminal: &super::remote::RemoteTerminal,
        cols: usize,
        rows: usize,
    ) {
        let revision = terminal.presentation_facts().resize_revision;
        let images_known = terminal.images().layout_known();
        if let Some(pending) = self.relocation.as_mut() {
            pending.retarget((cols, rows), revision, images_known);
        } else {
            self.relocation = Some(relocation::Relocation::capture(
                self, term, (cols, rows), revision, images_known,
            ));
        }
    }

    pub(super) fn relocating(&self) -> bool {
        self.relocation.is_some()
    }

    pub(super) fn relocation_scroll<T: EventListener>(&mut self, term: &Crosswords<T>) -> bool {
        if let Some(pending) = self.relocation.as_mut() {
            pending.did_scroll(term);
            true
        } else {
            false
        }
    }

    /// Publish new-width semantics and row leases together, only after the VT
    /// Size and the daemon's ordered graphics Screen have both arrived.
    pub(super) fn finish_resize<T: EventListener>(
        &mut self,
        term: &mut Crosswords<T>,
        terminal: &super::remote::RemoteTerminal,
    ) -> bool {
        let Some(pending) = self.relocation.as_ref() else { return false; };
        let facts = terminal.presentation_facts();
        if !pending.ready(term, facts.resize_revision, facts.resize_images_revision) {
            return false;
        }
        drop(facts);
        let top = pending.locate(term);
        let delta = (live_start(term) - top) as i32 - term.display_offset() as i32;
        term.scroll_display(Scroll::Delta(delta));
        *self = Self::capture(term, &terminal.images(), &terminal.presentation_facts());
        true
    }

    fn retain_rows<T: EventListener>(
        &mut self,
        term: &Crosswords<T>,
        images: &ImageStore,
        presentation: &super::presentation::ParsePresentation,
        requested: std::ops::Range<i64>,
    ) {
        let origin = live_start(term);
        let mut captured = Vec::new();
        for row in requested {
            let std::collections::btree_map::Entry::Vacant(entry) = self.rows.entry(row) else {
                continue;
            };
            let line = Line((row - origin) as i32);
            let cells = term.grid[line][..]
                .iter()
                .enumerate()
                .map(|(col, cell)| {
                    let extras = cell
                        .extras_id_checked()
                        .and_then(|id| term.grid.extras_table.get(id));
                    ReadingCell {
                        c: cell.c(),
                        style: term.grid.style_of(cell),
                        marks: extras
                            .map(|extra| extra.zerowidth.as_slice())
                            .filter(|marks| !marks.is_empty())
                            .map(Box::from),
                        hyperlink: extras.and_then(|extra| extra.hyperlink.clone()),
                        wide: cell.wide(),
                        wrapped: cell.contains_cell_flag(CellFlags::WRAPLINE),
                        tab_stop: presentation.is_tab_stop(col),
                    }
                })
                .collect();
            captured.push((
                line.0,
                lyrel_core::core::kitty_graphics::placeholder_row(&term.grid, line),
            ));
            entry.insert(ReadingRow {
                cells,
                images: CommittedImages::default(),
                captured_live_start: origin,
            });
        }
        if captured.is_empty() {
            return;
        }
        self.revision = self.revision.wrapping_add(1);
        for (line, leases) in images.commit_reading_rows(origin, &captured) {
            self.rows
                .get_mut(&(origin + i64::from(line)))
                .unwrap()
                .images = leases;
        }
    }

    fn retain_visible<T: EventListener>(
        &mut self,
        term: &Crosswords<T>,
        images: &ImageStore,
        presentation: &super::presentation::ParsePresentation,
    ) {
        let oldest = term.grid.lines_evicted() as i64;
        let end = live_start(term) + term.screen_lines() as i64;
        let joins = |row: i64| {
            let frozen = self.absolute_row(row);
            let following = self.absolute_row(row + 1);
            let line = Line((row - live_start(term)) as i32);
            let wrapped = frozen.and_then(|row| row.last()).map_or_else(
                || {
                    term.grid[line][Column(term.columns() - 1)]
                        .contains_cell_flag(CellFlags::WRAPLINE)
                },
                |cell| cell.wrapped,
            );
            let last = frozen.and_then(|row| row.last()).map_or_else(
                || term.grid[line][Column(term.columns() - 1)].c(),
                |cell| cell.c,
            );
            let next = following
                .and_then(|row| row.first())
                .map_or_else(|| term.grid[Line(line.0 + 1)][Column(0)].c(), |cell| cell.c);
            wrapped
                || (super::search::is_url_char(last)
                    && super::search::is_url_char(next)
                    && next != '@')
        };
        let mut first = (self.top - 1).max(oldest);
        let mut last = (self.top + term.screen_lines() as i64).min(end);
        while first > oldest && self.top - first < 32 && joins(first - 1) {
            first -= 1;
        }
        while last < end && last - self.top < term.screen_lines() as i64 + 32 && joins(last - 1) {
            last += 1;
        }
        self.visible = first..last;
        self.retain_rows(term, images, presentation, first..last);
    }

    pub(super) fn row(&self, viewport_row: i32) -> Option<&[ReadingCell]> {
        self.rows
            .get(&(self.top + viewport_row as i64))
            .map(|row| row.cells.as_slice())
    }

    pub(super) fn commit_images<T: EventListener>(&self, term: &Crosswords<T>) -> CommittedImages {
        let mut images = Vec::new();
        for (row, retained) in self
            .rows
            .range(self.top - 1..self.top + term.screen_lines() as i64)
        {
            retained.images.append_reading(
                &mut images,
                (row - self.top) as i32,
                live_start(term) - retained.captured_live_start,
            );
        }
        images.sort_unstable_by_key(|image| {
            (
                image.placement.z,
                image.placement.image_id,
                image.placement.key,
            )
        });
        CommittedImages::new(images)
    }

    pub(super) fn distance<T: EventListener>(&self, term: &Crosswords<T>) -> usize {
        (live_start(term) - self.top).max(0) as usize
    }

    /// Reconcile automatic emulator offset changes without moving our anchor.
    /// History is read lazily, first-write-wins, and cannot outlive retention.
    pub(super) fn reconcile<T: EventListener>(&mut self, term: &mut Crosswords<T>) -> bool {
        let live = live_start(term);
        if let Some(pending) = self.relocation.as_mut() {
            pending.sync_offset(term);
            return true;
        }
        if live < self.captured_live_start {
            return false;
        }
        self.top = self.top.clamp(term.grid.lines_evicted() as i64, live);
        let oldest = term.grid.lines_evicted() as i64;
        while self
            .rows
            .first_key_value()
            .is_some_and(|(row, _)| *row < oldest)
        {
            self.rows.pop_first();
            self.revision = self.revision.wrapping_add(1);
        }
        let distance = self.distance(term);
        let delta = distance as i32 - term.display_offset() as i32;
        if delta != 0 {
            term.scroll_display(Scroll::Delta(delta));
        }
        true
    }

    pub(super) fn maintain<T: EventListener>(
        &mut self,
        term: &Crosswords<T>,
        terminal: &super::remote::RemoteTerminal,
        selected: Option<std::ops::Range<usize>>,
    ) {
        if self.relocation.is_some() {
            return;
        }
        let needs = ReadingNeeds {
            top: live_start(term) - term.display_offset() as i64,
            oldest: term.grid.lines_evicted() as i64,
            rows: term.screen_lines(),
            cols: term.columns(),
            selected,
        };
        if self.needs.as_ref() == Some(&needs) {
            return;
        }
        self.top = needs.top;
        let images = terminal.images();
        let presentation = terminal.presentation_facts();
        self.retain_visible(term, &images, &presentation);
        if let Some(range) = needs.selected.as_ref().filter(|range| !range.is_empty()) {
            let first = ((range.start / self.cols) as i64).max(needs.oldest);
            let last = ((range.end.saturating_sub(1) / self.cols + 1) as i64)
                .min(live_start(term) + needs.rows as i64);
            if let Some(previous) = self
                .needs
                .as_ref()
                .and_then(|needs| needs.selected.as_ref())
                .filter(|range| !range.is_empty())
            {
                let previous_first = (previous.start / self.cols) as i64;
                let previous_last = (previous.end.saturating_sub(1) / self.cols + 1) as i64;
                self.retain_rows(
                    term,
                    &images,
                    &presentation,
                    first..last.min(previous_first),
                );
                self.retain_rows(term, &images, &presentation, first.max(previous_last)..last);
            } else {
                self.retain_rows(term, &images, &presentation, first..last);
            }
        }
        self.prune(needs.selected.clone());
        self.needs = Some(needs);
    }

    pub(super) fn columns(&self) -> usize {
        self.cols
    }

    #[cfg(test)]
    pub(super) fn allocation_observation(&self) -> serde_json::Value {
        serde_json::json!({
            "rows": self.rows.len(),
            "cells": self.rows.values().map(|row| row.cells.len()).sum::<usize>(),
            "imageSlices": self.rows.values().map(|row| row.images.len()).sum::<usize>(),
        })
    }

    pub(super) fn identity(&self) -> (u64, i64, usize, i64, i64) {
        (
            self.identity,
            self.top,
            self.cols,
            self.visible.start,
            self.visible.end,
        )
    }

    pub(super) fn content_identity(&self) -> (u64, u64) {
        (self.identity, self.revision)
    }

    pub(super) fn top(&self) -> i64 {
        self.top
    }

    pub(super) fn rows(&self) -> impl Iterator<Item = (i64, &[ReadingCell])> {
        self.rows
            .iter()
            .map(|(&row, retained)| (row, retained.cells.as_slice()))
    }

    pub(super) fn bounds(&self) -> (i64, i64) {
        (
            self.rows
                .first_key_value()
                .map_or(self.top, |(&row, _)| row),
            self.rows.last_key_value().map_or(self.top, |(&row, _)| row),
        )
    }

    pub(super) fn absolute_row(&self, row: i64) -> Option<&[ReadingCell]> {
        self.rows.get(&row).map(|row| row.cells.as_slice())
    }

    pub(super) fn intersects_rows(&self, first: i64, last: i64) -> bool {
        self.rows.range(first..=last).next().is_some()
    }

    /// A reading session needs the original mutable screen, its current
    /// viewport/sliver, and rows spanned by the user's selection — not every
    /// immutable history row it has ever visited.
    fn prune(&mut self, selected: Option<std::ops::Range<usize>>) {
        let visible = &self.visible;
        let before = self.rows.len();
        let captured =
            self.captured_live_start..self.captured_live_start + self.captured_rows as i64;
        let cols = self.cols.max(1);
        self.rows.retain(|&row, _| {
            visible.contains(&row)
                || captured.contains(&row)
                || selected.as_ref().is_some_and(|range| {
                    row >= (range.start / cols) as i64
                        && row <= (range.end.saturating_sub(1) / cols) as i64
                })
        });
        if self.rows.len() != before {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub(super) fn selected_text(
        &self,
        selection: &super::presentation::PresentedSelection,
    ) -> Option<String> {
        let range = selection.range();
        if range.is_empty() {
            return None;
        }
        let first = (range.start / self.cols) as i64;
        let last = (range.end.saturating_sub(1) / self.cols) as i64;
        let start = range.start;
        let end = range.end;
        let retained = self.rows.range(first..=last);
        if retained.clone().count() != (last - first + 1) as usize {
            return None;
        }
        Some(super::presentation::copy_rows(
            retained.map(|(&row, retained)| {
                let wrapped = retained.cells.last().is_some_and(|cell| cell.wrapped);
                (
                    wrapped,
                    retained.cells.iter().enumerate().map(move |(col, cell)| {
                        super::presentation::CopyCell {
                            c: if cell.c == '\u{10eeee}' { ' ' } else { cell.c },
                            marks: if cell.c == '\u{10eeee}' {
                                None
                            } else {
                                cell.marks.as_deref()
                            },
                            spacer: matches!(cell.wide, Wide::Spacer | Wide::LeadingSpacer),
                            tab_stop: cell.tab_stop,
                            selected: row as usize * self.cols + col >= start
                                && (row as usize * self.cols + col) < end,
                        }
                    }),
                )
            }),
            selection.clicks() >= 3,
        ))
    }
}
