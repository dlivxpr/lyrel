//! YAML 文档：一份共享逐行保真源码，加上按 revision 派生的无损结构树。
//!
//! 源码始终是同一 [`lyrel_markdown_editor::Document`] 的 plain-source
//! presentation；保存、撤销、reload 与关闭都走既有工作台生命周期。结构节点
//! 只保存源码范围与显示文本，不复制第二份可变内容，也不通过 JSON／YamlValue
//! 回写。anchors／aliases／tags／comments 与多文档都是真实语义，不展开成副本。
//!
//! 解析在 UI 线程外进行：有界输入、每份文档单飞、只安装与当前 revision／
//! generation 匹配的投影。解析失败或超预算时源码仍然可编辑。树内编辑由
//! `mutation` 规划局部 splice，并经共享 `Document::replace_ranges` 成为一条
//! 普通 history；写入前按 revision 与整篇解析复核。

use std::ops::Range;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui_kit::ScrollStrategy;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, h_flex, spinner::Spinner, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton, Render,
    SharedString, Subscription, Task, TestSupportExt as _, UniformListScrollHandle, Window, div,
    uniform_list,
};
use lyrel_markdown_editor::{Document, DocumentEvent, EditorState as SourceEditor};

use super::structure_edit::{EditField, StructureEditEvent, StructureEditPanel};
use super::structure_rows::{
    StructureRowData, source_body_for, structure_error, structure_loading, structure_row,
};
use super::structure_view::{self, SourceLocation, Treelike, VisibleRow};
use crate::ui::i18n::{L10nKey, t, t_fmt};

pub(crate) mod mutation;
pub(crate) mod projection;

use mutation::{Mutation, MutationError, NodeTarget, UnsupportedEdit};
use projection::{
    ContainerKind, ProjectionFailure, STRUCTURE_DEPTH, STRUCTURE_INPUT_BYTES, STRUCTURE_NODES,
    YamlNodeKind, YamlProjection, YamlScalarKind,
};

/// 合并连续输入；每份文档最多一个 worker 与一次在途解析。
const PARSE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(120);
/// 打开时自动展开根的直接子容器，复现上游的浅层可读树。
const AUTO_EXPAND_CHILDREN: usize = 8;
/// 键盘 PageUp／PageDown 跨过的可见行数。
const PAGE_ROWS: isize = 10;

/// 结构预览持有键盘时成立的上下文；源码面不声明它，文字编辑键不会被截走。
pub(crate) const KEY_CONTEXT: &str = "YamlStructure";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum YamlMode {
    Structure,
    Source,
}

impl YamlMode {
    pub(crate) fn opposite(self) -> Self {
        match self {
            Self::Structure => Self::Source,
            Self::Source => Self::Structure,
        }
    }
}

/// 一份 YAML 文件的内容：共享 Document 与按最新 revision 安装的投影。
pub(crate) struct YamlDocument {
    document: Entity<Document>,
    projection: Option<Arc<YamlProjection>>,
    failure: Option<ProjectionFailure>,
    parsing: bool,
    installed_revision: Option<u64>,
    generation: u64,
    pending: bool,
    worker_running: bool,
    cancel: Arc<AtomicBool>,
    _worker: Option<Task<()>>,
    _document: Subscription,
}

impl Drop for YamlDocument {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl YamlDocument {
    pub(super) fn new(document: Entity<Document>, cx: &mut Context<Self>) -> Self {
        let watch =
            cx.subscribe(&document, |this, _, _: &DocumentEvent, cx| this.invalidate(cx));
        let mut this = Self {
            document,
            projection: None,
            failure: None,
            parsing: false,
            installed_revision: None,
            generation: 0,
            pending: false,
            worker_running: false,
            cancel: Arc::default(),
            _worker: None,
            _document: watch,
        };
        this.invalidate(cx);
        this
    }

    pub(crate) fn document(&self) -> &Entity<Document> {
        &self.document
    }

    pub(crate) fn projection(&self) -> Option<&YamlProjection> {
        self.projection.as_deref()
    }

    pub(crate) fn failure(&self) -> Option<ProjectionFailure> {
        self.failure
    }

    pub(crate) fn is_parsing(&self) -> bool {
        self.parsing
    }

    pub(crate) fn installed_revision(&self) -> Option<u64> {
        self.installed_revision
    }

    fn invalidate(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Release);
        self.generation = self.generation.wrapping_add(1);
        self.projection = None;
        self.failure = None;
        self.parsing = true;
        self.pending = true;
        if !self.worker_running {
            self.start_worker(cx);
        }
        cx.notify();
    }

    fn start_worker(&mut self, cx: &mut Context<Self>) {
        self.worker_running = true;
        self._worker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PARSE_DEBOUNCE).await;
                let job = this
                    .update(cx, |this, cx| {
                        if !this.pending {
                            this.parsing = false;
                            this.worker_running = false;
                            cx.notify();
                            return None;
                        }
                        this.pending = false;
                        let document = this.document.read(cx);
                        let snapshot = document.snapshot();
                        let revision = document.revision();
                        let generation = this.generation;
                        this.cancel.store(false, Ordering::Release);
                        Some((snapshot, revision, generation, this.cancel.clone()))
                    })
                    .ok()
                    .flatten();
                let Some((snapshot, revision, generation, cancel)) = job else {
                    break;
                };
                #[cfg(any(test, feature = "native-markdown-tests"))]
                let gate = test_support::take_next_parse();
                let result = crate::ui::host_ops::off_thread(move || {
                    if cancel.load(Ordering::Acquire) {
                        return None;
                    }
                    #[cfg(any(test, feature = "native-markdown-tests"))]
                    if let Some(gate) = gate {
                        gate.block();
                    }
                    Some(projection::project(snapshot.text(), &cancel))
                })
                .await
                .flatten()
                .unwrap_or(Err(ProjectionFailure::Cancelled));
                let keep_going = this
                    .update(cx, |this, cx| {
                        let document = this.document.read(cx);
                        if this.generation == generation && document.revision() == revision {
                            match result {
                                Ok(projection) => {
                                    this.projection = Some(Arc::new(projection));
                                    this.failure = None;
                                    this.installed_revision = Some(revision);
                                }
                                Err(ProjectionFailure::Cancelled) => {}
                                Err(failure) => {
                                    this.projection = None;
                                    this.failure = Some(failure);
                                    this.installed_revision = None;
                                }
                            }
                        }
                        this.parsing = this.pending;
                        if !this.pending {
                            this.worker_running = false;
                        }
                        cx.notify();
                        this.pending
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        }));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditKind {
    Value,
    Key,
    AddMember,
    AddItem,
}

