//! 格式无关的结构树交互：可见行展开、行选择与源码行列定位。
//!
//! 这里只处理“一棵按子节点列表组织的树如何变成可见行、上下左右如何移动”
//! 这类真正跨格式成立的部分。节点语义、值和源码范围由各格式自己的投影
//! 提供；JSON 的无损模型在 `json_document/projection.rs`，后续 YAML／TOML／
//! XML 可以各自实现 [`Treelike`]，不需要依赖 JSON 的解析或未来的 JSON
//! mutation API。

use std::ops::Range;

/// 一个可供结构视图遍历的树。
///
/// 节点用稳定的 `usize` 索引标识；实现者要为同一个投影保持索引稳定，
/// 并在安装新 revision 时整体替换投影。
pub trait Treelike {
    fn root(&self) -> Option<usize>;
    fn children(&self, node: usize) -> &[usize];

    fn is_expandable(&self, node: usize) -> bool {
        !self.children(node).is_empty()
    }
}

/// 一行可见节点：节点自身，以及它在当前展开状态下的缩进深度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisibleRow {
    pub node: usize,
    pub depth: usize,
}

/// 按根和展开状态深度优先展开；折叠节点不产生子行。
pub fn visible_rows(tree: &impl Treelike, expanded: &[bool]) -> Vec<VisibleRow> {
    let mut rows = Vec::new();
    let Some(root) = tree.root() else {
        return rows;
    };
    let mut pending = vec![(root, 0_usize)];
    while let Some((node, depth)) = pending.pop() {
        rows.push(VisibleRow { node, depth });
        if expanded.get(node).copied().unwrap_or(false) {
            pending.extend(
                tree.children(node)
                    .iter()
                    .rev()
                    .map(|&child| (child, depth + 1)),
            );
        }
    }
    rows
}

/// 选中节点在可见行中的位置。
pub fn row_of(rows: &[VisibleRow], node: usize) -> Option<usize> {
    rows.iter().position(|row| row.node == node)
}

/// 从当前选中移动 `delta` 行（负值向上）。
///
/// 没有选中时，向下从第一行开始，向上从最后一行开始。返回值是节点索引。
pub fn move_selection(rows: &[VisibleRow], selected: Option<usize>, delta: isize) -> Option<usize> {
    if rows.is_empty() {
        return None;
    }
    let Some(selected) = selected else {
        return Some(match delta.is_negative() {
            true => rows.last()?.node,
            false => rows.first()?.node,
        });
    };
    let current = row_of(rows, selected)? as isize;
    let next = (current + delta).clamp(0, rows.len() as isize - 1) as usize;
    Some(rows[next].node)
}

pub fn first_node(rows: &[VisibleRow]) -> Option<usize> {
    rows.first().map(|row| row.node)
}

pub fn last_node(rows: &[VisibleRow]) -> Option<usize> {
    rows.last().map(|row| row.node)
}

/// 选中节点的父节点：向上最近、深度恰少一层的行。
pub fn parent_node(rows: &[VisibleRow], selected: usize) -> Option<usize> {
    let current = row_of(rows, selected)?;
    let depth = rows[current].depth;
    rows[..current]
        .iter()
        .rev()
        .find(|row| row.depth < depth)
        .map(|row| row.node)
}

/// 选中节点在展开状态下的第一个子节点。
pub fn first_child_node(rows: &[VisibleRow], selected: usize) -> Option<usize> {
    let current = row_of(rows, selected)?;
    let depth = rows[current].depth;
    rows.get(current + 1)
        .filter(|row| row.depth == depth + 1)
        .map(|row| row.node)
}

/// 源码字节偏移对应的 1-based 行、列（列按字符计）。
pub fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let mut line = 1;
    let mut column = 1;
    for ch in text[..offset].chars() {
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

/// 单行显示用的截断；保留完整值仍由各格式的源码范围负责。
pub fn truncated_chars(text: &str, max: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index == max {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

/// 结构行的一个源码定位；格式投影自己决定范围覆盖值、属性还是整行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLocation {
    pub range: Range<usize>,
    pub line: usize,
    pub column: usize,
}

impl SourceLocation {
    pub fn of(text: &str, range: Range<usize>) -> Self {
        let (line, column) = line_column(text, range.start);
        Self {
            range,
            line,
            column,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree {
        children: Vec<Vec<usize>>,
    }

    impl Treelike for Tree {
        fn root(&self) -> Option<usize> {
            Some(0)
        }

        fn children(&self, node: usize) -> &[usize] {
            &self.children[node]
        }
    }

    #[test]
    fn folding_keeps_parent_rows_and_navigation_crosses_visibly_open_children() {
        let tree = Tree {
            children: vec![vec![1, 4], vec![2, 3], vec![], vec![], vec![]],
        };
        let mut expanded = vec![false; 5];
        expanded[0] = true;
        expanded[1] = true;
        let rows = visible_rows(&tree, &expanded);
        assert_eq!(
            rows.iter()
                .map(|row| (row.node, row.depth))
                .collect::<Vec<_>>(),
            [(0, 0), (1, 1), (2, 2), (3, 2), (4, 1)]
        );
        assert_eq!(move_selection(&rows, Some(1), 1), Some(2));
        assert_eq!(parent_node(&rows, 2), Some(1));
        assert_eq!(first_child_node(&rows, 1), Some(2));
        expanded[1] = false;
        let rows = visible_rows(&tree, &expanded);
        assert_eq!(
            rows.iter().map(|row| row.node).collect::<Vec<_>>(),
            [0, 1, 4]
        );
        assert_eq!(move_selection(&rows, None, 1), Some(0));
        assert_eq!(move_selection(&rows, None, -1), Some(4));
    }

    #[test]
    fn source_location_is_one_based_and_truncation_is_character_safe() {
        assert_eq!(line_column("a\nbc\n", 3), (2, 2));
        assert_eq!(line_column("中文\nx", 7), (2, 1));
        assert_eq!(truncated_chars("中文字", 2), "中文…");
    }
}
