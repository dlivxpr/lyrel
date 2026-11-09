use super::metadata::{self, Marker, Style};
use super::{
    super::projection::{Kind, Leaf},
    Cell,
};
use crate::ui::code_editor::markdown::math;
use crate::ui::i18n::{L10nKey, t};
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
    scrolls: HashMap<u64, TableScroll>,
    metadata: HashMap<usize, Marker>,
    directions: HashMap<usize, bool>,
    viewport_width: Pixels,
    revision: Option<u64>,
    resize: Option<Resize>,
    style: Option<(SharedString, SharedString, Pixels, Pixels, Pixels, bool)>,
    generation: u64,
    geometry_generation: u64,
}
struct Measurement {
    version: u64,
    seen: u64,
    intrinsic: Pixels,
    wrapped: Option<(Pixels, Pixels)>,
}

#[derive(Default)]
struct TableScroll {
    handle: ScrollHandle,
    total: Pixels,
}

struct Resize {
    cell: Cell,
    revision: u64,
    origin: Pixels,
    widths: Vec<Pixels>,
    original: Pixels,
}
impl Layout {
    pub fn invalidate(&mut self) {
        self.cells.clear();
    }

    pub fn invalidate_cell(&mut self, id: u64) {
        self.cells.remove(&id);
    }

    pub fn invalidate_metadata(&mut self) {
        self.revision = None;
    }

