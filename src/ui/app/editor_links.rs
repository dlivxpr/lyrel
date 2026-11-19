//! Markdown link navigation's shell half.
//!
//! The editor resolves a link and, for a file destination, emits
//! `EditorPanelEvent::FollowLink` (`EditorPanelState::follow_link`). Locating
//! the originating presentation's tab and group is tab-set knowledge, so it
//! lives here with the rest of the shell coordination: the tabs are walked,
//! any anchor is decoded on the file's owning Host — never a local lookalike
//! — and the file opens through the editor's epoch-guarded open, placing a
//! reused buffer exactly where the pre-inversion link opener did.

use std::path::Path;

use super::*;
use crate::ui::host_ops::{HostOps, SharedHost};
use gpui_kit::EntityId;
use gpui_kit::component::input::Position;
use lyrel_workbench_editor::code_editor::markdown::links;
use lyrel_workbench_editor::code_editor::{BufferId, GroupId};

impl LyrelApp {
    /// Answers `EditorPanelEvent::FollowLink`: find the tab and group whose
    /// editor group still shows the originating presentation, then open the
    /// destination there. A presentation that no tab claims — buffer closed,
    /// group merged away — has nowhere to open and is dropped.
    pub(crate) fn editor_follow_link(
        &mut self,
        buffer: BufferId,
        source: EntityId,
        host: SharedHost,
        path: &Path,
        anchor: Option<String>,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((tab, group)) = self.tabs.iter().find_map(|entry| {
            let code = entry.code.as_ref()?;
            std::iter::once(code.group())
                .chain(code.split.as_ref().map(|other| other.id))
                .find_map(|group| {
                    self.editor
                        .read(cx)
                        .markdown_presentation_in(buffer, entry.id(), group, source)
                        .then_some((entry.id(), group))
                })
        }) else {
            return;
        };
        let origin = Some((buffer, revision));
        if let Some(anchor) = anchor {
            let requested = path.to_path_buf();
            let destination_host = host.clone();
            let path = path.to_path_buf();
            // Decode on the same owning Host, never by opening a local lookalike.
            HostOps::run_in(
                host,
                window,
                cx,
                move |h| links::locate_anchor(h, &requested, &anchor),
                move |app, result, window, cx| match result {
                    Ok(cursor) => app.editor_open_link(
                        destination_host,
                        &path,
                        Some(cursor),
                        origin,
                        tab,
                        group,
                        window,
                        cx,
                    ),
                    Err(error) => {
                        HostOps::notify_err(window, cx, t(L10nKey::MarkdownFollowLink), &error)
                    }
                },
            );
        } else {
            self.editor_open_link(host, path, None, origin, tab, group, window, cx);
        }
    }

    /// Opens a link's file destination in the originating presentation's own
    /// tab and group. A buffer that was already open is placed immediately,
    /// exactly like the old link opener; a load lands later through the
    /// `OpenCompleted` placement, which re-validates tab and group.
    fn editor_open_link(
        &mut self,
        host: SharedHost,
        path: &Path,
        cursor: Option<Position>,
        origin: Option<(BufferId, u64)>,
        tab: TabId,
        group: GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace;
        let window_host = self.spawn_host(cx);
        // `origin_epoch` is captured at open start; the revision captured when
        // the link was followed still guards against a document that moved on.
        let reused = self.editor.update(cx, |editor, cx| {
            editor.open_on_host_at(
                host,
                path,
                cursor,
                origin,
                workspace,
                tab,
                Some(group),
                window_host,
                window,
                cx,
            )
        });
        if let Some(id) = reused {
            self.editor_show_open_target(tab, Some(group), id, true, window, cx);
        }
    }
}
