//! Document-only overlay rendering owned by the editor entity: the empty
//! panel and the on-disk conflict banner. Both read and mutate nothing
//! outside `EditorPanelState` — the banner's answers are the entity's own
//! reload/drop/save methods — so the shell wraps them in a single
//! `editor.update` and supplies only the save root.

use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, MouseButton, div, px, rems};
use std::path::PathBuf;

use super::{BufferId, DiskConflict, EditorPanelState};
use lyrel_ui_base::dialog::{self, Tone};
use lyrel_ui_base::i18n::{L10nKey, t};

impl EditorPanelState {
    /// The panel with no file in it: a file glyph and where a file comes
    /// from. A press keeps the keyboard in the panel so the next ⌘W closes
    /// the panel, not whatever held the focus before.
    pub fn render_empty(&mut self, cx: &mut Context<Self>) -> gpui_kit::Div {
        v_flex()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.empty_focus.focus(window, cx)),
            )
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                Icon::new(IconName::File)
                    .large()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(L10nKey::OpenFileFromTree)),
            )
    }

    /// `text` is the document's save capability: a read-only picture can be
    /// reloaded or closed, but offering "Save" or "Keep mine" would promise a
    /// write that has no bytes to write. A diagram does have them. `root` is
    /// the shell-resolved save root for the Save answer.
    pub fn render_conflict_banner(
        &mut self,
        id: BufferId,
        conflict: DiskConflict,
        text: bool,
        root: PathBuf,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let rungs = cx.global::<lyrel_ui_base::presets::Surfaces>().window;
        // The floating notices' grammar, laid flat: a neutral strip over a
        // hairline, and the state carried by one amber dot. A strip tinted
        // amber end to end, with a white outlined button from the component
        // library beside a bare-text one, was three visual languages in one
        // 32px row.
        //
        // The filled answer is always the safe one: Keep mine / Save hold on
        // to the unsaved edits, Reload / Close throw them away, so those are
        // offered, not pressed on the reader.
        let row = h_flex()
            .flex_none()
            .w_full()
            .items_center()
            .gap(px(8.))
            .pl(px(dialog::INSET))
            .pr(px(6.))
            .h(px(dialog::FOOTER_H))
            .border_b_1()
            .border_color(theme.border)
            .text_size(rems(lyrel_ui_base::display::TAB_TEXT))
            .child(
                div()
                    .flex_none()
                    .size(px(6.))
                    .rounded_full()
                    .bg(theme.warning),
            );
        let message = |key| div().flex_1().min_w_0().truncate().child(t(key));
        match conflict {
            DiskConflict::Changed(observed) => row
                .child(message(L10nKey::FileChangedOnDisk))
                .child(dialog::button(
                    "editor-conflict-reload",
                    t(L10nKey::Reload),
                    Tone::Secondary,
                    true,
                    rungs,
                    cx,
                    cx.listener(move |this, _, window, cx| {
                        this.reload_from_disk(id, window, cx);
                    }),
                ))
                .when(text, |row| {
                    row.child(dialog::button(
                        "editor-conflict-keep",
                        t(L10nKey::KeepMine),
                        Tone::Primary,
                        true,
                        rungs,
                        cx,
                        cx.listener(move |this, _, _w, cx| {
                            if let Some(f) = this.buffers.get_mut(id) {
                                // The version seen on disk is now the one a
                                // save is allowed to replace.
                                f.disk_mtime = observed;
                                f.conflict = None;
                                cx.notify();
                            }
                        }),
                    ))
                })
                .into_any_element(),
            DiskConflict::Deleted => row
                .child(message(L10nKey::EditorFileDeletedOnDisk))
                .child(dialog::button(
                    "editor-deleted-close",
                    t(L10nKey::Close),
                    Tone::Secondary,
                    true,
                    rungs,
                    cx,
                    cx.listener(move |_, _, _, cx| {
                        cx.emit(super::EditorPanelEvent::BuffersDiscarded { ids: vec![id] });
                    }),
                ))
                .when(text, |row| {
                    row.child(dialog::button(
                        "editor-deleted-save",
                        t(L10nKey::Save),
                        Tone::Primary,
                        true,
                        rungs,
                        cx,
                        cx.listener(move |this, _, window, cx| {
                            this.save(id, true, &root, window, cx);
                        }),
                    ))
                })
                .into_any_element(),
        }
    }
}
