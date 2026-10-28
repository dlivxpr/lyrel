//! 剪贴板权限、输入路由与文件投递的 GPUI 适配。
//!
//! `TerminalView` 保持 pane、焦点和附件 identity；准备与上传状态仍由
//! `terminal::paste` 持有。所有动作、菜单、composer 和拖放入口共用这里的投递路径。
//! 远端剪贴板校验捕获既有 generation，relink 后的旧结果不会写入新附件。

use super::*;
use crate::terminal::paste::{
    PasteUploads, clipboard_paths, local_path_for_pane, pasted_paths_text, remote_paste_spec,
    stages_clipboard_image, write_clipboard_image,
};
use crate::terminal::prompt_input::bytes::paste_bytes;
use gpui_kit::ClipboardEntry;

fn trim_trailing_spaces(text: &str) -> String {
    text.split('\n')
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether this pane may hand a remote program's image to the system clipboard.
///
/// The daemon is the gate. It holds the `NativeSshSpec` that dialled the host
/// and answers a write the profile forbids with `EPERM` before a byte of image
/// reaches this process; a pane it never granted the permission to sends no
/// `ClipboardWrite` at all. This is a second opinion, and it can only give one
/// when the pane kept a copy of that spec. A pane restored by attaching to its
/// id did not keep one — reading that absence as "forbidden" is what put the
/// permission to sleep on the first restart after the user granted it. So:
/// refuse what this side can see is forbidden, and defer otherwise.
fn allows_remote_clipboard_write(
    workspace: Option<&crate::terminal::PaneWorkspace>,
    ssh_spec: Option<&crate::daemon::protocol::NativeSshSpec>,
) -> bool {
    remote_paste_spec(workspace, ssh_spec).is_none_or(|spec| spec.remote_clipboard_write)
}

fn validate_remote_clipboard_image(
    write: lyrel_core::core::clipboard::ClipboardWrite,
) -> Result<(gpui_kit::Image, Option<String>), String> {
    let (gpui_format, image_format) = match write.mime.as_str() {
        "image/png" => (gpui_kit::ImageFormat::Png, image::ImageFormat::Png),
        "image/jpeg" | "image/jpg" => (gpui_kit::ImageFormat::Jpeg, image::ImageFormat::Jpeg),
        "image/gif" => (gpui_kit::ImageFormat::Gif, image::ImageFormat::Gif),
        "image/webp" => (gpui_kit::ImageFormat::Webp, image::ImageFormat::WebP),
        _ => return Err("unsupported image MIME type".into()),
    };
    if image::guess_format(&write.data).ok() != Some(image_format) {
        return Err("image signature does not match its MIME type".into());
    }

    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(&write.data), image_format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(256 << 20);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|e| format!("invalid or oversized image: {e}"))?;

    Ok((
        gpui_kit::Image::from_bytes(gpui_format, write.data),
        write.id,
    ))
}

impl TerminalView {
    pub(super) fn poll_remote_clipboard_write(&mut self, cx: &mut Context<Self>) {
        if self.remote_clipboard_write_in_flight.is_some() {
            return;
        }
        let write = loop {
            let Some(write) = self.terminal.pop_clipboard_write() else {
                return;
            };
            if allows_remote_clipboard_write(self.workspace.as_ref(), self.ssh_spec.as_deref()) {
                break write;
            }
            self.terminal.finish_clipboard_write();
            self.terminal.write(lyrel_core::core::clipboard::response(
                write.id.as_deref(),
                "EPERM",
            ));
        };

        let generation = self.remote_clipboard_write_generation;
        self.remote_clipboard_write_in_flight = Some(generation);
        let request_id = write.id.clone();
        cx.spawn(async move |view, cx| {
            let validated = cx
                .background_spawn(async move { validate_remote_clipboard_image(write) })
                .await;
            view.update(cx, |view, cx| {
                if view.remote_clipboard_write_in_flight != Some(generation) {
                    return;
                }
                view.remote_clipboard_write_in_flight = None;
                view.terminal.finish_clipboard_write();
                match validated {
                    Ok((image, id)) => {
                        cx.write_to_clipboard(ClipboardItem::new_image(&image));
                        view.terminal
                            .write(lyrel_core::core::clipboard::response(id.as_deref(), "DONE"));
                    }
                    Err(reason) => {
                        log::warn!("refusing remote clipboard image: {reason}");
                        view.terminal.write(lyrel_core::core::clipboard::response(
                            request_id.as_deref(),
                            "EINVAL",
                        ));
                    }
                }
                view.poll_remote_clipboard_write(cx);
            })
            .ok();
        })
        .detach();
    }

