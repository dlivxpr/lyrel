//! 原生停放牌堆。App 持有一个 Entity，注入当前 tab 的真实卡片与本地化文案，订阅恢复意图。
//! 此模块不访问 daemon、PTY、文件系统或会话生命周期；Restore 只请求改变展示。
//! 所有 pane 参数都是不透明的客户端卡片 token，不是 daemon pane id；禁止持久化或传给 daemon。
//! 将 view 放在 workbench-content 内的绝对定位 overlay；status bar 必须是 content 的兄弟。

use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit::{
    base::Selectable as _,
    component::{
        ActiveTheme, FocusTrapElement, Sizable,
        button::{Button, ButtonVariants},
        scroll::Scrollbar,
        v_flex,
    },
};
use lyrel_core::core::machine::TabId;
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc, time::Instant};

mod graphics;
mod keyboard_region;
use graphics::{Frame, FrameCard, Pose, Sprite, Tokens};
use keyboard_region::KeyboardRegion;

/// 当前 tab 中稳定的客户端卡片身份（如 PaneSlot.entity_id.as_u64()）。
/// App 必须在本地/远端混合场景提供唯一 token；alive 只影响标签，不禁止查看已退出会话的 scrollback。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParkedCard {
    pane: u64,
    title: SharedString,
    path: SharedString,
    alive: bool,
    status: Option<SharedString>,
}

impl ParkedCard {
    pub fn new(
        pane: u64,
        title: impl Into<SharedString>,
        path: impl Into<SharedString>,
        alive: bool,
    ) -> Self {
        Self {
            pane,
            title: title.into(),
            path: path.into(),
            alive,
            status: None,
        }
    }

    pub fn with_status(mut self, status: impl Into<SharedString>) -> Self {
        self.status = Some(status.into());
        self
    }

    fn status<'a>(&'a self, labels: &'a ParkedDeckLabels) -> &'a str {
        self.status.as_deref().unwrap_or(if self.alive {
            &labels.alive
        } else {
            &labels.exited
        })
    }

    /// 返回 App 提供的不透明客户端 token；恢复会话的 Host/daemon 身份由 App 查找对应 slot。
    pub fn pane(&self) -> u64 {
        self.pane
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn is_alive(&self) -> bool {
        self.alive
    }
}

/// 由现有 L10nKey 系统解析后注入；更换语言时重新 set_labels，不把文案写入共享状态。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParkedDeckLabels {
    title: SharedString,
    restore: SharedString,
    park: SharedString,
    alive: SharedString,
    exited: SharedString,
    more: SharedString,
    hint: SharedString,
}

impl ParkedDeckLabels {
    pub fn new(
        title: impl Into<SharedString>,
        restore: impl Into<SharedString>,
        park: impl Into<SharedString>,
        alive: impl Into<SharedString>,
        exited: impl Into<SharedString>,
    ) -> Self {
        Self {
            title: title.into(),
            restore: restore.into(),
            park: park.into(),
            alive: alive.into(),
            exited: exited.into(),
            more: SharedString::default(),
            hint: SharedString::default(),
        }
    }

