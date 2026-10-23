use gpui_kit::component::ActiveTheme;
use gpui_kit::*;
use std::{hash::{Hash, Hasher}, sync::Arc};

use super::{ParkedCard, ParkedDeckLabels};
use crate::ui::presets::{Interactions, Parking};

// 批准的原生闸门几何，以 rem=16 为设计坐标；矩阵和逆命中共享同一比例。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Pose {
    pub x: f32,
    pub y: f32,
    pub angle: f32,
    pub scale: f32,
    pub flip: f32,
    pub width: f32,
    pub height: f32,
}

impl Default for Pose {
    fn default() -> Self {
        Self { x: 0., y: 0., angle: 0., scale: 1., flip: 0., width: 56., height: 74. }
    }
}

impl Pose {
    pub fn interpolate(self, to: Self, outer: f32, flip: f32) -> Self {
        let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
        Self {
            x: mix(self.x, to.x, outer), y: mix(self.y, to.y, outer),
            angle: mix(self.angle, to.angle, outer), scale: mix(self.scale, to.scale, outer),
            flip: mix(self.flip, to.flip, flip), width: mix(self.width, to.width, outer),
            height: mix(self.height, to.height, outer),
        }
    }
}

pub(super) fn ease(t: f32) -> f32 {
    if t <= 0. { return 0.; }
    if t >= 1. { return 1.; }
    let (mut low, mut high): (f32, f32) = (0., 1.);
    for _ in 0..16 {
        let u = (low + high) * 0.5;
        let x = 3. * (1. - u).powi(2) * u * 0.25 + 3. * (1. - u) * u * u * 0.25 + u.powi(3);
        if x < t { low = u; } else { high = u; }
    }
    let u = (low + high) * 0.5;
    3. * (1. - u).powi(2) * u * 0.1 + 3. * (1. - u) * u * u + u.powi(3)
}

#[derive(Clone, Copy, Debug)]
struct Affine { a: f32, b: f32, c: f32, d: f32, x: f32, y: f32 }

impl Affine {
    fn apply(self, x: f32, y: f32) -> [f32; 2] {
        [self.a * x + self.b * y + self.x, self.c * x + self.d * y + self.y]
    }

