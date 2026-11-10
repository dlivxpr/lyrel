//! Zorite-derived editor 的工作台样式与原生消费者入口。文件生命周期保留在 buffers/saving。
use gpui_kit::Context;
use gpui_kit::component::ActiveTheme as _;
use lyrel_markdown_editor::{EditorState, SyntaxStyle};
mod mermaid_host;

pub(super) fn configure(editor: &mut EditorState, cx: &mut Context<EditorState>) {
    let theme = cx.theme();
    let style = SyntaxStyle {
        marker: theme.muted_foreground,
        code: theme.foreground,
        code_bg: theme.muted,
        link: theme.link,
        tag: theme.link,
        quote: theme.muted_foreground,
        alert_note: theme.info,
        alert_tip: theme.success,
        alert_important: theme.primary,
        alert_warning: theme.warning,
        alert_caution: theme.danger,
        alert_icons: None,
        rule: theme.border,
        mark_bg: theme.warning.opacity(0.2),
        block_label: None,
        block_label_gen: 0,
        block_ref_count: None,
        popover_bg: theme.popover,
        popover_border: theme.border,
        popover_fg: theme.popover_foreground,
        popover_hover: theme.list_hover,
        popover_divider: theme.border,
        popover_danger: theme.danger,
        mono: gpui_kit::Font {
            family: theme.mono_font_family.clone(),
            ..Default::default()
        },
        property_icon: None,
    };
    editor.set_live_style(style.clone(), cx);
    editor.set_markdown_style(style, cx);
    mermaid_host::install(editor, cx);
}

#[cfg(any(test, feature = "native-markdown-tests"))]
mod foundation;
#[cfg(feature = "native-markdown-tests")]
pub(crate) fn native_cases() -> impl Iterator<Item = crate::native_markdown::Case> {
    foundation::native_cases().chain([
        crate::native_markdown::Case::new(
            "ui::code_editor::workbench_views::shared_context_and_ime_handoff",
            super::document_input::workbench_views::shared_context_and_ime_handoff,
        ),
        crate::native_markdown::Case::new(
            "ui::code_editor::workbench_views::focused_consumers_and_layout",
            super::document_input::workbench_views::focused_consumers_and_layout,
        ),
    ]).chain(mermaid_host::native_cases())
}
