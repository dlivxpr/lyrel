//! Native solid Powerline separators and their device-pixel closing edges.

use gpui_kit::{Bounds, Pixels, Point, point, px};

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum PowerlineShape {
    TriangleRight,
    TriangleLeft,
    HalfCircleRight,
    HalfCircleLeft,
    SlantLowerLeft,
    SlantLowerRight,
    SlantUpperLeft,
    SlantUpperRight,
}

impl PowerlineShape {
    pub(super) fn of(c: char) -> Option<Self> {
        Some(match c {
            '\u{e0b0}' => Self::TriangleRight,
            '\u{e0b2}' => Self::TriangleLeft,
            '\u{e0b4}' => Self::HalfCircleRight,
            '\u{e0b6}' => Self::HalfCircleLeft,
            '\u{e0b8}' => Self::SlantLowerLeft,
            '\u{e0ba}' => Self::SlantLowerRight,
            '\u{e0bc}' => Self::SlantUpperLeft,
            '\u{e0be}' => Self::SlantUpperRight,
            _ => return None,
        })
    }

    fn closing_edge_x(self, bounds: Bounds<Pixels>) -> Pixels {
        // Keep this match exhaustive so adding a shape cannot silently assign
        // its solid closing edge to the wrong side of the cell.
        match self {
            Self::TriangleRight
            | Self::HalfCircleRight
            | Self::SlantLowerLeft
            | Self::SlantUpperLeft => bounds.left(),
            Self::TriangleLeft
            | Self::HalfCircleLeft
            | Self::SlantLowerRight
            | Self::SlantUpperRight => bounds.right(),
        }
    }
}

pub(super) fn powerline_path(
    bounds: Bounds<Pixels>,
    shape: PowerlineShape,
) -> gpui_kit::Path<Pixels> {
    let (x0, y0) = (bounds.origin.x, bounds.origin.y);
    let (x1, y1) = (x0 + bounds.size.width, y0 + bounds.size.height);
    let ymid = y0 + bounds.size.height / 2.;

    let tri = |a: Point<Pixels>, b: Point<Pixels>, c: Point<Pixels>| {
        let mut p = gpui_kit::Path::new(a);
        p.line_to(b);
        p.line_to(c);
        p
    };
    let half_circle = |anchor_x: Pixels, dir: f32| {
        const SEGS: usize = 12;

        let (rx, ry) = (bounds.size.width.as_f32(), bounds.size.height.as_f32() / 2.);

        let at = |scale: f32, theta: f32| {
            let x = anchor_x.as_f32() + dir * rx * scale * theta.cos();
            let y = ymid.as_f32() + ry * scale * theta.sin();
            point(
                px(x.clamp(x0.as_f32(), x1.as_f32())),
                px(y.clamp(y0.as_f32(), y1.as_f32())),
            )
        };

        let step = std::f32::consts::PI / SEGS as f32;
        let mut p = gpui_kit::Path::new(at(1., -std::f32::consts::FRAC_PI_2));
        for i in 0..SEGS {
            let t0 = -std::f32::consts::FRAC_PI_2 + step * i as f32;
            let t1 = t0 + step;
            let ctrl = at(1. / (step / 2.).cos(), (t0 + t1) / 2.);
            p.curve_to(at(1., t1), ctrl);
        }
        p
    };
    match shape {
        PowerlineShape::TriangleRight => tri(point(x0, y0), point(x1, ymid), point(x0, y1)),
        PowerlineShape::TriangleLeft => tri(point(x1, y0), point(x0, ymid), point(x1, y1)),
        PowerlineShape::SlantLowerLeft => tri(point(x0, y0), point(x1, y1), point(x0, y1)),
        PowerlineShape::SlantLowerRight => tri(point(x1, y0), point(x1, y1), point(x0, y1)),
        PowerlineShape::SlantUpperLeft => tri(point(x0, y0), point(x1, y0), point(x0, y1)),
        PowerlineShape::SlantUpperRight => tri(point(x0, y0), point(x1, y0), point(x1, y1)),
        PowerlineShape::HalfCircleRight => half_circle(x0, 1.),
        PowerlineShape::HalfCircleLeft => half_circle(x1, -1.),
    }
}