struct EditSession {
    panel: Entity<StructureEditPanel>,
    kind: EditKind,
    target: NodeTarget,
    _events: Subscription,
}

#[derive(Default)]
struct EditCaps {
    value: bool,
    key: bool,
    add: bool,
    add_is_sequence: bool,
    remove: bool,
}

/// 一份 YAML 文件在一个分组里的呈现：模式、源码选择、展开与滚动。
pub(crate) struct YamlView {
    document: Entity<YamlDocument>,
    source: Entity<SourceEditor>,
    mode: YamlMode,
    focus: FocusHandle,
    structure_scroll: UniformListScrollHandle,
    expanded: Vec<bool>,
    rows: Vec<VisibleRow>,
    selected: Option<usize>,
    selected_path: Option<String>,
    editing: Option<EditSession>,
    mutation_error: Option<SharedString>,
    mutation_notice: Option<SharedString>,
    seen_revision: Option<u64>,
    _document: Subscription,
    _source_document: Subscription,
    _source: Subscription,
}

impl YamlView {
    pub(super) fn new(
        document: Entity<YamlDocument>,
        mode: YamlMode,
        wrap: bool,
        indent: usize,
        cx: &mut Context<Self>,
    ) -> Self {
        let source_document = document.read(cx).document().clone();
        let source = cx.new(|cx| {
            let mut source = SourceEditor::for_document(source_document.clone(), cx);
            source.set_plain_source(true, cx);
            source.set_soft_wrap(wrap, cx);
            source.set_tab_indent(indent);
            source
        });
        let watch_document = cx.observe(&document, |this, _, cx| {
            this.sync_structure(cx);
            cx.notify();
        });
        let watch_source_document =
            cx.subscribe(&source_document, |_, _, _: &DocumentEvent, cx| {
                cx.notify();
            });
        let watch_source = cx.observe(&source, |_, _, cx| cx.notify());
        let mut view = Self {
            document,
            source,
            mode,
            focus: cx.focus_handle(),
            structure_scroll: UniformListScrollHandle::new(),
            expanded: Vec::new(),
            rows: Vec::new(),
            selected: None,
            selected_path: None,
            editing: None,
            mutation_error: None,
            mutation_notice: None,
            seen_revision: None,
            _document: watch_document,
            _source_document: watch_source_document,
            _source: watch_source,
        };
        view.sync_structure(cx);
        view
    }

    pub(crate) fn document(&self) -> &Entity<YamlDocument> {
        &self.document
    }

    pub(crate) fn source(&self) -> &Entity<SourceEditor> {
        &self.source
    }

    pub(crate) fn mode(&self) -> YamlMode {
        self.mode
    }