    // Measurements contain numbers only. In particular, measuring an offscreen
    // cell must not keep its Paragraph or a GPUI retained layout alive.
    pub fn prepare(
        &mut self,
        leaves: &[Leaf],
        source: &super::super::Rope,
        revision: u64,
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
            cx.theme().font_size,
            cx.theme().mono_font_size,
            wrap,
        );
        if self.style.as_ref() != Some(&style) {
            self.invalidate();
            self.style = Some(style);
        }
        let mut direction_changed = false;
        if self.revision != Some(revision) {
            self.metadata.clear();
            let mut directions = HashMap::new();
            for leaf in leaves {
                if let Kind::TableCell(cell) = leaf.kind
                    && !directions.contains_key(&cell.start)
                    && let Some(rtl) =
                        leaf.text
                            .chars()
                            .find_map(|ch| match unicode_bidi::bidi_class(ch) {
                                unicode_bidi::BidiClass::R | unicode_bidi::BidiClass::AL => {
                                    Some(true)
                                }
                                unicode_bidi::BidiClass::L => Some(false),
                                _ => None,
                            })
                {
                    directions.insert(cell.start, rtl);
                }
            }
            direction_changed = self.directions != directions;
            self.directions = directions;
            for leaf in leaves {
                if let Kind::TableCell(cell) = leaf.kind
                    && cell.row == 0
                    && cell.column == 0
                    && let Some(marker) = metadata::read(source, cell.start)
                {
                    self.metadata.insert(cell.start, marker);
                }
            }
            self.revision = Some(revision);
            if self
                .resize
                .as_ref()
                .is_some_and(|resize| resize.revision != revision)
            {
                self.resize = None;
            }
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
                let style = self
                    .metadata
                    .get(&cell.start)
                    .map_or(Style::Grid, |marker| marker.style);
                let rtl = self.directions.get(&cell.start).copied().unwrap_or(false);
                let mut element = cell_element(
                    leaf, None, wrap, style, rtl, marked, math, context, window, cx,
                );
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
            // Automatic columns may grow to the viewport, but a long cell must
            // wrap before its intrinsic width makes that viewport unreachable.
            // Explicit widths and FitColumn below remain authoritative.
            let intrinsic = if wrap {
                measurement.intrinsic.min(width.max(window.rem_size() * 3.))
            } else {
                measurement.intrinsic
            };
            widths[cell.column] = widths[cell.column].max(intrinsic);
        }
        self.cells.retain(|_, cell| cell.seen == self.generation);
        let live: std::collections::HashSet<_> = self.table_ids.values().copied().collect();
        self.scrolls.retain(|id, _| live.contains(id));
        for (start, widths) in &mut columns {
            if let Some(marker) = self.metadata.get(start) {
                for (column, explicit) in widths.iter_mut().zip(&marker.widths) {
                    *column = window.rem_size() * (explicit.max(24.) / 16.);
                }
            }
            if let Some(resize) = self
                .resize
                .as_ref()
                .filter(|resize| resize.cell.start == *start)
            {
                widths.clone_from(&resize.widths);
            }
            let scroll = self
                .scrolls
                .get_mut(&self.table_ids[start])
                .expect("prepared table scroll");
            let total = widths.iter().copied().sum::<Pixels>();
            let previous = std::mem::replace(&mut scroll.total, total);
            if self.directions.get(start) == Some(&true) {
                let previous_max = (previous - self.viewport_width).max(Pixels::ZERO);
                let maximum = (total - width).max(Pixels::ZERO);
                let offset = (scroll.handle.offset().x + previous_max - maximum)
                    .clamp(-maximum, Pixels::ZERO);
                if offset != scroll.handle.offset().x {
                    scroll.handle.set_offset(point(offset, Pixels::ZERO));
                }
            }
        }
        let changed = self.columns != columns || direction_changed;
        if changed {
            self.geometry_generation = self.geometry_generation.wrapping_add(1);
        }
        self.columns = columns;
        self.viewport_width = width;
        changed
    }

    pub fn column_bounds(&self, cell: Cell) -> (Pixels, Pixels) {
        let widths = &self.columns[&cell.start];
        let left = widths[..cell.column]
            .iter()
            .copied()
            .fold(Pixels::ZERO, |a, b| a + b);
        let left = if self.directions.get(&cell.start) == Some(&true) {
            widths.iter().copied().sum::<Pixels>() - left - widths[cell.column]
        } else {
            left
        };
        (left, left + widths[cell.column])
    }

    pub(crate) fn is_rtl(&self, cell: Cell) -> bool {
        self.directions.get(&cell.start).copied().unwrap_or(false)
    }

    pub fn scroll(&self, cell: Cell) -> &ScrollHandle {
        &self.scrolls[&self.table_ids[&cell.start]].handle
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

    pub(crate) fn style(&self, cell: Cell) -> Style {
        self.metadata
            .get(&cell.start)
            .map_or(Style::Grid, |marker| marker.style)
    }

    pub(crate) fn alignment(&self, cell: Cell) -> markdown::mdast::AlignKind {
        cell.align
    }

    pub(crate) fn geometry_state(&self, cell: Cell) -> Option<(u64, Point<Pixels>, u64)> {
        let id = *self.table_ids.get(&cell.start)?;
        Some((
            id,
            self.scrolls.get(&id)?.handle.offset(),
            self.geometry_generation,
        ))
    }

    pub(crate) fn widths(&self, cell: Cell, rem: Pixels) -> Option<Vec<f32>> {
        Some(
            self.columns
                .get(&cell.start)?
                .iter()
                .map(|width| f32::from(*width / rem) * 16.)
                .collect(),
        )
    }

    pub(crate) fn natural_width(&self, leaves: &[Leaf], cell: Cell, rem: Pixels) -> f32 {
        leaves.iter().filter(|leaf| matches!(leaf.kind, Kind::TableCell(other) if other.start == cell.start && other.column == cell.column))
            .filter_map(|leaf| self.cells.get(&leaf.id).map(|measurement| measurement.intrinsic))
            .fold(rem * 3., |width, measured| width.max(measured)) / rem * 16.
    }

    pub(crate) fn begin_resize(&mut self, cell: Cell, revision: u64, origin: Pixels) -> bool {
        let Some(widths) = self.columns.get(&cell.start).cloned() else {
            return false;
        };
        let Some(&original) = widths.get(cell.column) else {
            return false;
        };
        self.resize = Some(Resize {
            cell,
            revision,
            origin,
            original,
            widths,
        });
        true
    }

    pub(crate) fn is_resizing(&self) -> bool {
        self.resize.is_some()
    }

    pub(crate) fn resize_to(&mut self, position: Pixels, rem: Pixels) {
        if let Some(resize) = &mut self.resize {
            let delta = position - resize.origin;
            let delta = if self.directions.get(&resize.cell.start) == Some(&true) {
                -delta
            } else {
                delta
            };
            let width = (resize.original + delta).max(rem * 1.5);
            if resize.widths[resize.cell.column] != width {
                resize.widths[resize.cell.column] = width;
                self.geometry_generation = self.geometry_generation.wrapping_add(1);
            }
        }
    }

    pub(crate) fn finish_resize(&mut self, rem: Pixels) -> Option<(Cell, u64, Vec<f32>)> {
        let resize = self.resize.take()?;
        if resize.widths[resize.cell.column] == resize.original {
            return None;
        }
        Some((
            resize.cell,
            resize.revision,
            resize
                .widths
                .into_iter()
                .map(|width| f32::from(width / rem) * 16.)
                .collect(),
        ))
    }

    pub(crate) fn cancel_resize(&mut self) {
        if self.resize.take().is_some() {
            self.geometry_generation = self.geometry_generation.wrapping_add(1);
        }
    }

    pub(crate) fn adjusted_widths(
        &self,
        leaves: &[Leaf],
        cell: Cell,
        rem: Pixels,
        delta: Option<f32>,
    ) -> Option<Vec<f32>> {
        let mut widths = self.widths(cell, rem)?;
        widths[cell.column] = delta.map_or_else(
            || self.natural_width(leaves, cell, rem).ceil(),
            |delta| (widths[cell.column] + delta).max(24.),
        );
        Some(widths)
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
            let style = self.style(cell);
            let rtl = self.directions.get(&cell.start).copied().unwrap_or(false);
            let measurement = self.cells.get_mut(&leaf.id).expect("prepared table cell");
            if measurement.wrapped.is_some_and(|(at, _)| at == width) {
                continue;
            }
            let mut element = cell_element(
                leaf,
                Some(width),
                wrap,
                style,
                rtl,
                marked,
                math,
                context,
                window,
                cx,
            );
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
        let style = self.style(first);
        let rtl = self.directions.get(&first.start).copied().unwrap_or(false);
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
        for visual_column in 0..widths.len() {
            let column = if rtl {
                widths.len() - 1 - visual_column
            } else {
                visual_column
            };
            let width = widths[column];
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
                        let context = leaf.foreground_context(context);
                        let content = cell_element(
                            leaf,
                            Some(width),
                            wrap,
                            style,
                            rtl,
                            marked,
                            math,
                            &context,
                            window,
                            cx,
                        );
                        div()
                            .w(width)
                            .min_w_0()
                            .flex_none()
                            .relative()
                            .child(super::super::mdx::view::render_field(
                                &field, content, window, cx,
                            ))
                            .when(first.is_header(), |cell| {
                                let id = leaf.id;
                                cell.child(
                                    gpui_kit::base::resize_handle::<
                                        (),
                                        super::super::SourcePresentation,
                                    >(
                                        ("markdown-table-width", id), Axis::Horizontal
                                    )
                                    .inside(if rtl {
                                        gpui_kit::base::HandleEdge::Leading
                                    } else {
                                        gpui_kit::base::HandleEdge::Trailing
                                    })
                                    .role(Role::Splitter)
                                    .aria_label(t(L10nKey::EditorTableResizeColumn))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |view, event, window, cx| {
                                            view.start_table_resize(id, event, window, cx)
                                        }),
                                    ),
                                )
                            })
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
            .when(rtl, |row| row.ml_auto())
            .min_h(height)
            .when(
                style == Style::Grid
                    || (first.row == 0 && matches!(style, Style::Striped | Style::Minimal)),
                |row| row.border_b_1().border_color(cx.theme().border),
            )
            .when(
                (first.row == 0 && matches!(style, Style::Grid | Style::Header))
                    || (style == Style::Striped && first.row > 0 && first.row % 2 == 0),
                |row| row.bg(cx.theme().muted),
            )
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
    _wrap: bool,
    style: Style,
    rtl: bool,
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
        // Fixed reference tables.rs:1057 and element.rs:3007: 10px cell
        // inset and 6px vertical padding at 16px body, scaled with the document.
        .px(rems(10. / 16.))
        .py(rems(6. / 16.))
        .font_family(cx.theme().font_family.clone())
        .text_size(rems(1.))
        .line_height(relative(1.45))
        .when(style == Style::Grid, |cell| {
            cell.border_r_1().border_color(cx.theme().border)
        })
        .when(cell.row == 0, |cell| cell.font_semibold())
        .when(cell.align == markdown::mdast::AlignKind::Center, |cell| {
            cell.text_center()
        })
        .when(
            cell.align == markdown::mdast::AlignKind::Right
                || (rtl && cell.align == markdown::mdast::AlignKind::None),
            |cell| cell.text_right(),
        )
        // A chosen column width constrains its own cell independently of the
        // source/prose soft-wrap preference; text must not cross other cells.
        .whitespace_normal()
        .child(content(leaf, marked, math, context, window, cx))
        .into_any_element()
}
