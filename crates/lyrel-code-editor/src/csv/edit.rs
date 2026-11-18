//! CSV／TSV 的局部源编辑：单元格、表格粘贴与行列增删。
//!
//! 上游 Glim 的 CSV reader 只有读取；这里补的是缺失的 mutation 层。每个操作
//! 只针对当前 [`CsvTable`] 已解析的源区间产生局部替换：绝不把整张表序列化，
//! 也不使用排序／筛选后的显示位置——身份始终是逻辑记录与逻辑列。
//!
//! 所有替换都在同一 revision 上计算，调用方把整批替换作为一次
//! `Document` 修改交给共享 history。未触及的字段保持原有拼写、引号、分隔符、
//! BOM／编码与逐行行尾；只有目标字段被重写。引号只在新值确实需要时添加，
//! 因此粘贴相同值时不会把 `"a"` 变成 `a`。
//!
//! 不等长记录保持不等长：删除列只影响实际写出该字段的记录；粘贴到超过
//! 记录字段数的列会补出中间的空字段，因为那正是被显式选中的目标列。

use std::ops::Range;

use super::table::{CsvCellId, CsvRecordId, CsvTable};

/// 一次逻辑编辑。它绑定表格的 revision，绝不跨 revision 复用区间。
#[derive(Clone, Debug)]
pub enum CsvEdit {
    /// 设置一个单元格；字段不存在时按列位置补出。
    Cell { cell: CsvCellId, value: String },
    /// 从 `origin` 开始的矩形粘贴；每一行是一个逻辑记录。
    Paste {
        origin: CsvCellId,
        rows: Vec<Vec<String>>,
    },
    /// 在选中记录之前／之后插入一条空记录。
    InsertRow { at: CsvRecordId, below: bool },
    /// 删除一条逻辑记录及其终止符，不留下空行。
    DeleteRow { at: CsvRecordId },
    /// 在选中列之前／之后插入一个空字段。
    InsertColumn { at: usize, right: bool },
    /// 删除一列；短记录不凭空补字段。
    DeleteColumn { at: usize },
}

/// 无法安全写回的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsvEditError {
    /// 表格来自另一 revision；旧区间不能用于当前源文。
    Stale,
    /// 目标记录、列或字段不存在。
    Missing,
}

/// 一次编辑产生的局部源替换。
#[derive(Debug, Default)]
pub struct CsvMutation {
    pub replacements: Vec<(Range<usize>, String)>,
}

impl CsvMutation {
    pub fn changed(&self) -> bool {
        !self.replacements.is_empty()
    }
}

impl CsvTable {
    /// 计算一次局部编辑。`revision` 是调用方当前文档的 revision；不相等时
    /// 拒绝，而不是把旧区间写进新源文。
    pub fn edit(&self, revision: u64, edit: CsvEdit) -> Result<CsvMutation, CsvEditError> {
        if revision != self.revision() {
            return Err(CsvEditError::Stale);
        }
        match edit {
            CsvEdit::Cell { cell, value } => self.set_cell(cell, &value),
            CsvEdit::Paste { origin, rows } => self.paste(origin, &rows),
            CsvEdit::InsertRow { at, below } => self.insert_row(at, below),
            CsvEdit::DeleteRow { at } => self.delete_row(at),
            CsvEdit::InsertColumn { at, right } => self.insert_column(at, right),
            CsvEdit::DeleteColumn { at } => self.delete_column(at),
        }
    }

    fn set_cell(&self, cell: CsvCellId, value: &str) -> Result<CsvMutation, CsvEditError> {
        let record = self.record(cell.record()).ok_or(CsvEditError::Missing)?;
        if cell.column() >= self.column_count() {
            return Err(CsvEditError::Missing);
        }
        let mut replacements = Vec::new();
        match record.field_span(cell.column()) {
            Some(span) => {
                if self.cell_value(cell).as_deref() != Some(value) {
                    replacements.push((span, escape_field(value, self.delimiter())));
                }
            }
            None => {
                let text = self.missing_tail(record.field_count(), cell.column(), value);
                push_insertion(&mut replacements, record.span().end, text);
            }
        }
        Ok(CsvMutation { replacements })
    }

