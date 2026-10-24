//! The daemon's terminal-layout adapter. Rio remains the only VT parser and
//! grid. Handler callbacks attach graphics lifetime to the same text mutations.

use super::*;
use rio_unicode::UnicodeWidthChar as _;
use rio_vt::ansi::mode::{Mode, NamedPrivateMode, PrivateMode};
use rio_vt::ansi::{ClearMode, LineClearMode, TabulationClearMode};
use rio_vt::config::colors::ColorRgb;
use rio_vt::crosswords::attr::Attr;
use rio_vt::crosswords::grid::row::SemanticPrompt;
use rio_vt::crosswords::pos::{CharsetIndex, Column, Line, StandardCharset};
use rio_vt::crosswords::square::Hyperlink;
use rio_vt::performer::handler::{Handler, ScpCharPath, ScpUpdateMode};

impl GraphicsState {
    /// The live screen's first text row in stable, oldest-first coordinates.
    pub fn history_origin(&self) -> i64 {
        self.term.grid.lines_evicted() as i64 + self.term.grid.history_size() as i64
    }

    pub fn active_screen(&self) -> u64 {
        self.screen
    }

    pub fn placements(&self) -> impl Iterator<Item = &Placement> {
        self.placements.values().map(|p| &p.placement)
    }

    /// Resource keys referenced by actual placeholder text on the active grid.
    /// Retention callers choose live rows or retained history; no pixels or
    /// second placement model are allocated. Inactive-screen resources remain
    /// screen-owned and must be protected separately while their grid is hidden.
    pub fn placeholder_resources(&self, history: bool) -> BTreeSet<u64> {
        let resources = BTreeSet::new();
        if !self
            .placements()
            .any(|p| p.virtual_placement && p.screen == self.screen)
        {
            return resources;
        }
        let start = if history {
            -(self.term.grid.history_size() as i32)
        } else {
            0
        };
        self.placeholder_resources_in(start, self.term.screen_lines() as i32)
    }

    pub(super) fn placeholder_resources_in(&self, start: i32, end: i32) -> BTreeSet<u64> {
        let mut resources = BTreeSet::new();
        for row in start..end {
            for slice in placeholder_row(&self.term.grid, Line(row)) {
                if let Some(prototype) =
                    placeholder_prototype(self.placements(), self.screen, &slice.run)
                    && slice.run.row < prototype.span_rows
                    && slice.run.col < prototype.span_cols
                {
                    let image = &self.resources[&prototype.resource].image;
                    let mut run = slice.run;
                    run.width = run.width.min(prototype.span_cols - run.col);
                    if rio_vt::ansi::kitty_virtual::compute_run_geometry(
                        &run,
                        prototype.span_cols,
                        prototype.span_rows,
                        image.width,
                        image.height,
                        (prototype.x, prototype.y, prototype.w, prototype.h),
                        f32::from(self.size.cell_w.max(1)),
                        f32::from(self.size.cell_h.max(1)),
                        0.,
                        0.,
                        0,
                        slice.column,
                    )
                    .is_some()
                    {
                        resources.insert(prototype.resource);
                    }
                }
            }
        }
        resources
    }
    /// Explicit user clear also releases the active screen's retained uploads
    /// when history is discarded; a plain ED2 retains upload identity.
    pub fn clear_presentation(&mut self, history: bool) -> Vec<GraphicsEffect> {
        let bytes: &[u8] = if history {
            b"\x1b[2J\x1b[3J"
        } else {
            b"\x1b[2J"
        };
        let mut effects = vec![GraphicsEffect::Output(bytes.to_vec())];
        effects.extend(self.output(bytes));
        if history {
            let screen = self.screen;
            self.discard_placements(|p| p.screen == screen);
            self.discard_resources(Some(screen));
            effects.extend(self.finish_layout(false));
        }
        effects
    }

