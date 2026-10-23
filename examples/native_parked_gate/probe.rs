use gpui_kit::component::{
    ActiveTheme, Sizable, Theme, ThemeMode, button::Button, scroll::ScrollableElement,
};
use gpui_kit::*;
use std::{cell::RefCell, rc::Rc, sync::Arc, time::Instant};

#[path = "graphics.rs"]
mod graphics;
use graphics::{Frame, Pose, Sprite, Tokens};

fn evidence_path() -> std::path::PathBuf {
    std::env::temp_dir().join("lyrel-issue-22/native-cards-diagnostics.txt")
}
const TITLES: [&str; 3] = ["lyrel", "构建日志", "长期运行的中文任务"];
const PATHS: [&str; 3] = [
    "~/src/lyrel",
    "~/项目/原生验证",
    "~/projects/long-path/工作台",
];

struct Startup {
    dark: bool,
    zoom: f32,
    reduced: bool,
    park: bool,
    readable: bool,
    width: f32,
    height: f32,
}

impl Startup {
    fn parse() -> Self {
        let mut options = Self {
            dark: true,
            zoom: 1.,
            reduced: false,
            park: false,
            readable: true,
            width: 1000.,
            height: 720.,
        };
        for argument in std::env::args().skip(1) {
            match argument.as_str() {
                "--theme=dark" => options.dark = true,
                "--theme=light" => options.dark = false,
                "--reduced-motion" => options.reduced = true,
                "--park" => options.park = true,
                "--readable-preview" => options.readable = true,
                "--reference-preview" => options.readable = false,
                _ if argument.starts_with("--zoom=") => {
                    options.zoom = argument[7..]
                        .parse::<f32>()
                        .ok()
                        .filter(|value| value.is_finite() && (0.75..=2.).contains(value))
                        .expect("--zoom requires a number from 0.75 to 2.0");
                }
                _ if argument.starts_with("--size=") => {
                    let (width, height) = argument[7..]
                        .split_once('x')
                        .expect("--size requires WIDTHxHEIGHT");
                    options.width = width
                        .parse::<f32>()
                        .ok()
                        .filter(|value| value.is_finite() && *value >= 440.)
                        .expect("width must be at least 440");
                    options.height = height
                        .parse::<f32>()
                        .ok()
                        .filter(|value| value.is_finite() && *value >= 520.)
                        .expect("height must be at least 520");
                }
                _ => panic!(
                    "unknown probe option: {argument}; use --theme=dark|light --zoom=0.75..2 --size=1000x720 --reduced-motion --park"
                ),
            }
        }
        options
    }
}

#[derive(Clone, Copy)]
struct Motion {
    from: Pose,
    to: Pose,
    start: Instant,
}

impl Motion {
    fn sample(&self, now: Instant, reduced: bool) -> Pose {
        if reduced {
            return self.to;
        }
        // CSS ease 的 cubic-bezier(.25,.1,.25,1)，不是线性或透视替代。
        let elapsed = now.duration_since(self.start).as_secs_f32();
        self.from.interpolate(
            self.to,
            graphics::ease(elapsed / 0.220),
            graphics::ease(elapsed / 0.260),
        )
    }

    fn running(&self, now: Instant, reduced: bool) -> bool {
        !reduced && now.duration_since(self.start).as_secs_f32() < 0.260
    }
}

struct Card {
    id: usize,
    title: String,
    path: &'static str,
    sprites: Arc<Vec<Sprite>>,
    motion: Motion,
}

struct ParkedProbe {
    cards: Vec<Card>,
    next_id: usize,
    active: usize,
    hovered: Option<usize>,
    selected: Option<usize>,
    keyboard: bool,
    fan: bool,
    aura_from: f32,
    aura_to: f32,
    aura_start: Instant,
    park: bool,
    readable: bool,
    dark: bool,
    zoom: f32,
    focus: FocusHandle,
    scroll: ScrollHandle,
    frame: Rc<RefCell<Frame>>,
    last: String,
    paint_error: Rc<RefCell<Option<String>>>,
}

