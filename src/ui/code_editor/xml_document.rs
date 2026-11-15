//! XML 文档：一份共享逐行保真源码，加上按 revision 派生的有序结构视图。
//!
//! 源码始终是同一 [`lyrel_markdown_editor::Document`] 的 plain-source
//! presentation，保存、撤销、reload 与关闭都走既有工作台生命周期。结构节点
//! 只保存源码范围与显示文本，不复制第二份可变内容，也不把 XML 转成通用对象。
//!
//! 结构解析在 UI 线程外进行：输入有界、每份文档单飞、只安装与当前
//! revision／generation 匹配的结果；解析失败、DTD 拒绝或超预算时源码仍然
//! 可编辑。元素、属性、文本、CDATA、注释与 PI 按源码顺序显示；混合内容不会
//! 被压成一个值。展开、选择、源码定位、完整值与编辑面板属于每个 presentation；
//! 文件自己只持有 Document 和最新的投影。

use std::ops::Range;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui_kit::ScrollStrategy;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Redo, Undo};
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

use mutation::{Mutation, MutationError, NodeTarget};
use projection::{ProjectionFailure, XML_INPUT_BYTES, XML_NODES, XmlNodeKind, XmlProjection};

/// 合并连续输入；每份文档最多一个 worker 与一次在途解析。
const PARSE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(120);
/// 键盘 PageUp／PageDown 跨过的可见行数。
const PAGE_ROWS: isize = 10;
/// 结构预览持有键盘时成立的上下文；源码面不声明它，文字编辑键不会被截走。
pub(crate) const KEY_CONTEXT: &str = "XmlStructure";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum XmlMode {
    Structure,
    Source,
}

impl XmlMode {
    pub(crate) fn opposite(self) -> Self {
        match self {
            Self::Structure => Self::Source,
            Self::Source => Self::Structure,
        }
    }
}

/// 一份 XML 文件的内容：共享 Document 与按最新 revision 安装的投影。
pub(crate) struct XmlDocument {
    document: Entity<Document>,
    projection: Option<Arc<XmlProjection>>,
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

impl Drop for XmlDocument {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl XmlDocument {
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

