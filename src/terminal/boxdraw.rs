use gpui_kit::{Bounds, Pixels, point, px, size};

pub(crate) enum Ink {
    Rect(Bounds<Pixels>),
    Shade(Bounds<Pixels>, f32),
    Path(gpui_kit::Path<Pixels>),
}

pub(crate) fn glyph(c: char, bounds: Bounds<Pixels>, scale: f32) -> Option<Vec<Ink>> {
    let math_delimiter = ('\u{239b}'..='\u{23ad}').contains(&c);
    if !math_delimiter && !('\u{2500}'..='\u{259f}').contains(&c) {
        return None;
    }
    let g = Cell::new(&bounds, scale);
    if math_delimiter {
        return Some(g.math_delimiter(c));
    }
    if let Some((u, d, l, r)) = arms_of(c) {
        return Some(g.arms(u, d, l, r));
    }
    g.doubles(c)
        .or_else(|| g.rounded(c))
        .or_else(|| g.dashed(c))
        .or_else(|| g.diagonal(c))
        .or_else(|| g.blocks(c))
}

#[derive(Clone, Copy, PartialEq)]
enum Arm {
    None,
    Light,
    Heavy,
}

struct Cell {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    cx: f32,
    cy: f32,
    t: f32,
    scale: f32,
}

fn light_thickness(cell_width: f32, scale: f32) -> f32 {
    let logical = (cell_width * 0.15).round().max(1.);
    (logical * scale).round().max(1.) / scale
}

impl Cell {
    fn new(b: &Bounds<Pixels>, scale: f32) -> Self {
        let x0 = b.origin.x.as_f32();
        let y0 = b.origin.y.as_f32();
        let x1 = x0 + b.size.width.as_f32();
        let y1 = y0 + b.size.height.as_f32();
        let scale = scale.max(0.1);
        Cell {
            x0,
            y0,
            x1,
            y1,
            cx: (x0 + x1) / 2.,
            cy: (y0 + y1) / 2.,
            t: light_thickness(x1 - x0, scale),
            scale,
        }
    }

    fn snap(&self, v: f32) -> f32 {
        (v * self.scale).round() / self.scale
    }

    fn rectb(&self, x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        let (sx0, sy0) = (self.snap(x), self.snap(y));
        let (sx1, sy1) = (self.snap(x + w), self.snap(y + h));
        Bounds::new(point(px(sx0), px(sy0)), size(px(sx1 - sx0), px(sy1 - sy0)))
    }

    fn rect(&self, x: f32, y: f32, w: f32, h: f32) -> Ink {
        Ink::Rect(self.rectb(x, y, w, h))
    }

    fn stroke_px(&self, w: f32) -> f32 {
        (w * self.scale).round().max(1.) / self.scale
    }

    fn vstroke(&self, x: f32, w: f32, ya: f32, yb: f32) -> Ink {
        let (x0, y0, y1) = (self.snap(x - w / 2.), self.snap(ya), self.snap(yb));
        Ink::Rect(Bounds::new(
            point(px(x0), px(y0)),
            size(px(self.stroke_px(w)), px(y1 - y0)),
        ))
    }

    fn hstroke(&self, y: f32, w: f32, xa: f32, xb: f32) -> Ink {
        let (y0, x0, x1) = (self.snap(y - w / 2.), self.snap(xa), self.snap(xb));
        Ink::Rect(Bounds::new(
            point(px(x0), px(y0)),
            size(px(x1 - x0), px(self.stroke_px(w))),
        ))
    }

