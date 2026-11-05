//! Same-repository pull-request stacks, top above base, gathered at the first
//! member's source position. Semantic port of tty7 6e3d2a414d2ea0de3ea1628480c5bce6e932b85e
//! (Apache-2.0). The source list stays intact: these indices are presentation
//! coordinates, never selection identity. Compute once when a list changes.

use std::collections::HashMap;

use super::{Item, PullRefs, RepoSlug};

/// One lane of the connector column; siblings have separate lanes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    Empty,
    Pipe,
    Node {
        up: bool,
        down: bool,
    },
    /// A side lane joins the row's node from the right.
    Join,
}

/// A presentation row over an unchanged source list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StackRow {
    Item {
        source_ix: usize,
        cells: Vec<Cell>,
    },
    /// The base branch of the item at this source index. Not a selectable PR.
    Trunk {
        source_ix: usize,
    },
}

/// Full repository identity, not `owner:branch`: one owner can own multiple
/// repositories, and a deleted/omitted repository cannot prove membership.
fn local_refs<'a>(item: &'a Item, repository: &str) -> Option<&'a PullRefs> {
    let refs = item.is_pr.then_some(item.pull.as_ref()).flatten()?;
    let local = |name: Option<&str>| name.is_some_and(|n| n.eq_ignore_ascii_case(repository));
    (local(refs.head_repository.as_deref())
        && local(refs.base_repository.as_deref())
        && !refs.head_ref.is_empty()
        && !refs.base_ref.is_empty())
    .then_some(refs)
}

/// Gather each connected stack while preserving the order of unrelated rows.
/// A missing head is a root; duplicate heads use the first listed PR. Walks
/// visit every row once, including cycles; a cyclic component has no trunk.
/// Branch names are case-sensitive, repository names are case-insensitive.
pub fn stack_order(items: &[Item], repository: &RepoSlug) -> Vec<StackRow> {
    let repository = repository.full();
    let refs = |ix: usize| local_refs(&items[ix], &repository);
    let mut by_head = HashMap::with_capacity(items.len());
    for ix in 0..items.len() {
        if let Some(pull) = refs(ix) {
            by_head.entry(pull.head_ref.as_str()).or_insert(ix);
        }
    }
    let parent: Vec<Option<usize>> = (0..items.len())
        .map(|ix| {
            refs(ix)
                .and_then(|pull| by_head.get(pull.base_ref.as_str()).copied())
                .filter(|&p| p != ix)
        })
        .collect();
    let mut children = vec![Vec::new(); items.len()];
    for (ix, parent) in parent.iter().enumerate() {
        if let Some(parent) = *parent {
            children[parent].push(ix);
        }
    }

    let mut seen = vec![false; items.len()];
    let mut from: Vec<Option<usize>> = vec![None; items.len()];
    let mut lane = vec![0; items.len()];
    let mut heir_taken = vec![false; items.len()];
    let mut groups = Vec::new();
    let starts = (0..items.len())
        .filter(|&ix| parent[ix].is_none())
        .chain(0..items.len());
    for start in starts {
        if seen[start] {
            continue;
        }
        let mut group = Vec::new();
        let mut lanes = 1;
        let mut todo = vec![(start, None)];
        while let Some((ix, via)) = todo.pop() {
            if std::mem::replace(&mut seen[ix], true) {
                continue;
            }
            from[ix] = via;
            lane[ix] = match via {
                Some(p) if !std::mem::replace(&mut heir_taken[p], true) => lane[p],
                Some(_) => {
                    lanes += 1;
                    lanes - 1
                }
                None => 0,
            };
            group.push(ix);
            todo.extend(children[ix].iter().rev().map(|&child| (child, Some(ix))));
        }
        // Reverse preorder puts every child above its base without recursion.
        group.reverse();
        groups.push((group, lanes));
    }
    groups.sort_by_key(|(group, _)| group.iter().min().copied());

    let mut out = Vec::with_capacity(items.len() + groups.len());
    let mut row_of = vec![0; items.len()];
    for (group, lanes) in groups {
        let bottom = group[group.len() - 1];
        let trunk = group.len() > 1 && parent[bottom].is_none();
        let mut cells = vec![vec![Cell::Empty; lanes]; group.len()];
        for (row, &ix) in group.iter().enumerate() {
            row_of[ix] = row;
            cells[row][lane[ix]] = Cell::Node {
                up: heir_taken[ix],
                down: from[ix].is_some() || trunk,
            };
        }
        for (row, &ix) in group.iter().enumerate() {
            let Some(p) = from[ix].filter(|&p| lane[p] != lane[ix]) else {
                continue;
            };
            for cell_row in &mut cells[row + 1..row_of[p]] {
                cell_row[lane[ix]] = Cell::Pipe;
            }
            cells[row_of[p]][lane[ix]] = Cell::Join;
        }
        out.extend(
            group
                .into_iter()
                .zip(cells)
                .map(|(source_ix, cells)| StackRow::Item { source_ix, cells }),
        );
        if trunk {
            out.push(StackRow::Trunk { source_ix: bottom });
        }
    }
    out
}