    pub fn with_more(
        mut self,
        more: impl Into<SharedString>,
        hint: impl Into<SharedString>,
    ) -> Self {
        self.more = more.into();
        self.hint = hint.into();
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParkedDeckEvent {
    /// 回传不透明客户端 token。App 查找对应 slot 并确认其仍属于当前 tab 的 parked 集合，
    /// 再解析真实 Host/daemon 身份；不得直接把 token 传给 daemon 或创建/重启不存在的会话。
    Restore(u64),
    Dismiss,
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
        let elapsed = now.duration_since(self.start).as_secs_f32();
        self.from.interpolate(
            self.to,
            graphics::ease(elapsed / 0.220),
            graphics::ease(elapsed / 0.260),
        )
    }

    fn running(&self, now: Instant, reduced: bool) -> bool {
        !reduced && self.from != self.to && now.duration_since(self.start).as_secs_f32() < 0.260
    }
}

struct Card {
    model: ParkedCard,
    sprites: Arc<Vec<Sprite>>,
    dirty: bool,
    motion: Motion,
}

/// 所有 hover、键盘目标、焦点、翻面和动画状态只存在于这个 GUI entity。
/// new 后先 set_labels，再 set_tab/sync_cards；每次切换 tab 都调用 set_tab。
/// 保持 entity，不要在 App::render 里重建它。
pub struct ParkedDeck {
    tab: Option<TabId>,
    cards: Vec<Card>,
    labels: ParkedDeckLabels,
    font: SharedString,
    park_ink: Option<Sprite>,
    active: Option<u64>,
    hovered: Option<u64>,
    selected: Option<u64>,
    pointer_fan: bool,
    keyboard: bool,
    drop_target: bool,
    overflow_open: bool,
    content_size: Size<Pixels>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    trigger: Option<FocusHandle>,
    rem: f32,
    aura_from: f32,
    aura_to: f32,
    aura_start: Instant,
    frame: Rc<RefCell<Rc<Frame>>>,
    paint_error: Rc<RefCell<Option<String>>>,
}

impl EventEmitter<ParkedDeckEvent> for ParkedDeck {}

impl Focusable for ParkedDeck {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ParkedDeck {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            tab: None,
            cards: Vec::new(),
            labels: ParkedDeckLabels::default(),
            font: SharedString::default(),
            park_ink: None,
            active: None,
            hovered: None,
            selected: None,
            pointer_fan: false,
            keyboard: false,
            drop_target: false,
            focus: cx.focus_handle().tab_stop(true),
            trigger: None,
            overflow_open: false,
            content_size: size(px(0.), px(0.)),
            scroll: UniformListScrollHandle::new(),
            rem: 16.,
            aura_from: 0.,
            aura_to: 0.,
            aura_start: Instant::now(),
            frame: Rc::new(RefCell::new(Rc::default())),
            paint_error: Rc::new(RefCell::new(None)),
        }
    }

    /// tab 变化时丢弃客户端瞬态和旧命中；不 emit Dismiss、不把焦点抢回旧 tab。
    pub fn set_tab(&mut self, tab: Option<TabId>, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.cards.clear();
        self.clear_interaction();
        cx.notify();
    }

    fn clear_interaction(&mut self) {
        self.active = None;
        self.hovered = None;
        self.selected = None;
        self.pointer_fan = false;
        self.keyboard = false;
        self.drop_target = false;
        self.overflow_open = false;
        self.trigger = None;
        self.aura_from = 0.;
        self.aura_to = 0.;
        *self.frame.borrow_mut() = Rc::default();
    }

    /// 相同输入不 notify；按客户端 token 重用资源和进行中的动画，重排不改变选中会话。
    pub fn sync_cards(
        &mut self,
        cards: Vec<ParkedCard>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cards.len() == cards.len()
            && self
                .cards
                .iter()
                .zip(&cards)
                .all(|(old, new)| old.model == *new)
        {
            return;
        }
        let now = Instant::now();
        let mut previous: HashMap<_, _> = std::mem::take(&mut self.cards)
            .into_iter()
            .map(|card| (card.model.pane, card))
            .collect();
        self.cards = cards
            .into_iter()
            .map(|model| {
                if let Some(mut card) = previous.remove(&model.pane) {
                    card.dirty |= card.model != model;
                    card.model = model;
                    card
                } else {
                    Card {
                        model,
                        sprites: Arc::new(Vec::new()),
                        dirty: true,
                        motion: Motion {
                            from: Pose::default(),
                            to: Pose::default(),
                            start: now,
                        },
                    }
                }
            })
            .collect();
        let contains = |pane| self.cards.iter().any(|card| card.model.pane == pane);
        let selection_removed = self.selected.is_some_and(|pane| !contains(pane));
        if self.hovered.is_some_and(|pane| !contains(pane)) {
            self.hovered = None;
        }
        if self.active.is_none_or(|pane| !contains(pane)) {
            self.active = self.cards.first().map(|card| card.model.pane);
        }
        if self.overflow_open
            && let Some(ix) = self
                .cards
                .iter()
                .position(|card| Some(card.model.pane) == self.active)
        {
            self.scroll.scroll_to_item(ix, ScrollStrategy::Top);
        }
        if selection_removed || self.cards.is_empty() {
            self.dismiss(window, cx);
        } else {
            self.update_targets(cx.reduce_motion(), self.focus.is_focused(window));
            cx.notify();
        }
        // 关闭的 pane 不能在下一次绘制前通过旧 frame 被点击。
        *self.frame.borrow_mut() = Rc::default();
    }