    fn inverse(self, position: Point<Pixels>) -> Option<[f32; 2]> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 0.00001 { return None; }
        let x = f32::from(position.x) - self.x;
        let y = f32::from(position.y) - self.y;
        Some([(self.d * x - self.b * y) / det, (-self.c * x + self.a * y) / det])
    }

    fn face(self, compression: f32, center: f32) -> Self {
        let shift = center * (1. - compression);
        Self {
            a: self.a * compression, c: self.c * compression,
            x: self.x + self.a * shift, y: self.y + self.c * shift, ..self
        }
    }

    fn resize(self, x: f32, y: f32) -> Self {
        Self { a: self.a * x, b: self.b * y, c: self.c * x, d: self.d * y, ..self }
    }

    fn raster(self, scale: f32, device_scale: f32, source: Point<Pixels>, viewbox: [f32; 4]) -> TransformationMatrix {
        let origin = self.apply(viewbox[0], viewbox[1]);
        let (a, b, c, d) = (self.a / scale, self.b / scale, self.c / scale, self.d / scale);
        TransformationMatrix {
            rotation_scale: [[a, b], [c, d]],
            translation: [
                (origin[0] - a * f32::from(source.x) - b * f32::from(source.y)) * device_scale,
                (origin[1] - c * f32::from(source.x) - d * f32::from(source.y)) * device_scale,
            ],
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Layer { Surface, Border, Ink, Muted, Restore, RestoreInk, Shadow, SelectedShadow }

#[derive(Clone, Debug)]
pub(super) struct Sprite {
    key: SharedString,
    data: Arc<str>,
    layer: Layer,
    back: Option<bool>,
    viewbox: [f32; 4],
}

fn sprite(layer: Layer, back: Option<bool>, viewbox: [f32; 4], body: &str) -> Sprite {
    let [x, y, width, height] = viewbox;
    let data: Arc<str> = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="{x} {y} {width} {height}">{body}</svg>"#).into();
    // paint_svg 的缓存键必须包含实际文字/字体，不能在同 pane 改名后复用旧 atlas。
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut hash);
    Sprite { key: format!("parked-deck/{:016x}", hash.finish()).into(), data, layer, back, viewbox }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

fn text(value: &str, font: &str, x: f32, y: f32, size: f32, width: f32, bold: bool) -> String {
    let mut result = String::new();
    let mut advance = 0.;
    for ch in value.chars() {
        let next = if ch.is_ascii() { size * 0.60 } else { size };
        if advance + next > width - size {
            result.push('…');
            break;
        }
        advance += next;
        result.push(ch);
    }
    // alpha-mask 的白色是 raster 数据，不是界面颜色；最终墨色由主题传给 paint_svg。
    format!(r#"<text x="{x}" y="{y}" text-anchor="middle" font-family="{},Segoe UI,Microsoft YaHei,PingFang SC,sans-serif" font-size="{size}" font-weight="{}" fill="white">{}</text>"#, escape(font), if bold { 600 } else { 400 }, escape(&result))
}

pub(super) fn card_sprites(card: &ParkedCard, labels: &ParkedDeckLabels, font: &str) -> Vec<Sprite> {
    let front = [0., 0., 56., 74.];
    let back = [0., 0., 120., 112.];
    let status = card.status(labels);
    let icon = r#"<g fill="none" stroke="white" stroke-width="1.8"><rect x="20" y="13" width="16" height="16" rx="2"/><path d="M23 17l3 3-3 3m6 1h4"/></g>"#;
    let mut result = vec![
        sprite(Layer::Shadow, None, [-50., -40., 156., 180.], r#"<defs><filter id="s" x="-200%" y="-200%" width="500%" height="500%"><feGaussianBlur stdDeviation="14"/></filter></defs><rect x="0" y="10" width="56" height="74" rx="10" fill="white" filter="url(#s)"/>"#),
        sprite(Layer::SelectedShadow, None, [-90., -90., 236., 300.], r#"<defs><filter id="s" x="-200%" y="-200%" width="500%" height="500%"><feGaussianBlur stdDeviation="27"/></filter></defs><rect x="0" y="22" width="56" height="74" rx="10" fill="white" filter="url(#s)"/>"#),
        sprite(Layer::Surface, None, front, r#"<rect x=".5" y=".5" width="55" height="73" rx="9.5" fill="white"/>"#),
        sprite(Layer::Border, None, front, r#"<rect x=".5" y=".5" width="55" height="73" rx="9.5" stroke="white" fill="none"/>"#),
        sprite(Layer::Ink, Some(false), front, &format!("{icon}{}", text(&card.title, font, 28., 45., 11., 46., true))),
        sprite(Layer::Muted, Some(false), front, &text(status, font, 28., 60., 10., 46., false)),
        sprite(Layer::Surface, Some(true), back, r#"<rect x="2.143" y="1.514" width="115.714" height="108.973" rx="19.286" ry="13.622" fill="white"/>"#),
        sprite(Layer::Ink, Some(true), back, &format!(r#"<g transform="translate(32 -3)">{icon}</g>{}"#, text(&card.title, font, 60., 44., 11., 108., true))),
        sprite(Layer::Muted, Some(true), back, &format!("{}{}", text(&card.path, font, 60., 60., 8., 108., false), text(status, font, 60., 77., 10., 108., false))),
    ];
    result.push(sprite(Layer::Restore, Some(true), back, r#"<rect x="27" y="84" width="66" height="24" rx="5" fill="white"/>"#));
    result.push(sprite(Layer::RestoreInk, Some(true), back, &text(&labels.restore, font, 60., 100., 10., 62., true)));
    result
}

#[derive(Clone, Copy)]
pub(super) struct Tokens {
    surface: Hsla, border: Hsla, foreground: Hsla, muted: Hsla,
    accent: Hsla, accent_ink: Hsla, warm: Hsla, warm_ink: Hsla, shadow: Hsla,
}

impl Tokens {
    pub fn from_theme(cx: &App) -> Self {
        let theme = cx.theme();
        let parking = cx.global::<Parking>();
        let primary = cx.global::<Interactions>().primary;
        Self {
            surface: theme.popover, border: theme.border, foreground: theme.foreground,
            muted: theme.muted_foreground, accent: rgb(primary.fill).into(),
            accent_ink: rgb(primary.on_fill).into(), warm: rgb(parking.affordance.fill).into(),
            warm_ink: rgb(parking.affordance.on_fill).into(), shadow: rgb(parking.shadow).into(),
        }
    }
}

#[derive(Clone, Debug)]
struct CardFrame {
    model: ParkedCard, outer: Affine, face: Affine, pose: Pose,
    selected: bool, highlighted: bool, sprites: Arc<Vec<Sprite>>,
}

#[derive(Clone, Default, Debug)]
pub(super) struct Frame {
    bounds: Bounds<Pixels>, viewport: Bounds<Pixels>, cards: Vec<CardFrame>,
    zoom: f32, fan: f32, park: bool, park_ink: Option<Sprite>,
}

pub(super) struct FrameCard {
    pub model: ParkedCard,
    pub pose: Pose,
    pub sprites: Arc<Vec<Sprite>>,
    pub selected: bool,
    pub highlighted: bool,
}

pub(super) fn park_label(label: &str, font: &str) -> Sprite {
    sprite(Layer::Ink, None, [0., 0., 360., 170.], &text(label, font, 180., 28., 11., 80., true))
}

impl Frame {
    pub fn new(bounds: Bounds<Pixels>, viewport: Bounds<Pixels>, zoom: f32, cards: Vec<FrameCard>, fan: f32, park: bool, park_ink: Sprite) -> Self {
        let mut result = Self { bounds, viewport, cards: Vec::with_capacity(cards.len()), zoom, fan, park, park_ink: Some(park_ink) };
        let target_x = cards.iter().find(|c| c.selected || c.highlighted).map(|c| c.pose.x).unwrap_or(0.);
        let half_width = (f32::from(viewport.size.width) / zoom / 2. - 105.).max(0.);
        let shift = target_x - target_x.clamp(-half_width, half_width);
        for card in cards {
            let pose = card.pose;
            let radians = pose.angle.to_radians();
            let (a, b, c, d) = (radians.cos() * pose.scale * zoom, -radians.sin() * pose.scale * zoom, radians.sin() * pose.scale * zoom, radians.cos() * pose.scale * zoom);
            let mut anchor_x = f32::from(bounds.center().x) + (pose.x - shift) * zoom;
            let mut anchor_y = f32::from(bounds.bottom()) + pose.y * zoom;
            if card.selected {
                // 不把恢复操作放到 content 顶边外；极小区域仍由共同 viewport 裁切。
                let w = pose.width * pose.scale * zoom;
                let h = pose.height * pose.scale * zoom;
                let left = f32::from(viewport.left()) + w / 2.;
                let right = (f32::from(viewport.right()) - w / 2.).max(left);
                anchor_x = anchor_x.clamp(left, right);
                anchor_y = anchor_y.max(f32::from(viewport.top()) + h).min(f32::from(viewport.bottom()));
            }
            let (sx, sy) = (pose.width / 56., pose.height / 74.);
            let outer = Affine { a: a * sx, b: b * sy, c: c * sx, d: d * sy,
                x: anchor_x - a * pose.width / 2. - b * pose.height,
                y: anchor_y - c * pose.width / 2. - d * pose.height };
            let compression = (std::f32::consts::PI * pose.flip).cos().abs();
            let face = if pose.flip > 0.5 { outer.resize(56. / 120., 74. / 112.).face(compression, 60.) } else { outer.face(compression, 28.) };
            result.cards.push(CardFrame { model: card.model, outer, face, pose, selected: card.selected,
                highlighted: card.highlighted, sprites: card.sprites });
        }
        result.cards.sort_by_key(|card| (card.selected, card.highlighted));
        result
    }

    pub fn viewport(&self) -> Bounds<Pixels> { self.viewport }

    pub fn keyboard_bounds(&self) -> Bounds<Pixels> {
        let width = self.viewport.size.width.min(px(360. * self.zoom));
        let height = self.viewport.size.height.min(px(154. * self.zoom));
        Bounds { origin: point(self.viewport.center().x - width / 2., self.viewport.bottom() - height), size: size(width, height) }
    }

    pub fn semantic_cards(&self) -> impl Iterator<Item = (&ParkedCard, Bounds<Pixels>, bool, Option<Bounds<Pixels>>)> + '_ {
        self.cards.iter().map(|card| {
            let bounds_for = |transform: Affine, points: [(f32, f32); 4]| {
                let points = points.map(|(x, y)| transform.apply(x, y));
                let min_x = points.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
                let min_y = points.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
                let max_x = points.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max);
                let max_y = points.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max);
                Bounds { origin: point(px(min_x), px(min_y)), size: size(px(max_x - min_x), px(max_y - min_y)) }.intersect(&self.viewport)
            };
            let bounds = bounds_for(card.outer, [(0., 0.), (56., 0.), (56., 74.), (0., 74.)]);
            let restore = (card.selected && card.pose.flip > 0.5).then(|| bounds_for(card.face, [(27., 84.), (93., 84.), (93., 108.), (27., 108.)]));
            (&card.model, bounds, card.selected, restore)
        })
    }

    pub fn deck_contains(&self, position: Point<Pixels>) -> bool {
        self.viewport.contains(&position)
            && (f32::from(position.x) - f32::from(self.bounds.center().x)).abs() <= 180. * self.zoom
            && f32::from(position.y) >= f32::from(self.bounds.bottom()) - 154. * self.zoom
    }

    pub fn hit(&self, position: Point<Pixels>) -> Option<(u64, bool)> {
        if !self.viewport.contains(&position) { return None; }
        for card in self.cards.iter().rev() {
            let Some([x, y]) = card.outer.inverse(position) else { continue; };
            if !(0. ..=56.).contains(&x) || !(0. ..=74.).contains(&y) { continue; }
            let dx = (x - x.clamp(10., 46.)).abs();
            let dy = (y - y.clamp(10., 64.)).abs();
            if dx * dx + dy * dy > 100. { continue; }
            let restore = card.selected && card.pose.flip > 0.5
                && card.face.inverse(position).is_some_and(|[x, y]| (27. ..=93.).contains(&x) && (84. ..=108.).contains(&y));
            return Some((card.model.pane, restore));
        }
        None
    }

    pub fn paint(&self, tokens: Tokens, window: &mut Window, cx: &App) -> Result<(), String> {
        if self.fan > 0. {
            let mut aura = tokens;
            aura.warm.a *= self.fan;
            self.halo(false, aura, window, cx)?;
        }
        if self.park { self.halo(true, tokens, window, cx)?; }
        let raster_scale = self.zoom * 1.58;
        let source = self.viewport.origin.map(|v| px((f32::from(v) * window.scale_factor()).round() / window.scale_factor()));
        let has_selected = self.cards.iter().any(|card| card.selected);
        for card in &self.cards {
            for sprite in card.sprites.iter() {
                if sprite.back.is_some_and(|back| back != (card.pose.flip > 0.5)) { continue; }
                let mut color = match sprite.layer {
                    Layer::Surface => tokens.surface.opacity(if sprite.back.is_some() { 0.96 } else { 0.90 }),
                    Layer::Border => if card.selected || card.highlighted { tokens.accent } else { tokens.border },
                    Layer::Ink => tokens.foreground,
                    Layer::Muted => tokens.muted,
                    Layer::Restore => tokens.accent,
                    Layer::RestoreInk => tokens.accent_ink,
                    Layer::Shadow if !card.selected => tokens.shadow.opacity(0.24),
                    Layer::SelectedShadow if card.selected => tokens.shadow.opacity(0.36),
                    Layer::Shadow | Layer::SelectedShadow => continue,
                };
                if has_selected && !card.selected { color.a *= 0.72; }
                let transform = if sprite.back.is_some() { card.face } else { card.outer };
                if sprite.back.is_some() && (0.4999..0.5001).contains(&card.pose.flip) { continue; }
                let raster_bounds = Bounds { origin: source, size: size(px(sprite.viewbox[2] * raster_scale), px(sprite.viewbox[3] * raster_scale)) };
                // atlas tile 重排不能改变相交文字和卡面的顺序；每个 sprite 各占 paint_layer。
                window.paint_layer(self.viewport, |window| window.paint_svg(raster_bounds, sprite.key.clone(), Some(sprite.data.as_bytes()),
                    transform.raster(raster_scale, window.scale_factor(), source, sprite.viewbox), color, cx))
                    .map_err(|error| format!("SVG {}: {error}", sprite.key))?;
            }
        }
        Ok(())
    }

    fn halo(&self, park: bool, tokens: Tokens, window: &mut Window, cx: &App) -> Result<(), String> {
        let (width, height) = if park { (360., 170.) } else { (260., 130.) };
        let data = if park {
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="360" height="170"><defs><filter id="p" x="-100%" y="-100%" width="300%" height="300%"><feGaussianBlur stdDeviation="7"/></filter></defs><path d="M1 170V169 A179 169 0 0 1 359 169V170" fill="white" fill-opacity=".08" stroke="white" stroke-opacity=".54"/><rect x="140" y="16" width="80" height="24" rx="12" fill="white" fill-opacity=".22" filter="url(#p)"/><rect x="140" y="12" width="80" height="24" rx="12" fill="white" fill-opacity=".82"/></svg>"#
        } else {
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="260" height="130"><path d="M1 130 A129 129 0 0 1 259 130" fill="white" fill-opacity=".07" stroke="white" stroke-opacity=".58"/></svg>"#
        };
        let bounds = Bounds { origin: point(self.bounds.center().x - px(width * self.zoom / 2.), self.bounds.bottom() - px(height * self.zoom)), size: size(px(width * self.zoom), px(height * self.zoom)) };
        window.paint_layer(self.viewport, |window| window.paint_svg(bounds, if park { "parked-deck/drop" } else { "parked-deck/aura" }.into(), Some(data.as_bytes()), TransformationMatrix::unit(), tokens.warm, cx))
            .map_err(|error| error.to_string())?;
        if park && let Some(ink) = &self.park_ink {
            window.paint_layer(self.viewport, |window| window.paint_svg(bounds, ink.key.clone(), Some(ink.data.as_bytes()), TransformationMatrix::unit(), tokens.warm_ink, cx))
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}