    fn paste(&self, origin: CsvCellId, rows: &[Vec<String>]) -> Result<CsvMutation, CsvEditError> {
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        let base_record = origin.record().index();
        if rows.is_empty()
            || width == 0
            || base_record >= self.record_count()
            || origin.column() >= self.column_count()
        {
            return Err(CsvEditError::Missing);
        }
        let base_column = origin.column();
        let last_column = base_column + width - 1;
        let mut replacements = Vec::new();
        let mut appended: Vec<String> = Vec::new();
        for (offset, row) in rows.iter().enumerate() {
            let value = |column: usize| -> &str {
                column
                    .checked_sub(base_column)
                    .and_then(|index| row.get(index))
                    .map_or("", String::as_str)
            };
            let Some(record) = self.record(CsvRecordId::from_index(base_record + offset)) else {
                let fields: Vec<String> = (0..=last_column)
                    .map(|column| escape_field(value(column), self.delimiter()))
                    .collect();
                appended.push(fields.join(&(self.delimiter() as char).to_string()));
                continue;
            };
            let fields = record.field_count();
            for column in base_column..fields.min(last_column + 1) {
                let cell = CsvCellId::new(base_record + offset, column);
                if self.cell_value(cell).as_deref() == Some(value(column)) {
                    continue;
                }
                if let Some(span) = record.field_span(column) {
                    replacements.push((span, escape_field(value(column), self.delimiter())));
                }
            }
            if last_column >= fields {
                let mut tail = String::new();
                for column in fields..=last_column {
                    tail.push(self.delimiter() as char);
                    tail.push_str(&escape_field(value(column), self.delimiter()));
                }
                push_insertion(&mut replacements, record.span().end, tail);
            }
        }
        if !appended.is_empty() {
            let source = self.source();
            let ended = source.ends_with('\n') || source.ends_with('\r');
            let prefix = match ended {
                true => "",
                false => "\n",
            };
            let suffix = match ended {
                true => "\n",
                false => "",
            };
            let text = format!("{prefix}{}{suffix}", appended.join("\n"));
            push_insertion(&mut replacements, source.len(), text);
        }
        Ok(CsvMutation { replacements })
    }

    fn insert_row(&self, at: CsvRecordId, below: bool) -> Result<CsvMutation, CsvEditError> {
        let record = self.record(at).ok_or(CsvEditError::Missing)?;
        let empty = self.empty_record();
        let span = record.span();
        let (offset, text) = match below {
            true => (span.end, format!("\n{empty}")),
            false => (span.start, format!("{empty}\n")),
        };
        let mut replacements = Vec::new();
        push_insertion(&mut replacements, offset, text);
        Ok(CsvMutation { replacements })
    }

    fn delete_row(&self, at: CsvRecordId) -> Result<CsvMutation, CsvEditError> {
        let record = self.record(at).ok_or(CsvEditError::Missing)?;
        let span = record.span();
        let source = self.source();
        let after = terminator_after(source, span.end);
        let range = match after > 0 {
            true => span.start..span.end + after,
            false => span.start - terminator_before(source, span.start)..span.end,
        };
        Ok(CsvMutation {
            replacements: vec![(range, String::new())],
        })
    }

    fn insert_column(&self, at: usize, right: bool) -> Result<CsvMutation, CsvEditError> {
        if at >= self.column_count() {
            return Err(CsvEditError::Missing);
        }
        let column = match right {
            true => at + 1,
            false => at,
        };
        let separator = (self.delimiter() as char).to_string();
        let mut replacements = Vec::new();
        for record in 0..self.record_count() {
            let record = self
                .record(CsvRecordId::from_index(record))
                .ok_or(CsvEditError::Missing)?;
            let fields = record.field_count();
            let offset = match column.cmp(&fields) {
                std::cmp::Ordering::Less => {
                    record
                        .field_span(column)
                        .ok_or(CsvEditError::Missing)?
                        .start
                }
                std::cmp::Ordering::Equal => record.span().end,
                // 短记录没有这一列；让它继续保持缺失，而不是补出中间字段。
                std::cmp::Ordering::Greater => continue,
            };
            push_insertion(&mut replacements, offset, separator.clone());
        }
        Ok(CsvMutation { replacements })
    }

    fn delete_column(&self, at: usize) -> Result<CsvMutation, CsvEditError> {
        if at >= self.column_count() {
            return Err(CsvEditError::Missing);
        }
        let mut replacements = Vec::new();
        for index in 0..self.record_count() {
            let record = self
                .record(CsvRecordId::from_index(index))
                .ok_or(CsvEditError::Missing)?;
            let fields = record.field_count();
            if at >= fields {
                continue;
            }
            let span = record.field_span(at).ok_or(CsvEditError::Missing)?;
            match fields {
                // 单字段记录删列后仍是一条记录：保留一个显式空字段。
                1 => {
                    if self.cell_source(CsvCellId::new(index, at)) != Some("\"\"") {
                        replacements.push((span, "\"\"".to_owned()));
                    }
                }
                _ if at + 1 < fields => {
                    replacements.push((span.start..span.end + 1, String::new()))
                }
                _ => replacements.push((span.start - 1..span.end, String::new())),
            }
        }
        Ok(CsvMutation { replacements })
    }

    /// 粘贴到不存在的字段时补出的尾部：中间缺失字段成为空字段。
    fn missing_tail(&self, fields: usize, column: usize, value: &str) -> String {
        let mut text = String::new();
        for _ in fields..column {
            text.push(self.delimiter() as char);
        }
        text.push(self.delimiter() as char);
        text.push_str(&escape_field(value, self.delimiter()));
        text
    }

    /// 新空记录必须能被 `csv` 读回：单列空记录只能写成显式 `""`，否则整行
    /// 会变成被读取器跳过的空行。
    fn empty_record(&self) -> String {
        match self.column_count() {
            0 => String::new(),
            1 => "\"\"".to_owned(),
            columns => (self.delimiter() as char).to_string().repeat(columns - 1),
        }
    }

