use super::{Inbox, Record, Severity, Source, ensure, history, navigate, source_exists};
use crate::ui::i18n::{L10nKey, t};
use gpui_kit::base::SelectableText;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    popover::Popover,
    scroll::Scrollbar,
    v_flex,
};
use gpui_kit::{
    Anchor, AnyWindowHandle, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter,
    FocusHandle, IntoElement, ParentElement as _, Render, ScrollHandle, ScrollStrategy,
    Styled as _, Subscription, TestSupportExt as _, UniformListScrollHandle, WeakEntity, Window,
    div, prelude::*, rems, uniform_list,
};

pub(crate) struct NotificationBell {
    view: Entity<InboxView>,
    inbox: Entity<Inbox>,
    open: bool,
    _watch: Subscription,
    _navigation: Subscription,
}

impl NotificationBell {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        ensure(cx);
        let inbox = history(cx).clone();
        let watch = cx.observe(&inbox, |_, _, cx| cx.notify());
        let view = cx.new(InboxView::new);
        let navigation = cx.subscribe(&view, |this, _, event: &SourceNavigation, cx| {
            this.open = false;
            cx.notify();
            let source = event.source.clone();
            let handle = event.window;
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| {
                    // Let Popover return focus before moving it to the source.
                    window.on_next_frame(move |window, cx| {
                        navigate(&source, window, cx);
                    });
                });
            });
        });
        Self {
            view,
            inbox,
            open: false,
            _watch: watch,
            _navigation: navigation,
        }
    }
}

impl Render for NotificationBell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let unread = self.inbox.read(cx).unread_count();
        Popover::new("notification-inbox-popover")
            .anchor(Anchor::BottomRight)
            .open(self.open)
            .on_open_change(cx.listener(|this, open, _, cx| {
                this.open = *open;
                if *open {
                    this.view.update(cx, |view, cx| {
                        view.detail = None;
                        cx.notify();
                    });
                    this.inbox.update(cx, |inbox, cx| {
                        inbox.mark_read();
                        cx.notify();
                    });
                }
                cx.notify();
            }))
            .trigger(
                Button::new("status-notifications")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Bell)
                    .when(unread > 0, |button| button.label(unread.to_string()))
                    .accessibility_label(t(L10nKey::NotificationInbox))
                    .tooltip(t(L10nKey::NotificationInbox)),
            )
            .child(self.view.clone())
    }
}

struct SourceNavigation {
    source: Source,
    window: AnyWindowHandle,
}
struct InboxView {
    inbox: Entity<Inbox>,
    scroll: UniformListScrollHandle,
    detail_scroll: ScrollHandle,
    detail: Option<Record>,
    focus: FocusHandle,
    keyboard_row: usize,
    _watch: Subscription,
    _machine_watch: Subscription,
    _windows_watch: Subscription,
}

impl InboxView {
    fn new(cx: &mut Context<Self>) -> Self {
        let inbox = history(cx).clone();
        let watch = cx.observe(&inbox, |this, inbox, cx| {
            if this.detail.as_ref().is_some_and(|detail| {
                !inbox
                    .read(cx)
                    .records()
                    .any(|record| record.id() == detail.id())
            }) {
                this.detail = None;
            }
            cx.notify();
        });
        let machine_watch =
            cx.observe_global::<crate::ui::machine_mirror::MachineMirrors>(|_, cx| cx.notify());
        let windows_watch =
            cx.observe_global::<crate::ui::windows::WindowRegistry>(|_, cx| cx.notify());
        Self {
            inbox,
            scroll: UniformListScrollHandle::new(),
            detail_scroll: ScrollHandle::new(),
            detail: None,
            focus: cx.focus_handle().tab_stop(true),
            keyboard_row: 0,
            _watch: watch,
            _machine_watch: machine_watch,
            _windows_watch: windows_watch,
        }
    }