    /// `AlternatePaste`, with the grid asked again before it pastes.
    ///
    /// The `!alt_screen` half of the binding's context comes from the frame
    /// that was last *painted*, and gpui matches keystrokes against that frame
    /// — so a program that took the alternate screen after the last paint is
    /// still "at a prompt" as far as the keymap is concerned. One frame is
    /// enough: the keystroke that launches a full-screen program and the
    /// Ctrl+V after it can land either side of a paint. Pasting there is not a
    /// mistake the user can take back — vim in normal mode runs the clipboard
    /// as commands — so the last word belongs to the terminal mode, not to the
    /// frame. Propagating hands the chord on to `on_key_down`, which encodes it
    /// as the SYN the program is waiting for.
    pub(super) fn alternate_paste(&mut self, cx: &mut Context<Self>) {
        if self.on_alt_screen() {
            cx.propagate();
            return;
        }
        self.paste_from_clipboard(cx);
    }

    pub fn paste(&mut self, text: String, cx: &mut Context<Self>) {
        if !self.accepts_input(cx) {
            return;
        }
        // The agent's input is under the box, so a paste meant for it goes
        // into the box — out of sight under it, it would be sent unseen.
        if self.composer_takes_typing(&text, cx) {
            return;
        }
        // Same reason as `commit_text`: what is pasted lands on the prompt, so
        // the prompt is what has to be on screen. Neither branch below moved
        // the viewport, and a paste is a bigger change than a keystroke to
        // make out of sight. This also clears the selection, which the tail of
        // this function used to do on its own.
        self.jump_to_prompt();
        if self.input_active() {
            self.input
                .paste_editor(&text, &mut self.history, &mut self.completion);
            self.lifecycle.show_cursor();
            cx.notify();
            return;
        }
        let bracketed = self
            .terminal
            .term
            .lock()
            .mode()
            .contains(Mode::BRACKETED_PASTE);
        self.write_gap_text(&text, paste_bytes(&text, bracketed), true, cx);
        cx.notify();
    }
    pub fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let cfg = cx.global::<Config>();
        let text = self.terminal.term.lock().selection_to_string();
        if let Some(mut text) = text {
            if cfg.clipboard_trim_trailing_spaces {
                text = trim_trailing_spaces(&text);
            }
            if !text.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }

