//! Presentation of daemon-authored program records; no lifecycle or agent inference.
use gpui_kit::component::{ActiveTheme as _, tooltip::Tooltip};
use gpui_kit::{AnyElement, App, SharedString, TestSupportExt as _, div, prelude::*, rems};
use lyrel_core::core::program_status::{
    BlockedKind, ProgramRecord, ProgramState, plain_program_text,
};
use lyrel_ui_base::i18n::{L10nKey, t};
use std::fmt::Write as _;

pub(crate) fn state_label(record: &ProgramRecord) -> &'static str {
    t(match record.state() {
        ProgramState::Idle => L10nKey::ProgramIdle,
        ProgramState::Working => L10nKey::ProgramWorking,
        ProgramState::Done => L10nKey::ProgramDone,
        ProgramState::Error => L10nKey::ProgramError,
        ProgramState::Blocked => match record.kind() {
            Some(BlockedKind::Permission) => L10nKey::ProgramPermission,
            Some(BlockedKind::Question) => L10nKey::ProgramQuestion,
            Some(BlockedKind::Auth) => L10nKey::ProgramAuth,
            None => L10nKey::ProgramBlocked,
        },
    })
}

pub(crate) fn record_label(record: &ProgramRecord) -> String {
    let mut label = String::from(t(L10nKey::ProgramSource));
    if let Some(app) = record.app() {
        let _ = write!(label, " · {}", plain_program_text(app));
    }
    let _ = write!(label, " · {}", state_label(record));
    if let Some(progress) = record.progress() {
        let _ = write!(label, " · {progress}%");
    }
    if let Some(title) = record.title().filter(|title| !title.is_empty()) {
        let _ = write!(label, " · {}", plain_program_text(title));
    }
    if let Some(message) = record.message().filter(|message| !message.is_empty()) {
        let _ = write!(label, " · {}", plain_program_text(message));
    }
    label
}

pub(crate) fn current_status(record: &ProgramRecord, cx: &App) -> AnyElement {
    let label: SharedString = record_label(record).into();
    let color = match record.state() {
        ProgramState::Blocked => cx.theme().warning,
        ProgramState::Error => cx.theme().danger,
        _ => cx.theme().muted_foreground,
    };
    div()
        .id("status-program")
        .test_support()
        .role(gpui_kit::Role::Status)
        .aria_label(label.clone())
        .flex_shrink_1()
        .min_w_0()
        .max_w(rems(24.))
        .text_color(color)
        .text_ellipsis()
        .child(label.clone())
        .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
        .into_any_element()
}