    pub(super) fn finish_layout(&mut self, screen_changed: bool) -> Vec<GraphicsEffect> {
        self.refresh_retention();
        let mut effects = self.drain_evictions();
        if !self.removed.is_empty() || !self.freed.is_empty() {
            effects.push(GraphicsEffect::Update(GraphicsUpdate::Remove {
                placements: std::mem::take(&mut self.removed),
                resources: std::mem::take(&mut self.freed),
            }));
        }
        for key in std::mem::take(&mut self.dirty) {
            if let Some(p) = self.placements.get(&key) {
                effects.push(GraphicsEffect::Update(GraphicsUpdate::Place(
                    p.placement.clone(),
                )));
            }
        }
        if screen_changed {
            effects.push(GraphicsEffect::Update(GraphicsUpdate::Screen {
                screen: self.screen,
                history_origin: self.history_origin(),
                epoch: self.viewport_epoch,
            }));
        }
        effects.extend(
            std::mem::take(&mut self.replies)
                .into_iter()
                .map(GraphicsEffect::Reply),
        );
        effects
    }

    fn discard_placements(&mut self, mut selected: impl FnMut(&Placement) -> bool) {
        let mut affected = Vec::new();
        self.placements.retain(|key, p| {
            if selected(&p.placement) {
                self.removed.push(*key);
                affected.push(p.placement.resource);
                self.dirty.remove(key);
                false
            } else {
                true
            }
        });
        affected.sort_unstable();
        affected.dedup();
        for resource in affected {
            if self.resources.get(&resource).is_some_and(|r| r.id == 0)
                && !self
                    .placements
                    .values()
                    .any(|p| p.placement.resource == resource)
            {
                self.free_resource(resource);
            }
        }
    }

    pub(super) fn expire_history(&mut self) {
        let screen = self.screen;
        let oldest = self.term.grid.lines_evicted() as i64;
        self.discard_placements(|p| {
            !p.virtual_placement
                && p.screen == screen
                && p.anchor_row + i64::from(p.clip_bottom) <= oldest
        });
        for (key, p) in &mut self.placements {
            let p = &mut p.placement;
            if !p.virtual_placement && p.screen == screen {
                let top = p
                    .clip_top
                    .max((oldest - p.anchor_row).clamp(0, i64::from(p.clip_bottom)) as u32);
                if top != p.clip_top {
                    p.clip_top = top;
                    self.dirty.insert(*key);
                }
            }
        }
    }

    pub(super) fn resize_layout(&mut self, size: WinSize) {
        use rio_vt::ansi::graphics::AtlasPlacement;
        let grid_changed = self.size.cols != size.cols || self.size.rows != size.rows;
        if grid_changed {
            self.viewport_epoch += 1;
            self.viewports.clear();
        }
        // Temporary, pixel-free adapter records ask Rio's public resize path
        // to retain its exact reflow map for both grids. They exist only during
        // resize: resources, ordering and retained placements stay here.
        for p in self
            .placements
            .values()
            .map(|p| &p.placement)
            .filter(|p| !p.virtual_placement)
        {
            let marker = AtlasPlacement {
                image_key: p.key,
                abs_row: p.anchor_row + i64::from(p.clip_top),
                col: p.anchor_col as usize,
                columns: 1,
                rows: (p.clip_bottom - p.clip_top) as usize,
                src_x: 0,
                src_y: 0,
                src_width: 1,
                src_height: 1,
                total_width: 1,
                total_height: 1,
                insert_cell_w: 1,
                insert_cell_h: 1,
            };
            if p.screen == self.screen {
                self.term.graphics.atlas_placements.push(marker);
            } else {
                self.term
                    .graphics
                    .kitty_inactive_screen
                    .atlas_placements
                    .push(marker);
            }
        }
        super::super::terminal_resize::resize(
            &mut self.term,
            CrosswordsSize::new(usize::from(size.cols.max(1)), usize::from(size.rows.max(1))),
            self.resize_mode,
        );
        let active = std::mem::take(&mut self.term.graphics.atlas_placements);
        let inactive =
            std::mem::take(&mut self.term.graphics.kitty_inactive_screen.atlas_placements);
        let mut retained = BTreeSet::new();
        for marker in active.into_iter().chain(inactive) {
            if let Some(p) = self.placements.get_mut(&marker.image_key) {
                p.placement.anchor_row = marker.abs_row - i64::from(p.placement.clip_top);
                retained.insert(marker.image_key);
                self.dirty.insert(marker.image_key);
            }
        }
        self.discard_placements(|p| !p.virtual_placement && !retained.contains(&p.key));
        self.size = size;
        if grid_changed {
            self.region = 0..i32::from(size.rows.max(1));
        }
        // A new font or scale changes the occupied span even when rows/cols
        // are unchanged. Recompute from retained request/crop, not old pixels.
        let geometries: Vec<_> = self
            .placements
            .iter()
            .filter_map(|(key, p)| {
                let p = &p.placement;
                let c = Control {
                    cols: p.cols,
                    rows: p.rows,
                    x: p.x,
                    y: p.y,
                    w: p.w,
                    h: p.h,
                    offset_x: p.offset_x.min(u32::from(size.cell_w.max(1)) - 1),
                    offset_y: p.offset_y.min(u32::from(size.cell_h.max(1)) - 1),
                    ..Control::default()
                };
                self.geometry(&c, &self.resources.get(&p.resource)?.image)
                    .map(|(_, _, cols, rows)| (*key, cols, rows))
            })
            .collect();
        for (key, cols, rows) in geometries {
            let p = &mut self.placements.get_mut(&key).unwrap().placement;
            let bottom_was_full = p.clip_bottom == p.span_rows;
            p.span_cols = cols;
            p.span_rows = rows;
            if bottom_was_full {
                p.clip_bottom = rows;
            } else {
                p.clip_bottom = p.clip_bottom.min(rows);
            }
            p.offset_x = p.offset_x.min(u32::from(size.cell_w.max(1)) - 1);
            p.offset_y = p.offset_y.min(u32::from(size.cell_h.max(1)) - 1);
            self.dirty.insert(key);
        }
        self.discard_placements(|p| p.clip_top >= p.clip_bottom);
        self.expire_history();
    }

