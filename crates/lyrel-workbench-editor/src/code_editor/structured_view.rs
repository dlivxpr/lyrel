//! 共享结构化树视图：源码/结构模式、展开、键盘导航、滚动同步、源码定位、
//! 完整值面板与结构编辑会话。
//!
//! JSON／JSONC／JSONL 与 TOML 的 presentation 侧只有格式壳
//! （[`StructuredViewFormat`]）不同：行配色与徽标、编辑命令集、mutation 规划、
//! 标签与元素 id 前缀。共享代码持有 Document 订阅、源码 presentation、选择/
//! 展开/滚动状态与 [`StructureEditPanel`] 会话；格式壳不复制这些。后续的
//! YAML／XML 迁移（#340）只需实现同一格式壳：徽标、面板提示、多行值、
//! focus_edit_value、自定义计数与自动展开策略都有对应 seam。

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::ScrollStrategy;
use gpui_kit::component::Theme;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Redo, Undo};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, h_flex, spinner::Spinner, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, Entity, FocusHandle, Focusable, Hsla, KeyDownEvent,
    MouseButton, Render, SharedString, Subscription, TestSupportExt as _, UniformListScrollHandle,
    Window, div, rems, uniform_list,
};
use lyrel_markdown_editor::{DocumentEvent, EditorState as SourceEditor};

use lyrel_document::tree::{self, SourceLocation, Treelike, VisibleRow};
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

use super::structure_edit::{EditField, StructureEditEvent, StructureEditPanel};
use super::structure_rows::{
    StructureRowData, source_body_for, structure_error, structure_loading, structure_row,
};
use super::structured_document::{StructuredDocument, StructuredDocumentFormat};

/// 打开时自动展开的直接子容器上限，复现上游的浅层可读树。
pub(crate) const AUTO_EXPAND_CHILDREN: usize = 8;
/// 键盘 PageUp／PageDown 跨过的可见行数。
const PAGE_ROWS: isize = 10;

/// 一份结构化文件在一个分组里的呈现模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StructuredMode {
    Structure,
    Source,
}

impl StructuredMode {
    pub fn opposite(self) -> Self {
        match self {
            Self::Structure => Self::Source,
            Self::Source => Self::Structure,
        }
    }
}

/// 一个节点的选择/详情标识：路径、源码范围、原始 detail 范围与它是否可用。
///
/// detail 是“选中该节点时在源码中定位的范围”：JSON 的属性取键到值末尾、
/// 记录取整行；TOML 直接用投影的 `detail`。`has_detail` 为假时详情不可用
/// （如 JSONL 的 Records 容器），不允许源码定位与完整值。路径借用投影，
/// revision 刷新时的选中匹配与完整值保留因此不分配字符串。
pub struct NodeIdentity<'a> {
    pub path: &'a str,
    pub range: Range<usize>,
    /// 原始 detail 范围；JSON 的 `detail_range()`／TOML 的 `detail`。
    pub detail: Range<usize>,
    /// 该节点能否作为源码定位与完整值的目标。
    pub has_detail: bool,
}

/// 一行可见结构节点的格式特定呈现数据。
pub struct NodeRow {
    pub key: SharedString,
    pub value: SharedString,
    /// 值的颜色由格式按节点种类决定；共享行不解释 kind。
    pub color: Hsla,
    /// 锚点、tag 或记录行的简短说明；无则为 None。
    pub badge: Option<SharedString>,
    pub record_line: Option<usize>,
    pub error: Option<SharedString>,
    pub aria_label: SharedString,
}

/// 工具栏一次树编辑命令的启用状态与触发方式。
pub struct EditButton<R> {
    /// 元素 id 后缀；共享代码加上 `"{前缀}-"` 作用域。
    pub id: &'static str,
    pub label: SharedString,
    pub enabled: bool,
    pub flow: EditFlow<R>,
}

/// 命令的触发方式：先开编辑面板再应用，或立即应用。
pub enum EditFlow<R> {
    Panel(R),
    Immediate(R),
}

impl<R: Clone> EditFlow<R> {
    /// 命令标签，无论走面板还是立即执行。
    pub fn request(&self) -> R {
        match self {
            Self::Panel(request) | Self::Immediate(request) => request.clone(),
        }
    }
}

/// 把命令展开的结果：面板描述、立即目标或源码 handoff。目标在打开面板时
/// 固定，提交时按 revision 复核。
pub enum EditPlan<T> {
    Panel(EditPanelDesc<T>),
    Immediate {
        target: T,
    },
    /// 格式壳要求的源码 handoff（如 YAML 的多行复杂键）：不猜测重排，把节点
    /// 的精确源码范围交给 Source 编辑，并给出一条说明。
    Handoff {
        range: Range<usize>,
        notice: SharedString,
    },
}

/// 打开编辑面板所需的一切：标题、字段、提示、多行值与固定目标。
pub struct EditPanelDesc<T> {
    pub title: SharedString,
    pub key: Option<EditField>,
    pub value: EditField,
    pub target: T,
    /// 面板里的提示（如命名空间、锚点影响）；无则为 None。
    pub notice: Option<SharedString>,
    /// 值字段是否多行（块标量、文本节点等）。
    pub multiline: bool,
}

/// 共享修改规划的失败：revision 漂移或格式引擎拒绝。任何失败都发生在
/// 写入之前。
#[derive(Debug)]
pub enum StructuredMutationError<E> {
    /// 源码 revision 已变化，或投影已不可用；旧范围绝不写入。
    Stale,
    /// 格式 mutation 引擎拒绝了这个命令。
    Plan(E),
}

