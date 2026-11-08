//! In-place formula editing. The tree and preedit are presentation state; only
//! `finish_formula` can write, through the owning MarkdownDocument command path.
use super::*;
use gpui_kit::component::{
    Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
};
use ratex_gpui::editor::{
    cursor::{Cursor, common_row_selection},
    geometry, input,
    model::{Atom, Row},
};
#[path = "math_edit/alignment.rs"]
pub(in crate::ui::code_editor::markdown) mod alignment;
#[path = "math_edit/context.rs"]
mod context;
#[path = "math_edit/lossless.rs"]
mod lossless;
use alignment::Align;
pub(super) use context::Tools;

actions!(
    markdown_formula,
    [
        CommitFormula,
        CancelFormula,
        EditFormulaSource,
        MatrixAddRow,
        MatrixRemoveRow,
        MatrixAddColumn,
        MatrixRemoveColumn,
        FormulaMenu,
        CopyFormulaLatex,
        ExportFormulaPng,
        ExportFormulaSvg,
        AlignFormulaLeft,
        AlignFormulaCenter,
        AlignFormulaRight
    ]
);

#[derive(Clone)]
pub(in crate::ui::code_editor::markdown) struct Editor {
    view: WeakEntity<SourcePresentation>,
    session: u64,
    extent: (f32, f32, f32),
    output: Option<super::super::math::Output>,
    preedit: String,
    caret: Option<geometry::Rect>,
}
pub(super) struct Session {
    range: Range<usize>,
    revision: u64,
    original: String,
    body: Range<usize>,
    seed: lossless::Seed,
    root: Row,
    cursor: Cursor,
    anchor: Option<Cursor>,
    preedit: String,
    preedit_selection: Range<usize>,
    command: Option<String>,
    completion_ix: usize,
    alignment: Option<Align>,
    generation: u64,
    rendered_generation: Option<u64>,
    id: u64,
    _blur: Option<Subscription>,
    style: Option<super::super::math::Style>,
    output: Option<super::super::math::Output>,
    render_error: Option<String>,
    caret_cache: Option<geometry::Rect>,
    selection_cache: Option<geometry::Rect>,
    extent: (f32, f32, f32),
    frame: Option<(Bounds<Pixels>, composition::Frame, Pixels)>,
    dragging: bool,
    formula: super::super::math::Formula,
}
impl Session {
    fn new(
        range: Range<usize>,
        revision: u64,
        original: String,
        formula: super::super::math::Formula,
        end: bool,
    ) -> Self {
        let opening = original.bytes().take_while(|b| *b == b'$').count();
        let closing_end = original.trim_end_matches([' ', '\t']).len();
        let closing = original[..closing_end]
            .bytes()
            .rev()
            .take_while(|b| *b == b'$')
            .count()
            .min(closing_end - opening);
        let body = opening..closing_end - closing;
        let seed = lossless::Seed::parse(&original[body.clone()]);
        let root = seed.row.clone();
        let cursor = Cursor {
            path: Vec::new(),
            index: if end { root.atoms.len() } else { 0 },
        };
        let mut session = Self {
            range,
            revision,
            original,
            body,
            seed,
            root,
            cursor,
            anchor: None,
            preedit: String::new(),
            preedit_selection: 0..0,
            command: None,
            completion_ix: 0,
            alignment: None,
            generation: 0,
            rendered_generation: None,
            id: 0,
            _blur: None,
            style: None,
            output: None,
            render_error: None,
            caret_cache: None,
            selection_cache: None,
            extent: (0., 0., 0.),
            frame: None,
            dragging: false,
            formula,
        };
        session.refresh_geometry(true);
        session
    }
    fn latex(&self) -> String {
        self.seed.serialize(&self.root)
    }
    fn dirty(&self) -> bool {
        self.root != self.seed.row
    }
    fn refresh_geometry(&mut self, changed: bool) {
        if changed {
            let list = ratex_layout::to_display_list(&geometry::layout_row(&self.root));
            self.extent = (
                list.width as f32,
                (list.height + list.depth) as f32,
                list.height as f32,
            );
        }
        self.caret_cache = geometry::caret_rect(&self.root, &self.cursor);
        self.selection_cache = self.anchor.as_ref().and_then(|anchor| {
            let (path, lo, hi) = common_row_selection(anchor, &self.cursor);
            if lo == hi {
                return None;
            }
            let a = geometry::caret_rect(
                &self.root,
                &Cursor {
                    path: path.clone(),
                    index: lo,
                },
            )?;
            let b = geometry::caret_rect(&self.root, &Cursor { path, index: hi })?;
            Some(geometry::Rect {
                x: a.x.min(b.x),
                y: a.y.min(b.y),
                w: (b.x - a.x).abs().max(0.1),
                h: a.h.max(b.h),
            })
        });
    }
    fn delete_selection(&mut self) -> bool {
        let Some(anchor) = self.anchor.take() else {
            return false;
        };
        let (path, lo, hi) = common_row_selection(&anchor, &self.cursor);
        if lo == hi {
            return false;
        }
        self.cursor = Cursor { path, index: lo };
        self.cursor.delete_range(&mut self.root, lo, hi);
        true
    }
    fn insert_text(&mut self, text: &str) {
        self.preedit.clear();
        self.preedit_selection = 0..0;
        for grapheme in unicode_segmentation::UnicodeSegmentation::graphemes(text, true) {
            if grapheme.chars().count() > 1 {
                self.delete_selection();
                self.cursor
                    .insert(&mut self.root, Atom::Sym(grapheme.into()));
                continue;
            }
            let ch = grapheme.chars().next().unwrap();
            if let Some(command) = &mut self.command {
                if ch.is_ascii_alphabetic() {
                    command.push(ch);
                    self.completion_ix = 0;
                    continue;
                }
                self.commit_command();
                if matches!(ch, ' ' | '{') {
                    continue;
                }
            }
            if ch == '\\' {
                self.command = Some(String::new());
                self.completion_ix = 0;
                continue;
            }
            if self.anchor.is_some() && matches!(ch, '/' | '(' | '{') {
                let (path, lo, hi) =
                    common_row_selection(self.anchor.as_ref().unwrap(), &self.cursor);
                self.cursor = Cursor { path, index: lo };
                self.anchor = None;
                if ch == '/' {
                    self.cursor.wrap_fraction(&mut self.root, lo, hi);
                } else {
                    let (open, close) = if ch == '(' {
                        ("(", ")")
                    } else {
                        (r"\{", r"\}")
                    };
                    self.cursor.wrap_delim(&mut self.root, lo, hi, open, close);
                }
            } else {
                self.delete_selection();
                input::type_char(&mut self.root, &mut self.cursor, ch);
            }
        }
    }
    fn commit_command(&mut self) {
        let Some(command) = self.command.take() else {
            return;
        };
        let matches = input::command_matches(&command);
        let command = matches
            .get(self.completion_ix)
            .or_else(|| matches.first())
            .copied()
            .unwrap_or(&command);
        let selected = self
            .anchor
            .take()
            .map(|anchor| common_row_selection(&anchor, &self.cursor));
        if let Some((path, lo, _)) = &selected {
            self.cursor = Cursor {
                path: path.clone(),
                index: *lo,
            };
        }
        let selection = selected.map(|(_, lo, hi)| (lo, hi));
        if !input::commit_command_selecting(&mut self.root, &mut self.cursor, command, selection) {
            if let Some((lo, hi)) = selection {
                self.cursor.delete_range(&mut self.root, lo, hi);
            }
            // Unknown commands remain explicit opaque atoms rather than disappearing.
            self.cursor
                .insert(&mut self.root, Atom::Sym(format!("\\{command}")));
        }
    }
    fn completions(&self) -> Vec<&'static str> {
        if !self.preedit.is_empty() {
            return Vec::new();
        }
        self.command
            .as_deref()
            .map(input::command_matches)
            .unwrap_or_default()
    }
    fn accept_completion(&mut self, name: &str) -> bool {
        if !self.preedit.is_empty() || !self.completions().contains(&name) {
            return false;
        }
        self.command = Some(name.into());
        self.completion_ix = 0;
        self.commit_command();
        true
    }
    fn edits(&self, source: &Rope) -> Vec<(Range<usize>, String)> {
        let mut edits = Vec::new();
        if self.dirty() {
            let (range, text) = minimal_edit(&self.original[self.body.clone()], &self.latex());
            edits.push((
                range.start + self.range.start + self.body.start
                    ..range.end + self.range.start + self.body.start,
                text,
            ));
        }
        if self.formula.block {
            if let Some(align) = self.alignment {
                if let Some(edit) = alignment::edit(source, self.range.start, align) {
                    if source.slice(edit.0.clone()).to_string() != edit.1 {
                        edits.push(edit);
                    }
                }
            }
        }
        edits.sort_by_key(|(range, _)| range.start);
        edits
    }
    fn caret(&self) -> Option<geometry::Rect> {
        self.caret_cache
    }
    fn caret_bounds(
        &self,
        view: &SourcePresentation,
        window: &Window,
        cx: &App,
    ) -> Option<Bounds<Pixels>> {
        let (bounds, frame, font) = self.frame?;
        if !frame.is_current(view, window, cx) {
            return None;
        }
        let rect = self.caret()?;
        Some(Bounds::new(
            bounds.origin + point(font * rect.x as f32, font * rect.y as f32),
            size(px(1.), font * rect.h as f32),
        ))
    }
}

