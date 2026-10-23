use gpui_kit::component::ActiveTheme;
use gpui_kit::*;
use std::sync::Arc;

// 几何值全部取自批准 HTML，以 16px rem 为比例锚点；不是物理像素常量。

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Pose {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) angle: f32,
    pub(super) scale: f32,
    pub(super) flip: f32,
    pub(super) width: f32,
    pub(super) height: f32,
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            x: 0.,
            y: 0.,
            angle: 0.,
            scale: 1.,
            flip: 0.,
            width: 56.,
            height: 74.,
        }
    }
}

impl Pose {
    pub(super) fn interpolate(self, to: Self, outer: f32, flip: f32) -> Self {
        let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
        Self {
            x: mix(self.x, to.x, outer),
            y: mix(self.y, to.y, outer),
            angle: mix(self.angle, to.angle, outer),
            scale: mix(self.scale, to.scale, outer),
            flip: mix(self.flip, to.flip, flip),
            width: mix(self.width, to.width, outer),
            height: mix(self.height, to.height, outer),
        }
    }
}

pub(super) fn ease(t: f32) -> f32 {
    if t <= 0. {
        return 0.;
    }
    if t >= 1. {
        return 1.;
    }
    let mut low: f32 = 0.;
    let mut high: f32 = 1.;
    for _ in 0..16 {
        let u = (low + high) * 0.5;
        let x = 3. * (1. - u).powi(2) * u * 0.25 + 3. * (1. - u) * u * u * 0.25 + u.powi(3);
        if x < t {
            low = u;
        } else {
            high = u;
        }
    }
    let u = (low + high) * 0.5;
    3. * (1. - u).powi(2) * u * 0.1 + 3. * (1. - u) * u * u + u.powi(3)
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Affine {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    x: f32,
    y: f32,
}

impl Affine {
    fn apply(self, x: f32, y: f32) -> [f32; 2] {
        [
            self.a * x + self.b * y + self.x,
            self.c * x + self.d * y + self.y,
        ]
    }
    fn inverse(self, position: Point<Pixels>) -> Option<[f32; 2]> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 0.00001 {
            return None;
        }
        let x = f32::from(position.x) - self.x;
        let y = f32::from(position.y) - self.y;
        Some([
            (self.d * x - self.b * y) / det,
            (-self.c * x + self.a * y) / det,
        ])
    }
    fn face(self, compression: f32, center: f32) -> Self {
        // rotateY 的正交投影以卡面中心为原点。背面多一次 180°，不镜像文字。
        let shift = center * (1. - compression);
        Self {
            a: self.a * compression,
            b: self.b,
            c: self.c * compression,
            d: self.d,
            x: self.x + self.a * shift,
            y: self.y + self.c * shift,
        }
    }
    fn resize(self, x: f32, y: f32) -> Self {
        Self {
            a: self.a * x,
            b: self.b * y,
            c: self.c * x,
            d: self.d * y,
            ..self
        }
    }
    fn raster(
        self,
        raster_scale: f32,
        device_scale: f32,
        source: Point<Pixels>,
        viewbox: [f32; 4],
    ) -> TransformationMatrix {
        // paint_svg 的矩阵操作 ScaledPixels，而命中逆矩阵操作逻辑 Pixels。
        let origin = self.apply(viewbox[0], viewbox[1]);
        let (a, b, c, d) = (
            self.a / raster_scale,
            self.b / raster_scale,
            self.c / raster_scale,
            self.d / raster_scale,
        );
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
pub(super) enum Layer {
    Surface,
    Border,
    Ink,
    Muted,
    Restore,
    RestoreInk,
    Shadow,
    SelectedShadow,
}

#[derive(Clone, Debug)]
pub(super) struct Sprite {
    key: SharedString,
    data: Arc<str>,
    layer: Layer,
    back: Option<bool>,
    candidate: Option<bool>,
    viewbox: [f32; 4],
}

fn sprite(key: String, layer: Layer, back: Option<bool>, body: &str) -> Sprite {
    let viewbox = match layer {
        Layer::Shadow => [-50., -40., 156., 180.],
        Layer::SelectedShadow => [-90., -90., 236., 300.],
        _ => [0., 0., 56., 74.],
    };
    make_sprite(key, layer, back, None, viewbox, body)
}

fn make_sprite(
    key: String,
    layer: Layer,
    back: Option<bool>,
    candidate: Option<bool>,
    viewbox: [f32; 4],
    body: &str,
) -> Sprite {
    let [x, y, width, height] = viewbox;
    Sprite { key: key.into(), layer, back, candidate, viewbox,
        data: format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="{x} {y} {width} {height}"><defs><clipPath id="face"><rect x="1" y="1" width="54" height="72" rx="9"/></clipPath><clipPath id="text"><rect x="4" y="1" width="48" height="72"/></clipPath></defs>{body}</svg>"#).into() }
}

fn preview_sprite(key: String, layer: Layer, candidate: bool, body: &str) -> Sprite {
    let viewbox = if candidate {
        [0., 0., 120., 112.]
    } else {
        [0., 0., 56., 74.]
    };
    make_sprite(key, layer, Some(true), Some(candidate), viewbox, body)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn text(text: &str, y: f32, size: f32, bold: bool) -> String {
    // 文本只在卡片创建时生成，SVG renderer 使用公开的系统字体 resolver。
    // 保留 HTML 的 nowrap/ellipsis：长字段另在诊断与普通 Kit 标签中完整可见。
    let max_width = 46.;
    let mut width = 0.;
    let mut result = String::new();
    let mut truncated = false;
    for ch in text.chars() {
        let advance = if ch.is_ascii() { size * 0.56 } else { size };
        if width + advance > max_width {
            truncated = true;
            break;
        }
        width += advance;
        result.push(ch);
    }
    if truncated {
        result.pop();
        result.push('…');
    }
    format!(
        r#"<g clip-path="url(#text)"><text x="28" y="{y}" text-anchor="middle" font-family="Segoe UI,Microsoft YaHei,PingFang SC,sans-serif" font-size="{size}" font-weight="{}" fill="white">{}</text></g>"#,
        if bold { 600 } else { 400 },
        escape(&result)
    )
}

pub(super) fn card_sprites(id: usize, title: &str, path: &str) -> Vec<Sprite> {
    let key = |name: &str| format!("native-gate/card/{id}/{name}");
    let icon = r#"<g fill="none" stroke="white" stroke-width="1.8"><rect x="20" y="10" width="16" height="16" rx="2"/><path d="M23 14l3 3-3 3m6 1h4"/></g>"#;
    let front_icon = r#"<g fill="none" stroke="white" stroke-width="1.8"><rect x="20" y="13" width="16" height="16" rx="2"/><path d="M23 17l3 3-3 3m6 1h4"/></g>"#;
    vec![
        sprite(
            key("shadow"),
            Layer::Shadow,
            None,
            r#"<defs><filter id="s" x="-200%" y="-200%" width="500%" height="500%"><feGaussianBlur stdDeviation="14"/></filter></defs><rect x="0" y="10" width="56" height="74" rx="10" fill="white" filter="url(#s)"/>"#,
        ),
        sprite(
            key("selected-shadow"),
            Layer::SelectedShadow,
            None,
            r#"<defs><filter id="s" x="-200%" y="-200%" width="500%" height="500%"><feGaussianBlur stdDeviation="27"/></filter></defs><rect x="0" y="22" width="56" height="74" rx="10" fill="white" filter="url(#s)"/>"#,
        ),
        sprite(
            key("surface"),
            Layer::Surface,
            None,
            r#"<rect x="0.5" y="0.5" width="55" height="73" rx="9.5" fill="white"/>"#,
        ),
        sprite(
            key("border"),
            Layer::Border,
            None,
            r#"<rect x="0.5" y="0.5" width="55" height="73" rx="9.5" stroke="white" fill="none"/>"#,
        ),
        sprite(
            key("front-ink"),
            Layer::Ink,
            Some(false),
            &format!("{front_icon}{}", text(title, 45., 11., true)),
        ),
        sprite(
            key("front-muted"),
            Layer::Muted,
            Some(false),
            &text("idle", 60., 10., false),
        ),
        preview_sprite(
            key("back-surface"),
            Layer::Surface,
            false,
            r#"<rect x="1" y="1" width="54" height="72" rx="9" fill="white"/>"#,
        ),
        // 原型测量：reference-card-geometry.json。三个 span 被 flex-shrink 裁成细条。
        preview_sprite(
            key("back-ink"),
            Layer::Ink,
            false,
            &format!(
                r#"{icon}<clipPath id="title-slice"><rect x="5" y="30" width="46" height="2.656"/></clipPath><g clip-path="url(#title-slice)">{}</g>"#,
                text(title, 42., 11., true)
            ),
        ),
        preview_sprite(
            key("back-muted"),
            Layer::Muted,
            false,
            &format!(
                r#"<clipPath id="path-slice"><rect x="5" y="36.656" width="46" height="2.422"/></clipPath><clipPath id="state-slice"><rect x="5" y="43.078" width="46" height="2.422"/></clipPath><g clip-path="url(#path-slice)">{}</g><g clip-path="url(#state-slice)">{}</g>"#,
                text(path, 47.5, 10., false),
                text("已停放 · 会话保持", 53.9, 10., false)
            ),
        ),
        preview_sprite(
            key("restore"),
            Layer::Restore,
            false,
            r#"<rect x="4" y="51.5" width="48" height="14.5" rx="5" fill="white"/>"#,
        ),
        preview_sprite(
            key("restore-ink"),
            Layer::RestoreInk,
            false,
            &text("恢复", 62.5, 10., true),
        ),
        // 用户已批准扩大选中预览；保留原型模式用于对照。
        preview_sprite(
            key("candidate-surface"),
            Layer::Surface,
            true,
            r#"<rect x="2.143" y="1.514" width="115.714" height="108.973" rx="19.286" ry="13.622" fill="white"/>"#,
        ),
        preview_sprite(
            key("candidate-ink"),
            Layer::Ink,
            true,
            &format!(
                r#"<g transform="translate(32 0)">{icon}</g><text x="60" y="44" text-anchor="middle" font-family="Segoe UI,Microsoft YaHei,PingFang SC,sans-serif" font-size="11" font-weight="600" fill="white">{}</text>"#,
                escape(title)
            ),
        ),
        preview_sprite(
            key("candidate-muted"),
            Layer::Muted,
            true,
            &format!(
                r#"<text x="60" y="60" text-anchor="middle" font-family="Segoe UI,Microsoft YaHei,PingFang SC,sans-serif" font-size="8" fill="white">{}</text><text x="60" y="77" text-anchor="middle" font-family="Segoe UI,Microsoft YaHei,PingFang SC,sans-serif" font-size="10" fill="white">仅本地绘制探针</text>"#,
                escape(path)
            ),
        ),
        preview_sprite(
            key("candidate-restore"),
            Layer::Restore,
            true,
            r#"<rect x="36" y="86" width="48" height="20" rx="5" fill="white"/>"#,
        ),
        preview_sprite(
            key("candidate-restore-ink"),
            Layer::RestoreInk,
            true,
            r#"<text x="60" y="100" text-anchor="middle" font-family="Segoe UI,Microsoft YaHei,PingFang SC,sans-serif" font-size="10" font-weight="700" fill="white">恢复</text>"#,
        ),
    ]
}

#[derive(Clone, Copy)]
pub(super) struct Tokens {
    surface: Hsla,
    border: Hsla,
    foreground: Hsla,
    muted: Hsla,
    accent: Hsla,
    accent_ink: Hsla,
    warm: Hsla,
    warm_ink: Hsla,
    shadow: Hsla,
}

impl Tokens {
    pub(super) fn from_theme(cx: &App, dark: bool) -> Self {
        let theme = cx.theme();
        Self {
            surface: theme.popover,
            border: theme.border,
            foreground: theme.foreground,
            muted: theme.muted_foreground,
            accent: theme.primary,
            accent_ink: theme.primary_foreground,
            // 探针独立 parking 交互角色，不借用 warning 状态色。
            warm: rgb(if dark { 0xe0a345 } else { 0xb7791f }).into(),
            warm_ink: rgb(0x22180a).into(),
            shadow: black(),
        }
    }
}

#[derive(Clone, Debug)]
struct CardFrame {
    id: usize,
    outer: Affine,
    face: Affine,
    pose: Pose,
    selected: bool,
    highlighted: bool,
    sprites: Arc<Vec<Sprite>>,
}

#[derive(Clone, Default)]
pub(super) struct Frame {
    bounds: Bounds<Pixels>,
    viewport: Bounds<Pixels>,
    cards: Vec<CardFrame>,
    zoom: f32,
    fan: f32,
    park: bool,
    candidate: bool,
    constrained: bool,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "内容边界={:?}；可见裁切/命中边界={:?}；status band 从 y={} 起；zoom={} constrained={} fan={} park={} readable_candidate={}",
            self.bounds,
            self.viewport,
            f32::from(self.viewport.bottom()),
            self.zoom,
            self.constrained,
            self.fan,
            self.park,
            self.candidate
        )?;
        for card in &self.cards {
            let corners =
                [(0., 0.), (56., 0.), (56., 74.), (0., 74.)].map(|(x, y)| card.outer.apply(x, y));
            writeln!(
                f,
                "card #{} pose={:?} outer={:?} face={:?} corners={:?} back={} selected={} highlighted={}",
                card.id,
                card.pose,
                card.outer,
                card.face,
                corners,
                card.pose.flip > 0.5,
                card.selected,
                card.highlighted
            )?;
        }
        Ok(())
    }
}

impl Frame {
    pub(super) fn new(
        bounds: Bounds<Pixels>,
        viewport: Bounds<Pixels>,
        zoom: f32,
        cards: Vec<(usize, Pose, Arc<Vec<Sprite>>, bool, bool)>,
        fan: f32,
        park: bool,
        candidate: bool,
    ) -> Self {
        // 不无限压缩命中目标：窄窗改为单牌，其他牌经 ←/→ 或上一/下一组仍可达。
        let narrow = f32::from(bounds.size.width) < 360. * zoom;
        let target = cards
            .iter()
            .find(|card| card.3 || card.4)
            .map(|card| card.0)
            .or_else(|| cards.first().map(|card| card.0));
        let mut result = Self {
            bounds,
            viewport,
            cards: Vec::new(),
            zoom,
            fan,
            park,
            candidate,
            constrained: narrow,
        };
        for (id, mut pose, sprites, selected, highlighted) in cards {
            if narrow && Some(id) != target {
                continue;
            }
            if narrow {
                pose.x = 0.;
            }
            let radians = pose.angle.to_radians();
            let a = radians.cos() * pose.scale * zoom;
            let b = -radians.sin() * pose.scale * zoom;
            let c = radians.sin() * pose.scale * zoom;
            let d = radians.cos() * pose.scale * zoom;
            let anchor_x = f32::from(bounds.center().x) + pose.x * zoom;
            let anchor_y = f32::from(bounds.bottom()) + pose.y * zoom;
            let (sx, sy) = (pose.width / 56., pose.height / 74.);
            let outer = Affine {
                a: a * sx,
                b: b * sy,
                c: c * sx,
                d: d * sy,
                x: anchor_x - a * pose.width / 2. - b * pose.height,
                y: anchor_y - c * pose.width / 2. - d * pose.height,
            };
            let compression = (std::f32::consts::PI * pose.flip).cos().abs();
            let face = if candidate && pose.flip > 0.5 {
                outer.resize(56. / 120., 74. / 112.).face(compression, 60.)
            } else {
                outer.face(compression, 28.)
            };
            result.cards.push(CardFrame {
                id,
                outer,
                face,
                pose,
                selected,
                highlighted,
                sprites,
            });
        }
        result
            .cards
            .sort_by_key(|card| (card.selected, card.highlighted));
        result
    }

    pub(super) fn viewport(&self) -> Bounds<Pixels> {
        self.viewport
    }

    pub(super) fn deck_contains(&self, position: Point<Pixels>) -> bool {
        self.viewport.contains(&position)
            && (f32::from(position.x) - f32::from(self.bounds.center().x)).abs() <= 180. * self.zoom
            && f32::from(position.y) >= f32::from(self.bounds.bottom()) - 154. * self.zoom
    }

    pub(super) fn hit(&self, position: Point<Pixels>) -> Option<(usize, bool)> {
        if !self.viewport.contains(&position) {
            return None;
        }
        for card in self.cards.iter().rev() {
            if let Some([x, y]) = card.outer.inverse(position) {
                if !(0. ..=56.).contains(&x) || !(0. ..=74.).contains(&y) {
                    continue;
                }
                let dx = (x - x.clamp(10., 46.)).abs();
                let dy = (y - y.clamp(10., 64.)).abs();
                if dx * dx + dy * dy > 100. {
                    continue;
                }
                let restore = card.selected
                    && card.pose.flip > 0.5
                    && card
                        .face
                        .inverse(position)
                        .map(|[x, y]| {
                            if self.candidate {
                                (36. ..=84.).contains(&x) && (86. ..=106.).contains(&y)
                            } else {
                                (4. ..=52.).contains(&x) && (51.5..=66.).contains(&y)
                            }
                        })
                        .unwrap_or(false);
                return Some((card.id, restore));
            }
        }
        None
    }

    pub(super) fn paint(
        &self,
        tokens: Tokens,
        window: &mut Window,
        cx: &App,
    ) -> Result<(), String> {
        if self.fan > 0. {
            let mut aura = tokens;
            aura.warm.a *= self.fan;
            self.halo(false, aura, window, cx)?;
        }
        if self.park {
            self.halo(true, tokens, window, cx)?;
        }
        // 固定最大倍率的 atlas entry；仅 zoom/DPI 改变时重新 rasterize。
        // 动画帧只变矩阵与着色，绝不逐帧 rasterize 文本整卡。
        let raster_scale = self.zoom * 1.58;
        // Scene 按变换前 bounds 进行裁切；源矩形必须落在当前可见 mask 内。
        // 每个重叠图元独占顺序层；paint_layer 不能将重叠图元批成同一 draw order。
        let source = self.viewport.origin.map(|value| {
            px((f32::from(value) * window.scale_factor()).round() / window.scale_factor())
        });
        for card in &self.cards {
            let back = card.pose.flip > 0.5;
            for sprite in card.sprites.iter() {
                if sprite.back.is_some_and(|side| side != back) {
                    continue;
                }
                if sprite
                    .candidate
                    .is_some_and(|candidate| candidate != self.candidate)
                {
                    continue;
                }
                let mut color = match sprite.layer {
                    Layer::Surface => {
                        tokens
                            .surface
                            .opacity(if sprite.back.is_some() { 0.96 } else { 0.90 })
                    }
                    Layer::Border => {
                        if card.selected || card.highlighted {
                            tokens.accent
                        } else {
                            tokens.border
                        }
                    }
                    Layer::Ink => tokens.foreground,
                    Layer::Muted => tokens.muted,
                    Layer::Restore => tokens.accent,
                    Layer::RestoreInk => tokens.accent_ink,
                    Layer::Shadow if !card.selected => tokens.shadow.opacity(0.24),
                    Layer::SelectedShadow if card.selected => tokens.shadow.opacity(0.36),
                    Layer::Shadow | Layer::SelectedShadow => continue,
                };
                if self.cards.iter().any(|card| card.selected) && !card.selected {
                    color.a *= 0.72;
                }
                let transform = if sprite.back.is_some() {
                    card.face
                } else {
                    card.outer
                };
                // 90° 恰为零面积，避免奇异矩阵；端点仍完全一致。
                if sprite.back.is_some() && card.pose.flip > 0.4999 && card.pose.flip < 0.5001 {
                    continue;
                }
                let raster_bounds = Bounds {
                    origin: source,
                    size: size(
                        px(sprite.viewbox[2] * raster_scale),
                        px(sprite.viewbox[3] * raster_scale),
                    ),
                };
                window
                    .paint_layer(self.viewport, |window| {
                        window.paint_svg(
                            raster_bounds,
                            sprite.key.clone(),
                            Some(sprite.data.as_bytes()),
                            transform.raster(
                                raster_scale,
                                window.scale_factor(),
                                source,
                                sprite.viewbox,
                            ),
                            color,
                            cx,
                        )
                    })
                    .map_err(|error| format!("SVG {}: {error}", sprite.key))?;
            }
        }
        Ok(())
    }

    fn halo(
        &self,
        park: bool,
        tokens: Tokens,
        window: &mut Window,
        cx: &App,
    ) -> Result<(), String> {
        let (width, height) = if park { (360., 170.) } else { (260., 130.) };
        // alpha-mask 分层着色，不能把 Park 文字与暖色胶囊涂成同色。
        let data = if park {
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="360" height="170"><defs><filter id="p" x="-100%" y="-100%" width="300%" height="300%"><feGaussianBlur stdDeviation="7"/></filter></defs><path d="M1 170V169 A179 169 0 0 1 359 169V170" fill="white" fill-opacity=".08" stroke="white" stroke-opacity=".54"/><rect x="151" y="16" width="58" height="24" rx="12" fill="white" fill-opacity=".22" filter="url(#p)"/><rect x="151" y="12" width="58" height="24" rx="12" fill="white" fill-opacity=".82"/></svg>"#
        } else {
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="260" height="130"><path d="M1 130 A129 129 0 0 1 259 130" fill="white" fill-opacity=".07" stroke="white" stroke-opacity=".58"/></svg>"#
        };
        let bounds = Bounds {
            origin: point(
                self.bounds.center().x - px(width * self.zoom / 2.),
                self.bounds.bottom() - px(height * self.zoom),
            ),
            size: size(px(width * self.zoom), px(height * self.zoom)),
        };
        window
            .paint_svg(
                bounds,
                if park {
                    "native-gate/park"
                } else {
                    "native-gate/aura"
                }
                .into(),
                Some(data.as_bytes()),
                TransformationMatrix::unit(),
                tokens.warm,
                cx,
            )
            .map_err(|error| error.to_string())?;
        if park {
            let ink = r#"<svg xmlns="http://www.w3.org/2000/svg" width="360" height="170"><text x="180" y="28" text-anchor="middle" font-family="sans-serif" font-size="11" font-weight="700" fill="white">Park</text></svg>"#;
            window
                .paint_svg(
                    bounds,
                    "native-gate/park-ink".into(),
                    Some(ink.as_bytes()),
                    TransformationMatrix::unit(),
                    tokens.warm_ink,
                    cx,
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}