    /// Unicode Technical delimiter pieces must meet at cell edges, not at font
    /// ascenders/descenders: otherwise terminal line spacing splits matrices.
    fn math_delimiter(&self, c: char) -> Vec<Ink> {
        if matches!(c, '⎜' | '⎟' | '⎢' | '⎥' | '⎪') {
            return vec![self.vstroke(self.cx, self.t, self.y0, self.y1)];
        }
        let right = matches!(c, '⎞' | '⎠' | '⎤' | '⎦' | '⎫' | '⎬' | '⎭');
        let lower = matches!(c, '⎝' | '⎠' | '⎣' | '⎦' | '⎩' | '⎭');
        let width = self.x1 - self.x0;
        let height = self.y1 - self.y0;
        let stem = self.snap(self.cx - self.t / 2.);
        let thickness = self.stroke_px(self.t);
        // Reflect whole pixel-aligned outlines, keeping the extension lane the
        // same for left and right pieces (including the shared brace extender).
        let at = |x: f32, y: f32| {
            point(
                px(if right { 2. * stem + thickness - x } else { x }),
                px(if lower { self.y0 + self.y1 - y } else { y }),
            )
        };
        let center = stem + thickness / 2.;
        let mut path = gpui_kit::Path::new(at(center, self.snap(self.y0)));
        // GPUI's path is a triangle fan, not an arbitrary polygon tessellator.
        // Separate convex strips keep a concave curved stroke from filling in.
        let curve = |path: &mut gpui_kit::Path<Pixels>,
                     from: (f32, f32),
                     control: (f32, f32),
                     to: (f32, f32)| {
            let edge = |t: f32| {
                let u = 1. - t;
                let x = u * u * from.0 + 2. * u * t * control.0 + t * t * to.0;
                let y = u * u * from.1 + 2. * u * t * control.1 + t * t * to.1;
                let dx = u * (control.0 - from.0) + t * (to.0 - control.0);
                let dy = u * (control.1 - from.1) + t * (to.1 - control.1);
                let half = thickness / (2. * dx.hypot(dy).max(f32::EPSILON));
                (
                    at(x - dy * half, y + dx * half),
                    at(x + dy * half, y - dx * half),
                )
            };
            let mut previous = edge(0.);
            for segment in 1..=8 {
                let next = edge(segment as f32 / 8.);
                path.move_to(previous.0);
                path.line_to(previous.1);
                path.line_to(next.1);
                path.line_to(next.0);
                previous = next;
            }
        };
        if matches!(c, '⎨' | '⎬') {
            let tip = self.x0 + width * 0.18;
            curve(
                &mut path,
                (center, self.snap(self.y0)),
                (center, self.cy),
                (tip, self.cy),
            );
            curve(
                &mut path,
                (tip, self.cy),
                (center, self.cy),
                (center, self.snap(self.y1)),
            );
            return vec![Ink::Path(path)];
        }
        let cap = self.snap(self.y0 + height * 0.12);
        let tip = self.x0 + width * 0.82;
        if matches!(c, '⎡' | '⎣' | '⎤' | '⎦') {
            let edge = if right {
                2. * stem + thickness - tip
            } else {
                tip
            };
            let (top, bottom) = if lower {
                (self.y0, self.y0 + self.y1 - cap)
            } else {
                (cap, self.y1)
            };
            return vec![
                self.vstroke(self.cx, self.t, top, bottom),
                self.hstroke(
                    if lower { bottom } else { top },
                    self.t,
                    center.min(edge),
                    center.max(edge),
                ),
            ];
        }
        let join = self.snap(
            self.y0
                + height
                    * if matches!(c, '⎧' | '⎩' | '⎫' | '⎭') {
                        0.4
                    } else {
                        0.85
                    },
        );
        curve(&mut path, (tip, cap), (center, cap), (center, join));
        let (top, bottom) = if lower {
            (self.y0, self.y0 + self.y1 - join)
        } else {
            (join, self.y1)
        };
        vec![Ink::Path(path), self.vstroke(self.cx, self.t, top, bottom)]
    }

    fn arms(&self, u: Arm, d: Arm, l: Arm, r: Arm) -> Vec<Ink> {
        let w = |a: Arm| match a {
            Arm::None => 0.,
            Arm::Light => self.t,
            Arm::Heavy => self.t * 2.,
        };
        let (wu, wd, wl, wr) = (w(u), w(d), w(l), w(r));
        let m = wu.max(wd).max(wl).max(wr) / 2.;
        let mut ink = Vec::new();
        if wu > 0. {
            ink.push(self.vstroke(self.cx, wu, self.y0, self.cy + m));
        }
        if wd > 0. {
            ink.push(self.vstroke(self.cx, wd, self.cy - m, self.y1));
        }
        if wl > 0. {
            ink.push(self.hstroke(self.cy, wl, self.x0, self.cx + m));
        }
        if wr > 0. {
            ink.push(self.hstroke(self.cy, wr, self.cx - m, self.x1));
        }
        ink
    }