/// revision 刷新后的选中恢复策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionRestore {
    /// 只接受 path+range 同时匹配；否则保持原索引（若仍有效）。TOML 的
    /// occurrence 在值被编辑后不允许仍按路径跟随。
    Exact,
    /// 先精确匹配，失败后回退到同路径节点；JSON 的 occurrence 语义。
    PathFallback,
}

/// 结构化树视图的格式壳：只提供格式特定的行呈现、编辑命令、mutation 规划
/// 与标签。模式/选择/展开/滚动/面板会话都属于共享视图；格式壳不持有状态。
pub trait StructuredViewFormat: StructuredDocumentFormat {
    /// 工具栏树命令的不透明标签（如 JSON 的 Value/Key/AddMember/AddRecord）。
    type EditRequest: Clone + 'static;
    /// 格式自己的 mutation 模型（如 `lyrel_document::json::mutation::Mutation`）。
    type Mutation: 'static;
    /// 格式 mutation 引擎的错误。
    type MutationError: 'static;
    /// 打开编辑面板时固定的目标 occurrence。
    type Target: Clone + 'static;

    /// 元素 id 前缀：json／toml／yaml／xml。
    fn element_prefix(&self) -> &'static str;
    /// 结构预览持有键盘时成立的上下文；源码面不声明它，文字编辑键不会被截走。
    const KEY_CONTEXT: &'static str;
    /// 结构模式是否注册共享 history 的撤销/重做动作（JSON 有，TOML 无）。
    const STRUCTURE_UNDO_REDO: bool;
    /// 是否提供完整值面板与双击打开完整值；YAML 为假——块标量与复杂键的
    /// 保真靠双击直接 reveal 源码，工具栏也不渲染完整值按钮。
    const FULL_VALUE: bool = true;

    /// 工具栏最左侧的格式标签。
    fn format_label(&self) -> SharedString;
    /// 源码/结构切换按钮与状态栏的 (按钮文案, 提示)。
    fn mode_labels(&self, mode: StructuredMode) -> (SharedString, SharedString);

    /// 投影节点总数：展开位图与可见行按它分配。
    fn node_count(&self, projection: &Self::Projection) -> usize;
    /// 状态栏/工具栏的结构节点计数；按记录计数的格式（JSONL）返回 None。
    fn display_node_count(&self, projection: &Self::Projection) -> Option<usize>;
    /// 逐行记录计数；无此概念的格式返回 None。
    fn record_count(&self, projection: &Self::Projection) -> Option<usize> {
        let _ = projection;
        None
    }
    /// 无效记录行数；无此概念的格式返回 0。
    fn invalid_records(&self, projection: &Self::Projection) -> usize {
        let _ = projection;
        0
    }
    /// 工具栏计数文案；无计数时返回 None。
    fn count_notice(&self, projection: &Self::Projection) -> Option<SharedString> {
        let _ = projection;
        None
    }
    /// 工具栏主计数之前的次计数文案（如 YAML 的多文档计数）；无则为 None。
    fn documents_notice(&self, projection: &Self::Projection) -> Option<SharedString> {
        let _ = projection;
        None
    }
    /// 逐行无效记录的可见说明（含计数）；无此概念的格式返回 None。
    fn invalid_records_notice(&self, projection: &Self::Projection) -> Option<SharedString> {
        let _ = projection;
        None
    }

    /// 预算失败或整份文档无法解析时的可见 (标题, 详情)；JSONL 的逐行错误
    /// 不挡住其他记录，由行错误与计数单独显示。
    fn fatal_error(
        &self,
        failure: Option<&Self::Failure>,
        projection: Option<&Self::Projection>,
    ) -> Option<(SharedString, SharedString)>;

