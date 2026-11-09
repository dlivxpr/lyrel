//! Native block commands and per-presentation disclosure. No text/history owner.
use super::super::blocks::{self, Fold, Kind as BlockKind};
use super::*;
use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenu},
};
use std::collections::BTreeMap;

actions!(
    markdown_blocks,
    [
        ToParagraph,
        ToHeading1,
        ToHeading2,
        ToHeading3,
        ToHeading4,
        ToHeading5,
        ToHeading6,
        ToQuote,
        ToCallout,
        ToTip,
        ToImportant,
        ToWarning,
        ToCaution,
        ToBullet,
        ToNumbered,
        ToTask,
        ToCodeBlock,
        ToMathBlock,
        ToggleFold,
        PersistCalloutFold,
        ExitQuote
    ]
);

pub(crate) fn bindings(cx: &App) -> Vec<KeyBinding> {
    let bind = |keys: &str, action: Box<dyn Action>| {
        let predicate = KeyBindingContextPredicate::parse("Markdown")
            .expect("static block context")
            .into();
        KeyBinding::load(keys, action, Some(predicate), false, None, cx)
            .expect("static block shortcut")
    };
    let modifier = if cfg!(target_os = "macos") {
        "cmd-alt"
    } else {
        "ctrl-alt"
    };
    vec![
        bind(&format!("{modifier}-0"), Box::new(ToParagraph)),
        bind(&format!("{modifier}-1"), Box::new(ToHeading1)),
        bind(&format!("{modifier}-2"), Box::new(ToHeading2)),
        bind(&format!("{modifier}-3"), Box::new(ToHeading3)),
        bind(&format!("{modifier}-4"), Box::new(ToHeading4)),
        bind(&format!("{modifier}-5"), Box::new(ToHeading5)),
        bind(&format!("{modifier}-6"), Box::new(ToHeading6)),
        bind(&format!("{modifier}-q"), Box::new(ToQuote)),
        bind(&format!("{modifier}-n"), Box::new(ToCallout)),
        KeyBinding::load(
            &format!("{modifier}-f"),
            Box::new(ToggleFold),
            Some(
                KeyBindingContextPredicate::parse("Markdown && !MarkdownTable")
                    .expect("static fold context")
                    .into(),
            ),
            false,
            None,
            cx,
        )
        .expect("static fold shortcut"),
        bind(&format!("{modifier}-shift-f"), Box::new(PersistCalloutFold)),
        bind(&format!("{modifier}-enter"), Box::new(ExitQuote)),
    ]
}

#[derive(Default)]
pub(super) struct State {
    overrides: BTreeMap<usize, bool>,
    pub(super) hidden: Vec<Range<usize>>,
}
impl State {
    fn closed(&self, fold: &Fold) -> bool {
        self.overrides
            .get(&fold.header.start)
            .copied()
            .unwrap_or(fold.persisted.unwrap_or(false))
    }
    fn rebuild(&mut self, index: &blocks::Index) {
        self.hidden.clear();
        let overrides = &self.overrides;
        self.hidden.extend(
            index
                .folds
                .iter()
                .filter(|fold| {
                    overrides
                        .get(&fold.header.start)
                        .copied()
                        .unwrap_or(fold.persisted.unwrap_or(false))
                })
                .map(|fold| fold.body.clone()),
        );
        self.hidden.sort_by_key(|range| range.start);
        let mut write = 0;
        for read in 0..self.hidden.len() {
            if write > 0 && self.hidden[read].start <= self.hidden[write - 1].end {
                self.hidden[write - 1].end = self.hidden[write - 1].end.max(self.hidden[read].end);
            } else {
                self.hidden[write] = self.hidden[read].clone();
                write += 1;
            }
        }
        self.hidden.truncate(write);
    }
    pub(super) fn is_hidden(&self, at: usize) -> bool {
        let ix = self.hidden.partition_point(|range| range.start <= at);
        ix > 0 && at < self.hidden[ix - 1].end
    }
    pub(super) fn rebase(&mut self, transaction: &Transaction) {
        self.overrides = std::mem::take(&mut self.overrides)
            .into_iter()
            .map(|(at, closed)| {
                (
                    transaction.map_selection(Selection::caret(at)).head(),
                    closed,
                )
            })
            .collect();
    }
}

