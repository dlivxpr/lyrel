//! 结构树局部编辑的共享输入面板。
//!
//! 一个面板只承载一次明确的树命令：可选一个键字段、一个值字段、错误提示
//! 与“应用／取消”。格式自己的树视图创建面板、订阅 [`StructureEditEvent`]
//! 并在事件里规划源区间修改；面板不持有文档，也不写任何内容。后续
//! YAML／TOML／XML 结构编辑复用同一控件，避免每个格式复制输入框、焦点与
//! 错误呈现。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Entity, EntityId, EventEmitter, Focusable as _, MouseButton,
    SharedString, Subscription, TestSupportExt as _, Window, div, rems,
};

use crate::ui::i18n::{L10nKey, t};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StructureEditEvent {
    Apply,
    Cancel,
}

/// 一个输入字段的标签、占位符与初始文本。标签由格式自己决定（“键”“值”
/// 或其它），面板只负责呈现。
#[derive(Clone, Debug)]
pub(crate) struct EditField {
    pub(crate) label: SharedString,
    pub(crate) placeholder: SharedString,
    pub(crate) initial: String,
}

impl EditField {
    pub(crate) fn new(
        label: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
        initial: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            placeholder: placeholder.into(),
            initial: initial.into(),
        }
    }
}

pub(crate) struct StructureEditPanel {
    owner: EntityId,
    title: SharedString,
    key: Option<Entity<InputState>>,
    key_label: Option<SharedString>,
    value: Entity<InputState>,
    multi_value: Option<Entity<TextareaState>>,
    value_label: SharedString,
    error: Option<SharedString>,
    notice: Option<SharedString>,
    _key: Option<Subscription>,
    _value: Subscription,
}

impl StructureEditPanel {
    pub(crate) fn new(
        owner: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
        title: SharedString,
        key: Option<EditField>,
        value: EditField,
        multiline_value: bool,
    ) -> Self {
        let value_label = value.label.clone();
        let initial = value.initial.clone();
        let value = new_input(window, cx, &value);
        let multi_value = multiline_value.then(|| {
            cx.new(|cx| {
                let mut state = TextareaState::new(window, cx);
                state.set_value(initial, window, cx);
                state
            })
        });
        let key_label = key.as_ref().map(|field| field.label.clone());
        let key = key.map(|field| new_input(window, cx, &field));
        let value_sub = cx.subscribe_in(&value, window, |_, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                cx.emit(StructureEditEvent::Apply);
            }
        });
        let key_sub = key.as_ref().map(|key| {
            cx.subscribe_in(key, window, |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    cx.emit(StructureEditEvent::Apply);
                }
            })
        });
        Self {
            owner,
            title,
            key,
            key_label,
            value,
            multi_value,
            value_label,
            error: None,
            notice: None,
            _key: key_sub,
            _value: value_sub,
        }
    }

    /// 焦点给第一个可编辑字段：有键字段时是键，否则是值。
    pub(crate) fn focus_first(&self, window: &mut Window, cx: &mut App) {
        if let Some(key) = &self.key {
            let handle = key.read(cx).focus_handle(cx).clone();
            window.focus(&handle, cx);
        } else if let Some(value) = &self.multi_value {
            let handle = value.read(cx).focus_handle(cx).clone();
            window.focus(&handle, cx);
        } else {
            let handle = self.value.read(cx).focus_handle(cx).clone();
            window.focus(&handle, cx);
        }
    }

    /// 测试消费者把键盘放进真实值控件（多行时为 `Textarea`）。
    #[cfg(test)]
    pub(crate) fn focus_value(&self, window: &mut Window, cx: &mut App) {
        let handle = match &self.multi_value {
            Some(value) => value.read(cx).focus_handle(cx).clone(),
            None => self.value.read(cx).focus_handle(cx).clone(),
        };
        window.focus(&handle, cx);
    }

    pub(crate) fn set_error(&mut self, error: Option<SharedString>, cx: &mut Context<Self>) {
        if self.error == error {
            return;
        }
        self.error = error;
        cx.notify();
    }

    /// 提交前即可见的影响说明（例如 YAML anchor 的别名引用范围）。它不是
    /// 错误，不阻止应用；格式自己决定文字与出现时机。
    pub(crate) fn set_notice(&mut self, notice: Option<SharedString>, cx: &mut Context<Self>) {
        if self.notice == notice {
            return;
        }
        self.notice = notice;
        cx.notify();
    }

    pub(crate) fn key_text(&self, cx: &App) -> Option<String> {
        self.key
            .as_ref()
            .map(|key| key.read(cx).value().to_string())
    }

    pub(crate) fn value_text(&self, cx: &App) -> String {
        match &self.multi_value {
            Some(value) => value.read(cx).value().to_string(),
            None => self.value.read(cx).value().to_string(),
        }
    }
}

