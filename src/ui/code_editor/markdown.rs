//! Zorite-derived editor 的工作台样式与原生消费者入口。文件生命周期保留在 buffers/saving。
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Context};
use lyrel_markdown_editor::{AlertIcons, EditorState, Labels, SyntaxStyle};
use crate::ui::i18n::{L10nKey, t};
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
    editor.set_labels(labels(), cx);
    cx.observe_global::<crate::core::config::Config>(|editor, cx| {
        editor.set_labels(labels(), cx);
    })
    .detach();
    writing::configure(editor, cx);
    mermaid_host::install(editor, cx);
    table_code::configure(editor, cx);
}

fn syntax_style(cx: &App) -> SyntaxStyle {
    let theme = cx.theme();
    let mut style = SyntaxStyle::default();
    style.marker = theme.muted_foreground;
    style.code = theme.foreground;
    style.code_bg = theme.muted;
    style.link = theme.link;
    style.tag = theme.link;
    style.quote = theme.muted_foreground;
    style.alert_note = theme.info;
    style.alert_tip = theme.success;
    style.alert_important = theme.primary;
    style.alert_warning = theme.warning;
    style.alert_caution = theme.danger;
    let mut icons = AlertIcons::default();
    icons.note = "stock/icons/info.svg".into();
    icons.tip = "stock/icons/lightbulb.svg".into();
    icons.important = "stock/icons/circle-alert.svg".into();
    icons.warning = "stock/icons/triangle-alert.svg".into();
    icons.caution = "stock/icons/octagon-alert.svg".into();
    style.alert_icons = Some(icons);
    style.rule = theme.border;
    style.mark_bg = theme.warning.opacity(0.2);
    style.popover_bg = theme.popover;
    style.popover_border = theme.border;
    style.popover_fg = theme.popover_foreground;
    style.popover_hover = theme.list_hover;
    style.popover_divider = theme.border;
    style.popover_danger = theme.danger;
    style.mono = gpui_kit::Font {
        family: theme.mono_font_family.clone(),
        ..Default::default()
    };
    style
}

fn labels() -> Labels {
    let mut labels = Labels::default();
    labels.cut = t(L10nKey::AppMenuCut).into();
    labels.copy = t(L10nKey::AppMenuCopy).into();
    labels.copy_as_markdown = t(L10nKey::MarkdownCopyAsMarkdown).into();
    labels.paste = t(L10nKey::AppMenuPaste).into();
    labels.code_copy = t(L10nKey::MarkdownCodeCopy).into();
    labels.math_copy = t(L10nKey::EditorFormulaCopyLatex).into();
    labels.turn_into = t(L10nKey::MarkdownTurnInto).into();
    labels.text = t(L10nKey::MarkdownBlockParagraph).into();
    labels.heading_1 = t(L10nKey::MarkdownBlockHeading1).into();
    labels.heading_2 = t(L10nKey::MarkdownBlockHeading2).into();
    labels.heading_3 = t(L10nKey::MarkdownBlockHeading3).into();
    labels.bulleted_list = t(L10nKey::MarkdownBlockBullet).into();
    labels.numbered_list = t(L10nKey::MarkdownBlockNumbered).into();
    labels.todo = t(L10nKey::MarkdownBlockTask).into();
    labels.quote = t(L10nKey::MarkdownBlockQuote).into();
    labels.callout = t(L10nKey::MarkdownBlockCallout).into();
    labels.code_block = t(L10nKey::MarkdownBlockCode).into();
    labels.math_block = t(L10nKey::MarkdownBlockMath).into();
    labels.insert_row_above = t(L10nKey::EditorTableInsertRowBefore).into();
    labels.insert_row_below = t(L10nKey::EditorTableInsertRowAfter).into();
    labels.duplicate_row = t(L10nKey::EditorTableDuplicateRow).into();
    labels.insert_column_left = t(L10nKey::EditorTableInsertColumnBefore).into();
    labels.insert_column_right = t(L10nKey::EditorTableInsertColumnAfter).into();
    labels.align_left = t(L10nKey::EditorTableAlignLeft).into();
    labels.align_center = t(L10nKey::EditorTableAlignCenter).into();
    labels.align_right = t(L10nKey::EditorTableAlignRight).into();
    labels.grid_style = t(L10nKey::EditorTableStyleGrid).into();
    labels.striped_style = t(L10nKey::EditorTableStyleStriped).into();
    labels.header_style = t(L10nKey::EditorTableStyleHeader).into();
    labels.minimal_style = t(L10nKey::EditorTableStyleMinimal).into();
    labels.delete_row = t(L10nKey::EditorTableDeleteRow).into();
    labels.delete_column = t(L10nKey::EditorTableDeleteColumn).into();
    labels.delete_table = t(L10nKey::EditorTableDeleteTable).into();
    labels.edit_properties = t(L10nKey::MarkdownEditProperties).into();
    labels.delete_property = t(L10nKey::MarkdownDeleteProperty).into();
    labels.delete_image = t(L10nKey::MarkdownDeleteImage).into();
    labels
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