    /// 节点的选择/详情标识；路径借用投影。
    fn node_identity<'a>(
        &self,
        projection: &'a Self::Projection,
        index: usize,
    ) -> Option<NodeIdentity<'a>>;

    /// 该节点能否打开完整值面板；默认与 `has_detail` 相同。XML 收窄为投影的
    /// `has_full_value()`（属性／文本／CDATA／注释／PI／声明）；YAML 提供完整值
    /// 面板为假，双击直接 reveal 源码。
    fn node_has_full_value(&self, projection: &Self::Projection, index: usize) -> bool {
        self.node_identity(projection, index)
            .is_some_and(|identity| identity.has_detail)
    }

    /// 一行可见节点的格式特定呈现数据（配色、徽标、记录行与行错误）。
    fn node_row(
        &self,
        projection: &Self::Projection,
        index: usize,
        theme: &Theme,
    ) -> Option<NodeRow>;

    /// 首次安装投影时的自动展开策略。
    fn default_expansion(&self, projection: &Self::Projection, expanded: &mut [bool]);

    /// revision 刷新后的选中恢复策略。
    fn selection_restore(&self) -> SelectionRestore {
        SelectionRestore::PathFallback
    }

    /// 投影能否用于结构编辑与结构就绪的额外条件（revision 匹配之外）：
    /// XML 拒绝带解析错误的投影，其余格式恒真。
    fn projection_editable(&self, projection: &Self::Projection) -> bool {
        let _ = projection;
        true
    }

    /// 当前选择的工具栏编辑命令（含启用状态与触发方式）。
    fn edit_buttons(
        &self,
        projection: &Self::Projection,
        selected: Option<usize>,
    ) -> Vec<EditButton<Self::EditRequest>>;

    /// 把命令展开成面板描述或立即目标；读取当前文本预填字段。
    fn describe_edit(
        &self,
        request: &Self::EditRequest,
        projection: &Self::Projection,
        text: &str,
        selected: Option<usize>,
    ) -> Option<EditPlan<Self::Target>>;

    /// 应用编辑（或立即命令）时由面板文本构造 mutation；格式自己解释
    /// key/value 与固定目标。
    fn build_mutation(
        &self,
        request: &Self::EditRequest,
        target: &Self::Target,
        key: Option<String>,
        value: String,
    ) -> Result<Self::Mutation, Self::MutationError>;

    /// 命令成功应用后显示在工具栏的提示（如 YAML 的锚点编辑说明）。在写入
    /// 前用当前投影计算；写入失败或过期则不显示。默认无。
    fn applied_notice(
        &self,
        request: &Self::EditRequest,
        target: &Self::Target,
        projection: &Self::Projection,
    ) -> Option<SharedString> {
        let _ = (request, target, projection);
        None
    }

    /// 规划一次局部修改，返回按源码顺序、互不重叠的替换清单；空清单是
    /// no-op。格式壳把 mutation 交给 `lyrel-document` 里对应的无损引擎。
    fn plan_mutation(
        &self,
        text: &str,
        projection: &Self::Projection,
        mutation: &Self::Mutation,
    ) -> Result<Vec<(Range<usize>, String)>, Self::MutationError>;

    /// 把规划失败映射成用户可见的说明。
    fn mutation_message(&self, error: StructuredMutationError<Self::MutationError>)
    -> SharedString;

    /// 后台解析进行中的可见标签。
    fn loading_label(&self) -> SharedString;
    /// 结构不可用时的 (标题, 详情)；源码仍然可编辑。
    fn unavailable(&self) -> (SharedString, SharedString);
    /// 完整值面板标题前缀与工具栏按钮文案（同一 L10nKey）。
    fn full_value_label(&self) -> SharedString;
    /// 完整值复制按钮文案。
    fn copy_value_label(&self) -> SharedString;
    /// 源码定位按钮文案。
    fn reveal_label(&self) -> SharedString;
}

/// 一个 presentation 里的完整值面板：路径与源码范围；文本在绘制时从当前
/// 快照读取，源码任一次变化都会关闭它。
struct FullValue {
    path: SharedString,
    range: Range<usize>,
}

/// 一次明确的树命令：打开面板时固定目标 occurrence 与源码 revision，
/// 提交时两者都必须仍匹配。面板只是输入控件，不持有文档。
struct EditSession<F: StructuredViewFormat> {
    panel: Entity<StructureEditPanel>,
    request: F::EditRequest,
    target: F::Target,
    /// The source revision the panel was opened against. A same-length edit
    /// elsewhere does not change the target ranges, so identity alone is not
    /// enough to reject a stale command.
    revision: u64,
    _events: Subscription,
}

/// 一份结构化文件在一个分组里的呈现：模式、源码选择、结构展开与滚动。
pub struct StructuredView<F: StructuredViewFormat> {
    document: Entity<StructuredDocument<F>>,
    source: Entity<SourceEditor>,
    mode: StructuredMode,
    focus: FocusHandle,
    structure_scroll: UniformListScrollHandle,
    expanded: Vec<bool>,
    rows: Vec<VisibleRow>,
    selected: Option<usize>,
    selected_path: Option<String>,
    selected_range: Option<Range<usize>>,
    full_value: Option<FullValue>,
    editing: Option<EditSession<F>>,
    mutation_error: Option<SharedString>,
    /// 成功应用命令或源码 handoff 后的提示（如 YAML 的锚点编辑说明）。
    notice: Option<SharedString>,
    seen_revision: Option<u64>,
    _document: Subscription,
    _source_document: Subscription,
    _source: Subscription,
}

