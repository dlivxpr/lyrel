use super::projection::{Kind, Projection};
use super::*;
use crate::ui::i18n::{L10nKey, t};
use gpui_kit::TestSupportExt as _;
use gpui_kit::base::StyledExt as _;
use gpui_kit::base::text::editor_host::{
    BlockNode, CodeBlock, FlowPosition, LeafGeometry, Paragraph, RenderContext,
    collect_leaf_geometry,
};
use gpui_kit::component::{ActiveTheme as _, input as actions};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::HashMap;
gpui_kit::actions!(markdown_images, [LoadImages]);

actions!(markdown_links, [EditLinkDestination, FollowLink]);

#[path = "assets.rs"]
mod assets;

struct CachedBlock {
    version: u64,
    seen: u64,
    node: BlockNode,
}

#[derive(Default)]
struct Viewport {
    heights: HashMap<u64, (u64, Pixels)>,
    frame: Vec<(u64, u64, Bounds<Pixels>)>,
    layout: Option<(Pixels, Pixels)>,
    anchor: Option<(u64, Pixels)>,
    offset: Point<Pixels>,
    bounds: Option<Bounds<Pixels>>,
}
/// A native editing presentation. It never owns a second text buffer or history.
pub(crate) struct SourcePresentation {
    document: Entity<MarkdownDocument>,
    state: PresentationState,
    focus: FocusHandle,
    scroll: ScrollHandle,
    geometry: Option<(u64, HashMap<u64, LeafGeometry>)>,
    table_geometry: HashMap<u64, Bounds<Pixels>>,
    dragging: bool,
    pointer_link: Option<(u64, String)>,
    reveal_caret: bool,
    wrap: bool,
    live: bool,
    projection: Projection,
    blocks: HashMap<u64, CachedBlock>,
    tables: super::tables::Layout,
    rendered_revision: Option<u64>,
    render_generation: u64,
    parse_task: Option<Task<()>>,
    parse_pending: Option<(Rope, u64, bool, super::mdx::Grammar)>,
    mermaid: super::mermaid::State,
    mermaid_task: Option<Task<()>>,
    _mermaid_theme: Subscription,
    math: super::math::State,
    extra_selections: Vec<Selection>,
    search_matches: std::rc::Rc<Vec<Range<usize>>>,
    image_snapshot: Option<std::sync::Arc<HashMap<String, std::sync::Arc<RenderImage>>>>,
    accessibility_snapshot: Option<(u64, SharedString)>,
    indent: crate::ui::editor_text::Indent,
    viewport_height: Pixels,
    viewport: Viewport,
    reveal_at: Option<usize>,
    selection_notification: bool,
    assets: assets::InsertionState,
    _observe: Subscription,
    mdx_state: super::mdx::ViewState,
}
impl SourcePresentation {
    pub(crate) fn new(
        document: Entity<MarkdownDocument>,
        wrap: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.subscribe(&document, |this, _, event: &DocumentEvent, cx| {
            if matches!(
                event,
                DocumentEvent::ResourceContextChanged | DocumentEvent::ImagesChanged
            ) {
                this.image_snapshot = None;
                if matches!(event, DocumentEvent::ResourceContextChanged) {
                    let document = this.document.read(cx);
                    if this.projection.grammar != document.grammar() {
                        this.projection.install(Projection::source(
                            document.source(),
                            document.revision(),
                            document.grammar(),
                        ));
                        this.rendered_revision = None;
                        this.schedule_parse(cx);
                    }
                }
                this.refresh_images(false, cx);
                this.tables.invalidate();
                this.invalidate_layout(cx);
                cx.notify();
                return;
            }
            if let DocumentEvent::Edited(transaction) = event {
                this.assets.rebase(transaction);
                this.state.rebase(transaction);
                if transaction.origin != this.state.id {
                    for selection in &mut this.extra_selections {
                        *selection = transaction.map_selection(*selection);
                    }
                }
                let doc = this.document.read(cx);
                if transaction.revision() == doc.revision() {
                    this.state.selection = Selection::new(
                        doc.boundary(this.state.selection.anchor, false),
                        doc.boundary(this.state.selection.head, false),
                    );
                }
                if this.projection.revision != transaction.revision()
                    && transaction.revision() == doc.revision()
                    && this
                        .projection
                        .revision
                        .wrapping_add(transaction.edits.len() as u64)
                        == transaction.revision()
                {
                    this.projection.rebase(doc.source(), transaction, this.live);
                    if this.live {
                        this.projection
                            .component_sources(doc.source(), &this.mdx_state);
                    }
                    this.rendered_revision = None;
                }
                this.sync_inline_reveal();
                this.schedule_parse(cx);
                this.schedule_math(cx);
                this.schedule_mermaid(cx);
                this.geometry = None;
                cx.notify();
            }
        });
        let projection = Projection::source(
            document.read(cx).source(),
            document.read(cx).revision(),
            document.read(cx).grammar(),
        );
        let mut typography = (
            cx.theme().font_family.clone(),
            cx.theme().mono_font_family.clone(),
            cx.theme().font_size,
            cx.theme().mono_font_size,
        );
        let mermaid_theme = cx.observe_global::<gpui_kit::component::Theme>(move |this, cx| {
            this.schedule_mermaid(cx);
            cx.notify();
            this.invalidate_layout(cx);
            let current = (
                cx.theme().font_family.clone(),
                cx.theme().mono_font_family.clone(),
                cx.theme().font_size,
                cx.theme().mono_font_size,
            );
            if current != typography {
                typography = current;
                this.viewport.layout = None;
            }
        });
        let mut view = Self {
            document,
            state: PresentationState::default(),
            focus: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            geometry: None,
            table_geometry: HashMap::new(),
            dragging: false,
            pointer_link: None,
            reveal_caret: false,
            wrap,
            live: true,
            projection,
            blocks: HashMap::new(),
            tables: super::tables::Layout::default(),
            rendered_revision: None,
            render_generation: 0,
            parse_task: None,
            parse_pending: None,
            mermaid: super::mermaid::State::default(),
            mermaid_task: None,
            _mermaid_theme: mermaid_theme,
            math: super::math::State::default(),
            extra_selections: Vec::new(),
            search_matches: std::rc::Rc::default(),
            image_snapshot: None,
            accessibility_snapshot: None,
            indent: crate::ui::editor_text::Indent {
                hard_tabs: false,
                size: 4,
            },
            viewport_height: Pixels::ZERO,
            viewport: Viewport::default(),
            reveal_at: None,
            selection_notification: false,
            assets: assets::InsertionState::default(),
            _observe: observe,
            mdx_state: super::mdx::ViewState::default(),
        };
        view.schedule_parse(cx);
        view
    }
    pub(crate) fn document(&self) -> &Entity<MarkdownDocument> {
        &self.document
    }
    pub(crate) fn indent(&self) -> crate::ui::editor_text::Indent {
        self.indent
    }
    pub(crate) fn set_indent(
        &mut self,
        indent: crate::ui::editor_text::Indent,
        cx: &mut Context<Self>,
    ) {
        self.indent = indent;
        cx.notify();
    }
    pub(crate) fn is_live(&self) -> bool {
        self.live
    }
    pub(crate) fn toggle_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.live = !self.live;
        self.geometry = None;
        self.rendered_revision = None;
        self.parse_pending = None;
        self.mermaid.cancel();
        if self.live {
            self.schedule_parse(cx);
        } else {
            self.projection.install(Projection::source(
                self.document.read(cx).source(),
                self.document.read(cx).revision(),
                self.document.read(cx).grammar(),
            ));
        }
        self.reveal_caret = true;
        self.focus.focus(window, cx);
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }
    fn schedule_parse(&mut self, cx: &mut Context<Self>) {
        if !self.live
            && self.projection.revision == self.document.read(cx).revision()
            && self.projection.grammar == self.document.read(cx).grammar()
        {
            return;
        }
        let document = self.document.read(cx);
        self.parse_pending = Some((
            document.source().clone(),
            document.revision(),
            self.live,
            document.grammar(),
        ));
        if self.parse_task.is_some() {
            return;
        }
        let identity = self.document.entity_id();
        self.parse_task = Some(cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(45))
                    .await;
                let request = view.update(cx, |view, _| {
                    let request = view.parse_pending.take();
                    if request.is_none() {
                        view.parse_task = None;
                    }
                    request
                });
                let Ok(Some((source, revision, live, grammar))) = request else {
                    break;
                };
                let projection = cx
                    .background_executor()
                    .spawn(async move {
                        Projection::parse(&source.to_string(), revision, live, grammar)
                    })
                    .await;
                let next = view.update(cx, |view, cx| {
                    if view.live == live
                        && view.document.entity_id() == identity
                        && view.document.read(cx).revision() == revision
                        && view.document.read(cx).grammar() == grammar
                    {
                        view.invalidate_layout(cx);
                        view.projection.install(projection);
                        if live {
                            view.projection.component_sources(
                                view.document.read(cx).source(),
                                &view.mdx_state,
                            );
                        }
                        view.reveal_mdx_selection(cx);
                        view.sync_inline_reveal();
                        view.state.selection = view.map_live_caret(view.state.selection, cx);
                        view.selection_notification = true;
                        view.rendered_revision = None;
                        view.geometry = None;
                        view.refresh_images(false, cx);
                        view.schedule_mermaid(cx);
                        view.schedule_math(cx);
                        cx.notify();
                    }
                    let next = view.parse_pending.is_some();
                    if !next {
                        view.parse_task = None;
                    }
                    next
                });
                if !matches!(next, Ok(true)) {
                    break;
                }
            }
        }));
    }
    fn refresh_images(&mut self, consent: bool, cx: &mut Context<Self>) {
        if !self.live || self.projection.revision != self.document.read(cx).revision() {
            return;
        }
        let mut urls: Vec<String> = self
            .projection
            .leaves
            .iter()
            .flat_map(|leaf| leaf.image_urls())
            .map(str::to_owned)
            .collect();
        urls.sort();
        urls.dedup();
        self.document
            .update(cx, |document, cx| document.sync_images(urls, consent, cx));
    }
    fn schedule_mermaid(&mut self, cx: &mut Context<Self>) {
        if !self.live || self.projection.revision != self.document.read(cx).revision() {
            // A parse in flight must not cancel unchanged diagrams.
            return;
        }
        self.mermaid.request(
            &self.projection,
            self.document.entity_id(),
            super::mermaid::Style::current(cx),
        );
        self.mermaid.install(
            &self.projection,
            self.document.entity_id(),
            &super::mermaid::Style::current(cx),
        );
        // One worker and one replaceable pending request per presentation.
        // Cancelling the control stops synchronous Merman work at checkpoints;
        // retaining the task prevents rapid edits from creating unbounded jobs.
        if self.mermaid_task.is_some() {
            return;
        }
        let Some(mut job) = self.mermaid.take_job() else {
            return;
        };
        self.mermaid_task = Some(cx.spawn(async move |view, cx| {
            loop {
                let batch = cx
                    .background_executor()
                    .spawn(async move { job.run() })
                    .await;
                let next = view.update(cx, |view, cx| {
                    // Keep completed results until a current projection can
                    // validate them; a parse gap must not strand pending work.
                    view.mermaid.complete(batch);
                    if view.live && view.projection.revision == view.document.read(cx).revision() {
                        view.invalidate_layout(cx);
                        view.mermaid.install(
                            &view.projection,
                            view.document.entity_id(),
                            &super::mermaid::Style::current(cx),
                        );
                        view.geometry = None;
                        cx.notify();
                    }
                    let next = view.mermaid.take_job();
                    if next.is_none() {
                        view.mermaid_task = None;
                    }
                    next
                });
                match next {
                    Ok(Some(next)) => job = next,
                    _ => break,
                }
            }
        }));
    }
    fn schedule_math(&mut self, cx: &mut Context<Self>) {
        if !self.live
            || self.math.running
            || self.projection.revision != self.document.read(cx).revision()
        {
            return;
        }
        let Some(style) = self.math.style else {
            return;
        };
        let revision = self.projection.revision;
        let identity = self.document.entity_id();
        let formulas: std::collections::HashSet<_> = self
            .projection
            .leaves
            .iter()
            .flat_map(|leaf| leaf.formulas())
            .cloned()
            .collect();
        self.math
            .outputs
            .retain(|formula, _| formulas.contains(formula));
        let pending: Vec<_> = formulas
            .into_iter()
            .filter(|formula| !self.math.outputs.contains_key(formula))
            .collect();
        if pending.is_empty() {
            return;
        }
        self.math.running = true;
        self.math.task = Some(cx.spawn(async move |view, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    pending
                        .into_iter()
                        .map(|formula| {
                            let output = super::math::render(&formula, style);
                            (formula, output)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = view.update(cx, |view, cx| {
                view.math.running = false;
                if view.live
                    && view.document.entity_id() == identity
                    && view.document.read(cx).revision() == revision
                    && view.projection.revision == revision
                    && view.math.style == Some(style)
                {
                    view.invalidate_layout(cx);
                    let current: std::collections::HashSet<_> = view
                        .projection
                        .leaves
                        .iter()
                        .flat_map(|leaf| leaf.formulas())
                        .cloned()
                        .collect();
                    for (formula, output) in results {
                        if current.contains(&formula) {
                            view.math.outputs.insert(formula, output);
                        }
                    }
                    view.tables.invalidate();
                    for cached in view.blocks.values_mut() {
                        cached.version = u64::MAX;
                    }
                    view.rendered_revision = None;
                    view.geometry = None;
                    cx.notify();
                }
                // A single in-flight batch coalesces all edits. A stale batch
                // never lands, and only the newest projection is queued next.
                view.schedule_math(cx);
            });
        }));
    }
    fn invalidate_layout(&mut self, cx: &App) {
        if let (Some(viewport), Some(caret)) = (
            self.viewport.bounds,
            self.caret_bounds(self.state.selection.head, cx),
        ) {
            self.reveal_caret |=
                caret.top() >= viewport.top() && caret.bottom() <= viewport.bottom();
        }
        self.geometry = None;
    }
    // Display-boundary insertion has right affinity after a hidden opening
    // delimiter and left affinity before a hidden closing delimiter.
    fn map_live_caret(&self, selection: Selection, cx: &App) -> Selection {
        if self.live
            && !self.is_composing()
            && selection.range().is_empty()
            && self.projection.revision == self.document.read(cx).revision()
        {
            if let Some(leaf) = self.projection.leaf_at(selection.head) {
                if leaf.inline_position(selection.head) {
                    return selection;
                }
                let mapped = leaf.source_offset(leaf.display_offset(selection.head));
                if mapped < selection.head
                    && self
                        .document
                        .read(cx)
                        .source()
                        .slice(mapped..selection.head)
                        .chars()
                        .last()
                        .is_some_and(|ch| matches!(ch, ' ' | '\t'))
                {
                    return selection;
                }
                return Selection::caret(mapped);
            }
        }
        selection
    }
    fn sync_mdx_inline_visibility(&mut self, cx: &App) {
        if !self.live { return; }
        let hidden = super::mdx::view::hidden_inline_ranges(
            &self.projection, self.document.read(cx).source(), &self.mdx_state,
        );
        let marked = self.state.marked_range();
        for leaf in &mut self.projection.leaves {
            if leaf.hide_ranges(&hidden, marked.as_ref()) {
                if matches!(leaf.kind, Kind::TableCell(_)) { self.tables.invalidate_cell(leaf.id); }
                if let Some(block) = self.blocks.get_mut(&leaf.id) { block.version = u64::MAX; }
                self.geometry = None;
                self.viewport.layout = None;
                self.rendered_revision = None;
            }
        }
    }
    fn sync_inline_reveal(&mut self) {
        let marked = self.state.marked_range();
        for leaf in &mut self.projection.leaves {
            if leaf.contextual_reveal(
                self.state.selection,
                &self.extra_selections,
                marked.as_ref(),
            ) {
                if matches!(leaf.kind, Kind::TableCell(_)) {
                    self.tables.invalidate_cell(leaf.id);
                    self.viewport.heights.clear();
                }
                if let Some(cached) = self.blocks.get_mut(&leaf.id) {
                    cached.version = u64::MAX;
                }
                self.geometry = None;
                self.rendered_revision = None;
            }
        }
    }
    pub(crate) fn finish_active_composition(
        document: &Entity<MarkdownDocument>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let owner = document
            .read(cx)
            .composition_owner
            .as_ref()
            .and_then(WeakEntity::upgrade);
        if let Some(owner) = owner {
            owner.update(cx, |view, cx| {
                view.command(Command::FinishComposition, window, cx)
            });
        }
    }
    pub(crate) fn add_selection(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let doc = self.document.read(cx);
        let range = doc.safe_range(range);
        let selection = Selection::new(range.start, range.end);
        if selection != self.state.selection && !self.extra_selections.contains(&selection) {
            self.extra_selections.push(selection);
        }
        self.reveal_caret = true;
        self.reveal_at = Some(selection.head);
        self.selection_notification = true;
        self.sync_inline_reveal();
        cx.notify();
    }
    pub(crate) fn set_search_matches(
        &mut self,
        matches: std::rc::Rc<Vec<Range<usize>>>,
        cx: &mut Context<Self>,
    ) {
        self.search_matches = matches;
        cx.notify();
    }
    pub(crate) fn select_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.extra_selections.clear();
        self.reveal_caret = true;
        self.reveal_at = None;
        self.selection_notification = true;
        if self.is_composing() {
            for cached in self.blocks.values_mut() {
                cached.version = u64::MAX;
            }
            self.rendered_revision = None;
        }
        self.document.update(cx, |document, cx| {
            document.command(
                &mut self.state,
                Command::Select(Selection::new(range.start, range.end)),
            );
            cx.emit(DocumentEvent::PresentationChanged);
        });
        self.reveal_mdx_selection(cx);
        self.state.selection = self.map_live_caret(self.state.selection, cx);
        self.sync_inline_reveal();
        cx.notify();
    }
    pub(crate) fn selection(&self) -> Selection {
        self.state.selection()
    }
    pub(crate) fn occurrence_range(&self) -> Range<usize> {
        self.extra_selections
            .last()
            .copied()
            .unwrap_or(self.state.selection)
            .range()
    }
    /// Dedicated MDX adapters own only presentation state. Finish the shared
    /// preedit before an adapter changes which child is laid out; source,
    /// selection and history remain owned by this presentation/document.
    pub(super) fn update_mdx_view_state(
        &mut self,
        update: impl FnOnce(&mut super::mdx::ViewState),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.command(Command::FinishComposition, window, cx);
        update(&mut self.mdx_state);
        self.sync_mdx_inline_visibility(cx);
        self.viewport.anchor = None;
        self.viewport.layout = None;
        self.geometry = None;
        self.rendered_revision = None;
        cx.notify();
    }
    pub(super) fn toggle_component_source(
        &mut self,
        id: super::mdx::ComponentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.command(Command::FinishComposition, window, cx);
        let Some(component) = self
            .projection
            .mdx
            .components
            .iter()
            .find(|component| component.id() == id)
        else {
            return;
        };
        let children = component.children_range();
        let at = self
            .state
            .selection
            .head
            .clamp(children.start, children.end);
        let leaving = self.mdx_state.source_components.contains(&id);
        self.update_mdx_view_state(
            |state| {
                if leaving {
                    state.source_components.remove(&id);
                } else {
                    state.source_components.insert(id);
                }
            },
            window,
            cx,
        );
        if leaving {
            self.schedule_parse(cx);
        } else {
            self.projection
                .component_sources(self.document.read(cx).source(), &self.mdx_state);
        }
        self.command(Command::Select(Selection::caret(at)), window, cx);
        self.focus.focus(window, cx);
    }
    pub(super) fn toggle_mdx_accordion(
        &mut self,
        id: super::mdx::ComponentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.command(Command::FinishComposition, window, cx);
        // Resolve the stable identity after composition may rebase the source.
        let Some(component) = self
            .projection
            .mdx
            .components
            .iter()
            .find(|component| component.id() == id)
        else {
            return;
        };
        let header = component.name_range().start;
        self.command(Command::Select(Selection::caret(header)), window, cx);
        self.update_mdx_view_state(
            |state| {
                if !state.collapsed.remove(&id) {
                    state.collapsed.insert(id);
                }
            },
            window,
            cx,
        );
        self.focus.focus(window, cx);
    }
    /// Source/undo selections reveal only their enclosing presentation state.
    /// Collapsing controls first select their own visible header, so they do not
    /// immediately reopen themselves through this hook.
    fn reveal_mdx_selection(&mut self, cx: &App) {
        if !self.live
            || self.projection.revision != self.document.read(cx).revision()
            || self.projection.grammar != self.document.read(cx).grammar()
        {
            return;
        }
        let source = self.document.read(cx).source();
        let tabs_changed = super::mdx::tabs::reveal_selection(
            &self.projection.mdx.components,
            source,
            &mut self.mdx_state,
            self.state.selection,
        );
        let accordions_changed = super::mdx::accordions::reveal_selection(
            &self.projection.mdx.components,
            source,
            &mut self.mdx_state,
            self.state.selection,
        );
        if tabs_changed || accordions_changed {
            self.viewport.anchor = None;
            self.viewport.layout = None;
            self.geometry = None;
            self.rendered_revision = None;
        }
        self.sync_mdx_inline_visibility(cx);
    }
    pub(super) fn activate_mdx_tab(
        &mut self,
        group: super::mdx::ComponentId,
        child: super::mdx::ComponentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Finalizing preedit can rebase every range. Resolve the stable IDs
        // afterwards, never select a range captured by the previous frame.
        self.command(Command::FinishComposition, window, cx);
        if !self.live || self.document.read(cx).grammar() != super::mdx::Grammar::Mdx {
            return;
        }
        let valid = self
            .projection
            .mdx
            .components
            .iter()
            .any(|component| component.id() == group && component.child_ids().contains(&child));
        if !valid {
            return;
        }
        self.update_mdx_view_state(
            |state| {
                state.active_child.insert(group, child);
            },
            window,
            cx,
        );
        let target = {
            let source = self.document.read(cx).source();
            let component = self
                .projection
                .mdx
                .components
                .iter()
                .find(|component| component.id() == child)
                .expect("validated tab child");
            let range = component.children_range();
            self.projection
                .leaves
                .iter()
                .find(|leaf| {
                    range.start <= leaf.range.start
                        && leaf.range.end <= range.end
                        && !matches!(leaf.kind, Kind::Mdx(_))
                        && super::mdx::view::visible_field(&super::mdx::view::FieldContext::new(
                            &self.projection,
                            leaf,
                            source,
                            &self.mdx_state,
                        ))
                })
                .map_or(component.name_range().start, |leaf| leaf.content_start())
        };
        self.command(Command::Select(Selection::caret(target)), window, cx);
        self.focus.focus(window, cx);
    }
    pub(crate) fn is_composing(&self) -> bool {
        self.state.composition.is_some()
    }
    pub(crate) fn is_soft_wrap(&self) -> bool {
        self.wrap
    }
    pub(crate) fn set_soft_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        self.wrap = wrap;
        self.geometry = None;
        self.viewport.layout = None;
        self.reveal_caret = true;
        cx.notify();
    }
    pub(crate) fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll.offset()
    }
    pub(crate) fn set_scroll_offset(&mut self, offset: Point<Pixels>, cx: &mut Context<Self>) {
        self.scroll.set_offset(offset);
        self.viewport.anchor = None;
        self.geometry = None;
        self.reveal_caret = false;
        cx.notify();
    }
    fn table_commands_enabled(&self) -> bool {
        self.live && self.state.composition.is_none() && self.extra_selections.is_empty()
            && self.projection.leaf_at(self.state.selection.head()).is_some_and(|leaf| {
                matches!(leaf.kind, Kind::TableCell(cell) if self.state.selection.range().start >= cell.table().start && self.state.selection.range().end <= cell.table().end)
            })
    }

    fn table_command(
        &mut self,
        operation: super::tables::Operation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.table_commands_enabled()
            && self.projection.revision == self.document.read(cx).revision()
        {
            let Some(leaf) = self.projection.leaf_at(self.state.selection.head()) else {
                return;
            };
            let Kind::TableCell(cell) = leaf.kind else {
                return;
            };
            self.command(
                Command::Table {
                    operation,
                    range: cell.table(),
                    revision: self.projection.revision,
                },
                window,
                cx,
            );
            self.focus.focus(window, cx);
        } else {
            cx.propagate();
        }
    }

    pub(crate) fn table_menu(
        &self,
        menu: gpui_kit::component::menu::PopupMenu,
    ) -> gpui_kit::component::menu::PopupMenu {
        use super::tables::*;
        use crate::ui::i18n::{L10nKey, t};
        if !self.live {
            return menu;
        }
        let disabled = !self.table_commands_enabled();
        menu.separator()
            .menu_with_disabled(
                t(L10nKey::EditorTableInsertRowBefore),
                Box::new(InsertRowBefore),
                disabled,
            )
            .menu_with_disabled(
                t(L10nKey::EditorTableInsertRowAfter),
                Box::new(InsertRowAfter),
                disabled,
            )
            .menu_with_disabled(
                t(L10nKey::EditorTableDeleteRow),
                Box::new(DeleteRow),
                disabled,
            )
            .menu_with_disabled(
                t(L10nKey::EditorTableInsertColumnBefore),
                Box::new(InsertColumnBefore),
                disabled,
            )
            .menu_with_disabled(
                t(L10nKey::EditorTableInsertColumnAfter),
                Box::new(InsertColumnAfter),
                disabled,
            )
            .menu_with_disabled(
                t(L10nKey::EditorTableDeleteColumn),
                Box::new(DeleteColumn),
                disabled,
            )
            .separator()
            .menu_with_disabled(
                t(L10nKey::EditorTableAlignDefault),
                Box::new(AlignDefault),
                disabled,
            )
            .menu_with_disabled(
                t(L10nKey::EditorTableAlignLeft),
                Box::new(AlignLeft),
                disabled,
            )
            .menu_with_disabled(
                t(L10nKey::EditorTableAlignCenter),
                Box::new(AlignCenter),
                disabled,
            )
            .menu_with_disabled(
                t(L10nKey::EditorTableAlignRight),
                Box::new(AlignRight),
                disabled,
            )
    }

    pub(crate) fn command(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.selection = self.map_live_caret(self.state.selection, cx);
        let command = if self.live
            && self.extra_selections.is_empty()
            && self.projection.grammar == self.document.read(cx).grammar()
            && self.projection.revision == self.document.read(cx).revision()
        {
            super::mdx::name_command(
                &self.projection.mdx,
                self.document.read(cx),
                &self.state,
                command,
            )
        } else {
            command
        };
        let command = match command {
            Command::Select(selection) => Command::Select(self.map_live_caret(selection, cx)),
            command => command,
        };
        let cell_input = |range: &Range<usize>, text: String, selected| {
            if self.live
                && self.projection.leaf_at(range.start).is_some_and(|leaf| {
                    matches!(leaf.kind, Kind::TableCell(_)) && range.end <= leaf.range.end
                })
            {
                super::tables::input(self.document.read(cx).source(), range, text, selected)
            } else {
                (text, selected)
            }
        };
        let command = match command {
            Command::Insert(text) => {
                Command::Insert(cell_input(&self.state.selection.range(), text, None).0)
            }
            Command::Commit(text) => {
                let range = self
                    .state
                    .marked_range()
                    .unwrap_or_else(|| self.state.selection.range());
                Command::Commit(cell_input(&range, text, None).0)
            }
            Command::Compose {
                range,
                text,
                selected,
            } => {
                let target = range
                    .clone()
                    .or_else(|| self.state.marked_range())
                    .unwrap_or_else(|| self.state.selection.range());
                let (text, selected) = cell_input(&target, text, selected);
                Command::Compose {
                    range,
                    text,
                    selected,
                }
            }
            Command::Replace { range, text, kind }
                if matches!(kind, EditKind::Typing | EditKind::Paste) =>
            {
                let (text, _) = cell_input(&range, text, None);
                Command::Replace { range, text, kind }
            }
            command => command,
        };
        let marked_before = self.state.marked_range();
        let mut after_extra = None;
        let command = if !self.extra_selections.is_empty() {
            let replacement = match &command {
                Command::Insert(text) | Command::Commit(text) if !self.is_composing() => {
                    Some((text.clone(), EditKind::Typing))
                }
                Command::Replace { text, kind, range }
                    if *range == self.state.selection.range() =>
                {
                    Some((text.clone(), *kind))
                }
                Command::Backspace | Command::Delete => Some((String::new(), EditKind::Command)),
                _ => None,
            };
            if let Some((text, kind)) = replacement {
                let doc = self.document.read(cx);
                let mut edits: Vec<_> = std::iter::once(self.state.selection)
                    .chain(self.extra_selections.iter().copied())
                    .map(|selection| {
                        let mut range = selection.range();
                        if range.is_empty() {
                            if matches!(command, Command::Backspace) {
                                range.start = doc.previous(range.start);
                            }
                            if matches!(command, Command::Delete) {
                                range.end = doc.next(range.end);
                            }
                        }
                        (range, text.clone())
                    })
                    .collect();
                edits.sort_by_key(|(range, _)| range.start);
                let mut delta: isize = 0;
                let mut resulting = Vec::new();
                for (range, text) in &edits {
                    resulting.push(Selection::caret(
                        (range.start as isize + delta) as usize + text.len(),
                    ));
                    delta += text.len() as isize - range.len() as isize;
                }
                after_extra = Some(resulting.into_iter().skip(1).collect());
                Command::BatchReplace { edits, kind }
            } else {
                if matches!(
                    command,
                    Command::Select(_) | Command::Undo | Command::Redo | Command::Compose { .. }
                ) {
                    self.extra_selections.clear();
                }
                command
            }
        } else {
            command
        };
        let owner = self
            .document
            .read(cx)
            .composition_owner
            .as_ref()
            .and_then(WeakEntity::upgrade);
        if let Some(owner) = owner.filter(|owner| owner.entity_id() != cx.entity().entity_id()) {
            owner.update(cx, |view, cx| {
                view.command(Command::FinishComposition, window, cx)
            });
        }
        let own_view = cx.entity().downgrade();
        let changed = self.document.update(cx, |document, cx| {
            let revision = document.revision();
            let transaction = document.command(&mut self.state, command);
            document.composition_owner = self.state.composition.as_ref().map(|_| own_view);
            let changed = document.revision() != revision;
            if changed {
                if self.projection.revision == revision {
                    self.projection
                        .rebase(document.source(), &transaction, self.live);
                    if self.live {
                        self.projection
                            .component_sources(document.source(), &self.mdx_state);
                    }
                }
                self.rendered_revision = None;
                cx.emit(DocumentEvent::Edited(transaction));
                cx.notify();
            } else {
                cx.emit(DocumentEvent::PresentationChanged);
            }
            changed
        });
        if let Some(selections) = after_extra {
            self.extra_selections = selections;
        }
        if marked_before != self.state.marked_range() {
            let after = self.state.marked_range();
            for leaf in &self.projection.leaves {
                if marked_before
                    .iter()
                    .chain(after.iter())
                    .any(|range| range.start <= leaf.range.end && range.end >= leaf.range.start)
                {
                    if let Some(cached) = self.blocks.get_mut(&leaf.id) {
                        cached.version = u64::MAX;
                    }
                }
            }
            self.rendered_revision = None;
        }
        self.reveal_mdx_selection(cx);
        self.sync_inline_reveal();
        if changed {
            self.geometry = None;
            self.schedule_parse(cx);
            window.notify_text_input_changed(&self.focus);
        } else {
            window.notify_text_selection_changed(&self.focus);
        }
        self.reveal_caret = true;
        self.reveal_at = None;
        cx.notify();
    }
    fn edit_link_destination(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.projection.revision != self.document.read(cx).revision() {
            return;
        }
        let Some(range) = self
            .projection
            .links
            .at(self.state.selection.head())
            .map(|link| link.destination.clone())
        else {
            return;
        };
        self.command(
            Command::Select(Selection::new(range.start, range.end)),
            window,
            cx,
        );
        self.focus.focus(window, cx);
    }
    fn pointer_link_target(&self, position: Point<Pixels>, cx: &App) -> Option<&str> {
        if !self.live || self.projection.revision != self.document.read(cx).revision() {
            return None;
        }
        let at = self.point_offset(position, cx)?;
        let link = self.projection.links.at(at)?;
        (link.label.start <= at && at <= link.label.end).then_some(link.target.as_str())
    }
    fn finish_pointer_link(
        &mut self,
        position: Point<Pixels>,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let Some((revision, target)) = self.pointer_link.take() else {
            return;
        };
        if link_modifier(modifiers)
            && self.document.read(cx).revision() == revision
            && self.pointer_link_target(position, cx) == Some(target.as_str())
        {
            self.follow_link(target, cx);
        }
    }
    pub(super) fn follow_link(&mut self, target: String, cx: &mut Context<Self>) {
        if self.projection.revision != self.document.read(cx).revision() {
            return;
        }
        if let Some(fragment) = target.strip_prefix('#') {
            if let Some(at) = self.projection.links.anchor(fragment) {
                self.select_range(at..at, cx);
                self.reveal_caret = true;
            }
            return;
        }
        let presentation = cx.entity().entity_id();
        let revision = self.document.read(cx).revision();
        self.document.update(cx, |_, cx| {
            cx.emit(DocumentEvent::FollowLink {
                presentation,
                revision,
                target,
            })
        });
    }
    fn caret_bounds(&self, byte: usize, cx: &App) -> Option<Bounds<Pixels>> {
        let (revision, geometry) = self.geometry.as_ref()?;
        if *revision != self.document.read(cx).revision() {
            return None;
        }
        let mapped = self.projection.leaf_at(byte)?;
        let leaf = geometry.get(&mapped.id)?;
        Some(Bounds::new(
            leaf.position_for_index(mapped.display_offset(byte)),
            size(px(1.), leaf.caret_height()),
        ))
    }
    pub(crate) fn range_to_bounds(&self, range: &Range<usize>, cx: &App) -> Option<Bounds<Pixels>> {
        self.caret_bounds(range.end, cx)
    }
    fn point_offset(&self, point: Point<Pixels>, cx: &App) -> Option<usize> {
        self.point_offset_for(point, false, cx)
    }
    fn point_offset_for(&self, point: Point<Pixels>, activate: bool, cx: &App) -> Option<usize> {
        let (revision, geometry) = self.geometry.as_ref()?;
        let doc = self.document.read(cx);
        if *revision != doc.revision() {
            return None;
        }
        let (id, leaf) = geometry.iter().min_by(|(a_id, a), (b_id, b)| {
            let distance = |id: &u64, leaf: &LeafGeometry| {
                let bounds = self
                    .table_geometry
                    .get(id)
                    .copied()
                    .unwrap_or_else(|| leaf.bounds());
                let dy = if point.y < bounds.top() {
                    bounds.top() - point.y
                } else if point.y > bounds.bottom() {
                    point.y - bounds.bottom()
                } else {
                    px(0.)
                };
                let dx = if point.x < bounds.left() {
                    bounds.left() - point.x
                } else if point.x > bounds.right() {
                    point.x - bounds.right()
                } else {
                    px(0.)
                };
                f32::from(dx).powi(2) + f32::from(dy).powi(2)
            };
            distance(a_id, a)
                .partial_cmp(&distance(b_id, b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
        let mapped = self
            .projection
            .leaves
            .iter()
            .find(|mapped| mapped.id == *id)?;
        let display = activate
            .then(|| leaf.atom_at(point))
            .flatten()
            .and_then(|atom| mapped.atom_source(atom.start));
        Some(doc.boundary(
            display.unwrap_or_else(|| mapped.source_offset(leaf.index_for_position(point))),
            false,
        ))
    }
    fn select_point(
        &mut self,
        point: Point<Pixels>,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.point_offset_for(point, !extend, cx) else {
            return;
        };
        let anchor = if extend {
            self.state.selection.anchor
        } else {
            at
        };
        self.command(Command::Select(Selection::new(anchor, at)), window, cx);
    }
    fn move_cursor(
        &mut self,
        horizontal: i32,
        vertical: i32,
        extend: bool,
        edge: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let doc = self.document.read(cx);
        let current = doc.boundary(self.state.selection.head, false);
        let at = if let Some(end) = edge {
            self.visual_line_edge(current, end, cx).unwrap_or_else(|| {
                let row = doc.source().offset_to_point(current).row;
                if end {
                    doc.source().line_end_offset(row)
                } else {
                    doc.source().line_start_offset(row)
                }
            })
        } else if vertical != 0 {
            self.vertical_target(current, vertical as f32, cx)
                .unwrap_or_else(|| {
                    let point = doc.source().offset_to_point(current);
                    let last = doc.source().offset_to_point(doc.source().len()).row;
                    let row = if vertical < 0 {
                        point.row.saturating_sub(1)
                    } else {
                        (point.row + 1).min(last)
                    };
                    let start = doc.source().line_start_offset(row);
                    let end = doc.source().line_end_offset(row);
                    doc.boundary((start + point.column as usize).min(end), false)
                })
        } else if !extend && !self.state.selection.range().is_empty() {
            if horizontal < 0 {
                self.state.selection.range().start
            } else {
                self.state.selection.range().end
            }
        } else if self.live {
            self.visible_step(current, horizontal < 0, cx)
        } else if horizontal < 0 {
            doc.previous(current)
        } else {
            doc.next(current)
        };
        let at = doc.boundary(at, false);
        let anchor = if extend {
            self.state.selection.anchor
        } else {
            at
        };
        self.command(Command::Select(Selection::new(anchor, at)), window, cx);
    }
    fn vertical_target(&self, at: usize, lines: f32, cx: &App) -> Option<usize> {
        let (revision, geometry) = self.geometry.as_ref()?;
        if *revision != self.document.read(cx).revision() {
            return None;
        }
        let mapped = self.projection.leaf_at(at)?;
        let current_id = mapped.id;
        let leaf = geometry.get(&mapped.id)?;
        let caret = leaf.position_for_index(mapped.display_offset(at));
        let backward = lines < 0.;
        // A sample just past a short row can still hit that row when the next
        // block has a margin (or a tall image). Search the current frame's
        // shaped rows, not source line numbers or absent/unlaid-out leaves.
        self.projection
            .leaves
            .iter()
            .filter_map(|mapped| {
                let leaf = geometry.get(&mapped.id)?;
                let y = if mapped.id == current_id {
                    caret.y + leaf.line_height() * (lines + 0.25)
                } else {
                    leaf.position_for_index(if backward { mapped.text.len() } else { 0 })
                        .y
                        + leaf.line_height() * 0.25
                };
                let display = leaf.index_for_position(point(caret.x, y));
                let position = leaf.position_for_index(display);
                let dy = if backward {
                    caret.y - position.y
                } else {
                    position.y - caret.y
                };
                (dy > Pixels::ZERO).then(|| {
                    (
                        mapped.source_offset(display),
                        dy,
                        (position.x - caret.x).abs(),
                    )
                })
            })
            .min_by(|a, b| {
                a.1.partial_cmp(&b.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
            })
            .map(|(at, _, _)| at)
    }
    fn visual_line_edge(&self, at: usize, end: bool, cx: &App) -> Option<usize> {
        let (revision, geometry) = self.geometry.as_ref()?;
        if *revision != self.document.read(cx).revision() {
            return None;
        }
        let mapped = self.projection.leaf_at(at)?;
        let leaf = geometry.get(&mapped.id)?;
        let caret = leaf.position_for_index(mapped.display_offset(at));
        let x = if end {
            leaf.bounds().right()
        } else {
            leaf.bounds().left()
        };
        self.point_offset(point(x, caret.y + leaf.line_height() * 0.25), cx)
    }
    fn delete_line(&mut self, end: bool, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.state.selection.head;
        let range = if self.state.selection.range().is_empty() {
            let target = self.visual_line_edge(at, end, cx).unwrap_or(at);
            target.min(at)..target.max(at)
        } else {
            self.state.selection.range()
        };
        self.command(
            Command::Replace {
                range,
                text: String::new(),
                kind: EditKind::Command,
            },
            window,
            cx,
        );
    }
    fn move_page(&mut self, direction: f32, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.state.selection.head;
        if let Some(caret) = self.caret_bounds(at, cx) {
            if let Some(target) = self.point_offset(
                point(caret.left(), caret.top() + self.viewport_height * direction),
                cx,
            ) {
                self.command(Command::Select(Selection::caret(target)), window, cx);
            }
        }
    }
    fn navigate_table(
        &mut self,
        navigation: super::tables::Navigation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.live || self.projection.revision != self.document.read(cx).revision() {
            return false;
        }
        let Some(target) = super::tables::navigate(
            &self.projection.leaves,
            self.state.selection.head,
            navigation,
        ) else {
            return false;
        };
        self.command(Command::FinishComposition, window, cx);
        if let Some(at) = target {
            self.command(Command::Select(Selection::caret(at)), window, cx);
        } else if navigation.backward() {
            window.focus_prev(cx);
        } else {
            window.focus_next(cx);
        }
        true
    }
    fn indent_selection(&mut self, outdent: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.navigate_table(
            super::tables::Navigation::Cell { backward: outdent },
            window,
            cx,
        ) {
            return;
        }
        // JSX fields expose native command controls, not indentation widgets.
        // Keep ordinary source/code/list/table Tab behavior unchanged while
        // allowing keyboard users to leave the single input for those controls.
        if self.live
            && self.document.read(cx).grammar() == super::mdx::Grammar::Mdx
            && (!self.focus.is_focused(window)
                || self
                    .projection
                    .leaf_at(self.state.selection.head)
                    .is_some_and(|leaf| matches!(leaf.kind, Kind::Mdx(_))))
        {
            self.command(Command::FinishComposition, window, cx);
            if outdent {
                window.focus_prev(cx);
            } else {
                window.focus_next(cx);
            }
            return;
        }
        let structure = self
            .projection
            .leaf_at(self.state.selection.range().start)
            .and_then(|leaf| match leaf.kind {
                Kind::List(style) | Kind::Quote(style) => Some(style),
                _ => None,
            });
        if self.live && self.extra_selections.is_empty() {
            if let Some(style) = structure {
                self.command(
                    Command::ListIndent {
                        outdent,
                        width: self.indent.size,
                        at: Some(style.start),
                    },
                    window,
                    cx,
                );
                return;
            }
        }
        let selected = self.state.selection.range();
        let text = if self.indent.hard_tabs {
            "\t".into()
        } else {
            " ".repeat(self.indent.size)
        };
        if !outdent && selected.is_empty() {
            self.command(Command::Insert(text), window, cx);
            return;
        }
        let doc = self.document.read(cx);
        let selections: Vec<_> = std::iter::once(self.state.selection)
            .chain(self.extra_selections.iter().copied())
            .collect();
        let mut rows = std::collections::BTreeSet::new();
        for selection in &selections {
            let selected = selection.range();
            let first = doc.source().offset_to_point(selected.start).row;
            let last = doc.source().offset_to_point(selected.end).row;
            for row in first..=last {
                if row > first && doc.source().line_start_offset(row) == selected.end {
                    break;
                }
                rows.insert(row);
            }
        }
        let mut edits = Vec::new();
        for row in rows {
            let start = doc.source().line_start_offset(row);
            if outdent {
                let end = doc.source().line_end_offset(row);
                let line = doc.source().slice(start..end).to_string();
                let remove = if line.starts_with('\t') {
                    1
                } else {
                    line.bytes()
                        .take(self.indent.size)
                        .take_while(|byte| *byte == b' ')
                        .count()
                };
                if remove > 0 {
                    edits.push((start..start + remove, String::new()));
                }
            } else {
                edits.push((start..start, text.clone()));
            }
        }
        if !edits.is_empty() {
            edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
            let mut selections = selections;
            for selection in &mut selections {
                for (range, text) in &edits {
                    let map = |at: usize| {
                        if at < range.start {
                            at
                        } else if at >= range.end {
                            at - range.len() + text.len()
                        } else {
                            range.start + text.len()
                        }
                    };
                    *selection = Selection::new(map(selection.anchor), map(selection.head));
                }
            }
            self.command(
                Command::BatchReplaceSelected {
                    edits,
                    selection: selections[0],
                },
                window,
                cx,
            );
            self.extra_selections = selections.into_iter().skip(1).collect();
        }
    }
    fn word_command(
        &mut self,
        backward: bool,
        extend: bool,
        delete: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if delete && !self.state.selection.range().is_empty() {
            self.command(
                Command::Replace {
                    range: self.state.selection.range(),
                    text: String::new(),
                    kind: EditKind::Command,
                },
                window,
                cx,
            );
            return;
        }
        let doc = self.document.read(cx);
        let at = self.state.selection.head;
        let row = doc.source().offset_to_point(at).row;
        let start = doc.source().line_start_offset(row);
        let end = doc.source().line_end_offset(row);
        let line = doc.source().slice(start..end).to_string();
        let target = if backward {
            line.unicode_word_indices()
                .rev()
                .find(|(ix, _)| start + ix < at)
                .map_or_else(|| doc.previous(start), |(ix, _)| start + ix)
        } else {
            line.unicode_word_indices()
                .find(|(ix, word)| start + ix + word.len() > at)
                .map_or_else(|| doc.next(end), |(ix, word)| start + ix + word.len())
        };
        let target = if self.live && self.projection.revision == doc.revision() {
            self.projection
                .leaf_at(target)
                .map_or(target, |leaf| leaf.atomic_boundary(target, !backward))
        } else {
            target
        };
        if delete {
            self.command(
                Command::Replace {
                    range: target.min(at)..target.max(at),
                    text: String::new(),
                    kind: EditKind::Command,
                },
                window,
                cx,
            );
        } else {
            let anchor = if extend {
                self.state.selection.anchor
            } else {
                target
            };
            self.command(Command::Select(Selection::new(anchor, target)), window, cx);
        }
    }
    fn add_vertical_cursor(&mut self, direction: i32, _: &mut Window, cx: &mut Context<Self>) {
        let at = self
            .extra_selections
            .last()
            .unwrap_or(&self.state.selection)
            .head;
        if let Some(offset) = self.vertical_target(at, direction as f32, cx) {
            self.add_selection(offset..offset, cx);
        }
    }
    fn visible_step(&self, at: usize, backward: bool, cx: &App) -> usize {
        let Some(leaf) = self.projection.leaf_at(at) else {
            return at;
        };
        if leaf.deferred {
            let document = self.document.read(cx);
            return if backward {
                document.previous(at)
            } else {
                document.next(at)
            };
        }
        let display = leaf.display_offset(at);
        if backward {
            if let Some((ix, _)) = leaf
                .text
                .grapheme_indices(true)
                .rev()
                .find(|(ix, _)| *ix < display)
            {
                return leaf.source_offset(ix);
            }
            self.projection
                .leaves
                .iter()
                .rev()
                // Adjacent table cells can share a source-range boundary.
                .skip_while(|other| other.id != leaf.id)
                .nth(1)
                .map_or(leaf.content_start(), |other| other.content_end())
        } else {
            if let Some((ix, _)) = leaf
                .text
                .grapheme_indices(true)
                .find(|(ix, _)| *ix > display)
            {
                return leaf.source_offset(ix);
            }
            if display < leaf.text.len() {
                return leaf.content_end();
            }
            self.projection
                .leaves
                .iter()
                .skip_while(|other| other.id != leaf.id)
                .nth(1)
                .map_or(leaf.content_end(), |other| other.content_start())
        }
    }
    fn enter(&mut self, hard: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.live
            && self
                .projection
                .leaf_at(self.state.selection.head)
                .is_some_and(|leaf| leaf.editing_math(self.state.selection.head))
        {
            self.command(Command::Insert("\n".into()), window, cx);
            return;
        }
        if self.navigate_table(
            super::tables::Navigation::Exit { backward: hard },
            window,
            cx,
        ) {
            return;
        }
        let mut range = self.state.selection.range();
        let structure = self
            .projection
            .leaf_at(range.start)
            .and_then(|leaf| match leaf.kind {
                Kind::List(style) | Kind::Quote(style) => Some(style),
                _ => None,
            });
        if self.live && self.extra_selections.is_empty() {
            if let Some(style) = structure {
                self.command(
                    Command::ListBreak {
                        hard,
                        at: Some(style.start),
                    },
                    window,
                    cx,
                );
                return;
            }
        }
        let region = self
            .live
            .then(|| self.projection.leaf_at(range.start))
            .flatten()
            .filter(|leaf| !matches!(leaf.kind, Kind::Prose | Kind::Heading(_) | Kind::Rule));
        let command = if let Some(leaf) = region {
            let (prefix, text) =
                super::regions::line_break(leaf.kind, &leaf.text, leaf.display_offset(range.start));
            if range.is_empty() {
                range.start -= prefix;
            }
            Command::Replace {
                range,
                text,
                kind: EditKind::Command,
            }
        } else if self.live {
            Command::ParagraphBreak { hard }
        } else {
            Command::Insert("\n".into())
        };
        self.command(command, window, cx);
    }
    fn delete_prose(&mut self, backward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.live
            || !self.state.selection.range().is_empty()
            || !self.extra_selections.is_empty()
        {
            self.command(
                if backward {
                    Command::Backspace
                } else {
                    Command::Delete
                },
                window,
                cx,
            );
            return;
        }
        let at = self.state.selection.head;
        let Some(leaf) = self.projection.leaf_at(at) else {
            return;
        };
        if matches!(leaf.kind, Kind::TableCell(_))
            && ((backward && at <= leaf.content_start()) || (!backward && at >= leaf.content_end()))
        {
            self.navigate_table(super::tables::Navigation::Boundary { backward }, window, cx);
            return;
        }
        if backward
            && matches!(leaf.kind, Kind::List(_) | Kind::Quote(_))
            && at <= leaf.content_start()
        {
            self.command(Command::ListBackspace, window, cx);
            return;
        }
        if super::regions::label(leaf.kind).is_some() || leaf.editing_math(at) {
            self.command(
                if backward {
                    Command::Backspace
                } else {
                    Command::Delete
                },
                window,
                cx,
            );
            return;
        }
        let other = if backward && at <= leaf.content_start() {
            self.projection
                .leaves
                .iter()
                .rev()
                .find(|other| {
                    other.range.end < leaf.range.start
                        && (!other.text.is_empty() || other.kind == Kind::Rule)
                })
                .map(|other| {
                    if matches!(other.kind, Kind::TableCell(_)) {
                        return at..at;
                    }
                    if super::regions::label(other.kind).is_some() {
                        self.document.read(cx).previous(at)..at
                    } else {
                        other.content_end()..leaf.content_start()
                    }
                })
        } else if !backward && at >= leaf.content_end() {
            self.projection
                .leaves
                .iter()
                .find(|other| {
                    other.range.start > leaf.range.end
                        && (!other.text.is_empty() || other.kind == Kind::Rule)
                })
                .map(|other| {
                    if matches!(other.kind, Kind::TableCell(_)) {
                        return at..at;
                    }
                    if super::regions::label(other.kind).is_some() {
                        at..self.document.read(cx).next(at)
                    } else {
                        leaf.content_end()..other.content_start()
                    }
                })
        } else {
            let next = self.visible_step(at, backward, cx);
            Some(next.min(at)..next.max(at))
        };
        if let Some(range) = other.filter(|range| !range.is_empty()) {
            self.command(
                Command::Replace {
                    range,
                    text: String::new(),
                    kind: EditKind::Command,
                },
                window,
                cx,
            );
        }
    }
    fn copy(&mut self, cut: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mut ranges: Vec<_> = std::iter::once(self.state.selection)
            .chain(self.extra_selections.iter().copied())
            .map(Selection::range)
            .filter(|range| !range.is_empty())
            .collect();
        if ranges.is_empty() {
            return;
        }
        ranges.sort_by_key(|range| range.start);
        let doc = self.document.read(cx);
        let text = ranges
            .iter()
            .map(|range| doc.source().slice(range.clone()).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let range = self.state.selection.range();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if cut {
            self.command(
                Command::Replace {
                    range,
                    text: String::new(),
                    kind: EditKind::Command,
                },
                window,
                cx,
            );
        }
    }
    fn paste_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        if item
            .entries()
            .iter()
            .any(|entry| matches!(entry, ClipboardEntry::Image(_)))
        {
            let images = item
                .entries
                .into_iter()
                .filter_map(|entry| match entry {
                    ClipboardEntry::Image(image) => Some(assets::IncomingImage::Clipboard(image)),
                    _ => None,
                })
                .collect();
            self.insert_images(images, window, cx);
            return;
        }
        if let Some(text) = item.text() {
            // New clipboard line breaks follow the current codec's LF insertion
            // convention; this does not rewrite any existing document region.
            let text = if text.contains("\r\n") {
                text.replace("\r\n", "\n")
            } else {
                text
            };
            self.command(
                Command::Replace {
                    range: self.state.selection.range(),
                    text,
                    kind: EditKind::Paste,
                },
                window,
                cx,
            );
        }
    }
}
impl Focusable for SourcePresentation {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl EntityInputHandler for SourcePresentation {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let doc = self.document.read(cx);
        let range = doc.utf16_range(range);
        *adjusted = Some(doc.utf16_offset(range.start)..doc.utf16_offset(range.end));
        Some(doc.source().slice(range).to_string())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let doc = self.document.read(cx);
        let selected = self.state.selection;
        let range = selected.range();
        Some(UTF16Selection {
            range: doc.utf16_offset(range.start)..doc.utf16_offset(range.end),
            reversed: selected.is_reversed(),
        })
    }
    fn marked_text_range(&self, _: &mut Window, cx: &mut Context<Self>) -> Option<Range<usize>> {
        let doc = self.document.read(cx);
        let range = self.state.marked_range()?;
        Some(doc.utf16_offset(range.start)..doc.utf16_offset(range.end))
    }
    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command(Command::FinishComposition, window, cx);
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(range) = range {
            let range = self.document.read(cx).utf16_range(range);
            if self.state.marked_range().as_ref() != Some(&range) {
                self.command(Command::FinishComposition, window, cx);
                self.command(
                    Command::Replace {
                        range,
                        text: text.into(),
                        kind: EditKind::Typing,
                    },
                    window,
                    cx,
                );
                return;
            }
        }
        self.command(Command::Commit(text.into()), window, cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range.map(|range| self.document.read(cx).utf16_range(range));
        self.command(
            Command::Compose {
                range,
                text: text.into(),
                selected,
            },
            window,
            cx,
        );
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.document.read(cx).utf16_range(range);
        self.caret_bounds(range.start, cx)
    }
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(
            self.document
                .read(cx)
                .utf16_offset(self.point_offset(point, cx)?),
        )
    }
    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.document.read(cx).utf16_range(range);
        self.command(
            Command::Select(Selection::new(range.start, range.end)),
            window,
            cx,
        );
    }
    fn text_length_utf16(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<usize> {
        let doc = self.document.read(cx);
        Some(doc.utf16_offset(doc.source().len()))
    }
}
impl Render for SourcePresentation {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let style = super::math::Style::new(cx.theme().foreground, window.rem_size());
        if self.math.style != Some(style) {
            self.math.style = Some(style);
            self.math.outputs.clear();
            self.tables.invalidate();
            for cached in self.blocks.values_mut() {
                cached.version = u64::MAX;
            }
            self.rendered_revision = None;
            self.schedule_math(cx);
        }
        if self.live && self.rendered_revision != Some(self.document.read(cx).revision()) {
            self.sync_mdx_inline_visibility(cx);
        }
        let view = cx.entity().downgrade();
        let ready = self
            .image_snapshot
            .get_or_insert_with(|| {
                std::sync::Arc::new(
                    self.document
                        .read(cx)
                        .images
                        .states
                        .iter()
                        .filter_map(|(url, state)| {
                            if let super::images::State::Ready(image) = state {
                                Some((url.clone(), image.clone()))
                            } else {
                                None
                            }
                        })
                        .collect(),
                )
            })
            .clone();
        let context = RenderContext::from_app(cx)
            .with_body_font_size(window.rem_size())
            .with_markdown_extensions(super::math::extensions())
            .with_image_source(move |uri| {
                ready.get(uri.as_ref()).map_or_else(
                    || ImageSource::Custom(std::sync::Arc::new(|_, _| None)),
                    |image| ImageSource::Render(image.clone()),
                )
            })
            .on_link_click(move |_, event, _, cx| {
                let _ = view.update(cx, |view, cx| {
                    view.dragging = false;
                    if let ClickEvent::Mouse(click) = event {
                        if click.up.button == MouseButton::Left {
                            view.finish_pointer_link(click.up.position, click.up.modifiers, cx);
                        }
                    }
                });
            });
        let table_width = self
            .viewport
            .bounds
            .map_or(window.viewport_size().width, |bounds| bounds.size.width)
            - window.rem_size() * 1.5;
        if self.tables.prepare(
            &self.projection.leaves,
            table_width,
            self.wrap,
            false,
            self.state.marked_range().as_ref(),
            &self.math,
            &context,
            window,
            cx,
        ) {
            self.viewport.heights.clear();
        }
        if self.reveal_caret {
            let at = self.reveal_at.unwrap_or(self.state.selection.head);
            if let Some(leaf) = self.projection.leaf_at(at) {
                if let Kind::TableCell(cell) = leaf.kind {
                    self.tables.reveal(cell, table_width);
                }
            }
        }
        if self.viewport.layout.is_none() {
            self.viewport.heights.clear();
        }
        // Heights are derived, versioned layout hints, never source ownership.
        // A table row is one variable-height virtual item; columns are culled separately.
        let mut before = Pixels::ZERO;
        let mut through = Pixels::ZERO;
        let mut ix = 0;
        let mut y = Pixels::ZERO;
        let screen = self.viewport_height.max(window.rem_size() * 40.);
        let top = -self.scroll.offset().y;
        let mut visible = None;
        let mut caret_row = None;
        let reading_anchor = self
            .viewport
            .anchor
            .filter(|_| !self.reveal_caret && self.scroll.offset() == self.viewport.offset)
            .map(|(id, _)| id);
        let mut anchor_row = None;
        let component_source = self.document.read(cx).source().clone();
        while ix < self.projection.leaves.len() {
            let leaf = &self.projection.leaves[ix];
            let mut end = ix + 1;
            if let Kind::TableCell(cell) = leaf.kind {
                while end < self.projection.leaves.len()
                    && matches!(self.projection.leaves[end].kind, Kind::TableCell(other) if other.same_row(cell))
                {
                    end += 1;
                }
            }
            if self.live
                && !super::mdx::view::visible_field(&super::mdx::view::FieldContext::new(
                    &self.projection,
                    leaf,
                    &component_source,
                    &self.mdx_state,
                ))
            {
                ix = end;
                continue;
            }
            let height = self
                .viewport
                .heights
                .get(&leaf.id)
                .filter(|(version, _)| *version == leaf.version)
                .map_or_else(
                    || {
                        let lines = if leaf.deferred && self.wrap {
                            let width = self
                                .viewport
                                .bounds
                                .map_or(window.rem_size() * 40., |bounds| bounds.size.width);
                            let columns =
                                (f32::from(width) / f32::from(window.rem_size()) * 1.5).max(1.);
                            (leaf.range.len() as f32 / columns).ceil().max(1.)
                        } else {
                            1.
                        };
                        window.rem_size() * (lines * 1.5 + 0.5)
                    },
                    |(_, height)| *height,
                );
            if y + height >= top - screen && y <= top + screen * 2. {
                if visible.is_none() {
                    before = y;
                }
                through = y + height;
                let range = visible.get_or_insert(ix..end);
                range.end = end;
            }
            let at = self.reveal_at.unwrap_or(self.state.selection.head);
            if self.projection.leaves[ix..end]
                .iter()
                .any(|leaf| leaf.range.start <= at && at <= leaf.range.end)
            {
                caret_row = Some((ix..end, y, height));
            }
            if reading_anchor == Some(leaf.id) {
                anchor_row = Some((ix..end, y, height));
            }
            y += height;
            ix = end;
        }
        let mut visible = visible.unwrap_or(0..0);
        if let Some((range, y, height)) = anchor_row {
            if range.start < visible.start || range.end > visible.end {
                // Remeasuring can move the old anchor outside the estimated
                // viewport. Materialize it before prepaint corrects its position.
                let offset = point(self.scroll.offset().x, -y);
                self.scroll.set_offset(offset);
                self.viewport.offset = offset;
                visible = range;
                before = y;
                through = y + height;
            }
        }
        if self.reveal_caret {
            if let Some((range, y, height)) = caret_row {
                if range.start < visible.start || range.end > visible.end {
                    self.scroll.set_offset(point(self.scroll.offset().x, -y));
                    self.viewport.anchor = None;
                    visible = range;
                    before = y;
                    through = y + height;
                }
            }
        }
        let after = (y - through).max(Pixels::ZERO);
        let revision = self.projection.revision;
        for leaf in &mut self.projection.leaves[visible.clone()] {
            leaf.materialize(self.document.read(cx).source());
        }
        {
            self.render_generation += 1;
            for leaf in &self.projection.leaves[visible.clone()] {
                let cached = self.blocks.entry(leaf.id).or_insert_with(|| CachedBlock {
                    version: u64::MAX,
                    seen: 0,
                    node: BlockNode::Unknown,
                });
                if cached.version != leaf.version {
                    if self.live && matches!(leaf.kind, Kind::Code(_) | Kind::Mermaid(_)) {
                        cached.node = BlockNode::CodeBlock(
                            CodeBlock::from_code(leaf.text.clone(), None::<SharedString>)
                                .with_leaf_key(leaf.id),
                        );
                        cached.version = leaf.version;
                        cached.seen = self.render_generation;
                        continue;
                    }
                    let mut paragraph = Paragraph::default().with_leaf_key(leaf.id);
                    let marked = self.state.marked_range();
                    for node in leaf.inline_nodes(marked.as_ref(), &self.math) {
                        paragraph.push(node);
                    }
                    let mut node = match leaf.kind {
                        Kind::Heading(level) if self.live => BlockNode::Heading {
                            level,
                            children: paragraph,
                            span: None,
                        },
                        _ => BlockNode::Paragraph(paragraph),
                    };
                    if self.live {
                        if let Kind::List(style) | Kind::Quote(style) = leaf.kind {
                            if style.task.is_none() {
                                if let Some(marker) = style.marker {
                                    node = BlockNode::List {
                                        children: vec![BlockNode::ListItem {
                                            children: vec![node],
                                            spread: false,
                                            checked: None,
                                            span: None,
                                        }],
                                        ordered: matches!(
                                            marker,
                                            super::lists::Marker::Ordered(..)
                                        ),
                                        start: match marker {
                                            super::lists::Marker::Ordered(number, _) => {
                                                Some(number)
                                            }
                                            _ => None,
                                        },
                                        span: None,
                                    };
                                }
                            }
                            for _ in 0..style.quotes {
                                node = BlockNode::Blockquote {
                                    children: vec![node],
                                    span: None,
                                };
                            }
                        }
                    }
                    cached.node = node;
                    cached.version = leaf.version;
                }
                cached.seen = self.render_generation;
            }
            self.blocks
                .retain(|_, cached| cached.seen == self.render_generation);
            self.rendered_revision = Some(revision);
        }
        let references: Vec<_> = self.projection.leaves[visible.clone()]
            .iter()
            .map(|leaf| &self.blocks[&leaf.id].node)
            .collect();
        let block = div()
            .w_full()
            .when_some(self.projection.error.as_ref(), |block, error| {
                block.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(crate::ui::i18n::t(
                            crate::ui::i18n::L10nKey::EditorMdxParseError,
                        ))
                        .child(div().text_xs().child(error.clone())),
                )
            })
            .child(div().h(before).flex_none())
            .children(
                self.projection.leaves[visible.clone()]
                    .iter()
                    .enumerate()
                    .filter_map(|(ix, leaf)| {
                        if self.live
                            && !super::mdx::view::visible_field(
                                &super::mdx::view::FieldContext::new(
                                    &self.projection,
                                    leaf,
                                    &component_source,
                                    &self.mdx_state,
                                ),
                            )
                        {
                            return None;
                        }
                        let rendered = if let Kind::TableCell(cell) = leaf.kind {
                            let global = visible.start + ix;
                            if global > visible.start
                                && matches!(self.projection.leaves[global - 1].kind, Kind::TableCell(other) if other.same_row(cell)) {
                                return None;
                            }
                            let end = global + self.projection.leaves[global..].iter().take_while(|leaf| {
                                matches!(leaf.kind, Kind::TableCell(other) if other.same_row(cell))
                            }).count();
                            let scroll = self.tables.scroll(cell).clone();
                            let (row, ids) = self.tables.row(&self.projection.leaves[global..end],
                                -scroll.offset().x, table_width, self.wrap,
                                self.state.marked_range().as_ref(), &self.math, &context,
                                &self.projection, &component_source, &self.mdx_state, window, cx);
                            let view = cx.entity().downgrade();
                            let row = row.on_children_prepainted(move |bounds, _, cx| {
                                let _ = view.update(cx, |view, _| {
                                    for (id, bounds) in ids.iter().zip(bounds) {
                                        if let Some(id) = id { view.table_geometry.insert(*id, bounds); }
                                    }
                                });
                            });
                            div().w_full().relative()
                                .child(div().id(("markdown-table-row", leaf.id)).w_full()
                                    .overflow_hidden().track_scroll(&scroll).child(row))
                                .child(gpui_kit::base::ScrollableMask::new(Axis::Horizontal, &scroll)
                                    .id(("markdown-table-pan", leaf.id)))
                                .into_any_element()
                        } else {
                            self.blocks[&leaf.id].node.render_with_id(
                                ("markdown-leaf", leaf.id),
                                FlowPosition::at(&references, ix, ix + 1 == references.len()).first(true).last(true),
                                &context, window, cx,
                            ).into_any_element()
                        };
                        let rendered = if self.live && matches!(leaf.kind, Kind::Mermaid(_)) {
                            let selection = self.state.selection.range();
                            let active = selection.start <= leaf.range.end
                                && selection.end >= leaf.range.start;
                            let at = leaf.range.start + leaf.text.find('\n').map_or(0, |at| at + 1);
                            self.mermaid.render(
                                leaf,
                                active,
                                rendered.into_any_element(),
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.select_range(at..at, cx);
                                    this.focus.focus(window, cx);
                                }),
                                cx,
                            )
                        } else {
                            rendered.into_any_element()
                        };
                        let rendered = if self.live && !matches!(leaf.kind, Kind::TableCell(_)) {
                            let field = super::mdx::view::FieldContext::new(
                                &self.projection,
                                leaf,
                                &component_source,
                                &self.mdx_state,
                            );
                            super::mdx::view::render_field(&field, rendered, window, cx)
                        } else {
                            rendered
                        };
                        Some(MeasuredBlock {
                            view: cx.entity(),
                            id: leaf.id,
                            version: leaf.version,
                            child: div()
                                .id(("markdown-block", leaf.id))
                                .w(table_width)
                                .when(
                                    self.live
                                        && super::regions::label(leaf.kind).is_some()
                                        && !matches!(leaf.kind, Kind::Mermaid(_))
                                        && !leaf.text.is_empty(),
                                    |block| {
                                        block
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(crate::ui::i18n::t(
                                                        super::regions::label(leaf.kind)
                                                            .expect("source region label"),
                                                    )),
                                            )
                                    },
                                )
                                .when(self.live && leaf.kind == Kind::Rule, |block| {
                                    block.child(
                                        BlockNode::HorizontalRule { span: None }.render_with_id(
                                            ("markdown-rule", leaf.id),
                                            FlowPosition::at(&references, ix, true),
                                            &context,
                                            window,
                                            cx,
                                        ),
                                    )
                                })
                                .child({
                                    let style = match leaf.kind {
                                        Kind::List(style) | Kind::Quote(style) if self.live => {
                                            Some(style)
                                        }
                                        _ => None,
                                    };
                                    if let Some(style) = style {
                                        let mut row = div().flex().items_start().min_w_0().w_full();
                                        let levels = style.depth.saturating_sub(1)
                                            + usize::from(
                                                style.depth > 0 && style.marker.is_none(),
                                            );
                                        row = row.pl(rems(levels as f32 * 1.5));
                                        if let Some(checked) = style.task {
                                            let at = leaf.range.start;
                                            row = row.gap_2().child(
                                                div().on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                    cx.stop_propagation();
                                                }).child(
                                                gpui_kit::component::checkbox::Checkbox::new((
                                                    "markdown-task",
                                                    leaf.id,
                                                ))
                                                .checked(checked)
                                                .accessibility_label(crate::ui::i18n::t_fmt(
                                                    L10nKey::EditorMarkdownTaskLabel,
                                                    &[("text", &leaf.text)],
                                                ))
                                                .on_change(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.command(
                                                            Command::ToggleTask { at },
                                                            window,
                                                            cx,
                                                        );
                                                        this.focus.focus(window, cx);
                                                    },
                                                )),
                                                ),
                                            );
                                        }
                                        row.child(div().flex_1().min_w_0().child(rendered))
                                            .into_any_element()
                                    } else {
                                        rendered
                                    }
                                })
                                .into_any_element(),
                        })
                    }),
            )
            .child(div().h(after).flex_none());
        let image_status = if self.live {
            use crate::ui::i18n::{L10nKey, t};
            use gpui_kit::component::{
                Sizable as _,
                button::{Button, ButtonVariants as _},
            };
            let document = self.document.read(cx);
            let mut states: Vec<_> = document.images.states.iter().collect();
            states.sort_by_key(|(url, _)| *url);
            let has_images = !states.is_empty();
            let rows = states
                .into_iter()
                .map(|(url, state)| {
                    let (key, detail) = match state {
                        super::images::State::Unloaded => (L10nKey::EditorImageUnloaded, None),
                        super::images::State::Loading(_) => (L10nKey::EditorImageLoading, None),
                        super::images::State::Ready(_) => (L10nKey::EditorImageLoaded, None),
                        super::images::State::Failed(error) => {
                            (L10nKey::EditorImageFailed, Some(error))
                        }
                    };
                    div()
                        .id(SharedString::from(format!("markdown-image:{url}")))
                        .role(Role::Status)
                        .test_support()
                        .aria_label(t(key))
                        .text_xs()
                        .truncate()
                        .child(format!(
                            "{} · {}{}",
                            t(key),
                            url,
                            detail.map_or(String::new(), |error| format!(" · {error}"))
                        ))
                })
                .collect::<Vec<_>>();
            div()
                .v_flex()
                .flex_none()
                .when(has_images, |bar| {
                    bar.px_3()
                        .py_1()
                        .gap_1()
                        .child(
                            Button::new("markdown-load-images")
                                .ghost()
                                .small()
                                .label(t(L10nKey::EditorLoadImages))
                                .on_click(
                                    cx.listener(|view, _, _, cx| view.refresh_images(true, cx)),
                                ),
                        )
                        .children(rows)
                })
                .into_any_element()
        } else {
            div().into_any_element()
        };
        let key_context = KeyContext::parse(if !self.focus.is_focused(window) {
            "Markdown"
        } else if self.table_commands_enabled() {
            "Input mode=editor Markdown MarkdownTable"
        } else {
            "Input mode=editor Markdown"
        })
        .expect("static editor key context");
        let root =
            div()
                .id("markdown-source")
                .test_support()
                .size_full()
                .flex()
                .flex_col()
                .min_h_0()
                .key_context(key_context)
                .track_focus(&self.focus)
                .cursor_text()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .when(!self.live, |root| {
                    root.font_family(cx.theme().mono_font_family.clone())
                })
                .when(!self.wrap, |root| root.whitespace_nowrap())
                .role(Role::TextInput)
                .aria_label(t(if self.live {
                    L10nKey::EditorMarkdownLiveLabel
                } else {
                    L10nKey::EditorMarkdownSourceLabel
                }))
                .when(window.is_a11y_active() || cfg!(test), |root| {
                    let document = self.document.read(cx);
                    let (revision, value) = self.accessibility_snapshot.get_or_insert_with(|| {
                        (document.revision(), document.source().to_string().into())
                    });
                    if *revision != document.revision() {
                        *revision = document.revision();
                        *value = document.source().to_string().into();
                    }
                    root.aria_value(value.clone())
                })
                .on_a11y_action(AccessibleAction::Focus, {
                    let view = cx.entity();
                    move |_, window, cx| {
                        view.read(cx).focus.clone().focus(window, cx);
                    }
                })
                .on_a11y_action(AccessibleAction::SetValue, {
                    let view = cx.entity();
                    move |data, window, cx| {
                        if let Some(accesskit::ActionData::Value(text)) = data {
                            view.update(cx, |view, cx| {
                                let end = view.document.read(cx).source().len();
                                view.command(
                                    Command::Replace {
                                        range: 0..end,
                                        text: text.to_string(),
                                        kind: EditKind::Command,
                                    },
                                    window,
                                    cx,
                                );
                            });
                        }
                    }
                })
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    let key = &event.keystroke;
                    if this.live && key.modifiers.alt && !key.modifiers.control && !key.modifiers.platform
                        && matches!(key.key.as_str(), "pageup" | "pagedown") {
                        let target = super::mdx::tabs::adjacent(&this.projection.mdx.components, this.document.read(cx).source(), &this.mdx_state, this.state.selection, key.key == "pageup");
                        if let Some((group, child)) = target {
                            this.activate_mdx_tab(group, child, window, cx);
                            cx.stop_propagation();
                            return;
                        }
                    }
                    if event.keystroke.key == "escape" && this.state.composition.is_some() {
                        this.command(Command::CancelComposition, window, cx);
                        cx.stop_propagation();
                    }
                }))
                .drag_over::<ExternalPaths>(|style, _, _, cx| style.bg(cx.theme().drag_border.opacity(0.12)))
                .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                    this.drop_images(paths, window, cx);
                    cx.stop_propagation();
                }))
                .on_action(cx.listener(|view, _: &LoadImages, _, cx| view.refresh_images(true, cx)))
                .on_action(cx.listener(|this, _: &EditLinkDestination, window, cx| {
                    this.edit_link_destination(window, cx);
                }))
                .on_action(cx.listener(|this, _: &FollowLink, _, cx| {
                    if let Some(link) = this.projection.links.at(this.state.selection.head()).cloned() {
                        this.follow_link(link.target, cx);
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        this.focus.focus(window, cx);
                        this.pointer_link = link_modifier(event.modifiers)
                            .then(|| this.pointer_link_target(event.position, cx))
                            .flatten()
                            .map(|target| (this.projection.revision, target.to_owned()));
                        if this.pointer_link.is_some() {
                            this.dragging = false;
                        } else {
                            this.dragging = true;
                            this.select_point(event.position, event.modifiers.shift, window, cx);
                        }
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                    if this.dragging {
                        this.select_point(event.position, true, window, cx);
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseUpEvent, _, cx| {
                        this.dragging = false;
                        // editor_host may consume a rendered link's mouse-up.
                        // Otherwise the editing surface completes the same
                        // captured gesture; neither path navigates twice.
                        this.finish_pointer_link(event.position, event.modifiers, cx);
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| {
                        this.dragging = false;
                        this.pointer_link = None;
                    }),
                )
                .on_action(cx.listener(|this, _: &super::tables::InsertRowBefore, window, cx| this.table_command(super::tables::Operation::InsertRowBefore, window, cx)))
                .on_action(cx.listener(|this, _: &super::tables::InsertRowAfter, window, cx| this.table_command(super::tables::Operation::InsertRowAfter, window, cx)))
                .on_action(cx.listener(|this, _: &super::tables::DeleteRow, window, cx| this.table_command(super::tables::Operation::DeleteRow, window, cx)))
                .on_action(cx.listener(|this, _: &super::tables::InsertColumnBefore, window, cx| this.table_command(super::tables::Operation::InsertColumnBefore, window, cx)))
                .on_action(cx.listener(|this, _: &super::tables::InsertColumnAfter, window, cx| this.table_command(super::tables::Operation::InsertColumnAfter, window, cx)))
                .on_action(cx.listener(|this, _: &super::tables::DeleteColumn, window, cx| this.table_command(super::tables::Operation::DeleteColumn, window, cx)))
                .on_action(cx.listener(|this, _: &super::tables::AlignDefault, window, cx| this.table_command(super::tables::Operation::AlignDefault, window, cx)))
                .on_action(cx.listener(|this, _: &super::tables::AlignLeft, window, cx| this.table_command(super::tables::Operation::AlignLeft, window, cx)))
                .on_action(cx.listener(|this, _: &super::tables::AlignCenter, window, cx| this.table_command(super::tables::Operation::AlignCenter, window, cx)))
                .on_action(cx.listener(|this, _: &super::tables::AlignRight, window, cx| this.table_command(super::tables::Operation::AlignRight, window, cx)))
                .on_action(
                    cx.listener(|this, _: &actions::Copy, window, cx| this.copy(false, window, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &actions::Cut, window, cx| this.copy(true, window, cx)),
                )
                .on_action(cx.listener(|this, _: &actions::Paste, window, cx| {
                    this.paste_clipboard(window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::Undo, window, cx| {
                    this.command(Command::Undo, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::Redo, window, cx| {
                    this.command(Command::Redo, window, cx)
                }))
                .on_action(cx.listener(|this, _: &crate::core::actions::ToggleMarkdownTask, window, cx| {
                    let task = this.projection.leaf_at(this.state.selection.head).and_then(|leaf| match leaf.kind {
                        Kind::List(style) if style.task.is_some() => Some(style.start),
                        _ => None,
                    });
                    if this.live && !this.is_composing() {
                        if let Some(at) = task {
                            this.command(Command::ToggleTask { at }, window, cx);
                            return;
                        }
                    }
                    cx.propagate();
                }))
                .on_action(cx.listener(|this, _: &actions::Backspace, window, cx| {
                    this.delete_prose(true, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::Delete, window, cx| {
                    this.delete_prose(false, window, cx)
                }))
                .on_action(cx.listener(|this, action: &actions::Enter, window, cx| {
                    this.enter(action.shift, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::Indent, window, cx| {
                    this.indent_selection(false, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::IndentInline, window, cx| {
                    this.indent_selection(false, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::Outdent, window, cx| {
                    this.indent_selection(true, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::OutdentInline, window, cx| {
                    this.indent_selection(true, window, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &actions::DeleteToBeginningOfLine, window, cx| {
                        this.delete_line(false, window, cx)
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &actions::DeleteToEndOfLine, window, cx| {
                        this.delete_line(true, window, cx)
                    }),
                )
                .on_action(cx.listener(|this, _: &actions::MovePageUp, window, cx| {
                    this.move_page(-1., window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::MovePageDown, window, cx| {
                    this.move_page(1., window, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &actions::MoveToPreviousWord, window, cx| {
                        this.word_command(true, false, false, window, cx)
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &actions::MoveToNextWord, window, cx| {
                        this.word_command(false, false, false, window, cx)
                    }),
                )
                .on_action(cx.listener(
                    |this, _: &actions::SelectToPreviousWordStart, window, cx| {
                        this.word_command(true, true, false, window, cx)
                    },
                ))
                .on_action(
                    cx.listener(|this, _: &actions::SelectToNextWordEnd, window, cx| {
                        this.word_command(false, true, false, window, cx)
                    }),
                )
                .on_action(cx.listener(
                    |this, _: &actions::DeleteToPreviousWordStart, window, cx| {
                        this.word_command(true, false, true, window, cx)
                    },
                ))
                .on_action(
                    cx.listener(|this, _: &actions::DeleteToNextWordEnd, window, cx| {
                        this.word_command(false, false, true, window, cx)
                    }),
                )
                .on_action(cx.listener(
                    |this, _: &gpui_kit::base::input::AddCursorAbove, window, cx| {
                        this.add_vertical_cursor(-1, window, cx)
                    },
                ))
                .on_action(cx.listener(
                    |this, _: &gpui_kit::base::input::AddCursorBelow, window, cx| {
                        this.add_vertical_cursor(1, window, cx)
                    },
                ))
                .on_action(cx.listener(|this, _: &actions::SelectAll, window, cx| {
                    let end = this.document.read(cx).source().len();
                    this.command(Command::Select(Selection::new(0, end)), window, cx);
                }))
                .on_action(cx.listener(|this, _: &actions::MoveLeft, window, cx| {
                    this.move_cursor(-1, 0, false, None, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::MoveRight, window, cx| {
                    this.move_cursor(1, 0, false, None, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::MoveUp, window, cx| {
                    this.move_cursor(0, -1, false, None, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::MoveDown, window, cx| {
                    this.move_cursor(0, 1, false, None, window, cx)
                }))
                .on_action(cx.listener(
                    |this, _: &gpui_kit::base::actions::SelectLeft, window, cx| {
                        this.move_cursor(-1, 0, true, None, window, cx)
                    },
                ))
                .on_action(cx.listener(
                    |this, _: &gpui_kit::base::actions::SelectRight, window, cx| {
                        this.move_cursor(1, 0, true, None, window, cx)
                    },
                ))
                .on_action(cx.listener(
                    |this, _: &gpui_kit::base::actions::SelectUp, window, cx| {
                        this.move_cursor(0, -1, true, None, window, cx)
                    },
                ))
                .on_action(cx.listener(
                    |this, _: &gpui_kit::base::actions::SelectDown, window, cx| {
                        this.move_cursor(0, 1, true, None, window, cx)
                    },
                ))
                .on_action(
                    cx.listener(|this, _: &actions::MoveToStartOfLine, window, cx| {
                        this.move_cursor(0, 0, false, Some(false), window, cx)
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &actions::MoveToEndOfLine, window, cx| {
                        this.move_cursor(0, 0, false, Some(true), window, cx)
                    }),
                )
                .on_action(cx.listener(|this, _: &actions::MoveHome, window, cx| {
                    this.move_cursor(0, 0, false, Some(false), window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::MoveEnd, window, cx| {
                    this.move_cursor(0, 0, false, Some(true), window, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &actions::SelectToStartOfLine, window, cx| {
                        this.move_cursor(0, 0, true, Some(false), window, cx)
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &actions::SelectToEndOfLine, window, cx| {
                        this.move_cursor(0, 0, true, Some(true), window, cx)
                    }),
                )
                .on_action(cx.listener(|this, _: &actions::MoveToStart, window, cx| {
                    this.command(Command::Select(Selection::caret(0)), window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::MoveToEnd, window, cx| {
                    let end = this.document.read(cx).source().len();
                    this.command(Command::Select(Selection::caret(end)), window, cx);
                }))
                .on_action(cx.listener(|this, _: &actions::SelectToStart, window, cx| {
                    this.command(
                        Command::Select(Selection::new(this.state.selection.anchor, 0)),
                        window,
                        cx,
                    )
                }))
                .on_action(cx.listener(|this, _: &actions::SelectToEnd, window, cx| {
                    let end = this.document.read(cx).source().len();
                    this.command(
                        Command::Select(Selection::new(this.state.selection.anchor, end)),
                        window,
                        cx,
                    );
                }))
                .on_action(cx.listener(|this, _: &actions::Escape, window, cx| {
                    if this.state.composition.is_some() {
                        this.command(Command::CancelComposition, window, cx);
                    } else if this.navigate_table(super::tables::Navigation::Exit { backward: false }, window, cx) {
                        // Escape leaves the table without changing its source.
                    } else {
                        cx.propagate();
                    }
                }))
                .when_some(self.assets.status(), |root, status| root.child(
                    div().id("markdown-image-insertion-status").test_support().role(Role::Status)
                        .flex_shrink_0().aria_label(status.clone()).text_sm().text_color(cx.theme().muted_foreground).child(status)
                ))
                .child(image_status)
                .when(self.table_commands_enabled(), |root| {
                    use gpui_kit::component::{Sizable as _, button::{Button, ButtonVariants as _}, menu::DropdownMenu as _};
                    let view = cx.entity().downgrade();
                    let focus = self.focus.clone();
                    root.child(gpui_kit::component::h_flex().flex_none()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(Button::new("markdown-table-commands").small().ghost()
                            .label(crate::ui::i18n::t(crate::ui::i18n::L10nKey::EditorTableCommands))
                            .dropdown_menu(move |menu, _, cx| {
                                let menu = menu.action_context(focus.clone());
                                match view.upgrade() {
                                    Some(view) => view.read(cx).table_menu(menu),
                                    None => menu,
                                }
                            })))
                })
                .when(self.live && self.projection.links.at(self.state.selection.head()).is_some(), |root| {
                    use gpui_kit::component::button::{Button, ButtonVariants as _};
                    use gpui_kit::component::Sizable as _;
                    root.child(gpui_kit::component::h_flex().gap_2().flex_none()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(Button::new("markdown-link-destination").small().ghost()
                            .label(crate::ui::i18n::t(if self.projection.links.at(self.state.selection.head())
                                .is_some_and(|link| !link.source.contains(&link.destination.start)) {
                                    crate::ui::i18n::L10nKey::MarkdownEditReferenceDestination
                                } else { crate::ui::i18n::L10nKey::MarkdownEditLinkDestination }))
                            .on_click(cx.listener(|this, _, window, cx| this.edit_link_destination(window, cx))))
                        .child(Button::new("markdown-follow-link").small().ghost()
                            .label(crate::ui::i18n::t(crate::ui::i18n::L10nKey::MarkdownFollowLink))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.focus.focus(window, cx);
                                if let Some(link) = this.projection.links.at(this.state.selection.head()).cloned() {
                                    this.follow_link(link.target, cx);
                                }
                            }))))
                })
                .child(crate::ui::scrollbar::with_vertical_scrollbar(
                    "markdown-source-scrollbar",
                    div()
                        .id("markdown-source-scroll")
                        .flex_1().min_h_0().w_full()
                        .overflow_y_scroll()
                        .overflow_x_scroll()
                        .track_scroll(&self.scroll)
                        .on_scroll(cx.listener(|view, _, _, cx| {
                            view.geometry = None;
                            view.reveal_caret = false;
                            cx.notify();
                        }))
                        .child(
                            div()
                                .p_3()
                                .capture_any_mouse_down(cx.listener(
                                    |this, event: &MouseDownEvent, window, cx| {
                                        if event.button != MouseButton::Left
                                            || !this.geometry.as_ref().is_some_and(|(revision, geometry)| {
                                                *revision == this.document.read(cx).revision()
                                                    && geometry.values().any(|leaf| leaf.bounds().contains(&event.position))
                                            })
                                        {
                                            return;
                                        }
                                        if link_modifier(event.modifiers) {
                                            this.pointer_link = this.pointer_link_target(event.position, cx)
                                                .map(|target| (this.projection.revision, target.to_owned()));
                                            if this.pointer_link.is_some() {
                                                this.dragging = false;
                                                return;
                                            }
                                        }
                                        this.focus.focus(window, cx);
                                        this.dragging = true;
                                        this.select_point(
                                            event.position,
                                            event.modifiers.shift,
                                            window,
                                            cx,
                                        );
                                        cx.stop_propagation();
                                    },
                                ))
                                .child(block),
                        ),
                    &self.scroll,
                ));
        SourceElement {
            child: root.into_any_element(),
            view: cx.entity(),
            revision,
            color: cx.theme().selection,
            caret: cx.theme().caret,
            visible,
            table_context: context,
            preserve_reading_anchor: reading_anchor.is_some(),
        }
    }
}

struct MeasuredBlock {
    child: AnyElement,
    view: Entity<SourcePresentation>,
    id: u64,
    version: u64,
}
impl IntoElement for MeasuredBlock {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for MeasuredBlock {
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
        self.view.update(cx, |view, _| {
            view.viewport.frame.push((self.id, self.version, bounds))
        });
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}
struct SourceElement {
    child: AnyElement,
    view: Entity<SourcePresentation>,
    revision: u64,
    color: Hsla,
    caret: Hsla,
    preserve_reading_anchor: bool,
    visible: Range<usize>,
    table_context: RenderContext,
}
impl IntoElement for SourceElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for SourceElement {
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
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, _| {
            view.viewport.frame.clear();
            view.table_geometry.clear();
        });
        let table_measurement_changed = self.view.update(cx, |view, cx| {
            let width = view
                .viewport
                .bounds
                .map_or(window.viewport_size().width, |bounds| bounds.size.width)
                - window.rem_size() * 1.5;
            let marked = view.state.marked_range();
            let columns = view.tables.prepare(
                &view.projection.leaves,
                width,
                view.wrap,
                true,
                marked.as_ref(),
                &view.math,
                &self.table_context,
                window,
                cx,
            );
            let rows = view.tables.measure_rows(
                &view.projection.leaves[self.visible.clone()],
                view.wrap,
                marked.as_ref(),
                &view.math,
                &self.table_context,
                window,
                cx,
            );
            if columns {
                view.viewport.heights.clear();
            }
            if columns || rows {
                cx.notify();
            }
            columns || rows
        });
        let decorations = {
            let view = self.view.read(cx);
            view.projection
                .leaves
                .iter()
                .filter(|leaf| view.blocks.contains_key(&leaf.id))
                .map(|leaf| {
                    let mut ranges = Vec::new();
                    for range in view.search_matches.iter() {
                        let start = range.start.max(leaf.range.start);
                        let end = range.end.min(leaf.range.end);
                        let mut color = self.color;
                        color.a *= 0.45;
                        if start < end {
                            ranges.push((
                                leaf.display_offset(start)..leaf.display_offset(end),
                                color,
                            ));
                        }
                    }
                    for selection in std::iter::once(view.state.selection)
                        .chain(view.extra_selections.iter().copied())
                    {
                        let selection = selection.range();
                        let start = selection.start.max(leaf.range.start);
                        let end = selection.end.min(leaf.range.end);
                        if start < end {
                            ranges.push((
                                leaf.display_offset(start)..leaf.display_offset(end),
                                self.color,
                            ));
                        }
                    }
                    (leaf.id, ranges)
                })
                .collect()
        };
        let (_, geometry) = collect_leaf_geometry(decorations, || self.child.prepaint(window, cx));
        self.view.update(cx, |view, cx| {
            if table_measurement_changed {
                view.geometry = None;
                view.viewport.bounds = Some(view.scroll.bounds());
                view.viewport_height = view.scroll.bounds().size.height;
                return;
            }
            if view.document.read(cx).revision() != self.revision
                || view.projection.revision != self.revision
            {
                view.geometry = None;
                return;
            }
            let viewport = view.scroll.bounds();
            let layout = (viewport.size.width, window.rem_size());
            let reflow = view.viewport.layout.is_some_and(|old| old != layout);
            if reflow {
                view.viewport.heights.clear();
            }
            view.viewport.layout = Some(layout);
            let mut measured = false;
            for (id, version, bounds) in &view.viewport.frame {
                let height = bounds.size.height.max(window.rem_size());
                measured |= view.viewport.heights.insert(*id, (*version, height))
                    != Some((*version, height));
            }
            // Anchor a source leaf, not a document-wide pixel offset. Only
            // compensate layout changes, never an explicit user scroll.
            let offset = view.scroll.offset();
            let mut preserve_anchor = false;
            if self.preserve_reading_anchor && !view.reveal_caret {
                if let Some((id, y)) = view.viewport.anchor {
                    if let Some((_, _, row)) =
                        view.viewport.frame.iter().find(|(key, _, _)| *key == id)
                    {
                        let delta = y - row.top();
                        // Keep a pending correction, but once it settles anchor
                        // the top visible row instead of an overscan-only row.
                        preserve_anchor = delta.abs() > px(0.5);
                        let next_y =
                            (offset.y + delta).clamp(-view.scroll.max_offset().y, Pixels::ZERO);
                        if (next_y - offset.y).abs() > px(0.5) {
                            view.scroll.set_offset(point(offset.x, next_y));
                            measured = true;
                        }
                    }
                }
            }
            if !preserve_anchor {
                view.viewport.anchor = view
                    .viewport
                    .frame
                    .iter()
                    .find(|(_, _, row)| {
                        row.top() >= viewport.top() && row.top() < viewport.bottom()
                    })
                    .or_else(|| {
                        view.viewport
                            .frame
                            .iter()
                            .find(|(_, _, row)| row.bottom() > viewport.top())
                    })
                    .map(|(id, _, row)| (*id, row.top()));
            }
            view.viewport.offset = view.scroll.offset();
            if reflow {
                view.invalidate_layout(cx);
            }
            if measured || reflow {
                cx.notify();
            }
            let previous_caret = view.caret_bounds(view.state.selection.head, cx);
            view.geometry = Some((self.revision, geometry));
            view.viewport_height = viewport.size.height;
            view.viewport.bounds = Some(viewport);
            if view.focus.is_focused(window)
                && (view.selection_notification
                    || previous_caret != view.caret_bounds(view.state.selection.head, cx))
            {
                view.selection_notification = false;
                window.notify_text_selection_changed(&view.focus);
                window.invalidate_character_coordinates();
            }
            if view.reveal_caret && view.focus.is_focused(window) {
                let at = view.reveal_at.unwrap_or(view.state.selection.head);
                if let Some(caret) = view.caret_bounds(at, cx) {
                    let inset = window.rem_size() * 0.75;
                    let delta = if caret.top() < viewport.top() + inset {
                        viewport.top() + inset - caret.top()
                    } else if caret.bottom() > viewport.bottom() - inset {
                        viewport.bottom() - inset - caret.bottom()
                    } else {
                        px(0.)
                    };
                    let delta_x = if caret.left() < viewport.left() + inset {
                        viewport.left() + inset - caret.left()
                    } else if caret.right() > viewport.right() - inset {
                        viewport.right() - inset - caret.right()
                    } else {
                        px(0.)
                    };
                    if caret.top() >= viewport.top()
                        && caret.bottom() <= viewport.bottom()
                        && caret.left() >= viewport.left()
                        && caret.right() <= viewport.right()
                    {
                        // Completion is observable only in a frame laid out at
                        // the final offset, not the frame requesting that offset.
                        view.reveal_caret = false;
                        view.reveal_at = None;
                        cx.notify();
                    } else {
                        let offset = view.scroll.offset();
                        let maximum = view.scroll.max_offset();
                        let next = point(
                            (offset.x + delta_x).clamp(-maximum.x, Pixels::ZERO),
                            (offset.y + delta).clamp(-maximum.y, Pixels::ZERO),
                        );
                        if next != offset {
                            view.scroll.set_offset(next);
                            view.geometry = None;
                            // Window::refresh ignores calls during drawing.
                            // Notify the owning entity so virtualization rebuilds
                            // for the new offset and verifies the pending reveal.
                            cx.notify();
                        }
                    }
                }
            }
            if view.scroll.offset() != offset {
                // This frame was measured before the scroll correction. Async
                // resources must not infer caret visibility from stale bounds.
                view.geometry = None;
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
        let (focus, carets) = {
            let view = self.view.read(cx);
            let carets: Vec<_> = if view.focus.is_focused(window) {
                std::iter::once(view.state.selection)
                    .chain(view.extra_selections.iter().copied())
                    .filter(|selection| selection.range().is_empty())
                    .filter_map(|selection| view.caret_bounds(selection.head, cx))
                    .collect()
            } else {
                Vec::new()
            };
            (view.focus.clone(), carets)
        };
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        let viewport = self.view.read(cx).scroll.bounds();
        for caret in carets {
            if caret.top() >= viewport.top()
                && caret.bottom() <= viewport.bottom()
                && caret.left() >= viewport.left()
                && caret.right() <= viewport.right()
            {
                window.paint_quad(fill(caret, self.caret));
            }
        }
    }
}

fn link_modifier(modifiers: Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.platform && !modifiers.control && !modifiers.alt && !modifiers.shift
    } else {
        modifiers.control && !modifiers.platform && !modifiers.alt && !modifiers.shift
    }
}
