//! CSV／TSV 记录读取与源区间：完整逻辑表、稳定身份、只读查询。
//!
//! 这里没有 GUI 依赖，也**不持有第二份可写内容**：权威源文是一份共享
//! `lyrel_markdown_editor::Document` 的 LF／UTF-8 snapshot，表只保存每条逻辑
//! 记录的字节区间和每个字段的原始区间。字段值在需要时从同一次 snapshot 解码；
//! 因此显示、排序、筛选与复制都不会把解码后的字符串变回另一份文档。
//!
//! 记录读取沿用 Glim `glim-services/src/csv.rs` 的 `csv` crate 设置
//! （`has_headers(false)`、`flexible(true)`），所以 quoted delimiter、引号内
//! 换行和不等长记录按逻辑记录而不是物理行划分。上游没有源区间或稳定身份；
//! 这里用 `csv::ByteRecord::position()` 取得记录起点，用读取器位置取得记录终点，
//! 再用一个只定位分隔符边界的扫描器补出字段区间，并逐字段与 `csv` 解码值核对。
//! 任何无法精确对应的语法（未闭合引号、引号后再出现字符）都返回明确问题，
//! 不生成一份“猜测的记录”。解析预算超限或取消同样返回错误，绝不返回截断表。
//!
//! 所有区间都是 Document 所持规范文本的 byte offset：BOM 与逐行磁盘行尾由
//! `Document` 自己的 representation 保存，不在这里换算。

use std::ops::Range;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// 单元格可见摘录的字符数；完整值仍可由 `cell_value`、详情面板和剪贴板取得。
pub(crate) const CELL_EXCERPT_CHARS: usize = 64;

/// 详情面板一次排版的最大字符数。超过时面板说明并保留完整复制与源码定位；
/// 它是显示预算，不是字段值的读取上限。
pub(crate) const DETAIL_CHARS: usize = 8 * 1024;

/// 一次表格解析允许的完整逻辑集合；每项注明单位，超限时拒绝整个表格。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CsvLimits {
    /// 参与解析的规范源文字节数。
    pub(crate) source_bytes: usize,
    /// 完整逻辑记录数。
    pub(crate) records: usize,
    /// 任一记录允许的最多字段数，也就是逻辑列数。
    pub(crate) columns: usize,
    /// 全部记录的字段总数。
    pub(crate) cells: usize,
}

pub(crate) const DEFAULT_LIMITS: CsvLimits = CsvLimits {
    source_bytes: 8 * 1024 * 1024,
    records: 200_000,
    columns: 4_096,
    cells: 1_000_000,
};

/// 超限时拒绝的是哪一种预算。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TableLimit {
    SourceBytes,
    Records,
    Columns,
    Cells,
}

impl TableLimit {
    pub(crate) fn value(self, limits: CsvLimits) -> u64 {
        match self {
            Self::SourceBytes => limits.source_bytes as u64,
            Self::Records => limits.records as u64,
            Self::Columns => limits.columns as u64,
            Self::Cells => limits.cells as u64,
        }
    }
}

