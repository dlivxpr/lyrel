//! TOML 文档：一份共享逐行保真源码，加上按 revision 派生的无损结构视图。
//!
//! 源码始终是同一 [`lyrel_markdown_editor::Document`] 的 plain-source
//! presentation；结构节点只保存源码范围与显示文本，不复制第二份可变内容，
//! 也不通过 `toml_edit::DocumentMut` 整篇回写。ordinary table、inline
//! table、array of tables、dotted/quoted key 与 date/time 等标量都保留原
//! 拼写和形态。
//!
//! 结构解析在 UI 线程外进行：输入有界、每份文档单飞、只安装与当前
//! revision／generation 匹配的结果；解析失败或超预算时源码仍然可编辑。
//! 结构视图的展开、选择、源码定位和完整值属于每个 presentation；文件自己
//! 只持有 Document 和最新的投影。

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
    AnyElement, App, ClipboardItem, Context, Entity, FocusHandle, Focusable, KeyDownEvent,
    MouseButton, Render, SharedString, Subscription, Task, TestSupportExt as _,
    UniformListScrollHandle, Window, div, rems, uniform_list,
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

use mutation::{Mutation, MutationError, TomlTarget};
use projection::{
    ProjectionFailure, STRUCTURE_INPUT_BYTES, STRUCTURE_NODES, STRUCTURE_PATH_BYTES, TomlNodeKind,
    TomlProjection, TomlValueKind,
};

/// 合并连续输入；每份文档最多一个 worker 与一次在途解析。
const PARSE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(120);
/// 打开时自动展开根附近的浅层容器，复现上游的浅层可读树。
const AUTO_EXPAND_CHILDREN: usize = 8;
const AUTO_EXPAND_DEPTH: usize = 3;
const AUTO_EXPAND_NODES: usize = 256;
/// 键盘 PageUp／PageDown 跨过的可见行数。
const PAGE_ROWS: isize = 10;

/// 结构预览持有键盘时成立的上下文；源码面不声明它，文字编辑键不会被截走。
pub(crate) const KEY_CONTEXT: &str = "TomlStructure";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TomlMode {
    Structure,
    Source,
}

impl TomlMode {
    pub(crate) fn opposite(self) -> Self {
        match self {
            Self::Structure => Self::Source,
            Self::Source => Self::Structure,
        }
    }
}

