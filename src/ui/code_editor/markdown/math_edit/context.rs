//! Formula command surfaces retain only weak ownership and checked snapshots.
use super::*;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};

#[derive(Default)]
pub(in crate::ui::code_editor::markdown::source) struct Tools {
    pub(super) menu: Option<(Entity<PopupMenu>, Point<Pixels>)>,
    subscription: Option<Subscription>,
    pub(super) exporting: bool,
    error: Option<String>,
}
#[derive(Clone)]
struct Target {
    document: EntityId,
    presentation: EntityId,
    revision: u64,
    range: Range<usize>,
    original: String,
    formula: super::super::super::math::Formula,
    session: Option<(u64, u64)>,
}
impl Target {
    fn current(&self, view: &SourcePresentation, cx: &App) -> bool {
        let document = view.document.read(cx);
        self.document == view.document.entity_id() && self.revision == document.revision()
            && document.composition_owner.as_ref().is_none_or(|owner| owner.entity_id() == self.presentation)
            && self.range.end <= document.source().len()
            && document.source().slice(self.range.clone()).to_string() == self.original
            && self.session.is_none_or(|(id, generation)| view.formula.as_ref().is_some_and(|s| s.id == id && s.generation == generation && s.preedit.is_empty()))
    }
}
impl SourcePresentation {
    fn formula_target(&self, cx: &Context<Self>) -> Option<Target> {
        if !self.live || self.state.composition.is_some() || self.formula_tools.exporting { return None; }
        let document = self.document.read(cx);
        if document.composition_owner.as_ref().is_some_and(|owner| owner.entity_id() != cx.entity_id()) { return None; }
        let (range, formula, session) = if let Some(session) = &self.formula {
            if !session.preedit.is_empty() { return None; }
            (session.range.clone(), session.formula.with_tex(session.latex()), Some((session.id, session.generation)))
        } else {
            let (range, formula) = self.formula_at(self.state.selection.head(), false, cx)
                .or_else(|| self.formula_at(self.state.selection.head(), true, cx))?;
            (range, formula, None)
        };
        Some(Target { document: self.document.entity_id(), presentation: cx.entity_id(), revision: document.revision(), original: document.source().slice(range.clone()).to_string(), range, formula, session })
    }
    pub(in crate::ui::code_editor::markdown::source) fn formula_completion_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(session) = &mut self.formula else { return false; };
        if !session.preedit.is_empty() { return false; }
        let matches = session.completions();
        if matches.is_empty() { return false; }
        let key = &event.keystroke;
        if key.modifiers.control || key.modifiers.platform || key.modifiers.alt || key.modifiers.shift { return false; }
        match key.key.as_str() {
            "up" => session.completion_ix = (session.completion_ix + matches.len() - 1) % matches.len(),
            "down" => session.completion_ix = (session.completion_ix + 1) % matches.len(),
            "enter" => { let name = matches[session.completion_ix.min(matches.len() - 1)]; session.accept_completion(name); self.formula_changed(window, cx); return true; }
            "escape" => { session.command = None; self.formula_changed(window, cx); return true; }
            _ => return false,
        }
        cx.notify(); true
    }
    pub(in crate::ui::code_editor::markdown::source) fn formula_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &mut self.formula else { return; };
        if !session.preedit.is_empty() { return; }
        let matches = session.completions();
        if let Some(name) = matches.get(session.completion_ix.min(matches.len().saturating_sub(1))) {
            session.accept_completion(name); self.formula_changed(window, cx);
        } else if session.command.is_some() { session.commit_command(); self.formula_changed(window, cx); }
        else { self.finish_formula(true, false, window, cx); }
    }
    pub(in crate::ui::code_editor::markdown::source) fn formula_alignment(&self, at: usize, fallback: Align) -> Align {
        self.formula.as_ref().filter(|s| s.range.start == at).and_then(|s| s.alignment).unwrap_or(fallback)
    }
    pub(in crate::ui::code_editor::markdown::source) fn align_formula(&mut self, align: Align, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &mut self.formula else { return; };
        if !session.formula.block || !session.preedit.is_empty() { return; }
        session.alignment = Some(align);
        self.focus.focus(window, cx); self.formula_changed(window, cx);
    }
    pub(in crate::ui::code_editor::markdown::source) fn copy_formula_latex(&mut self, cx: &mut Context<Self>) {
        if let Some(target) = self.formula_target(cx) { if target.current(self, cx) { cx.write_to_clipboard(ClipboardItem::new_string(target.formula.tex)); } }
    }
    pub(in crate::ui::code_editor::markdown::source) fn formula_context_at(&mut self, point: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.live || !self.geometry_is_current(cx) || self.state.composition.is_some() || self.formula.as_ref().is_some_and(|s| !s.preedit.is_empty()) { return false; }
        let hit = self.formula.as_ref().and_then(|session| session.frame.filter(|(bounds, frame, _)| bounds.contains(&point) && frame.is_current(self, window, cx)).map(|_| session.range.start))
            .or_else(|| self.geometry.as_ref().and_then(|(_, geometry)| self.projection.leaves.iter().find_map(|leaf| {
                let geometry = geometry.get(&leaf.id)?;
                let atom = geometry.atom_at(point)?;
                let at = leaf.atom_source(atom.start)?;
                leaf.formula_at(at, false).map(|(range, _)| range.start)
            })));
        let Some(at) = hit else { return false; };
        if self.formula.as_ref().is_some_and(|s| s.range.start != at) { return false; }
        if self.formula.is_none() { self.state.selection = Selection::caret(at); }
        self.open_formula_menu(Some(point), window, cx); true
    }
    pub(in crate::ui::code_editor::markdown::source) fn formula_menu(&self, menu: PopupMenu) -> PopupMenu {
        let at = self.state.selection.head();
        let applicable = self.live && (self.formula.is_some() || self.projection.leaves.iter().any(|leaf| leaf.formula_at(at, false).or_else(|| leaf.formula_at(at, true)).is_some()));
        menu.separator().menu_with_disabled(t(L10nKey::EditorFormulaMenu), Box::new(FormulaMenu), !applicable)
    }
    pub(in crate::ui::code_editor::markdown::source) fn open_formula_menu(&mut self, position: Option<Point<Pixels>>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.formula_target(cx) else { return; };
        let position = position.or_else(|| self.formula.as_ref().and_then(|s| s.caret_bounds(self, window, cx)).map(|bounds| bounds.bottom_left()))
            .or_else(|| self.caret_bounds(self.state.selection.head(), cx).map(|bounds| bounds.bottom_left()));
        let Some(position) = position else { return; };
        let owner = cx.entity().downgrade();
        let alignable = self.formula.as_ref().is_some_and(|s| s.formula.block);
        let align = self.formula_alignment(target.range.start, self.projection.leaves.iter().find(|leaf| leaf.range.start == target.range.start).map_or(Align::Center, |leaf| leaf.math_alignment));
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            menu = menu.label(t(L10nKey::EditorFormulaMenu));
            for (key, operation) in [(L10nKey::EditorFormulaCopyLatex, 0), (L10nKey::EditorFormulaExportPng, 1), (L10nKey::EditorFormulaExportSvg, 2)] {
                let owner = owner.clone(); let target = target.clone();
                menu = menu.item(PopupMenuItem::new(t(key)).on_click(move |_, window, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        if !target.current(view, cx) { return; }
                        if operation == 0 { cx.write_to_clipboard(ClipboardItem::new_string(target.formula.tex.clone())); }
                        else { view.export_formula_target(target.clone(), operation == 1, window, cx); }
                    });
                }));
            }
            if alignable {
                menu = menu.separator();
                for (key, value) in [(L10nKey::EditorFormulaAlignLeft, Align::Left), (L10nKey::EditorFormulaAlignCenter, Align::Center), (L10nKey::EditorFormulaAlignRight, Align::Right)] {
                    let owner = owner.clone(); let target = target.clone();
                    menu = menu.item(PopupMenuItem::new(t(key)).checked(align == value).on_click(move |_, window, cx| {
                        let _ = owner.update(cx, |view, cx| { if target.current(view, cx) { view.align_formula(value, window, cx); } });
                    }));
                }
            }
            menu
        });
        self.formula_tools.subscription = Some(cx.subscribe_in(&menu, window, |view, menu, _: &DismissEvent, window, cx| {
            let restore = menu.read(cx).focus_handle(cx).contains_focused(window, cx);
            view.formula_tools.menu = None;
            if restore { view.focus.focus(window, cx); }
            cx.notify();
        }));
        self.formula_tools.menu = Some((menu.clone(), position));
        menu.read(cx).focus_handle(cx).focus(window, cx); cx.notify();
    }
    pub(in crate::ui::code_editor::markdown::source) fn export_formula(&mut self, png: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(target) = self.formula_target(cx) { self.export_formula_target(target, png, window, cx); }
    }
    fn export_formula_target(&mut self, target: Target, png: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.formula_tools.exporting || !target.current(self, cx) { return; }
        self.formula_tools.exporting = true; self.formula_tools.error = None;
        let style = super::super::super::math::Style::new(cx.theme().foreground, window.rem_size());
        // This OS dialog is explicitly a local-machine export, even for SSH
        // documents. Never resolve the owning document's remote path with fs.
        let directory = crate::ui::code_editor::dirs_home();
        let answer = cx.prompt_for_new_path(&directory, Some(if png { "formula.png" } else { "formula.svg" }));
        cx.spawn_in(window, async move |view, cx| {
            let result = answer.await;
            let path = match result {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => { let _ = view.update_in(cx, |view, window, cx| view.finish_formula_export(None, window, cx)); return; }
                error => { let message = format!("{error:?}"); let _ = view.update_in(cx, |view, window, cx| view.finish_formula_export(Some(message), window, cx)); return; }
            };
            let check = path.clone();
            let exists = cx.background_executor().spawn(async move { check.exists() }).await;
            if exists {
                let confirmation = view.update_in(cx, |_, window, cx| window.prompt(
                    PromptLevel::Warning,
                    &crate::ui::i18n::t_fmt(L10nKey::EditorReplaceExisting, &[("path", &path.display().to_string())]),
                    None, &crate::ui::confirm_answers(t(L10nKey::EditorOverwrite), t(L10nKey::Cancel)), cx));
                let accepted = match confirmation { Ok(answer) => matches!(answer.await, Ok(0)), Err(_) => false };
                if !accepted { let _ = view.update_in(cx, |view, window, cx| view.finish_formula_export(None, window, cx)); return; }
            }
            let current = view.update(cx, |view, cx| target.current(view, cx)).unwrap_or(false);
            if !current { let _ = view.update_in(cx, |view, window, cx| view.finish_formula_export(None, window, cx)); return; }
            let formula = target.formula.clone();
            let rendered = cx.background_executor().spawn(async move {
                let format = if png { super::super::super::math::Export::Png } else { super::super::super::math::Export::Svg };
                super::super::super::math::export(&formula, style)
            }).await;
            let bytes = match rendered {
                Ok(bytes) => bytes,
                Err(error) => { let _ = view.update_in(cx, |view, window, cx| view.finish_formula_export(Some(error.to_string()), window, cx)); return; }
            };
            if !view.update(cx, |view, cx| target.current(view, cx)).unwrap_or(false) {
                let _ = view.update_in(cx, |view, window, cx| view.finish_formula_export(None, window, cx)); return;
            }
            let result = cx.background_executor().spawn(async move {
                use std::io::Write as _;
                // A destination that appeared after the dialog must not silently
                // become an overwrite. Existing destinations were confirmed above.
                let mut file = std::fs::OpenOptions::new().write(true).create(exists).truncate(exists).create_new(!exists).open(&path)?;
                file.write_all(&bytes)
            }).await;
            let _ = view.update_in(cx, |view, window, cx| view.finish_formula_export(result.err().map(|error| error.to_string()), window, cx));
        }).detach();
        cx.notify();
    }
    fn finish_formula_export(&mut self, error: Option<String>, _: &mut Window, cx: &mut Context<Self>) {
        self.formula_tools.exporting = false; self.formula_tools.error = error;
        // The OS dialog and PopupMenu restore their own focus. A late render or
        // write result must not focus an old presentation over a newer preedit.
        cx.notify();
    }
    pub(in crate::ui::code_editor::markdown::source) fn formula_overlay(&self, window: &Window) -> Option<AnyElement> {
        self.formula_tools.menu.as_ref().map(|(menu, position)| deferred(anchored().position(*position)
            .snap_to_window_with_margin(window.rem_size() * 0.5).child(menu.clone())).into_any_element())
    }
    pub(in crate::ui::code_editor::markdown::source) fn formula_feedback(&self, cx: &App) -> AnyElement {
        let mut result = div().id("markdown-formula-export-status").test_support().role(Role::Status).text_sm().px_3();
        if self.formula_tools.exporting { result = result.child(div().id("markdown-formula-exporting").test_support().child(t(L10nKey::EditorFormulaExporting))); }
        if let Some(error) = &self.formula_tools.error {
            let message = format!("{} {error}", t(L10nKey::EditorFormulaExportFailed));
            result = result.child(div().id("markdown-formula-export-error").test_support().aria_label(message.clone()).text_color(cx.theme().danger).child(message));
        }
        result.into_any_element()
    }
}