    fn doubles(&self, c: char) -> Option<Vec<Ink>> {
        let t = self.t;
        let h = t / 2.;
        let d = (t * 1.5).max(2.0);
        let (x0, x1, y0, y1, cx, cy) = (self.x0, self.x1, self.y0, self.y1, self.cx, self.cy);
        let (va, vb) = (cx - d, cx + d);
        let (ha, hb) = (cy - d, cy + d);
        let v = |x: f32, ya: f32, yb: f32| self.vstroke(x, t, ya, yb);
        let hz = |y: f32, xa: f32, xb: f32| self.hstroke(y, t, xa, xb);
        Some(match c {
            '═' => vec![hz(ha, x0, x1), hz(hb, x0, x1)],
            '║' => vec![v(va, y0, y1), v(vb, y0, y1)],
            '╒' => vec![hz(ha, cx - h, x1), hz(hb, cx - h, x1), v(cx, ha - h, y1)],
            '╓' => vec![hz(cy, va - h, x1), v(va, cy - h, y1), v(vb, cy - h, y1)],
            '╔' => vec![
                v(va, ha - h, y1),
                hz(ha, va - h, x1),
                v(vb, hb - h, y1),
                hz(hb, vb - h, x1),
            ],
            '╕' => vec![hz(ha, x0, cx + h), hz(hb, x0, cx + h), v(cx, ha - h, y1)],
            '╖' => vec![hz(cy, x0, vb + h), v(va, cy - h, y1), v(vb, cy - h, y1)],
            '╗' => vec![
                v(vb, ha - h, y1),
                hz(ha, x0, vb + h),
                v(va, hb - h, y1),
                hz(hb, x0, va + h),
            ],
            '╘' => vec![v(cx, y0, hb + h), hz(ha, cx - h, x1), hz(hb, cx - h, x1)],
            '╙' => vec![v(va, y0, cy + h), v(vb, y0, cy + h), hz(cy, va - h, x1)],
            '╚' => vec![
                v(va, y0, hb + h),
                hz(hb, va - h, x1),
                v(vb, y0, ha + h),
                hz(ha, vb - h, x1),
            ],
            '╛' => vec![v(cx, y0, hb + h), hz(ha, x0, cx + h), hz(hb, x0, cx + h)],
            '╜' => vec![v(va, y0, cy + h), v(vb, y0, cy + h), hz(cy, x0, vb + h)],
            '╝' => vec![
                v(vb, y0, hb + h),
                hz(hb, x0, vb + h),
                v(va, y0, ha + h),
                hz(ha, x0, va + h),
            ],
            '╞' => vec![v(cx, y0, y1), hz(ha, cx - h, x1), hz(hb, cx - h, x1)],
            '╟' => vec![v(va, y0, y1), v(vb, y0, y1), hz(cy, vb - h, x1)],
            '╠' => vec![
                v(va, y0, y1),
                v(vb, y0, ha + h),
                v(vb, hb - h, y1),
                hz(ha, vb - h, x1),
                hz(hb, vb - h, x1),
            ],
            '╡' => vec![v(cx, y0, y1), hz(ha, x0, cx + h), hz(hb, x0, cx + h)],
            '╢' => vec![v(va, y0, y1), v(vb, y0, y1), hz(cy, x0, va + h)],
            '╣' => vec![
                v(vb, y0, y1),
                v(va, y0, ha + h),
                v(va, hb - h, y1),
                hz(ha, x0, va + h),
                hz(hb, x0, va + h),
            ],
            '╤' => vec![hz(ha, x0, x1), hz(hb, x0, x1), v(cx, hb - h, y1)],
            '╥' => vec![hz(cy, x0, x1), v(va, cy - h, y1), v(vb, cy - h, y1)],
            '╦' => vec![
                hz(ha, x0, x1),
                hz(hb, x0, va + h),
                hz(hb, vb - h, x1),
                v(va, hb - h, y1),
                v(vb, hb - h, y1),
            ],
            '╧' => vec![hz(ha, x0, x1), hz(hb, x0, x1), v(cx, y0, ha + h)],
            '╨' => vec![hz(cy, x0, x1), v(va, y0, cy + h), v(vb, y0, cy + h)],
            '╩' => vec![
                hz(hb, x0, x1),
                hz(ha, x0, va + h),
                hz(ha, vb - h, x1),
                v(va, y0, ha + h),
                v(vb, y0, ha + h),
            ],
            '╪' => vec![v(cx, y0, y1), hz(ha, x0, x1), hz(hb, x0, x1)],
            '╫' => vec![v(va, y0, y1), v(vb, y0, y1), hz(cy, x0, x1)],
            '╬' => vec![
                v(va, y0, ha + h),
                v(vb, y0, ha + h),
                v(va, hb - h, y1),
                v(vb, hb - h, y1),
                hz(ha, x0, va + h),
                hz(ha, vb - h, x1),
                hz(hb, x0, va + h),
                hz(hb, vb - h, x1),
            ],
            _ => return None,
        })
    }