impl<F: StructuredViewFormat> StructuredView<F> {
    pub fn new(
        document: Entity<StructuredDocument<F>>,
        mode: StructuredMode,
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
            notice: None,
            seen_revision: None,
            _document: watch_document,
            _source_document: watch_source_document,
            _source: watch_source,
        };
        view.sync_structure(cx);
        view
    }

    pub fn document(&self) -> &Entity<StructuredDocument<F>> {
        &self.document
    }

    pub fn source(&self) -> &Entity<SourceEditor> {
        &self.source
    }

    pub fn mode(&self) -> StructuredMode {
        self.mode
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.mode {
            StructuredMode::Source => self.source.read(cx).focus_handle(cx),
            StructuredMode::Structure => self.focus.clone(),
        }
    }

    pub fn scroll_offset(&self, cx: &App) -> gpui_kit::Point<gpui_kit::Pixels> {
        match self.mode {
            StructuredMode::Source => self.source.read(cx).scroll_offset(),
            StructuredMode::Structure => self.structure_scroll_offset(),
        }
    }

    pub fn set_scroll_offset(
        &mut self,
        offset: gpui_kit::Point<gpui_kit::Pixels>,
        cx: &mut Context<Self>,
    ) {
        match self.mode {
            StructuredMode::Source => self
                .source
                .update(cx, |source, cx| source.set_scroll_offset(offset, cx)),
            StructuredMode::Structure => {
                self.structure_scroll
                    .0
                    .borrow()
                    .base_handle
                    .set_offset(offset);
                cx.notify();
            }
        }
    }

    pub fn source_scroll_offset(&self, cx: &App) -> gpui_kit::Point<gpui_kit::Pixels> {
        self.source.read(cx).scroll_offset()
    }

    pub fn structure_scroll_offset(&self) -> gpui_kit::Point<gpui_kit::Pixels> {
        self.structure_scroll.0.borrow().base_handle.offset()
    }

    pub fn selected_location(&self, cx: &App) -> Option<SourceLocation> {
        let range = self.selected_detail_range(cx)?;
        let snapshot = self.document.read(cx).document().read(cx).snapshot();
        Some(SourceLocation::of(snapshot.text(), range))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn selected_path(&self, cx: &App) -> Option<SharedString> {
        let document = self.document.read(cx);
        let projection = document.projection()?;
        document
            .format()
            .node_identity(projection, self.selected?)
            .map(|identity| SharedString::from(identity.path))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn edit_texts(&self, cx: &App) -> Option<(Option<String>, String)> {
        let session = self.editing.as_ref()?;
        Some((
            session.panel.read(cx).key_text(cx),
            session.panel.read(cx).value_text(cx),
        ))
    }

    /// 焦点交给打开面板的值字段；供格式命令（如测试与快捷键）使用。
    #[cfg(any(test, feature = "test-support"))]
    pub fn focus_edit_value(&self, window: &mut Window, cx: &mut App) {
        if let Some(session) = &self.editing {
            session
                .panel
                .update(cx, |panel, cx| panel.focus_value(window, cx));
        }
    }

    /// 成功应用命令或源码 handoff 后的提示（YAML 的 `mutation_notice`）。
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn notice(&self) -> Option<SharedString> {
        self.notice.clone()
    }

    pub fn full_value_text(&self, cx: &App) -> Option<SharedString> {
        let detail = self.full_value.as_ref()?;
        let snapshot = self.document.read(cx).document().read(cx).snapshot();
        snapshot
            .text()
            .get(detail.range.clone())
            .map(|text| SharedString::from(text.to_owned()))
    }

    pub fn full_value_path(&self) -> Option<SharedString> {
        self.full_value.as_ref().map(|detail| detail.path.clone())
    }

    pub fn is_parsing(&self, cx: &App) -> bool {
        self.document.read(cx).is_parsing()
    }

    pub fn node_count(&self, cx: &App) -> Option<usize> {
        let document = self.document.read(cx);
        document
            .projection()
            .and_then(|projection| document.format().display_node_count(projection))
    }

    pub fn record_count(&self, cx: &App) -> Option<usize> {
        let document = self.document.read(cx);
        document
            .projection()
            .and_then(|projection| document.format().record_count(projection))
    }

    pub fn invalid_records(&self, cx: &App) -> usize {
        let document = self.document.read(cx);
        document.projection().map_or(0, |projection| {
            document.format().invalid_records(projection)
        })
    }

    pub fn fatal_error(&self, cx: &App) -> Option<(SharedString, SharedString)> {
        let document = self.document.read(cx);
        document
            .format()
            .fatal_error(document.failure().as_ref(), document.projection())
    }

    pub fn structure_ready(&self, cx: &App) -> bool {
        let document = self.document.read(cx);
        document.projection().is_some_and(|projection| {
            document.format().projection_editable(projection) && projection.root().is_some()
        })
    }

    pub fn select_source_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.show_source(cx);
        self.source
            .update(cx, |source, cx| source.select_range(range, cx));
    }

    pub fn add_source_selection(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.show_source(cx);
        self.source
            .update(cx, |source, cx| source.add_selection(range, cx));
    }

    pub fn replace_source_range(
        &mut self,
        range: Range<usize>,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        self.show_source(cx);
        self.source
            .update(cx, |source, cx| source.replace_range(range, text, cx));
    }

    pub fn replace_source_ranges(
        &mut self,
        replacements: Vec<(Range<usize>, String)>,
        cx: &mut Context<Self>,
    ) {
        self.show_source(cx);
        self.source
            .update(cx, |source, cx| source.replace_ranges(replacements, cx));
    }

    pub fn replace_source_lines(
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

    pub fn set_source_history_selection(
        &mut self,
        before: Range<usize>,
        after: Range<usize>,
        cx: &mut Context<Self>,
    ) {
        self.source.update(cx, |source, cx| {
            source.set_history_selection(before, after, cx)
        });
    }

    pub fn seed_presentation(
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

    pub fn expansion_state(&self) -> (Vec<bool>, Option<usize>) {
        (self.expanded.clone(), self.selected)
    }

    pub fn finish_composition(&mut self, cx: &mut Context<Self>) {
        self.source
            .update(cx, |source, cx| source.finish_owned_composition(cx));
    }

    pub fn set_soft_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        self.source
            .update(cx, |source, cx| source.set_soft_wrap(wrap, cx));
    }

    pub fn set_format(&mut self, format: lyrel_document::text::TextFormat, cx: &mut Context<Self>) {
        self.source
            .update(cx, |source, cx| source.set_format(format, cx));
    }

    pub fn show_source(&mut self, cx: &mut Context<Self>) {
        self.close_edit(None, cx);
        if self.mode != StructuredMode::Source {
            self.mode = StructuredMode::Source;
            cx.notify();
        }
    }

    pub fn toggle_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_composition(cx);
        self.close_edit(None, cx);
        self.mode = self.mode.opposite();
        let focus = self.focus_handle(cx);
        focus.focus(window, cx);
        cx.notify();
    }

    pub fn reveal_selected_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_edit(None, cx);
        let Some(range) = self.selected_detail_range(cx) else {
            return;
        };
        self.finish_composition(cx);
        self.mode = StructuredMode::Source;
        self.source
            .update(cx, |source, cx| source.select_range(range, cx));
        let focus = self.source.read(cx).focus_handle(cx);
        focus.focus(window, cx);
        cx.notify();
    }

    pub fn open_selected_full_value(&mut self, cx: &mut Context<Self>) {
        self.close_edit(None, cx);
        let Some(index) = self.selected else {
            return;
        };
        let document = self.document.read(cx);
        let detail = document
            .projection()
            .filter(|projection| document.format().node_has_full_value(projection, index))
            .and_then(|projection| document.format().node_identity(projection, index))
            .map(|identity| (SharedString::from(identity.path), identity.detail));
        self.full_value = detail.map(|(path, range)| FullValue { path, range });
        cx.notify();
    }

    pub fn close_full_value(&mut self, cx: &mut Context<Self>) {
        if self.full_value.take().is_some() {
            cx.notify();
        }
    }

    pub fn copy_full_value(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.full_value_text(cx) {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        }
    }

    /// 当前已安装的投影，且它的 revision 必须仍等于源码权威、格式壳仍认可它
    /// 可用于结构编辑。任何不一致都按过期处理，不允许用旧范围规划写入。
    fn current_projection(&self, cx: &App) -> Option<Arc<F::Projection>> {
        let document = self.document.read(cx);
        let projection = document.shared_projection()?;
        let installed = document.installed_revision()?;
        let source = document.document().clone();
        let format = document.format();
        (format.projection_editable(&projection) && source.read(cx).revision() == installed)
            .then_some(projection)
    }

    fn current_text(&self, cx: &App) -> Option<Arc<String>> {
        let source = self.document.read(cx).document().clone();
        Some(source.read(cx).snapshot().shared_text())
    }

    fn structure_editable(&self, cx: &App) -> bool {
        self.current_projection(cx).is_some()
    }
}