fn new_input(
    window: &mut Window,
    cx: &mut Context<StructureEditPanel>,
    field: &EditField,
) -> Entity<InputState> {
    let placeholder = field.placeholder.clone();
    let initial = field.initial.clone();
    cx.new(|cx| {
        let mut state = InputState::new(window, cx).placeholder(placeholder);
        state.set_value(initial, window, cx);
        state
    })
}

impl EventEmitter<StructureEditEvent> for StructureEditPanel {}

impl Render for StructureEditPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = self.owner;
        let muted = cx.theme().muted_foreground;
        let warning = cx.theme().warning;
        v_flex()
            .id(("structure-edit", owner))
            .test_support()
            .flex_none()
            .gap_1()
            .px_2()
            .py_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id(("structure-edit-title", owner))
                            .test_support()
                            .flex_none()
                            .text_xs()
                            .text_color(muted)
                            .child(self.title.clone()),
                    )
                    .when_some(self.error.clone(), |row, error| {
                        row.child(
                            div()
                                .id(("structure-edit-error", owner))
                                .test_support()
                                .aria_value(error.clone())
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .text_color(warning)
                                .child(error),
                        )
                    }),
            )
            .when_some(self.notice.clone(), |panel, notice| {
                panel.child(
                    div()
                        .id(("structure-edit-notice", owner))
                        .test_support()
                        .aria_value(notice.clone())
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().yellow)
                        .child(notice),
                )
            })
            .when_some(self.key.clone(), |panel, key| {
                panel.child(field(
                    owner,
                    "structure-edit-key-label",
                    self.key_label
                        .clone()
                        .unwrap_or_else(|| SharedString::from("")),
                    key,
                    ("structure-edit-key", owner),
                ))
            })
            .when_some(self.multi_value.clone(), |panel, value| {
                panel.child(multi_field(owner, self.value_label.clone(), value))
            })
            .when(self.multi_value.is_none(), |panel| {
                panel.child(field(
                    owner,
                    "structure-edit-value-label",
                    self.value_label.clone(),
                    self.value.clone(),
                    ("structure-edit-value", owner),
                ))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new(("structure-edit-cancel", owner))
                            .ghost()
                            .xsmall()
                            .label(t(L10nKey::Cancel))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(StructureEditEvent::Cancel);
                            })),
                    )
                    .child(
                        Button::new(("structure-edit-apply", owner))
                            .xsmall()
                            .label(t(L10nKey::StructureEditApply))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(StructureEditEvent::Apply);
                            })),
                    ),
            )
    }
}

fn multi_field(owner: EntityId, label: SharedString, state: Entity<TextareaState>) -> AnyElement {
    h_flex()
        .items_start()
        .gap_2()
        .child(
            div()
                .id(("structure-edit-value-label", owner))
                .flex_none()
                .w(rems(5.))
                .text_xs()
                .child(label),
        )
        .child(
            div()
                .id(("structure-edit-value-multiline", owner))
                .test_support()
                .flex_1()
                .min_w_0()
                .child(Textarea::new(&state).small().h(rems(10.))),
        )
        .into_any_element()
}

fn field(
    owner: EntityId,
    label_id: &'static str,
    label: SharedString,
    input: Entity<InputState>,
    input_id: (&'static str, EntityId),
) -> AnyElement {
    h_flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .id((label_id, owner))
                .flex_none()
                .w(rems(5.))
                .text_xs()
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(Input::new(&input).id(input_id).small()),
        )
        .into_any_element()
}
