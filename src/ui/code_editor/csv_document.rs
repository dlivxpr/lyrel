//! CSV／TSV 文档：一份逐行保真的共享源码，加上从它派生的虚拟化表格。
//!
//! 表格不是第二份文档：`CsvDocument` 只按当前 `Document` revision 解析一份
//! 完整逻辑表（记录／字段源区间与稳定身份在 `lyrel_code_editor::csv::table`），
//! 每个分组各有自己的选择、列窗口、筛选、排序、滚动与焦点。筛选和排序只
//! 重排视图里的记录编号；打开、导航、排序、筛选和复制都不修改源文。
//!
//! 源码面是同一 `Document` 上的 plain-source `EditorState`，保存、外部重载、
//! 冲突、关闭、撤销和重做仍由现有工作台所有者处理。本票不提供单元格／行／列
//! 编辑控件，也不把显示位置当作记录身份：`#288` 将使用这里保留的
//! [`table::CsvCellId`] 与原始区间做局部写回。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, scroll::Scrollbar,
    spinner::Spinner, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, ElementId, Entity, FocusHandle, Focusable, Pixels,
    Point, Render, ScrollHandle, ScrollStrategy, SharedString, Subscription, TestSupportExt as _,
    UniformListScrollHandle, Window, div, px, rems, uniform_list,
};
use lyrel_markdown_editor::{Document, DocumentEvent, EditorState as SourceEditor};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::ui::i18n::{L10nKey, t, t_fmt};
use lyrel_code_editor::csv::CsvDocumentState;
use lyrel_code_editor::csv::TableInstall;
use lyrel_code_editor::csv::edit::{self, CsvEdit};
use lyrel_code_editor::csv::table;
use lyrel_code_editor::csv::table::{
    CsvCellId, CsvSort, CsvTable, CsvTableError, DEFAULT_LIMITS, DETAIL_CHARS,
};

gpui_kit::actions!(
    csv_document,
    [
        EditCell,
        InsertRowAbove,
        InsertRowBelow,
        DeleteRow,
        InsertColumnLeft,
        InsertColumnRight,
        DeleteColumn,
    ]
);

/// 只在表格持有键盘时成立，源码编辑和终端的键不会被表格导航截走。
pub(crate) const KEY_CONTEXT: &str = "CsvDocument";

/// 输入后短暂合并多次修改，不把每个按键都排成一次整表解析。
const PARSE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(60);

const ROW_HEIGHT: f32 = 26.;
const ROW_HEADER_WIDTH: f32 = 56.;
const CELL_WIDTH: f32 = 160.;
/// 一次画出的逻辑列数；其余列通过分页按钮到达，记录集合本身仍是完整的。
const COLUMN_WINDOW: usize = 20;
/// 每页方向键推动的记录数。
const PAGE_ROWS: isize = 10;

const TAG_CELL: u8 = 0;
const TAG_EXCERPT: u8 = 1;
const TAG_COLUMN: u8 = 2;
/// 单元格输入控件的稳定标识；测试通过它找到真实 Input，而不是隐藏文本。
const TAG_EDITOR: u8 = 4;

/// 一个分组画的是表格还是源码。分屏时两个分组默认各占一面。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CsvMode {
    Table,
    Source,
}

impl CsvMode {
    pub(crate) fn opposite(self) -> Self {
        match self {
            Self::Table => Self::Source,
            Self::Source => Self::Table,
        }
    }
}

/// 一份 CSV／TSV 文件的内容：共享 Document 与它当前的完整逻辑表。
///
/// 解析属于文件，不属于分组；列窗口、排序、筛选和选择属于各 presentation。
pub(crate) struct CsvDocument {
    document: Entity<Document>,
    delimiter: u8,
    state: CsvDocumentState,
    _parse: Option<gpui_kit::Task<()>>,
    _document: Subscription,
}

impl Drop for CsvDocument {
    fn drop(&mut self) {
        self.state.cancel();
    }
}

impl CsvDocument {
    pub(super) fn new(document: Entity<Document>, delimiter: u8, cx: &mut Context<Self>) -> Self {
        let watch = cx.subscribe(&document, |this, _, _: &DocumentEvent, cx| {
            this.invalidate(cx);
        });
        let mut this = Self {
            document,
            delimiter,
            state: CsvDocumentState::new(),
            _parse: None,
            _document: watch,
        };
        this.invalidate(cx);
        this
    }

    pub(crate) fn document(&self) -> &Entity<Document> {
        &self.document
    }

    pub(crate) fn delimiter(&self) -> u8 {
        self.delimiter
    }

    pub(crate) fn table(&self) -> Option<Arc<CsvTable>> {
        self.state.table()
    }

    pub(crate) fn error(&self) -> Option<CsvTableError> {
        self.state.error()
    }

    pub(crate) fn is_parsing(&self) -> bool {
        self.state.is_parsing()
    }

    /// 源文一旦变化就丢弃旧表：旧记录区间不再对应当前 revision，不能继续
    /// 用于定位或复制，也不能被迟到的解析结果替换。
    fn invalidate(&mut self, cx: &mut Context<Self>) {
        if self.state.invalidate() {
            self.schedule(cx);
        }
        cx.notify();
    }

    /// 合并等待中的修订；每份文档最多一个在途解析，完成后只处理最新修订。
    /// debounce 后才取得共享 snapshot，不在每次输入时复制全文。
    fn schedule(&mut self, cx: &mut Context<Self>) {
        let revision = self.document.read(cx).revision();
        let identity = self.document.entity_id();
        let generation = self.state.generation();
        let delimiter = self.delimiter;
        self._parse = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PARSE_DEBOUNCE).await;
            let captured = this
                .update(cx, |this, cx| {
                    let document = this.document.read(cx);
                    if this.state.generation() != generation || document.revision() != revision {
                        return None;
                    }
                    let text = document.snapshot().shared_text();
                    let cancel = this.state.begin_parse();
                    Some((text, cancel))
                })
                .ok()
                .flatten();
            let Some((text, cancel)) = captured else {
                return;
            };
            #[cfg(test)]
            let gate = test_support::take_next_parse();
            let limits = DEFAULT_LIMITS;
            let outcome = crate::ui::host_ops::off_thread(move || {
                if cancel.load(Ordering::Acquire) {
                    return None;
                }
                #[cfg(test)]
                if let Some(gate) = gate {
                    gate.block();
                }
                Some(CsvTable::parse(text, revision, delimiter, limits, &cancel))
            })
            .await
            .flatten();
            let _ = this.update(cx, |this, cx| {
                this.state.end_parse();
                this._parse = None;
                let fresh = this.document.entity_id() == identity
                    && this.document.read(cx).revision() == revision
                    && this.state.generation() == generation;
                match this.state.install(fresh, outcome) {
                    TableInstall::Installed => cx.notify(),
                    TableInstall::Retry => this.schedule(cx),
                }
            });
        }));
    }
}

/// 一份 CSV／TSV 文件在一个分组里的呈现：表格或源码、自己的选择与查询。
pub(crate) struct CsvView {
    document: Entity<CsvDocument>,
    source: Entity<SourceEditor>,
    mode: CsvMode,
    focus: FocusHandle,
    /// 表格的纵向虚拟化滚动；源码滚动由 SourceEditor 自己持有。
    scroll: UniformListScrollHandle,
    horizontal: ScrollHandle,
    filter_input: Entity<InputState>,
    filter: String,
    sort: Option<CsvSort>,
    /// 可见记录编号，按当前筛选／排序；文件顺序本身不在其中。
    order: Vec<usize>,
    /// 当前光标单元格；它是逻辑记录与逻辑列，不是显示位置。
    selection: Option<CsvCellId>,
    /// 矩形选择的固定角；单击与方向键把它重置到光标。
    anchor: Option<CsvCellId>,
    /// 正在编辑的单元格与真实文本输入。输入组件是 gpui-kit 的 `InputState`，
    /// 与筛选框使用同一套编辑／IME 路径，不是隐藏的假控件。
    editing: Option<CsvCellId>,
    /// 开始编辑时的表格 revision；源文在此期间变化则丢弃这次输入。
    editing_revision: Option<u64>,
    editor: Option<Entity<InputState>>,
    column_start: usize,
    query_generation: u64,
    query_running: bool,
    query_pending: bool,
    query_cancel: Arc<AtomicBool>,
    queried_revision: Option<u64>,
    table_revision_seen: Option<u64>,
    _query: Option<gpui_kit::Task<()>>,
    _document: Subscription,
    _source: Subscription,
    _filter: Subscription,
    _editor: Option<Subscription>,
}

