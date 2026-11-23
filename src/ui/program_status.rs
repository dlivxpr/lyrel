//! Presentation of daemon-authored program records; no lifecycle or agent inference.
use gpui_kit::component::{ActiveTheme as _, h_flex, tooltip::Tooltip};
use gpui_kit::{AnyElement, App, SharedString, TestSupportExt as _, div, prelude::*, px, rems};
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

/// The global status bar's compact label. The generic `Program` prefix only
/// appears when the record carries no program name — a named program reads as
/// `name · state …`, an unnamed one as `Program · state …`.
pub(crate) fn status_label(record: &ProgramRecord) -> String {
    let mut label = String::new();
    if let Some(app) = record.app() {
        label.push_str(&plain_program_text(app));
    } else {
        label.push_str(t(L10nKey::ProgramSource));
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
    let label: SharedString = status_label(record).into();
    // The dot is the semantic channel; the text label carries the details.
    // Idle rests at the muted rung, Working draws the accent the caffeine cup
    // already uses for held activity, Done the success rung, and the two
    // attention states keep their warning/danger roles.
    let dot = match record.state() {
        ProgramState::Idle => cx.theme().muted_foreground,
        ProgramState::Working => cx.theme().accent,
        ProgramState::Done => cx.theme().success,
        ProgramState::Error => cx.theme().danger,
        ProgramState::Blocked => cx.theme().warning,
    };
    h_flex()
        .id("status-program")
        .test_support()
        .role(gpui_kit::Role::Status)
        .aria_label(label.clone())
        .flex_shrink_1()
        .min_w_0()
        .items_center()
        .gap(px(6.))
        .max_w(rems(24.))
        .child(
            div()
                .id("status-program-dot")
                .test_support()
                .flex_none()
                .size(px(6.))
                .rounded_full()
                .bg(dot),
        )
        .child(div().min_w_0().truncate().text_ellipsis().child(label.clone()))
        .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
        .into_any_element()
}
