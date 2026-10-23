use super::{Inbox, Record, Severity, Source, ensure, history, navigate, source_exists};
use crate::ui::i18n::{L10nKey, t};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    popover::Popover,
    scroll::Scrollbar,
    v_flex,
};
use gpui_kit::{
    Anchor, AnyWindowHandle, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle,
    IntoElement, ParentElement as _, Render, ScrollStrategy, Styled as _, Subscription,
    TestSupportExt as _, UniformListScrollHandle, WeakEntity, Window, div, prelude::*, rems,
    uniform_list,
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
    focus: FocusHandle,
    keyboard_row: usize,
    _watch: Subscription,
    _machine_watch: Subscription,
    _windows_watch: Subscription,
}

impl InboxView {
    fn new(cx: &mut Context<Self>) -> Self {
        let inbox = history(cx).clone();
        let watch = cx.observe(&inbox, |_, _, cx| cx.notify());
        let machine_watch =
            cx.observe_global::<crate::ui::machine_mirror::MachineMirrors>(|_, cx| cx.notify());
        let windows_watch =
            cx.observe_global::<crate::ui::windows::WindowRegistry>(|_, cx| cx.notify());
        Self {
            inbox,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle().tab_stop(true),
            keyboard_row: 0,
            _watch: watch,
            _machine_watch: machine_watch,
            _windows_watch: windows_watch,
        }
    }
}

impl EventEmitter<SourceNavigation> for InboxView {}

impl Render for InboxView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                        .h((height - window.rem_size() * 5.).max(window.rem_size() * 5.))
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
        .child(
            div()
                .min_w_0()
                .text_sm()
                .text_ellipsis()
                .child(record.message().to_owned()),
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
        })
}
