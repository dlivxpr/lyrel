//! JSON 文档：一份共享逐行保真源码，加上按 revision 派生的无损结构视图。
//!
//! JSON／JSONC 默认显示结构树，JSONL／NDJSON 显示逐行记录；源码始终是同一
//! [`lyrel_markdown_editor::Document`] 的 plain-source presentation，保存、
//! 撤销、reload 与关闭都走既有工作台生命周期。结构节点只保存源码范围与
//! 显示文本，不复制第二份可变内容，也不通过 `serde_json::Value` 回写。
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
use gpui_kit::component::input::{Redo, Undo};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, Entity, FocusHandle, Focusable, KeyDownEvent,
    MouseButton, Render, SharedString, Subscription, Task, TestSupportExt as _,
    UniformListScrollHandle, Window, div, px, relative, rems, uniform_list,
};
use lyrel_markdown_editor::{Document, DocumentEvent, EditorState as SourceEditor};

use super::structure_edit::{EditField, StructureEditEvent, StructureEditPanel};
use super::structure_view::{self, SourceLocation, Treelike, VisibleRow};
use crate::ui::i18n::{L10nKey, t, t_fmt};

pub(crate) mod mutation;
pub(crate) mod projection;
pub(crate) use projection::JsonFormat;

use mutation::{Mutation, MutationError, NodeTarget};
use projection::{
    JsonNodeKind, JsonProjection, ProjectionFailure, STRUCTURE_INPUT_BYTES, STRUCTURE_NODES,
    STRUCTURE_RECORDS,
};

/// 合并连续输入；每份文档最多一个 worker 与一次在途解析。
const PARSE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(120);
/// 打开时自动展开根的直接子容器，复现上游的浅层可读树。
const AUTO_EXPAND_CHILDREN: usize = 8;
/// 键盘 PageUp／PageDown 跨过的可见行数。
const PAGE_ROWS: isize = 10;

/// 结构预览持有键盘时成立的上下文；源码面不声明它，文字编辑键不会被截走。
pub(crate) const KEY_CONTEXT: &str = "JsonStructure";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JsonMode {
    Structure,
    Source,
}

impl JsonMode {
    pub(crate) fn opposite(self) -> Self {
        match self {
            Self::Structure => Self::Source,
            Self::Source => Self::Structure,
        }
    }
}