    pub(crate) fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.mode {
            YamlMode::Source => self.source.read(cx).focus_handle(cx),
            YamlMode::Structure => self.focus.clone(),
        }
    }

    pub(crate) fn scroll_offset(&self, cx: &App) -> gpui_kit::Point<gpui_kit::Pixels> {
        match self.mode {
            YamlMode::Source => self.source.read(cx).scroll_offset(),
            YamlMode::Structure => self.structure_scroll_offset(),
        }
    }

    pub(crate) fn set_scroll_offset(
        &mut self,
        offset: gpui_kit::Point<gpui_kit::Pixels>,
        cx: &mut Context<Self>,
    ) {
        match self.mode {
            YamlMode::Source => self
                .source
                .update(cx, |source, cx| source.set_scroll_offset(offset, cx)),
            YamlMode::Structure => {
                self.structure_scroll
                    .0
                    .borrow()
                    .base_handle
                    .set_offset(offset);
                cx.notify();
            }
        }
    }

    pub(crate) fn source_scroll_offset(&self, cx: &App) -> gpui_kit::Point<gpui_kit::Pixels> {
        self.source.read(cx).scroll_offset()
    }

    pub(crate) fn structure_scroll_offset(&self) -> gpui_kit::Point<gpui_kit::Pixels> {
        self.structure_scroll.0.borrow().base_handle.offset()
    }

    pub(crate) fn selected_location(&self, cx: &App) -> Option<SourceLocation> {
        let range = self.selected_detail_range(cx)?;
        let snapshot = self.document.read(cx).document().read(cx).snapshot();
        Some(SourceLocation::of(snapshot.text(), range))
    }

    #[cfg(any(test, feature = "native-markdown-tests"))]
    pub(crate) fn selected_path(&self, cx: &App) -> Option<SharedString> {
        let projection = self.document.read(cx).projection()?;
        projection
            .nodes()
            .get(self.selected?)
            .map(|node| SharedString::from(node.path.clone()))
    }

    #[cfg(any(test, feature = "native-markdown-tests"))]
    pub(crate) fn edit_texts(&self, cx: &App) -> Option<(Option<String>, String)> {
        let session = self.editing.as_ref()?;
        Some((
            session.panel.read(cx).key_text(cx),
            session.panel.read(cx).value_text(cx),
        ))
    }

    #[cfg(any(test, feature = "native-markdown-tests"))]
    pub(crate) fn mutation_notice(&self) -> Option<SharedString> {
        self.mutation_notice.clone()
    }

    pub(crate) fn is_parsing(&self, cx: &App) -> bool {
        self.document.read(cx).is_parsing()
    }

    pub(crate) fn node_count(&self, cx: &App) -> Option<usize> {
        self.document
            .read(cx)
            .projection()
            .map(YamlProjection::node_count)
    }

    pub(crate) fn document_count(&self, cx: &App) -> Option<usize> {
        self.document
            .read(cx)
            .projection()
            .map(YamlProjection::document_count)
    }

    /// 预算失败或整份 YAML 无法解析时的可见说明。
    pub(crate) fn fatal_error(&self, cx: &App) -> Option<(SharedString, SharedString)> {
        let document = self.document.read(cx);
        if let Some(failure) = document.failure() {
            let message = match failure {
                ProjectionFailure::Cancelled => return None,
                ProjectionFailure::InputTooLarge => t_fmt(
                    L10nKey::EditorYamlInputLimit,
                    &[("mib", &(STRUCTURE_INPUT_BYTES / (1024 * 1024)).to_string())],
                )
                .into(),
                ProjectionFailure::TooManyNodes => t_fmt(
                    L10nKey::EditorYamlNodeLimit,
                    &[("count", &(STRUCTURE_NODES).to_string())],
                )
                .into(),
                ProjectionFailure::PathsTooLarge => t_fmt(
                    L10nKey::EditorYamlPathLimit,
                    &[(
                        "mib",
                        &(projection::STRUCTURE_PATH_BYTES / (1024 * 1024)).to_string(),
                    )],
                )
                .into(),
                ProjectionFailure::TooDeep => t_fmt(
                    L10nKey::EditorYamlDepthLimit,
                    &[("count", &(STRUCTURE_DEPTH).to_string())],
                )
                .into(),
            };
            return Some((t(L10nKey::EditorYamlStructureUnavailable).into(), message));
        }
        let projection = document.projection()?;
        let error = projection.error()?;
        Some((
            t(L10nKey::EditorYamlInvalid).into(),
            t_fmt(
                L10nKey::EditorYamlInvalidDetail,
                &[
                    ("line", &error.line.to_string()),
                    ("column", &error.column.to_string()),
                    ("message", &error.message),
                ],
            )
            .into(),
        ))
    }

    pub(crate) fn structure_ready(&self, cx: &App) -> bool {
        self.document
            .read(cx)
            .projection()
            .is_some_and(|projection| projection.root().is_some())
    }

    pub(crate) fn select_source_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.show_source(cx);
        self.source
            .update(cx, |source, cx| source.select_range(range, cx));
    }

    pub(crate) fn add_source_selection(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.show_source(cx);
        self.source
            .update(cx, |source, cx| source.add_selection(range, cx));
    }

    pub(crate) fn replace_source_range(
        &mut self,
        range: Range<usize>,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        self.show_source(cx);
        self.source
            .update(cx, |source, cx| source.replace_range(range, text, cx));
    }

    pub(crate) fn replace_source_ranges(
        &mut self,
        replacements: Vec<(Range<usize>, String)>,
        cx: &mut Context<Self>,
    ) {
        self.show_source(cx);
        self.source
            .update(cx, |source, cx| source.replace_ranges(replacements, cx));
    }

    pub(crate) fn replace_source_lines(
        &mut self,
        range: Range<usize>,
        lines: Vec<Range<usize>>,
        leading: bool,
        trailing: bool,
        cx: &mut Context<Self>,
    ) {
        self.show_source(cx);
        self.source.update(cx, |source, cx| {
            source.replace_lines(range, lines, leading, trailing, cx)
        });
    }

    pub(crate) fn set_source_history_selection(
        &mut self,
        before: Range<usize>,
        after: Range<usize>,
        cx: &mut Context<Self>,
    ) {
        self.source
            .update(cx, |source, cx| source.set_history_selection(before, after, cx));
    }

    pub(crate) fn seed_presentation(
        &mut self,
        selection: Range<usize>,
        source_scroll: gpui_kit::Point<gpui_kit::Pixels>,
        structure_scroll: gpui_kit::Point<gpui_kit::Pixels>,
        expanded: Vec<bool>,
        selected: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        self.source.update(cx, |source, cx| {
            source.select_range(selection, cx);
            source.set_scroll_offset(source_scroll, cx);
        });
        if expanded.len() == self.expanded.len() {
            self.expanded = expanded;
        }
        self.selected = selected;
        self.structure_scroll
            .0
            .borrow()
            .base_handle
            .set_offset(structure_scroll);
        self.rebuild_rows(cx);
        cx.notify();
    }

    pub(crate) fn expansion_state(&self) -> (Vec<bool>, Option<usize>) {
        (self.expanded.clone(), self.selected)
    }

    pub(crate) fn finish_composition(&mut self, cx: &mut Context<Self>) {
        self.source
            .update(cx, |source, cx| source.finish_owned_composition(cx));
    }

    pub(crate) fn set_soft_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        self.source
            .update(cx, |source, cx| source.set_soft_wrap(wrap, cx));
    }

    pub(crate) fn set_format(&mut self, format: lyrel_text::TextFormat, cx: &mut Context<Self>) {
        self.source
            .update(cx, |source, cx| source.set_format(format, cx));
    }

    pub(crate) fn show_source(&mut self, cx: &mut Context<Self>) {
        self.close_edit(None, cx);
        if self.mode != YamlMode::Source {
            self.mode = YamlMode::Source;
            cx.notify();
        }
    }

    pub(crate) fn toggle_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_composition(cx);
        self.close_edit(None, cx);
        self.mode = self.mode.opposite();
        let focus = self.focus_handle(cx);
        focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn reveal_selected_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_edit(None, cx);
        let Some(range) = self.selected_detail_range(cx) else {
            return;
        };
        self.finish_composition(cx);
        self.mode = YamlMode::Source;
        self.source
            .update(cx, |source, cx| source.select_range(range, cx));
        let focus = self.source.read(cx).focus_handle(cx);
        focus.focus(window, cx);
        cx.notify();
    }

    fn current_projection(&self, cx: &App) -> Option<Arc<YamlProjection>> {
        let document = self.document.read(cx);
        let projection = document.projection.clone()?;
        let installed = document.installed_revision()?;
        let source = document.document().clone();
        (source.read(cx).revision() == installed).then_some(projection)
    }

    fn current_text(&self, cx: &App) -> Option<Arc<String>> {
        let source = self.document.read(cx).document().clone();
        Some(source.read(cx).snapshot().shared_text())
    }

    fn selected_node<'a>(&self, projection: &'a YamlProjection) -> Option<&'a projection::YamlNode> {
        projection.nodes().get(self.selected?)
    }

    fn selected_target(&self, cx: &App) -> Option<NodeTarget> {
        let projection = self.current_projection(cx)?;
        Some(NodeTarget::of(self.selected_node(&projection)?))
    }

    fn edit_caps(&self, cx: &App) -> EditCaps {
        let mut caps = EditCaps::default();
        let Some(projection) = self.current_projection(cx) else {
            return caps;
        };
        let Some(node) = self.selected_node(&projection) else {
            return caps;
        };
        caps.value = node.value_editable();
        caps.key = node.rename_key_editable();
        caps.add = node.can_add();
        caps.add_is_sequence = node.container == Some(ContainerKind::Sequence);
        caps.remove = node.can_remove();
        caps
    }

    fn start_value_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(projection) = self.current_projection(cx) else {
            return;
        };
        let Some(node) = self.selected_node(&projection) else {
            return;
        };
        if !node.value_editable() {
            self.mutation_error = Some(self.edit_unsupported(node));
            cx.notify();
            return;
        }
        let Some(range) = node.value_range.clone() else {
            return;
        };
        let Some(text) = self.current_text(cx) else {
            return;
        };
        let Some(raw) = text.get(range.clone()) else {
            return;
        };
        let target = NodeTarget::of(node);
        let title = t_fmt(L10nKey::EditorYamlEditValueTitle, &[("path", &node.path)]).into();
        let notice = self
            .effective_anchor(node, false, &projection)
            .map(|(anchor, count)| self.anchor_notice(&Some(anchor), count, false))
            .flatten();
        self.open_edit(
            window,
            cx,
            EditKind::Value,
            target,
            title,
            notice,
            None,
            EditField::new(
                t(L10nKey::EditorYamlValueName),
                t(L10nKey::EditorYamlValuePlaceholder),
                raw,
            ),
        );
    }

    fn start_key_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(projection) = self.current_projection(cx) else {
            return;
        };
        let Some(node) = self.selected_node(&projection) else {
            return;
        };
        if !node.rename_key_editable() {
            self.mutation_error = Some(self.edit_unsupported(node));
            cx.notify();
            return;
        }
        let Some(raw) = node.key_text.clone() else {
            return;
        };
        let target = NodeTarget::of(node);
        let title = t_fmt(L10nKey::EditorYamlRenameKeyTitle, &[("path", &node.path)]).into();
        let notice = self
            .effective_anchor(node, true, &projection)
            .map(|(anchor, count)| self.anchor_notice(&Some(anchor), count, true))
            .flatten();
        self.open_edit(
            window,
            cx,
            EditKind::Key,
            target,
            title,
            notice,
            None,
            EditField::new(
                t(L10nKey::EditorYamlKeyName),
                t(L10nKey::EditorYamlKeyPlaceholder),
                raw,
            ),
        );
    }

    fn start_add_member(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(projection) = self.current_projection(cx) else {
            return;
        };
        let Some(node) = self.selected_node(&projection) else {
            return;
        };
        let notice = self
            .effective_anchor(node, false, &projection)
            .map(|(anchor, count)| self.anchor_notice(&Some(anchor), count, false))
            .flatten();
        match node.container {
            Some(ContainerKind::Mapping) => {
                let target = NodeTarget::of(node);
                let title =
                    t_fmt(L10nKey::EditorYamlAddKeyTitle, &[("path", &node.path)]).into();
                self.open_edit(
                    window,
                    cx,
                    EditKind::AddMember,
                    target,
                    title,
                    notice,
                    Some(EditField::new(
                        t(L10nKey::EditorYamlKeyName),
                        t(L10nKey::EditorYamlKeyPlaceholder),
                        "",
                    )),
                    EditField::new(
                        t(L10nKey::EditorYamlValueName),
                        t(L10nKey::EditorYamlValuePlaceholder),
                        "",
                    ),
                );
            }
            Some(ContainerKind::Sequence) => {
                let target = NodeTarget::of(node);
                let title =
                    t_fmt(L10nKey::EditorYamlAddItemTitle, &[("path", &node.path)]).into();
                self.open_edit(
                    window,
                    cx,
                    EditKind::AddItem,
                    target,
                    title,
                    notice,
                    None,
                    EditField::new(
                        t(L10nKey::EditorYamlValueName),
                        t(L10nKey::EditorYamlValuePlaceholder),
                        "",
                    ),
                );
            }
            None => {
                self.mutation_error = Some(t(L10nKey::EditorYamlEditUnsupported).into());
                cx.notify();
            }
        }
    }

    fn remove_selected(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.selected_target(cx) else {
            return;
        };
        let impact = self.impact_for(&target, true, cx);
        match self.apply_mutation(Mutation::Remove { target }, cx) {
            Ok(()) => {
                self.mutation_error = None;
                self.mutation_notice = impact.and_then(|impact| impact.edited);
                cx.notify();
            }
            Err(error) => {
                self.mutation_error = Some(self.mutation_message(error));
                cx.notify();
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn open_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        kind: EditKind,
        target: NodeTarget,
        title: SharedString,
        notice: Option<SharedString>,
        key: Option<EditField>,
        value: EditField,
    ) {
        self.close_edit(None, cx);
        self.mutation_error = None;
        self.mutation_notice = None;
        let owner = cx.entity_id();
        let panel = cx.new(|cx| StructureEditPanel::new(owner, window, cx, title, key, value));
        if let Some(notice) = notice {
            panel.update(cx, |panel, cx| panel.set_notice(Some(notice), cx));
        }
        let events = cx.subscribe_in(
            &panel,
            window,
            |view, _, event: &StructureEditEvent, window, cx| match event {
                StructureEditEvent::Apply => view.apply_edit(window, cx),
                StructureEditEvent::Cancel => view.close_edit(Some(window), cx),
            },
        );
        panel.update(cx, |panel, cx| panel.focus_first(window, cx));
        self.editing = Some(EditSession {
            panel,
            kind,
            target,
            _events: events,
        });
        cx.notify();
    }

    fn close_edit(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            if let Some(window) = window {
                self.focus.focus(window, cx);
            }
            cx.notify();
        }
    }

    fn mark_edit_stale(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.editing else {
            return;
        };
        let message: SharedString = t(L10nKey::EditorYamlEditStale).into();
        session
            .panel
            .update(cx, |panel, cx| panel.set_error(Some(message), cx));
    }

    fn apply_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.editing.as_ref() else {
            return;
        };
        let panel = session.panel.clone();
        let kind = session.kind;
        let target = session.target.clone();
        let (impact, mutation) = match kind {
            EditKind::Value => (
                self.impact_for(&target, true, cx),
                Mutation::SetValue {
                    target,
                    value: panel.read(cx).value_text(cx),
                },
            ),
            EditKind::Key => (
                self.impact_for(&target, false, cx),
                Mutation::RenameKey {
                    target,
                    key: panel.read(cx).value_text(cx),
                },
            ),
            EditKind::AddMember => (
                self.impact_for(&target, true, cx),
                Mutation::AddMember {
                    container: target,
                    key: panel.read(cx).key_text(cx).unwrap_or_default(),
                    value: panel.read(cx).value_text(cx),
                },
            ),
            EditKind::AddItem => (
                self.impact_for(&target, true, cx),
                Mutation::AddItem {
                    container: target,
                    value: panel.read(cx).value_text(cx),
                },
            ),
        };
        match self.apply_mutation(mutation, cx) {
            Ok(()) => {
                self.mutation_error = None;
                self.mutation_notice = impact.and_then(|impact| impact.edited);
                self.close_edit(Some(window), cx);
            }
            Err(error) => {
                let message = self.mutation_message(error);
                panel.update(cx, |panel, cx| panel.set_error(Some(message), cx));
            }
        }
    }

    /// 一次命令一条 history：规划出的替换清单交给共享 Document 的正常
    /// `replace_ranges`。revision 不一致时拒绝写入。
    fn apply_mutation(
        &mut self,
        mutation: Mutation,
        cx: &mut Context<Self>,
    ) -> Result<(), MutationError> {
        let (projection, installed, editor, revision, text) = {
            let yaml_document = self.document.read(cx);
            let document = yaml_document.document().clone();
            let editor = self.source.clone();
            let projection = yaml_document.projection.clone();
            let installed = yaml_document.installed_revision();
            let revision = document.read(cx).revision();
            let text = document.read(cx).snapshot().shared_text();
            (projection, installed, editor, revision, text)
        };
        let projection = projection.ok_or(MutationError::Stale)?;
        if installed != Some(revision) {
            return Err(MutationError::Stale);
        }
        let replacements = mutation::plan(text.as_str(), &projection, &mutation)?;
        if !replacements.is_empty() {
            editor.update(cx, |editor, cx| editor.replace_ranges(replacements, cx));
        }
        Ok(())
    }

    fn edit_unsupported(&self, node: &projection::YamlNode) -> SharedString {
        let key = match (node.alias.is_some(), node.block_scalar, node.entry, node.key_editable) {
            (true, _, _, _) => L10nKey::EditorYamlEditAlias,
            (_, true, _, _) => L10nKey::EditorYamlEditBlockScalar,
            (_, _, true, false) => L10nKey::EditorYamlEditComplexKey,
            _ => L10nKey::EditorYamlEditUnsupported,
        };
        t(key).into()
    }

    fn mutation_message(&self, error: MutationError) -> SharedString {
        match error {
            MutationError::Stale => t(L10nKey::EditorYamlEditStale).into(),
            MutationError::Unsupported(UnsupportedEdit::Alias) => {
                t(L10nKey::EditorYamlEditAlias).into()
            }
            MutationError::Unsupported(UnsupportedEdit::BlockScalar) => {
                t(L10nKey::EditorYamlEditBlockScalar).into()
            }
            MutationError::Unsupported(UnsupportedEdit::ComplexKey) => {
                t(L10nKey::EditorYamlEditComplexKey).into()
            }
            MutationError::Unsupported(UnsupportedEdit::MissingValue) => {
                t(L10nKey::EditorYamlEditMissingValue).into()
            }
            MutationError::Unsupported(UnsupportedEdit::MultiLine) => {
                t(L10nKey::EditorYamlEditMultiLine).into()
            }
            MutationError::Unsupported(UnsupportedEdit::NotScalar) => {
                t(L10nKey::EditorYamlEditNotScalar).into()
            }
            MutationError::Unsupported(UnsupportedEdit::LastBlockItem) => {
                t(L10nKey::EditorYamlEditLastBlockItem).into()
            }
            MutationError::Unsupported(UnsupportedEdit::EmptyDocument) => {
                t(L10nKey::EditorYamlEditEmptyDocument).into()
            }
            MutationError::Unsupported(
                UnsupportedEdit::NotContainer,
            ) => t(L10nKey::EditorYamlEditUnsupported).into(),
            MutationError::InvalidFragment(message) => t_fmt(
                L10nKey::EditorYamlEditInvalidValue,
                &[("message", &message)],
            )
            .into(),
            MutationError::ResultInvalid {
                line,
                column,
                message,
            } => t_fmt(
                L10nKey::EditorYamlEditResultInvalid,
                &[
                    ("line", &line.to_string()),
                    ("column", &column.to_string()),
                    ("message", &message),
                ],
            )
            .into(),
            MutationError::AliasesReference { name, count } => t_fmt(
                L10nKey::EditorYamlEditAliasesReference,
                &[
                    ("name", &name.unwrap_or_else(|| "?".to_owned())),
                    ("count", &count.to_string()),
                ],
            )
            .into(),
        }
    }

    fn selected_detail_range(&self, cx: &App) -> Option<Range<usize>> {
        let projection = self.document.read(cx).projection()?;
        let node = projection.nodes().get(self.selected?)?;
        node.has_source_detail().then(|| node.detail_range())
    }

    fn sync_structure(&mut self, cx: &mut Context<Self>) {
        let (projection, revision) = {
            let document = self.document.read(cx);
            (document.projection.clone(), document.installed_revision())
        };
        let Some(projection) = projection else {
            self.rows.clear();
            self.selected = None;
            self.selected_path = None;
            self.seen_revision = None;
            self.mark_edit_stale(cx);
            return;
        };
        let node_count = projection.node_count();
        let first_install = self.expanded.len() != node_count;
        if first_install {
            self.expanded = vec![false; node_count];
            default_expansion(projection.as_ref(), &mut self.expanded);
        }
        if let Some(root) = projection.root()
            && root < self.expanded.len()
        {
            self.expanded[root] = true;
        }
        let revision_changed = revision != self.seen_revision;
        self.seen_revision = revision;
        if revision_changed {
            if let Some(path) = self.selected_path.as_deref() {
                self.selected = projection
                    .nodes()
                    .iter()
                    .position(|node| node.path == path)
                    .filter(|index| *index < node_count);
            } else {
                self.selected = self.selected.filter(|index| *index < node_count);
            }
        } else if self.selected.is_some_and(|index| index >= node_count) {
            self.selected = None;
        }
        if self.selected.is_none() {
            self.selected_path = None;
        }
        if revision_changed {
            self.mark_edit_stale(cx);
        }
        self.rows = structure_view::visible_rows(projection.as_ref(), &self.expanded);
    }

    fn rebuild_rows(&mut self, cx: &App) {
        self.rows = self
            .document
            .read(cx)
            .projection()
            .map(|projection| structure_view::visible_rows(projection, &self.expanded))
            .unwrap_or_default();
    }

    fn select_node(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(path) = self
            .document
            .read(cx)
            .projection()
            .and_then(|projection| projection.nodes().get(index))
            .map(|node| node.path.clone())
        else {
            return;
        };
        self.close_edit(None, cx);
        self.mutation_error = None;
        self.mutation_notice = None;
        self.selected = Some(index);
        self.selected_path = Some(path);
        if let Some(row) = structure_view::row_of(&self.rows, index) {
            self.structure_scroll
                .scroll_to_item(row, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    fn toggle_node(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.expanded.get(index).is_none() {
            return;
        }
        self.expanded[index] = !self.expanded[index];
        self.rebuild_rows(cx);
        cx.notify();
    }

    fn activate_node(&mut self, index: usize, cx: &mut Context<Self>) {
        self.select_node(index, cx);
        let expandable = self
            .document
            .read(cx)
            .projection()
            .is_some_and(|projection| projection.is_expandable(index));
        if expandable {
            self.toggle_node(index, cx);
        }
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || event.keystroke.modifiers.modified() {
            return;
        }
        let key = event.keystroke.key.as_str();
        let step = match key {
            "down" => 1,
            "up" => -1,
            "pagedown" => PAGE_ROWS,
            "pageup" => -PAGE_ROWS,
            _ => 0,
        };
        if step != 0 {
            let Some(node) = structure_view::move_selection(&self.rows, self.selected, step) else {
                return;
            };
            self.select_node(node, cx);
            cx.stop_propagation();
            return;
        }
        match key {
            "home" => {
                if let Some(node) = structure_view::first_node(&self.rows) {
                    self.select_node(node, cx);
                    cx.stop_propagation();
                }
            }
            "end" => {
                if let Some(node) = structure_view::last_node(&self.rows) {
                    self.select_node(node, cx);
                    cx.stop_propagation();
                }
            }
            "right" => {
                let Some(selected) = self.selected else {
                    if let Some(node) = structure_view::first_node(&self.rows) {
                        self.select_node(node, cx);
                    }
                    cx.stop_propagation();
                    return;
                };
                let (expandable, expanded, child) = {
                    let projection = self.document.read(cx).projection();
                    let expandable = projection.is_some_and(|p| p.is_expandable(selected));
                    let expanded = self.expanded.get(selected).copied().unwrap_or(false);
                    let child = structure_view::first_child_node(&self.rows, selected);
                    (expandable, expanded, child)
                };
                if expandable && !expanded {
                    self.toggle_node(selected, cx);
                } else if let Some(child) = child {
                    self.select_node(child, cx);
                }
                cx.stop_propagation();
            }
            "left" => {
                let Some(selected) = self.selected else {
                    return;
                };
                let expanded = self.expanded.get(selected).copied().unwrap_or(false);
                if expanded {
                    self.toggle_node(selected, cx);
                } else if let Some(parent) = structure_view::parent_node(&self.rows, selected) {
                    self.select_node(parent, cx);
                }
                cx.stop_propagation();
            }
            "space" => {
                if let Some(selected) = self.selected {
                    self.toggle_node(selected, cx);
                    cx.stop_propagation();
                }
            }
            "enter" => {
                self.reveal_selected_source(window, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity_id();
        let document = self.document.read(cx);
        let parsing = document.is_parsing();
        let fatal = self.fatal_error(cx);
        let documents = self.document_count(cx);
        let nodes = self.node_count(cx);
        let location = self.selected_location(cx).map(|location| {
            t_fmt(
                L10nKey::EditorLnCol,
                &[
                    ("line", &location.line.to_string()),
                    ("column", &location.column.to_string()),
                ],
            )
        });
        let has_detail = self.selected_location(cx).is_some();
        let caps = self.edit_caps(cx);
        let editable = self.current_projection(cx).is_some();
        let mutation_error = self.mutation_error.clone();
        let mutation_notice = self.mutation_notice.clone();
        let mode = self.mode;
        let (toggle, tip) = match mode {
            YamlMode::Structure => (
                t(L10nKey::EditorYamlSource),
                t(L10nKey::EditorYamlShowSource),
            ),
            YamlMode::Source => (
                t(L10nKey::EditorYamlStructure),
                t(L10nKey::EditorYamlShowStructure),
            ),
        };
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let warning = cx.theme().warning;
        h_flex()
            .id(("yaml-toolbar", entity))
            .flex_none()
            .w_full()
            .items_center()
            .min_w_0()
            .flex_wrap()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(muted)
                    .child("YAML"),
            )
            .when_some(documents, |row, documents| {
                row.child(
                    div()
                        .id(("yaml-documents", entity))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(t_fmt(
                            L10nKey::EditorYamlDocuments,
                            &[("count", &documents.to_string())],
                        )),
                )
            })
            .when_some(nodes, |row, nodes| {
                row.child(
                    div()
                        .id(("yaml-count", entity))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(t_fmt(
                            L10nKey::EditorYamlNodes,
                            &[("count", &nodes.to_string())],
                        )),
                )
            })
            .when(parsing, |row| {
                row.child(
                    h_flex()
                        .id(("yaml-parsing", entity))
                        .test_support()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .text_color(muted)
                        .child(Spinner::new().xsmall())
                        .child(t(L10nKey::EditorYamlParsing)),
                )
            })
            .when_some(fatal, |row, (title, _)| {
                row.child(
                    div()
                        .id(("yaml-structure-error", entity))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(warning)
                        .child(title),
                )
            })
            .when_some(location, |row, location| {
                row.child(
                    div()
                        .id(("yaml-location", entity))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(location),
                )
            })
            .when_some(mutation_error, |row, error| {
                row.child(
                    div()
                        .id(("yaml-mutation-error", entity))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(warning)
                        .child(error),
                )
            })
            .when_some(mutation_notice, |row, notice| {
                row.child(
                    div()
                        .id(("yaml-anchor-notice", entity))
                        .test_support()
                        .aria_value(notice.clone())
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().yellow)
                        .child(notice),
                )
            })
            .when(editable, |row| {
                row.child(
                    Button::new(("yaml-edit-value", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.value)
                        .label(t(L10nKey::EditorJsonEditValue))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_value_edit(window, cx)),
                        ),
                )
                .child(
                    Button::new(("yaml-rename-key", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.key)
                        .label(t(L10nKey::EditorJsonRenameKey))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_key_rename(window, cx)),
                        ),
                )
                .child(
                    Button::new(("yaml-add-member", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.add)
                        .label(match caps.add_is_sequence {
                            true => t(L10nKey::EditorJsonAddItem),
                            false => t(L10nKey::EditorJsonAddKey),
                        })
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_add_member(window, cx)),
                        ),
                )
                .child(
                    Button::new(("yaml-remove-member", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.remove)
                        .label(t(L10nKey::EditorJsonRemove))
                        .on_click(cx.listener(|this, _, _, cx| this.remove_selected(cx))),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new(("yaml-reveal", entity))
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .disabled(!has_detail)
                    .label(t(L10nKey::EditorJsonReveal))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.reveal_selected_source(window, cx)),
                    ),
            )
            .child(
                Button::new(("yaml-mode-toggle", entity))
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .label(toggle)
                    .tooltip(tip)
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_mode(window, cx))),
            )
    }

    fn structure_body(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some((title, detail)) = self.fatal_error(cx) {
            return v_flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(structure_error(
                    ("yaml-structure-error-body", cx.entity_id()),
                    title,
                    detail,
                    cx,
                ))
                .children(self.render_edit_panel())
                .into_any_element();
        }
        if !self.structure_ready(cx) {
            let body = if self.is_parsing(cx) {
                structure_loading(
                    ("yaml-structure-loading", cx.entity_id()),
                    t(L10nKey::EditorYamlParsing).into(),
                    cx,
                )
            } else {
                structure_error(
                    ("yaml-structure-error-body", cx.entity_id()),
                    t(L10nKey::EditorYamlStructureUnavailable).into(),
                    t(L10nKey::EditorYamlSourceStaysEditable).into(),
                    cx,
                )
            };
            return v_flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(body)
                .children(self.render_edit_panel())
                .into_any_element();
        }
        let entity = cx.entity_id();
        let list = uniform_list(
            ("yaml-structure", entity),
            self.rows.len(),
            cx.processor(move |view, range: Range<usize>, _, cx| {
                range
                    .map(|row_index| {
                        let Some(projection) = view.document.read(cx).projection() else {
                            return div().into_any_element();
                        };
                        let row = view.rows[row_index];
                        let Some(node) = projection.nodes().get(row.node) else {
                            return div().into_any_element();
                        };
                        let expandable = projection.is_expandable(row.node);
                        let expanded = view.expanded.get(row.node).copied().unwrap_or(false);
                        let selected = view.selected == Some(row.node);
                        let color = match node.kind {
                            YamlNodeKind::Scalar(YamlScalarKind::String) => cx.theme().green,
                            YamlNodeKind::Scalar(
                                YamlScalarKind::Integer | YamlScalarKind::Float,
                            ) => cx.theme().blue,
                            YamlNodeKind::Scalar(YamlScalarKind::Boolean) => cx.theme().magenta,
                            YamlNodeKind::Scalar(
                                YamlScalarKind::Null
                                | YamlScalarKind::Timestamp
                                | YamlScalarKind::Other,
                            ) => cx.theme().muted_foreground,
                            YamlNodeKind::Alias => cx.theme().yellow,
                            YamlNodeKind::Tagged => cx.theme().magenta,
                            YamlNodeKind::File
                            | YamlNodeKind::Mapping
                            | YamlNodeKind::Sequence
                            | YamlNodeKind::Empty => cx.theme().muted_foreground,
                        };
                        let badge = badge_for(node, projection);
                        let row_id =
                            SharedString::from(format!("yaml-row-{}-{}", entity.as_u64(), row.node));
                        let index = row.node;
                        structure_row(
                            StructureRowData {
                                id: row_id,
                                depth: row.depth,
                                expandable,
                                expanded,
                                selected,
                                striped: row_index % 2 == 1,
                                key: node.key.clone().into(),
                                value: node.value.clone().into(),
                                color,
                                badge,
                                record_line: None,
                                error: node.alias_error.clone().map(SharedString::from),
                                aria_label: node.path.clone().into(),
                            },
                            cx.listener(
                                |view, _: &gpui_kit::MouseDownEvent, window, cx| {
                                    view.focus.focus(window, cx);
                                    cx.stop_propagation();
                                },
                            ),
                            cx.listener(
                                move |view, event: &gpui_kit::ClickEvent, window, cx| {
                                    view.focus.focus(window, cx);
                                    if event.click_count() > 1 {
                                        view.select_node(index, cx);
                                        view.reveal_selected_source(window, cx);
                                    } else {
                                        view.activate_node(index, cx);
                                    }
                                },
                            ),
                            cx,
                        )
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.structure_scroll)
        .flex_1()
        .min_h_0()
        .w_full();
        v_flex()
            .id(("yaml-structure-body", entity))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(list)
            .children(self.render_edit_panel())
            .into_any_element()
    }

    fn render_edit_panel(&self) -> Option<AnyElement> {
        self.editing
            .as_ref()
            .map(|session| session.panel.clone().into_any_element())
    }

    fn source_body(&self) -> AnyElement {
        source_body_for(&self.source)
    }

    /// 选中节点自身或最近的带锚点祖先。编辑锚定 collection 的后代同样会
    /// 改变别名解析结果，因此影响说明不能只看直接值。
    fn effective_anchor(
        &self,
        node: &projection::YamlNode,
        key: bool,
        projection: &YamlProjection,
    ) -> Option<(String, usize)> {
        if key {
            if let Some(anchor) = &node.key_anchor {
                return Some((anchor.clone(), node.key_alias_refs));
            }
        } else if let Some(anchor) = &node.anchor {
            return Some((anchor.clone(), node.alias_refs));
        }
        let mut parent = node.parent;
        while let Some(index) = parent {
            let ancestor = projection.nodes().get(index)?;
            if let Some(anchor) = &ancestor.anchor {
                return Some((anchor.clone(), ancestor.alias_refs));
            }
            parent = ancestor.parent;
        }
        None
    }

    fn anchor_notice(
        &self,
        anchor: &Option<String>,
        aliases: usize,
        key: bool,
    ) -> Option<SharedString> {
        let name = anchor.as_ref()?;
        let anchor = format!("&{name}");
        let where_ = if key {
            t(L10nKey::EditorYamlKeyName)
        } else {
            t(L10nKey::EditorYamlValueName)
        };
        Some(
            t_fmt(
                match aliases {
                    0 => L10nKey::EditorYamlAnchorImpactNone,
                    _ => L10nKey::EditorYamlAnchorImpact,
                },
                &[
                    ("anchor", &anchor),
                    ("count", &aliases.to_string()),
                    ("field", where_),
                ],
            )
            .into(),
        )
    }

    fn impact_for(
        &self,
        target: &NodeTarget,
        value: bool,
        cx: &App,
    ) -> Option<AnchorImpact> {
        let projection = self.current_projection(cx)?;
        let node = projection
            .nodes()
            .iter()
            .find(|node| node.path == target.path && node.range == target.range)?;
        let (name, count) = self.effective_anchor(node, !value, &projection)?;
        let anchor = format!("&{name}");
        let edited: SharedString = t_fmt(
            match count {
                0 => L10nKey::EditorYamlAnchorEditedNone,
                _ => L10nKey::EditorYamlAnchorEdited,
            },
            &[("anchor", &anchor), ("count", &count.to_string())],
        )
        .into();
        Some(AnchorImpact {
            edited: Some(edited),
        })
    }
}

struct AnchorImpact {
    edited: Option<SharedString>,
}

fn badge_for(node: &projection::YamlNode, projection: &YamlProjection) -> Option<SharedString> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(anchor) = &node.key_anchor {
        parts.push(format!("&{anchor} (key)"));
    }
    if let Some(tag) = &node.key_tag {
        parts.push(tag.clone());
    }
    if let Some(anchor) = &node.anchor {
        parts.push(format!("&{anchor}"));
    }
    if let Some(tag) = &node.tag {
        parts.push(tag.clone());
    }
    if let Some(alias) = &node.alias {
        match (&node.alias_error, node.alias_target) {
            (Some(_), _) => parts.push(format!("*{alias} → missing")),
            (_, Some(target)) => {
                let anchor = projection
                    .nodes()
                    .get(target)
                    .and_then(|target| target.anchor.as_deref())
                    .unwrap_or("?");
                parts.push(format!("*{alias} → &{anchor}"));
            }
            _ => parts.push(format!("*{alias}")),
        }
    }
    match parts.is_empty() {
        true => None,
        false => Some(parts.join(" · ").into()),
    }
}

fn default_expansion(projection: &YamlProjection, expanded: &mut [bool]) {
    let Some(root) = projection.root() else {
        return;
    };
    if root >= expanded.len() {
        return;
    }
    expanded[root] = true;
    for &child in projection.children(root) {
        if projection.is_expandable(child)
            && projection.children(child).len() <= AUTO_EXPAND_CHILDREN
        {
            expanded[child] = true;
            for &grandchild in projection.children(child) {
                if projection.is_expandable(grandchild)
                    && projection.children(grandchild).len() <= AUTO_EXPAND_CHILDREN
                {
                    expanded[grandchild] = true;
                }
            }
        }
    }
}

impl Focusable for YamlView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.mode {
            YamlMode::Structure => self.focus.clone(),
            YamlMode::Source => self.source.read(cx).focus_handle(cx),
        }
    }
}

