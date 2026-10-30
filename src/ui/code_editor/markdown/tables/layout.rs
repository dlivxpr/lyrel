use super::{
    super::projection::{Kind, Leaf},
    Cell,
};
use crate::ui::code_editor::markdown::math;
use gpui_kit::base::{
    StyledExt as _,
    text::editor_host::{BlockNode, FlowPosition, Paragraph, RenderContext},
};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::{collections::HashMap, ops::Range};

#[derive(Default)]
pub(crate) struct Layout {
    cells: HashMap<u64, Measurement>,
    columns: HashMap<usize, Vec<Pixels>>,
    table_ids: HashMap<usize, u64>,
    scrolls: HashMap<u64, ScrollHandle>,
    style: Option<(SharedString, SharedString, Pixels, bool)>,
    generation: u64,
}
struct Measurement {
    version: u64,
    seen: u64,
    intrinsic: Pixels,
    wrapped: Option<(Pixels, Pixels)>,
}
impl Layout {
    pub fn invalidate(&mut self) {
        self.cells.clear();
    }

    pub fn invalidate_cell(&mut self, id: u64) {
        self.cells.remove(&id);
    }

    // Measurements contain numbers only. In particular, measuring an offscreen
    // cell must not keep its Paragraph or a GPUI retained layout alive.
    pub fn prepare(
        &mut self,
        leaves: &[Leaf],
        width: Pixels,
        wrap: bool,
        measure: bool,
        marked: Option<&Range<usize>>,
        math: &math::State,
        context: &RenderContext,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let style = (
            cx.theme().font_family.clone(),
            cx.theme().mono_font_family.clone(),
            window.rem_size(),
            wrap,
        );
        if self.style.as_ref() != Some(&style) {
            self.invalidate();
            self.style = Some(style);
        }
        let mut columns: HashMap<usize, Vec<Pixels>> = HashMap::new();
        self.generation += 1;
        self.table_ids.clear();
        for leaf in leaves {
            let Kind::TableCell(cell) = leaf.kind else {
                continue;
            };
            let measurement = self.cells.entry(leaf.id).or_insert(Measurement {
                version: u64::MAX,
                seen: self.generation,
                intrinsic: Pixels::ZERO,
                wrapped: None,
            });
            if measure && measurement.version != leaf.version {
                let mut element = cell_element(leaf, None, wrap, marked, math, context, window, cx);
                measurement.intrinsic = element
                    .layout_as_root(
                        size(AvailableSpace::MaxContent, AvailableSpace::MinContent),
                        window,
                        cx,
                    )
                    .width;
                measurement.version = leaf.version;
                measurement.wrapped = None;
            }
            measurement.seen = self.generation;
            let id = *self.table_ids.entry(cell.start).or_insert(leaf.id);
            self.scrolls.entry(id).or_default();
            let widths = columns.entry(cell.start).or_default();
            widths.resize(widths.len().max(cell.column + 1), window.rem_size() * 3.);
            widths[cell.column] = widths[cell.column].max(measurement.intrinsic);
        }
        self.cells.retain(|_, cell| cell.seen == self.generation);
        let live: std::collections::HashSet<_> = self.table_ids.values().copied().collect();
        self.scrolls.retain(|id, _| live.contains(id));
        for widths in columns.values_mut() {
            if wrap {
                let share = width / widths.len().max(1) as f32;
                for column in widths.iter_mut() {
                    let floor = (*column / 2.)
                        .clamp(window.rem_size() * 8., window.rem_size() * 24.)
                        .min(*column);
                    *column = (*column).min(share.max(floor));
                }
            }
            let total = widths.iter().copied().fold(Pixels::ZERO, |a, b| a + b);
            let extra = (width - total).max(Pixels::ZERO) / widths.len().max(1) as f32;
            for column in widths {
                *column += extra;
            }
        }
        let changed = self.columns != columns;
        self.columns = columns;
        changed
    }

    pub fn column_bounds(&self, cell: Cell) -> (Pixels, Pixels) {
        let widths = &self.columns[&cell.start];
        let left = widths[..cell.column]
            .iter()
            .copied()
            .fold(Pixels::ZERO, |a, b| a + b);
        (left, left + widths[cell.column])
    }

    pub fn scroll(&self, cell: Cell) -> &ScrollHandle {
        &self.scrolls[&self.table_ids[&cell.start]]
    }

    pub fn reveal(&self, cell: Cell, width: Pixels) {
        let (left, right) = self.column_bounds(cell);
        let scroll = self.scroll(cell);
        let x = -scroll.offset().x;
        let target = if left < x {
            left
        } else if right > x + width {
            right - width
        } else {
            x
        };
        scroll.set_offset(point(-target, Pixels::ZERO));
    }