    pub fn set_labels(&mut self, labels: ParkedDeckLabels, cx: &mut Context<Self>) {
        if self.labels == labels {
            return;
        }
        self.labels = labels;
        for card in &mut self.cards {
            card.dirty = true;
        }
        self.park_ink = None;
        cx.notify();
    }

    pub fn set_drop_target(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.drop_target == visible {
            return;
        }
        self.drop_target = visible;
        cx.notify();
    }

    pub fn is_drop_target_visible(&self) -> bool {
        self.drop_target
    }

    pub fn drop_target_contains(&self, position: Point<Pixels>) -> bool {
        self.drop_target && self.frame.borrow().park_target_contains(position)
    }

    pub fn cards(&self) -> impl ExactSizeIterator<Item = &ParkedCard> {
        self.cards.iter().map(|card| &card.model)
    }
    pub fn selected_pane(&self) -> Option<u64> {
        self.selected
    }
    pub fn last_paint_error(&self) -> Option<String> {
        self.paint_error.borrow().clone()
    }

    /// trigger 省略时记录当前真实焦点；既不默认 focus 新建 view，也不从 render 抢焦点。
    pub fn focus(
        &mut self,
        trigger: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cards.is_empty() {
            return;
        }
        self.retain_trigger(trigger, window, cx);
        self.keyboard = true;
        self.focus.focus(window, cx);
        self.update_targets(cx.reduce_motion(), true);
        cx.notify();
    }

    pub fn preview(
        &mut self,
        pane: u64,
        trigger: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.cards.iter().any(|card| card.model.pane == pane) {
            return;
        }
        self.retain_trigger(trigger, window, cx);
        self.active = Some(pane);
        self.selected = Some(pane);
        self.focus.focus(window, cx);
        self.update_targets(cx.reduce_motion(), true);
        cx.notify();
    }

    fn retain_trigger(&mut self, trigger: Option<FocusHandle>, window: &Window, cx: &App) {
        if self.overflow_open {
            return;
        }
        if trigger.is_some() {
            self.trigger = trigger;
        } else if !self.focus.contains_focused(window, cx) {
            self.trigger = window.focused(cx);
        }
    }

