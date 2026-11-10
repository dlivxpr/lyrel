//! 写作 Host：仅管理 transient 菜单/颜色草稿，提交仍走同一 Document。
use crate::ui::i18n::{L10nKey, t};
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    color_picker::{ColorPickerState, ColorSelect},
    dialog::{DialogClose, DialogFooter},
    menu::{PopupMenu, PopupMenuItem},
};
use gpui_kit::component::{input::Input, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyrel_markdown_editor::{ColorKind, EditorState, WritingIntent, WritingRequest};

struct ColorDraft {
    picker: Entity<ColorPickerState>,
    title: SharedString,
    _observe: Subscription,
}
impl Render for ColorDraft {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .child(ColorSelect::new(&self.picker).accessibility_label(self.title.clone()))
            .when(!self.picker.read(cx).is_open(), |view| {
                view.child(
                    div()
                        .debug_selector(|| "writing-color-hex".into())
                        .child(Input::new(self.picker.read(cx).hex_input())),
                )
            })
    }
}

pub(super) fn configure(editor: &mut EditorState, cx: &mut Context<EditorState>) {
    let owner = cx.weak_entity();
    editor.set_writing_provider(move |request, window, cx| {
        let owner = owner.upgrade()?;
        if owner.read(cx).document().read(cx).revision() != request.revision() {
            return None;
        }
        match request.intent() {
            WritingIntent::Insert => Some(
                cx.new(|cx| WritingPopup::new(&owner, request, window, cx))
                    .into(),
            ),
            WritingIntent::TextColor | WritingIntent::HighlightColor => {
                open_color(&owner, request, window, cx);
                None
            }
        }
    });
}

struct WritingPopup {
    menu: Entity<PopupMenu>,
    position: Point<Pixels>,
    _dismiss: Subscription,
}
impl WritingPopup {
    fn new(
        editor: &Entity<EditorState>,
        request: WritingRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = editor.read(cx).focus_handle(cx);
        let menu = PopupMenu::build(window, cx, |mut menu, _, _| {
            menu = menu.action_context(focus).scrollable(true);
            for &(label, snippet, caret) in INSERTIONS {
                let editor = editor.downgrade();
                let request = request.clone();
                menu = menu.item(PopupMenuItem::new(t(label)).on_click(move |_, window, cx| {
                    let _ = editor.update(cx, |editor, cx| {
                        if editor.document().read(cx).revision() != request.revision() {
                            return;
                        }
                        let range = request.range();
                        let before = editor.selected_range();
                        let leading =
                            range.start > 0 && !editor.text()[..range.start].ends_with('\n');
                        let trailing = range.end < editor.text().len()
                            && !editor.text()[range.end..].starts_with('\n');
                        let inline = matches!(
                            label,
                            L10nKey::MarkdownSlashLink | L10nKey::MarkdownSlashInlineMath
                        );
                        let prefix = if leading && !inline { "\n" } else { "" };
                        let suffix = if trailing && !inline { "\n" } else { "" };
                        let inserted = format!("{prefix}{snippet}{suffix}");
                        editor.replace_range(range.clone(), &inserted, cx);
                        let at = range.start + prefix.len() + caret;
                        editor.select_range(at..at, cx);
                        editor.set_history_selection(before, at..at, cx);
                        editor.focus_handle(cx).focus(window, cx);
                        if label == L10nKey::MarkdownBlockMath {
                            editor.edit_math_at_caret(cx);
                        }
                    });
                }));
            }
            let table_editor = editor.downgrade();
            let table_request = request.clone();
            menu = menu.item(PopupMenuItem::new(t(L10nKey::EditorTableInsert)).on_click(
                move |_, window, cx| {
                    let Some(editor) = table_editor.upgrade() else {
                        return;
                    };
                    if editor.read(cx).document().read(cx).revision() != table_request.revision() {
                        return;
                    }
                    editor.update(cx, |editor, cx| {
                        editor.select_range(table_request.range(), cx)
                    });
                    super::table_code::open_table_picker(&editor, window, cx);
                },
            ));
            let image_editor = editor.downgrade();
            let image_request = request.clone();
            menu.item(PopupMenuItem::new(t(L10nKey::MarkdownSlashImage)).on_click(
                move |_, window, cx| {
                    let Some(editor) = image_editor.upgrade() else {
                        return;
                    };
                    if editor.read(cx).document().read(cx).revision() != image_request.revision() {
                        return;
                    }
                    editor.update(cx, |editor, cx| {
                        editor.select_range(image_request.range(), cx);
                        editor.focus_handle(cx).focus(window, cx);
                    });
                    window.dispatch_action(Box::new(lyrel_markdown_editor::InsertImage), cx);
                },
            ))
        });
        let owner = editor.downgrade();
        let dismiss = cx.subscribe(&menu, move |_, _, _: &DismissEvent, cx| {
            let owner = owner.clone();
            cx.defer(move |cx| {
                let _ = owner.update(cx, |view, cx| view.clear_writing_overlay(cx));
            });
        });
        menu.read(cx).focus_handle(cx).focus(window, cx);
        Self {
            menu,
            position: request.position(),
            _dismiss: dismiss,
        }
    }
}
impl Render for WritingPopup {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        deferred(
            anchored()
                .position(self.position)
                .snap_to_window_with_margin(window.rem_size() * 0.5)
                .child(self.menu.clone()),
        )
        .with_priority(2)
    }
}