impl CsvView {
    pub(super) fn new(
        document: Entity<CsvDocument>,
        mode: CsvMode,
        wrap: bool,
        indent: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let source_document = document.read(cx).document().clone();
        let source = cx.new(move |cx| {
            let mut source = SourceEditor::for_document(source_document, cx);
            source.set_plain_source(true, cx);
            source.set_soft_wrap(wrap, cx);
            source.set_tab_indent(indent);
            source
        });
        let filter_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::EditorCsvFilter)));
        let watch_document = cx.observe(&document, |this, _, cx| this.sync_table(cx));
        let watch_source = cx.observe(&source, |_, _, cx| cx.notify());
        let watch_filter = cx.subscribe_in(
            &filter_input,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.filter = this.filter_input.read(cx).value().to_string();
                    this.restart_query(cx);
                }
            },
        );
        let mut this = Self {
            document,
            source,
            mode,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            horizontal: ScrollHandle::new(),
            filter_input,
            filter: String::new(),
            sort: None,
            order: Vec::new(),
            selection: None,
            anchor: None,
            editing: None,
            editing_revision: None,
            editor: None,
            column_start: 0,
            query_generation: 0,
            query_running: false,
            query_pending: false,
            query_cancel: Arc::default(),
            queried_revision: None,
            table_revision_seen: None,
            _query: None,
            _document: watch_document,
            _source: watch_source,
            _filter: watch_filter,
            _editor: None,
        };
        this.sync_table(cx);
        this
    }

    pub(crate) fn document(&self) -> &Entity<CsvDocument> {
        &self.document
    }

    pub(crate) fn source(&self) -> &Entity<SourceEditor> {
        &self.source
    }

    pub(crate) fn mode(&self) -> CsvMode {
        self.mode
    }

    pub(crate) fn selection(&self) -> Option<CsvCellId> {
        self.selection
    }

    pub(crate) fn sort(&self) -> Option<CsvSort> {
        self.sort
    }

    pub(crate) fn filter(&self) -> &str {
        &self.filter
    }

    #[cfg(test)]
    pub(crate) fn order(&self) -> &[usize] {
        &self.order
    }

    pub(crate) fn table(&self, cx: &App) -> Option<Arc<CsvTable>> {
        self.document.read(cx).table()
    }

    pub(crate) fn facts(&self, cx: &App) -> Option<(usize, usize)> {
        self.table(cx)
            .map(|table| (table.record_count(), table.column_count()))
    }

    pub(crate) fn is_parsing(&self, cx: &App) -> bool {
        self.document.read(cx).is_parsing()
    }

    pub(crate) fn parse_error(&self, cx: &App) -> Option<CsvTableError> {
        self.document.read(cx).error()
    }

    pub(crate) fn visible_count(&self) -> usize {
        self.order.len()
    }

    pub(crate) fn source_scroll_offset(&self, cx: &App) -> Point<Pixels> {
        self.source.read(cx).scroll_offset()
    }

    pub(crate) fn table_scroll_offset(&self) -> Point<Pixels> {
        self.scroll.0.borrow().base_handle.offset()
    }

    pub(crate) fn set_table_scroll_offset(
        &mut self,
        offset: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.scroll.0.borrow().base_handle.set_offset(offset);
        cx.notify();
    }

    pub(crate) fn horizontal_offset(&self) -> Point<Pixels> {
        self.horizontal.offset()
    }

    pub(crate) fn scroll_offset(&self, cx: &App) -> Point<Pixels> {
        match self.mode {
            CsvMode::Table => self.table_scroll_offset(),
            CsvMode::Source => self.source_scroll_offset(cx),
        }
    }

    pub(crate) fn set_scroll_offset(&mut self, offset: Point<Pixels>, cx: &mut Context<Self>) {
        match self.mode {
            CsvMode::Table => self.set_table_scroll_offset(offset, cx),
            CsvMode::Source => self
                .source
                .update(cx, |source, cx| source.set_scroll_offset(offset, cx)),
        }
    }

    pub(crate) fn selected_range(&self, cx: &App) -> std::ops::Range<usize> {
        self.source.read(cx).selected_range()
    }

    pub(crate) fn select_source_range(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) {
        self.show_source(cx);
        self.source
            .update(cx, |source, cx| source.select_range(range, cx));
    }

    pub(crate) fn add_source_selection(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) {
        self.show_source(cx);
        self.source
            .update(cx, |source, cx| source.add_selection(range, cx));
    }

    pub(crate) fn replace_source_range(
        &mut self,
        range: std::ops::Range<usize>,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        self.show_source(cx);
        self.source
            .update(cx, |source, cx| source.replace_range(range, text, cx));
    }

    pub(crate) fn replace_source_ranges(
        &mut self,
        replacements: Vec<(std::ops::Range<usize>, String)>,
        cx: &mut Context<Self>,
    ) {
        self.show_source(cx);
        let source = self.source.clone();
        source.update(cx, |source, cx| source.replace_ranges(replacements, cx));
    }

    pub(crate) fn replace_source_lines(
        &mut self,
        range: std::ops::Range<usize>,
        lines: Vec<std::ops::Range<usize>>,
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
        before: std::ops::Range<usize>,
        after: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) {
        let source = self.source.clone();
        source.update(cx, |source, cx| {
            source.set_history_selection(before, after, cx)
        });
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
        self.commit_edit(None, None, cx);
        if self.mode != CsvMode::Source {
            self.mode = CsvMode::Source;
            cx.notify();
        }
    }

    /// Flip table and source. Switching away from source commits an in-flight
    /// composition through the document's ordinary history first.
    pub(crate) fn toggle_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_composition(cx);
        self.commit_edit(None, None, cx);
        self.mode = self.mode.opposite();
        let focus = self.focus_handle(cx);
        focus.focus(window, cx);
        cx.notify();
    }

    /// Copies the origin group's presentation state into a split view without
    /// changing which side that new view shows.
    pub(crate) fn seed_presentation(
        &mut self,
        selection: Option<CsvCellId>,
        source_selection: std::ops::Range<usize>,
        source_scroll: Point<Pixels>,
        table_scroll: Point<Pixels>,
        table_horizontal: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.selection = selection;
        self.anchor = selection;
        let source = self.source.clone();
        source.update(cx, |source, cx| {
            source.select_range(source_selection, cx);
            source.set_scroll_offset(source_scroll, cx);
        });
        self.scroll.0.borrow().base_handle.set_offset(table_scroll);
        self.horizontal.set_offset(table_horizontal);
    }

    /// 当前单元格的完整值；不存在字段返回 `None`。
    pub(crate) fn selected_value(&self, cx: &App) -> Option<String> {
        let table = self.table(cx)?;
        table.cell_value(self.selection?)
    }

    pub(crate) fn copy_selected(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(value) = self.selected_value(cx) else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(value));
        true
    }

    /// 选中矩形作为制表符分隔文本复制；单格仍是完整原值。
    pub(crate) fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(table) = self.table(cx) else {
            return false;
        };
        let Some(cursor) = self.selection else {
            return false;
        };
        let Some(anchor) = self.selection_origin() else {
            return false;
        };
        let Some(text) = table.selection_text(anchor, cursor) else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    /// 单元格是否在当前矩形选择内（固定角与光标之间）。
    fn range_contains(&self, cell: CsvCellId) -> bool {
        let Some(cursor) = self.selection else {
            return false;
        };
        let anchor = self.anchor.unwrap_or(cursor);
        let record = cell.record().index();
        let column = cell.column();
        let (first_record, last_record) = (
            anchor.record().index().min(cursor.record().index()),
            anchor.record().index().max(cursor.record().index()),
        );
        let (first_column, last_column) = (
            anchor.column().min(cursor.column()),
            anchor.column().max(cursor.column()),
        );
        (first_record..=last_record).contains(&record)
            && (first_column..=last_column).contains(&column)
    }

    /// 当前选择矩形的固定角；没有矩形时就是光标。
    fn selection_origin(&self) -> Option<CsvCellId> {
        self.selection.map(|cursor| self.anchor.unwrap_or(cursor))
    }

    /// 只有仍描述当前源文 revision 的表格才能写回；过期解析结果一律拒绝。
    fn editable_table(&self, cx: &App) -> Option<Arc<CsvTable>> {
        let table = self.table(cx)?;
        let revision = self.document.read(cx).document().read(cx).revision();
        (table.revision() == revision).then_some(table)
    }

    /// 用一个真实的 `InputState` 编辑单元格；它与筛选框共用文本、光标与 IME
    /// 路径。开始编辑时选中原值，第一次输入替换它。
    fn begin_edit(&mut self, cell: CsvCellId, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing == Some(cell) {
            return;
        }
        let Some(table) = self.editable_table(cx) else {
            return;
        };
        if table.record(cell.record()).is_none() || cell.column() >= table.column_count() {
            return;
        }
        let value = table.cell_value(cell).unwrap_or_default();
        let revision = table.revision();
        let editor = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(value.clone(), window, cx);
            state.set_selected_range(0..value.len(), cx);
            state
        });
        let watch = cx.subscribe_in(
            &editor,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { shift, .. } => {
                    this.commit_edit(Some(window), Some(if *shift { -1 } else { 1 }), cx)
                }
                InputEvent::Blur => this.commit_edit(None, None, cx),
                _ => {}
            },
        );
        let focus = editor.read(cx).focus_handle(cx).clone();
        focus.focus(window, cx);
        self.editing = Some(cell);
        self.editing_revision = Some(revision);
        self.editor = Some(editor);
        self._editor = Some(watch);
        self.select_cell(cell.record().index(), cell.column(), cx);
        cx.notify();
    }

    fn begin_edit_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(cell) = self.selection {
            self.begin_edit(cell, window, cx);
        }
    }

    /// 提交单元格输入。`advance` 是回车后的记录步进；失焦提交保持原位。
    fn commit_edit(
        &mut self,
        window: Option<&mut Window>,
        advance: Option<isize>,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.editor.take() else {
            return;
        };
        self._editor = None;
        let Some(cell) = self.editing.take() else {
            return;
        };
        let value = editor.read(cx).value().to_string();
        let record = cell.record().index();
        let column = cell.column();
        let count = self.table(cx).map(|table| table.record_count());
        // 输入开始后源文被外部重载或另一呈现修改时，这份文本属于旧 revision，
        // 不能写回新表格。
        let stale =
            self.editing_revision != Some(self.document.read(cx).document().read(cx).revision());
        self.editing_revision = None;
        if !stale && self.apply_edit(CsvEdit::Cell { cell, value }, cx) {
            if let (Some(step), Some(count)) = (advance, count) {
                let next = (record as isize + step).clamp(0, count as isize - 1) as usize;
                self.select_identity(next, column, cx);
            }
        }
        if let Some(window) = window {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.take().is_none() {
            return;
        }
        self._editor = None;
        self.editing = None;
        self.editing_revision = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// 把一次逻辑编辑作为同一 Document history 中的一条记录写回。整批替换
    /// 一次交给源码呈现，绝不序列化整张表；表格随后按新 revision 重新解析。
    fn apply_edit(&mut self, edit: CsvEdit, cx: &mut Context<Self>) -> bool {
        let Some(table) = self.editable_table(cx) else {
            return false;
        };
        let revision = table.revision();
        let Ok(mutation) = table.edit(revision, edit) else {
            return false;
        };
        if mutation.changed() {
            let source = self.source.clone();
            source.update(cx, |source, cx| {
                source.replace_ranges(mutation.replacements, cx)
            });
        }
        cx.notify();
        true
    }

    fn select_identity(&mut self, record: usize, column: usize, cx: &mut Context<Self>) {
        self.selection = Some(CsvCellId::new(record, column));
        self.anchor = self.selection;
        self.column_start = (column / COLUMN_WINDOW) * COLUMN_WINDOW;
        if let Some(position) = self.order.iter().position(|candidate| *candidate == record) {
            self.scroll
                .scroll_to_item(position, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    fn insert_row(&mut self, below: bool, cx: &mut Context<Self>) {
        let Some(cell) = self.selection else {
            return;
        };
        let Some(table) = self.editable_table(cx) else {
            return;
        };
        let count = table.record_count();
        let column = cell.column().min(table.column_count().saturating_sub(1));
        if !self.apply_edit(
            CsvEdit::InsertRow {
                at: cell.record(),
                below,
            },
            cx,
        ) {
            return;
        }
        let record = match below {
            true => (cell.record().index() + 1).min(count),
            false => cell.record().index(),
        };
        self.select_identity(record, column, cx);
    }

    fn delete_row(&mut self, cx: &mut Context<Self>) {
        let Some(cell) = self.selection else {
            return;
        };
        let Some(table) = self.editable_table(cx) else {
            return;
        };
        let count = table.record_count();
        let column = cell.column().min(table.column_count().saturating_sub(1));
        if !self.apply_edit(CsvEdit::DeleteRow { at: cell.record() }, cx) {
            return;
        }
        let remaining = count.saturating_sub(1);
        match remaining {
            0 => {
                self.selection = None;
                self.anchor = None;
                cx.notify();
            }
            _ => self.select_identity(cell.record().index().min(remaining - 1), column, cx),
        }
    }

    fn insert_column(&mut self, right: bool, cx: &mut Context<Self>) {
        let Some(cell) = self.selection else {
            return;
        };
        if !self.apply_edit(
            CsvEdit::InsertColumn {
                at: cell.column(),
                right,
            },
            cx,
        ) {
            return;
        }
        let column = match right {
            true => cell.column() + 1,
            false => cell.column(),
        };
        self.select_identity(cell.record().index(), column, cx);
    }

    fn delete_column(&mut self, cx: &mut Context<Self>) {
        let Some(cell) = self.selection else {
            return;
        };
        let Some(table) = self.editable_table(cx) else {
            return;
        };
        let columns = table.column_count();
        if !self.apply_edit(CsvEdit::DeleteColumn { at: cell.column() }, cx) {
            return;
        }
        let column = cell.column().min(columns.saturating_sub(2));
        self.select_identity(cell.record().index(), column, cx);
    }

    /// 粘贴从当前单元格开始的表格块。记录身份是逻辑记录，筛选／排序只影响
    /// 可见顺序；超出末尾的行追加到文件末尾。
    fn paste(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        let Some(text) = item.text() else {
            return false;
        };
        let rows = edit::parse_clipboard(&text);
        let Some(cell) = self.selection else {
            return false;
        };
        if !self.apply_edit(CsvEdit::Paste { origin: cell, rows }, cx) {
            return false;
        }
        self.select_identity(cell.record().index(), cell.column(), cx);
        true
    }

    fn undo(&mut self, redo: bool, cx: &mut Context<Self>) {
        let source = self.source.clone();
        source.update(cx, |source, cx| {
            if redo {
                source.redo_document(cx);
            } else {
                source.undo_document(cx);
            }
        });
        cx.notify();
    }

    /// 把表格选择定位到源码里的同一逻辑字段；不等长记录缺少该字段时定位到
    /// 整条记录。切换后由共享 Document 的正常选择／滚动呈现它。
    pub(crate) fn reveal_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_edit(None, None, cx);
        let Some(table) = self.table(cx) else {
            return;
        };
        let Some(cell) = self.selection else {
            return;
        };
        let Some(span) = table
            .cell_span(cell)
            .or_else(|| table.record_span(cell.record()))
        else {
            return;
        };
        self.mode = CsvMode::Source;
        let source = self.source.clone();
        source.update(cx, |source, cx| {
            source.select_range(span, cx);
            source.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
    }

    fn sync_table(&mut self, cx: &mut Context<Self>) {
        let revision = self.document.read(cx).table().map(|table| table.revision());
        if revision != self.table_revision_seen {
            self.table_revision_seen = revision;
            self.restart_query(cx);
        }
        cx.notify();
    }

    fn restart_query(&mut self, cx: &mut Context<Self>) {
        self.query_generation = self.query_generation.wrapping_add(1);
        self.query_cancel.store(true, Ordering::Release);
        self.query_pending = false;
        self.queried_revision = None;
        let Some(table) = self.table(cx) else {
            // 重新解析期间保留选择身份；新表到达后按记录编号重新协调。
            self.order.clear();
            cx.notify();
            return;
        };
        if self.filter.is_empty() && self.sort.is_none() {
            self.order = (0..table.record_count()).collect();
            self.queried_revision = Some(table.revision());
            self.reconcile_selection(&table);
            cx.notify();
            return;
        }
        self.query_pending = true;
        if !self.query_running {
            self.schedule_query(cx);
        }
        cx.notify();
    }

    fn schedule_query(&mut self, cx: &mut Context<Self>) {
        let Some(table) = self.table(cx) else {
            self.query_pending = false;
            return;
        };
        let revision = table.revision();
        let generation = self.query_generation;
        let filter = self.filter.clone();
        let sort = self.sort;
        let cancel = Arc::new(AtomicBool::new(false));
        self.query_cancel = cancel.clone();
        self.query_running = true;
        #[cfg(test)]
        let gate = test_support::take_next_query();
        self._query = Some(cx.spawn(async move |this, cx| {
            let outcome = crate::ui::host_ops::off_thread(move || {
                if cancel.load(Ordering::Acquire) {
                    return None;
                }
                #[cfg(test)]
                if let Some(gate) = gate {
                    gate.block();
                }
                table.query(&filter, sort, &cancel)
            })
            .await
            .flatten();
            let _ = this.update(cx, |this, cx| {
                this.query_running = false;
                this._query = None;
                this.install_query(revision, generation, outcome, cx);
            });
        }));
    }

    fn install_query(
        &mut self,
        revision: u64,
        generation: u64,
        outcome: Option<table::CsvQueryResult>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.query_generation {
            if self.query_pending && !self.query_running {
                self.schedule_query(cx);
            }
            cx.notify();
            return;
        }
        if self.document.read(cx).table().map(|table| table.revision()) != Some(revision) {
            return;
        }
        self.query_pending = false;
        match outcome {
            Some(result) => {
                self.order = result.records;
                self.queried_revision = Some(revision);
                if let Some(table) = self.table(cx) {
                    self.reconcile_selection(&table);
                }
            }
            None => {
                self.order.clear();
                self.queried_revision = None;
            }
        }
        cx.notify();
    }

    fn reconcile_selection(&mut self, table: &CsvTable) {
        if table.record_count() == 0 || table.column_count() == 0 || self.order.is_empty() {
            self.selection = None;
            self.anchor = None;
            return;
        }
        let order = &self.order;
        let clamp = |cell: CsvCellId| {
            let column = cell.column().min(table.column_count() - 1);
            let record = cell.record().index();
            let record = match order.iter().position(|candidate| *candidate == record) {
                Some(_) => record,
                None => order[0],
            };
            CsvCellId::new(record, column)
        };
        let cursor = self
            .selection
            .map(clamp)
            .unwrap_or_else(|| CsvCellId::new(order[0], 0));
        let anchor = self.anchor.map(clamp).unwrap_or(cursor);
        self.selection = Some(cursor);
        self.anchor = Some(anchor);
        self.ensure_column_window(table);
    }

    fn ensure_column_window(&mut self, table: &CsvTable) {
        if table.column_count() == 0 {
            self.column_start = 0;
            return;
        }
        let column = self
            .selection
            .map(CsvCellId::column)
            .unwrap_or(0)
            .min(table.column_count() - 1);
        self.column_start = (column / COLUMN_WINDOW) * COLUMN_WINDOW;
    }

    fn select_cell(&mut self, record: usize, column: usize, cx: &mut Context<Self>) {
        self.selection = Some(CsvCellId::new(record, column));
        self.anchor = self.selection;
        if let Some(table) = self.table(cx) {
            self.ensure_column_window(&table);
        }
        if let Some(position) = self.order.iter().position(|candidate| *candidate == record) {
            self.scroll
                .scroll_to_item(position, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    /// 指针点击：Shift 保留固定角扩展矩形，普通点击重新开始。
    fn click_cell(&mut self, record: usize, column: usize, extend: bool, cx: &mut Context<Self>) {
        if !extend {
            self.select_cell(record, column, cx);
            return;
        }
        if self.anchor.is_none() {
            self.anchor = self.selection;
        }
        self.selection = Some(CsvCellId::new(record, column));
        if let Some(table) = self.table(cx) {
            self.ensure_column_window(&table);
        }
        cx.notify();
    }

    fn move_selection(&mut self, rows: isize, columns: isize, cx: &mut Context<Self>) {
        let Some(table) = self.table(cx) else {
            return;
        };
        if table.record_count() == 0 || table.column_count() == 0 || self.order.is_empty() {
            return;
        }
        let current = self
            .selection
            .unwrap_or_else(|| CsvCellId::new(self.order[0], 0));
        let position = self
            .order
            .iter()
            .position(|candidate| *candidate == current.record().index())
            .unwrap_or(0) as isize;
        let position = (position + rows).clamp(0, self.order.len() as isize - 1) as usize;
        let record = self.order[position];
        let column = (current.column() as isize + columns)
            .clamp(0, table.column_count() as isize - 1) as usize;
        self.selection = Some(CsvCellId::new(record, column));
        self.anchor = self.selection;
        self.ensure_column_window(&table);
        self.scroll
            .scroll_to_item(position, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Shift+方向键扩展矩形；固定角保持不动。
    fn extend_selection(&mut self, rows: isize, columns: isize, cx: &mut Context<Self>) {
        let Some(table) = self.table(cx) else {
            return;
        };
        if table.record_count() == 0 || table.column_count() == 0 || self.order.is_empty() {
            return;
        }
        let current = self
            .selection
            .unwrap_or_else(|| CsvCellId::new(self.order[0], 0));
        if self.anchor.is_none() {
            self.anchor = Some(current);
        }
        let position = self
            .order
            .iter()
            .position(|candidate| *candidate == current.record().index())
            .unwrap_or(0) as isize;
        let position = (position + rows).clamp(0, self.order.len() as isize - 1) as usize;
        let record = self.order[position];
        let column = (current.column() as isize + columns)
            .clamp(0, table.column_count() as isize - 1) as usize;
        self.selection = Some(CsvCellId::new(record, column));
        self.ensure_column_window(&table);
        self.scroll
            .scroll_to_item(position, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn jump_row(&mut self, last: bool, cx: &mut Context<Self>) {
        let Some(table) = self.table(cx) else {
            return;
        };
        if self.order.is_empty() || table.column_count() == 0 {
            return;
        }
        let record = match last {
            true => *self.order.last().expect("non-empty order"),
            false => self.order[0],
        };
        let column = self
            .selection
            .map(CsvCellId::column)
            .unwrap_or(0)
            .min(table.column_count() - 1);
        self.selection = Some(CsvCellId::new(record, column));
        self.anchor = self.selection;
        self.ensure_column_window(&table);
        let position = match last {
            true => self.order.len() - 1,
            false => 0,
        };
        self.scroll
            .scroll_to_item(position, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn jump_column(&mut self, last: bool, cx: &mut Context<Self>) {
        let Some(table) = self.table(cx) else {
            return;
        };
        if table.record_count() == 0 || self.order.is_empty() || table.column_count() == 0 {
            return;
        }
        let record = self
            .selection
            .and_then(|cell| {
                self.order
                    .iter()
                    .find(|candidate| **candidate == cell.record().index())
                    .copied()
            })
            .unwrap_or(self.order[0]);
        let column = match last {
            true => table.column_count() - 1,
            false => 0,
        };
        self.selection = Some(CsvCellId::new(record, column));
        self.anchor = self.selection;
        self.ensure_column_window(&table);
        cx.notify();
    }

    fn toggle_sort(&mut self, column: usize, cx: &mut Context<Self>) {
        self.sort = match self.sort {
            Some(sort) if sort.column == column && !sort.descending => Some(CsvSort {
                column,
                descending: true,
            }),
            Some(sort) if sort.column == column && sort.descending => None,
            _ => Some(CsvSort {
                column,
                descending: false,
            }),
        };
        self.restart_query(cx);
    }

    fn shift_columns(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(table) = self.table(cx) else {
            return;
        };
        let columns = table.column_count();
        if columns == 0 {
            return;
        }
        let next = match forward {
            true => self.column_start.saturating_add(COLUMN_WINDOW),
            false => self.column_start.saturating_sub(COLUMN_WINDOW),
        }
        .min(columns.saturating_sub(1));
        let next = next - next % COLUMN_WINDOW;
        if next != self.column_start {
            self.column_start = next;
            cx.notify();
        }
    }

    fn columns_range(&self, table: &CsvTable) -> std::ops::Range<usize> {
        let start = self
            .column_start
            .min(table.column_count().saturating_sub(1));
        start..(start + COLUMN_WINDOW).min(table.column_count())
    }

    fn clear_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.clear();
        self.filter_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.restart_query(cx);
    }

    fn key_down(
        &mut self,
        event: &gpui_kit::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() {
            // 输入中的回车由 InputState 事件提交；这里只接管仍会冒泡的取消键。
            if event.keystroke.key == "escape" {
                self.cancel_edit(window, cx);
                cx.stop_propagation();
            }
            return;
        }
        let secondary = event.keystroke.modifiers.secondary(cx);
        let shift = event.keystroke.modifiers.shift;
        match event.keystroke.key.as_str() {
            "down" => match shift {
                true => self.extend_selection(1, 0, cx),
                false => self.move_selection(1, 0, cx),
            },
            "up" => match shift {
                true => self.extend_selection(-1, 0, cx),
                false => self.move_selection(-1, 0, cx),
            },
            "right" => match shift {
                true => self.extend_selection(0, 1, cx),
                false => self.move_selection(0, 1, cx),
            },
            "left" => match shift {
                true => self.extend_selection(0, -1, cx),
                false => self.move_selection(0, -1, cx),
            },
            "pagedown" => self.move_selection(PAGE_ROWS, 0, cx),
            "pageup" => self.move_selection(-PAGE_ROWS, 0, cx),
            "tab" => self.move_selection(0, if shift { -1 } else { 1 }, cx),
            "home" => match secondary {
                true => self.jump_row(false, cx),
                false => self.jump_column(false, cx),
            },
            "end" => match secondary {
                true => self.jump_row(true, cx),
                false => self.jump_column(true, cx),
            },
            "enter" => self.reveal_source(window, cx),
            "escape" if !self.filter.is_empty() => self.clear_filter(window, cx),
            "c" if secondary => {
                if !self.copy_selection(cx) {
                    return;
                }
            }
            "v" if secondary => {
                if !self.paste(cx) {
                    return;
                }
            }
            "z" if secondary => self.undo(shift, cx),
            "y" if secondary => self.undo(true, cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn table_loading(&self, view: gpui_kit::EntityId, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .id(("csv-parsing", view))
            .test_support()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap_2()
            .text_xs()
            .text_color(muted)
            .child(Spinner::new().small())
            .child(t(L10nKey::EditorCsvParsing))
            .into_any_element()
    }

    fn query_loading(&self, view: gpui_kit::EntityId, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .id(("csv-querying", view))
            .test_support()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap_2()
            .text_xs()
            .text_color(muted)
            .child(Spinner::new().small())
            .child(t(L10nKey::EditorCsvQuerying))
            .into_any_element()
    }

    fn table_failure(
        &self,
        error: &CsvTableError,
        view: gpui_kit::EntityId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (foreground, muted, warning) = {
            let theme = cx.theme();
            (theme.foreground, theme.muted_foreground, theme.warning)
        };
        let (title, detail, hint) = match error {
            CsvTableError::Issue(issue) => (
                t(L10nKey::EditorCsvSourceIncomplete),
                issue_message(issue),
                t(L10nKey::EditorCsvFixSource),
            ),
            CsvTableError::Budget(limit) => (
                t(L10nKey::EditorCsvTableLimit),
                limit_message(*limit),
                t(L10nKey::EditorCsvLimitHint),
            ),
            CsvTableError::Reader(message) => (
                t(L10nKey::EditorCsvTableUnavailable),
                t_fmt(L10nKey::EditorCsvReaderFailed, &[("error", message)]).into(),
                t(L10nKey::EditorCsvLimitHint),
            ),
            CsvTableError::Cancelled => (
                t(L10nKey::EditorCsvTableUnavailable),
                t(L10nKey::EditorCsvParsing).into(),
                t(L10nKey::EditorCsvLimitHint),
            ),
        };
        v_flex()
            .id(("csv-table-error", view))
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
            .child(
                div()
                    .max_w(rems(34.))
                    .text_xs()
                    .text_color(muted)
                    .child(hint),
            )
            .child(
                Button::new(("csv-show-source", view))
                    .ghost()
                    .xsmall()
                    .label(t(L10nKey::EditorCsvShowSource))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_source(cx);
                        let source = this.source.clone();
                        source.update(cx, |source, cx| source.focus_handle(cx).focus(window, cx));
                    })),
            )
            .into_any_element()
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity_id();
        let table_mode = self.mode == CsvMode::Table;
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let warning = cx.theme().warning;
        let facts = self.facts(cx);
        let parsing = self.is_parsing(cx);
        let failed = self.parse_error(cx).is_some();
        let (toggle_label, toggle_tip) = match table_mode {
            true => (
                t(L10nKey::EditorMarkdownSource),
                t(L10nKey::EditorCsvShowSource),
            ),
            false => (t(L10nKey::EditorCsvTable), t(L10nKey::EditorCsvShowTable)),
        };
        let format = match self.document.read(cx).delimiter() {
            b'\t' => "TSV",
            _ => "CSV",
        };
        let filtered = !self.filter.is_empty();
        let sort = self.sort;
        let columns = facts.map(|(_, columns)| columns).unwrap_or(0);
        let can_previous_columns = self.column_start > 0;
        let can_next_columns = self.column_start + COLUMN_WINDOW < columns;
        h_flex()
            .id(("csv-toolbar", view))
            .flex_none()
            .w_full()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(border)
            .child(div().flex_none().text_xs().text_color(muted).child(format))
            .when_some(facts, |row, (records, columns)| {
                row.child(
                    div()
                        .id(("csv-facts", view))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(t_fmt(
                            L10nKey::EditorCsvFacts,
                            &[
                                ("records", &records.to_string()),
                                ("columns", &columns.to_string()),
                            ],
                        )),
                )
            })
            .when(parsing, |row| {
                row.child(
                    h_flex()
                        .id(("csv-toolbar-parsing", view))
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .text_color(muted)
                        .child(Spinner::new().xsmall())
                        .child(t(L10nKey::EditorCsvParsing)),
                )
            })
            .when(failed, |row| {
                row.child(
                    div()
                        .id(("csv-toolbar-failed", view))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(warning)
                        .child(t(L10nKey::EditorCsvTableUnavailable)),
                )
            })
            .child(div().flex_1())
            .when(table_mode, |row| {
                row.child(
                    div().w(px(180.)).flex_none().child(
                        Input::new(&self.filter_input)
                            .id(("csv-filter", view))
                            .small(),
                    ),
                )
                .when(filtered, |row| {
                    row.child(
                        Button::new(("csv-filter-clear", view))
                            .ghost()
                            .xsmall()
                            .flex_none()
                            .label(t(L10nKey::EditorCsvFilterClear))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.clear_filter(window, cx);
                            })),
                    )
                })
                .child(
                    Button::new(("csv-columns-prev", view))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .icon(IconName::ChevronLeft)
                        .tooltip(t(L10nKey::EditorCsvColumnPrevious))
                        .disabled(!can_previous_columns)
                        .on_click(cx.listener(|this, _, _, cx| this.shift_columns(false, cx))),
                )
                .child(
                    Button::new(("csv-columns-next", view))
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .icon(IconName::ChevronRight)
                        .tooltip(t(L10nKey::EditorCsvColumnNext))
                        .disabled(!can_next_columns)
                        .on_click(cx.listener(|this, _, _, cx| this.shift_columns(true, cx))),
                )
                .when_some(sort, |row, sort| {
                    row.child(
                        div()
                            .id(("csv-sort", view))
                            .test_support()
                            .flex_none()
                            .text_xs()
                            .text_color(muted)
                            .child(sort_label(sort)),
                    )
                })
            })
            .child(
                Button::new(("csv-mode-toggle", view))
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .label(toggle_label)
                    .tooltip(toggle_tip)
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_mode(window, cx))),
            )
            .into_any_element()
    }

    fn render_header(&self, columns: std::ops::Range<usize>, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity_id();
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let secondary = cx.theme().secondary;
        let primary = cx.theme().primary;
        h_flex()
            .id(("csv-header", view))
            .flex_none()
            .w_full()
            .h(px(ROW_HEIGHT))
            .items_center()
            .overflow_hidden()
            .border_b_1()
            .border_color(border)
            .bg(secondary)
            .child(
                div()
                    .w(px(ROW_HEADER_WIDTH))
                    .flex_shrink_0()
                    .px_2()
                    .text_xs()
                    .text_color(muted)
                    .child(t(L10nKey::EditorCsvRowShort)),
            )
            .children(columns.map(|column| {
                let sorted = self.sort.filter(|sort| sort.column == column);
                let label = match sorted {
                    Some(sort) => format!(
                        "{}\u{2009}{}",
                        column + 1,
                        if sort.descending { "↓" } else { "↑" }
                    ),
                    None => (column + 1).to_string(),
                };
                div()
                    .id(column_element_id(view, column))
                    .test_support()
                    .w(px(CELL_WIDTH))
                    .h_full()
                    .flex_shrink_0()
                    .px_2()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(match sorted {
                        Some(_) => primary,
                        None => muted,
                    })
                    .child(div().w_full().min_w_0().truncate().child(label))
                    .aria_label(t_fmt(
                        L10nKey::EditorCsvColumn,
                        &[("column", &(column + 1).to_string())],
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_sort(column, cx)))
            }))
            .into_any_element()
    }

    fn render_details(&self, table: Option<&CsvTable>, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity_id();
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let selected = self.selection.zip(table);
        let (position, text, truncated, copyable) = match selected {
            Some((cell, table)) => {
                let position = t_fmt(
                    L10nKey::EditorCsvCell,
                    &[
                        ("record", &(cell.record().index() + 1).to_string()),
                        ("column", &(cell.column() + 1).to_string()),
                    ],
                );
                match table.detail_text(cell) {
                    Some((text, truncated)) => (position, text, truncated, true),
                    None => (
                        position,
                        t_fmt(
                            L10nKey::EditorCsvMissingField,
                            &[("column", &(cell.column() + 1).to_string())],
                        ),
                        false,
                        false,
                    ),
                }
            }
            None => (
                String::new(),
                t(L10nKey::EditorCsvSelectCell).to_owned(),
                false,
                false,
            ),
        };
        let shown = match text.is_empty() {
            true => t(L10nKey::EditorCsvEmptyField).to_string(),
            false => text.clone(),
        };
        let multi = self
            .anchor
            .zip(self.selection)
            .is_some_and(|(anchor, cursor)| anchor != cursor);
        v_flex()
            .id(("csv-details", view))
            .flex_none()
            .w_full()
            .gap_1()
            .px_2()
            .py_1()
            .border_t_1()
            .border_color(border)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(muted)
                    .child(t(L10nKey::EditorCsvFullValue))
                    .when(!position.is_empty(), |row| {
                        row.child(div().id(("csv-cell-position", view)).child(position))
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new(("csv-copy", view))
                            .ghost()
                            .xsmall()
                            .flex_none()
                            .label(match multi {
                                true => t(L10nKey::EditorCsvCopyCells),
                                false => t(L10nKey::EditorCsvCopyValue),
                            })
                            .disabled(!copyable && !multi)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.copy_selection(cx);
                                this.focus.focus(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id(("csv-cell-detail", view))
                    .test_support()
                    .aria_label(shown.clone())
                    .max_h(px(72.))
                    .overflow_scroll()
                    .text_xs()
                    .text_color(foreground)
                    .children(
                        shown
                            .split('\n')
                            .map(|line| div().whitespace_nowrap().child(line.to_owned())),
                    ),
            )
            .when(truncated, |panel| {
                panel.child(
                    div()
                        .id(("csv-detail-note", view))
                        .test_support()
                        .text_xs()
                        .text_color(muted)
                        .child(t_fmt(
                            L10nKey::EditorCsvDetailTruncated,
                            &[("chars", &DETAIL_CHARS.to_string())],
                        )),
                )
            })
            .into_any_element()
    }

    fn render_table_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity_id();
        if self.is_parsing(cx) {
            return self.table_loading(view, cx);
        }
        if let Some(error) = self.parse_error(cx) {
            return self.table_failure(&error, view, cx);
        }
        let Some(table) = self.table(cx) else {
            return self.table_loading(view, cx);
        };
        if table.record_count() == 0 {
            return self.table_message(view, L10nKey::EditorCsvNoRecords, cx);
        }
        if self.order.is_empty() {
            if self.queried_revision != Some(table.revision()) {
                return self.query_loading(view, cx);
            }
            return self.table_message(view, L10nKey::EditorCsvNoMatch, cx);
        }
        let columns = self.columns_range(&table);
        let width = ROW_HEADER_WIDTH + CELL_WIDTH * columns.len() as f32;
        let border = cx.theme().border;
        let weak = cx.entity().downgrade();
        let rows = uniform_list(
            ("csv-rows", view),
            self.order.len(),
            move |range, _window, cx| {
                let Some(view_entity) = weak.upgrade() else {
                    return Vec::new();
                };
                let this = view_entity.read(cx);
                let Some(table) = this.table(cx) else {
                    return Vec::new();
                };
                let columns = this.columns_range(&table);
                range
                    .filter_map(|position| {
                        let record = *this.order.get(position)?;
                        Some(render_record(
                            &this,
                            record,
                            columns.clone(),
                            &table,
                            &view_entity,
                            cx,
                        ))
                    })
                    .collect()
            },
        )
        .track_scroll(&self.scroll)
        .size_full();
        v_flex()
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, window, cx| this.focus.focus(window, cx)),
            )
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        div()
                            .id(("csv-horizontal", view))
                            .w_full()
                            .h_full()
                            .overflow_x_scroll()
                            .track_scroll(&self.horizontal)
                            .child(
                                v_flex()
                                    .w(px(width))
                                    .h_full()
                                    .min_h_0()
                                    .child(self.render_header(columns, cx))
                                    .child(
                                        div()
                                            .relative()
                                            .flex_1()
                                            .min_h_0()
                                            .w_full()
                                            .child(rows)
                                            .child(
                                                Scrollbar::vertical(&self.scroll)
                                                    .viewport_from_layout(),
                                            ),
                                    ),
                            ),
                    )
                    .child(Scrollbar::horizontal(&self.horizontal).viewport_from_layout()),
            )
            .child(
                div()
                    .h(Scrollbar::width())
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(border),
            )
            .child(self.render_details(Some(table.as_ref()), cx))
            .into_any_element()
    }

    fn table_message(
        &self,
        view: gpui_kit::EntityId,
        key: L10nKey,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                v_flex()
                    .id(("csv-table-empty", view))
                    .test_support()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .text_xs()
                    .text_color(muted)
                    .child(t(key)),
            )
            .child(self.render_details(None, cx))
            .into_any_element()
    }
}

fn render_record(
    view: &CsvView,
    record: usize,
    columns: std::ops::Range<usize>,
    table: &CsvTable,
    entity: &Entity<CsvView>,
    cx: &App,
) -> AnyElement {
    let view_id = entity.entity_id();
    let theme = cx.theme();
    let border = theme.border;
    let muted = theme.muted_foreground;
    let selected_background = theme.secondary;
    let range_background = theme.accent;
    let selected_border = theme.primary;
    let transparent = gpui_kit::transparent_black();
    h_flex()
        .id(cell_element_id(view_id, record, 0, TAG_ROW))
        .test_support()
        .h(px(ROW_HEIGHT))
        .w_full()
        .flex_none()
        .overflow_hidden()
        .border_b_1()
        .border_color(border)
        .child(
            div()
                .id("csv-row-number")
                .role(gpui_kit::Role::RowHeader)
                .w(px(ROW_HEADER_WIDTH))
                .flex_shrink_0()
                .px_2()
                .flex()
                .items_center()
                .text_xs()
                .text_color(muted)
                .child((record + 1).to_string())
                .aria_label(t_fmt(
                    L10nKey::EditorCsvRow,
                    &[("row", &(record + 1).to_string())],
                )),
        )
        .children(columns.map(|column| {
            let cell = CsvCellId::new(record, column);
            let selected = view.selection == Some(cell);
            let in_range = view.range_contains(cell);
            let excerpt: SharedString = table.cell_excerpt(cell).unwrap_or_default().into();
            let cell_view = entity.clone();
            let editing = view.editing == Some(cell);
            let editor = view.editor.clone();
            div()
                .id(cell_element_id(view_id, record, column, TAG_CELL))
                .test_support()
                .role(gpui_kit::Role::Cell)
                .aria_label(excerpt.clone())
                .aria_selected(selected)
                .w(px(CELL_WIDTH))
                .h_full()
                .flex_shrink_0()
                .px_2()
                .flex()
                .items_center()
                .overflow_hidden()
                .border_1()
                .border_color(match selected {
                    true => selected_border,
                    false => transparent,
                })
                .when(in_range && !selected, |cell| cell.bg(range_background))
                .when(selected, |cell| cell.bg(selected_background))
                .child(match (editing, editor) {
                    (true, Some(editor)) => div()
                        .id(cell_element_id(view_id, record, column, TAG_EDITOR))
                        .test_support()
                        .w_full()
                        .min_w_0()
                        .child(Input::new(&editor).xsmall())
                        .into_any_element(),
                    _ => div()
                        .id(cell_element_id(view_id, record, column, TAG_EXCERPT))
                        .test_support()
                        .w_full()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .child(excerpt)
                        .into_any_element(),
                })
                .on_click(move |event, window, cx| {
                    let _ = cell_view.update(cx, |this, cx| {
                        this.focus.focus(window, cx);
                        this.click_cell(record, column, event.modifiers().shift, cx);
                        if event.click_count() == 2 {
                            this.begin_edit(cell, window, cx);
                        }
                    });
                })
                .into_any_element()
        }))
        .into_any_element()
}

fn issue_message(issue: &table::CsvIssue) -> SharedString {
    use table::CsvIssueKind;
    let line = issue.line.to_string();
    match issue.kind {
        CsvIssueKind::UnterminatedQuote => {
            t_fmt(L10nKey::EditorCsvUnterminatedQuote, &[("line", &line)])
        }
        CsvIssueKind::UnsupportedSyntax => {
            t_fmt(L10nKey::EditorCsvUnsupportedSyntax, &[("line", &line)])
        }
    }
    .into()
}

fn limit_message(limit: table::TableLimit) -> SharedString {
    let value = limit.value(DEFAULT_LIMITS);
    match limit {
        table::TableLimit::SourceBytes => t_fmt(
            L10nKey::EditorCsvLimitSource,
            &[("mib", &(value / (1024 * 1024)).to_string())],
        ),
        table::TableLimit::Records => t_fmt(
            L10nKey::EditorCsvLimitRecords,
            &[("limit", &value.to_string())],
        ),
        table::TableLimit::Columns => t_fmt(
            L10nKey::EditorCsvLimitColumns,
            &[("limit", &value.to_string())],
        ),
        table::TableLimit::Cells => t_fmt(
            L10nKey::EditorCsvLimitCells,
            &[("limit", &value.to_string())],
        ),
    }
    .into()
}

pub(super) fn sort_label(sort: CsvSort) -> SharedString {
    t_fmt(
        L10nKey::EditorCsvSortStatus,
        &[
            ("column", &(sort.column + 1).to_string()),
            (
                "direction",
                match sort.descending {
                    true => t(L10nKey::EditorCsvSortDescending),
                    false => t(L10nKey::EditorCsvSortAscending),
                },
            ),
        ],
    )
    .into()
}

fn source_body(source: &Entity<SourceEditor>) -> AnyElement {
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

impl Focusable for CsvView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.mode {
            CsvMode::Table => self.focus.clone(),
            CsvMode::Source => self.source.read(cx).focus_handle(cx),
        }
    }
}

impl Render for CsvView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity_id();
        let mode = self.mode;
        let item = v_flex()
            .id(("csv-document", view))
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(self.render_toolbar(cx));
        let body = match mode {
            CsvMode::Table => self.render_table_body(cx),
            CsvMode::Source => source_body(&self.source),
        };
        let item = item.child(body);
        match mode {
            CsvMode::Table => item
                .key_context(KEY_CONTEXT)
                .track_focus(&self.focus)
                .on_key_down(
                    cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                        this.key_down(event, window, cx);
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &gpui_kit::component::input::Undo, _, cx| {
                        this.undo(false, cx)
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &gpui_kit::component::input::Redo, _, cx| {
                        this.undo(true, cx)
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &gpui_kit::component::input::Paste, _, cx| {
                        this.paste(cx);
                    }),
                )
                .on_action(cx.listener(|this, _: &EditCell, window, cx| {
                    this.begin_edit_selected(window, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &InsertRowAbove, _, cx| this.insert_row(false, cx)),
                )
                .on_action(cx.listener(|this, _: &InsertRowBelow, _, cx| this.insert_row(true, cx)))
                .on_action(cx.listener(|this, _: &DeleteRow, _, cx| this.delete_row(cx)))
                .on_action(
                    cx.listener(|this, _: &InsertColumnLeft, _, cx| this.insert_column(false, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &InsertColumnRight, _, cx| this.insert_column(true, cx)),
                )
                .on_action(cx.listener(|this, _: &DeleteColumn, _, cx| this.delete_column(cx)))
                .on_action(cx.listener(
                    |this, _: &gpui_kit::component::input::Escape, window, cx| {
                        if this.editing.is_some() {
                            this.cancel_edit(window, cx);
                        } else if !this.filter.is_empty() {
                            this.clear_filter(window, cx);
                        } else {
                            return;
                        }
                        cx.stop_propagation();
                    },
                ))
                .into_any_element(),
            CsvMode::Source => item.into_any_element(),
        }
    }
}

/// 表格的右侧菜单。表格模式只给真实可用的操作；源码模式沿用文本命令，
/// 但不出现任何单元格／行／列编辑入口。
pub(super) fn menu_entries(
    menu: PopupMenu,
    view: &Entity<CsvView>,
    mode: CsvMode,
    cx: &mut App,
) -> PopupMenu {
    use gpui_kit::component::input as edit;
    let weak = view.downgrade();
    let has_selection = view.read(cx).selection().is_some();
    let copy = {
        let weak = weak.clone();
        PopupMenuItem::new(t(L10nKey::EditorCsvCopyValue))
            .disabled(!has_selection)
            .on_click(move |_, _, cx| {
                let _ = weak.update(cx, |view, cx| {
                    view.copy_selected(cx);
                });
            })
    };
    let copy_cells = {
        let weak = weak.clone();
        PopupMenuItem::new(t(L10nKey::EditorCsvCopyCells))
            .disabled(!has_selection)
            .on_click(move |_, _, cx| {
                let _ = weak.update(cx, |view, cx| {
                    view.copy_selection(cx);
                });
            })
    };
    let reveal = {
        let weak = weak.clone();
        PopupMenuItem::new(t(L10nKey::EditorCsvRevealSource))
            .disabled(!has_selection)
            .on_click(move |_, window, cx| {
                let _ = weak.update(cx, |view, cx| view.reveal_source(window, cx));
            })
    };
    let toggle = match mode {
        CsvMode::Table => L10nKey::EditorCsvShowSource,
        CsvMode::Source => L10nKey::EditorCsvShowTable,
    };
    let menu = menu
        .separator()
        .menu(
            t(toggle),
            Box::new(crate::core::actions::ToggleDocumentPreview),
        )
        .item(copy)
        .item(copy_cells)
        .item(reveal);
    match mode {
        CsvMode::Table => menu
            .menu(t(L10nKey::AppMenuUndo), Box::new(edit::Undo))
            .menu(t(L10nKey::AppMenuRedo), Box::new(edit::Redo))
            .menu_with_disabled(
                t(L10nKey::AppMenuPaste),
                Box::new(edit::Paste),
                cx.read_from_clipboard().is_none(),
            )
            .separator()
            .menu_with_disabled(
                t(L10nKey::EditorCsvEditCell),
                Box::new(EditCell),
                !has_selection,
            )
            .separator()
            .menu_with_disabled(
                t(L10nKey::EditorCsvInsertRowAbove),
                Box::new(InsertRowAbove),
                !has_selection,
            )
            .menu_with_disabled(
                t(L10nKey::EditorCsvInsertRowBelow),
                Box::new(InsertRowBelow),
                !has_selection,
            )
            .menu_with_disabled(
                t(L10nKey::EditorCsvDeleteRow),
                Box::new(DeleteRow),
                !has_selection,
            )
            .separator()
            .menu_with_disabled(
                t(L10nKey::EditorCsvInsertColumnLeft),
                Box::new(InsertColumnLeft),
                !has_selection,
            )
            .menu_with_disabled(
                t(L10nKey::EditorCsvInsertColumnRight),
                Box::new(InsertColumnRight),
                !has_selection,
            )
            .menu_with_disabled(
                t(L10nKey::EditorCsvDeleteColumn),
                Box::new(DeleteColumn),
                !has_selection,
            ),
        CsvMode::Source => {
            let has_selection = !view.read(cx).selected_range(cx).is_empty();
            menu.separator()
                .menu(t(L10nKey::AppMenuUndo), Box::new(edit::Undo))
                .menu(t(L10nKey::AppMenuRedo), Box::new(edit::Redo))
                .separator()
                .menu_with_disabled(t(L10nKey::AppMenuCut), Box::new(edit::Cut), !has_selection)
                .menu_with_disabled(
                    t(L10nKey::AppMenuCopy),
                    Box::new(edit::Copy),
                    !has_selection,
                )
                .menu_with_disabled(
                    t(L10nKey::AppMenuPaste),
                    Box::new(edit::Paste),
                    cx.read_from_clipboard().is_none(),
                )
                .menu(t(L10nKey::AppMenuSelectAll), Box::new(edit::SelectAll))
                .menu(
                    t(L10nKey::EditorGoToLineAction),
                    Box::new(crate::core::actions::EditorGoToLine),
                )
        }
    }
}

fn scoped_id(view: gpui_kit::EntityId, record: usize, column: usize, tag: u8) -> ElementId {
    let mut bytes = [0u8; 20];
    bytes[..8].copy_from_slice(&view.as_u64().to_le_bytes());
    bytes[8..16].copy_from_slice(&(record as u64).to_le_bytes());
    bytes[16..19].copy_from_slice(&(column as u32).to_le_bytes()[..3]);
    bytes[19] = tag;
    ElementId::from(bytes)
}

fn cell_element_id(view: gpui_kit::EntityId, record: usize, column: usize, tag: u8) -> ElementId {
    scoped_id(view, record, column, tag)
}

fn column_element_id(view: gpui_kit::EntityId, column: usize) -> ElementId {
    scoped_id(view, 0, column, TAG_COLUMN)
}

const TAG_ROW: u8 = 3;

/// 解析／查询的确定性测试门闩：只编译进测试，不改变产品路径。
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::{cell::RefCell, sync::mpsc, time::Duration};

    pub(super) struct Gate {
        receiver: mpsc::Receiver<()>,
        entered: Arc<AtomicBool>,
    }

    thread_local! {
        static PARSE_GATE: RefCell<Option<Gate>> = const { RefCell::new(None) };
        static QUERY_GATE: RefCell<Option<Gate>> = const { RefCell::new(None) };
    }

    fn pause(
        cell: &'static std::thread::LocalKey<RefCell<Option<Gate>>>,
    ) -> (mpsc::Sender<()>, Arc<AtomicBool>) {
        let (tx, rx) = mpsc::channel();
        let entered = Arc::new(AtomicBool::new(false));
        cell.with_borrow_mut(|gate| {
            *gate = Some(Gate {
                receiver: rx,
                entered: entered.clone(),
            })
        });
        (tx, entered)
    }

    pub(crate) fn pause_next_parse() -> (mpsc::Sender<()>, Arc<AtomicBool>) {
        pause(&PARSE_GATE)
    }

    pub(crate) fn pause_next_query() -> (mpsc::Sender<()>, Arc<AtomicBool>) {
        pause(&QUERY_GATE)
    }

    pub(super) fn take_next_parse() -> Option<Gate> {
        PARSE_GATE.with_borrow_mut(Option::take)
    }

    pub(super) fn take_next_query() -> Option<Gate> {
        QUERY_GATE.with_borrow_mut(Option::take)
    }

    impl Gate {
        pub(super) fn block(self) {
            self.entered.store(true, Ordering::SeqCst);
            let _ = self.receiver.recv_timeout(Duration::from_secs(20));
        }
    }
}

#[cfg(test)]
#[path = "csv_document/tests.rs"]
mod tests;