impl ParkedProbe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle().tab_stop(true);
        focus.focus(window, cx);
        let scroll = ScrollHandle::new();
        scroll.scroll_to_bottom();
        let mut view = Self {
            cards: Vec::new(),
            next_id: 0,
            active: 0,
            hovered: None,
            selected: None,
            keyboard: false,
            fan: false,
            park: false,
            dark: true,
            zoom: 1.0,
            readable: false,
            aura_from: 0.,
            aura_to: 0.,
            aura_start: Instant::now(),
            scroll,
            focus,
            frame: Rc::new(RefCell::new(Frame::default())),
            last: "独立绘制探针；无会话、无 shell、无产品停放逻辑".into(),
            paint_error: Rc::new(RefCell::new(None)),
        };
        view.add_cards(3, cx);
        view
    }

    fn add_cards(&mut self, count: usize, cx: &mut Context<Self>) {
        let now = Instant::now();
        for _ in 0..count {
            let id = self.next_id;
            self.next_id += 1;
            let title = if id < 3 {
                TITLES[id].to_string()
            } else {
                format!("任务 {id} · build")
            };
            let sprites = graphics::card_sprites(id, &title, PATHS[id % 3]);
            self.cards.push(Card {
                id,
                title,
                path: PATHS[id % 3],
                sprites: Arc::new(sprites),
                motion: Motion {
                    from: Pose::default(),
                    to: Pose::default(),
                    start: now,
                },
            });
        }
        self.retarget(cx);
    }

    fn page(&self) -> usize {
        self.active / 3
    }

    fn aura(&self, now: Instant, reduced: bool) -> f32 {
        if reduced {
            self.aura_to
        } else {
            self.aura_from
                + (self.aura_to - self.aura_from)
                    * graphics::ease(now.duration_since(self.aura_start).as_secs_f32() / 0.180)
        }
    }

    fn retarget(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let reduced = cx.reduce_motion();
        let start = self.page() * 3;
        let count = self.cards.len().saturating_sub(start).min(3);
        let fan = self.fan || self.keyboard || self.selected.is_some();
        let aura_to = if fan { 1. } else { 0. };
        if self.aura_to != aura_to {
            self.aura_from = self.aura(now, reduced);
            self.aura_to = aura_to;
            self.aura_start = now;
        }
        for (ix, card) in self.cards.iter_mut().enumerate() {
            let offset = ix as f32 - start as f32 - (count as f32 - 1.) / 2.;
            let highlighted = self.hovered == Some(card.id) || (self.keyboard && ix == self.active);
            let selected = self.selected == Some(card.id);
            let pose = Pose {
                x: offset * if fan { 58. } else { 13. },
                y: if selected {
                    -66.
                } else if highlighted {
                    -30.
                } else {
                    offset.abs() * -8.
                },
                angle: if selected || highlighted {
                    0.
                } else {
                    offset * if fan { 12. } else { 7. }
                },
                scale: if selected {
                    1.58
                } else if highlighted {
                    1.28
                } else {
                    1.
                },
                flip: if selected { 1. } else { 0. },
                width: if selected && self.readable { 120. } else { 56. },
                height: if selected && self.readable { 112. } else { 74. },
            };
            if card.motion.to != pose {
                let from = card.motion.sample(now, reduced);
                card.motion = Motion {
                    from,
                    to: pose,
                    start: now,
                };
            }
        }
        cx.notify();
    }

    fn navigate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.cards.is_empty() {
            return;
        }
        self.active = ix.min(self.cards.len() - 1);
        self.keyboard = true;
        self.hovered = None;
        self.selected = None;
        self.focus.focus(window, cx);
        self.scroll.scroll_to_bottom();
        self.last = format!(
            "键盘目标 #{}：{}；Enter 预览",
            self.cards[self.active].id, self.cards[self.active].title
        );
        self.retarget(cx);
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = None;
        self.hovered = None;
        self.keyboard = true;
        self.focus.focus(window, cx);
        self.last = "预览已关闭；焦点返回牌堆（←/→ 选择）".into();
        self.retarget(cx);
    }

    fn restore(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.cards.iter().position(|card| card.id == id) {
            let card = self.cards.remove(ix);
            self.last = format!(
                "本地探针已移除 #{} {}（{}）；没有恢复真实会话",
                card.id, card.title, card.path
            );
            self.selected = None;
            self.hovered = None;
            self.active = ix.min(self.cards.len().saturating_sub(1));
            self.keyboard = true;
            self.focus.focus(window, cx);
            self.retarget(cx);
        }
    }

    fn activate(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected == Some(id) {
            self.dismiss(window, cx);
        } else {
            self.selected = Some(id);
            if let Some(ix) = self.cards.iter().position(|card| card.id == id) {
                self.active = ix;
            }
            self.focus.focus(window, cx);
            self.last = format!("预览 #{id}；Enter/恢复执行本地移除，Escape 返回牌堆");
            self.retarget(cx);
        }
    }

    fn pointer_move(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let frame = self.frame.borrow();
        let hit = frame.hit(position).map(|hit| hit.0);
        let fan = frame.deck_contains(position) || hit.is_some();
        drop(frame);
        if self.hovered != hit || self.fan != fan {
            self.hovered = hit;
            self.fan = fan;
            self.keyboard = false;
            self.last = format!(
                "pointer ({:.1}, {:.1}) → {:?}；focus={}",
                f32::from(position.x),
                f32::from(position.y),
                hit,
                self.focus.is_focused(window)
            );
            self.retarget(cx);
        }
    }

    fn pointer_down(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hit = self.frame.borrow().hit(position);
        if let Some((id, restore)) = hit {
            self.keyboard = false;
            if restore {
                self.restore(id, window, cx);
            } else {
                self.activate(id, window, cx);
            }
        } else if self.selected.is_some() {
            self.dismiss(window, cx);
        }
    }

    fn change_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dark = !self.dark;
        apply_theme(self.dark, self.zoom, Some(window), cx);
        cx.notify();
    }

    fn change_zoom(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.zoom = (self.zoom + delta).clamp(0.75, 2.0);
        let zoom = self.zoom;
        Theme::update(cx, |theme| theme.font_size = px(16. * zoom));
        cx.notify();
    }

    fn dump(&mut self, window: &Window, cx: &mut Context<Self>) {
        let frame = self.frame.borrow();
        let body = format!(
            "#23 停放卡片原生探针\n平台：{}\n批准设计：docs/research/main-ui-draft.html:705-915,2160-2196\n技术：公开 Window::paint_svg + TransformationMatrix；SVG alpha-mask atlas 缓存；无 perspective 的 rotateY 正交投影\n卡数={} active={} selected={:?} hovered={:?} reduced={} zoom={} scale_factor={} focus={}\n状态={}\n绘制错误={:?}\n{:#?}\n边界：本文件是运行时诊断，不是验收结论；字体替代、阴影、实际命中和双平台仍需主 agent 实窗核验。\n",
            std::env::consts::OS,
            self.cards.len(),
            self.active,
            self.selected,
            self.hovered,
            cx.reduce_motion(),
            self.zoom,
            window.scale_factor(),
            self.focus.is_focused(window),
            self.last,
            self.paint_error.borrow(),
            *frame,
        );
        drop(frame);
        let path = evidence_path();
        self.last = match path
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .and_then(|_| std::fs::write(&path, body))
        {
            Ok(()) => format!("诊断已写入 {}", path.display()),
            Err(error) => format!("诊断写入失败：{error}"),
        };
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if matches!(key, "left" | "right" | "home" | "end" | "enter" | "space")
            && !self.focus.is_focused(window)
        {
            return;
        }
        match key {
            "left" => self.navigate(self.active.saturating_sub(1), window, cx),
            "right" => self.navigate(self.active + 1, window, cx),
            "home" => self.navigate(0, window, cx),
            "end" => self.navigate(self.cards.len().saturating_sub(1), window, cx),
            "enter" | "space" => {
                if let Some(id) = self.selected {
                    self.restore(id, window, cx);
                } else if let Some(card) = self.cards.get(self.active) {
                    self.activate(card.id, window, cx);
                }
            }
            "escape" => self.dismiss(window, cx),
            "f6" => {
                self.keyboard = true;
                self.focus.focus(window, cx);
                self.retarget(cx);
            }
            "pageup" => {
                let offset = self.scroll.offset();
                self.scroll
                    .set_offset(point(offset.x, offset.y + px(120. * self.zoom)));
                cx.notify();
            }
            "pagedown" => {
                let offset = self.scroll.offset();
                self.scroll
                    .set_offset(point(offset.x, offset.y - px(120. * self.zoom)));
                cx.notify();
            }
            "m" => {
                let reduced = !cx.reduce_motion();
                cx.set_reduce_motion(reduced);
                self.retarget(cx);
            }
            "p" => {
                self.park = !self.park;
                cx.notify();
            }
            "v" => {
                self.readable = !self.readable;
                self.last = if self.readable {
                    "已批准可读预览：120×112，保留 rest/hover/flip 拓扑"
                } else {
                    "原型约束：56×74，背面字段被 CSS flex-shrink 裁成细条"
                }
                .into();
                self.retarget(cx);
            }
            "t" => self.change_theme(window, cx),
            "-" => self.change_zoom(-0.125, cx),
            "+" | "=" => self.change_zoom(0.125, cx),
            "d" => self.dump(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for ParkedProbe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let reduced = cx.reduce_motion();
        let start = self.page() * 3;
        let cards: Vec<_> = self
            .cards
            .iter()
            .skip(start)
            .take(3)
            .map(|card| {
                (
                    card.id,
                    card.motion.sample(now, reduced),
                    card.sprites.clone(),
                    self.selected == Some(card.id),
                    self.hovered == Some(card.id)
                        || (self.keyboard
                            && self.cards.get(self.active).map(|c| c.id) == Some(card.id)),
                )
            })
            .collect();
        if self
            .cards
            .iter()
            .any(|card| card.motion.running(now, reduced))
            || (!reduced
                && self.aura_from != self.aura_to
                && now.duration_since(self.aura_start).as_secs_f32() < 0.180)
        {
            window.request_animation_frame();
        }
        let frame = self.frame.clone();
        let weak = cx.entity().downgrade();
        let move_weak = weak.clone();
        let exit_weak = weak.clone();
        let tokens = Tokens::from_theme(cx, self.dark);
        let open = self.aura(now, reduced);
        let park = self.park;
        let readable = self.readable;
        let error = self.paint_error.clone();
        let canvas = canvas(
            move |bounds, window, _| {
                let viewport = bounds.intersect(&window.content_mask().bounds);
                let scene = Frame::new(
                    bounds,
                    viewport,
                    f32::from(window.rem_size()) / 16.,
                    cards,
                    open,
                    park,
                    readable,
                );
                *frame.borrow_mut() = scene.clone();
                scene
            },
            move |_, scene, window, cx| {
                window.with_content_mask(
                    Some(ContentMask {
                        bounds: scene.viewport(),
                    }),
                    |window| {
                        if let Err(message) = scene.paint(tokens, window, cx) {
                            *error.borrow_mut() = Some(message);
                        }
                    },
                );
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = move_weak
                            .update(cx, |this, cx| this.pointer_move(event.position, window, cx));
                    }
                });
                window.on_mouse_event(move |_: &MouseExitEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = exit_weak.update(cx, |this, cx| {
                            if this.hovered.take().is_some() || this.fan {
                                this.fan = false;
                                this.retarget(cx);
                            }
                        });
                    }
                });
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                        let _ = weak
                            .update(cx, |this, cx| this.pointer_down(event.position, window, cx));
                    }
                });
            },
        )
        .size_full()
        .min_h(rems(340. / 16.))
        .flex_shrink_0();
        let paint_error = self.paint_error.borrow().clone();
        let toolbar = div()
            .id("probe-controls")
            .h(rems(5.))
            .flex_shrink_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .p_2()
            .child(Button::new("focus-deck").small().label("牌堆 F6").on_click(
                cx.listener(|this, _, window, cx| this.navigate(this.active, window, cx)),
            ))
            .child(
                Button::new("more")
                    .small()
                    .label("增加至 20 张")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.add_cards(20usize.saturating_sub(this.cards.len()), cx);
                    })),
            )
            .child(
                Button::new("reset")
                    .small()
                    .label("重置 3 张")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.cards.clear();
                        this.active = 0;
                        this.selected = None;
                        this.hovered = None;
                        this.next_id = 0;
                        this.keyboard = false;
                        this.fan = false;
                        this.add_cards(3, cx);
                        this.focus.focus(window, cx);
                    })),
            )
            .child(
                Button::new("previous")
                    .small()
                    .label("上一组")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.navigate(this.active.saturating_sub(3), window, cx)
                    })),
            )
            .child(Button::new("next").small().label("下一组").on_click(
                cx.listener(|this, _, window, cx| this.navigate(this.active + 3, window, cx)),
            ))
            .child(
                Button::new("park")
                    .small()
                    .label(if self.park {
                        "隐藏 Park P"
                    } else {
                        "显示 Park P"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.park = !this.park;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("motion")
                    .small()
                    .label(if reduced {
                        "减少动态 M ✓"
                    } else {
                        "减少动态 M"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        let reduced = !cx.reduce_motion();
                        cx.set_reduce_motion(reduced);
                        this.retarget(cx);
                    })),
            )
            .child(
                Button::new("theme")
                    .small()
                    .label("明暗 T")
                    .on_click(cx.listener(Self::change_theme_click)),
            )
            .child(
                Button::new("readable")
                    .small()
                    .label(if self.readable {
                        "可读预览 V"
                    } else {
                        "原型约束 V"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.readable = !this.readable;
                        this.last = if this.readable {
                            "已批准可读预览：120×112"
                        } else {
                            "原型约束：背面细条是原 HTML 的测量结果"
                        }
                        .into();
                        this.retarget(cx);
                    })),
            )
            .child(
                Button::new("zoom-minus")
                    .small()
                    .label("缩小 −")
                    .on_click(cx.listener(|this, _, _, cx| this.change_zoom(-0.125, cx))),
            )
            .child(
                Button::new("zoom-plus")
                    .small()
                    .label("放大 +")
                    .on_click(cx.listener(|this, _, _, cx| this.change_zoom(0.125, cx))),
            )
            .child(
                Button::new("dump")
                    .small()
                    .label("导出诊断 D")
                    .on_click(cx.listener(|this, _, window, cx| this.dump(window, cx))),
            );
        div().size_full().flex().flex_col().bg(cx.theme().background).text_color(cx.theme().foreground)
            .on_key_down(cx.listener(Self::key_down))
            .child(toolbar.overflow_y_scrollbar())
            .child(div().px_3().h_6().overflow_hidden().text_ellipsis().text_sm().child(format!(
                "#23 停放卡片 · 第 {}/{} 组 · {} 张 · zoom {:.0}% · scale {:.2} · ←/→ Home/End / Enter / Escape",
                self.page() + 1, self.cards.len().div_ceil(3).max(1), self.cards.len(), self.zoom * 100., window.scale_factor())))
            .child(div().px_3().h_6().overflow_hidden().text_ellipsis().text_sm().child(self.last.clone()))
            .child(div().px_3().h_6().overflow_hidden().text_ellipsis().text_sm().child(paint_error.unwrap_or_else(|| "无已报告绘制错误；PageUp/PageDown 滚动受限场景（不等于验收通过）".into())))
            .child(div().id("native-card-stage").track_focus(&self.focus).flex_1().min_h_0().min_w_0()
                .overflow_y_scroll().track_scroll(&self.scroll).vertical_scrollbar(&self.scroll)
                .border_1().border_color(if self.focus.is_focused(window) { cx.theme().primary } else { cx.theme().border })
                .child(canvas))
            .child(div().h(rems(26. / 16.)).flex_shrink_0().px_3().flex().items_center()
                .bg(cx.theme().muted).border_t_1().border_color(cx.theme().primary).text_xs()
                .child("独立 status band · 卡片 / 光环 / 阴影 / 命中不得进入本区"))
    }
}