    /// Escape/outside 共同入口；清理状态后恢复 trigger，再向 App 发出 Dismiss。
    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let was_open = self.selected.is_some()
            || self.overflow_open
            || self.keyboard
            || self.trigger.is_some();
        self.selected = None;
        self.hovered = None;
        self.pointer_fan = false;
        self.keyboard = false;
        let lost_more = self.overflow_open && self.cards.len() <= self.deck_capacity();
        self.overflow_open = false;
        if self.focus.contains_focused(window, cx) {
            if self.cards.is_empty() {
                window.blur(cx);
                self.trigger = None;
            } else if lost_more {
                self.focus.focus(window, cx);
                self.trigger = None;
            } else if let Some(trigger) = self.trigger.take() {
                trigger.focus(window, cx);
            }
        }
        self.update_targets(cx.reduce_motion(), false);
        if was_open {
            cx.emit(ParkedDeckEvent::Dismiss);
        }
        cx.notify();
    }

    fn restore(&mut self, pane: u64, cx: &mut Context<Self>) {
        if self.cards.iter().any(|card| card.model.pane == pane) {
            // 不乐观移除卡片。请求失败时仍保留身份、预览和原来的会话。
            cx.emit(ParkedDeckEvent::Restore(pane));
        }
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

    fn update_targets(&mut self, reduced: bool, focused: bool) {
        let now = Instant::now();
        let keyboard = self.keyboard && focused;
        let fan = self.pointer_fan || keyboard || self.selected.is_some();
        let aura_to = if fan { 1. } else { 0. };
        if self.aura_to != aura_to {
            self.aura_from = self.aura(now, reduced);
            self.aura_to = aura_to;
            self.aura_start = now;
        }
        let capacity = self.deck_capacity();
        let count = self.cards.len().min(capacity);
        let target = self.hovered.or(if keyboard { self.active } else { None });
        for (ix, card) in self.cards.iter_mut().enumerate() {
            let offset = ix as f32 - (count as f32 - 1.) / 2.;
            let highlighted = target == Some(card.model.pane);
            let selected = self.selected == Some(card.model.pane);
            let pose = Pose {
                x: offset * if fan { 58. } else { 13. },
                y: if selected {
                    -66.
                } else if highlighted {
                    -30.
                } else {
                    offset.abs().min(3.) * -8.
                },
                angle: if selected || highlighted {
                    0.
                } else {
                    offset.clamp(-3., 3.) * if fan { 12. } else { 7. }
                },
                scale: if selected {
                    1.58
                } else if highlighted {
                    1.28
                } else {
                    1.
                },
                flip: if selected { 1. } else { 0. },
                width: if selected { 120. } else { 56. },
                height: if selected { 112. } else { 74. },
            };
            if card.motion.to != pose {
                card.motion = Motion {
                    from: card.motion.sample(now, reduced),
                    to: pose,
                    start: now,
                };
            }
        }
    }

    fn navigate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.cards.is_empty() {
            return;
        }
        self.retain_trigger(None, window, cx);
        self.active = Some(self.cards[ix.min(self.cards.len() - 1)].model.pane);
        if ix >= self.deck_capacity() {
            self.overflow_open = true;
        }
        if self.overflow_open {
            self.scroll
                .scroll_to_item(ix.min(self.cards.len() - 1), ScrollStrategy::Top);
        }
        self.selected = None;
        self.hovered = None;
        self.keyboard = true;
        self.focus.focus(window, cx);
        self.update_targets(cx.reduce_motion(), true);
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.contains_focused(window, cx) {
            return;
        }
        if event.keystroke.modifiers.control
            || event.keystroke.modifiers.platform
            || event.keystroke.modifiers.alt
        {
            return;
        }
        let ix = self
            .cards
            .iter()
            .position(|card| Some(card.model.pane) == self.active)
            .unwrap_or(0);
        match event.keystroke.key.as_str() {
            "left" | "up" => self.navigate(ix.saturating_sub(1), window, cx),
            "right" | "down" => self.navigate(ix + 1, window, cx),
            "home" => self.navigate(0, window, cx),
            "end" => self.navigate(self.cards.len().saturating_sub(1), window, cx),
            "pageup" => self.navigate(ix.saturating_sub(5), window, cx),
            "pagedown" => self.navigate(ix + 5, window, cx),
            "enter" | "space" => {
                if let Some(pane) = self.selected {
                    self.restore(pane, cx);
                } else if let Some(pane) = self.active {
                    self.preview(pane, None, window, cx);
                }
            }
            "escape" if self.overflow_open && self.selected.is_some() => {
                self.close_preview(window, cx)
            }
            "escape" => self.dismiss(window, cx),
            "tab" if self.selected.is_some() => {
                // 翻面预览只有一个主操作，Tab 保持在它的 keyboard/AX owner；Escape 可退出。
                self.keyboard = true;
                self.update_targets(cx.reduce_motion(), true);
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn pointer_move(&mut self, position: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        if self.overflow_open {
            return;
        }
        let frame = self.frame.borrow();
        let hit = frame.hit(position).map(|hit| hit.0);
        let fan = !self.cards.is_empty() && (frame.deck_contains(position) || hit.is_some());
        drop(frame);
        if self.hovered == hit && self.pointer_fan == fan {
            return;
        }
        self.hovered = hit;
        self.pointer_fan = fan;
        self.keyboard = false;
        self.update_targets(cx.reduce_motion(), self.focus.is_focused(window));
        cx.notify();
    }

    fn pointer_down(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overflow_open && self.selected.is_none() {
            return;
        }
        let hit = self.frame.borrow().hit(position);
        if let Some((pane, restore)) = hit {
            self.keyboard = false;
            if restore {
                self.restore(pane, cx);
            } else if self.selected == Some(pane) {
                if self.overflow_open {
                    self.close_preview(window, cx);
                } else {
                    self.dismiss(window, cx);
                }
            } else {
                self.preview(pane, None, window, cx);
            }
            window.prevent_default();
            cx.stop_propagation();
        } else if self.selected.is_some() {
            if self.overflow_open {
                self.close_preview(window, cx);
            } else {
                self.dismiss(window, cx);
            }
        }
    }

    fn pointer_exit(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.hovered.is_none() && !self.pointer_fan {
            return;
        }
        self.hovered = None;
        self.pointer_fan = false;
        self.update_targets(cx.reduce_motion(), self.focus.is_focused(window));
        cx.notify();
    }

    fn close_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = None;
        self.focus.focus(window, cx);
        let ix = self
            .cards
            .iter()
            .position(|card| Some(card.model.pane) == self.active)
            .unwrap_or(0);
        self.scroll.scroll_to_item(ix, ScrollStrategy::Top);
        self.update_targets(cx.reduce_motion(), true);
        cx.notify();
    }

    // The fan keeps its approved 58-unit spacing and 32-unit usable targets.
    // Budget for rotated/lifted outer cards instead of shrinking the cards.
    fn deck_capacity(&self) -> usize {
        let zoom = self.zoom();
        if f32::from(self.content_size.height) / zoom < 126. {
            return 0;
        }
        let width = f32::from(self.content_size.width) / zoom;
        let fits = |width: f32| {
            if width < 210. {
                0
            } else {
                ((width - 210.) / 58.).floor() as usize + 1
            }
        };
        let comfortable = fits(width);
        if self.cards.len() <= comfortable {
            comfortable
        } else {
            fits(width - 104.)
        }
    }

    fn zoom(&self) -> f32 {
        self.rem / 16.
    }

    fn open_overflow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cards.is_empty() {
            return;
        }
        self.retain_trigger(None, window, cx);
        self.overflow_open = true;
        self.selected = None;
        self.hovered = None;
        self.keyboard = true;
        if self.active.is_none() {
            self.active = self.cards.first().map(|card| card.model.pane);
        }
        self.focus.focus(window, cx);
        let ix = self
            .cards
            .iter()
            .position(|card| Some(card.model.pane) == self.active)
            .unwrap_or(0);
        self.scroll.scroll_to_item(ix, ScrollStrategy::Top);
        cx.notify();
    }

    fn render_overflow(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.weak_entity();
        let count = self.cards.len();
        v_flex()
            .id("parked-overflow")
            .test_support()
            .absolute()
            .inset_2()
            .max_w(rems(32.))
            .max_h(rems(28.))
            .min_w_0()
            .min_h_0()
            .bg(cx.theme().popover)
            .text_color(cx.theme().foreground)
            .border_1()
            .border_color(cx.theme().border)
            .rounded_lg()
            .overflow_hidden()
            .track_focus(&self.focus)
            .role(Role::Group)
            .aria_label(self.labels.title.clone())
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.dismiss(window, cx)))
            .child(
                div()
                    .p_2()
                    .text_sm()
                    .child(format!("{} · {}", self.labels.title, count)),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        uniform_list("parked-overflow-list", count, move |range, _, cx| {
                            let Some(deck) = weak.upgrade() else {
                                return Vec::new();
                            };
                            let this = deck.read(cx);
                            range
                                .filter_map(|ix| this.cards.get(ix))
                                .map(|card| {
                                    let pane = card.model.pane;
                                    let row_weak = weak.clone();
                                    let active = this.active == Some(pane);
                                    let title = card.model.title.clone();
                                    let details = format!(
                                        "{} · {}",
                                        card.model.status(&this.labels),
                                        card.model.path
                                    );
                                    Button::new(("parked-overflow-row", pane as usize))
                                        .ghost()
                                        .w_full()
                                        .h(rems(3.75))
                                        .min_w_0()
                                        .px_2()
                                        .py_1()
                                        .when(active, |row| {
                                            row.bg(cx.theme().secondary)
                                                .border_1()
                                                .border_color(cx.theme().primary)
                                        })
                                        .selected(active)
                                        .accessibility_label(format!("{}\n{}", title, details))
                                        .tooltip(format!("{}\n{}", title, details))
                                        .on_click(move |_, window, cx| {
                                            let _ = row_weak.update(cx, |this, cx| {
                                                this.preview(pane, None, window, cx)
                                            });
                                        })
                                        .child(
                                            v_flex()
                                                .w_full()
                                                .min_w_0()
                                                .gap_1()
                                                .child(div().text_sm().text_ellipsis().child(title))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .text_ellipsis()
                                                        .child(details),
                                                ),
                                        )
                                        .into_any_element()
                                })
                                .collect()
                        })
                        .track_scroll(&self.scroll)
                        .size_full(),
                    )
                    .child(Scrollbar::vertical(&self.scroll).viewport_from_layout()),
            )
            .child(
                div()
                    .p_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.labels.hint.clone()),
            )
            .focus_trap("parked-overflow", &self.focus)
            .into_any_element()
    }

    fn refresh_resources(&mut self, cx: &App) {
        let font = cx.theme().font_family.clone();
        if self.font != font {
            self.font = font;
            for card in &mut self.cards {
                card.dirty = true;
            }
            self.park_ink = None;
        }
        let capacity = self.deck_capacity();
        for (ix, card) in self.cards.iter_mut().enumerate() {
            if card.dirty && (ix < capacity || self.selected == Some(card.model.pane)) {
                card.sprites = Arc::new(graphics::card_sprites(
                    &card.model,
                    &self.labels,
                    &self.font,
                ));
                card.dirty = false;
            }
        }
        if self.park_ink.is_none() {
            self.park_ink = Some(graphics::park_label(&self.labels.park, &self.font));
        }
    }
}