impl SourcePresentation {
    pub(super) fn sync_formula_surface(&mut self, cx: &Context<Self>) {
        self.math.editor = self.formula.as_ref().map(|session| {
            (
                session.range.clone(),
                Editor {
                    view: cx.entity().downgrade(),
                    session: session.id,
                    extent: session.extent,
                    output: session.output.clone(),
                    preedit: session.preedit.clone(),
                    caret: session.caret_cache,
                },
            )
        });
    }
    pub(super) fn formula_at(
        &self,
        at: usize,
        end: bool,
        cx: &App,
    ) -> Option<(Range<usize>, super::super::math::Formula)> {
        if !self.live || self.projection.revision != self.document.read(cx).revision() {
            return None;
        }
        self.projection
            .leaves
            .iter()
            .find_map(|leaf| leaf.formula_at(at, end))
    }
    pub(super) fn begin_formula_from_pointer(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.live || !self.geometry_is_current(cx) {
            return false;
        }
        let Some((_, geometry)) = &self.geometry else {
            return false;
        };
        let hit = self.projection.leaves.iter().find_map(|leaf| {
            let geometry = geometry.get(&leaf.id)?;
            let atom = geometry.atom_at(position)?;
            let at = leaf.atom_source(atom.start)?;
            let (_, formula) = leaf.formula_at(at, false)?;
            let super::super::math::Output::Ready { width, height, .. } =
                self.math.outputs.get(&formula)?
            else {
                return None;
            };
            let before = geometry.position_for_index(atom.start);
            let after = geometry.position_for_index(atom.end);
            let pixel_width = (after.x - before.x).abs().max(px(1.));
            let font = pixel_width / width.max(0.01);
            let top = before.y - (font * *height - geometry.line_height()).max(Pixels::ZERO);
            Some((
                at,
                f32::from(position.x - before.x) / f32::from(pixel_width),
                f32::from(position.y - top) / f32::from(font * height.max(0.01)),
            ))
        });
        let Some((at, x, y)) = hit else {
            return false;
        };
        if !self.begin_formula(at, false, window, cx) {
            return false;
        }
        if let Some(session) = &mut self.formula {
            session.cursor = geometry::cursor_at(
                &session.root,
                (session.extent.0 * x) as f64,
                (session.extent.1 * y) as f64,
            );
            session.refresh_geometry(false);
        }
        cx.notify();
        true
    }
    pub(super) fn begin_formula(
        &mut self,
        at: usize,
        end: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((target, _)) = self.formula_at(at, end, cx) else {
            return false;
        };
        if self
            .formula
            .as_ref()
            .is_some_and(|session| session.range == target)
        {
            return true;
        }
        let previous_end = self.formula.as_ref().map(|session| session.range.end);
        let old_len = self.document.read(cx).source().len();
        self.finish_formula(true, false, window, cx);
        let new_len = self.document.read(cx).source().len();
        let at = if previous_end.is_some_and(|last| at >= last) {
            at.saturating_add(new_len).saturating_sub(old_len)
        } else {
            at
        };
        Self::finish_active_composition(&self.document.clone(), window, cx);
        self.command(Command::FinishComposition, window, cx);
        let Some((range, formula)) = self.formula_at(at, end, cx) else {
            return false;
        };
        let document = self.document.read(cx);
        let original = document.source().slice(range.clone()).to_string();
        let mut session = Session::new(
            range.clone(),
            document.revision(),
            original,
            formula.clone(),
            end,
        );
        self.input_generation = self.input_generation.wrapping_add(1);
        session.id = self.input_generation;
        session.output = self.math.outputs.get(&formula).cloned();
        self.formula = Some(session);
        self.formula_tools.palette_open = false;
        self.state.selection = Selection::caret(range.start);
        self.extra_selections.clear();
        self.dragging = false;
        self.sync_formula_surface(cx);
        self.sync_inline_reveal();
        let session_id = self.formula.as_ref().unwrap().id;
        let blur = cx.on_focus_out(&self.focus.clone(), window, move |view, _, window, cx| {
            if !view.focus.contains_focused(window, cx)
                && view.formula_tools.menu.is_none()
                && !view.formula_tools.exporting
                && !view.formula_tools.palette_open
                && view
                    .formula
                    .as_ref()
                    .is_some_and(|session| session.id == session_id)
            {
                view.finish_formula(true, false, window, cx);
            }
        });
        self.formula.as_mut().unwrap()._blur = Some(blur);
        let owner = cx.entity().downgrade();
        self.document
            .update(cx, |document, _| document.composition_owner = Some(owner));
        self.input_generation = self.input_generation.wrapping_add(1);
        self.focus.focus(window, cx);
        self.formula_changed(window, cx);
        true
    }
    pub(super) fn finish_formula(
        &mut self,
        commit: bool,
        backward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut session) = self.formula.take() else {
            return;
        };
        self.formula_tools.palette_open = false;
        self.invalidate_formula_cell(&session.range);
        if commit && !session.preedit.is_empty() {
            let text = std::mem::take(&mut session.preedit);
            session.insert_text(&text);
        }
        if commit {
            session.commit_command();
        }
        self.math.editor = None;
        self.formula_render = None;
        let owner_id = cx.entity_id();
        self.document.update(cx, |document, _| {
            if document
                .composition_owner
                .as_ref()
                .is_some_and(|owner| owner.entity_id() == owner_id)
            {
                document.composition_owner = None;
            }
        });
        let valid = {
            let document = self.document.read(cx);
            session.revision == document.revision()
                && session.range.end <= document.source().len()
                && document.source().slice(session.range.clone()).to_string() == session.original
        };
        let mut after = session.range.end;
        let edits = if commit && valid {
            session.edits(self.document.read(cx).source())
        } else {
            Vec::new()
        };
        if !edits.is_empty() {
            after = edits
                .iter()
                .fold(after, |at, (range, text)| at - range.len() + text.len());
            let before = edits
                .iter()
                .filter(|(range, _)| range.end <= session.range.start)
                .fold(session.range.start, |at, (range, text)| {
                    at - range.len() + text.len()
                });
            self.command(
                Command::BatchReplaceSelected {
                    edits,
                    selection: Selection::caret(if backward { before } else { after }),
                },
                window,
                cx,
            );
        } else if valid {
            self.state.selection =
                Selection::caret(if backward { session.range.start } else { after });
        }
        self.input_generation = self.input_generation.wrapping_add(1);
        self.invalidate_formula_blocks();
        self.sync_inline_reveal();
        self.retire_native_composition(commit, cx);
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }
    fn invalidate_formula_cell(&mut self, range: &Range<usize>) {
        for leaf in &self.projection.leaves {
            if matches!(leaf.kind, Kind::TableCell(_))
                && leaf.range.start <= range.start && range.end <= leaf.range.end
            { self.tables.invalidate_cell(leaf.id); }
        }
    }
    pub(super) fn invalidate_formula_blocks(&mut self) {
        if let Some(range) = self.formula.as_ref().map(|session| session.range.clone()) {
            self.invalidate_formula_cell(&range);
        }
        for cached in self.blocks.values_mut() {
            cached.version = u64::MAX;
        }
        self.rendered_revision = None;
        self.geometry = None;
    }
    pub(super) fn rebase_formula(&mut self, transaction: &Transaction, cx: &mut Context<Self>) {
        let Some(session) = &mut self.formula else {
            return;
        };
        let touched = transaction.edits.iter().any(|edit| {
            edit.range.start < session.range.end && edit.range.end > session.range.start
        });
        let mapped =
            transaction.map_selection(Selection::new(session.range.start, session.range.end));
        session.range = mapped.range();
        session.revision = transaction.revision();
        session.frame = None;
        if let Some((range, _)) = &mut self.math.editor {
            *range = session.range.clone();
        }
        self.formula_render = None;
        session.rendered_generation = None;
        let invalid = {
            let document = self.document.read(cx);
            touched
                || session.range.end > document.source().len()
                || document.source().slice(session.range.clone()).to_string() != session.original
        };
        if invalid {
            self.formula = None;
            self.math.editor = None;
            self.input_generation = self.input_generation.wrapping_add(1);
            let owner_id = cx.entity_id();
            self.document.update(cx, |document, _| {
                if document
                    .composition_owner
                    .as_ref()
                    .is_some_and(|owner| owner.entity_id() == owner_id)
                {
                    document.composition_owner = None;
                }
            });
        }
    }
    pub(super) fn formula_command(
        &mut self,
        command: &Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.formula.is_none() {
            return false;
        }
        match command {
            Command::Select(selection) => {
                let selected = self
                    .release_formula(cx.entity_id(), Some(*selection), cx)
                    .unwrap_or(*selection);
                self.command(Command::Select(selected), window, cx);
                true
            }
            Command::Reload { .. } => true,
            Command::Replace {
                text,
                kind: EditKind::Paste,
                ..
            } => {
                self.formula_paste(text, window, cx);
                true
            }
            Command::Insert(text) | Command::Commit(text) => {
                self.formula.as_mut().unwrap().insert_text(text);
                self.formula_changed(window, cx);
                true
            }
            Command::Compose {
                range,
                text,
                selected,
            } => self.formula_preedit(range.clone(), text, selected.clone(), window, cx),
            Command::CancelComposition => {
                if self.formula.as_ref().is_some_and(|s| !s.preedit.is_empty()) {
                    self.formula.as_mut().unwrap().preedit.clear();
                    self.formula_changed(window, cx);
                } else if self.formula.as_ref().is_some_and(|s| s.command.is_some()) {
                    self.formula.as_mut().unwrap().command = None;
                    self.formula_changed(window, cx);
                } else {
                    self.finish_formula(false, false, window, cx);
                }
                true
            }
            Command::Backspace | Command::Delete => {
                let session = self.formula.as_mut().unwrap();
                if let Some(command) = &mut session.command {
                    if command.pop().is_none() {
                        session.command = None;
                    }
                    session.completion_ix = 0;
                } else if !session.delete_selection() {
                    if matches!(command, Command::Delete) {
                        let lo = session.cursor.index;
                        session.cursor.delete_range(&mut session.root, lo, lo + 1);
                    } else {
                        session.cursor.backspace(&mut session.root);
                    }
                }
                self.formula_changed(window, cx);
                true
            }
            _ => {
                self.finish_formula(true, false, window, cx);
                false
            }
        }
    }
    pub(super) fn formula_move(
        &mut self,
        horizontal: i32,
        vertical: i32,
        extend: bool,
        edge: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = &mut self.formula else {
            return false;
        };
        if !session.preedit.is_empty() {
            return true;
        }
        if vertical != 0 && !extend && edge.is_none() {
            let count = session.completions().len();
            if count > 0 {
                session.completion_ix = if vertical < 0 {
                    (session.completion_ix + count - 1) % count
                } else {
                    (session.completion_ix + 1) % count
                };
                cx.notify();
                return true;
            }
        }
        self.input_generation = self.input_generation.wrapping_add(1);
        let before = session.cursor.clone();
        if !extend && horizontal != 0 {
            if let Some(anchor) = session.anchor.take() {
                let (path, lo, hi) = common_row_selection(&anchor, &session.cursor);
                if lo < hi {
                    session.cursor = Cursor {
                        path,
                        index: if horizontal < 0 { lo } else { hi },
                    };
                    session.refresh_geometry(false);
                    self.invalidate_formula_blocks();
                    window.notify_text_selection_changed(&self.focus);
                    cx.notify();
                    return true;
                }
            }
        }
        if extend {
            session.anchor.get_or_insert_with(|| before.clone());
        } else {
            session.anchor = None;
        }
        if let Some(end) = edge {
            session.cursor.index = if end {
                session.cursor.row(&session.root).atoms.len()
            } else {
                0
            };
        } else if vertical < 0 {
            session.cursor.move_up(&session.root);
        } else if vertical > 0 {
            session.cursor.move_down(&session.root);
        } else if horizontal < 0 {
            session.cursor.move_left(&session.root);
        } else {
            session.cursor.move_right(&session.root);
        }
        session.refresh_geometry(false);
        let exit =
            edge.is_none() && before == session.cursor && !extend && session.cursor.path.is_empty();
        self.invalidate_formula_blocks();
        if exit {
            // A root boundary leaves the formula, not its source delimiters.
            self.finish_formula(true, horizontal < 0 || vertical < 0, window, cx);
        } else {
            window.notify_text_selection_changed(&self.focus);
            cx.notify();
        }
        true
    }
    // Bound input actions run before `on_key_down`. Keep document-edge actions
    // inside the structural row instead of committing it as a source selection.
    pub(super) fn formula_document_edge(
        &mut self,
        end: bool,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = &mut self.formula else {
            return false;
        };
        if !session.preedit.is_empty() {
            return true;
        }
        if extend {
            session.anchor.get_or_insert_with(|| session.cursor.clone());
        } else {
            session.anchor = None;
        }
        session.cursor = Cursor {
            path: Vec::new(),
            index: if end { session.root.atoms.len() } else { 0 },
        };
        session.refresh_geometry(false);
        self.input_generation = self.input_generation.wrapping_add(1);
        self.invalidate_formula_blocks();
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
        true
    }
    pub(super) fn formula_select_all(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = &mut self.formula else {
            return false;
        };
        if !session.preedit.is_empty() {
            return true;
        }
        session.anchor = Some(Cursor::start());
        self.formula_document_edge(true, true, window, cx)
    }
    pub(super) fn formula_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.formula.is_none() || !self.focus.is_focused(window) {
            return false;
        }
        let key = &event.keystroke;
        if self.formula.as_ref().is_some_and(|s| !s.preedit.is_empty()) && key.key != "escape" {
            return false;
        }
        let primary = if cfg!(target_os = "macos") {
            key.modifiers.platform
        } else {
            key.modifiers.control
        };
        if self.formula_completion_key(event, window, cx) {
            return true;
        }
        match key.key.as_str() {
            "home" | "end" if primary => {
                self.formula_document_edge(key.key == "end", key.modifiers.shift, window, cx)
            }
            "f10" if key.modifiers.shift => {
                self.open_formula_menu(None, window, cx);
                true
            }
            "contextmenu" => {
                self.open_formula_menu(None, window, cx);
                true
            }
            "f6" => {
                window.focus_next(cx);
                true
            }
            "escape" => {
                self.command(Command::CancelComposition, window, cx);
                true
            }
            "enter" => {
                self.formula_enter(window, cx);
                true
            }
            "tab" => {
                if self.formula.as_ref().is_some_and(|s| s.command.is_some()) {
                    self.formula.as_mut().unwrap().commit_command();
                    self.formula_changed(window, cx);
                } else {
                    self.formula_move(
                        if key.modifiers.shift { -1 } else { 1 },
                        0,
                        false,
                        None,
                        window,
                        cx,
                    );
                }
                true
            }
            "a" if primary => self.formula_select_all(window, cx),
            "left" if !primary && !key.modifiers.alt => {
                self.formula_move(-1, 0, key.modifiers.shift, None, window, cx)
            }
            "right" if !primary && !key.modifiers.alt => {
                self.formula_move(1, 0, key.modifiers.shift, None, window, cx)
            }
            "up" => self.formula_move(0, -1, key.modifiers.shift, None, window, cx),
            "down" => self.formula_move(0, 1, key.modifiers.shift, None, window, cx),
            "home" => self.formula_move(0, 0, key.modifiers.shift, Some(false), window, cx),
            "end" => self.formula_move(0, 0, key.modifiers.shift, Some(true), window, cx),
            _ => false,
        }
    }
    pub(super) fn formula_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input_generation = self.input_generation.wrapping_add(1);
        self.document.update(cx, |document, _| {
            document.pending_input_generation = document.pending_input_generation.wrapping_add(1);
        });
        if let Some(session) = &mut self.formula {
            session.generation = session.generation.wrapping_add(1);
            session.refresh_geometry(true);
        }
        self.invalidate_formula_blocks();
        self.schedule_formula_render(window, cx);
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }
    pub(super) fn schedule_formula_render(&mut self, window: &Window, cx: &mut Context<Self>) {
        let style = super::super::math::Style::new(cx.theme().foreground, window.rem_size());
        let Some(session) = &mut self.formula else {
            return;
        };
        if session.style != Some(style) {
            session.style = Some(style);
            session.generation = session.generation.wrapping_add(1);
        }
        if self.formula_render.is_some() {
            return;
        }
        self.spawn_formula_render(cx);
    }
    fn spawn_formula_render(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.formula else {
            return;
        };
        let Some(style) = session.style else {
            return;
        };
        if session.rendered_generation == Some(session.generation) {
            return;
        }
        let session_id = session.id;
        let generation = session.generation;
        let revision = session.revision;
        let range = session.range.clone();
        let identity = self.document.entity_id();
        let mut formula = session
            .formula
            .with_tex(session.seed.preview(&session.root));
        formula.block = true; // The public geometry API uses display-style em units.
        self.formula_render = Some(cx.spawn(async move |view, cx| {
            let output = cx
                .background_executor()
                .spawn(async move { super::super::math::render(&formula, style) })
                .await;
            let _ = view.update(cx, |view, cx| {
                view.formula_render = None;
                let Some(session) = &mut view.formula else {
                    return;
                };
                if view.document.entity_id() != identity
                    || session.id != session_id
                    || session.revision != revision
                    || session.range != range
                {
                    return;
                }
                if session.generation == generation && session.style == Some(style) {
                    session.rendered_generation = Some(generation);
                    session.render_error = match &output {
                        super::super::math::Output::Failed(error) => Some(error.clone()),
                        _ => None,
                    };
                    // Failure retains the previous usable raster, but never alters the model.
                    if matches!(output, super::super::math::Output::Ready { .. })
                        || session.output.is_none()
                    {
                        session.output = Some(output);
                    }
                    view.invalidate_formula_blocks();
                    cx.notify();
                } else {
                    view.spawn_formula_render(cx);
                }
            });
        }));
    }
    pub(super) fn formula_pointer(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self
            .formula
            .as_ref()
            .and_then(|s| s.frame)
            .is_some_and(|(_, frame, _)| frame.is_current(self, window, cx))
        {
            return false;
        }
        self.input_generation = self.input_generation.wrapping_add(1);
        let Some(session) = &mut self.formula else {
            return false;
        };
        let Some((bounds, _, font)) = session.frame else {
            return false;
        };
        if !bounds.contains(&event.position) {
            return false;
        }
        if !event.modifiers.shift {
            session.anchor = None;
        }
        let cursor = geometry::cursor_at(
            &session.root,
            f32::from(event.position.x - bounds.left()) as f64 / f32::from(font) as f64,
            f32::from(event.position.y - bounds.top()) as f64 / f32::from(font) as f64,
        );
        if event.modifiers.shift {
            session.anchor.get_or_insert_with(|| session.cursor.clone());
        }
        if event.click_count >= 2 {
            let rect = geometry::caret_rect(&session.root, &cursor);
            if let Some(rect) = rect {
                let (path, lo, hi) = if event.click_count >= 3 {
                    let (path, len) =
                        geometry::row_len_at(&session.root, rect.x, rect.y + rect.h / 2.);
                    (path, 0, len)
                } else {
                    geometry::span_at(&session.root, rect.x, rect.y + rect.h / 2.)
                };
                session.anchor = Some(Cursor {
                    path: path.clone(),
                    index: lo,
                });
                session.cursor = Cursor { path, index: hi };
            }
        } else {
            session.cursor = cursor;
        }
        session.anchor.get_or_insert_with(|| session.cursor.clone());
        session.dragging = true;
        session.refresh_geometry(false);
        self.invalidate_formula_blocks();
        self.dragging = false;
        self.focus.focus(window, cx);
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
        true
    }
    pub(super) fn formula_copy(
        &mut self,
        cut: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = &mut self.formula else {
            return false;
        };
        if let Some(anchor) = &session.anchor {
            let (path, lo, hi) = common_row_selection(anchor, &session.cursor);
            let cursor = Cursor { path, index: lo };
            let text = session
                .seed
                .selection_latex(cursor.row(&session.root), lo..hi);
            if lo != hi {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            if cut {
                session.delete_selection();
                self.formula_changed(window, cx);
            }
        }
        true
    }
    pub(super) fn formula_paste(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = &mut self.formula else {
            return false;
        };
        session.delete_selection();
        let row = lossless::Seed::parse(text).row;
        // Pasting TeX inserts actual structures, rather than typing escape syntax.
        let mut cursor = session.cursor.clone();
        for atom in row.atoms {
            let index = cursor.index;
            let path = cursor.path.clone();
            cursor.insert(&mut session.root, atom);
            cursor = Cursor {
                path,
                index: index + 1,
            };
        }
        session.cursor = cursor;
        self.formula_changed(window, cx);
        true
    }
    pub(super) fn formula_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let at = self
            .formula
            .as_ref()
            .map(|s| s.range.start + s.formula.insertion_offset());
        self.finish_formula(true, false, window, cx);
        if let Some(at) = at {
            self.state.selection = Selection::caret(at);
        }
        if self.live {
            self.toggle_mode(window, cx);
        }
        self.focus.focus(window, cx);
    }
    fn formula_palette(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(session) = &self.formula else { return Empty.into_any_element(); };
        let session_id = session.id;
        let mut palette = div().id("formula-symbol-palette").test_support().flex().flex_wrap().gap_1().w_64().max_h(rems(24.)).overflow_y_scroll();
        for &(glyph, name) in input::PALETTE {
            let label = crate::ui::i18n::t_fmt(L10nKey::EditorFormulaInsertSymbol, &[("symbol", name)]);
            palette = palette.child(Button::new(name).small().ghost().w_10().h_8().label(glyph)
                .tooltip(label.clone()).accessibility_label(label)
                .on_mouse_down(MouseButton::Left, |_, window, cx| { window.prevent_default(); cx.stop_propagation(); })
                .on_click(cx.listener(move |view, _, window, cx| {
                    if let Some(session) = view.formula.as_mut()
                        && session.id == session_id && session.preedit.is_empty()
                    {
                        session.command = Some(name.into());
                        session.completion_ix = 0;
                        session.commit_command();
                        view.formula_changed(window, cx);
                        view.formula_tools.palette_open = false;
                    }
                    view.focus.focus(window, cx);
                })));
        }
        palette.into_any_element()
    }
    pub(super) fn formula_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(session) = &self.formula else {
            return div().into_any_element();
        };
        let matrix = session
            .cursor
            .path
            .iter()
            .any(|step| matches!(step.slot, ratex_gpui::editor::cursor::Slot::Cell(..)));
        let mut bar = div()
            .id("markdown-formula-tools")
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_3()
            .py_1();
        for (id, key, action) in [
            (
                "formula-commit",
                L10nKey::EditorFormulaCommit,
                Box::new(CommitFormula) as Box<dyn Action>,
            ),
            (
                "formula-cancel",
                L10nKey::EditorFormulaCancel,
                Box::new(CancelFormula),
            ),
            (
                "formula-source",
                L10nKey::EditorFormulaSource,
                Box::new(EditFormulaSource),
            ),
        ] {
            bar = bar.child(
                Button::new(id)
                    .small()
                    .ghost()
                    .label(t(key))
                    .on_click(cx.listener(move |_, _, window, cx| {
                        window.dispatch_action(action.boxed_clone(), cx)
                    })),
            );
        }
        let owner = cx.weak_entity();
        let session_id = session.id;
        let content_owner = owner.clone();
        bar = bar.child(gpui_kit::component::popover::Popover::new(("formula-palette-popover", session_id))
            .open(self.formula_tools.palette_open)
            .on_open_change(move |open, _, cx| {
                let _ = owner.update(cx, |view, cx| {
                    if view.formula.as_ref().is_some_and(|session| session.id == session_id) {
                        view.formula_tools.palette_open = *open; cx.notify();
                    }
                });
            })
            .trigger(Button::new("formula-symbols").small().ghost().label(t(L10nKey::EditorFormulaSymbols)))
            .content(move |_, _, cx| content_owner.update(cx, |view, cx| {
                if view.formula.as_ref().is_some_and(|session| session.id == session_id) { view.formula_palette(cx) }
                else { Empty.into_any_element() }
            }).unwrap_or_else(|_| Empty.into_any_element())));
        if matrix {
            for (id, key, action) in [
                (
                    "formula-row-add",
                    L10nKey::EditorFormulaAddRow,
                    Box::new(MatrixAddRow) as Box<dyn Action>,
                ),
                (
                    "formula-row-remove",
                    L10nKey::EditorFormulaRemoveRow,
                    Box::new(MatrixRemoveRow),
                ),
                (
                    "formula-column-add",
                    L10nKey::EditorFormulaAddColumn,
                    Box::new(MatrixAddColumn),
                ),
                (
                    "formula-column-remove",
                    L10nKey::EditorFormulaRemoveColumn,
                    Box::new(MatrixRemoveColumn),
                ),
            ] {
                bar = bar.child(Button::new(id).small().ghost().label(t(key)).on_click(
                    cx.listener(move |_, _, window, cx| {
                        window.dispatch_action(action.boxed_clone(), cx)
                    }),
                ));
            }
        }
        if let Some(command) = &session.command {
            bar = bar.child(format!("\\{command}"));
        }
        let matches = session.completions();
        let selected = session.completion_ix.min(matches.len().saturating_sub(1));
        for (ix, name) in matches
            .iter()
            .enumerate()
            .skip(selected.saturating_sub(3))
            .take(8)
        {
            let name = *name;
            let session_id = session.id;
            bar = bar.child(
                Button::new(SharedString::from(format!("formula-complete-{name}")))
                    .small()
                    .ghost()
                    .label(format!("\\{name}"))
                    .selected(ix == selected)
                    .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                        if *hovered {
                            if let Some(session) = &mut view.formula {
                                if session.id == session_id
                                    && session.preedit.is_empty()
                                    && session.completion_ix != ix
                                {
                                    session.completion_ix = ix;
                                    cx.notify();
                                }
                            }
                        }
                    }))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        if view.formula.as_mut().is_some_and(|session| {
                            session.id == session_id && session.accept_completion(name)
                        }) {
                            view.formula_changed(window, cx);
                        }
                        view.focus.focus(window, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, window, cx| {
                            if view.formula.as_mut().is_some_and(|session| {
                                session.id == session_id && session.accept_completion(name)
                            }) {
                                view.formula_changed(window, cx);
                            }
                            view.focus.focus(window, cx);
                            cx.stop_propagation();
                        }),
                    ),
            );
        }
        bar = bar.child(
            Button::new("formula-context-menu")
                .small()
                .ghost()
                .label(t(L10nKey::EditorFormulaMenu))
                .on_click(
                    cx.listener(|view, _, window, cx| view.open_formula_menu(None, window, cx)),
                ),
        );
        bar = bar.child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(t(L10nKey::EditorFormulaHelp)),
        );
        if let Some(error) = &session.render_error {
            bar = bar.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(format!("{} {error}", t(L10nKey::EditorMarkdownMathError))),
            );
        }
        bar.into_any_element()
    }
    pub(super) fn formula_matrix(
        &mut self,
        operation: u8,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = &mut self.formula else {
            return;
        };
        session.anchor = None;
        match operation {
            0 => session.cursor.matrix_add_row(&mut session.root),
            1 => session.cursor.matrix_remove_row(&mut session.root),
            2 => session.cursor.matrix_add_col(&mut session.root),
            _ => session.cursor.matrix_remove_col(&mut session.root),
        }
        self.focus.focus(window, cx);
        self.formula_changed(window, cx);
    }
    pub(super) fn formula_preedit(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = &mut self.formula else {
            return false;
        };
        let start = range
            .as_ref()
            .map_or(0, |range| utf16_boundary(&session.preedit, range.start));
        let end = range.as_ref().map_or(session.preedit.len(), |range| {
            utf16_boundary(&session.preedit, range.end)
        });
        if start > end {
            return true;
        }
        let offset = session.preedit[..start].encode_utf16().count();
        session.preedit.replace_range(start..end, text);
        let len = text.encode_utf16().count();
        let selected = selected.unwrap_or(len..len);
        session.preedit_selection =
            offset + selected.start.min(len)..offset + selected.end.min(len);
        self.input_generation = self.input_generation.wrapping_add(1);
        self.invalidate_formula_blocks();
        window.notify_text_input_changed(&self.focus);
        cx.notify();
        true
    }
    pub(super) fn formula_commit_text(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = &mut self.formula else {
            return false;
        };
        if let Some(range) = range {
            if range.end > session.preedit.encode_utf16().count() {
                return true;
            }
            let start = utf16_boundary(&session.preedit, range.start);
            let end = utf16_boundary(&session.preedit, range.end);
            if start > end {
                return true;
            }
            let mut committed = std::mem::take(&mut session.preedit);
            committed.replace_range(start..end, text);
            session.insert_text(&committed);
        } else {
            session.insert_text(text);
        }
        self.formula_changed(window, cx);
        true
    }
    pub(super) fn formula_unmark(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(session) = &mut self.formula else {
            return false;
        };
        if session.preedit.is_empty() {
            return true;
        }
        let text = std::mem::take(&mut session.preedit);
        session.insert_text(&text);
        self.formula_changed(window, cx);
        true
    }
    pub(super) fn formula_native_text(
        &self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
    ) -> Option<String> {
        let session = self.formula.as_ref()?;
        let start = utf16_boundary(&session.preedit, range.start);
        let end = utf16_boundary(&session.preedit, range.end.max(range.start));
        *adjusted = Some(
            session.preedit[..start].encode_utf16().count()
                ..session.preedit[..end].encode_utf16().count(),
        );
        Some(session.preedit[start..end].into())
    }
    pub(super) fn formula_native_selection(&self) -> Option<UTF16Selection> {
        self.formula.as_ref().map(|session| UTF16Selection {
            range: session.preedit_selection.clone(),
            reversed: false,
        })
    }
    pub(super) fn formula_native_marked(&self) -> Option<Range<usize>> {
        self.formula
            .as_ref()
            .filter(|s| !s.preedit.is_empty())
            .map(|s| 0..s.preedit.encode_utf16().count())
    }
    pub(super) fn formula_native_bounds(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<Bounds<Pixels>> {
        let session = self.formula.as_ref()?;
        let mut bounds = session.caret_bounds(self, window, cx)?;
        if !session.preedit.is_empty() {
            let end = utf16_boundary(&session.preedit, session.preedit_selection.end);
            let font = Font {
                family: cx.theme().font_family.clone(),
                ..Font::default()
            };
            let size = session.frame?.2;
            bounds.origin.x += px(crate::ui::tab_strip::measure_text(
                window.text_system(),
                &font,
                f32::from(size),
                &session.preedit[..end],
            ));
        }
        Some(bounds)
    }
    pub(super) fn formula_native_length(&self) -> Option<usize> {
        self.formula
            .as_ref()
            .map(|s| s.preedit.encode_utf16().count())
    }
    pub(super) fn formula_native_select(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = &mut self.formula else {
            return false;
        };
        let len = session.preedit.encode_utf16().count();
        session.preedit_selection = range.start.min(len)..range.end.min(len);
        cx.notify();
        true
    }
    pub(super) fn release_formula(
        &mut self,
        owner_id: EntityId,
        requested: Option<Selection>,
        cx: &mut App,
    ) -> Option<Selection> {
        let Some(mut session) = self.formula.take() else {
            return requested;
        };
        self.formula_tools.palette_open = false;
        self.invalidate_formula_cell(&session.range);
        self.math.editor = None;
        self.formula_render = None;
        self.invalidate_formula_blocks();
        let text = std::mem::take(&mut session.preedit);
        session.insert_text(&text);
        session.commit_command();
        self.document.update(cx, |document, cx| {
            if document
                .composition_owner
                .as_ref()
                .is_some_and(|owner| owner.entity_id() == owner_id)
            {
                document.composition_owner = None;
            }
            let valid = document.revision() == session.revision
                && session.range.end <= document.source().len()
                && document.source().slice(session.range.clone()).to_string() == session.original;
            let edits = if valid {
                session.edits(document.source())
            } else {
                Vec::new()
            };
            if !edits.is_empty() {
                let transaction = document.command(
                    &mut self.state,
                    Command::BatchReplace {
                        edits,
                        kind: EditKind::Command,
                    },
                );
                let selected = requested.map(|selection| transaction.map_selection(selection));
                cx.emit(DocumentEvent::Edited(transaction));
                cx.notify();
                selected
            } else {
                requested
            }
        })
    }
}