    fn rounded(&self, c: char) -> Option<Vec<Ink>> {
        let (sx, sy): (f32, f32) = match c {
            '╭' => (1., 1.),
            '╮' => (-1., 1.),
            '╯' => (-1., -1.),
            '╰' => (1., -1.),
            _ => return None,
        };
        let h = self.t / 2.;
        let r = ((self.x1 - self.x0).min(self.y1 - self.y0) / 2.).max(h * 2.);
        let (cx, cy) = (self.cx, self.cy);
        let mut ink = Vec::new();
        let lap = 1. / self.scale;
        if sy > 0. {
            ink.push(self.vstroke(cx, self.t, cy + r - lap, self.y1));
        } else {
            ink.push(self.vstroke(cx, self.t, self.y0, cy - r + lap));
        }
        if sx > 0. {
            ink.push(self.hstroke(cy, self.t, cx + r - lap, self.x1));
        } else {
            ink.push(self.hstroke(cy, self.t, self.x0, cx - r + lap));
        }
        let (ax, ay) = (cx + sx * r, cy + sy * r);
        let at = |radius: f32, theta: f32| {
            let (x, y) = (
                ax - sx * radius * theta.cos(),
                ay - sy * radius * theta.sin(),
            );
            point(px(x), px(y))
        };
        const SEGS: usize = 3;
        const INNER_PTS: usize = 4;
        let step = std::f32::consts::FRAC_PI_2 / SEGS as f32;
        let mut path: Option<gpui_kit::Path<Pixels>> = None;
        for i in 0..SEGS {
            let t0 = step * i as f32;
            let t1 = step * (i + 1) as f32;
            let start = at(r + h, t0);
            let p = match path.as_mut() {
                Some(p) => {
                    p.move_to(start);
                    p
                }
                None => path.insert(gpui_kit::Path::new(start)),
            };
            let ctrl = at((r + h) / (step / 2.).cos(), (t0 + t1) / 2.);
            p.curve_to(at(r + h, t1), ctrl);
            p.line_to(at(r - h, t1));
            for k in (0..INNER_PTS).rev() {
                p.line_to(at(r - h, t0 + (t1 - t0) * k as f32 / INNER_PTS as f32));
            }
        }
        if let Some(p) = path {
            ink.push(Ink::Path(p));
        }
        Some(ink)
    }

    fn dashed(&self, c: char) -> Option<Vec<Ink>> {
        let (n, heavy, vertical) = match c {
            '╌' => (2, false, false),
            '╍' => (2, true, false),
            '╎' => (2, false, true),
            '╏' => (2, true, true),
            '┄' => (3, false, false),
            '┅' => (3, true, false),
            '┆' => (3, false, true),
            '┇' => (3, true, true),
            '┈' => (4, false, false),
            '┉' => (4, true, false),
            '┊' => (4, false, true),
            '┋' => (4, true, true),
            _ => return None,
        };
        let w = if heavy { self.t * 2. } else { self.t };
        let (a0, a1) = if vertical {
            (self.y0, self.y1)
        } else {
            (self.x0, self.x1)
        };
        let seg = (a1 - a0) / n as f32;
        let ink = (0..n)
            .map(|i| {
                let s = a0 + seg * (i as f32 + 0.15);
                let len = seg * 0.7;
                if vertical {
                    self.vstroke(self.cx, w, s, s + len)
                } else {
                    self.hstroke(self.cy, w, s, s + len)
                }
            })
            .collect();
        Some(ink)
    }

    fn diagonal(&self, c: char) -> Option<Vec<Ink>> {
        let (w, hgt) = (self.x1 - self.x0, self.y1 - self.y0);
        let v = self.t * (w * w + hgt * hgt).sqrt() / w;
        let p = |x: f32, y: f32| point(px(x), px(y));
        let quad = |top_x: f32, bot_x: f32| {
            let mut path = gpui_kit::Path::new(p(top_x, self.y0));
            path.line_to(p(top_x, self.y0 + v));
            path.line_to(p(bot_x, self.y1));
            path.line_to(p(bot_x, self.y1 - v));
            Ink::Path(path)
        };
        Some(match c {
            '╱' => vec![quad(self.x1, self.x0)],
            '╲' => vec![quad(self.x0, self.x1)],
            '╳' => vec![quad(self.x1, self.x0), quad(self.x0, self.x1)],
            _ => return None,
        })
    }