    /// Scroll only graphics wholly contained in the affected page. Preserve
    /// the full texture mapping while permanently clipping the departing edge.
    fn scroll_graphics(&mut self, start: i32, end: i32, delta: i32, old_base: i64) {
        if delta == 0 || self.placements.is_empty() {
            return;
        }
        let base = self.history_origin();
        let shift = base - old_base;
        let top = old_base + i64::from(start);
        let bottom = old_base + i64::from(end);
        let keep_history =
            self.screen == 0 && start == 0 && end == i32::from(self.size.rows.max(1)) && delta < 0;
        // A full main-screen scroll advances the viewport over stable text
        // rows, including a picture straddling the history/live boundary.
        if keep_history {
            return;
        }
        for (key, p) in &mut self.placements {
            let p = &mut p.placement;
            if p.screen != self.screen || p.virtual_placement {
                continue;
            }
            let pt = p.anchor_row + i64::from(p.clip_top);
            let pb = p.anchor_row + i64::from(p.clip_bottom);
            if pt >= top && pb <= bottom {
                p.anchor_row += i64::from(delta) + shift;
                p.clip_top = p.clip_top.max((top + shift - p.anchor_row).max(0) as u32);
                p.clip_bottom = p
                    .clip_bottom
                    .min((bottom + shift - p.anchor_row).max(0) as u32);
                self.dirty.insert(*key);
            } else if shift != 0 && pb > old_base {
                // Fixed rows outside a top-origin scrolling region must not
                // drift merely because the region added rows to history.
                p.anchor_row += shift;
                self.dirty.insert(*key);
            }
        }
        self.discard_placements(|p| p.clip_top >= p.clip_bottom);
    }

    fn index_scroll(&mut self, reverse: bool, action: impl FnOnce(&mut Crosswords<VoidListener>)) {
        let row = self.term.grid.cursor.pos.row.0;
        let base = self.history_origin();
        let delta = if reverse && row == self.region.start {
            1
        } else if !reverse && row + 1 == self.region.end {
            -1
        } else {
            0
        };
        action(&mut self.term);
        self.scroll_graphics(self.region.start, self.region.end, delta, base);
    }

    fn free_resource(&mut self, key: u64) {
        if let Some(resource) = self.resources.remove(&key) {
            drop(resource);
            self.freed.push(key);
        }
    }

    fn discard_resources(&mut self, screen: Option<u64>) {
        let keys: Vec<_> = self
            .resources
            .iter()
            .filter_map(|(key, r)| {
                screen
                    .is_none_or(|screen| r.screen == screen)
                    .then_some(*key)
            })
            .collect();
        for key in keys {
            self.free_resource(key);
        }
    }

