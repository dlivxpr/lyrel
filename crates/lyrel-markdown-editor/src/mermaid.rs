//! Mermaid 只暴露源文坐标与派生结果；渲染、缓存和异步生命周期归 Host。
use super::*;

#[derive(Clone)]
pub enum MermaidPresentation {
    Loading(SharedString),
    Failed(SharedString),
    /// 原生图像及其逻辑像素尺寸，不以纹理像素/DPI 推断文档几何。
    Ready(Arc<RenderImage>, f32, f32),
}

#[derive(Clone, Debug)]
pub struct MermaidBlock {
    range: Range<usize>,
    rows: Range<usize>,
    source: SharedString,
}
impl MermaidBlock {
    pub fn range(&self) -> Range<usize> { self.range.clone() }
    pub fn rows(&self) -> Range<usize> { self.rows.clone() }
    pub fn source(&self) -> &str { &self.source }
}

/// 使用编辑器自身 scanner 的 fenced-block 边界，不建立第二份可编辑文本。
pub fn mermaid_blocks(content: &str) -> Vec<MermaidBlock> {
    let mut starts = vec![0];
    starts.extend(content.match_indices('\n').map(|(at, _)| at + 1));
    markdown_syntax::mermaid_blocks(content).into_iter().map(|(rows, source)| {
        // The separator after the closing fence is not part of this owner;
        // replacing the full fenced source must revoke its lease even if the
        // following newline remains untouched.
        let end = starts.get(rows.end).map_or(content.len(), |at| at.saturating_sub(1));
        let range = starts[rows.start]..end;
        MermaidBlock { range, rows, source: source.into() }
    }).collect()
}

impl EditorState {
    /// 当前块的真实 Host 派生状态；无 provider/非 Mermaid 行返回 None。
    pub fn mermaid_presentation(&self, opening_row: usize) -> Option<MermaidPresentation> {
        let provider = self.block_mermaid.as_ref()?;
        let scan = self.scan_data();
        let (_, source) = scan.mermaid.iter().find(|(rows, _)| rows.start == opening_row)?;
        Some(provider(opening_row, source))
    }
}