/// 解析无法给出可信记录的原因。源码在这些情况下仍可编辑。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CsvTableError {
    /// 请求已被替换、关闭或超时；调用方不得安装任何结果。
    Cancelled,
    /// 超过本次解析的 [`CsvLimits`]；`Source` 仍持有完整文件。
    Budget(TableLimit),
    /// 源码还不完整，或使用了无法与 `csv` 解码值精确对应的引号语法。
    Issue(CsvIssue),
    /// `csv` 读取器自身报告的错误（对内存中的 UTF-8 源文通常不可达）。
    Reader(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CsvIssueKind {
    UnterminatedQuote,
    UnsupportedSyntax,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CsvIssue {
    pub(crate) kind: CsvIssueKind,
    /// 逻辑记录起始的物理行号，从 1 开始。
    pub(crate) line: usize,
}

/// 一条逻辑记录的稳定身份：同一 revision 内按文件顺序编号。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CsvRecordId(usize);

impl CsvRecordId {
    pub(crate) fn index(self) -> usize {
        self.0
    }

    /// 按文件顺序构造记录身份；只供已解析表的局部编辑遍历使用。
    pub(crate) fn from_index(index: usize) -> Self {
        Self(index)
    }
}

/// 单元格身份是逻辑记录与逻辑列，而不是排序／筛选后的显示位置。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CsvCellId {
    record: CsvRecordId,
    column: usize,
}

impl CsvCellId {
    pub(crate) fn new(record: usize, column: usize) -> Self {
        Self {
            record: CsvRecordId(record),
            column,
        }
    }

    pub(crate) fn record(self) -> CsvRecordId {
        self.record
    }

    pub(crate) fn column(self) -> usize {
        self.column
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CsvRecord {
    /// 记录内容（不含终止换行）在规范源文中的区间。
    span: Range<usize>,
    #[cfg(test)]
    /// 记录起始的物理行号，从 1 开始；quoted 换行不会把记录切成多条。
    line: usize,
    /// 每个字段的原始区间（含引号，不含分隔符）。
    fields: Vec<Range<usize>>,
}

impl CsvRecord {
    pub(crate) fn span(&self) -> Range<usize> {
        self.span.clone()
    }

    #[cfg(test)]
    pub(crate) fn line(&self) -> usize {
        self.line
    }

    pub(crate) fn field_span(&self, column: usize) -> Option<Range<usize>> {
        self.fields.get(column).cloned()
    }

    /// 这条记录实际写出的字段数；不等长记录可以少于逻辑列数。
    pub(crate) fn field_count(&self) -> usize {
        self.fields.len()
    }
}

/// 一份已解析的完整逻辑表，绑定到产生它的 Document revision。
pub(crate) struct CsvTable {
    text: Arc<String>,
    records: Vec<CsvRecord>,
    columns: usize,
    revision: u64,
    delimiter: u8,
}

/// 一次只读查询的可见顺序。记录编号仍是 [`CsvRecordId`]。
pub(crate) struct CsvQueryResult {
    pub(crate) records: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CsvSort {
    pub(crate) column: usize,
    pub(crate) descending: bool,
}

impl CsvTable {
    /// 读取一份完整逻辑表。
    ///
    /// `text` 是共享 snapshot 的同一个 allocation；`revision` 用来拒绝过期
    /// 结果。解析在调用线程完成，调用方负责放到 UI 线程之外并持有取消标志。
    pub(crate) fn parse(
        text: Arc<String>,
        revision: u64,
        delimiter: u8,
        limits: CsvLimits,
        cancel: &AtomicBool,
    ) -> Result<Self, CsvTableError> {
        if text.len() > limits.source_bytes {
            return Err(CsvTableError::Budget(TableLimit::SourceBytes));
        }
        if cancelled(cancel) {
            return Err(CsvTableError::Cancelled);
        }
        let mut reader = csv::ReaderBuilder::new()
            .delimiter(delimiter)
            .has_headers(false)
            .flexible(true)
            .from_reader(text.as_bytes());
        let mut record = csv::ByteRecord::new();
        let mut records = Vec::new();
        let mut columns = 0usize;
        let mut cells = 0usize;
        let mut last_end = 0usize;
        loop {
            if cancelled(cancel) {
                return Err(CsvTableError::Cancelled);
            }
            let read = reader
                .read_byte_record(&mut record)
                .map_err(|error| CsvTableError::Reader(error.to_string()))?;
            if !read {
                break;
            }
            let line = record
                .position()
                .map(|position| position.line() as usize)
                .unwrap_or(0);
            let start = record
                .position()
                .map(|position| position.byte() as usize)
                .unwrap_or(last_end);
            let mut end = reader.position().byte() as usize;
            last_end = end;
            // `csv` 会跳过空行；若位置落在被跳过的空行之前，记录从第一个
            // 实际字段字节开始，而不是把那些物理行算进记录区间。
            let mut start = start;
            while start < end && matches!(text.as_bytes()[start], b'\n' | b'\r') {
                start += 1;
            }
            // 记录终点是下一条记录的开头；去掉本条记录自己的终止符。
            if end > start && text.as_bytes()[end - 1] == b'\n' {
                end -= 1;
            }
            if end > start && text.as_bytes()[end - 1] == b'\r' {
                end -= 1;
            }
            if record.is_empty() {
                if start == end {
                    // `csv` 的空行不是逻辑记录，也不占物理行的身份。
                    continue;
                }
                return Err(CsvTableError::Issue(CsvIssue {
                    kind: CsvIssueKind::UnsupportedSyntax,
                    line,
                }));
            }
            let fields = scan_fields(&text.as_bytes()[start..end], delimiter)
                .map_err(|kind| CsvTableError::Issue(CsvIssue { kind, line }))?;
            if fields.len() != record.len() {
                return Err(CsvTableError::Issue(CsvIssue {
                    kind: CsvIssueKind::UnsupportedSyntax,
                    line,
                }));
            }
            for (index, field) in fields.iter().enumerate() {
                let Some(expected) = record.get(index) else {
                    return Err(CsvTableError::Issue(CsvIssue {
                        kind: CsvIssueKind::UnsupportedSyntax,
                        line,
                    }));
                };
                let expected = std::str::from_utf8(expected).map_err(|_| {
                    CsvTableError::Issue(CsvIssue {
                        kind: CsvIssueKind::UnsupportedSyntax,
                        line,
                    })
                })?;
                let (decoded, _) =
                    decode_field(&text[start + field.start..start + field.end], usize::MAX);
                if decoded != expected {
                    return Err(CsvTableError::Issue(CsvIssue {
                        kind: CsvIssueKind::UnsupportedSyntax,
                        line,
                    }));
                }
            }
            if records.len() >= limits.records {
                return Err(CsvTableError::Budget(TableLimit::Records));
            }
            columns = columns.max(fields.len());
            if columns > limits.columns {
                return Err(CsvTableError::Budget(TableLimit::Columns));
            }
            cells = cells.saturating_add(fields.len());
            if cells > limits.cells {
                return Err(CsvTableError::Budget(TableLimit::Cells));
            }
            let fields = fields
                .into_iter()
                .map(|field| start + field.start..start + field.end)
                .collect();
            records.push(CsvRecord {
                span: start..end,
                #[cfg(test)]
                line,
                fields,
            });
        }
        Ok(Self {
            text,
            records,
            columns,
            revision,
            delimiter,
        })
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn delimiter(&self) -> u8 {
        self.delimiter
    }

    /// 解析时的规范源文（LF、无 BOM）。局部编辑用它检查记录边界上的换行。
    pub(crate) fn source(&self) -> &str {
        &self.text
    }

    pub(crate) fn record_count(&self) -> usize {
        self.records.len()
    }

    pub(crate) fn column_count(&self) -> usize {
        self.columns
    }

    pub(crate) fn record(&self, record: CsvRecordId) -> Option<&CsvRecord> {
        self.records.get(record.index())
    }

    pub(crate) fn record_span(&self, record: CsvRecordId) -> Option<Range<usize>> {
        self.record(record).map(CsvRecord::span)
    }

    #[cfg(test)]
    pub(crate) fn record_line(&self, record: CsvRecordId) -> Option<usize> {
        self.record(record).map(CsvRecord::line)
    }

    /// 字段的原始源区间；缺失的末尾字段（不等长记录）返回 `None`。
    pub(crate) fn cell_span(&self, cell: CsvCellId) -> Option<Range<usize>> {
        self.record(cell.record())?.field_span(cell.column())
    }

    /// 原始字段文本，含引号与双引号转义，不含分隔符。
    pub(crate) fn cell_source(&self, cell: CsvCellId) -> Option<&str> {
        self.cell_span(cell).map(|span| &self.text[span])
    }

    /// 解码后的完整字段值；不存在的字段返回 `None`，存在但为空返回 `""`。
    pub(crate) fn cell_value(&self, cell: CsvCellId) -> Option<String> {
        let raw = self.cell_source(cell)?;
        Some(decode_field(raw, usize::MAX).0)
    }

    /// 单元格可显示摘录：最多 [`CELL_EXCERPT_CHARS`] 个字符，硬换行压平。
    pub(crate) fn cell_excerpt(&self, cell: CsvCellId) -> Option<String> {
        let raw = self.cell_source(cell)?;
        let (excerpt, _) = decode_field(raw, CELL_EXCERPT_CHARS);
        Some(excerpt.chars().map(flatten).collect())
    }

    /// 详情面板文本与是否被显示预算截断；完整值仍由 [`Self::cell_value`] 取得。
    pub(crate) fn detail_text(&self, cell: CsvCellId) -> Option<(String, bool)> {
        let raw = self.cell_source(cell)?;
        Some(decode_field(raw, DETAIL_CHARS))
    }

    /// 只读筛选／排序。返回值是逻辑记录编号的顺序；源文、记录和字段区间不变。
    ///
    /// 空的筛选与无排序会保留文件顺序。筛选是对任一字段解码值的
    /// Unicode 小写子串匹配。取消在记录边界与排序比较中检查，取消结果被丢弃。
    pub(crate) fn query(
        &self,
        filter: &str,
        sort: Option<CsvSort>,
        cancel: &AtomicBool,
    ) -> Option<CsvQueryResult> {
        let needle = filter.to_lowercase();
        let mut records = Vec::new();
        for record in 0..self.records.len() {
            if record % 256 == 0 && cancelled(cancel) {
                return None;
            }
            if needle.is_empty() {
                records.push(record);
                continue;
            }
            'fields: for column in 0..self.columns {
                let Some(value) = self.cell_value(CsvCellId::new(record, column)) else {
                    continue;
                };
                if value.to_lowercase().contains(&needle) {
                    records.push(record);
                    break 'fields;
                }
            }
        }
        if let Some(sort) = sort.filter(|sort| sort.column < self.columns) {
            let mut keyed: Vec<(usize, String)> = records
                .into_iter()
                .map(|record| {
                    (
                        record,
                        self.cell_value(CsvCellId::new(record, sort.column))
                            .unwrap_or_default(),
                    )
                })
                .collect();
            keyed.sort_by(|left, right| {
                if cancelled(cancel) {
                    return std::cmp::Ordering::Equal;
                }
                let order = match sort.descending {
                    true => right.1.cmp(&left.1),
                    false => left.1.cmp(&right.1),
                };
                order.then(left.0.cmp(&right.0))
            });
            if cancelled(cancel) {
                return None;
            }
            records = keyed.into_iter().map(|(record, _)| record).collect();
        }
        Some(CsvQueryResult { records })
    }
}

fn cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Acquire)
}

/// 在记录区间内定位字段边界。只认分隔符和字段开头的引号；返回的区间相对
/// `raw`。引号后出现字符或引号未闭合都拒绝，避免生成一份猜测的 writable 表。
fn scan_fields(raw: &[u8], delimiter: u8) -> Result<Vec<Range<usize>>, CsvIssueKind> {
    let mut fields = Vec::new();
    let mut start = 0usize;
    loop {
        if start == raw.len() {
            fields.push(start..start);
            return Ok(fields);
        }
        if raw[start] == b'"' {
            let mut cursor = start + 1;
            loop {
                if cursor >= raw.len() {
                    return Err(CsvIssueKind::UnterminatedQuote);
                }
                if raw[cursor] == b'"' {
                    if cursor + 1 < raw.len() && raw[cursor + 1] == b'"' {
                        cursor += 2;
                        continue;
                    }
                    cursor += 1;
                    break;
                }
                cursor += 1;
            }
            if cursor < raw.len() && raw[cursor] != delimiter {
                return Err(CsvIssueKind::UnsupportedSyntax);
            }
            fields.push(start..cursor);
            if cursor == raw.len() {
                return Ok(fields);
            }
            start = cursor + 1;
            continue;
        }
        match raw[start..].iter().position(|byte| *byte == delimiter) {
            Some(offset) => {
                let end = start + offset;
                fields.push(start..end);
                start = end + 1;
            }
            None => {
                fields.push(start..raw.len());
                return Ok(fields);
            }
        }
    }
}

/// 解码一个字段，最多保留 `limit` 个解码字符。返回 (文本, 是否被截断)。
///
/// 引号字段只以字段首尾的引号判定；`scan_fields` 已保证这种字段的引号闭合并
/// 只含双引号转义。未加引号的字段按原文返回，字符数预算只影响显示。
fn decode_field(raw: &str, limit: usize) -> (String, bool) {
    let quoted = raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"');
    let inner = if quoted { &raw[1..raw.len() - 1] } else { raw };
    let mut out = String::new();
    let mut count = 0usize;
    if !quoted {
        for ch in inner.chars() {
            if count >= limit {
                return (out, true);
            }
            out.push(ch);
            count += 1;
        }
        return (out, false);
    }
    let mut chars = inner.chars().peekable();
    while let Some(ch) = chars.next() {
        if count >= limit {
            return (out, true);
        }
        if ch == '"' && chars.peek() == Some(&'"') {
            chars.next();
            out.push('"');
        } else {
            out.push(ch);
        }
        count += 1;
    }
    (out, false)
}

fn flatten(ch: char) -> char {
    match ch {
        '\r' | '\n' | '\t' | '\u{0085}' | '\u{2028}' | '\u{2029}' => ' ',
        other => other,
    }
}

#[cfg(test)]
#[path = "table_tests.rs"]
mod tests;