    fn sync_screen(&mut self) {
        let screen = u64::from(
            self.term
                .mode()
                .contains(rio_vt::crosswords::Mode::ALT_SCREEN),
        );
        if screen == self.screen {
            return;
        }
        // Alternate graphics are a bounded, transient screen. Clear on both
        // edges so old in-flight decodes cannot revive a previous invocation.
        self.discard_placements(|p| p.screen == 1);
        self.discard_resources(Some(1));
        self.screen = screen;
        self.viewport_epoch += 1;
        self.viewports.clear();
    }
}

impl Handler for GraphicsState {
    fn input(&mut self, c: char) {
        if self.placements.is_empty() {
            self.term.input(c);
            return;
        }
        let base = self.history_origin();
        let width = c.width().unwrap_or(0);
        let wraps = width != 0
            && self
                .term
                .mode()
                .contains(rio_vt::crosswords::Mode::LINE_WRAP)
            && (self.term.grid.cursor.should_wrap
                || self.term.grid.cursor.pos.col.0 + width > self.term.grid.columns())
            && self.term.grid.cursor.pos.row.0 + 1 == self.region.end;
        self.term.input(c);
        if wraps {
            self.scroll_graphics(self.region.start, self.region.end, -1, base);
        }
    }
    fn input_str(&mut self, s: &str) {
        if self.placements.is_empty() {
            self.term.input_str(s);
        } else {
            for c in s.chars() {
                self.input(c);
            }
        }
    }
    fn input_ascii_str(&mut self, mut s: &str) {
        if self.placements.is_empty() {
            self.term.input_ascii_str(s);
            return;
        }
        while !s.is_empty() {
            if self.term.grid.cursor.should_wrap {
                self.input(char::from(s.as_bytes()[0]));
                s = &s[1..];
            } else {
                // Batch through a row; only its wrap boundary needs a graphics
                // callback. Ordinary ASCII output keeps Rio's fast bulk path.
                let end = s
                    .len()
                    .min(self.term.grid.columns() - self.term.grid.cursor.pos.col.0);
                self.term.input_ascii_str(&s[..end]);
                s = &s[end..];
            }
        }
    }
    fn input_codepoints(&mut self, codepoints: &[u32]) {
        if self.placements.is_empty() {
            self.term.input_codepoints(codepoints);
        } else {
            for cp in codepoints {
                if let Some(c) = char::from_u32(*cp) {
                    self.input(c);
                }
            }
        }
    }
    fn linefeed(&mut self) {
        self.index_scroll(false, |t| t.linefeed());
    }
    fn newline(&mut self) {
        self.index_scroll(false, |t| t.newline());
    }
    fn reverse_index(&mut self) {
        self.index_scroll(true, |t| t.reverse_index());
    }
    fn scroll_up(&mut self, rows: usize) {
        let base = self.history_origin();
        self.term.scroll_up(rows);
        self.scroll_graphics(
            self.region.start,
            self.region.end,
            -(rows.min((self.region.end - self.region.start) as usize) as i32),
            base,
        );
    }
    fn scroll_down(&mut self, rows: usize) {
        let base = self.history_origin();
        self.term.scroll_down(rows);
        self.scroll_graphics(
            self.region.start,
            self.region.end,
            rows.min((self.region.end - self.region.start) as usize) as i32,
            base,
        );
    }
    fn insert_blank_lines(&mut self, count: usize) {
        let row = self.term.grid.cursor.pos.row.0;
        let base = self.history_origin();
        self.term.insert_blank_lines(count);
        if self.region.contains(&row) {
            self.scroll_graphics(
                row,
                self.region.end,
                count.min((self.region.end - row) as usize) as i32,
                base,
            );
        }
    }
    fn delete_lines(&mut self, count: usize) {
        let row = self.term.grid.cursor.pos.row.0;
        let base = self.history_origin();
        self.term.delete_lines(count);
        if self.region.contains(&row) {
            self.scroll_graphics(
                row,
                self.region.end,
                -(count.min((self.region.end - row) as usize) as i32),
                base,
            );
        }
    }
    fn clear_screen(&mut self, mode: ClearMode) {
        let screen = self.screen;
        let base = self.history_origin();
        let end = base + i64::from(self.size.rows.max(1));
        match mode {
            ClearMode::All => self.discard_placements(|p| {
                !p.virtual_placement
                    && p.screen == screen
                    && p.anchor_row + i64::from(p.clip_bottom) > base
                    && p.anchor_row + i64::from(p.clip_top) < end
            }),
            ClearMode::Saved => {
                self.viewport_epoch += 1;
                self.viewports.clear();
                for (key, p) in &mut self.placements {
                    let p = &mut p.placement;
                    if !p.virtual_placement
                        && p.screen == screen
                        && p.anchor_row + i64::from(p.clip_top) < base
                    {
                        p.clip_top = (base - p.anchor_row).max(0).min(i64::from(u32::MAX)) as u32;
                        self.dirty.insert(*key);
                    }
                }
                self.discard_placements(|p| p.clip_top >= p.clip_bottom);
            }
            _ => (),
        }
        self.term.clear_screen(mode);
    }
    fn reset_state(&mut self) {
        self.viewport_epoch += 1;
        self.viewports.clear();
        self.discard_placements(|_| true);
        self.discard_resources(None);
        self.term.reset_state();
        self.screen = 0;
        self.region = 0..i32::from(self.size.rows.max(1));
    }
    fn set_private_mode(&mut self, mode: PrivateMode) {
        self.term.set_private_mode(mode);
        if mode == PrivateMode::Named(NamedPrivateMode::ColumnMode) {
            self.region = 0..i32::from(self.size.rows.max(1));
        }
        self.sync_screen();
    }
    fn unset_private_mode(&mut self, mode: PrivateMode) {
        self.term.unset_private_mode(mode);
        if mode == PrivateMode::Named(NamedPrivateMode::ColumnMode) {
            self.region = 0..i32::from(self.size.rows.max(1));
        }
        self.sync_screen();
    }
    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        let bottom = bottom
            .unwrap_or(usize::from(self.size.rows.max(1)))
            .min(usize::from(self.size.rows.max(1)));
        if top.max(1) < bottom {
            self.region = top.max(1) as i32 - 1..bottom as i32;
        }
        self.term.set_scrolling_region(top, Some(bottom));
    }
    fn text_area_size_pixels(&mut self) {
        let height = u32::from(self.size.rows) * u32::from(self.size.cell_h);
        let width = u32::from(self.size.cols) * u32::from(self.size.cell_w);
        if width != 0 && height != 0 {
            self.replies
                .push(format!("\x1b[4;{height};{width}t").into_bytes());
        }
    }
    fn cells_size_pixels(&mut self) {
        let (w, h) = (self.size.cell_w, self.size.cell_h);
        if w != 0 && h != 0 {
            self.replies.push(format!("\x1b[6;{h};{w}t").into_bytes());
        }
    }
    fn set_title(&mut self, title: Option<String>) {
        self.term.set_title(title);
    }

    fn set_current_directory(&mut self, path: std::path::PathBuf) {
        self.term.set_current_directory(path);
    }

    fn set_semantic_prompt(&mut self, prompt: SemanticPrompt) {
        self.term.set_semantic_prompt(prompt);
    }

    fn set_user_var(&mut self, name: String, value: String) {
        self.term.set_user_var(name, value);
    }

    fn set_cursor_style(&mut self, style: Option<CursorShape>, blinking: bool) {
        self.term.set_cursor_style(style, blinking);
    }

    fn set_cursor_shape(&mut self, shape: CursorShape) {
        self.term.set_cursor_shape(shape);
    }

    fn goto(&mut self, line: Line, col: Column) {
        self.term.goto(line, col);
    }

    fn goto_line(&mut self, line: Line) {
        self.term.goto_line(line);
    }

    fn goto_col(&mut self, col: Column) {
        self.term.goto_col(col);
    }

    fn insert_blank(&mut self, count: usize) {
        self.term.insert_blank(count);
    }

    fn move_up(&mut self, rows: usize) {
        self.term.move_up(rows);
    }

    fn move_down(&mut self, rows: usize) {
        self.term.move_down(rows);
    }

    fn device_status(&mut self, arg: usize) {
        self.term.device_status(arg);
    }

    fn move_forward(&mut self, cols: Column) {
        self.term.move_forward(cols);
    }

    fn move_backward(&mut self, cols: Column) {
        self.term.move_backward(cols);
    }

    fn move_down_and_cr(&mut self, rows: usize) {
        self.term.move_down_and_cr(rows);
    }

    fn move_up_and_cr(&mut self, rows: usize) {
        self.term.move_up_and_cr(rows);
    }

    fn put_tab(&mut self, count: u16) {
        self.term.put_tab(count);
    }

    fn backspace(&mut self) {
        self.term.backspace();
    }

    fn carriage_return(&mut self) {
        self.term.carriage_return();
    }

    fn bell(&mut self) {
        self.term.bell();
    }

    fn desktop_notification(&mut self, title: String, body: String) {
        self.term.desktop_notification(title, body);
    }

    fn substitute(&mut self) {
        self.term.substitute();
    }

    fn set_horizontal_tabstop(&mut self) {
        self.term.set_horizontal_tabstop();
    }

    fn erase_chars(&mut self, count: Column) {
        self.term.erase_chars(count);
    }

    fn delete_chars(&mut self, count: usize) {
        self.term.delete_chars(count);
    }

    fn move_backward_tabs(&mut self, count: u16) {
        self.term.move_backward_tabs(count);
    }

    fn move_forward_tabs(&mut self, count: u16) {
        self.term.move_forward_tabs(count);
    }

    fn save_cursor_position(&mut self) {
        self.term.save_cursor_position();
    }

    fn restore_cursor_position(&mut self) {
        self.term.restore_cursor_position();
    }

    fn clear_line(&mut self, mode: LineClearMode) {
        self.term.clear_line(mode);
    }

    fn set_tabs(&mut self, interval: u16) {
        self.term.set_tabs(interval);
    }

    fn clear_tabs(&mut self, mode: TabulationClearMode) {
        self.term.clear_tabs(mode);
    }

    fn terminal_attribute(&mut self, attr: Attr) {
        self.term.terminal_attribute(attr);
    }

    fn set_mode(&mut self, mode: Mode) {
        self.term.set_mode(mode);
    }

    fn unset_mode(&mut self, mode: Mode) {
        self.term.unset_mode(mode);
    }

    fn report_mode(&mut self, mode: Mode) {
        self.term.report_mode(mode);
    }

    fn report_private_mode(&mut self, mode: PrivateMode) {
        self.term.report_private_mode(mode);
    }

    fn set_keypad_application_mode(&mut self) {
        self.term.set_keypad_application_mode();
    }

    fn unset_keypad_application_mode(&mut self) {
        self.term.unset_keypad_application_mode();
    }

    fn set_active_charset(&mut self, index: CharsetIndex) {
        self.term.set_active_charset(index);
    }

    fn configure_charset(&mut self, index: CharsetIndex, charset: StandardCharset) {
        self.term.configure_charset(index, charset);
    }

    fn set_color(&mut self, index: usize, color: ColorRgb) {
        self.term.set_color(index, color);
    }

    fn dynamic_color_sequence(&mut self, sequence: String, index: usize, terminator: &str) {
        self.term
            .dynamic_color_sequence(sequence, index, terminator);
    }

    fn reset_color(&mut self, index: usize) {
        self.term.reset_color(index);
    }

    fn clipboard_store(&mut self, clipboard: u8, data: &[u8]) {
        self.term.clipboard_store(clipboard, data);
    }

    fn clipboard_load(&mut self, clipboard: u8, terminator: &str) {
        self.term.clipboard_load(clipboard, terminator);
    }

    fn decaln(&mut self) {
        self.term.decaln();
    }

    fn push_title(&mut self) {
        self.term.push_title();
    }

    fn pop_title(&mut self) {
        self.term.pop_title();
    }

    fn text_area_size_chars(&mut self) {
        self.term.text_area_size_chars();
    }

    fn set_hyperlink(&mut self, hyperlink: Option<Hyperlink>) {
        self.term.set_hyperlink(hyperlink);
    }

    fn set_progress_report(&mut self, report: rio_vt::event::ProgressReport) {
        self.term.set_progress_report(report);
    }

    fn set_modify_other_keys(&mut self, level: u8) {
        self.term.set_modify_other_keys(level);
    }

    fn set_scp(&mut self, char_path: ScpCharPath, update_mode: ScpUpdateMode) {
        self.term.set_scp(char_path, update_mode);
    }
}