impl<F: StructuredViewFormat> StructuredView<F> {
    /// 工具栏树命令：只有当前投影可信（revision 匹配）时才允许执行；面板
    /// 描述或立即目标由格式壳给出，提交时再按 revision 复核。
    fn start_edit_command(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        request: F::EditRequest,
    ) {
        let Some(projection) = self.current_projection(cx) else {
            return;
        };
        let Some(text) = self.current_text(cx) else {
            return;
        };
        let format = self.document.read(cx).format().clone();
        let Some(plan) = format.describe_edit(&request, &projection, text.as_str(), self.selected)
        else {
            return;
        };
        match plan {
            EditPlan::Panel(desc) => self.open_edit(window, cx, request, desc),
            EditPlan::Handoff { range, notice } => {
                self.handoff_to_source(range, notice, window, cx)
            }
            EditPlan::Immediate { target } => {
                let notice = format.applied_notice(&request, &target, &projection);
                let result = format
                    .build_mutation(&request, &target, None, String::new())
                    .map_err(StructuredMutationError::Plan)
                    .and_then(|mutation| self.apply_mutation(mutation, cx));
                match result {
                    Ok(()) => {
                        self.mutation_error = None;
                        self.notice = notice;
                        cx.notify();
                    }
                    Err(error) => {
                        self.mutation_error = Some(format.mutation_message(error));
                        cx.notify();
                    }
                }
            }
        }
    }

    /// 格式壳明确要求的源码 handoff：切到 Source、选中节点的精确范围并给出
    /// 一条说明（如 YAML 多行复杂键的保真提示）。与 reveal 不同，它不是
    /// 导航，而是一次带解释的编辑移交，同一个 Document history/save 适用。
    fn handoff_to_source(
        &mut self,
        range: Range<usize>,
        notice: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_edit(None, cx);
        self.finish_composition(cx);
        self.mutation_error = None;
        self.notice = Some(notice);
        self.mode = StructuredMode::Source;
        self.source
            .update(cx, |source, cx| source.select_range(range, cx));
        let focus = self.source.read(cx).focus_handle(cx);
        focus.focus(window, cx);
        cx.notify();
    }