    pub fn copy_contextual(&mut self, clear_on_copy: bool, cx: &mut Context<Self>) -> bool {
        if self.input_active() {
            if let Some(text) = self.input.editor().selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                if clear_on_copy {
                    self.input.editor_mut().clear_selection();
                    cx.notify();
                }
                return true;
            }
        }
        if self.has_selection() {
            self.copy_selection(cx);
            if clear_on_copy {
                self.terminal.term.lock().selection = None;
                cx.notify();
            }
            return true;
        }
        false
    }
    pub fn cut_contextual(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.input_active() {
            return false;
        }
        if let Some(text) = self.input.editor().selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.input.editor_mut().delete_selection();
            self.close_completion();
            self.lifecycle.show_cursor();
            cx.notify();
        }
        true
    }

    pub fn paste_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let paths = clipboard_paths(&item);
        if !paths.is_empty() {
            self.paste_local_paths(paths, cx);
            return;
        }
        if let Some(text) = item.text() {
            self.paste(text, cx);
            return;
        }
        if self.input_active() {
            return;
        }
        if let Some(img) = item.entries().iter().find_map(|e| match e {
            ClipboardEntry::Image(img) => Some(img),
            _ => None,
        }) {
            self.paste_clipboard_image(img, cx);
        }
    }

    pub(in crate::terminal) fn drop_files(
        &mut self,
        paths: &ExternalPaths,
        cx: &mut Context<Self>,
    ) {
        self.paste_local_paths(paths.paths().to_vec(), cx);
    }

    /// A row dragged out of a remote Files tree names a file on that remote,
    /// not here: there is nothing to upload, and the path is pasted as the
    /// tree spells it.
    pub(in crate::terminal) fn drop_remote_path(
        &mut self,
        drag: &crate::ui::file_tree::RemotePathDrag,
        cx: &mut Context<Self>,
    ) {
        self.paste_paths(vec![drag.path.to_string_lossy().into_owned()], cx);
    }

    /// Paste paths already spelled the way the pane's host reads them: into
    /// the composer as attachments when it has the keyboard, and as shell
    /// words onto the line otherwise. Every route that turns files into
    /// something to paste — a drop, a copied file, a screenshot, an upload to
    /// a remote pane — ends here.
    fn paste_paths(&mut self, spelled: Vec<String>, cx: &mut Context<Self>) {
        let Some(spelled) = self.composer_takes_paths(spelled, cx) else {
            return;
        };
        let text = pasted_paths_text(&spelled, self.shell_program().as_deref());
        self.paste(text, cx);
    }

    /// Paste files that live on this machine so the pane's program can open
    /// them: uploaded first when the pane runs on an SSH host, renamed for a
    /// WSL pane, and pasted as they are everywhere else.
    pub(in crate::terminal) fn paste_local_paths(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        // Every step of an upload is a blocking daemon round trip, which a
        // keystroke handler must not make, so a remote pane pastes from a
        // background task and this returns without touching the line.
        if self.upload_for_remote(paths.clone(), cx) {
            return;
        }
        // The upload declined: a local pane, a WSL pane, which needs a rewrite
        // rather than a transfer, or a workspace with no SSH spec to
        // piggyback on.
        let shares_localhost = self
            .workspace
            .as_ref()
            .is_some_and(|w| w.shares_localhost());
        let spelled: Vec<String> = paths
            .iter()
            .map(|p| local_path_for_pane(&p.to_string_lossy(), shares_localhost))
            .collect();
        self.paste_paths(spelled, cx);
    }

    fn paste_clipboard_image(&mut self, img: &gpui_kit::Image, cx: &mut Context<Self>) {
        // A screenshot pasted into the composer is one of its attachments,
        // shown as a chip like a copied file. Forwarded as SYN it would land
        // in the agent's own input, hidden under the box.
        if self.composer_takes_files()
            && let Some(path) = write_clipboard_image(img)
        {
            self.paste_local_paths(vec![path], cx);
            return;
        }
        if self.paste_clipboard_image_as_path(img, cx) {
            return;
        }
        self.terminal.write(vec![0x16]);
        self.terminal.term.lock().selection = None;
        cx.notify();
    }

    /// Stage the clipboard image and paste a path for it, answering whether
    /// this took the paste over. `false` leaves the caller forwarding SYN —
    /// see [`stages_clipboard_image`] for when that is the better path.
    fn paste_clipboard_image_as_path(
        &mut self,
        img: &gpui_kit::Image,
        cx: &mut Context<Self>,
    ) -> bool {
        let is_remote =
            remote_paste_spec(self.workspace.as_ref(), self.ssh_spec.as_deref()).is_some();
        if !stages_clipboard_image(is_remote) {
            return false;
        }
        let Some(path) = write_clipboard_image(img) else {
            return false;
        };
        // A macOS pane only gets here when it is remote — a local one returned
        // above — so pasting a path is right on every platform, and staying
        // silent would be the very no-op this route exists to avoid.
        self.paste_local_paths(vec![path], cx);
        true
    }

    /// Upload local files to the pane's remote host and paste their remote
    /// paths, all off the UI thread. Answers whether this pane took the paste
    /// over; `false` means a local, WSL, or spec-less pane the caller should
    /// paste the local paths for.
    ///
    /// The paste lands as soon as the transfers start — Ctrl+V must not stall
    /// on the wire — and each file appears at its path only once it is whole,
    /// since the upload writes a temporary name and renames it into place. The
    /// transfers are watched to the end, and a failure warns the user that the
    /// path they were handed is dangling.
    fn upload_for_remote(
        &mut self,
        sources: Vec<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(spec) = remote_paste_spec(self.workspace.as_ref(), self.ssh_spec.as_deref())
        else {
            return false;
        };
        let host = format!("{}@{}", spec.user, spec.host);
        let route = crate::ui::sftp::SftpRoute::new(self.pane_id, self.workspace.clone());
        let prepared = self.remote_paste.prepare(&route, &mut cx.to_async());
        let pane_id = self.pane_id;
        cx.spawn(async move |this, cx| {
            let prepared = prepared.await;
            let dir = match this.update(cx, |view, _| view.remote_paste.prepared(prepared)) {
                Ok(Ok(dir)) => dir,
                Ok(Err(reason)) => {
                    Self::paste_local_fallback(&this, cx, &sources, &host, &reason);
                    return;
                }
                Err(_) => return,
            };
            let started =
                PasteUploads::start(route.clone(), pane_id, dir, sources.clone(), cx).await;
            let uploads = match started {
                Ok(uploads) => uploads,
                Err(reason) => {
                    Self::paste_local_fallback(&this, cx, &sources, &host, &reason);
                    return;
                }
            };
            let spelled = uploads.paths();
            let pasted = this.update_in(cx, |view, window, cx| {
                view.paste_paths(spelled, cx);
                for (local, reason) in uploads.failures() {
                    view.warn_paste_upload_failed(local, &host, reason, window, cx);
                }
            });
            if pasted.is_err() {
                return;
            }
            for (local, started) in uploads.into_started() {
                if let Err(reason) = started.watch(route.clone(), cx).await {
                    let _ = this.update_in(cx, |view, window, cx| {
                        view.warn_paste_upload_failed(&local, &host, &reason, window, cx);
                    });
                }
            }
        })
        .detach();
        true
    }

    /// Fall back to the local paths when nothing could be staged on the remote
    /// — the paste is never dropped — and say why they are local.
    fn paste_local_fallback(
        this: &gpui_kit::WeakEntity<Self>,
        cx: &mut gpui_kit::AsyncApp,
        sources: &[std::path::PathBuf],
        host: &str,
        reason: &str,
    ) {
        let spelled: Vec<String> = sources
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let _ = this.update_in(cx, |view, window, cx| {
            view.paste_paths(spelled, cx);
            if let Some(first) = sources.first() {
                view.warn_paste_upload_failed(first, host, reason, window, cx);
            }
        });
    }

    /// One notification per failed file — the pane's line already has a path
    /// in it, and the user is the only one who can tell whether it matters.
    fn warn_paste_upload_failed(
        &self,
        local: &std::path::Path,
        host: &str,
        reason: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        log::warn!(
            "pasted upload of {} to {host} failed: {reason}",
            local.display()
        );
        let name = local
            .file_name()
            .map_or_else(|| local.to_string_lossy(), |n| n.to_string_lossy());
        window.notify_app(
            crate::ui::notification_history::Notification::info(crate::ui::i18n::t_fmt(
                crate::ui::i18n::L10nKey::SftpPasteUploadFailed,
                &[("name", &name), ("host", host), ("error", reason)],
            ))
            .pane(self.owner_workspace(), self.pane_id, &self.pane_route()),
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pane restored by attaching to its id carries no copy of the spec that
    /// dialled the host. The daemon still has one, and still refuses a write
    /// the profile forbids — so a missing copy here is not a verdict.
    #[test]
    fn a_pane_without_its_own_spec_defers_to_the_daemons_verdict() {
        let mut spec: crate::daemon::protocol::NativeSshSpec = serde_json::from_str(
            r#"{"host":"build-box","port":22,"user":"me","auth_mode":"auto"}"#,
        )
        .unwrap();
        assert!(!super::allows_remote_clipboard_write(None, Some(&spec)));
        spec.remote_clipboard_write = true;
        assert!(super::allows_remote_clipboard_write(None, Some(&spec)));
        assert!(super::allows_remote_clipboard_write(None, None));
    }

    #[test]
    fn remote_clipboard_images_must_match_their_declared_mime() {
        let pixel = image::RgbaImage::from_pixel(1, 1, image::Rgba([4, 5, 6, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(pixel)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        let valid = lyrel_core::core::clipboard::ClipboardWrite {
            mime: "image/png".into(),
            data: png.clone(),
            id: None,
        };
        assert!(super::validate_remote_clipboard_image(valid).is_ok());

        let mismatched = lyrel_core::core::clipboard::ClipboardWrite {
            mime: "image/jpeg".into(),
            data: png,
            id: None,
        };
        assert!(super::validate_remote_clipboard_image(mismatched).is_err());
    }

    #[test]
    fn trim_trailing_spaces_strips_per_line_and_preserves_structure() {
        assert_eq!(trim_trailing_spaces("a  \nb\t\nc"), "a\nb\nc");
        assert_eq!(trim_trailing_spaces("a  \n"), "a\n");
        assert_eq!(trim_trailing_spaces("a  "), "a");
        assert_eq!(trim_trailing_spaces("  a  "), "  a");
    }
}

#[cfg(test)]
mod consumer_tests;