impl ParkedProbe {
    fn change_theme_click(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.change_theme(window, cx);
    }
}

fn apply_theme(dark: bool, zoom: f32, window: Option<&mut Window>, cx: &mut App) {
    Theme::change(
        if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        window,
        cx,
    );
    // 集中主题 token：与批准 HTML 的同名角色对应；不加载用户主题或配置。
    Theme::update(cx, |theme| {
        theme.background = rgb(if dark { 0x18181a } else { 0xfcfcfb }).into();
        theme.foreground = rgb(if dark { 0xececed } else { 0x1c1c1e }).into();
        theme.popover = rgb(if dark { 0x242429 } else { 0xffffff }).into();
        theme.muted = rgb(if dark { 0x26262a } else { 0xeeeeec }).into();
        theme.muted_foreground = rgb(if dark { 0xa1a1aa } else { 0x62626a }).into();
        theme.border = rgb(if dark { 0x34343a } else { 0xd7d7d4 }).into();
        theme.primary = rgb(if dark { 0x78a8f5 } else { 0x1f6bf0 }).into();
        theme.primary_foreground = rgb(if dark { 0x10131a } else { 0xffffff }).into();
        theme.font_size = px(16. * zoom);
    });
}

pub(super) fn run() {
    let options = Startup::parse();
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            apply_theme(options.dark, options.zoom, None, cx);
            if options.reduced {
                cx.set_reduce_motion(true);
            }
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(options.width), px(options.height)),
                        cx,
                    ))),
                    window_min_size: Some(size(px(440.), px(520.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("#23 Native parked-card gate".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        let mut probe = ParkedProbe::new(window, cx);
                        probe.dark = options.dark;
                        probe.zoom = options.zoom;
                        probe.park = options.park;
                        probe.readable = options.readable;
                        probe
                    })
                },
            )
            .expect("failed to open native parked-card probe");
            cx.activate(true);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
        });
}