    fn render_detail(&self, window: &Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let record = self.detail.as_ref().expect("detail is open");
        let message: gpui_kit::SharedString = record.message().to_owned().into();
        let copy = message.clone();
        let width =
            (window.viewport_size().width - window.rem_size() * 2.).min(window.rem_size() * 38.);
        let height = (window.viewport_size().height * 0.65).min(window.rem_size() * 28.);
        v_flex()
            .id("notification-detail")
            .test_support()
            .w(width)
            .h(height)
            .min_h_0()
            .gap_2()
            .child(
                h_flex()
                    .flex_none()
                    .justify_between()
                    .gap_2()
                    .child(
                        Button::new("notification-detail-back")
                            .ghost()
                            .small()
                            .label(t(L10nKey::NotificationBack))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.detail = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("notification-detail-copy")
                            .ghost()
                            .small()
                            .label(t(L10nKey::NotificationCopy))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.to_string()));
                            }),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(
                        div()
                            .id("notification-detail-scroll")
                            .test_support()
                            .track_focus(&self.focus)
                            .on_key_down(cx.listener(
                                |this, event: &gpui_kit::KeyDownEvent, window, cx| {
                                    let scroll = &this.detail_scroll;
                                    let mut offset = scroll.offset();
                                    let page = scroll.bounds().size.height;
                                    offset.y = match event.keystroke.key.as_str() {
                                        "down" => offset.y - window.rem_size() * 1.5,
                                        "up" => offset.y + window.rem_size() * 1.5,
                                        "pagedown" => offset.y - page,
                                        "pageup" => offset.y + page,
                                        "home" => gpui_kit::Pixels::ZERO,
                                        "end" => -scroll.max_offset().y,
                                        _ => return,
                                    }
                                    .clamp(-scroll.max_offset().y, gpui_kit::Pixels::ZERO);
                                    scroll.set_offset(offset);
                                    cx.stop_propagation();
                                    cx.notify();
                                },
                            ))
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.detail_scroll)
                            .child(
                                div()
                                    .id("notification-detail-message")
                                    .test_support()
                                    .w_full()
                                    .min_w_0()
                                    .pr_3()
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child(SelectableText::new(record.id(), message)),
                            ),
                    )
                    .child(Scrollbar::vertical(&self.detail_scroll).viewport_from_layout()),
            )
            .into_any_element()
    }
}

impl EventEmitter<SourceNavigation> for InboxView {}

impl Render for InboxView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.detail.is_some() {
            return self.render_detail(window, cx);
        }
        let count = self.inbox.read(cx).records().len();
        let inbox = self.inbox.clone();
        let view = cx.weak_entity();
        // The native popover owns dismissal/focus return. Only this list owns
        // scrolling; row identity is the persisted UUID, never the list index.
        let width =
            (window.viewport_size().width - window.rem_size() * 2.).min(window.rem_size() * 30.);
        let height = (window.viewport_size().height * 0.65).min(window.rem_size() * 28.);
        v_flex()
            .id("notification-inbox")
            .test_support()
            .w(width)
            .max_h(height)
            .min_h_0()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .justify_between()
                    .child(div().text_sm().child(t(L10nKey::NotificationInbox)))
                    .child(
                        Button::new("notification-clear-history")
                            .ghost()
                            .small()
                            .label(t(L10nKey::NotificationClearHistory))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.inbox.update(cx, |inbox, cx| {
                                    inbox.clear();
                                    cx.notify();
                                });
                            })),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(L10nKey::NotificationScope)),
            )
            .when(count == 0, |column| {
                column.child(
                    div()
                        .id("notification-inbox-empty")
                        .test_support()
                        .py_6()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t(L10nKey::NotificationEmpty)),
                )
            })
            .when(count > 0, |column| {
                column.child(
                    div()
                        .id("notification-scroll-region")
                        .test_support()
                        .track_focus(&self.focus)
                        .relative()
                        .h((height - window.rem_size() * 8.).max(window.rem_size() * 5.))
                        .min_h_0()
                        .w_full()
                        .on_key_down(cx.listener(
                            move |this, event: &gpui_kit::KeyDownEvent, _, cx| {
                                let last = count.saturating_sub(1);
                                this.keyboard_row = match event.keystroke.key.as_str() {
                                    "down" => (this.keyboard_row + 1).min(last),
                                    "up" => this.keyboard_row.saturating_sub(1),
                                    "pagedown" => (this.keyboard_row + 4).min(last),
                                    "pageup" => this.keyboard_row.saturating_sub(4),
                                    "home" => 0,
                                    "end" => last,
                                    _ => return,
                                };
                                this.scroll
                                    .scroll_to_item(this.keyboard_row, ScrollStrategy::Top);
                                cx.stop_propagation();
                                cx.notify();
                            },
                        ))
                        .child(
                            uniform_list(
                                "notification-records",
                                count,
                                move |range, window, cx| {
                                    range
                                        .map(|index| {
                                            let record =
                                                &inbox.read(cx).snapshot.records[count - index - 1];
                                            render_record(record, view.clone(), window, cx)
                                        })
                                        .collect()
                                },
                            )
                            .track_scroll(&self.scroll)
                            .size_full(),
                        )
                        .child(Scrollbar::vertical(&self.scroll).viewport_from_layout()),
                )
            })
            .into_any_element()
    }
}