    pub(crate) fn projection(&self) -> Option<&XmlProjection> {
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
    AddAttribute,
    AddChild,
    AddText,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct EditCaps {
    value: bool,
    rename: bool,
    add_attribute: bool,
    add_child: bool,
    add_text: bool,
    remove: bool,
}

/// 一次明确的树命令：目标 occurrence 在打开面板时固定，提交时按当前
/// revision 重新解析；面板只是输入控件，不持有文档。
struct EditSession {
    panel: Entity<StructureEditPanel>,
    kind: EditKind,
    target: Option<NodeTarget>,
    /// The document revision when the panel opened. A target alone cannot
    /// detect a same-length edit elsewhere; the revision is the guard.
    revision: u64,
    _events: Subscription,
}

/// 一份 XML 文件在一个分组里的呈现：模式、源码选择、结构展开与滚动。
pub(crate) struct XmlView {
    document: Entity<XmlDocument>,
    source: Entity<SourceEditor>,
    mode: XmlMode,
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

impl XmlView {
    pub(super) fn new(
        document: Entity<XmlDocument>,
        mode: XmlMode,
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

    pub(crate) fn document(&self) -> &Entity<XmlDocument> {
        &self.document
    }

    pub(crate) fn source(&self) -> &Entity<SourceEditor> {
        &self.source
    }

    pub(crate) fn mode(&self) -> XmlMode {
        self.mode
    }

    pub(crate) fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.mode {
            XmlMode::Source => self.source.read(cx).focus_handle(cx),
            XmlMode::Structure => self.focus.clone(),
        }
    }

    pub(crate) fn scroll_offset(&self, cx: &App) -> gpui_kit::Point<gpui_kit::Pixels> {
        match self.mode {
            XmlMode::Source => self.source.read(cx).scroll_offset(),
            XmlMode::Structure => self.structure_scroll_offset(),
        }
    }

    pub(crate) fn set_scroll_offset(
        &mut self,
        offset: gpui_kit::Point<gpui_kit::Pixels>,
        cx: &mut Context<Self>,
    ) {
        match self.mode {
            XmlMode::Source => self
                .source
                .update(cx, |source, cx| source.set_scroll_offset(offset, cx)),
            XmlMode::Structure => {
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

    /// 测试消费者把键盘放进真实值控件（单行 `Input` 或多行 `Textarea`）。
    #[cfg(any(test, feature = "native-markdown-tests"))]
    pub(crate) fn focus_edit_value(&self, window: &mut Window, cx: &mut App) {
        if let Some(session) = &self.editing {
            session
                .panel
                .update(cx, |panel, cx| panel.focus_value(window, cx));
        }
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

    pub(crate) fn element_count(&self, cx: &App) -> Option<usize> {
        self.document
            .read(cx)
            .projection()
            .map(XmlProjection::element_count)
    }

    pub(crate) fn attribute_count(&self, cx: &App) -> Option<usize> {
        self.document
            .read(cx)
            .projection()
            .map(XmlProjection::attribute_count)
    }

    /// 预算失败、DTD 拒绝或整份 XML 无法解析时的可见说明。
    pub(crate) fn fatal_error(&self, cx: &App) -> Option<(SharedString, SharedString)> {
        let document = self.document.read(cx);
        if let Some(failure) = document.failure() {
            let message = match failure {
                ProjectionFailure::Cancelled => return None,
                ProjectionFailure::InputTooLarge => t_fmt(
                    L10nKey::EditorXmlInputLimit,
                    &[("mib", &(XML_INPUT_BYTES / (1024 * 1024)).to_string())],
                )
                .into(),
                ProjectionFailure::TooManyNodes => t_fmt(
                    L10nKey::EditorXmlNodeLimit,
                    &[("count", &XML_NODES.to_string())],
                )
                .into(),
                ProjectionFailure::PathsTooLarge => t_fmt(
                    L10nKey::EditorXmlPathLimit,
                    &[(
                        "mib",
                        &(projection::XML_PATH_BYTES / (1024 * 1024)).to_string(),
                    )],
                )
                .into(),
                ProjectionFailure::DtdDenied { line, column } => t_fmt(
                    L10nKey::EditorXmlDtdDeniedDetail,
                    &[("line", &line.to_string()), ("column", &column.to_string())],
                )
                .into(),
            };
            let title = match failure {
                ProjectionFailure::DtdDenied { .. } => t(L10nKey::EditorXmlDtdDenied),
                _ => t(L10nKey::EditorXmlStructureUnavailable),
            };
            return Some((title.into(), message));
        }
        let projection = document.projection()?;
        let error = projection.error()?;
        Some((
            t(L10nKey::EditorXmlInvalid).into(),
            t_fmt(
                L10nKey::EditorXmlInvalidDetail,
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
            .is_some_and(|projection| projection.root().is_some() && projection.error().is_none())
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
        if self.mode != XmlMode::Source {
            self.mode = XmlMode::Source;
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
        self.mode = XmlMode::Source;
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
            .filter(|node| node.has_full_value())
            .map(|node| (SharedString::from(node.path.clone()), node.range.clone()));
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

    /// 当前已安装的投影，且它的 revision 必须仍等于源码权威、没有解析
    /// 错误。任何不一致都按过期处理，不允许用旧范围规划写入。
    fn current_projection(&self, cx: &App) -> Option<Arc<XmlProjection>> {
        let document = self.document.read(cx);
        let projection = document.projection.clone()?;
        let installed = document.installed_revision()?;
        let source = document.document().clone();
        (projection.error().is_none() && source.read(cx).revision() == installed)
            .then_some(projection)
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
        let Some(index) = self.selected else {
            return caps;
        };
        let Some(node) = projection.nodes().get(index) else {
            return caps;
        };
        caps.value = node.kind.is_value();
        caps.rename = matches!(node.kind, XmlNodeKind::Element | XmlNodeKind::Attribute);
        caps.add_attribute = node.kind == XmlNodeKind::Element;
        caps.add_child = node.kind == XmlNodeKind::Element;
        caps.add_text = node.kind == XmlNodeKind::Element;
        caps.remove = match node.kind {
            XmlNodeKind::Element => !node.is_element_root(),
            XmlNodeKind::Attribute
            | XmlNodeKind::Text
            | XmlNodeKind::Cdata
            | XmlNodeKind::Comment => true,
            _ => false,
        };
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
        if !node.kind.is_value() {
            return;
        }
        let Some(text) = self.current_text(cx) else {
            return;
        };
        let Some(raw) = node.value_range.clone().and_then(|range| text.get(range)) else {
            return;
        };
        let target = NodeTarget::of(node);
        let (title_key, label, multiline) = match node.kind {
            XmlNodeKind::Attribute => (
                L10nKey::EditorXmlEditAttributeValueTitle,
                L10nKey::EditorXmlAttributeValue,
                false,
            ),
            XmlNodeKind::Text => (
                L10nKey::EditorXmlEditTextTitle,
                L10nKey::EditorXmlTextValue,
                true,
            ),
            XmlNodeKind::Cdata => (
                L10nKey::EditorXmlEditCdataTitle,
                L10nKey::EditorXmlCdataValue,
                true,
            ),
            XmlNodeKind::Comment => (
                L10nKey::EditorXmlEditCommentTitle,
                L10nKey::EditorXmlCommentValue,
                true,
            ),
            _ => return,
        };
        let title = t_fmt(title_key, &[("path", &node.path)]).into();
        let notice = node
            .is_namespace
            .then(|| SharedString::from(t(L10nKey::EditorXmlNamespaceNotice)));
        self.open_edit(
            window,
            cx,
            EditKind::Value,
            Some(target),
            title,
            notice,
            None,
            EditField::new(t(label), t(L10nKey::EditorXmlValuePlaceholder), raw),
            multiline,
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
        if !matches!(node.kind, XmlNodeKind::Element | XmlNodeKind::Attribute) {
            return;
        }
        let target = NodeTarget::of(node);
        let title = t_fmt(L10nKey::EditorXmlRenameTitle, &[("path", &node.path)]).into();
        let notice =
            (node.is_namespace).then(|| SharedString::from(t(L10nKey::EditorXmlNamespaceNotice)));
        self.open_edit(
            window,
            cx,
            EditKind::Rename,
            Some(target),
            title,
            notice,
            None,
            EditField::new(
                t(L10nKey::EditorXmlName),
                t(L10nKey::EditorXmlNamePlaceholder),
                node.key.clone(),
            ),
            false,
        );
    }

    fn start_add_attribute(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((target, path)) = self.selected_element(cx) else {
            return;
        };
        let title = t_fmt(L10nKey::EditorXmlAddAttributeTitle, &[("path", &path)]).into();
        self.open_edit(
            window,
            cx,
            EditKind::AddAttribute,
            Some(target),
            title,
            None,
            Some(EditField::new(
                t(L10nKey::EditorXmlName),
                t(L10nKey::EditorXmlNamePlaceholder),
                "",
            )),
            EditField::new(
                t(L10nKey::EditorXmlAttributeValue),
                t(L10nKey::EditorXmlValuePlaceholder),
                "",
            ),
            false,
        );
    }

    fn start_add_child(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((target, path)) = self.selected_element(cx) else {
            return;
        };
        let title = t_fmt(L10nKey::EditorXmlAddChildTitle, &[("path", &path)]).into();
        self.open_edit(
            window,
            cx,
            EditKind::AddChild,
            Some(target),
            title,
            None,
            None,
            EditField::new(
                t(L10nKey::EditorXmlName),
                t(L10nKey::EditorXmlNamePlaceholder),
                "",
            ),
            false,
        );
    }

    fn start_add_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((target, path)) = self.selected_element(cx) else {
            return;
        };
        let title = t_fmt(L10nKey::EditorXmlAddTextTitle, &[("path", &path)]).into();
        self.open_edit(
            window,
            cx,
            EditKind::AddText,
            Some(target),
            title,
            None,
            None,
            EditField::new(
                t(L10nKey::EditorXmlTextValue),
                t(L10nKey::EditorXmlTextPlaceholder),
                "",
            ),
            true,
        );
    }

    fn selected_element(&self, cx: &App) -> Option<(NodeTarget, String)> {
        let projection = self.current_projection(cx)?;
        let node = projection.nodes().get(self.selected?)?;
        (node.kind == XmlNodeKind::Element).then(|| (NodeTarget::of(node), node.path.clone()))
    }

    fn remove_selected(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.selected_target(cx) else {
            return;
        };
        match self.apply_mutation(Mutation::Remove { target }, None, cx) {
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

    #[allow(clippy::too_many_arguments)]
    fn open_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        kind: EditKind,
        target: Option<NodeTarget>,
        title: SharedString,
        notice: Option<SharedString>,
        key: Option<EditField>,
        value: EditField,
        multiline_value: bool,
    ) {
        self.close_edit(None, cx);
        self.full_value = None;
        self.mutation_error = None;
        let owner = cx.entity_id();
        let panel = cx.new(|cx| {
            StructureEditPanel::new(owner, window, cx, title, key, value, multiline_value)
        });
        if let Some(notice) = notice {
            panel.update(cx, |panel, cx| panel.set_notice(Some(notice), cx));
        }
        let revision = self.document.read(cx).document().read(cx).revision();
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
        let message: SharedString = t(L10nKey::EditorXmlEditStale).into();
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
        let opened_revision = session.revision;
        let mutation = match (kind, target) {
            (EditKind::Value, Some(target)) => Mutation::SetValue {
                target,
                value: panel.read(cx).value_text(cx),
            },
            (EditKind::Rename, Some(target)) => Mutation::Rename {
                target,
                name: panel.read(cx).value_text(cx),
            },
            (EditKind::AddAttribute, Some(target)) => Mutation::AddAttribute {
                element: target,
                name: panel.read(cx).key_text(cx).unwrap_or_default(),
                value: panel.read(cx).value_text(cx),
            },
            (EditKind::AddChild, Some(target)) => Mutation::AddChild {
                parent: target,
                name: panel.read(cx).value_text(cx),
            },
            (EditKind::AddText, Some(target)) => Mutation::AddText {
                parent: target,
                text: panel.read(cx).value_text(cx),
            },
            (
                EditKind::Value
                | EditKind::Rename
                | EditKind::AddAttribute
                | EditKind::AddChild
                | EditKind::AddText,
                None,
            ) => {
                let message = t(L10nKey::EditorXmlEditUnsupported).into();
                panel.update(cx, |panel, cx| panel.set_error(Some(message), cx));
                return;
            }
        };
        match self.apply_mutation(mutation, Some(opened_revision), cx) {
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
        opened_revision: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Result<(), MutationError> {
        let (projection, installed, editor, revision, text) = {
            let xml_document = self.document.read(cx);
            let document = xml_document.document().clone();
            let editor = self.source.clone();
            let projection = xml_document.projection.clone();
            let installed = xml_document.installed_revision();
            let revision = document.read(cx).revision();
            let text = document.read(cx).snapshot().shared_text();
            (projection, installed, editor, revision, text)
        };
        let projection = projection.ok_or(MutationError::Stale)?;
        if installed != Some(revision)
            || projection.error().is_some()
            || opened_revision.is_some_and(|opened| opened != revision)
        {
            return Err(MutationError::Stale);
        }
        let replacements = mutation::plan(text.as_str(), &projection, &mutation)?;
        if !replacements.is_empty() {
            editor.update(cx, |editor, cx| editor.replace_ranges(replacements, cx));
        }
        Ok(())
    }

    fn mutation_message(&self, error: MutationError) -> SharedString {
        match error {
            MutationError::Stale => t(L10nKey::EditorXmlEditStale).into(),
            MutationError::Unsupported => t(L10nKey::EditorXmlEditUnsupported).into(),
            MutationError::InvalidName(message) => {
                t_fmt(L10nKey::EditorXmlEditInvalidName, &[("message", &message)]).into()
            }
            MutationError::InvalidValue(message) => {
                t_fmt(L10nKey::EditorXmlEditInvalidValue, &[("message", &message)]).into()
            }
            MutationError::InvalidCdata => t(L10nKey::EditorXmlEditInvalidCdata).into(),
            MutationError::InvalidComment => t(L10nKey::EditorXmlEditInvalidComment).into(),
            MutationError::ResultInvalid {
                line,
                column,
                message,
            } => t_fmt(
                L10nKey::EditorXmlEditResultInvalid,
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
        let node_count = projection.nodes().len();
        let first_install = self.expanded.len() != node_count;
        if first_install {
            self.expanded = vec![false; node_count];
            if let Some(root) = projection.root()
                && root < self.expanded.len()
            {
                self.expanded[root] = true;
                for &child in projection.children(root) {
                    if projection.nodes()[child].kind == XmlNodeKind::Element {
                        self.expanded[child] = true;
                        break;
                    }
                }
            }
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
                    .any(|node| node.path == detail.path && node.range == detail.range)
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
    /// 可写内容或第二套 history。
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
                    let expandable =
                        projection.is_some_and(|projection| projection.is_expandable(selected));
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
        let parsing = self.is_parsing(cx);
        let fatal = self.fatal_error(cx);
        let counts = match (self.element_count(cx), self.attribute_count(cx)) {
            (Some(elements), Some(attributes)) => Some(t_fmt(
                L10nKey::EditorXmlFacts,
                &[
                    ("elements", &elements.to_string()),
                    ("attributes", &attributes.to_string()),
                ],
            )),
            _ => None,
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
        let has_full_value = self
            .document
            .read(cx)
            .projection()
            .and_then(|projection| projection.nodes().get(self.selected?))
            .is_some_and(|node| node.has_full_value());
        let caps = self.edit_caps(cx);
        let editable = self.structure_editable(cx);
        let mutation_error = self.mutation_error.clone();
        let mode = self.mode;
        let (toggle, tip) = match mode {
            XmlMode::Structure => (t(L10nKey::EditorXmlSource), t(L10nKey::EditorXmlShowSource)),
            XmlMode::Source => (
                t(L10nKey::EditorXmlStructure),
                t(L10nKey::EditorXmlShowStructure),
            ),
        };
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let warning = cx.theme().warning;
        h_flex()
            .id(("xml-toolbar", entity))
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
            .child(div().flex_none().text_xs().text_color(muted).child("XML"))
            .when_some(counts, |row, counts| {
                row.child(
                    div()
                        .id(("xml-count", entity))
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
                        .id(("xml-parsing", entity))
                        .test_support()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .text_color(muted)
                        .child(Spinner::new().xsmall())
                        .child(t(L10nKey::EditorXmlParsing)),
                )
            })
            .when_some(fatal, |row, (title, _)| {
                row.child(
                    div()
                        .id(("xml-structure-error", entity))
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
                        .id(("xml-location", entity))
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
                        .id(("xml-mutation-error", entity))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(warning)
                        .aria_value(error.clone())
                        .child(error),
                )
            })
            .when(editable, |row| {
                row.child(
                    Button::new(("xml-edit-value", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.value)
                        .label(t(L10nKey::EditorXmlEditValue))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_value_edit(window, cx)),
                        ),
                )
                .child(
                    Button::new(("xml-rename", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.rename)
                        .label(t(L10nKey::EditorXmlRename))
                        .on_click(cx.listener(|this, _, window, cx| this.start_rename(window, cx))),
                )
                .child(
                    Button::new(("xml-add-attribute", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.add_attribute)
                        .label(t(L10nKey::EditorXmlAddAttribute))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_add_attribute(window, cx)),
                        ),
                )
                .child(
                    Button::new(("xml-add-child", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.add_child)
                        .label(t(L10nKey::EditorXmlAddChild))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_add_child(window, cx)),
                        ),
                )
                .child(
                    Button::new(("xml-add-text", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.add_text)
                        .label(t(L10nKey::EditorXmlAddText))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_add_text(window, cx)),
                        ),
                )
                .child(
                    Button::new(("xml-remove", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!caps.remove)
                        .label(t(L10nKey::EditorXmlRemove))
                        .on_click(cx.listener(|this, _, _, cx| this.remove_selected(cx))),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new(("xml-reveal", entity))
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .disabled(!has_detail)
                    .label(t(L10nKey::EditorXmlReveal))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.reveal_selected_source(window, cx)),
                    ),
            )
            .child(
                Button::new(("xml-full-value-button", entity))
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .disabled(!has_full_value)
                    .label(t(L10nKey::EditorXmlFullValue))
                    .on_click(cx.listener(|this, _, _, cx| this.open_selected_full_value(cx))),
            )
            .child(
                Button::new(("xml-mode-toggle", entity))
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
                    ("xml-structure-error-body", cx.entity_id()),
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
                    ("xml-structure-loading", cx.entity_id()),
                    t(L10nKey::EditorXmlParsing).into(),
                    cx,
                )
            } else {
                structure_error(
                    ("xml-structure-error-body", cx.entity_id()),
                    t(L10nKey::EditorXmlStructureUnavailable).into(),
                    t(L10nKey::EditorXmlSourceStaysEditable).into(),
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
            ("xml-structure", entity),
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
                        let value = node.value.clone();
                        let key = node.key.clone();
                        let kind = node.kind;
                        let has_full_value = node.has_full_value();
                        let color = match kind {
                            XmlNodeKind::Attribute | XmlNodeKind::Text | XmlNodeKind::Cdata => {
                                cx.theme().green
                            }
                            XmlNodeKind::Element => cx.theme().foreground,
                            XmlNodeKind::Comment
                            | XmlNodeKind::ProcessingInstruction
                            | XmlNodeKind::Declaration => cx.theme().muted_foreground,
                            XmlNodeKind::Document => cx.theme().muted_foreground,
                        };
                        let badge = match kind {
                            XmlNodeKind::Element => node.namespace.as_deref().map(|uri| {
                                SharedString::from(structure_view::truncated_chars(uri, 48))
                            }),
                            XmlNodeKind::Attribute if node.is_namespace => {
                                Some(SharedString::from("xmlns"))
                            }
                            XmlNodeKind::Attribute => node.namespace.as_deref().map(|uri| {
                                SharedString::from(structure_view::truncated_chars(uri, 48))
                            }),
                            XmlNodeKind::Cdata => Some(SharedString::from("CDATA")),
                            XmlNodeKind::Comment => Some(SharedString::from("comment")),
                            XmlNodeKind::ProcessingInstruction => Some(SharedString::from("PI")),
                            XmlNodeKind::Declaration => Some(SharedString::from("declaration")),
                            XmlNodeKind::Document => None,
                            XmlNodeKind::Text => None,
                        };
                        let index = row.node;
                        let id = SharedString::from(format!("xml-row-{}-{index}", entity.as_u64()));
                        structure_row(
                            StructureRowData {
                                id,
                                depth: row.depth,
                                expandable,
                                expanded,
                                selected,
                                striped: row_index % 2 == 1,
                                key: SharedString::from(key),
                                value: SharedString::from(value),
                                color,
                                badge,
                                record_line: None,
                                error: None,
                                aria_label: SharedString::from(path),
                            },
                            cx.listener(|view, _: &gpui_kit::MouseDownEvent, window, cx| {
                                view.focus.focus(window, cx);
                                cx.stop_propagation();
                            }),
                            cx.listener(move |view, event: &gpui_kit::ClickEvent, window, cx| {
                                view.focus.focus(window, cx);
                                if event.click_count() > 1 {
                                    view.select_node(index, cx);
                                    if has_full_value {
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
            .id(("xml-structure-body", entity))
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
                .id(("xml-full-value", entity))
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
                                .child(format!("{} · {path}", t(L10nKey::EditorXmlFullValue))),
                        )
                        .child(
                            Button::new(("xml-full-value-copy", entity))
                                .ghost()
                                .xsmall()
                                .flex_none()
                                .label(t(L10nKey::EditorXmlCopyValue))
                                .on_click(cx.listener(|this, _, _, cx| this.copy_full_value(cx))),
                        )
                        .child(
                            Button::new(("xml-full-value-close", entity))
                                .ghost()
                                .xsmall()
                                .flex_none()
                                .label(t(L10nKey::Close))
                                .on_click(cx.listener(|this, _, _, cx| this.close_full_value(cx))),
                        ),
                )
                .child(
                    div()
                        .id(("xml-full-value-text", entity))
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

impl Focusable for XmlView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.mode {
            XmlMode::Structure => self.focus.clone(),
            XmlMode::Source => self.source.read(cx).focus_handle(cx),
        }
    }
}

impl Render for XmlView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.mode;
        let focus = self.focus.clone();
        let item = v_flex()
            .id(("xml-document", cx.entity_id()))
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(self.toolbar(cx));
        let body = match mode {
            XmlMode::Structure => self.structure_body(cx),
            XmlMode::Source => self.source_body(),
        };
        let item = item.child(body);
        match mode {
            XmlMode::Structure => {
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
            XmlMode::Source => item.into_any_element(),
        }
    }
}

/// 右侧菜单的 XML 项：结构／源码切换；其余编辑命令仍由普通文本菜单提供。
pub(super) fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
) -> gpui_kit::component::menu::PopupMenu {
    menu.separator().menu(
        t(L10nKey::EditorXmlToggleStructure),
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
#[path = "xml_document/tests.rs"]
mod tests;