pub(super) fn powerline_solid_edge(
    bounds: Bounds<Pixels>,
    shape: PowerlineShape,
    scale_factor: f32,
    fg_alpha: f32,
) -> Option<Bounds<Pixels>> {
    // A filled path anti-aliases a closing edge that falls between device
    // pixels. Cover exactly that partially occupied pixel with opaque
    // foreground color; an already aligned edge needs no extra primitive.
    //
    // Only opaque separators qualify. A translucent one (DIM) would composite
    // the cover quad and the path on top of each other, pushing that single
    // column past the glyph's own alpha and tinting the neighboring cell.
    if !fg_alpha.is_finite() || fg_alpha < 1. {
        return None;
    }
    let scale_factor = if scale_factor.is_finite() && scale_factor > 0. {
        scale_factor
    } else {
        1.
    };
    let physical_edge = shape.closing_edge_x(bounds).as_f32() * scale_factor;
    let physical_left = physical_edge.floor();
    let physical_right = physical_edge.ceil();
    if physical_left == physical_right {
        return None;
    }

    Some(Bounds::from_corners(
        point(px(physical_left / scale_factor), bounds.top()),
        point(px(physical_right / scale_factor), bounds.bottom()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::element::DIM_OPACITY;
    use gpui_kit::size;

    #[test]
    fn powerline_shape_maps_only_the_solid_separators() {
        for (c, shape) in [
            ('\u{e0b0}', PowerlineShape::TriangleRight),
            ('\u{e0b2}', PowerlineShape::TriangleLeft),
            ('\u{e0b4}', PowerlineShape::HalfCircleRight),
            ('\u{e0b6}', PowerlineShape::HalfCircleLeft),
            ('\u{e0b8}', PowerlineShape::SlantLowerLeft),
            ('\u{e0ba}', PowerlineShape::SlantLowerRight),
            ('\u{e0bc}', PowerlineShape::SlantUpperLeft),
            ('\u{e0be}', PowerlineShape::SlantUpperRight),
        ] {
            assert_eq!(PowerlineShape::of(c), Some(shape), "U+{:04X}", c as u32);
        }
        for c in [
            '\u{e0b1}', '\u{e0b3}', '\u{e0b5}', '\u{e0b7}', '\u{e0b9}', '\u{e0bb}', '\u{e0bd}',
            '\u{e0bf}', '\u{e0a0}', '\u{e0c0}', '\u{2500}', '❯', '➜',
        ] {
            assert_eq!(PowerlineShape::of(c), None, "U+{:04X}", c as u32);
        }
    }

    #[test]
    fn powerline_path_fills_exactly_one_cell() {
        let (x0, y0, w, h) = (px(10.), px(20.), px(9.), px(21.));
        let bounds = Bounds::new(point(x0, y0), size(w, h));
        for shape in [
            PowerlineShape::TriangleRight,
            PowerlineShape::TriangleLeft,
            PowerlineShape::HalfCircleRight,
            PowerlineShape::HalfCircleLeft,
            PowerlineShape::SlantLowerLeft,
            PowerlineShape::SlantLowerRight,
            PowerlineShape::SlantUpperLeft,
            PowerlineShape::SlantUpperRight,
        ] {
            let path = powerline_path(bounds, shape);
            assert!(!path.vertices.is_empty(), "{shape:?} produced no geometry");
            let (mut min_x, mut max_x) = (px(f32::MAX), px(f32::MIN));
            for v in &path.vertices {
                let p = v.xy_position;
                assert!(
                    p.x >= x0 && p.x <= x0 + w && p.y >= y0 && p.y <= y0 + h,
                    "{shape:?} vertex at {p:?} escapes the cell"
                );
                min_x = min_x.min(p.x);
                max_x = max_x.max(p.x);
            }
            assert_eq!(min_x, x0, "{shape:?} does not reach the left cell edge");
            assert_eq!(
                max_x,
                x0 + w,
                "{shape:?} does not reach the right cell edge"
            );
            match shape {
                PowerlineShape::HalfCircleRight => assert!(
                    path.vertices[1].xy_position.x <= x0 + w * 0.4
                        && path.vertices[path.vertices.len() - 2].xy_position.x <= x0 + w * 0.4,
                    "right caps should not collapse into a diagonal wedge"
                ),
                PowerlineShape::HalfCircleLeft => assert!(
                    path.vertices[1].xy_position.x >= x0 + w * 0.6
                        && path.vertices[path.vertices.len() - 2].xy_position.x >= x0 + w * 0.6,
                    "left caps should not collapse into a diagonal wedge"
                ),
                _ => {}
            }
        }
    }

    #[test]
    fn powerline_solid_edge_covers_only_the_fractional_device_pixel() {
        let bounds = Bounds::new(point(px(10.2), px(20.)), size(px(9.2), px(21.)));
        let scale = 1.25;

        for shape in [
            PowerlineShape::TriangleRight,
            PowerlineShape::HalfCircleRight,
            PowerlineShape::SlantLowerLeft,
            PowerlineShape::SlantUpperLeft,
        ] {
            assert_eq!(
                powerline_solid_edge(bounds, shape, scale, 1.),
                Some(Bounds::from_corners(
                    point(px(12. / scale), bounds.top()),
                    point(px(13. / scale), bounds.bottom()),
                )),
                "{shape:?} must cover only the device pixel containing its left edge"
            );
        }

        for shape in [
            PowerlineShape::TriangleLeft,
            PowerlineShape::HalfCircleLeft,
            PowerlineShape::SlantLowerRight,
            PowerlineShape::SlantUpperRight,
        ] {
            assert_eq!(
                powerline_solid_edge(bounds, shape, scale, 1.),
                Some(Bounds::from_corners(
                    point(px(24. / scale), bounds.top()),
                    point(px(25. / scale), bounds.bottom()),
                )),
                "{shape:?} must cover only the device pixel containing its right edge"
            );
        }
    }

    #[test]
    fn powerline_solid_edge_skips_device_aligned_edges() {
        let bounds = Bounds::new(point(px(10.), px(20.)), size(px(9.), px(21.)));

        for shape in [
            PowerlineShape::TriangleRight,
            PowerlineShape::TriangleLeft,
            PowerlineShape::HalfCircleRight,
            PowerlineShape::HalfCircleLeft,
            PowerlineShape::SlantLowerLeft,
            PowerlineShape::SlantLowerRight,
            PowerlineShape::SlantUpperLeft,
            PowerlineShape::SlantUpperRight,
        ] {
            assert_eq!(
                powerline_solid_edge(bounds, shape, 2., 1.),
                None,
                "{shape:?}"
            );
        }
    }

    #[test]
    fn powerline_solid_edge_sanitizes_invalid_scale() {
        let bounds = Bounds::new(point(px(10.25), px(20.)), size(px(9.), px(21.)));
        let expected = Some(Bounds::from_corners(
            point(px(10.), bounds.top()),
            point(px(11.), bounds.bottom()),
        ));

        for scale in [0., -1., f32::NAN] {
            assert_eq!(
                powerline_solid_edge(bounds, PowerlineShape::TriangleRight, scale, 1.),
                expected
            );
        }
    }

    #[test]
    fn powerline_solid_edge_skips_translucent_separators() {
        let bounds = Bounds::new(point(px(10.2), px(20.)), size(px(9.2), px(21.)));

        for shape in [
            PowerlineShape::TriangleRight,
            PowerlineShape::TriangleLeft,
            PowerlineShape::HalfCircleRight,
            PowerlineShape::HalfCircleLeft,
            PowerlineShape::SlantLowerLeft,
            PowerlineShape::SlantLowerRight,
            PowerlineShape::SlantUpperLeft,
            PowerlineShape::SlantUpperRight,
        ] {
            assert!(
                powerline_solid_edge(bounds, shape, 1.25, 1.).is_some(),
                "{shape:?} still needs the cover quad when opaque"
            );
            for alpha in [DIM_OPACITY, 0., 0.99, f32::NAN] {
                assert_eq!(
                    powerline_solid_edge(bounds, shape, 1.25, alpha),
                    None,
                    "{shape:?} must not stack a cover quad under a translucent path"
                );
            }
        }
    }
}