/// 一份 JSON 文件的内容：共享 Document 与按最新 revision 安装的投影。
pub(crate) struct JsonDocument {
    document: Entity<Document>,
    format: JsonFormat,
    projection: Option<Arc<JsonProjection>>,
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

impl Drop for JsonDocument {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl JsonDocument {
    pub(super) fn new(
        document: Entity<Document>,
        format: JsonFormat,
        cx: &mut Context<Self>,
    ) -> Self {
        let watch = cx.subscribe(&document, |this, _, _: &DocumentEvent, cx| {
            this.invalidate(cx)
        });
        let mut this = Self {
            document,
            format,
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

    pub(crate) fn format(&self) -> JsonFormat {
        self.format
    }

    pub(crate) fn projection(&self) -> Option<&JsonProjection> {
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
                        let format = this.format;
                        let generation = this.generation;
                        this.cancel.store(false, Ordering::Release);
                        Some((snapshot, revision, generation, format, this.cancel.clone()))
                    })
                    .ok()
                    .flatten();
                let Some((snapshot, revision, generation, format, cancel)) = job else {
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
                    Some(projection::project(snapshot.text(), format, &cancel))
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
    Key,
    AddMember,
    AddRecord,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct EditCaps {
    value: bool,
    key: bool,
    add: bool,
    add_is_array: bool,
    remove: bool,
    record_root: bool,
    add_record: bool,
    remove_record: bool,
}

/// 一次明确的树命令：打开面板时固定目标 occurrence 与源码 revision，
/// 提交时两者都必须仍匹配。面板不持有文档；`AddRecord` 的 `target`
/// 是可选的前一条记录，`None` 表示追加到文件末尾。
struct EditSession {
    panel: Entity<StructureEditPanel>,
    kind: EditKind,
    target: Option<NodeTarget>,
    revision: u64,
    _events: Subscription,
}

/// 一份 JSON 文件在一个分组里的呈现：模式、源码选择、结构展开与滚动。
pub(crate) struct JsonView {
    document: Entity<JsonDocument>,
    source: Entity<SourceEditor>,
    mode: JsonMode,
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

impl JsonView {
    pub(super) fn new(
        document: Entity<JsonDocument>,
        mode: JsonMode,
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

    pub(crate) fn document(&self) -> &Entity<JsonDocument> {
        &self.document
    }

    pub(crate) fn source(&self) -> &Entity<SourceEditor> {
        &self.source
    }

    pub(crate) fn mode(&self) -> JsonMode {
        self.mode
    }

    pub(crate) fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.mode {
            JsonMode::Source => self.source.read(cx).focus_handle(cx),
            JsonMode::Structure => self.focus.clone(),
        }
    }

    pub(crate) fn scroll_offset(&self, cx: &App) -> gpui_kit::Point<gpui_kit::Pixels> {
        match self.mode {
            JsonMode::Source => self.source.read(cx).scroll_offset(),
            JsonMode::Structure => self.structure_scroll_offset(),
        }
    }

    pub(crate) fn set_scroll_offset(
        &mut self,
        offset: gpui_kit::Point<gpui_kit::Pixels>,
        cx: &mut Context<Self>,
    ) {
        match self.mode {
            JsonMode::Source => self
                .source
                .update(cx, |source, cx| source.set_scroll_offset(offset, cx)),
            JsonMode::Structure => {
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
        let projection = self.document.read(cx).projection()?;
        (!matches!(projection.format(), JsonFormat::JsonLines)).then_some(projection.node_count())
    }

    pub(crate) fn record_count(&self, cx: &App) -> Option<usize> {
        let projection = self.document.read(cx).projection()?;
        matches!(projection.format(), JsonFormat::JsonLines).then_some(projection.record_count())
    }

    pub(crate) fn invalid_records(&self, cx: &App) -> usize {
        self.document
            .read(cx)
            .projection()
            .map_or(0, JsonProjection::invalid_records)
    }

    /// 预算失败或整份 JSON 无法解析时的可见说明；JSONL 的逐行错误不挡住
    /// 其他记录，由行错误与计数单独显示。
    pub(crate) fn fatal_error(&self, cx: &App) -> Option<(SharedString, SharedString)> {
        let document = self.document.read(cx);
        if let Some(failure) = document.failure() {
            let message = match failure {
                ProjectionFailure::Cancelled => return None,
                ProjectionFailure::InputTooLarge => t_fmt(
                    L10nKey::EditorJsonInputLimit,
                    &[("mib", &(STRUCTURE_INPUT_BYTES / (1024 * 1024)).to_string())],
                )
                .into(),
                ProjectionFailure::TooManyNodes => t_fmt(
                    L10nKey::EditorJsonNodeLimit,
                    &[("count", &STRUCTURE_NODES.to_string())],
                )
                .into(),
                ProjectionFailure::TooManyRecords => t_fmt(
                    L10nKey::EditorJsonRecordLimit,
                    &[("count", &STRUCTURE_RECORDS.to_string())],
                )
                .into(),
                ProjectionFailure::PathsTooLarge => t_fmt(
                    L10nKey::EditorJsonPathLimit,
                    &[(
                        "mib",
                        &(projection::STRUCTURE_PATH_BYTES / (1024 * 1024)).to_string(),
                    )],
                )
                .into(),
            };
            return Some((t(L10nKey::EditorJsonStructureUnavailable).into(), message));
        }
        let projection = document.projection()?;
        let error = projection.error()?;
        Some((
            t(L10nKey::EditorJsonInvalid).into(),
            t_fmt(
                L10nKey::EditorJsonInvalidDetail,
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
        if self.mode != JsonMode::Source {
            self.mode = JsonMode::Source;
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
        self.mode = JsonMode::Source;
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
            .map(|node| (SharedString::from(node.path.clone()), node.detail_range()));
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

    /// 当前已安装的投影，且它的 revision 必须仍等于源码权威。任何不一致
    /// 都按过期处理，不允许用旧范围规划写入。
    fn current_projection(&self, cx: &App) -> Option<Arc<JsonProjection>> {
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

    fn selected_target(&self, cx: &App) -> Option<NodeTarget> {
        let projection = self.current_projection(cx)?;
        let node = projection.nodes().get(self.selected?)?;
        Some(NodeTarget::of(node))
    }

    fn edit_caps(&self, cx: &App) -> EditCaps {
        let mut caps = EditCaps::default();
        let Some(projection) = self.current_projection(cx) else {
            return caps;
        };
        let format = self.document.read(cx).format();
        let Some(index) = self.selected else {
            caps.add_record = format == JsonFormat::JsonLines;
            return caps;
        };
        let Some(node) = projection.nodes().get(index) else {
            caps.add_record = format == JsonFormat::JsonLines;
            return caps;
        };
        caps.record_root = projection.is_record_root(index);
        caps.value = caps.record_root || value_editable(node);
        caps.key = node.key_range.is_some();
        caps.add = matches!(node.kind, JsonNodeKind::Object | JsonNodeKind::Array);
        caps.add_is_array = node.kind == JsonNodeKind::Array;
        caps.remove = node.parent.is_some_and(|parent| {
            node.key_range.is_some()
                || projection
                    .nodes()
                    .get(parent)
                    .is_some_and(|parent| parent.kind == JsonNodeKind::Array)
        });
        caps.add_record = format == JsonFormat::JsonLines;
        caps.remove_record = caps.record_root;
        caps
    }

    fn start_value_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(projection) = self.current_projection(cx) else {
            return;
        };
        let Some(index) = self.selected else {
            return;
        };
        let Some(node) = projection.nodes().get(index) else {
            return;
        };
        let record_root = projection.is_record_root(index);
        if !record_root && !value_editable(node) {
            return;
        }
        let Some(text) = self.current_text(cx) else {
            return;
        };
        let Some(raw) = text.get(node.range.clone()) else {
            return;
        };
        let target = NodeTarget::of(node);
        let title_key = match record_root {
            true => L10nKey::EditorJsonEditRecordTitle,
            false => L10nKey::EditorJsonEditValueTitle,
        };
        let title = t_fmt(title_key, &[("path", &node.path)]).into();
        self.open_edit(
            window,
            cx,
            EditKind::Value,
            Some(target),
            title,
            None,
            EditField::new(
                t(L10nKey::EditorJsonValueName),
                t(L10nKey::EditorJsonValuePlaceholder),
                raw,
            ),
        );
    }

    fn start_key_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(projection) = self.current_projection(cx) else {
            return;
        };
        let Some(node) = self
            .selected
            .and_then(|index| projection.nodes().get(index))
        else {
            return;
        };
        let (Some(key_text), Some(_)) = (node.key_text.clone(), node.key_range.as_ref()) else {
            return;
        };
        let target = NodeTarget::of(node);
        let title = t_fmt(L10nKey::EditorJsonRenameKeyTitle, &[("path", &node.path)]).into();
        self.open_edit(
            window,
            cx,
            EditKind::Key,
            Some(target),
            title,
            None,
            EditField::new(
                t(L10nKey::EditorJsonKeyName),
                t(L10nKey::EditorJsonKeyPlaceholder),
                key_text,
            ),
        );
    }

    fn start_add_member(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(projection) = self.current_projection(cx) else {
            return;
        };
        let Some(node) = self
            .selected
            .and_then(|index| projection.nodes().get(index))
        else {
            return;
        };
        let (title_key, key_field) = match node.kind {
            JsonNodeKind::Object => (
                L10nKey::EditorJsonAddKeyTitle,
                Some(EditField::new(
                    t(L10nKey::EditorJsonKeyName),
                    t(L10nKey::EditorJsonKeyPlaceholder),
                    "",
                )),
            ),
            JsonNodeKind::Array => (L10nKey::EditorJsonAddItemTitle, None),
            _ => return,
        };
        let target = NodeTarget::of(node);
        let title = t_fmt(title_key, &[("path", &node.path)]).into();
        self.open_edit(
            window,
            cx,
            EditKind::AddMember,
            Some(target),
            title,
            key_field,
            EditField::new(
                t(L10nKey::EditorJsonValueName),
                t(L10nKey::EditorJsonValuePlaceholder),
                "",
            ),
        );
    }

    /// JSONL／NDJSON 的记录动作：选中一条记录时在其后插入，否则追加到
    /// 文件末尾；新记录必须保持单行。
    fn start_add_record(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(projection) = self.current_projection(cx) else {
            return;
        };
        if self.document.read(cx).format() != JsonFormat::JsonLines {
            return;
        }
        let record = self
            .selected
            .filter(|index| projection.is_record_root(*index));
        let after = record.map(|index| NodeTarget::of(&projection.nodes()[index]));
        let line = record.and_then(|index| projection.nodes()[index].record_line);
        let title = match line {
            Some(line) => t_fmt(
                L10nKey::EditorJsonAddRecordAfterTitle,
                &[("line", &line.to_string())],
            )
            .into(),
            None => t(L10nKey::EditorJsonAddRecordEndTitle).into(),
        };
        self.open_edit(
            window,
            cx,
            EditKind::AddRecord,
            after,
            title,
            None,
            EditField::new(
                t(L10nKey::EditorJsonValueName),
                t(L10nKey::EditorJsonValuePlaceholder),
                "",
            ),
        );
    }

    fn remove_selected_record(&mut self, cx: &mut Context<Self>) {
        let Some(projection) = self.current_projection(cx) else {
            return;
        };
        if self.document.read(cx).format() != JsonFormat::JsonLines {
            return;
        }
        let Some(index) = self
            .selected
            .filter(|index| projection.is_record_root(*index))
        else {
            return;
        };
        let target = NodeTarget::of(&projection.nodes()[index]);
        match self.apply_mutation(Mutation::RemoveRecord { target }, cx) {
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
        match self.apply_mutation(Mutation::RemoveMember { target }, cx) {
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
        target: Option<NodeTarget>,
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
        self.editing = Some(EditSession {
            panel,
            kind,
            target,
            revision: self.document.read(cx).document().read(cx).revision(),
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
        let message: SharedString = t(L10nKey::EditorJsonEditStale).into();
        session
            .panel
            .update(cx, |panel, cx| panel.set_error(Some(message), cx));
    }

    fn apply_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.editing.as_ref() else {
            return;
        };
        if self.document.read(cx).document().read(cx).revision() != session.revision {
            self.mark_edit_stale(cx);
            return;
        }
        let panel = session.panel.clone();
        let kind = session.kind;
        let target = session.target.clone();
        let mutation = match (kind, target) {
            (EditKind::Value, Some(target)) => Mutation::SetValue {
                target,
                value: panel.read(cx).value_text(cx),
            },
            (EditKind::Key, Some(target)) => Mutation::RenameKey {
                target,
                key: panel.read(cx).value_text(cx),
            },
            (EditKind::AddMember, Some(target)) => Mutation::AddMember {
                container: target,
                key: panel.read(cx).key_text(cx),
                value: panel.read(cx).value_text(cx),
            },
            (EditKind::AddRecord, after) => Mutation::AddRecord {
                after,
                value: panel.read(cx).value_text(cx),
            },
            (EditKind::Value | EditKind::Key | EditKind::AddMember, None) => {
                let message = t(L10nKey::EditorJsonEditUnsupported).into();
                panel.update(cx, |panel, cx| panel.set_error(Some(message), cx));
                return;
            }
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
        let (projection, installed, editor, revision, text, format) = {
            let json_document = self.document.read(cx);
            let document = json_document.document().clone();
            let editor = self.source.clone();
            let format = json_document.format();
            let projection = json_document.projection.clone();
            let installed = json_document.installed_revision();
            let revision = document.read(cx).revision();
            let text = document.read(cx).snapshot().shared_text();
            (projection, installed, editor, revision, text, format)
        };
        let projection = projection.ok_or(MutationError::Stale)?;
        if installed != Some(revision) {
            return Err(MutationError::Stale);
        }
        let replacements = mutation::plan(text.as_str(), &projection, format, &mutation)?;
        if !replacements.is_empty() {
            editor.update(cx, |editor, cx| editor.replace_ranges(replacements, cx));
        }
        Ok(())
    }

    fn mutation_message(&self, error: MutationError) -> SharedString {
        match error {
            MutationError::Stale => t(L10nKey::EditorJsonEditStale).into(),
            MutationError::Unsupported => t(L10nKey::EditorJsonEditUnsupported).into(),
            MutationError::InvalidValue(message) => t_fmt(
                L10nKey::EditorJsonEditInvalidValue,
                &[("message", &message)],
            )
            .into(),
            MutationError::RecordNotSingleLine => t(L10nKey::EditorJsonEditRecordLine).into(),
            MutationError::ResultInvalid {
                line,
                column,
                message,
            } => t_fmt(
                L10nKey::EditorJsonEditResultInvalid,
                &[
                    ("line", &line.to_string()),
                    ("column", &column.to_string()),
                    ("message", &message),
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
            let path = self.selected_path.clone();
            let range = self.selected_range.clone();
            self.selected = match path {
                Some(path) => projection
                    .nodes()
                    .iter()
                    .position(|node| {
                        node.path == path
                            && range.as_ref().is_some_and(|range| node.range == *range)
                    })
                    .or_else(|| {
                        // The selected occurrence still exists but its own
                        // value changed; keep it by path. Stale write targets
                        // are never trusted: mutations re-resolve path+range.
                        projection.nodes().iter().position(|node| node.path == path)
                    })
                    .filter(|index| *index < node_count),
                None => self.selected.filter(|index| *index < node_count),
            };
        } else if self.selected.is_some_and(|index| index >= node_count) {
            self.selected = None;
        }
        match self
            .selected
            .and_then(|index| projection.nodes().get(index))
        {
            Some(node) => {
                self.selected_path = Some(node.path.clone());
                self.selected_range = Some(node.range.clone());
            }
            None => {
                self.selected = None;
                self.selected_path = None;
                self.selected_range = None;
            }
        }
        if revision_changed {
            if let Some(detail) = &self.full_value
                && !projection
                    .nodes()
                    .iter()
                    .any(|node| node.path == detail.path && node.detail_range() == detail.range)
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

    /// 结构树持有键盘时的普通撤销：仍由共享源码 presentation 的既有 history
    /// 执行（`EditorState::undo_document`），不切换 Source 模式，也不另建
    /// 可写内容或第二套 history。撤销后键盘仍留在树上。
    fn undo_structure(&mut self, cx: &mut Context<Self>) {
        let source = self.source.clone();
        source.update(cx, |source, cx| source.undo_document(cx));
    }

    /// 结构树持有键盘时的普通重做；见 [`Self::undo_structure`]。
    fn redo_structure(&mut self, cx: &mut Context<Self>) {
        let source = self.source.clone();
        source.update(cx, |source, cx| source.redo_document(cx));
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
        let format = document.format();
        let parsing = document.is_parsing();
        let fatal = self.fatal_error(cx);
        let invalid_records = self.invalid_records(cx);
        let counts = match format {
            JsonFormat::JsonLines => self
                .record_count(cx)
                .map(|count| t_fmt(L10nKey::EditorJsonRecords, &[("count", &count.to_string())])),
            JsonFormat::Json | JsonFormat::Jsonc => self
                .node_count(cx)
                .map(|count| t_fmt(L10nKey::EditorJsonNodes, &[("count", &count.to_string())])),
        };
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
            JsonMode::Structure => (
                t(L10nKey::EditorJsonSource),
                t(L10nKey::EditorJsonShowSource),
            ),
            JsonMode::Source => (
                t(L10nKey::EditorJsonStructure),
                t(L10nKey::EditorJsonShowStructure),
            ),
        };
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let warning = cx.theme().warning;
        h_flex()
            .id(("json-toolbar", entity))
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
                    .child(format.label()),
            )
            .when_some(counts, |row, counts| {
                row.child(
                    div()
                        .id(("json-count", entity))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(counts),
                )
            })
            .when(invalid_records > 0, |row| {
                row.child(
                    div()
                        .id(("json-invalid-records", entity))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(warning)
                        .child(t_fmt(
                            L10nKey::EditorJsonInvalidRecords,
                            &[("count", &invalid_records.to_string())],
                        )),
                )
            })
            .when(parsing, |row| {
                row.child(
                    h_flex()
                        .id(("json-parsing", entity))
                        .test_support()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .text_color(muted)
                        .child(Spinner::new().xsmall())
                        .child(t(L10nKey::EditorJsonParsing)),
                )
            })
            .when_some(fatal, |row, (title, _)| {
                row.child(
                    div()
                        .id(("json-structure-error", entity))
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
                        .id(("json-location", entity))
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
                        .id(("json-mutation-error", entity))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(warning)
                        .child(error),
                )
            })
            .when(editable, |row| {
                row.child(
                    Button::new(("json-edit-value", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.value)
                        .label(match caps.record_root {
                            true => t(L10nKey::EditorJsonEditRecord),
                            false => t(L10nKey::EditorJsonEditValue),
                        })
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_value_edit(window, cx)),
                        ),
                )
                .child(
                    Button::new(("json-rename-key", entity))
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
                    Button::new(("json-add-member", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.add)
                        .label(match caps.add_is_array {
                            true => t(L10nKey::EditorJsonAddItem),
                            false => t(L10nKey::EditorJsonAddKey),
                        })
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_add_member(window, cx)),
                        ),
                )
                .child(
                    Button::new(("json-remove-member", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.remove)
                        .label(t(L10nKey::EditorJsonRemove))
                        .on_click(cx.listener(|this, _, _, cx| this.remove_selected(cx))),
                )
                .when(format == JsonFormat::JsonLines, |row| {
                    row.child(
                        Button::new(("json-add-record", entity))
                            .ghost()
                            .xsmall()
                            .flex_none()
                            .disabled(!caps.add_record)
                            .label(t(L10nKey::EditorJsonAddRecord))
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.start_add_record(window, cx)
                                }),
                            ),
                    )
                    .child(
                        Button::new(("json-remove-record", entity))
                            .ghost()
                            .xsmall()
                            .flex_none()
                            .disabled(!caps.remove_record)
                            .label(t(L10nKey::EditorJsonRemoveRecord))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.remove_selected_record(cx)),
                            ),
                    )
                })
            })
            .child(div().flex_1())
            .child(
                Button::new(("json-reveal", entity))
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
                Button::new(("json-full-value-button", entity))
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .disabled(!has_detail)
                    .label(t(L10nKey::EditorJsonFullValue))
                    .on_click(cx.listener(|this, _, _, cx| this.open_selected_full_value(cx))),
            )
            .child(
                Button::new(("json-mode-toggle", entity))
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
                .child(structural_error(title, detail, cx))
                .children(self.render_edit_panel())
                .into_any_element();
        }
        if !self.structure_ready(cx) {
            let body = if self.is_parsing(cx) {
                loading_structure(cx).into_any_element()
            } else {
                structural_error(
                    t(L10nKey::EditorJsonStructureUnavailable).into(),
                    t(L10nKey::EditorJsonSourceStaysEditable).into(),
                    cx,
                )
                .into_any_element()
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
            ("json-structure", entity),
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
                        let record_line = node.record_line;
                        let error = node.error.clone();
                        let has_detail = node.has_source_detail();
                        let color = match kind {
                            JsonNodeKind::String => cx.theme().green,
                            JsonNodeKind::Number => cx.theme().blue,
                            JsonNodeKind::Boolean => cx.theme().magenta,
                            JsonNodeKind::InvalidRecord => cx.theme().warning,
                            JsonNodeKind::Object | JsonNodeKind::Array | JsonNodeKind::Records => {
                                cx.theme().muted_foreground
                            }
                            JsonNodeKind::Null => cx.theme().muted_foreground,
                        };
                        let index = row.node;
                        let row_id =
                            SharedString::from(format!("json-row-{}-{index}", entity.as_u64()));
                        div()
                            .id(row_id)
                            .test_support()
                            .role(gpui_kit::Role::TreeItem)
                            .aria_label(path.clone())
                            .aria_value(value.clone())
                            .aria_selected(selected)
                            .when(expandable, |row| row.aria_expanded(expanded))
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(30.))
                            .pl(px(10. + row.depth.min(24) as f32 * 14.))
                            .pr_3()
                            .text_sm()
                            .font_family("Menlo")
                            .when(row_index % 2 == 1, |row| {
                                row.bg(cx.theme().foreground.opacity(0.025))
                            })
                            .when(selected, |row| row.bg(cx.theme().sidebar_accent))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, _, window, cx| {
                                    view.focus.focus(window, cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .on_click(cx.listener(
                                move |view, event: &gpui_kit::ClickEvent, window, cx| {
                                    view.focus.focus(window, cx);
                                    if event.click_count() > 1 {
                                        view.select_node(index, cx);
                                        if has_detail {
                                            view.open_selected_full_value(cx);
                                        }
                                    } else {
                                        view.activate_node(index, cx);
                                    }
                                },
                            ))
                            .child(div().w(px(14.)).flex_shrink_0().when(expandable, |spacer| {
                                spacer.child(Icon::new(if expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                }))
                            }))
                            .when_some(record_line, |row, line| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!("L{line}")),
                                )
                            })
                            .child(
                                div()
                                    .max_w(relative(0.42))
                                    .truncate()
                                    .text_color(cx.theme().blue)
                                    .child(key),
                            )
                            .child(div().text_color(cx.theme().muted_foreground).child(":"))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(color)
                                    .child(value),
                            )
                            .when_some(error, |row, error| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(cx.theme().warning)
                                        .child(error),
                                )
                            })
                            .into_any_element()
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
            .id(("json-structure-body", entity))
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
                .id(("json-full-value", entity))
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
                                .child(format!("{} · {path}", t(L10nKey::EditorJsonFullValue))),
                        )
                        .child(
                            Button::new(("json-full-value-copy", entity))
                                .ghost()
                                .xsmall()
                                .flex_none()
                                .label(t(L10nKey::EditorJsonCopyValue))
                                .on_click(cx.listener(|this, _, _, cx| this.copy_full_value(cx))),
                        )
                        .child(
                            Button::new(("json-full-value-close", entity))
                                .ghost()
                                .xsmall()
                                .flex_none()
                                .label(t(L10nKey::Close))
                                .on_click(cx.listener(|this, _, _, cx| this.close_full_value(cx))),
                        ),
                )
                .child(
                    div()
                        .id(("json-full-value-text", entity))
                        .test_support()
                        .aria_value(text.clone())
                        .flex_1()
                        .min_h_0()
                        .overflow_scroll()
                        .px_2()
                        .pb_2()
                        .font_family("Menlo")
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

fn value_editable(node: &projection::JsonNode) -> bool {
    matches!(
        node.kind,
        JsonNodeKind::String | JsonNodeKind::Number | JsonNodeKind::Boolean | JsonNodeKind::Null
    ) && !node.range.is_empty()
}

fn default_expansion(projection: &JsonProjection, expanded: &mut [bool]) {
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
        }
    }
}

fn source_body_for(source: &Entity<SourceEditor>) -> AnyElement {
    let source = source.clone();
    let measure = source.clone();
    div()
        .on_children_prepainted(move |bounds, _, cx| {
            if let Some(bounds) = bounds.first() {
                measure.update(cx, |source, cx| source.set_scroll_viewport(*bounds, cx));
            }
        })
        .flex_1()
        .min_w_0()
        .min_h_0()
        .child(v_flex().size_full().min_h_0().child(source))
        .into_any_element()
}

fn structural_error(
    title: SharedString,
    detail: SharedString,
    cx: &mut Context<JsonView>,
) -> impl IntoElement {
    let (foreground, muted, warning) = {
        let theme = cx.theme();
        (theme.foreground, theme.muted_foreground, theme.warning)
    };
    v_flex()
        .id(("json-structure-error-body", cx.entity_id()))
        .test_support()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .items_center()
        .justify_center()
        .gap_2()
        .px_4()
        .child(
            Icon::new(IconName::TriangleAlert)
                .large()
                .text_color(warning),
        )
        .child(div().text_sm().text_color(foreground).child(title))
        .child(
            div()
                .max_w(rems(34.))
                .text_xs()
                .text_color(muted)
                .child(detail),
        )
}

fn loading_structure(cx: &mut Context<JsonView>) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    v_flex()
        .id(("json-structure-loading", cx.entity_id()))
        .flex_1()
        .min_w_0()
        .min_h_0()
        .items_center()
        .justify_center()
        .gap_2()
        .text_xs()
        .text_color(muted)
        .child(Spinner::new().small())
        .child(t(L10nKey::EditorJsonParsing))
}

impl Focusable for JsonView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.mode {
            JsonMode::Structure => self.focus.clone(),
            JsonMode::Source => self.source.read(cx).focus_handle(cx),
        }
    }
}

impl Render for JsonView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.mode;
        let focus = self.focus.clone();
        let item = v_flex()
            .id(("json-document", cx.entity_id()))
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(self.toolbar(cx));
        let body = match mode {
            JsonMode::Structure => self.structure_body(cx),
            JsonMode::Source => self.source_body(),
        };
        let item = item.child(body);
        match mode {
            JsonMode::Structure => {
                item.key_context(KEY_CONTEXT)
                    .track_focus(&focus)
                    .on_action(cx.listener(|this, _: &Undo, _, cx| this.undo_structure(cx)))
                    .on_action(cx.listener(|this, _: &Redo, _, cx| this.redo_structure(cx)))
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
            JsonMode::Source => item.into_any_element(),
        }
    }
}

/// 右侧菜单的 JSON 项：默认只给结构与源码切换；结构和源码切换之外的编辑
/// 命令仍由普通文本菜单提供。
pub(super) fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
) -> gpui_kit::component::menu::PopupMenu {
    menu.separator().menu(
        t(L10nKey::EditorJsonToggleStructure),
        Box::new(crate::core::actions::ToggleDocumentPreview),
    )
}

/// 解析的确定性测试门闩：只编译进测试与 native 测试引擎，不改变产品路径。
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
#[path = "json_document/tests.rs"]
mod tests;