    fn open_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        request: F::EditRequest,
        desc: EditPanelDesc<F::Target>,
    ) {
        self.close_edit(None, cx);
        self.full_value = None;
        self.mutation_error = None;
        self.notice = None;
        let owner = cx.entity_id();
        let panel = cx.new(|cx| {
            StructureEditPanel::new(
                owner,
                window,
                cx,
                desc.title,
                desc.key,
                desc.value,
                desc.multiline,
            )
        });
        if let Some(notice) = desc.notice {
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
        let revision = self.document.read(cx).document().read(cx).revision();
        self.editing = Some(EditSession {
            panel,
            request,
            target: desc.target,
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
        let message = self
            .document
            .read(cx)
            .format()
            .mutation_message(StructuredMutationError::Stale);
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
        let request = session.request.clone();
        let target = session.target.clone();
        let key = panel.read(cx).key_text(cx);
        let value = panel.read(cx).value_text(cx);
        let format = self.document.read(cx).format().clone();
        // 提示在写入前用当前投影计算；写入失败或过期则不显示。
        let notice = self
            .document
            .read(cx)
            .projection()
            .and_then(|projection| format.applied_notice(&request, &target, projection));
        let mutation = match format.build_mutation(&request, &target, key, value) {
            Ok(mutation) => mutation,
            Err(error) => {
                let message = self
                    .document
                    .read(cx)
                    .format()
                    .mutation_message(StructuredMutationError::Plan(error));
                panel.update(cx, |panel, cx| panel.set_error(Some(message), cx));
                return;
            }
        };
        match self.apply_mutation(mutation, cx) {
            Ok(()) => {
                self.mutation_error = None;
                self.notice = notice;
                self.close_edit(Some(window), cx);
            }
            Err(error) => {
                let message = self.document.read(cx).format().mutation_message(error);
                panel.update(cx, |panel, cx| panel.set_error(Some(message), cx));
            }
        }
    }

    /// 一次命令一条 history：规划出的替换清单交给共享 Document 的正常
    /// `replace_ranges`，不另建可写内容。revision 不一致时拒绝写入。
    fn apply_mutation(
        &mut self,
        mutation: F::Mutation,
        cx: &mut Context<Self>,
    ) -> Result<(), StructuredMutationError<F::MutationError>> {
        let (projection, installed, editor, revision, text, format) = {
            let document = self.document.read(cx);
            let source = document.document().clone();
            let projection = document.shared_projection();
            let installed = document.installed_revision();
            let revision = source.read(cx).revision();
            let text = source.read(cx).snapshot().shared_text();
            let format = document.format().clone();
            (
                projection,
                installed,
                self.source.clone(),
                revision,
                text,
                format,
            )
        };
        let projection = projection.ok_or(StructuredMutationError::Stale)?;
        if installed != Some(revision) || !format.projection_editable(&projection) {
            return Err(StructuredMutationError::Stale);
        }
        let replacements = format
            .plan_mutation(text.as_str(), &projection, &mutation)
            .map_err(StructuredMutationError::Plan)?;
        if !replacements.is_empty() {
            editor.update(cx, |editor, cx| editor.replace_ranges(replacements, cx));
        }
        Ok(())
    }

    fn selected_detail_range(&self, cx: &App) -> Option<Range<usize>> {
        let document = self.document.read(cx);
        let projection = document.projection()?;
        let identity = document
            .format()
            .node_identity(projection, self.selected?)?;
        identity.has_detail.then(|| identity.detail)
    }

    fn sync_structure(&mut self, cx: &mut Context<Self>) {
        let (projection, revision) = {
            let document = self.document.read(cx);
            (document.shared_projection(), document.installed_revision())
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
        let format = self.document.read(cx).format().clone();
        let node_count = format.node_count(&projection);
        let first_install = self.expanded.len() != node_count;
        if first_install {
            self.expanded = vec![false; node_count];
            format.default_expansion(&projection, &mut self.expanded);
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
            self.selected = match (format.selection_restore(), path, range) {
                (restore, Some(path), Some(range)) => {
                    let exact = (0..node_count).find(|&index| {
                        format
                            .node_identity(&projection, index)
                            .is_some_and(|identity| {
                                identity.path == path && identity.range == range
                            })
                    });
                    match exact {
                        Some(index) => Some(index),
                        // The selected occurrence still exists but its own
                        // value changed; keep it by path when the format
                        // allows it. Stale write targets are never trusted:
                        // mutations re-resolve path+range.
                        None => match restore {
                            SelectionRestore::Exact => None,
                            SelectionRestore::PathFallback => (0..node_count).find(|&index| {
                                format
                                    .node_identity(&projection, index)
                                    .is_some_and(|identity| identity.path == path)
                            }),
                        },
                    }
                }
                (SelectionRestore::PathFallback, Some(path), None) => {
                    (0..node_count).find(|&index| {
                        format
                            .node_identity(&projection, index)
                            .is_some_and(|identity| identity.path == path)
                    })
                }
                _ => self.selected.filter(|index| *index < node_count),
            };
        } else if self.selected.is_some_and(|index| index >= node_count) {
            self.selected = None;
        }
        match self
            .selected
            .and_then(|index| format.node_identity(&projection, index))
        {
            Some(identity) => {
                self.selected_path = Some(identity.path.to_owned());
                self.selected_range = Some(identity.range);
            }
            None => {
                self.selected = None;
                self.selected_path = None;
                self.selected_range = None;
            }
        }
        if revision_changed {
            if let Some(detail) = &self.full_value
                && !(0..node_count).any(|index| {
                    format
                        .node_identity(&projection, index)
                        .is_some_and(|identity| {
                            identity.path == &*detail.path && identity.detail == detail.range
                        })
                })
            {
                self.full_value = None;
            }
            self.mark_edit_stale(cx);
        }
        self.rows = tree::visible_rows(projection.as_ref(), &self.expanded);
    }

    fn rebuild_rows(&mut self, cx: &App) {
        self.rows = self
            .document
            .read(cx)
            .projection()
            .map(|projection| tree::visible_rows(projection, &self.expanded))
            .unwrap_or_default();
    }

    fn select_node(&mut self, index: usize, cx: &mut Context<Self>) {
        let document = self.document.read(cx);
        let Some(identity) = document
            .projection()
            .and_then(|projection| document.format().node_identity(projection, index))
        else {
            return;
        };
        let path = identity.path.to_owned();
        let range = identity.range;
        self.close_edit(None, cx);
        self.mutation_error = None;
        self.notice = None;
        self.selected = Some(index);
        self.selected_path = Some(path);
        self.selected_range = Some(range);
        if let Some(row) = tree::row_of(&self.rows, index) {
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
            let Some(node) = tree::move_selection(&self.rows, self.selected, step) else {
                return;
            };
            self.select_node(node, cx);
            cx.stop_propagation();
            return;
        }
        match key {
            "home" => {
                if let Some(node) = tree::first_node(&self.rows) {
                    self.select_node(node, cx);
                    cx.stop_propagation();
                }
            }
            "end" => {
                if let Some(node) = tree::last_node(&self.rows) {
                    self.select_node(node, cx);
                    cx.stop_propagation();
                }
            }
            "right" => {
                let Some(selected) = self.selected else {
                    if let Some(node) = tree::first_node(&self.rows) {
                        self.select_node(node, cx);
                    }
                    cx.stop_propagation();
                    return;
                };
                let (expandable, expanded, child) = {
                    let projection = self.document.read(cx).projection();
                    let expandable = projection.is_some_and(|p| p.is_expandable(selected));
                    let expanded = self.expanded.get(selected).copied().unwrap_or(false);
                    let child = tree::first_child_node(&self.rows, selected);
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
                } else if let Some(parent) = tree::parent_node(&self.rows, selected) {
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
        let prefix = self.document.read(cx).format().element_prefix();
        let document = self.document.read(cx);
        let format_label = document.format().format_label();
        let parsing = document.is_parsing();
        let loading = document.format().loading_label();
        let fatal = self.fatal_error(cx);
        let counts = document
            .projection()
            .and_then(|projection| document.format().count_notice(projection));
        let documents_notice = document
            .projection()
            .and_then(|projection| document.format().documents_notice(projection));
        let invalid_notice = document
            .projection()
            .and_then(|projection| document.format().invalid_records_notice(projection));
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
        let has_full_value = self.selected.is_some_and(|index| {
            document
                .projection()
                .is_some_and(|projection| document.format().node_has_full_value(projection, index))
        });
        let edit_buttons: Vec<EditButton<F::EditRequest>> = match self.current_projection(cx) {
            Some(projection) => document.format().edit_buttons(&projection, self.selected),
            None => Vec::new(),
        };
        let editable = self.structure_editable(cx);
        let mutation_error = self.mutation_error.clone();
        let notice = self.notice.clone();
        let mode = self.mode;
        let (toggle, tip) = document.format().mode_labels(mode);
        let reveal_label = document.format().reveal_label();
        let full_value_label = document.format().full_value_label();
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let warning = cx.theme().warning;
        h_flex()
            .id(scoped_id(prefix, "toolbar", entity))
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
                    .child(format_label),
            )
            .when_some(documents_notice, |row, documents| {
                row.child(
                    div()
                        .id(scoped_id(prefix, "documents", entity))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(documents),
                )
            })
            .when_some(counts, |row, counts| {
                row.child(
                    div()
                        .id(scoped_id(prefix, "count", entity))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(counts),
                )
            })
            .when_some(invalid_notice, |row, notice| {
                row.child(
                    div()
                        .id(scoped_id(prefix, "invalid-records", entity))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(warning)
                        .child(notice),
                )
            })
            .when(parsing, |row| {
                row.child(
                    h_flex()
                        .id(scoped_id(prefix, "parsing", entity))
                        .test_support()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .text_color(muted)
                        .child(Spinner::new().xsmall())
                        .child(loading),
                )
            })
            .when_some(fatal, |row, (title, _)| {
                row.child(
                    div()
                        .id(scoped_id(prefix, "structure-error", entity))
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
                        .id(scoped_id(prefix, "location", entity))
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
                        .id(scoped_id(prefix, "mutation-error", entity))
                        .test_support()
                        .aria_value(error.clone())
                        .flex_none()
                        .text_xs()
                        .text_color(warning)
                        .child(error),
                )
            })
            .when_some(notice, |row, notice| {
                row.child(
                    div()
                        .id(scoped_id(prefix, "notice", entity))
                        .test_support()
                        .aria_value(notice.clone())
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().yellow)
                        .child(notice),
                )
            })
            .when(editable, |row| {
                let mut row = row;
                for button in &edit_buttons {
                    let request = button.flow.request();
                    row = row.child(
                        Button::new(scoped_id(prefix, button.id, entity))
                            .ghost()
                            .xsmall()
                            .flex_none()
                            .disabled(!button.enabled)
                            .label(button.label.clone())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start_edit_command(window, cx, request.clone())
                            })),
                    );
                }
                row
            })
            .child(div().flex_1())
            .child(
                Button::new(scoped_id(prefix, "reveal", entity))
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .disabled(!has_detail)
                    .label(reveal_label)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.reveal_selected_source(window, cx)),
                    ),
            )
            .when(F::FULL_VALUE, |row| {
                row.child(
                    Button::new(scoped_id(prefix, "full-value-button", entity))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(!has_full_value)
                        .label(full_value_label)
                        .on_click(cx.listener(|this, _, _, cx| this.open_selected_full_value(cx))),
                )
            })
            .child(
                Button::new(scoped_id(prefix, "mode-toggle", entity))
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .label(toggle)
                    .tooltip(tip)
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_mode(window, cx))),
            )
    }

    fn structure_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity_id();
        let prefix = self.document.read(cx).format().element_prefix();
        let format = self.document.read(cx).format().clone();
        if let Some((title, detail)) = self.fatal_error(cx) {
            return v_flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(structure_error(
                    scoped_id(prefix, "structure-error-body", entity),
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
                    scoped_id(prefix, "structure-loading", entity),
                    format.loading_label(),
                    cx,
                )
            } else {
                let (title, detail) = format.unavailable();
                structure_error(
                    scoped_id(prefix, "structure-error-body", entity),
                    title,
                    detail,
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
        let list = uniform_list(
            scoped_id(prefix, "structure", entity),
            self.rows.len(),
            cx.processor(move |view, range: Range<usize>, _, cx| {
                range
                    .map(|row_index| {
                        let Some(projection) = view.document.read(cx).projection() else {
                            return div().into_any_element();
                        };
                        let row = view.rows[row_index];
                        let Some(_identity) = format.node_identity(projection, row.node) else {
                            return div().into_any_element();
                        };
                        let Some(node_row) = format.node_row(projection, row.node, cx.theme())
                        else {
                            return div().into_any_element();
                        };
                        let expandable = projection.is_expandable(row.node);
                        let expanded = view.expanded.get(row.node).copied().unwrap_or(false);
                        let selected = view.selected == Some(row.node);
                        let has_full_value = format.node_has_full_value(projection, row.node);
                        let index = row.node;
                        let row_id = SharedString::from(format!(
                            "{}-row-{}-{index}",
                            prefix,
                            entity.as_u64()
                        ));
                        structure_row(
                            StructureRowData {
                                id: row_id,
                                depth: row.depth,
                                expandable,
                                expanded,
                                selected,
                                striped: row_index % 2 == 1,
                                key: node_row.key,
                                value: node_row.value,
                                color: node_row.color,
                                badge: node_row.badge,
                                record_line: node_row.record_line,
                                error: node_row.error,
                                aria_label: node_row.aria_label,
                            },
                            cx.listener(|view, _: &gpui_kit::MouseDownEvent, window, cx| {
                                view.focus.focus(window, cx);
                                cx.stop_propagation();
                            }),
                            cx.listener(move |view, event: &gpui_kit::ClickEvent, window, cx| {
                                view.focus.focus(window, cx);
                                if event.click_count() > 1 {
                                    view.select_node(index, cx);
                                    if F::FULL_VALUE {
                                        if has_full_value {
                                            view.open_selected_full_value(cx);
                                        }
                                    } else {
                                        view.reveal_selected_source(window, cx);
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
            .id(scoped_id(prefix, "structure-body", entity))
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
        let prefix = self.document.read(cx).format().element_prefix();
        let format = self.document.read(cx).format().clone();
        Some(
            v_flex()
                .id(scoped_id(prefix, "full-value", entity))
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
                                .child(format!("{} · {path}", format.full_value_label())),
                        )
                        .child(
                            Button::new(scoped_id(prefix, "full-value-copy", entity))
                                .ghost()
                                .xsmall()
                                .flex_none()
                                .label(format.copy_value_label())
                                .on_click(cx.listener(|this, _, _, cx| this.copy_full_value(cx))),
                        )
                        .child(
                            Button::new(scoped_id(prefix, "full-value-close", entity))
                                .ghost()
                                .xsmall()
                                .flex_none()
                                .label(t(L10nKey::Close))
                                .on_click(cx.listener(|this, _, _, cx| this.close_full_value(cx))),
                        ),
                )
                .child(
                    div()
                        .id(scoped_id(prefix, "full-value-text", entity))
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

impl<F: StructuredViewFormat> Focusable for StructuredView<F> {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.mode {
            StructuredMode::Structure => self.focus.clone(),
            StructuredMode::Source => self.source.read(cx).focus_handle(cx),
        }
    }
}

impl<F: StructuredViewFormat> Render for StructuredView<F> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.mode;
        let focus = self.focus.clone();
        let focus_ring = cx.theme().primary;
        let entity = cx.entity_id();
        let prefix = self.document.read(cx).format().element_prefix();
        let item = v_flex()
            .id(scoped_id(prefix, "document", entity))
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(self.toolbar(cx));
        let body = match mode {
            StructuredMode::Structure => self.structure_body(cx),
            StructuredMode::Source => self.source_body(),
        };
        let item = item.child(body);
        match mode {
            StructuredMode::Structure => {
                let item = item
                    .key_context(F::KEY_CONTEXT)
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
                    }));
                if F::STRUCTURE_UNDO_REDO {
                    item.on_action(cx.listener(|this, _: &Undo, _, cx| this.undo_structure(cx)))
                        .on_action(cx.listener(|this, _: &Redo, _, cx| this.redo_structure(cx)))
                        .into_any_element()
                } else {
                    item.into_any_element()
                }
            }
            StructuredMode::Source => item.into_any_element(),
        }
    }
}

/// 右侧菜单的结构切换项：默认只给结构与源码切换；结构与源码切换之外的
/// 编辑命令由普通文本菜单或结构工具栏提供。
pub fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
    toggle_label: L10nKey,
) -> gpui_kit::component::menu::PopupMenu {
    menu.separator().menu(
        t(toggle_label),
        Box::new(lyrel_ui_base::actions::ToggleDocumentPreview),
    )
}

/// 元素 id 加格式前缀作用域：`"{prefix}-{id}"`；与静态元组 id 同为
/// `NamedInteger` 表示，测试按字符串值查找。
fn scoped_id(prefix: &'static str, id: &str, entity: gpui_kit::EntityId) -> gpui_kit::ElementId {
    gpui_kit::ElementId::NamedInteger(
        SharedString::from(format!("{prefix}-{id}")),
        entity.as_u64(),
    )
}