impl Render for YamlView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.mode;
        let focus = self.focus.clone();
        let item = v_flex()
            .id(("yaml-document", cx.entity_id()))
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(self.toolbar(cx));
        let body = match mode {
            YamlMode::Structure => self.structure_body(cx),
            YamlMode::Source => self.source_body(),
        };
        let item = item.child(body);
        match mode {
            YamlMode::Structure => item
                .key_context(KEY_CONTEXT)
                .track_focus(&focus)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.focus.focus(window, cx);
                        cx.stop_propagation();
                    }),
                )
                .on_key_down(
                    cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        this.key(event, window, cx)
                    }),
                )
                .into_any_element(),
            YamlMode::Source => item.into_any_element(),
        }
    }
}

/// 右侧菜单的 YAML 项：默认只给结构与源码切换。
pub(super) fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
) -> gpui_kit::component::menu::PopupMenu {
    menu.separator().menu(
        t(L10nKey::EditorYamlToggleStructure),
        Box::new(crate::core::actions::ToggleDocumentPreview),
    )
}

/// 解析的确定性测试门闩：只编译进测试与 native 测试引擎。
#[cfg(any(test, feature = "native-markdown-tests"))]
pub(crate) mod test_support {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::{cell::RefCell, sync::mpsc, time::Duration};

    pub(super) struct ParseGate {
        receiver: mpsc::Receiver<()>,
        entered: Arc<AtomicBool>,
    }

    thread_local! {
        static GATE: RefCell<Option<ParseGate>> = const { RefCell::new(None) };
    }

    /// Pauses the next worker parse before it reads the projection.
    pub(crate) fn pause_next_parse() -> (mpsc::Sender<()>, Arc<AtomicBool>) {
        let (tx, rx) = mpsc::channel();
        let entered = Arc::new(AtomicBool::new(false));
        GATE.with_borrow_mut(|gate| {
            *gate = Some(ParseGate {
                receiver: rx,
                entered: entered.clone(),
            })
        });
        (tx, entered)
    }

    pub(super) fn take_next_parse() -> Option<ParseGate> {
        GATE.with_borrow_mut(Option::take)
    }

    impl ParseGate {
        pub(super) fn block(self) {
            self.entered.store(true, Ordering::SeqCst);
            let _ = self.receiver.recv_timeout(Duration::from_secs(20));
        }
    }
}

#[cfg(test)]
#[path = "yaml_document/tests.rs"]
mod tests;