    fn blocks(&self, c: char) -> Option<Vec<Ink>> {
        let (x0, x1, y0, y1, cx, cy) = (self.x0, self.x1, self.y0, self.y1, self.cx, self.cy);
        let (w, hgt) = (x1 - x0, y1 - y0);
        let r = |x: f32, y: f32, ww: f32, hh: f32| self.rect(x, y, ww, hh);
        let ul = || r(x0, y0, cx - x0, cy - y0);
        let ur = || r(cx, y0, x1 - cx, cy - y0);
        let ll = || r(x0, cy, cx - x0, y1 - cy);
        let lr = || r(cx, cy, x1 - cx, y1 - cy);
        Some(match c {
            '▀' => vec![r(x0, y0, w, hgt / 2.)],
            '▁'..='█' => {
                let k = (c as u32 - 0x2580) as f32;
                let hh = hgt * k / 8.;
                vec![r(x0, y1 - hh, w, hh)]
            }
            '▉'..='▏' => {
                let k = (0x2590 - c as u32) as f32;
                vec![r(x0, y0, w * k / 8., hgt)]
            }
            '▐' => vec![r(cx, y0, x1 - cx, hgt)],
            '░' => vec![Ink::Shade(self.rectb(x0, y0, w, hgt), 0.25)],
            '▒' => vec![Ink::Shade(self.rectb(x0, y0, w, hgt), 0.5)],
            '▓' => vec![Ink::Shade(self.rectb(x0, y0, w, hgt), 0.75)],
            '▔' => vec![r(x0, y0, w, hgt / 8.)],
            '▕' => vec![r(x1 - w / 8., y0, w / 8., hgt)],
            '▖' => vec![ll()],
            '▗' => vec![lr()],
            '▘' => vec![ul()],
            '▙' => vec![ul(), ll(), lr()],
            '▚' => vec![ul(), lr()],
            '▛' => vec![ul(), ur(), ll()],
            '▜' => vec![ul(), ur(), lr()],
            '▝' => vec![ur()],
            '▞' => vec![ur(), ll()],
            '▟' => vec![ur(), ll(), lr()],
            _ => return None,
        })
    }
}

