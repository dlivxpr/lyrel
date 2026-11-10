//! Zorite-derived editor 的工作台样式与原生消费者入口。文件生命周期保留在 buffers/saving。
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Context};
use lyrel_markdown_editor::{AlertIcons, EditorState, SyntaxStyle};
pub(super) mod images;
mod links;
mod mermaid_host;
#[cfg(any(test, feature = "native-markdown-tests"))]
mod resources_tests;
pub(crate) mod table_code;
mod writing;

pub(super) fn configure(editor: &mut EditorState, cx: &mut Context<EditorState>) {
    // Reference document: 16px body, 1.45 line-height, H1/H2/H3 1.8/1.5/1.3.
    // Keep the native width fluid (no extra max-width); the shell owns zoom.
    editor.set_document_inset(1.75, 1.);
    editor.set_presentation_style_provider(syntax_style);
    let style = syntax_style(cx);
    editor.set_live_style(style.clone(), cx);
    editor.set_markdown_style(style, cx);
    writing::configure(editor, cx);
    mermaid_host::install(editor, cx);
    table_code::configure(editor, cx);
}

fn syntax_style(cx: &App) -> SyntaxStyle {
    let theme = cx.theme();
    SyntaxStyle {
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
        alert_icons: Some(AlertIcons {
            note: "stock/icons/info.svg".into(),
            tip: "stock/icons/lightbulb.svg".into(),
            important: "stock/icons/circle-alert.svg".into(),
            warning: "stock/icons/triangle-alert.svg".into(),
            caution: "stock/icons/octagon-alert.svg".into(),
        }),
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
    }
}

#[cfg(any(test, feature = "native-markdown-tests"))]
mod foundation;
#[cfg(any(test, feature = "native-markdown-tests"))]
mod table_code_tests;
#[cfg(any(test, feature = "native-markdown-tests"))]
mod writing_tests;
#[cfg(feature = "native-markdown-tests")]
pub(crate) fn native_cases() -> impl Iterator<Item = crate::native_markdown::Case> {
    foundation::native_cases()
        .chain(resources_tests::native_cases())
        .chain(writing_tests::native_cases())
        .chain(table_code_tests::native_cases())
        .chain([
            crate::native_markdown::Case::new(
                "ui::code_editor::workbench_views::shared_context_and_ime_handoff",
                super::document_input::workbench_views::shared_context_and_ime_handoff,
            ),
            crate::native_markdown::Case::new(
                "ui::code_editor::workbench_views::focused_consumers_and_layout",
                super::document_input::workbench_views::focused_consumers_and_layout,
            ),
        ])
        .chain(mermaid_host::native_cases())
        .chain(super::saving::native_cases())
        .chain(super::markdown_formula::native_cases())
}