impl SourcePresentation {
    fn refresh_block_index(&mut self, cx: &App) {
        if !self.live || self.projection.block_index.ready {
            return;
        }
        if let Some(syntax) = self.format_syntax(cx) {
            self.projection.block_index = blocks::Index::from_ast(syntax.source(), syntax.root());
        }
    }
    pub(super) fn reveal_block_command(&mut self, command: &Command, cx: &App) {
        if !self.live || self.folds.hidden.is_empty() {
            return;
        }
        let target = match command {
            Command::Select(selection) => Some(selection.range()),
            Command::Replace { range, .. }
            | Command::Compose {
                range: Some(range), ..
            } => Some(range.clone()),
            _ => None,
        };
        let Some(target) = target else {
            return;
        };
        let mut changed = false;
        for fold in &self.projection.block_index.folds {
            if target.start <= fold.body.end
                && fold.body.start <= target.end
                && self.folds.closed(fold)
            {
                self.folds.overrides.insert(fold.header.start, false);
                changed = true;
            }
        }
        if changed {
            self.folds.rebuild(&self.projection.block_index);
            self.sync_mdx_inline_visibility(cx);
            self.viewport.layout = None;
            self.geometry = None;
            self.rendered_revision = None;
        }
    }
    pub(super) fn block_enter(
        &mut self,
        hard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.live
            || self.is_composing()
            || !self.extra_selections.is_empty()
            || self.projection.revision != self.document.read(cx).revision()
        {
            return false;
        }
        let range = self.state.selection.range();
        let Some(leaf) = self.projection.leaf_at(range.start) else {
            return false;
        };
        if !matches!(
            leaf.kind,
            Kind::Quote(_) | Kind::Heading(_) | Kind::Prose | Kind::Source
        ) {
            return false;
        }
        let quoted = matches!(leaf.kind, Kind::Quote(_));
        let source = self.document.read(cx).source();
        let row = source.offset_to_point(range.start).row;
        let start = source.line_start_offset(row);
        let end = source.line_end_offset(row);
        if range.end > end {
            return false;
        }
        let line = source.slice(start..end).to_string();
        if !quoted {
            let trimmed = line.trim_start();
            let hashes = trimmed.bytes().take_while(|byte| *byte == b'#').count();
            if range.is_empty() && (1..=6).contains(&hashes) && trimmed[hashes..].trim().is_empty()
            {
                self.command(
                    Command::Replace {
                        range: start..end,
                        text: String::new(),
                        kind: EditKind::Command,
                    },
                    window,
                    cx,
                );
                return true;
            }
            return false;
        }
        let mut prefix_end = 0;
        let mut last_quote = None;
        for (ix, ch) in line.char_indices() {
            if ch == '>' {
                last_quote = Some(ix);
                prefix_end = ix + 1;
            } else if ch == ' ' || ch == '\t' {
                prefix_end = ix + ch.len_utf8();
            } else {
                break;
            }
        }
        let Some(last_quote) = last_quote else {
            return false;
        };
        let command = if line[prefix_end..].is_empty() && range.is_empty() {
            Command::Replace {
                range: start + last_quote..start + prefix_end,
                text: String::new(),
                kind: EditKind::Command,
            }
        } else {
            let prefix = &line[..prefix_end];
            Command::Replace {
                range,
                text: format!("{}\n{prefix}", if hard { "  " } else { "" }),
                kind: EditKind::Command,
            }
        };
        self.command(command, window, cx);
        true
    }
    pub(super) fn sync_block_folds(&mut self) {
        if !self.live {
            self.folds.hidden.clear();
            return;
        }
        for fold in &self.projection.block_index.folds {
            let touched = std::iter::once(&self.state.selection)
                .chain(self.extra_selections.iter())
                .any(|selection| {
                    let range = selection.range();
                    if range.is_empty() {
                        fold.body.start <= range.start && range.start <= fold.body.end
                    } else {
                        range.start < fold.body.end && fold.body.start < range.end
                    }
                });
            if touched && self.folds.closed(fold) {
                self.folds.overrides.insert(fold.header.start, false);
                self.geometry = None;
                self.viewport.layout = None;
                self.rendered_revision = None;
            }
        }
        self.folds.rebuild(&self.projection.block_index);
    }
    fn current_block(&self, cx: &App) -> Option<blocks::Block> {
        if !self.live
            || self.is_composing()
            || !self.extra_selections.is_empty()
            || self.projection.revision != self.document.read(cx).revision()
        {
            return None;
        }
        let source = self.document.read(cx).source();
        let at = self.state.selection.head();
        self.projection
            .block_index
            .at(at)
            .cloned()
            .map(|mut block| {
                if matches!(
                    block.kind,
                    BlockKind::Text | BlockKind::Bullet | BlockKind::Numbered | BlockKind::Task
                ) {
                    let row = source.offset_to_point(at).row;
                    block.range = source.line_start_offset(row)..source.line_end_offset(row);
                }
                block
            })
            .or_else(|| {
                let leaf = self.projection.leaf_at(at)?;
                matches!(leaf.kind, Kind::Prose | Kind::Source).then(|| blocks::Block {
                    range: leaf.range.clone(),
                    kind: BlockKind::Text,
                })
            })
    }
    fn convert_block(&mut self, kind: BlockKind, window: &mut Window, cx: &mut Context<Self>) {
        self.command(Command::FinishComposition, window, cx);
        self.refresh_block_index(cx);
        if let Some(block) = self.current_block(cx) {
            let rope = self.document.read(cx).source();
            let mut range = block.range.clone();
            if range.end < rope.len()
                && rope.slice(range.end..range.end + 1).chars().next() == Some('\n')
            {
                range.end += 1;
            }
            let source = rope.slice(range).to_string();
            let base = block.range.start;
            let local = blocks::Block {
                range: 0..source.len(),
                kind: block.kind,
            };
            if let Some(conversion) = blocks::convert(
                &source,
                &local,
                kind,
                self.state.selection.head().saturating_sub(base),
            ) {
                self.command(conversion.command(base), window, cx);
            }
        }
        self.focus.focus(window, cx);
    }
    fn fold_at(&self, at: usize) -> Option<&Fold> {
        self.projection
            .block_index
            .folds
            .iter()
            .filter(|fold| fold.header.start <= at && at <= fold.body.end)
            .min_by_key(|fold| fold.body.end - fold.header.start)
    }
    fn toggle_block_fold(
        &mut self,
        at: usize,
        persist: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.live || self.projection.revision != self.document.read(cx).revision() {
            return;
        }
        self.command(Command::FinishComposition, window, cx);
        self.refresh_block_index(cx);
        let fold = if persist {
            let Some(block) = self
                .current_block(cx)
                .filter(|block| matches!(block.kind, BlockKind::Callout(_)))
            else {
                return;
            };
            self.projection
                .block_index
                .folds
                .iter()
                .find(|fold| fold.header.start == block.range.start)
        } else {
            self.fold_at(at)
        };
        let Some(fold) = fold.cloned() else {
            return;
        };
        let closed = !self.folds.closed(&fold);
        if closed {
            // Finish preedit and retire every invisible input/drag target before
            // removing body leaves from both layout and keyboard navigation.
            self.extra_selections.clear();
            self.dragging = false;
            self.drag_origin = None;
            self.drag_pointer = None;
            self.command(
                Command::Select(Selection::caret(fold.header.end)),
                window,
                cx,
            );
        }
        if persist {
            let source = self
                .document
                .read(cx)
                .source()
                .slice(fold.header.clone())
                .to_string();
            if !self.projection.block_index.blocks.iter().any(|block| {
                block.range.start == fold.header.start
                    && matches!(block.kind, BlockKind::Callout(_))
            }) {
                return;
            }
            let Some(end) = source.find(']') else {
                return;
            };
            let range = fold
                .marker
                .clone()
                .unwrap_or(fold.header.start + end + 1..fold.header.start + end + 1);
            self.command(
                Command::Replace {
                    range,
                    text: if closed { "-" } else { "+" }.into(),
                    kind: EditKind::Command,
                },
                window,
                cx,
            );
        }
        if persist {
            self.folds.overrides.remove(&fold.header.start);
        } else {
            self.folds.overrides.insert(fold.header.start, closed);
        }
        self.folds.rebuild(&self.projection.block_index);
        self.geometry = None;
        self.viewport.layout = None;
        self.rendered_revision = None;
        self.focus.focus(window, cx);
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }
    fn exit_quote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(block) = self
            .current_block(cx)
            .filter(|block| matches!(block.kind, BlockKind::Quote | BlockKind::Callout(_)))
        else {
            return;
        };
        let source = self.document.read(cx).source();
        let end = block.range.end;
        let text = if end == source.len() { "\n\n" } else { "\n" };
        self.command(
            Command::BatchReplaceSelected {
                kind: EditKind::Command,
                edits: vec![(end..end, text.into())],
                selection: Selection::caret(end + text.len()),
            },
            window,
            cx,
        );
        self.focus.focus(window, cx);
    }
    pub(super) fn decorate_block(&self, at: usize, content: AnyElement, cx: &App) -> AnyElement {
        if !self.live {
            return content;
        }
        let Some(block) = self.projection.block_index.blocks.iter().find(|block| {
            block.range.start <= at
                && at <= block.range.end
                && matches!(block.kind, BlockKind::Quote | BlockKind::Callout(_))
        }) else {
            return content;
        };
        let color = match block.kind {
            BlockKind::Callout("TIP") => cx.theme().success,
            BlockKind::Callout("IMPORTANT") => cx.theme().info,
            BlockKind::Callout("WARNING") => cx.theme().warning,
            BlockKind::Callout("CAUTION") => cx.theme().danger,
            _ => cx.theme().muted_foreground,
        };
        div()
            .w_full()
            .min_w_0()
            .border_l_2()
            .border_color(color)
            .pl_3()
            .pr_2()
            .when(block.range.start == at, |view| {
                let key = match block.kind {
                    BlockKind::Callout("NOTE") => Some(L10nKey::MarkdownBlockNote),
                    BlockKind::Callout("TIP") => Some(L10nKey::MarkdownBlockTip),
                    BlockKind::Callout("IMPORTANT") => Some(L10nKey::MarkdownBlockImportant),
                    BlockKind::Callout("WARNING") => Some(L10nKey::MarkdownBlockWarning),
                    BlockKind::Callout("CAUTION") => Some(L10nKey::MarkdownBlockCaution),
                    _ => None,
                };
                view.when_some(key, |view, key| {
                    view.child(div().text_sm().text_color(color).child(t(key)))
                })
            })
            .child(content)
            .into_any_element()
    }
    pub(super) fn block_actions<E: InteractiveElement>(
        &self,
        root: E,
        cx: &mut Context<Self>,
    ) -> E {
        root.on_action(cx.listener(|this, _: &ToParagraph, window, cx| {
            this.convert_block(BlockKind::Text, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToHeading1, window, cx| {
            this.convert_block(BlockKind::Heading(1), window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToHeading2, window, cx| {
            this.convert_block(BlockKind::Heading(2), window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToHeading3, window, cx| {
            this.convert_block(BlockKind::Heading(3), window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToHeading4, window, cx| {
            this.convert_block(BlockKind::Heading(4), window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToHeading5, window, cx| {
            this.convert_block(BlockKind::Heading(5), window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToHeading6, window, cx| {
            this.convert_block(BlockKind::Heading(6), window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToQuote, window, cx| {
            this.convert_block(BlockKind::Quote, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToCallout, window, cx| {
            this.convert_block(BlockKind::Callout("NOTE"), window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToTip, window, cx| {
            this.convert_block(BlockKind::Callout("TIP"), window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToImportant, window, cx| {
            this.convert_block(BlockKind::Callout("IMPORTANT"), window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToWarning, window, cx| {
            this.convert_block(BlockKind::Callout("WARNING"), window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToCaution, window, cx| {
            this.convert_block(BlockKind::Callout("CAUTION"), window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToBullet, window, cx| {
            this.convert_block(BlockKind::Bullet, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToNumbered, window, cx| {
            this.convert_block(BlockKind::Numbered, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToTask, window, cx| {
            this.convert_block(BlockKind::Task, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToCodeBlock, window, cx| {
            this.convert_block(BlockKind::Code, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToMathBlock, window, cx| {
            this.convert_block(BlockKind::Math, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToggleFold, window, cx| {
            this.toggle_block_fold(this.state.selection.head(), false, window, cx)
        }))
        .on_action(cx.listener(|this, _: &PersistCalloutFold, window, cx| {
            this.toggle_block_fold(this.state.selection.head(), true, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ExitQuote, window, cx| this.exit_quote(window, cx)))
    }
    fn block_menu(&self, menu: PopupMenu, cx: &App) -> PopupMenu {
        let disabled = self.current_block(cx).is_none();
        let kind = self.current_block(cx).map(|block| block.kind);
        let item = |menu: PopupMenu, key, action: Box<dyn Action>, target| {
            menu.menu_with_check_and_disabled(t(key), kind == Some(target), action, disabled)
        };
        let menu = item(
            menu,
            L10nKey::MarkdownBlockParagraph,
            Box::new(ToParagraph),
            BlockKind::Text,
        );
        let menu = item(
            menu,
            L10nKey::MarkdownBlockHeading1,
            Box::new(ToHeading1),
            BlockKind::Heading(1),
        );
        let menu = item(
            menu,
            L10nKey::MarkdownBlockHeading2,
            Box::new(ToHeading2),
            BlockKind::Heading(2),
        );
        let menu = item(
            menu,
            L10nKey::MarkdownBlockHeading3,
            Box::new(ToHeading3),
            BlockKind::Heading(3),
        );
        let menu = item(
            menu,
            L10nKey::MarkdownBlockHeading4,
            Box::new(ToHeading4),
            BlockKind::Heading(4),
        );
        let menu = item(
            menu,
            L10nKey::MarkdownBlockHeading5,
            Box::new(ToHeading5),
            BlockKind::Heading(5),
        );
        let menu = item(
            menu,
            L10nKey::MarkdownBlockHeading6,
            Box::new(ToHeading6),
            BlockKind::Heading(6),
        );
        let menu = item(
            menu.separator(),
            L10nKey::MarkdownBlockQuote,
            Box::new(ToQuote),
            BlockKind::Quote,
        );
        let menu = item(
            menu,
            L10nKey::MarkdownBlockNote,
            Box::new(ToCallout),
            BlockKind::Callout("NOTE"),
        );
        let menu = item(
            menu,
            L10nKey::MarkdownBlockTip,
            Box::new(ToTip),
            BlockKind::Callout("TIP"),
        );
        let menu = item(
            menu,
            L10nKey::MarkdownBlockImportant,
            Box::new(ToImportant),
            BlockKind::Callout("IMPORTANT"),
        );
        let menu = item(
            menu,
            L10nKey::MarkdownBlockWarning,
            Box::new(ToWarning),
            BlockKind::Callout("WARNING"),
        );
        let menu = item(
            menu,
            L10nKey::MarkdownBlockCaution,
            Box::new(ToCaution),
            BlockKind::Callout("CAUTION"),
        );
        let menu = item(
            menu.separator(),
            L10nKey::MarkdownBlockBullet,
            Box::new(ToBullet),
            BlockKind::Bullet,
        );
        let menu = item(
            menu,
            L10nKey::MarkdownBlockNumbered,
            Box::new(ToNumbered),
            BlockKind::Numbered,
        );
        let menu = item(
            menu,
            L10nKey::MarkdownBlockTask,
            Box::new(ToTask),
            BlockKind::Task,
        );
        let menu = item(
            menu,
            L10nKey::MarkdownBlockCode,
            Box::new(ToCodeBlock),
            BlockKind::Code,
        );
        let menu = item(
            menu,
            L10nKey::MarkdownBlockMath,
            Box::new(ToMathBlock),
            BlockKind::Math,
        );
        menu.separator()
            .menu_with_disabled(
                t(L10nKey::MarkdownBlockToggleFold),
                Box::new(ToggleFold),
                !self.live || self.fold_at(self.state.selection.head()).is_none(),
            )
            .menu_with_disabled(
                t(L10nKey::MarkdownBlockPersistFold),
                Box::new(PersistCalloutFold),
                !matches!(kind, Some(BlockKind::Callout(_))),
            )
            .menu_with_disabled(
                t(L10nKey::MarkdownBlockExitQuote),
                Box::new(ExitQuote),
                !matches!(kind, Some(BlockKind::Quote | BlockKind::Callout(_))),
            )
            .separator()
            .menu(
                t(L10nKey::MarkdownBlockSource),
                Box::new(crate::core::actions::ToggleDocumentPreview),
            )
    }
    pub(super) fn block_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity().downgrade();
        let focus = self.focus.clone();
        div()
            .flex_none()
            .px_3()
            .py_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                Button::new("markdown-block-commands")
                    .small()
                    .ghost()
                    .label(t(L10nKey::MarkdownBlockCommands))
                    .dropdown_menu(move |menu, _, cx| {
                        let menu = menu.action_context(focus.clone());
                        match view.upgrade() {
                            Some(view) => view.read(cx).block_menu(menu, cx),
                            None => menu,
                        }
                    }),
            )
            .into_any_element()
    }
    pub(super) fn block_disclosure(
        &self,
        at: usize,
        id: u64,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.live {
            return None;
        }
        let fold = self
            .projection
            .block_index
            .folds
            .iter()
            .find(|fold| fold.header.start == at)?;
        let closed = self.folds.closed(fold);
        let title = self
            .document
            .read(cx)
            .source()
            .slice(fold.header.clone())
            .to_string();
        let label = crate::ui::i18n::t_fmt(
            if closed {
                L10nKey::MarkdownBlockExpandLabel
            } else {
                L10nKey::MarkdownBlockCollapseLabel
            },
            &[("text", &title)],
        );
        Some(
            div()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Button::new(("markdown-block-fold", id))
                        .small()
                        .ghost()
                        .label(t(if closed {
                            L10nKey::MarkdownBlockExpand
                        } else {
                            L10nKey::MarkdownBlockCollapse
                        }))
                        .accessibility_label(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.toggle_block_fold(at, false, window, cx)
                        })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
#[path = "headings/tests.rs"]
mod tests;