fn arms_of(c: char) -> Option<(Arm, Arm, Arm, Arm)> {
    use Arm::{Heavy as H, Light as L, None as N};
    Some(match c {
        '─' => (N, N, L, L),
        '━' => (N, N, H, H),
        '│' => (L, L, N, N),
        '┃' => (H, H, N, N),
        '┌' => (N, L, N, L),
        '┍' => (N, L, N, H),
        '┎' => (N, H, N, L),
        '┏' => (N, H, N, H),
        '┐' => (N, L, L, N),
        '┑' => (N, L, H, N),
        '┒' => (N, H, L, N),
        '┓' => (N, H, H, N),
        '└' => (L, N, N, L),
        '┕' => (L, N, N, H),
        '┖' => (H, N, N, L),
        '┗' => (H, N, N, H),
        '┘' => (L, N, L, N),
        '┙' => (L, N, H, N),
        '┚' => (H, N, L, N),
        '┛' => (H, N, H, N),
        '├' => (L, L, N, L),
        '┝' => (L, L, N, H),
        '┞' => (H, L, N, L),
        '┟' => (L, H, N, L),
        '┠' => (H, H, N, L),
        '┡' => (H, L, N, H),
        '┢' => (L, H, N, H),
        '┣' => (H, H, N, H),
        '┤' => (L, L, L, N),
        '┥' => (L, L, H, N),
        '┦' => (H, L, L, N),
        '┧' => (L, H, L, N),
        '┨' => (H, H, L, N),
        '┩' => (H, L, H, N),
        '┪' => (L, H, H, N),
        '┫' => (H, H, H, N),
        '┬' => (N, L, L, L),
        '┭' => (N, L, H, L),
        '┮' => (N, L, L, H),
        '┯' => (N, L, H, H),
        '┰' => (N, H, L, L),
        '┱' => (N, H, H, L),
        '┲' => (N, H, L, H),
        '┳' => (N, H, H, H),
        '┴' => (L, N, L, L),
        '┵' => (L, N, H, L),
        '┶' => (L, N, L, H),
        '┷' => (L, N, H, H),
        '┸' => (H, N, L, L),
        '┹' => (H, N, H, L),
        '┺' => (H, N, L, H),
        '┻' => (H, N, H, H),
        '┼' => (L, L, L, L),
        '┽' => (L, L, H, L),
        '┾' => (L, L, L, H),
        '┿' => (L, L, H, H),
        '╀' => (H, L, L, L),
        '╁' => (L, H, L, L),
        '╂' => (H, H, L, L),
        '╃' => (H, L, H, L),
        '╄' => (H, L, L, H),
        '╅' => (L, H, H, L),
        '╆' => (L, H, L, H),
        '╇' => (H, L, H, H),
        '╈' => (L, H, H, H),
        '╉' => (H, H, H, L),
        '╊' => (H, H, L, H),
        '╋' => (H, H, H, H),
        '╴' => (N, N, L, N),
        '╵' => (L, N, N, N),
        '╶' => (N, N, N, L),
        '╷' => (N, L, N, N),
        '╸' => (N, N, H, N),
        '╹' => (H, N, N, N),
        '╺' => (N, N, N, H),
        '╻' => (N, H, N, N),
        '╼' => (N, N, L, H),
        '╽' => (L, H, N, N),
        '╾' => (N, N, H, L),
        '╿' => (H, L, N, N),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> Bounds<Pixels> {
        Bounds::new(point(px(10.), px(20.)), size(px(9.), px(21.)))
    }

    fn extents(ink: &[Ink]) -> (f32, f32, f32, f32) {
        let (mut nx, mut xx, mut ny, mut xy) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        let mut visit = |x: f32, y: f32| {
            nx = nx.min(x);
            xx = xx.max(x);
            ny = ny.min(y);
            xy = xy.max(y);
        };
        for i in ink {
            match i {
                Ink::Rect(b) | Ink::Shade(b, _) => {
                    let (x, y) = (b.origin.x.as_f32(), b.origin.y.as_f32());
                    visit(x, y);
                    visit(x + b.size.width.as_f32(), y + b.size.height.as_f32());
                }
                Ink::Path(p) => {
                    for v in &p.vertices {
                        visit(v.xy_position.x.as_f32(), v.xy_position.y.as_f32());
                    }
                }
            }
        }
        (nx, xx, ny, xy)
    }

    #[test]
    fn math_delimiter_pieces_join_across_rows() {
        for scale in [1., 1.25, 1.5, 2.] {
            for height in [15., 21., 28.] {
                for pieces in ["⎛⎜⎝", "⎞⎟⎠", "⎡⎢⎣", "⎤⎥⎦", "⎧⎪⎨⎪⎩", "⎫⎪⎬⎪⎭"]
                {
                    let mut previous_edge: Option<f32> = None;
                    for (row, c) in pieces.chars().enumerate() {
                        let bounds = Bounds::new(
                            point(px(10.37), px(20.11 + row as f32 * height)),
                            size(px(9.03), px(height)),
                        );
                        let ink = glyph(c, bounds, scale)
                            .unwrap_or_else(|| panic!("{c} falls back to disconnected font ink"));
                        let (_, _, top, bottom) = extents(&ink);
                        let expected_top = (bounds.origin.y.as_f32() * scale).round() / scale;
                        let expected_bottom =
                            ((bounds.origin.y.as_f32() + height) * scale).round() / scale;
                        if let Some(edge) = previous_edge {
                            assert!((top - edge).abs() < 0.001, "{pieces}: gap before {c}");
                            assert!((top - expected_top).abs() < 0.001);
                        }
                        if row + 1 < pieces.chars().count() {
                            assert!(
                                (bottom - expected_bottom).abs() < 0.001,
                                "{c}: misses next row"
                            );
                        }
                        previous_edge = Some(bottom);
                    }
                }
            }
        }
    }

    #[test]
    fn the_whole_range_is_covered() {
        for cp in 0x2500u32..=0x259f {
            let c = char::from_u32(cp).unwrap();
            assert!(
                glyph(c, cell(), 1.).is_some(),
                "U+{cp:04X} {c} fell through to the font"
            );
        }
    }

    #[test]
    fn ink_stays_inside_the_cell() {
        let b = cell();
        let (x0, y0) = (b.origin.x.as_f32(), b.origin.y.as_f32());
        let (x1, y1) = (x0 + b.size.width.as_f32(), y0 + b.size.height.as_f32());
        for cp in (0x239bu32..=0x23ad).chain(0x2500..=0x259f) {
            let c = char::from_u32(cp).unwrap();
            let (nx, xx, ny, xy) = extents(&glyph(c, b, 1.).unwrap());
            assert!(
                nx >= x0 - 0.5 && xx <= x1 + 0.5 && ny >= y0 - 0.5 && xy <= y1 + 0.5,
                "U+{cp:04X} {c} paints outside the cell: \
                 x {nx}..{xx} vs {x0}..{x1}, y {ny}..{xy} vs {y0}..{y1}"
            );
        }
    }

    #[test]
    fn arms_reach_their_edges() {
        let b = cell();
        let (x0, y0) = (b.origin.x.as_f32(), b.origin.y.as_f32());
        let (x1, y1) = (x0 + b.size.width.as_f32(), y0 + b.size.height.as_f32());
        for cp in 0x2500u32..=0x259f {
            let c = char::from_u32(cp).unwrap();
            let Some((u, d, l, r)) = arms_of(c) else {
                continue;
            };
            let (nx, xx, ny, xy) = extents(&glyph(c, b, 1.).unwrap());
            if u != Arm::None {
                assert_eq!(ny, y0, "{c}: up arm misses the top edge");
            }
            if d != Arm::None {
                assert_eq!(xy, y1, "{c}: down arm misses the bottom edge");
            }
            if l != Arm::None {
                assert_eq!(nx, x0, "{c}: left arm misses the left edge");
            }
            if r != Arm::None {
                assert_eq!(xx, x1, "{c}: right arm misses the right edge");
            }
        }
    }

    #[test]
    fn doubles_rounded_and_diagonals_reach_their_edges() {
        let b = cell();
        let (x0, y0) = (b.origin.x.as_f32(), b.origin.y.as_f32());
        let (x1, y1) = (x0 + b.size.width.as_f32(), y0 + b.size.height.as_f32());
        let expect = [
            ('═', false, false, true, true),
            ('║', true, true, false, false),
            ('╔', false, true, false, true),
            ('╬', true, true, true, true),
            ('╠', true, true, false, true),
            ('╦', false, true, true, true),
            ('╭', false, true, false, true),
            ('╮', false, true, true, false),
            ('╯', true, false, true, false),
            ('╰', true, false, false, true),
            ('╱', true, true, true, true),
            ('╲', true, true, true, true),
        ];
        for (c, u, d, l, r) in expect {
            let (nx, xx, ny, xy) = extents(&glyph(c, b, 1.).unwrap());
            if u {
                assert_eq!(ny, y0, "{c}: misses the top edge");
            }
            if d {
                assert_eq!(xy, y1, "{c}: misses the bottom edge");
            }
            if l {
                assert_eq!(nx, x0, "{c}: misses the left edge");
            }
            if r {
                assert_eq!(xx, x1, "{c}: misses the right edge");
            }
        }
    }

    #[test]
    fn double_cross_keeps_its_open_centre() {
        let b = cell();
        let cx = b.origin.x.as_f32() + b.size.width.as_f32() / 2.;
        let cy = b.origin.y.as_f32() + b.size.height.as_f32() / 2.;
        for i in glyph('╬', b, 1.).unwrap() {
            let Ink::Rect(r) = i else {
                panic!("╬ should be rects only");
            };
            let (x, y) = (r.origin.x.as_f32(), r.origin.y.as_f32());
            let inside = cx > x
                && cx < x + r.size.width.as_f32()
                && cy > y
                && cy < y + r.size.height.as_f32();
            assert!(!inside, "╬'s centre is covered");
        }
    }

    #[test]
    fn blocks_cover_their_nominal_area() {
        let b = cell();
        let (x0, y0) = (b.origin.x.as_f32(), b.origin.y.as_f32());
        let (w, h) = (b.size.width.as_f32(), b.size.height.as_f32());
        let (nx, xx, ny, xy) = extents(&glyph('█', b, 1.).unwrap());
        assert_eq!(
            (nx, xx, ny, xy),
            (x0, x0 + w, y0, y0 + h),
            "█ isn't the full cell"
        );
        let (_, _, ny, xy) = extents(&glyph('▀', b, 1.).unwrap());
        assert_eq!(ny, y0, "▀ doesn't reach the top");
        assert!((xy - (y0 + h / 2.)).abs() <= 0.5, "▀ isn't the top half");
        let (_, _, ny, xy) = extents(&glyph('▄', b, 1.).unwrap());
        assert_eq!(xy, y0 + h, "▄ doesn't reach the bottom");
        assert!((ny - (y0 + h / 2.)).abs() <= 0.5, "▄ isn't the bottom half");
        for (c, alpha) in [('░', 0.25), ('▒', 0.5), ('▓', 0.75)] {
            let ink = glyph(c, b, 1.).unwrap();
            assert_eq!(ink.len(), 1);
            let Ink::Shade(r, a) = &ink[0] else {
                panic!("{c} should be a shade");
            };
            assert_eq!(*a, alpha);
            assert_eq!(r.size.width.as_f32(), w, "{c} doesn't wash the full cell");
        }
    }

    #[test]
    fn stroke_weights_are_ordered_and_visible() {
        let light = {
            let Ink::Rect(r) = &glyph('│', cell(), 1.).unwrap()[0] else {
                panic!()
            };
            r.size.width.as_f32()
        };
        let heavy = {
            let Ink::Rect(r) = &glyph('┃', cell(), 1.).unwrap()[0] else {
                panic!()
            };
            r.size.width.as_f32()
        };
        assert!(light >= 1., "light stroke thinner than a pixel");
        assert!(heavy > light, "heavy stroke isn't heavier");
        let tiny = Bounds::new(point(px(0.), px(0.)), size(px(2.), px(4.)));
        let Ink::Rect(r) = &glyph('│', tiny, 1.).unwrap()[0] else {
            panic!()
        };
        assert!(r.size.width.as_f32() >= 1.);
    }

    #[test]
    fn straight_strokes_snap_to_device_pixels() {
        let scale = 2.0;
        let b = Bounds::new(point(px(10.37), px(20.11)), size(px(9.03), px(21.)));
        let on_grid = |v: f32| ((v * scale).round() - v * scale).abs() < 1e-3;
        for cp in 0x2500u32..=0x259f {
            let c = char::from_u32(cp).unwrap();
            for i in glyph(c, b, scale).unwrap() {
                let (Ink::Rect(r) | Ink::Shade(r, _)) = i else {
                    continue;
                };
                let (x, y) = (r.origin.x.as_f32(), r.origin.y.as_f32());
                let (x2, y2) = (x + r.size.width.as_f32(), y + r.size.height.as_f32());
                assert!(
                    on_grid(x) && on_grid(y) && on_grid(x2) && on_grid(y2),
                    "U+{cp:04X} {c}: stroke edge off the device grid \
                     ({x}, {y})..({x2}, {y2}) at scale {scale}"
                );
            }
        }
        let below = Bounds::new(point(px(10.37), px(41.11)), size(px(9.03), px(21.)));
        let bottom = extents(&glyph('│', b, scale).unwrap()).3;
        let top = extents(&glyph('│', below, scale).unwrap()).2;
        assert_eq!(bottom, top, "adjacent │ cells no longer tile");
    }

    #[test]
    fn stroke_weight_is_uniform_across_cells_at_any_scale() {
        for (cw, lh) in [(7.8f32, 18.0f32), (9.03, 21.0), (9.6, 22.0), (10.8, 25.0)] {
            for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0] {
                let widths: Vec<f32> = (0..24)
                    .map(|i| {
                        let b = Bounds::new(point(px(cw * i as f32), px(0.)), size(px(cw), px(lh)));
                        let Ink::Rect(r) = &glyph('│', b, scale).unwrap()[0] else {
                            panic!("│ should be a rect")
                        };
                        (r.size.width.as_f32() * scale).round()
                    })
                    .collect();
                let (lo, hi) = (
                    widths.iter().cloned().fold(f32::MAX, f32::min),
                    widths.iter().cloned().fold(f32::MIN, f32::max),
                );
                assert_eq!(
                    lo, hi,
                    "│ weight varies {lo}..{hi} device px across columns \
                     (cell_width {cw}, scale {scale}): {widths:?}"
                );
                assert!(lo >= 1., "│ thinner than a device pixel at scale {scale}");

                let heights: Vec<f32> = (0..24)
                    .map(|r| {
                        let b = Bounds::new(point(px(0.), px(lh * r as f32)), size(px(cw), px(lh)));
                        let Ink::Rect(rect) = &glyph('─', b, scale).unwrap()[0] else {
                            panic!("─ should be a rect")
                        };
                        (rect.size.height.as_f32() * scale).round()
                    })
                    .collect();
                let (lo, hi) = (
                    heights.iter().cloned().fold(f32::MAX, f32::min),
                    heights.iter().cloned().fold(f32::MIN, f32::max),
                );
                assert_eq!(
                    lo, hi,
                    "─ weight varies {lo}..{hi} device px across rows \
                     (cell_width {cw}, scale {scale}): {heights:?}"
                );
            }
        }
    }

    #[test]
    fn integer_scales_keep_their_previous_thickness() {
        for (cw, lh) in [(7.8f32, 18.0f32), (9.03, 21.0), (9.6, 22.0), (10.8, 25.0)] {
            for scale in [1.0f32, 2.0, 3.0] {
                let previous = (cw * 0.15).round().max(1.);
                assert_eq!(
                    light_thickness(cw, scale),
                    previous,
                    "cell_width {cw} at scale {scale} changed weight"
                );
                let b = Bounds::new(point(px(0.), px(0.)), size(px(cw), px(lh)));
                let Ink::Rect(l) = &glyph('│', b, scale).unwrap()[0] else {
                    panic!()
                };
                let Ink::Rect(h) = &glyph('┃', b, scale).unwrap()[0] else {
                    panic!()
                };
                assert!(h.size.width.as_f32() > l.size.width.as_f32());
            }
        }
    }
}