const INSERTIONS: &[(L10nKey, &str, usize)] = &[
    (L10nKey::MarkdownBlockParagraph, "", 0),
    (L10nKey::MarkdownBlockHeading1, "# ", 2),
    (L10nKey::MarkdownBlockHeading2, "## ", 3),
    (L10nKey::MarkdownBlockHeading3, "### ", 4),
    (L10nKey::MarkdownBlockHeading4, "#### ", 5),
    (L10nKey::MarkdownBlockHeading5, "##### ", 6),
    (L10nKey::MarkdownBlockHeading6, "###### ", 7),
    (L10nKey::MarkdownBlockBullet, "- ", 2),
    (L10nKey::MarkdownBlockNumbered, "1. ", 3),
    (L10nKey::MarkdownBlockTask, "- [ ] ", 6),
    (L10nKey::MarkdownBlockQuote, "> ", 2),
    (L10nKey::MarkdownBlockNote, "> [!NOTE]\n> ", 12),
    (L10nKey::MarkdownBlockTip, "> [!TIP]\n> ", 11),
    (L10nKey::MarkdownBlockImportant, "> [!IMPORTANT]\n> ", 17),
    (L10nKey::MarkdownBlockWarning, "> [!WARNING]\n> ", 15),
    (L10nKey::MarkdownBlockCaution, "> [!CAUTION]\n> ", 15),
    (L10nKey::MarkdownBlockCode, "```\n\n```", 4),
    (L10nKey::MarkdownBlockMath, "$$\n\n$$", 3),
    (L10nKey::MarkdownSlashDivider, "---\n", 4),
    (L10nKey::MarkdownSlashLink, "[](https://)", 1),
    (L10nKey::MarkdownSlashInlineMath, "$$", 1),
];

fn open_color(
    editor: &Entity<EditorState>,
    request: WritingRequest,
    window: &mut Window,
    cx: &mut App,
) {
    let highlight = request.intent() == WritingIntent::HighlightColor;
    let kind = if highlight {
        ColorKind::Highlight
    } else {
        ColorKind::Text
    };
    let title = t(if highlight {
        L10nKey::MarkdownFormatHighlightColor
    } else {
        L10nKey::MarkdownFormatTextColor
    });
    let initial = editor
        .read(cx)
        .selection_color(kind)
        .map(|color| Hsla::from(rgba(color)))
        .unwrap_or_else(|| {
            if highlight {
                cx.theme().warning.opacity(0.5)
            } else {
                cx.theme().foreground
            }
        });
    let picker = cx.new(|cx| ColorPickerState::new(window, cx).default_value(initial));
    picker.update(cx, |picker, cx| picker.sync_pending_value(window, cx));
    let draft = cx.new(|cx| ColorDraft {
        picker: picker.clone(),
        title: title.into(),
        _observe: cx.observe(&picker, |_, _, cx| cx.notify()),
    });
    let target = editor.downgrade();
    let return_focus = editor.read(cx).focus_handle(cx);
    window.open_dialog(cx, move |dialog, _, _| {
        let commit_picker = picker.clone();
        let target = target.clone();
        let request = request.clone();
        let clear_picker = picker.clone();
        let focus = return_focus.clone();
        dialog
            .title(title)
            .child(draft.clone())
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("writing-color-clear")
                            .debug_selector(|| "writing-color-clear".into())
                            .ghost()
                            .label(t(if highlight {
                                L10nKey::MarkdownClearHighlightColor
                            } else {
                                L10nKey::MarkdownClearTextColor
                            }))
                            .on_click(move |_, window, cx| {
                                clear_picker
                                    .update(cx, |picker, cx| picker.clear_value(window, cx));
                            }),
                    )
                    .child(DialogClose::new().trigger(|button| button.label(t(L10nKey::Cancel))))
                    .child(
                        Button::new("writing-color-apply")
                            .debug_selector(|| "writing-color-apply".into())
                            .primary()
                            .label(t(L10nKey::MarkdownApplyColor))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(
                                    Box::new(gpui_kit::component::dialog::Confirm {
                                        secondary: false,
                                    }),
                                    cx,
                                );
                            }),
                    ),
            )
            .on_ok(move |_, window, cx| {
                let input = commit_picker.read(cx).hex_input().clone();
                let hex = input.read(cx).value();
                let color = if hex.is_empty() && commit_picker.read(cx).value().is_none() {
                    None
                } else {
                    let Some(color) = commit_picker
                        .update(cx, |picker, cx| picker.commit_hex(hex.as_str(), window, cx))
                    else {
                        return false;
                    };
                    Some(pack_rgba(color))
                };
                let _ = target.update(cx, |editor, cx| {
                    if editor.document().read(cx).revision() != request.revision() {
                        return;
                    }
                    editor.select_range(request.range(), cx);
                    editor.color_selection(kind, color, cx);
                    editor.focus_handle(cx).focus(window, cx);
                });
                true
            })
            .on_close(move |_, window, cx| focus.focus(window, cx))
    });
}
fn pack_rgba(color: Hsla) -> u32 {
    let rgba: Rgba = color.into();
    let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u32;
    (channel(rgba.r) << 24) | (channel(rgba.g) << 16) | (channel(rgba.b) << 8) | channel(rgba.a)
}