impl Render for ParkedDeck {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.cards.is_empty() && !self.drop_target {
            return div()
                .id("parked-deck")
                .test_support()
                .size_full()
                .into_any_element();
        }
        self.rem = f32::from(window.rem_size());
        let capacity = self.deck_capacity();
        self.refresh_resources(cx);
        let overflow = self.overflow_open && self.selected.is_none();
        let more = self.cards.len() > capacity || self.overflow_open;
        let overflow_list = overflow.then(|| self.render_overflow(cx));
        let more_button = more.then(|| {
            Button::new("parked-more")
                .ghost()
                .small()
                .w(rems(6.))
                .label(self.labels.more.clone())
                .accessibility_label(format!("{}: {}", self.labels.more, self.labels.title))
                .on_click(cx.listener(|this, _, window, cx| this.open_overflow(window, cx)))
        });
        if self.focus.is_focused(window) && window.last_input_was_keyboard() {
            self.keyboard = true;
        }
        self.update_targets(cx.reduce_motion(), self.focus.is_focused(window));
        let now = Instant::now();
        let reduced = cx.reduce_motion();
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
        let keyboard = self.keyboard && self.focus.is_focused(window);
        let target = self.hovered.or(if keyboard { self.active } else { None });
        let cards = self
            .cards
            .iter()
            .enumerate()
            .filter(|(ix, card)| *ix < capacity || self.selected == Some(card.model.pane))
            .map(|(_, card)| FrameCard {
                model: card.model.clone(),
                pose: card.motion.sample(now, reduced),
                sprites: card.sprites.clone(),
                selected: self.selected == Some(card.model.pane),
                highlighted: target == Some(card.model.pane),
            })
            .collect();
        let labels = self.labels.clone();
        let focus = self.focus.clone();
        let frame = self.frame.clone();
        let weak = cx.entity().downgrade();
        let prepaint_weak = weak.clone();
        let error = self.paint_error.clone();
        let tokens = Tokens::from_theme(cx);
        let aura = self.aura(now, reduced);
        let park = self.drop_target;
        let has_cards = !self.cards.is_empty() && !overflow;
        let measure = cx.weak_entity();
        let park_ink = self
            .park_ink
            .clone()
            .expect("park label refreshed before paint");
        div()
            .id("parked-deck")
            .test_support()
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let viewport = bounds.intersect(&window.content_mask().bounds);
                        let size = viewport.size;
                        let measure = measure.clone();
                        cx.defer(move |cx| {
                            let _ = measure.update(cx, |this, cx| {
                                if this.content_size != size {
                                    this.content_size = size;
                                    *this.frame.borrow_mut() = Rc::default();
                                    cx.notify();
                                }
                            });
                        });
                        let mut deck_bounds = bounds;
                        if more {
                            deck_bounds.size.width =
                                (deck_bounds.size.width - window.rem_size() * 6.5).max(px(0.));
                        }
                        let scene = Frame::new(
                            bounds,
                            viewport,
                            f32::from(window.rem_size()) / 16.,
                            cards,
                            aura,
                            park,
                            park_ink,
                            deck_bounds,
                        );
                        let park_hover =
                            park && scene.park_target_contains(window.mouse_position());
                        let scene = Rc::new(scene.with_drop_hover(park_hover));
                        *frame.borrow_mut() = scene.clone();
                        let group_bounds = scene.keyboard_bounds();
                        let key_weak = prepaint_weak.clone();
                        let mut semantics = div()
                            .relative()
                            .w(group_bounds.size.width)
                            .h(group_bounds.size.height);
                        if park {
                            for (id, bounds) in [
                                ("park-drop-target", scene.park_target_bounds()),
                                ("park-drop-pill", scene.park_pill_bounds()),
                            ] {
                                semantics = semantics.child(
                                    div()
                                        .id(id)
                                        .test_support()
                                        .absolute()
                                        .left(bounds.left() - group_bounds.left())
                                        .top(bounds.top() - group_bounds.top())
                                        .w(bounds.size.width)
                                        .h(bounds.size.height)
                                        .role(Role::Group)
                                        .aria_label(labels.park.clone()),
                                );
                            }
                        }
                        for (model, bounds, selected, restore) in
                            scene.semantic_cards().filter(|_| has_cards)
                        {
                            if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
                                continue;
                            }
                            let pane = model.pane;
                            let status = model.status(&labels);
                            let card_weak = prepaint_weak.clone();
                            let mut card = div()
                                .id(("parked-card", pane as usize))
                                .test_support()
                                .absolute()
                                .left(bounds.left() - group_bounds.left())
                                .top(bounds.top() - group_bounds.top())
                                .w(bounds.size.width)
                                .h(bounds.size.height)
                                .role(Role::Button)
                                .aria_label(format!("{}\n{}\n{}", model.title, model.path, status))
                                .aria_selected(selected)
                                .on_a11y_action(AccessibleAction::Click, move |_, window, cx| {
                                    let _ = card_weak.update(cx, |this, cx| {
                                        this.preview(pane, None, window, cx)
                                    });
                                });
                            if !selected && target == Some(pane) {
                                card = card.aria_active_descendant();
                            }
                            if selected {
                                card = card.child(
                                    div()
                                        .id("parked-preview")
                                        .test_support()
                                        .absolute()
                                        .size_full()
                                        .role(Role::Group)
                                        .aria_label(model.title.clone()),
                                );
                            }
                            if let Some(restore) = restore {
                                let restore_weak = prepaint_weak.clone();
                                card = card.child(
                                    div()
                                        .id("parked-restore")
                                        .test_support()
                                        .absolute()
                                        .left(restore.left() - bounds.left())
                                        .top(restore.top() - bounds.top())
                                        .w(restore.size.width)
                                        .h(restore.size.height)
                                        .role(Role::Button)
                                        .aria_label(format!("{}: {}", labels.restore, model.title))
                                        .aria_active_descendant()
                                        .on_a11y_action(
                                            AccessibleAction::Click,
                                            move |_, _, cx| {
                                                let _ = restore_weak
                                                    .update(cx, |this, cx| this.restore(pane, cx));
                                            },
                                        ),
                                );
                            }
                            semantics = semantics.child(card);
                        }
                        let mut semantics = if has_cards {
                            KeyboardRegion::new(semantics.into_any_element(), labels.title)
                                .track_focus(&focus)
                                .on_key_down(move |event, window, cx| {
                                    let _ = key_weak
                                        .update(cx, |this, cx| this.key_down(event, window, cx));
                                })
                                .into_any_element()
                        } else {
                            semantics.into_any_element()
                        };
                        window.with_content_mask(
                            Some(ContentMask { bounds: viewport }),
                            |window| {
                                semantics.prepaint_as_root(
                                    group_bounds.origin,
                                    group_bounds.size.map(AvailableSpace::Definite),
                                    window,
                                    cx,
                                );
                            },
                        );
                        (scene, semantics)
                    },
                    move |_, (scene, mut semantics), window, cx| {
                        window.with_content_mask(
                            Some(ContentMask {
                                bounds: scene.viewport(),
                            }),
                            |window| {
                                match scene.paint(tokens, window, cx) {
                                    Ok(()) => {
                                        *error.borrow_mut() = None;
                                    }
                                    Err(message) => {
                                        if error.borrow().as_ref() != Some(&message) {
                                            log::error!("parked deck paint failed: {message}");
                                        }
                                        *error.borrow_mut() = Some(message);
                                    }
                                }
                                semantics.paint(window, cx);
                            },
                        );
                        let move_weak = weak.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = move_weak.update(cx, |this, cx| {
                                    this.pointer_move(event.position, window, cx)
                                });
                            }
                        });
                        let exit_weak = weak.clone();
                        window.on_mouse_event(move |_: &MouseExitEvent, phase, window, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ =
                                    exit_weak.update(cx, |this, cx| this.pointer_exit(window, cx));
                            }
                        });
                        let outside_weak = weak.clone();
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ = outside_weak.update(cx, |this, cx| {
                                    if !this.overflow_open
                                        && this.selected.is_some()
                                        && this.frame.borrow().hit(event.position).is_none()
                                    {
                                        this.dismiss(window, cx);
                                    }
                                });
                            }
                        });
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                                let _ = weak.update(cx, |this, cx| {
                                    this.pointer_down(event.position, window, cx)
                                });
                            }
                        });
                    },
                )
                .size_full(),
            )
            .when_some(more_button, |root, button| {
                root.child(div().absolute().right_2().bottom_2().child(button))
            })
            .when_some(overflow_list, |root, list| root.child(list))
            .into_any_element()
    }
}