    pub fn measure_rows(
        &mut self,
        leaves: &[Leaf],
        wrap: bool,
        marked: Option<&Range<usize>>,
        math: &math::State,
        context: &RenderContext,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let mut changed = false;
        for leaf in leaves {
            let Kind::TableCell(cell) = leaf.kind else {
                continue;
            };
            let width = self.columns[&cell.start][cell.column];
            let measurement = self.cells.get_mut(&leaf.id).expect("prepared table cell");
            if measurement.wrapped.is_some_and(|(at, _)| at == width) {
                continue;
            }
            let mut element =
                cell_element(leaf, Some(width), wrap, marked, math, context, window, cx);
            let height = element
                .layout_as_root(
                    size(AvailableSpace::Definite(width), AvailableSpace::MinContent),
                    window,
                    cx,
                )
                .height;
            changed |= measurement.wrapped != Some((width, height));
            measurement.wrapped = Some((width, height));
        }
        changed
    }

    pub fn row(
        &mut self,
        leaves: &[Leaf],
        viewport_x: Pixels,
        viewport_width: Pixels,
        wrap: bool,
        marked: Option<&Range<usize>>,
        math: &math::State,
        context: &RenderContext,
        projection: &super::super::projection::Projection,
        source: &gpui_kit::base::input::Rope,
        mdx_state: &super::super::mdx::ViewState,
        window: &mut Window,
        cx: &mut Context<super::super::SourcePresentation>,
    ) -> (Div, Vec<Option<u64>>) {
        let Kind::TableCell(first) = leaves[0].kind else {
            unreachable!()
        };
        let widths = &self.columns[&first.start];
        let mut height = window.rem_size();
        for leaf in leaves {
            if let Some((_, measured)) = self.cells.get(&leaf.id).and_then(|cell| cell.wrapped) {
                height = height.max(measured);
            }
        }
        let mut x = Pixels::ZERO;
        let mut children = Vec::new();
        let mut ids = Vec::new();
        let mut blank = Pixels::ZERO;
        for (column, width) in widths.iter().copied().enumerate() {
            let visible = x + width >= viewport_x && x <= viewport_x + viewport_width;
            x += width;
            if !visible {
                blank += width;
                continue;
            }
            if blank > Pixels::ZERO {
                children.push(div().w(blank).flex_none().into_any_element());
                ids.push(None);
                blank = Pixels::ZERO;
            }
            let leaf = leaves
                .iter()
                .find(|leaf| matches!(leaf.kind, Kind::TableCell(cell) if cell.column == column));
            children.push(match leaf {
                Some(leaf) => {
                    let field = super::super::mdx::view::FieldContext::new(
                        projection, leaf, source, mdx_state,
                    );
                    if super::super::mdx::view::visible_field(&field) {
                        let content = cell_element(
                            leaf,
                            Some(width),
                            wrap,
                            marked,
                            math,
                            context,
                            window,
                            cx,
                        );
                        div()
                            .w(width)
                            .min_w_0()
                            .flex_none()
                            .child(super::super::mdx::view::render_field(
                                &field, content, window, cx,
                            ))
                            .into_any_element()
                    } else {
                        div().w(width).flex_none().into_any_element()
                    }
                }
                None => div().w(width).flex_none().into_any_element(),
            });
            ids.push(leaf.map(|leaf| leaf.id));
        }
        if blank > Pixels::ZERO {
            children.push(div().w(blank).flex_none().into_any_element());
            ids.push(None);
        }
        let row = div()
            .flex()
            .flex_row()
            .items_stretch()
            .flex_none()
            .w(x)
            .min_h(height)
            .border_b_1()
            .border_color(cx.theme().border)
            .when(first.row == 0, |row| row.bg(cx.theme().muted))
            .children(children);
        (row, ids)
    }
}

fn content(
    leaf: &Leaf,
    marked: Option<&Range<usize>>,
    math: &math::State,
    context: &RenderContext,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let mut paragraph = Paragraph::default().with_leaf_key(leaf.id);
    for node in leaf.inline_nodes(marked, math) {
        paragraph.push(node);
    }
    let node = BlockNode::Paragraph(paragraph);
    let references = [&node];
    node.render_with_id(
        ("markdown-table-cell", leaf.id),
        FlowPosition::at(&references, 0, true)
            .first(true)
            .last(true),
        context,
        window,
        cx,
    )
    .into_any_element()
}

fn cell_element(
    leaf: &Leaf,
    width: Option<Pixels>,
    wrap: bool,
    marked: Option<&Range<usize>>,
    math: &math::State,
    context: &RenderContext,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Kind::TableCell(cell) = leaf.kind else {
        unreachable!()
    };
    div()
        .flex_none()
        .when_some(width, |cell, width| cell.w(width))
        .min_w_0()
        .px_3()
        .py_2()
        .border_r_1()
        .border_color(cx.theme().border)
        .when(cell.row == 0, |cell| cell.font_semibold())
        .when(cell.align == markdown::mdast::AlignKind::Center, |cell| {
            cell.text_center()
        })
        .when(cell.align == markdown::mdast::AlignKind::Right, |cell| {
            cell.text_right()
        })
        .when(wrap, |cell| cell.whitespace_normal())
        .when(!wrap, |cell| cell.whitespace_nowrap())
        .child(content(leaf, marked, math, context, window, cx))
        .into_any_element()
}