fn render_record(
    record: &Record,
    view: WeakEntity<InboxView>,
    window: &Window,
    cx: &App,
) -> impl IntoElement + use<> {
    let severity = record.severity();
    let color = match severity {
        Severity::Success => cx.theme().success,
        Severity::Info => cx.theme().info,
        Severity::Warning => cx.theme().warning,
        Severity::Error => cx.theme().danger,
    };
    let timestamp = chrono::DateTime::from_timestamp(record.timestamp(), 0)
        .map(|timestamp| {
            timestamp
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| record.timestamp().to_string());
    let source = record.source().cloned();
    let available = source
        .as_ref()
        .is_some_and(|source| source_exists(source, window, cx));
    let detail_id = record.id();
    let detail_view = view.clone();
    let copy = record.message().to_owned();
    let preview = record
        .message()
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    v_flex()
        .id(record.id())
        .h(rems(6.))
        .w_full()
        .min_w_0()
        .px_1()
        .py_2()
        .gap_1()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            h_flex()
                .flex_none()
                .gap_2()
                .text_xs()
                .child(div().text_color(color).child(severity.label()))
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .child(timestamp),
                )
                .when(record.is_unread(), |row| {
                    row.child(div().child(t(L10nKey::NotificationUnread)))
                }),
        )
        .child(div().min_w_0().text_sm().text_ellipsis().child(preview))
        .child(
            h_flex()
                .flex_none()
                .gap_1()
                .min_w_0()
                .child(
                    Button::new(format!("notification-detail-{}", record.id()))
                        .ghost()
                        .xsmall()
                        .label(t(L10nKey::NotificationDetails))
                        .on_click(move |_, window, cx| {
                            let _ = detail_view.update(cx, |this, cx| {
                                this.detail = this
                                    .inbox
                                    .read(cx)
                                    .records()
                                    .find(|record| record.id() == detail_id)
                                    .cloned();
                                this.detail_scroll.set_offset(gpui_kit::Point::default());
                                window.focus(&this.focus, cx);
                                cx.notify();
                            });
                        }),
                )
                .child(
                    Button::new(format!("notification-copy-{}", record.id()))
                        .ghost()
                        .xsmall()
                        .label(t(L10nKey::NotificationCopy))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                        }),
                )
                .when_some(source, |row, source| {
                    let label = if available {
                        source.label.clone()
                    } else {
                        format!(
                            "{} · {}",
                            source.label,
                            t(L10nKey::NotificationSourceUnavailable)
                        )
                    };
                    row.child(
                        Button::new(format!("notification-source-{}", record.id()))
                            .ghost()
                            .xsmall()
                            .flex_1()
                            .min_w_0()
                            .max_w_full()
                            .label(label.clone())
                            .tooltip(label)
                            .disabled(!available)
                            .on_click(move |_, window, cx| {
                                let _ = view.update(cx, |_, cx| {
                                    cx.emit(SourceNavigation {
                                        source: source.clone(),
                                        window: window.window_handle(),
                                    })
                                });
                            }),
                    )
                }),
        )
}
