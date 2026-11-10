//! Lyrel 的窄写作 Host seam；不持有第二份源文或历史。
use super::*;

/// 请求当前 presentation 的插入菜单或颜色选择器。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WritingIntent {
    Insert,
    TextColor,
    HighlightColor,
}

/// 浮层捕获的源文目标；Host 必须在提交前核对 revision。
#[derive(Clone)]
pub struct WritingRequest {
    intent: WritingIntent,
    range: Range<usize>,
    revision: u64,
    position: Point<Pixels>,
}
impl WritingRequest {
    pub fn intent(&self) -> WritingIntent { self.intent }
    pub fn range(&self) -> Range<usize> { self.range.clone() }
    pub fn revision(&self) -> u64 { self.revision }
    pub fn position(&self) -> Point<Pixels> { self.position }
}

pub(crate) type WritingProvider = std::rc::Rc<dyn Fn(WritingRequest, &mut Window, &mut App) -> Option<gpui_kit::AnyView>>;
pub(crate) type PresentationStyleProvider = std::rc::Rc<dyn Fn(&App) -> SyntaxStyle>;

impl EditorState {
    /// 安装真实 Host 后，插入快捷键、行首 slash 与自定义颜色才可用。
    pub fn set_writing_provider(&mut self, provider: impl Fn(WritingRequest, &mut Window, &mut App) -> Option<gpui_kit::AnyView> + 'static) {
        self.writing_provider = Some(std::rc::Rc::new(provider));
        self.host_custom_colors = true;
    }
    /// 样式随工作台 theme 读取，不改变 view 的 live/source 模式或文档。
    pub fn set_presentation_style_provider(&mut self, provider: impl Fn(&App) -> SyntaxStyle + 'static) {
        self.presentation_style_provider = Some(std::rc::Rc::new(provider));
    }
    /// 当前选区包裹的显式颜色；混合或无色选区返回 None。
    pub fn selection_color(&self, kind: ColorKind) -> Option<u32> {
        use zorite_markdown::syntax::{StyledKind, styled_tag};
        let range = self.selected_range.clone();
        let text = &self.content[range.clone()];
        let want = if kind == ColorKind::Text { StyledKind::Span } else { StyledKind::Mark };
        let tag = if text.starts_with('<') && text.ends_with(want.close()) {
            styled_tag(&text[..=text.find('>')?])?
        } else {
            let before = &self.content[..range.start];
            if !self.content[range.end..].starts_with(want.close()) { return None; }
            styled_tag(&before[before.rfind('<')?..])?
        };
        if tag.kind != want { return None; }
        if kind == ColorKind::Text { tag.color } else { tag.background }
    }
    pub(super) fn set_selection_color(&mut self, kind: ColorKind, color: Option<u32>, cx: &mut Context<Self>) {
        use zorite_markdown::syntax::{StyledKind, styled_tag};
        self.finish_composition(cx);
        let before = self.selected_range.clone();
        self.trim_dangling_tags();
        let selected = self.selected_range.clone();
        if selected.is_empty() { return; }
        let want = if kind == ColorKind::Text { StyledKind::Span } else { StyledKind::Mark };
        let close = want.close();
        let matches = |tag: &str| styled_tag(tag).is_some_and(|tag| tag.kind == want);
        let mut range = selected.clone();
        let mut body = selected.clone();
        if let Some(at) = self.content[..selected.start].rfind('<')
            && matches(&self.content[at..selected.start])
            && self.content[selected.end..].starts_with(close)
        {
            range = at..selected.end + close.len();
        } else if self.content[selected.clone()].starts_with('<')
            && let Some(end) = self.content[selected.clone()].find('>')
            && matches(&self.content[selected.start..=selected.start + end])
            && self.content[selected.clone()].ends_with(close)
            && selected.len() >= end + 1 + close.len()
        {
            body = selected.start + end + 1..selected.end - close.len();
        }
        if kind == ColorKind::Highlight {
            if self.content[body.clone()].starts_with("==") && self.content[body.clone()].ends_with("==") && body.len() >= 4 {
                body = body.start + 2..body.end - 2;
            } else if self.content[..body.start].ends_with("==") && self.content[body.end..].starts_with("==") {
                range = range.start - 2..range.end + 2;
            }
        }
        let (open, close) = match color {
            Some(color) => {
                let hex = zorite_markdown::syntax::hex_color(color);
                if kind == ColorKind::Text { (format!("<span style=\"color:{hex}\">"), "</span>") }
                else { (format!("<mark style=\"background:{hex}\">"), "</mark>") }
            }
            None => (String::new(), ""),
        };
        let new = format!("{open}{}{close}", &self.content[body.clone()]);
        if self.content[range.clone()] == new { return; }
        let after = range.start + open.len()..range.start + open.len() + body.len();
        self.last_edit = EditKind::Other;
        self.splice(range, new, after.clone(), cx);
        self.set_history_selection(before, after, cx);
        self.last_edit = EditKind::Other;
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }
    /// 内容 inset 用 rem 表达；scroll owner 保持整个 pane。
    pub fn set_document_inset(&mut self, horizontal: f32, vertical: f32) {
        self.document_inset = (horizontal.max(0.), vertical.max(0.));
    }
    pub fn clear_writing_overlay(&mut self, cx: &mut Context<Self>) {
        self.writing_overlay = None;
        cx.notify();
    }
    pub(super) fn request_writing(&mut self, intent: WritingIntent, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if self.source_only || self.markdown_style.is_none() { return; }
        let Some(provider) = self.writing_provider.clone() else { return; };
        self.finish_composition(cx);
        let request = WritingRequest {
            intent,
            range,
            revision: self.document.read(cx).revision(),
            position: self.caret_screen_bounds().map_or_else(Point::default, |b| point(b.left(), b.bottom())),
        };
        let editor = cx.weak_entity();
        // Host creation reads this view and may open a focus-owning overlay.
        // Defer until the native input's mutable entity borrow has ended.
        window.defer(cx, move |window, cx| {
            let overlay = provider(request, window, cx);
            let _ = editor.update(cx, |editor, cx| {
                editor.writing_overlay = overlay;
                cx.notify();
            });
        });
    }
    pub(super) fn insert_writing_block(&mut self, _: &InsertWritingBlock, window: &mut Window, cx: &mut Context<Self>) {
        self.request_writing(WritingIntent::Insert, self.selected_range.clone(), window, cx);
    }
    pub(super) fn text_color_picker(&mut self, _: &TextColorPicker, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            self.request_writing(WritingIntent::TextColor, self.selected_range.clone(), window, cx);
        }
    }
    pub(super) fn highlight_color_picker(&mut self, _: &HighlightColorPicker, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            self.request_writing(WritingIntent::HighlightColor, self.selected_range.clone(), window, cx);
        }
    }
    /// Quiet heading disclosure is view-local; unlike callouts, it is not a source edit.
    pub(super) fn toggle_heading_row(&mut self, row: usize, cx: &mut Context<Self>) {
        let start = self.line_starts()[row];
        let end = self.line_end(row);
        let key = self.content[start..end].trim().to_string();
        if !self.folded_headings.remove(&key) {
            let single = std::collections::HashSet::from([key.clone()]);
            let caret_row = self.row_col(self.cursor_offset()).0;
            if markdown_syntax::heading_fold_regions(&self.content, &single).iter().any(|r| caret_row > r.start && caret_row < r.end) {
                self.move_to(end, cx);
            }
            self.folded_headings.insert(key);
        }
        self.invalidate_geometry();
        cx.notify();
    }
    pub(super) fn toggle_heading_disclosure(&mut self, _: &ToggleHeadingDisclosure, _: &mut Window, cx: &mut Context<Self>) {
        let (row, _) = self.row_col(self.cursor_offset());
        if let Some(row) = (0..=row).rev().find(|&row| markdown_syntax::heading_level(self.line_str(row)).is_some() && !self.scan_data().fence_odd.get(row).copied().unwrap_or(false)) {
            self.toggle_heading_row(row, cx);
        }
    }
}