fn minimal_edit(old: &str, new: &str) -> (Range<usize>, String) {
    let mut start = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(start) || !new.is_char_boundary(start) {
        start -= 1;
    }
    let mut suffix = old[start..]
        .bytes()
        .rev()
        .zip(new[start..].bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    (
        start..old.len() - suffix,
        new[start..new.len() - suffix].into(),
    )
}

pub(in crate::ui::code_editor::markdown) fn render(
    editor: &Editor,
    font: Pixels,
    _: &mut Window,
    cx: &mut App,
) -> Option<gpui_kit::base::text::InlineElement> {
    let width = font * editor.extent.0.max(0.5);
    let height = font * editor.extent.1;
    let baseline = font * editor.extent.2;
    let mut child = div()
        .id("markdown-formula-editor")
        .test_support()
        .role(Role::TextInput)
        .aria_label(t(L10nKey::EditorFormulaEditing))
        .relative()
        .w(width)
        .h(height.max(font));
    if let Some(super::super::math::Output::Ready {
        image,
        width,
        height,
        ..
    }) = &editor.output
    {
        child = child.child(img(image.clone()).w(font * *width).h(font * *height));
    } else {
        child = child.child(t(L10nKey::EditorMarkdownMathLoading));
    }
    if !editor.preedit.is_empty() {
        if let Some(caret) = editor.caret {
            child = child.child(
                div()
                    .absolute()
                    .left(font * caret.x as f32)
                    .top(font * caret.y as f32)
                    .font_family(cx.theme().font_family.clone())
                    .text_size(font)
                    .bg(cx.theme().background)
                    .border_b_1()
                    .border_color(cx.theme().caret)
                    .child(editor.preedit.clone()),
            );
        }
    }
    Some(
        gpui_kit::base::text::InlineElement::new(Surface {
            view: editor.view.clone(),
            session: editor.session,
            child: child.into_any_element(),
            font,
        })
        .with_baseline(baseline),
    )
}

struct Surface {
    view: WeakEntity<SourcePresentation>,
    session: u64,
    child: AnyElement,
    font: Pixels,
}
impl IntoElement for Surface {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Surface {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
        let _ = self.view.update(cx, |view, cx| {
            let frame = composition::Frame::new(view, window, cx);
            if let Some(session) = &mut view.formula {
                if session.id == self.session {
                    session.frame = Some((bounds, frame, self.font));
                }
            }
        });
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
        let Some(view) = self.view.upgrade() else {
            return;
        };
        let current = view.read(cx);
        let Some(session) = &current.formula else {
            return;
        };
        if session.id != self.session {
            return;
        }
        if let Some(rect) = session.selection_cache {
            let rect = Bounds::new(
                bounds.origin + point(self.font * rect.x as f32, self.font * rect.y as f32),
                size(self.font * rect.w as f32, self.font * rect.h as f32),
            );
            window.paint_quad(fill(rect, cx.theme().selection.opacity(0.45)));
        }
        if current.focus.is_focused(window) {
            if let Some(caret) = session.caret_bounds(current, window, cx) {
                window.paint_quad(fill(caret, cx.theme().caret));
            }
        }
        let weak = self.view.clone();
        let session_id = self.session;
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if !phase.bubble() {
                return;
            }
            let _ = weak.update(cx, |view, cx| {
                let Some(session) = &mut view.formula else {
                    return;
                };
                if session.id != session_id {
                    return;
                }
                if !session.dragging {
                    return;
                }
                if event.pressed_button != Some(MouseButton::Left) {
                    session.dragging = false;
                    return;
                }
                let Some((bounds, _, font)) = session.frame else {
                    return;
                };
                session.cursor = geometry::cursor_at(
                    &session.root,
                    f32::from(event.position.x - bounds.left()) as f64 / f32::from(font) as f64,
                    f32::from(event.position.y - bounds.top()) as f64 / f32::from(font) as f64,
                );
                session.refresh_geometry(false);
                view.invalidate_formula_blocks();
                view.input_generation = view.input_generation.wrapping_add(1);
                window.notify_text_selection_changed(&view.focus);
                cx.notify();
            });
        });
    }
}

fn utf16_boundary(text: &str, target: usize) -> usize {
    let mut units = 0;
    for (at, ch) in text.char_indices() {
        if units + ch.len_utf16() > target {
            return at;
        }
        units += ch.len_utf16();
    }
    text.len()
}

#[cfg(test)]
#[path = "math_edit/tests.rs"]
mod tests;