    /// 选中矩形作为制表符分隔文本；单格返回完整原值，多格按 TSV 引号规则编码。
    pub fn selection_text(&self, anchor: CsvCellId, cursor: CsvCellId) -> Option<String> {
        if self.column_count() == 0 || self.record_count() == 0 {
            return None;
        }
        let first_record = anchor.record().index().min(cursor.record().index());
        let last_record = anchor.record().index().max(cursor.record().index());
        if last_record >= self.record_count() {
            return None;
        }
        let first_column = anchor.column().min(cursor.column());
        let last_column = anchor
            .column()
            .max(cursor.column())
            .min(self.column_count() - 1);
        if first_record == last_record && first_column == last_column {
            return self.cell_value(CsvCellId::new(first_record, first_column));
        }
        let mut rows = Vec::new();
        for record in first_record..=last_record {
            let mut fields = Vec::new();
            for column in first_column..=last_column {
                let value = self
                    .cell_value(CsvCellId::new(record, column))
                    .unwrap_or_default();
                fields.push(escape_tsv(&value));
            }
            rows.push(fields.join("\t"));
        }
        Some(rows.join("\n"))
    }
}

/// 同一偏移上的多个零长度插入按调用顺序合并，避免相邻记录尾部和追加行
/// 在同一处插入时彼此颠倒。
fn push_insertion(replacements: &mut Vec<(Range<usize>, String)>, at: usize, text: String) {
    if let Some((range, existing)) = replacements
        .iter_mut()
        .find(|(range, _)| range.start == at && range.end == at)
    {
        debug_assert_eq!(range.start, range.end);
        existing.push_str(&text);
        return;
    }
    replacements.push((at..at, text));
}

fn terminator_after(source: &str, at: usize) -> usize {
    let bytes = source.as_bytes();
    match (bytes.get(at), bytes.get(at + 1)) {
        (Some(b'\r'), Some(b'\n')) => 2,
        (Some(b'\n') | Some(b'\r'), _) => 1,
        _ => 0,
    }
}

fn terminator_before(source: &str, at: usize) -> usize {
    let bytes = source.as_bytes();
    if at >= 2 && &bytes[at - 2..at] == b"\r\n" {
        return 2;
    }
    match at.checked_sub(1).and_then(|index| bytes.get(index)) {
        Some(b'\n') | Some(b'\r') => 1,
        _ => 0,
    }
}

/// 一个字段的源文本。只在新值包含分隔符、引号或换行时加引号；已有内容
/// 不受影响，因为调用方只在解码值确实变化时才替换。
pub fn escape_field(value: &str, delimiter: u8) -> String {
    if value
        .chars()
        .any(|ch| ch == delimiter as char || ch == '"' || ch == '\n' || ch == '\r')
    {
        return format!("\"{}\"", value.replace('"', "\"\""));
    }
    value.to_owned()
}

fn escape_tsv(value: &str) -> String {
    if value
        .chars()
        .any(|ch| ch == '\t' || ch == '"' || ch == '\n' || ch == '\r')
    {
        return format!("\"{}\"", value.replace('"', "\"\""));
    }
    value.to_owned()
}

/// 解析剪贴板里的表格文本：制表符分隔，支持引号包裹的制表符与换行。
/// 引号未闭合时退回单格原文：`csv` reader 在 EOF 处会宽松接受未闭合引号并
/// 丢掉引号字符，而粘贴是显式用户操作，不能静默改写复制的内容。
pub fn parse_clipboard(text: &str) -> Vec<Vec<String>> {
    if text.is_empty() {
        return Vec::new();
    }
    if !clipboard_quoting_is_closed(text) {
        return vec![vec![text.to_owned()]];
    }
    let mut reader = ::csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut rows: Vec<Vec<String>> = Vec::new();
    for record in reader.records() {
        let Ok(record) = record else {
            return vec![vec![text.to_owned()]];
        };
        rows.push(record.iter().map(str::to_owned).collect());
    }
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    if width == 0 {
        return Vec::new();
    }
    for row in &mut rows {
        row.resize(width, String::new());
    }
    rows
}

/// 剪贴板文本里的引号是否都在字段内闭合。只检查字段起始的引号；未闭合时
/// 调用方保留原文，不让宽松 reader 丢掉引号。
fn clipboard_quoting_is_closed(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut at_field_start = true;
    let mut cursor = 0;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\t' | b'\n' | b'\r' => {
                at_field_start = true;
                cursor += 1;
            }
            b'"' if at_field_start => {
                cursor += 1;
                loop {
                    if cursor >= bytes.len() {
                        return false;
                    }
                    if bytes[cursor] == b'"' {
                        if cursor + 1 < bytes.len() && bytes[cursor + 1] == b'"' {
                            cursor += 2;
                            continue;
                        }
                        cursor += 1;
                        break;
                    }
                    cursor += 1;
                }
                at_field_start = false;
            }
            _ => {
                at_field_start = false;
                cursor += 1;
            }
        }
    }
    true
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