/// 一份 TOML 文件的内容：共享 Document 与按最新 revision 安装的投影。
pub(crate) struct TomlDocument {
    document: Entity<Document>,
    projection: Option<Arc<TomlProjection>>,
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

impl Drop for TomlDocument {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl TomlDocument {
    pub(super) fn new(document: Entity<Document>, cx: &mut Context<Self>) -> Self {
        let watch = cx.subscribe(&document, |this, _, _: &DocumentEvent, cx| {
            this.invalidate(cx)
        });
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

    pub(crate) fn projection(&self) -> Option<&TomlProjection> {
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
                #[cfg(test)]
                let gate = test_support::take_next_parse();
                let result = crate::ui::host_ops::off_thread(move || {
                    if cancel.load(Ordering::Acquire) {
                        return None;
                    }
                    #[cfg(test)]
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

/// 一个 presentation 里的完整值面板：路径与源码范围；文本在绘制时从当前
/// 快照读取，源码任一次变化都会关闭它。
struct FullValue {
    path: SharedString,
    range: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditKind {
    Value,
    Rename,
    AddKey,
    AddTable,
    AddArray,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct EditCaps {
    value: bool,
    rename: bool,
    add_key: bool,
    add_table: bool,
    add_array: bool,
    add_entry: bool,
    remove: bool,
}

/// 一次明确的树命令：目标节点在打开面板时固定，提交时按当前 revision
/// 重新解析；面板只是输入控件，不持有文档。
struct EditSession {
    panel: Entity<StructureEditPanel>,
    kind: EditKind,
    target: TomlTarget,
    /// The source revision the panel was opened against. A same-length edit
    /// elsewhere does not change the target ranges, so identity alone is not
    /// enough to reject a stale command.
    revision: u64,
    _events: Subscription,
}

/// 一份 TOML 文件在一个分组里的呈现：模式、源码选择、结构展开与滚动。
pub(crate) struct TomlView {
    document: Entity<TomlDocument>,
    source: Entity<SourceEditor>,
    mode: TomlMode,
    focus: FocusHandle,
    structure_scroll: UniformListScrollHandle,
    expanded: Vec<bool>,
    rows: Vec<VisibleRow>,
    selected: Option<usize>,
    selected_path: Option<String>,
    selected_range: Option<Range<usize>>,
    full_value: Option<FullValue>,
    editing: Option<EditSession>,
    mutation_error: Option<SharedString>,
    seen_revision: Option<u64>,
    _document: Subscription,
    _source_document: Subscription,
    _source: Subscription,
}

impl TomlView {
    pub(super) fn new(
        document: Entity<TomlDocument>,
        mode: TomlMode,
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
            cx.subscribe(&source_document, |this, _, _: &DocumentEvent, cx| {
                // 旧范围在另一 revision 上没有意义；源码变化时先收起详情。
                this.full_value = None;
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
            selected_range: None,
            full_value: None,
            editing: None,
            mutation_error: None,
            seen_revision: None,
            _document: watch_document,
            _source_document: watch_source_document,
            _source: watch_source,
        };
        view.sync_structure(cx);
        view
    }

    pub(crate) fn document(&self) -> &Entity<TomlDocument> {
        &self.document
    }

    pub(crate) fn source(&self) -> &Entity<SourceEditor> {
        &self.source
    }

    pub(crate) fn mode(&self) -> TomlMode {
        self.mode
    }

    pub(crate) fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.mode {
            TomlMode::Source => self.source.read(cx).focus_handle(cx),
            TomlMode::Structure => self.focus.clone(),
        }
    }

    pub(crate) fn scroll_offset(&self, cx: &App) -> gpui_kit::Point<gpui_kit::Pixels> {
        match self.mode {
            TomlMode::Source => self.source.read(cx).scroll_offset(),
            TomlMode::Structure => self.structure_scroll_offset(),
        }
    }

    pub(crate) fn set_scroll_offset(
        &mut self,
        offset: gpui_kit::Point<gpui_kit::Pixels>,
        cx: &mut Context<Self>,
    ) {
        match self.mode {
            TomlMode::Source => self
                .source
                .update(cx, |source, cx| source.set_scroll_offset(offset, cx)),
            TomlMode::Structure => {
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

    #[cfg(test)]
    pub(crate) fn selected_path(&self, cx: &App) -> Option<SharedString> {
        let projection = self.document.read(cx).projection()?;
        projection
            .nodes()
            .get(self.selected?)
            .map(|node| SharedString::from(node.path.clone()))
    }

    #[cfg(test)]
    pub(crate) fn edit_texts(&self, cx: &App) -> Option<(Option<String>, String)> {
        let session = self.editing.as_ref()?;
        Some((
            session.panel.read(cx).key_text(cx),
            session.panel.read(cx).value_text(cx),
        ))
    }

    pub(crate) fn full_value_text(&self, cx: &App) -> Option<SharedString> {
        let detail = self.full_value.as_ref()?;
        let snapshot = self.document.read(cx).document().read(cx).snapshot();
        snapshot
            .text()
            .get(detail.range.clone())
            .map(|text| SharedString::from(text.to_owned()))
    }

    pub(crate) fn full_value_path(&self) -> Option<SharedString> {
        self.full_value.as_ref().map(|detail| detail.path.clone())
    }

    pub(crate) fn is_parsing(&self, cx: &App) -> bool {
        self.document.read(cx).is_parsing()
    }

    pub(crate) fn node_count(&self, cx: &App) -> Option<usize> {
        self.document
            .read(cx)
            .projection()
            .map(TomlProjection::node_count)
    }

    pub(crate) fn fatal_error(&self, cx: &App) -> Option<(SharedString, SharedString)> {
        let document = self.document.read(cx);
        if let Some(failure) = document.failure() {
            let message = match failure {
                ProjectionFailure::Cancelled => return None,
                ProjectionFailure::InputTooLarge => t_fmt(
                    L10nKey::EditorTomlInputLimit,
                    &[("mib", &(STRUCTURE_INPUT_BYTES / (1024 * 1024)).to_string())],
                )
                .into(),
                ProjectionFailure::TooManyNodes => t_fmt(
                    L10nKey::EditorTomlNodeLimit,
                    &[("count", &STRUCTURE_NODES.to_string())],
                )
                .into(),
                ProjectionFailure::PathsTooLarge => t_fmt(
                    L10nKey::EditorTomlPathLimit,
                    &[("mib", &(STRUCTURE_PATH_BYTES / (1024 * 1024)).to_string())],
                )
                .into(),
            };
            return Some((t(L10nKey::EditorTomlStructureUnavailable).into(), message));
        }
        let projection = document.projection()?;
        let error = projection.error()?;
        Some((
            t(L10nKey::EditorTomlInvalid).into(),
            t_fmt(
                L10nKey::EditorTomlInvalidDetail,
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
        self.source.update(cx, |source, cx| {
            source.set_history_selection(before, after, cx)
        });
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
        if self.mode != TomlMode::Source {
            self.mode = TomlMode::Source;
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
        self.mode = TomlMode::Source;
        self.source
            .update(cx, |source, cx| source.select_range(range, cx));
        let focus = self.source.read(cx).focus_handle(cx);
        focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn open_selected_full_value(&mut self, cx: &mut Context<Self>) {
        self.close_edit(None, cx);
        let Some(index) = self.selected else {
            return;
        };
        let detail = self
            .document
            .read(cx)
            .projection()
            .and_then(|projection| projection.nodes().get(index))
            .filter(|node| node.has_source_detail())
            .map(|node| (SharedString::from(node.path.clone()), node.detail.clone()));
        self.full_value = detail.map(|(path, range)| FullValue { path, range });
        cx.notify();
    }

    pub(crate) fn close_full_value(&mut self, cx: &mut Context<Self>) {
        if self.full_value.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn copy_full_value(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.full_value_text(cx) {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        }
    }

    fn current_projection(&self, cx: &App) -> Option<Arc<TomlProjection>> {
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

    fn structure_editable(&self, cx: &App) -> bool {
        self.current_projection(cx).is_some()
    }

    fn selected_target(&self, cx: &App) -> Option<TomlTarget> {
        let projection = self.current_projection(cx)?;
        let node = projection.nodes().get(self.selected?)?;
        Some(TomlTarget::of(node))
    }

    fn selected_path_text(&self, cx: &App) -> Option<String> {
        let projection = self.document.read(cx).projection.clone()?;
        projection
            .nodes()
            .get(self.selected?)
            .map(|node| node.path.clone())
    }

    fn edit_caps(&self, cx: &App) -> EditCaps {
        let mut caps = EditCaps::default();
        let Some(projection) = self.current_projection(cx) else {
            return caps;
        };
        let Some(node) = self
            .selected
            .and_then(|index| projection.nodes().get(index))
        else {
            return caps;
        };
        caps.value = node.kind.is_value() && !node.range.is_empty();
        caps.rename = match node.kind {
            TomlNodeKind::Value(_) => node.key_range.is_some(),
            TomlNodeKind::Table => node.explicit_table && !node.table_is_dotted,
            TomlNodeKind::ArrayOfTables => true,
            _ => false,
        };
        caps.add_key = match node.kind {
            TomlNodeKind::Root | TomlNodeKind::ArrayOfTablesEntry => true,
            TomlNodeKind::Table => node.explicit_table && !node.inline_context,
            TomlNodeKind::Value(TomlValueKind::InlineTable) => true,
            _ => false,
        };
        let table_container = matches!(
            node.kind,
            TomlNodeKind::Root | TomlNodeKind::Table | TomlNodeKind::ArrayOfTablesEntry
        ) && !node.inline_context
            && (node.kind != TomlNodeKind::Table || node.explicit_table);
        caps.add_table = table_container;
        caps.add_array = table_container;
        caps.add_entry = node.kind == TomlNodeKind::ArrayOfTables;
        caps.remove = match node.kind {
            TomlNodeKind::Root => false,
            TomlNodeKind::Value(_) => !node.inline_context || node.inline_direct,
            TomlNodeKind::Table => !node.inline_context,
            TomlNodeKind::ArrayOfTables | TomlNodeKind::ArrayOfTablesEntry => true,
        };
        caps
    }

    fn start_value_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(projection) = self.current_projection(cx) else {
            return;
        };
        let Some(node) = self
            .selected
            .and_then(|index| projection.nodes().get(index))
        else {
            return;
        };
        if !node.kind.is_value() || node.range.is_empty() {
            return;
        }
        let Some(text) = self.current_text(cx) else {
            return;
        };
        let Some(raw) = text.get(node.range.clone()) else {
            return;
        };
        let target = TomlTarget::of(node);
        let title = t_fmt(L10nKey::EditorTomlEditValueTitle, &[("path", &node.path)]).into();
        self.open_edit(
            window,
            cx,
            EditKind::Value,
            target,
            title,
            None,
            EditField::new(
                t(L10nKey::EditorTomlValueName),
                t(L10nKey::EditorTomlValuePlaceholder),
                raw,
            ),
        );
    }

    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(projection) = self.current_projection(cx) else {
            return;
        };
        let Some(node) = self
            .selected
            .and_then(|index| projection.nodes().get(index))
        else {
            return;
        };
        let Some(key_text) = node.key_text.clone() else {
            return;
        };
        let target = TomlTarget::of(node);
        let (label, placeholder) = match node.kind {
            TomlNodeKind::Table => (
                L10nKey::EditorTomlTableName,
                L10nKey::EditorTomlTablePlaceholder,
            ),
            TomlNodeKind::ArrayOfTables => (
                L10nKey::EditorTomlArrayName,
                L10nKey::EditorTomlArrayPlaceholder,
            ),
            _ => (
                L10nKey::EditorTomlKeyName,
                L10nKey::EditorTomlKeyPlaceholder,
            ),
        };
        let title = t_fmt(L10nKey::EditorTomlRenameTitle, &[("path", &node.path)]).into();
        self.open_edit(
            window,
            cx,
            EditKind::Rename,
            target,
            title,
            None,
            EditField::new(t(label), t(placeholder), key_text),
        );
    }

    fn start_add_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.selected_target(cx) else {
            return;
        };
        let Some(path) = self.selected_path_text(cx) else {
            return;
        };
        let title = t_fmt(L10nKey::EditorTomlAddKeyTitle, &[("path", &path)]).into();
        self.open_edit(
            window,
            cx,
            EditKind::AddKey,
            target,
            title,
            Some(EditField::new(
                t(L10nKey::EditorTomlKeyName),
                t(L10nKey::EditorTomlKeyPlaceholder),
                "",
            )),
            EditField::new(
                t(L10nKey::EditorTomlValueName),
                t(L10nKey::EditorTomlValuePlaceholder),
                "",
            ),
        );
    }

    fn start_add_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_named_insert(window, cx, EditKind::AddTable);
    }

    fn start_add_array(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_named_insert(window, cx, EditKind::AddArray);
    }

    fn start_named_insert(&mut self, window: &mut Window, cx: &mut Context<Self>, kind: EditKind) {
        let Some(target) = self.selected_target(cx) else {
            return;
        };
        let Some(path) = self.selected_path_text(cx) else {
            return;
        };
        let (title_key, name_key, placeholder_key) = match kind {
            EditKind::AddTable => (
                L10nKey::EditorTomlAddTableTitle,
                L10nKey::EditorTomlTableName,
                L10nKey::EditorTomlTablePlaceholder,
            ),
            EditKind::AddArray => (
                L10nKey::EditorTomlAddArrayTitle,
                L10nKey::EditorTomlArrayName,
                L10nKey::EditorTomlArrayPlaceholder,
            ),
            _ => return,
        };
        let title = t_fmt(title_key, &[("path", &path)]).into();
        self.open_edit(
            window,
            cx,
            kind,
            target,
            title,
            None,
            EditField::new(t(name_key), t(placeholder_key), ""),
        );
    }

    fn add_entry(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.selected_target(cx) else {
            return;
        };
        match self.apply_mutation(Mutation::AddEntry { array: target }, cx) {
            Ok(()) => {
                self.mutation_error = None;
                cx.notify();
            }
            Err(error) => {
                self.mutation_error = Some(self.mutation_message(error));
                cx.notify();
            }
        }
    }

    fn remove_selected(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.selected_target(cx) else {
            return;
        };
        match self.apply_mutation(Mutation::Remove { target }, cx) {
            Ok(()) => {
                self.mutation_error = None;
                cx.notify();
            }
            Err(error) => {
                self.mutation_error = Some(self.mutation_message(error));
                cx.notify();
            }
        }
    }

    fn open_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        kind: EditKind,
        target: TomlTarget,
        title: SharedString,
        key: Option<EditField>,
        value: EditField,
    ) {
        self.close_edit(None, cx);
        self.full_value = None;
        self.mutation_error = None;
        let owner = cx.entity_id();
        let panel =
            cx.new(|cx| StructureEditPanel::new(owner, window, cx, title, key, value, false));
        let events = cx.subscribe_in(
            &panel,
            window,
            |view, _, event: &StructureEditEvent, window, cx| match event {
                StructureEditEvent::Apply => view.apply_edit(window, cx),
                StructureEditEvent::Cancel => view.close_edit(Some(window), cx),
            },
        );
        panel.update(cx, |panel, cx| panel.focus_first(window, cx));
        let revision = self.document.read(cx).document().read(cx).revision();
        self.editing = Some(EditSession {
            panel,
            kind,
            target,
            revision,
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
        let message: SharedString = t(L10nKey::EditorTomlEditStale).into();
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
        let session_revision = session.revision;
        let revision = self.document.read(cx).document().read(cx).revision();
        if revision != session_revision {
            let message: SharedString = t(L10nKey::EditorTomlEditStale).into();
            panel.update(cx, |panel, cx| panel.set_error(Some(message), cx));
            return;
        }
        let mutation = match kind {
            EditKind::Value => Mutation::SetValue {
                target,
                value: panel.read(cx).value_text(cx),
            },
            EditKind::Rename => Mutation::Rename {
                target,
                key: panel.read(cx).value_text(cx),
            },
            EditKind::AddKey => Mutation::AddKey {
                container: target,
                key: panel.read(cx).key_text(cx).unwrap_or_default(),
                value: panel.read(cx).value_text(cx),
            },
            EditKind::AddTable => Mutation::AddTable {
                container: target,
                key: panel.read(cx).value_text(cx),
            },
            EditKind::AddArray => Mutation::AddArray {
                container: target,
                key: panel.read(cx).value_text(cx),
            },
        };
        match self.apply_mutation(mutation, cx) {
            Ok(()) => {
                self.mutation_error = None;
                self.close_edit(Some(window), cx);
            }
            Err(error) => {
                let message = self.mutation_message(error);
                panel.update(cx, |panel, cx| panel.set_error(Some(message), cx));
            }
        }
    }

    /// 一次命令一条 history：规划出的替换清单交给共享 Document 的正常
    /// `replace_ranges`，不另建可写内容。revision 不一致时拒绝写入。
    fn apply_mutation(
        &mut self,
        mutation: Mutation,
        cx: &mut Context<Self>,
    ) -> Result<(), MutationError> {
        let (projection, installed, editor, revision, text) = {
            let toml_document = self.document.read(cx);
            let document = toml_document.document().clone();
            let editor = self.source.clone();
            let projection = toml_document.projection.clone();
            let installed = toml_document.installed_revision();
            let revision = document.read(cx).revision();
            let text = document.read(cx).snapshot().shared_text();
            (projection, installed, editor, revision, text)
        };
        let projection = projection.ok_or(MutationError::Stale)?;
        if installed != Some(revision) {
            return Err(MutationError::Stale);
        }
        let _ = projection;
        let replacements = mutation::plan(text.as_str(), &mutation)?;
        if !replacements.is_empty() {
            editor.update(cx, |editor, cx| editor.replace_ranges(replacements, cx));
        }
        Ok(())
    }

    fn mutation_message(&self, error: MutationError) -> SharedString {
        match error {
            MutationError::Stale => t(L10nKey::EditorTomlEditStale).into(),
            MutationError::Unsupported => t(L10nKey::EditorTomlEditUnsupported).into(),
            MutationError::InvalidKey(message) => {
                t_fmt(L10nKey::EditorTomlEditInvalidKey, &[("message", &message)]).into()
            }
            MutationError::InvalidValue(message) => t_fmt(
                L10nKey::EditorTomlEditInvalidValue,
                &[("message", &message)],
            )
            .into(),
            MutationError::DuplicateKey(key) => {
                t_fmt(L10nKey::EditorTomlEditDuplicate, &[("key", &key)]).into()
            }
            MutationError::ResultInvalid {
                line,
                column,
                message,
            } => t_fmt(
                L10nKey::EditorTomlEditResultInvalid,
                &[
                    ("line", &line.to_string()),
                    ("column", &column.to_string()),
                    ("message", &message),
                ],
            )
            .into(),
            MutationError::ResultShape => t(L10nKey::EditorTomlEditResultShape).into(),
        }
    }

    fn selected_detail_range(&self, cx: &App) -> Option<Range<usize>> {
        let projection = self.document.read(cx).projection()?;
        let node = projection.nodes().get(self.selected?)?;
        node.has_source_detail().then(|| node.detail.clone())
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
            self.selected_range = None;
            self.full_value = None;
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
            if let (Some(path), Some(range)) =
                (self.selected_path.as_deref(), self.selected_range.clone())
            {
                self.selected = projection
                    .nodes()
                    .iter()
                    .position(|node| node.path == path && node.range == range)
                    .filter(|index| *index < node_count);
            } else {
                self.selected = self.selected.filter(|index| *index < node_count);
            }
        } else if self.selected.is_some_and(|index| index >= node_count) {
            self.selected = None;
        }
        if self.selected.is_none() {
            self.selected_path = None;
            self.selected_range = None;
        }
        if revision_changed {
            if let Some(detail) = &self.full_value
                && !projection
                    .nodes()
                    .iter()
                    .any(|node| node.path == detail.path && node.detail == detail.range)
            {
                self.full_value = None;
            }
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
        let Some((path, range)) = self
            .document
            .read(cx)
            .projection()
            .and_then(|projection| projection.nodes().get(index))
            .map(|node| (node.path.clone(), node.range.clone()))
        else {
            return;
        };
        self.close_edit(None, cx);
        self.mutation_error = None;
        self.selected = Some(index);
        self.selected_path = Some(path);
        self.selected_range = Some(range);
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
        let counts = self
            .node_count(cx)
            .map(|count| t_fmt(L10nKey::EditorTomlNodes, &[("count", &count.to_string())]));
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
        let editable = self.structure_editable(cx);
        let mutation_error = self.mutation_error.clone();
        let mode = self.mode;
        let (toggle, tip) = match mode {
            TomlMode::Structure => (
                t(L10nKey::EditorTomlSource),
                t(L10nKey::EditorTomlShowSource),
            ),
            TomlMode::Source => (
                t(L10nKey::EditorTomlStructure),
                t(L10nKey::EditorTomlShowStructure),
            ),
        };
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let warning = cx.theme().warning;
        h_flex()
            .id(("toml-toolbar", entity))
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
            .child(div().flex_none().text_xs().text_color(muted).child("TOML"))
            .when_some(counts, |row, counts| {
                row.child(
                    div()
                        .id(("toml-count", entity))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(counts),
                )
            })
            .when(parsing, |row| {
                row.child(
                    h_flex()
                        .id(("toml-parsing", entity))
                        .test_support()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .text_color(muted)
                        .child(Spinner::new().xsmall())
                        .child(t(L10nKey::EditorTomlParsing)),
                )
            })
            .when_some(fatal, |row, (title, _)| {
                row.child(
                    div()
                        .id(("toml-structure-error", entity))
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
                        .id(("toml-location", entity))
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
                        .id(("toml-mutation-error", entity))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(warning)
                        .child(error),
                )
            })
            .when(editable, |row| {
                row.child(
                    Button::new(("toml-edit-value", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.value)
                        .label(t(L10nKey::EditorTomlEditValue))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_value_edit(window, cx)),
                        ),
                )
                .child(
                    Button::new(("toml-rename", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.rename)
                        .label(t(L10nKey::EditorTomlRename))
                        .on_click(cx.listener(|this, _, window, cx| this.start_rename(window, cx))),
                )
                .child(
                    Button::new(("toml-add-key", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.add_key)
                        .label(t(L10nKey::EditorTomlAddKey))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_add_key(window, cx)),
                        ),
                )
                .child(
                    Button::new(("toml-add-table", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.add_table)
                        .label(t(L10nKey::EditorTomlAddTable))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_add_table(window, cx)),
                        ),
                )
                .child(
                    Button::new(("toml-add-array", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.add_array)
                        .label(t(L10nKey::EditorTomlAddArray))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_add_array(window, cx)),
                        ),
                )
                .child(
                    Button::new(("toml-add-entry", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.add_entry)
                        .label(t(L10nKey::EditorTomlAddEntry))
                        .on_click(cx.listener(|this, _, _, cx| this.add_entry(cx))),
                )
                .child(
                    Button::new(("toml-remove", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.remove)
                        .label(t(L10nKey::EditorTomlRemove))
                        .on_click(cx.listener(|this, _, _, cx| this.remove_selected(cx))),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new(("toml-reveal", entity))
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .disabled(!has_detail)
                    .label(t(L10nKey::EditorTomlReveal))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.reveal_selected_source(window, cx)),
                    ),
            )
            .child(
                Button::new(("toml-full-value-button", entity))
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .disabled(!has_detail)
                    .label(t(L10nKey::EditorTomlFullValue))
                    .on_click(cx.listener(|this, _, _, cx| this.open_selected_full_value(cx))),
            )
            .child(
                Button::new(("toml-mode-toggle", entity))
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
                    ("toml-structure-error-body", cx.entity_id()),
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
                    ("toml-structure-loading", cx.entity_id()),
                    t(L10nKey::EditorTomlParsing).into(),
                    cx,
                )
            } else {
                structure_error(
                    ("toml-structure-error-body", cx.entity_id()),
                    t(L10nKey::EditorTomlStructureUnavailable).into(),
                    t(L10nKey::EditorTomlSourceStaysEditable).into(),
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
            ("toml-structure", entity),
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
                        let path = node.path.clone();
                        let key = node.key.clone();
                        let value = node.value.clone();
                        let kind = node.kind;
                        let color = match kind {
                            TomlNodeKind::Value(TomlValueKind::String) => cx.theme().green,
                            TomlNodeKind::Value(TomlValueKind::Integer)
                            | TomlNodeKind::Value(TomlValueKind::Float) => cx.theme().blue,
                            TomlNodeKind::Value(TomlValueKind::Boolean) => cx.theme().magenta,
                            TomlNodeKind::Value(TomlValueKind::Datetime) => cx.theme().cyan,
                            TomlNodeKind::Value(TomlValueKind::Array)
                            | TomlNodeKind::Value(TomlValueKind::InlineTable) => {
                                cx.theme().muted_foreground
                            }
                            TomlNodeKind::Table
                            | TomlNodeKind::ArrayOfTables
                            | TomlNodeKind::ArrayOfTablesEntry => cx.theme().yellow,
                            TomlNodeKind::Root => cx.theme().foreground,
                        };
                        let index = row.node;
                        let has_detail = node.has_source_detail();
                        let row_id =
                            SharedString::from(format!("toml-row-{}-{index}", entity.as_u64()));
                        structure_row(
                            StructureRowData {
                                id: row_id,
                                depth: row.depth,
                                expandable,
                                expanded,
                                selected,
                                striped: row_index % 2 == 1,
                                key: key.into(),
                                value: value.into(),
                                color,
                                badge: None,
                                record_line: None,
                                error: None,
                                aria_label: path.into(),
                            },
                            cx.listener(|view, _: &gpui_kit::MouseDownEvent, window, cx| {
                                view.focus.focus(window, cx);
                                cx.stop_propagation();
                            }),
                            cx.listener(move |view, event: &gpui_kit::ClickEvent, window, cx| {
                                view.focus.focus(window, cx);
                                if event.click_count() > 1 {
                                    view.select_node(index, cx);
                                    if has_detail {
                                        view.open_selected_full_value(cx);
                                    }
                                } else {
                                    view.activate_node(index, cx);
                                }
                            }),
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
        let detail = self.render_full_value(cx);
        v_flex()
            .id(("toml-structure-body", entity))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(list)
            .children(self.render_edit_panel())
            .children(detail)
            .into_any_element()
    }

    fn render_edit_panel(&self) -> Option<AnyElement> {
        self.editing
            .as_ref()
            .map(|session| session.panel.clone().into_any_element())
    }

    fn render_full_value(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let path = self.full_value_path()?;
        let text = self.full_value_text(cx)?;
        let entity = cx.entity_id();
        Some(
            v_flex()
                .id(("toml-full-value", entity))
                .test_support()
                .flex_none()
                .max_h(rems(18.))
                .border_t_1()
                .border_color(cx.theme().border)
                .child(
                    h_flex()
                        .flex_none()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{} · {path}", t(L10nKey::EditorTomlFullValue))),
                        )
                        .child(
                            Button::new(("toml-full-value-copy", entity))
                                .ghost()
                                .xsmall()
                                .flex_none()
                                .label(t(L10nKey::EditorTomlCopyValue))
                                .on_click(cx.listener(|this, _, _, cx| this.copy_full_value(cx))),
                        )
                        .child(
                            Button::new(("toml-full-value-close", entity))
                                .ghost()
                                .xsmall()
                                .flex_none()
                                .label(t(L10nKey::Close))
                                .on_click(cx.listener(|this, _, _, cx| this.close_full_value(cx))),
                        ),
                )
                .child(
                    div()
                        .id(("toml-full-value-text", entity))
                        .test_support()
                        .aria_value(text.clone())
                        .flex_1()
                        .min_h_0()
                        .overflow_scroll()
                        .px_2()
                        .pb_2()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs()
                        .whitespace_normal()
                        .child(text),
                )
                .into_any_element(),
        )
    }

    fn source_body(&self) -> AnyElement {
        source_body_for(&self.source)
    }
}

fn default_expansion(projection: &TomlProjection, expanded: &mut [bool]) {
    let Some(root) = projection.root() else {
        return;
    };
    if root >= expanded.len() {
        return;
    }
    expanded[root] = true;
    let mut budget = AUTO_EXPAND_NODES;
    expand_children(projection, root, 1, expanded, &mut budget);
}

fn expand_children(
    projection: &TomlProjection,
    node: usize,
    depth: usize,
    expanded: &mut [bool],
    budget: &mut usize,
) {
    if depth > AUTO_EXPAND_DEPTH || *budget == 0 {
        return;
    }
    for &child in projection.children(node) {
        if projection.is_expandable(child)
            && projection.children(child).len() <= AUTO_EXPAND_CHILDREN
            && *budget > 0
        {
            expanded[child] = true;
            *budget -= 1;
            expand_children(projection, child, depth + 1, expanded, budget);
        }
    }
}

impl Focusable for TomlView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.mode {
            TomlMode::Structure => self.focus.clone(),
            TomlMode::Source => self.source.read(cx).focus_handle(cx),
        }
    }
}

impl Render for TomlView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.mode;
        let focus = self.focus.clone();
        let focus_ring = cx.theme().primary;
        let item = v_flex()
            .id(("toml-document", cx.entity_id()))
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(self.toolbar(cx));
        let body = match mode {
            TomlMode::Structure => self.structure_body(cx),
            TomlMode::Source => self.source_body(),
        };
        let item = item.child(body);
        match mode {
            TomlMode::Structure => {
                item.key_context(KEY_CONTEXT)
                    .track_focus(&focus)
                    .focus_visible(move |style| style.border_1().border_color(focus_ring))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.focus.focus(window, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        this.key(event, window, cx)
                    }))
                    .into_any_element()
            }
            TomlMode::Source => item.into_any_element(),
        }
    }
}

/// 右侧菜单的 TOML 项：默认只给结构与源码切换；编辑命令由结构工具栏提供。
pub(super) fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
) -> gpui_kit::component::menu::PopupMenu {
    menu.separator().menu(
        t(L10nKey::EditorTomlToggleStructure),
        Box::new(crate::core::actions::ToggleDocumentPreview),
    )
}

/// 解析的确定性单元测试门闩，不编译进产品或独立 native 消费者。
#[cfg(test)]
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

    /// Pauses the next worker parse before it reads the projection. The
    /// returned sender releases it; the flag reports that the worker arrived.
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
#[path = "toml_document/tests.rs"]
mod tests;
