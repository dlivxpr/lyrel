use super::*;

mod fidelity;
#[cfg(any(test, feature = "native-markdown-tests"))]
mod fidelity_tests;
#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_cases() -> impl Iterator<Item = crate::native_markdown::Case> {
    fidelity_tests::native_cases()
}

/// A captured workbench intent. The editor never resolves it from current focus.
#[derive(Clone, Debug)]
pub(crate) enum AfterUnsaved {
    CloseTab(TabId),
    CloseFiles {
        tab: TabId,
        ids: Vec<BufferId>,
        group: split::GroupId,
    },
    ClosePane(gpui_kit::EntityId),
    CloseWindow,
    Quit,
}

pub(super) struct SaveWaiter {
    ids: Vec<BufferId>,
    then: AfterUnsaved,
}

#[derive(Clone, Copy)]
pub(crate) enum BarKind {
    GoToLine { id: BufferId },
    SaveAs { id: BufferId },
    Rename { id: BufferId, offset: usize },
}

pub(super) struct EditorBar {
    kind: BarKind,
    identity: SaveIdentity,
    input: Entity<InputState>,
    _sub: Subscription,
}

#[derive(Clone)]
struct SaveIdentity {
    id: BufferId,
    host: HostId,
    path: PathBuf,
    untitled: Option<u32>,
    epoch: u64,
    document: gpui_kit::EntityId,
}

fn document_identity(file: &OpenFile, cx: &gpui_kit::App) -> gpui_kit::EntityId {
    match &file.input {
        EditorInput::Code(input) => input.entity_id(),
        EditorInput::Markdown(view) => view.read(cx).document().entity_id(),
        // SVG's identity is the shared source Document, not one group's view.
        EditorInput::Svg(view) => view.read(cx).document().read(cx).document().entity_id(),
        // JSON's identity is likewise the shared source Document and history.
        EditorInput::Json(view) => view.read(cx).document().read(cx).document().entity_id(),
        // YAML's identity is likewise the shared source Document and history.
        EditorInput::Yaml(view) => view.read(cx).document().read(cx).document().entity_id(),
        // TOML uses the same one-document identity and ordinary history.
        EditorInput::Toml(view) => view.read(cx).document().read(cx).document().entity_id(),
        // CSV/TSV has the same one-document identity as its source view.
        EditorInput::Csv(view) => view.read(cx).document().read(cx).document().entity_id(),
        // XML likewise saves the shared source Document, never a tree copy.
        EditorInput::Xml(view) => view.read(cx).document().read(cx).document().entity_id(),
        // Read-only content still has a stable identity for save-side prompts.
        EditorInput::Image(view) => view.entity_id(),
        EditorInput::Sqlite(view) => view.entity_id(),
        EditorInput::Book(view) => view.read(cx).document().entity_id(),
        EditorInput::Pdf(view) => view.read(cx).document().entity_id(),
        // A draw.io preview also never saves in this ticket; the document is
        // the stable identity a prompt would name.
        EditorInput::Drawio(view) => view.read(cx).document().entity_id(),
    }
}

impl EditorPanelState {
    fn save_identity(&self, id: BufferId, cx: &gpui_kit::App) -> Option<SaveIdentity> {
        let file = self.buffers.get(id)?;
        Some(SaveIdentity {
            id,
            host: file.host.id(),
            path: file.path.clone(),
            untitled: file.untitled,
            epoch: self.prompt_epoch,
            document: document_identity(file, cx),
        })
    }

    fn accepts_prompt(&mut self, identity: &SaveIdentity, cx: &gpui_kit::App) -> bool {
        if self.prompt_epoch != identity.epoch {
            return false;
        }
        let valid = self.buffers.get(identity.id).is_some_and(|file| {
            file.host.id() == identity.host
                && file.path == identity.path
                && file.untitled == identity.untitled
                && document_identity(file, cx) == identity.document
        });
        if !valid {
            self.saves_failed(identity.id);
        }
        valid
    }

    pub(crate) fn retire_prompts(&mut self) {
        self.prompt_epoch = self.prompt_epoch.wrapping_add(1);
        self.unsaved_prompt_open = false;
        self.waiters.clear();
        self.dismiss_bar();
    }

    pub(crate) fn save(
        &mut self,
        id: BufferId,
        force: bool,
        root: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_at(id, force, None, root, window, cx);
    }

    pub(crate) fn save_at(
        &mut self,
        id: BufferId,
        force: bool,
        save_as: Option<PathBuf>,
        root: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Read-only content — a picture, database, book or PDF — has no bytes
        // the workbench may write back, so Save and Save As do nothing here.
        if self
            .buffers
            .get(id)
            .is_some_and(|file| !file.input.is_savable())
        {
            return;
        }
        if let Some(file) = self.buffers.get(id) {
            file.input.finish_composition(window, cx);
        }
        let Some(file) = self.buffers.get_mut(id) else {
            return;
        };
        if file.saving {
            if let Some(target) = save_as {
                file.save_as_pending = Some(target);
            } else {
                file.save_pending = true;
            }
            return;
        }
        if file.untitled.is_some() && save_as.is_none() {
            self.save_as(id, root, window, cx);
            return;
        }
        // A diagram save must write the engine's live model, not the last
        // committed bytes: defer the write through every live surface's sync
        // flush until each request it actually sent is confirmed.
        let flushed = std::mem::take(&mut file.drawio_flush_done);
        if file.input.drawio().is_some() && file.drawio_flush.is_none() && !flushed {
            let has_bytes = file
                .input
                .drawio_document(cx)
                .is_some_and(|document| document.bytes().is_some());
            if !has_bytes {
                // A failed document has no ready bytes to write.
                return;
            }
            let mut views = Vec::new();
            if let Some(primary) = file.input.drawio() {
                views.push(primary.clone());
            }
            for (_, _, view) in file.presentations.drawios().iter() {
                if !views.iter().any(|existing| existing == view) {
                    views.push(view.clone());
                }
            }
            // Every live presentation must flush: a short-circuit would leave
            // an engine whose model is newer than the committed bytes. Each
            // queued request keeps its own ticket, so an earlier commit, a
            // sibling's callback or a previous save cannot release this one.
            let mut tickets = Vec::new();
            let mut refused = None;
            for view in &views {
                if let Some(request) = view.update(cx, |view, _| view.flush()) {
                    tickets.push((view.downgrade(), request));
                }
                if refused.is_none() {
                    refused = view.read(cx).unconfirmed_reason();
                }
            }
            if !tickets.is_empty() || refused.is_some() {
                file.drawio_flush = Some(DrawioFlush {
                    tickets,
                    refused,
                    deadline: std::time::Instant::now() + super::DRAWIO_FLUSH_TIMEOUT,
                    force,
                    target: save_as,
                });
                cx.notify();
                return;
            }
        }
        // Markdown hard breaks and unknown syntax are source: ordinary saves do
        // not apply code cleanup. Explicit workbench cleanup remains undoable.
        if file.input.code().is_some() {
            let text = file.input.text().unwrap().read(cx).text().to_string();
            if let Some(fixed) = lyrel_document::text::apply_save_rules(&text, &file.config) {
                let input = file.input.clone();
                replace_buffer_text(&input, &fixed, window, cx);
            }
        }
        let captured = match fidelity::SaveContent::capture(self.buffers.get(id).unwrap(), cx) {
            Ok(captured) => captured,
            Err(error) => {
                self.offer_utf8(id, error.unmappable, save_as, window, cx);
                return;
            }
        };
        let identity = self.save_identity(id, cx).unwrap();
        let file = self.buffers.get_mut(id).unwrap();
        file.saving = true;
        let expect = match (save_as.is_some(), force, file.conflict) {
            (true, _, _) | (_, true, _) | (_, _, Some(DiskConflict::Deleted)) => None,
            _ => Some(file.disk_mtime),
        };
        let fidelity::SaveContent {
            bytes,
            written,
            format,
            representation,
            revision,
        } = captured;
        let host = file.host.clone();
        let target = save_as.clone().unwrap_or_else(|| file.path.clone());
        let directory = target.parent().map(Path::to_path_buf);
        HostOps::run_in(
            host,
            window,
            cx,
            move |host| write_checked(host, &target, &bytes, expect),
            move |editor, outcome, window, cx| {
                let Some(file) = editor.buffers.get_mut(id) else {
                    return;
                };
                file.saving = false;
                let pending = std::mem::take(&mut file.save_pending);
                let next_target = file.save_as_pending.take();
                // A rename or another Save As can change a buffer's identity
                // while I/O is in flight. Its old write cannot clean the new file.
                if file.host.id() != identity.host
                    || file.path != identity.path
                    || file.untitled != identity.untitled
                    || document_identity(file, cx) != identity.document
                {
                    editor.saves_failed(id);
                    // A request queued against the old identity must not be
                    // silently redirected to the renamed/replaced document.
                    cx.notify();
                    return;
                }
                match outcome {
                    SaveOutcome::Saved(mtime) => {
                        if let Some(target) = save_as.as_ref() {
                            // Adopt the target only after a successful write. A
                            // cancelled/failed Save As keeps the original identity.
                            if editor
                                .buffers
                                .at(identity.host, target)
                                .is_some_and(|other| other != id)
                            {
                                editor.saves_failed(id);
                                cx.notify();
                                return;
                            }
                            let file = editor.buffers.get_mut(id).unwrap();
                            file.path = target.clone();
                            file.untitled = None;
                            file.reload_seq = file.reload_seq.wrapping_add(1);
                            file.reload_request = file.reload_request.wrapping_add(1);
                            file.input.set_highlighter(file.language(), cx);
                            let source_only = file
                                .path
                                .extension()
                                .is_some_and(|extension| extension.eq_ignore_ascii_case("mdx"));
                            file.presentations
                                .set_source_only(source_only, &file.input, cx);
                        }
                        let file = editor.buffers.get_mut(id).unwrap();
                        file.disk_mtime = mtime;
                        file.saved = written;
                        file.saved_format = format;
                        file.saved_representation = representation;
                        file.conflict = None;
                        editor.buffers.note_edit(id, cx);
                        if editor.buffers.get(id).is_some_and(|file| {
                            file.dirty
                                || revision.is_some_and(|saved_revision| {
                                    file.input.source_edit_revision(cx) != Some(saved_revision)
                                })
                        }) {
                            // New edits/representation changes retire this close.
                            // A later save is not consent to close that newer work.
                            editor.saves_failed(id);
                        }
                        editor.on_saved(id, identity.host, directory, cx);
                        if save_as.is_some() {
                            cx.emit(EditorPanelEvent::IdentityChanged);
                        }
                        if let Some(target) = next_target {
                            editor.save_to(id, target, window, cx);
                        } else if pending {
                            editor.save(id, false, Path::new(""), window, cx);
                        } else {
                            let ready = editor.take_saved_intents();
                            for intent in ready {
                                cx.emit(EditorPanelEvent::Continue(intent));
                            }
                        }
                    }
                    SaveOutcome::Conflict(observed) => {
                        file.conflict = Some(DiskConflict::Changed(observed));
                        editor.saves_failed(id);
                        editor.ask_overwrite(id, window, cx);
                    }
                    SaveOutcome::Failed(error) => {
                        let context = t_fmt(L10nKey::EditorSaveFailed, &[("name", &file.label())]);
                        HostOps::notify_err(window, cx, &context, &error);
                        editor.saves_failed(id);
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// A finished save: the gutter re-fetches, the panel re-derives its edit
    /// state, and the shell hears about it for LSP and SCM.
    fn on_saved(
        &mut self,
        id: BufferId,
        host: HostId,
        directory: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.gutter_refetch(id);
        self.buffer_changed(id, cx);
        cx.emit(EditorPanelEvent::BufferSaved {
            id,
            host,
            directory,
        });
    }

    fn ask_overwrite(&mut self, id: BufferId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(identity) = self.save_identity(id, cx) else {
            return;
        };
        let name = self.buffers.get(id).unwrap().label();
        let answer = window.prompt(
            PromptLevel::Warning,
            &t_fmt(L10nKey::EditorSaveConflictTitle, &[("name", &name)]),
            Some(t(L10nKey::EditorSaveConflictBody)),
            &crate::ui::confirm_answers(t(L10nKey::EditorOverwrite), t(L10nKey::Cancel)),
            cx,
        );
        cx.spawn_in(window, async move |view, cx| {
            let choice = answer.await;
            let _ = view.update_in(cx, |editor, window, cx| {
                if !editor.accepts_prompt(&identity, cx) {
                    return;
                }
                if matches!(choice, Ok(0)) {
                    editor.save(id, true, Path::new(""), window, cx);
                } else {
                    cx.emit(EditorPanelEvent::FocusDocument);
                }
            });
        })
        .detach();
    }

    fn offer_utf8(
        &mut self,
        id: BufferId,
        unmappable: char,
        target: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(identity) = self.save_identity(id, cx) else {
            return;
        };
        let file = self.buffers.get(id).unwrap();
        let name = file.label();
        // Only documents with a text format reach the encoding prompt.
        let encoding = file.format(cx).expect("text document").encoding_label();
        self.saves_failed(id);
        let answer = window.prompt(
            PromptLevel::Warning,
            &t_fmt(
                L10nKey::EditorEncodeFailedTitle,
                &[("name", &name), ("encoding", encoding)],
            ),
            Some(&t_fmt(
                L10nKey::EditorEncodeFailedBody,
                &[("ch", &unmappable.to_string()), ("encoding", encoding)],
            )),
            &crate::ui::confirm_answers(t(L10nKey::EditorSaveAsUtf8), t(L10nKey::Cancel)),
            cx,
        );
        cx.spawn_in(window, async move |view, cx| {
            let choice = answer.await;
            let _ = view.update_in(cx, |editor, window, cx| {
                if !editor.accepts_prompt(&identity, cx) {
                    return;
                }
                if matches!(choice, Ok(0)) {
                    let file = editor.buffers.get_mut(id).unwrap();
                    file.set_format(
                        TextFormat {
                            line_ending: file.format(cx).expect("text document").line_ending,
                            ..TextFormat::default()
                        },
                        cx,
                    );
                    editor.save_at(id, false, target, Path::new(""), window, cx);
                } else {
                    cx.emit(EditorPanelEvent::FocusDocument);
                }
            });
        })
        .detach();
    }

    pub(crate) fn save_as(
        &mut self,
        id: BufferId,
        root: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A read-only picture must not open a path picker it can never write
        // through: Save As is about bytes this document does not have.
        if self
            .buffers
            .get(id)
            .is_some_and(|file| !file.input.is_savable())
        {
            return;
        }
        let Some(identity) = self.save_identity(id, cx) else {
            return;
        };
        let file = self.buffers.get(id).unwrap();
        let suggested = if file.untitled.is_some() {
            let extension = if file.input.markdown().is_some() {
                "md"
            } else if file.input.svg().is_some() {
                "svg"
            } else if file.input.yaml().is_some() {
                "yaml"
            } else if file.input.toml().is_some() {
                "toml"
            } else {
                "txt"
            };
            PathBuf::from(format!("{}.{extension}", file.label()))
        } else {
            PathBuf::from(file.label().to_string())
        };
        let dir = file
            .path
            .parent()
            .filter(|_| file.untitled.is_none())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.to_path_buf());
        if !file.host.id().is_local() {
            let initial = if dir.as_os_str().is_empty() {
                suggested
            } else {
                file.host.join(&dir, &suggested.to_string_lossy())
            };
            self.open_bar(
                BarKind::SaveAs { id },
                initial.display().to_string(),
                window,
                cx,
            );
            return;
        }
        let dir = if dir.as_os_str().is_empty() {
            dirs_home()
        } else {
            dir
        };
        let answer = cx.prompt_for_new_path(&dir, suggested.to_str());
        cx.spawn_in(window, async move |view, cx| {
            let result = answer.await;
            let _ = view.update_in(cx, |editor, window, cx| {
                if !editor.accepts_prompt(&identity, cx) {
                    return;
                }
                if let Ok(Ok(Some(path))) = result {
                    editor.save_to(id, path, window, cx);
                } else {
                    editor.saves_failed(id);
                    cx.emit(EditorPanelEvent::FocusDocument);
                }
            });
        })
        .detach();
    }

    fn save_to(
        &mut self,
        id: BufferId,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = self.buffers.get(id) else {
            return;
        };
        if self
            .buffers
            .at(file.host.id(), &path)
            .is_some_and(|other| other != id)
        {
            self.saves_failed(id);
            window.notify_app(
                t_fmt(
                    L10nKey::EditorAlreadyOpen,
                    &[("path", &path.display().to_string())],
                ),
                cx,
            );
            return;
        }
        self.save_at(id, true, Some(path), Path::new(""), window, cx);
    }

    pub(crate) fn open_bar(
        &mut self,
        kind: BarKind,
        initial: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = match kind {
            BarKind::GoToLine { id } | BarKind::SaveAs { id } | BarKind::Rename { id, .. } => id,
        };
        let Some(identity) = self.save_identity(id, cx) else {
            return;
        };
        self.dismiss_bar();
        let placeholder = match kind {
            BarKind::GoToLine { id } => {
                let total = self
                    .buffers
                    .get(id)
                    .and_then(|file| file.input.text().map(|t| t.read(cx).text().lines_len()))
                    .unwrap_or(1);
                t_fmt(
                    L10nKey::EditorGoToLinePlaceholder,
                    &[("total", &total.to_string())],
                )
            }
            BarKind::SaveAs { .. } => t(L10nKey::EditorSaveAsPlaceholder).to_string(),
            BarKind::Rename { .. } => t_fmt(L10nKey::LspRenamePlaceholder, &[("name", &initial)]),
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(initial)
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            move |editor, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    editor.submit_bar(window, cx);
                }
                InputEvent::Blur
                    if editor
                        .bar
                        .as_ref()
                        .is_some_and(|bar| !matches!(bar.kind, BarKind::SaveAs { .. })) =>
                {
                    editor.dismiss_bar();
                    cx.notify();
                }
                _ => {}
            },
        );
        input.update(cx, |input, cx| input.focus(window, cx));
        self.bar = Some(EditorBar {
            kind,
            identity,
            input,
            _sub: sub,
        });
        cx.notify();
    }

    pub(crate) fn dismiss_bar(&mut self) {
        self.bar = None;
    }

    pub(crate) fn has_bar(&self) -> bool {
        self.bar.is_some()
    }

    pub(crate) fn bar_has_focus(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        self.bar.as_ref().is_some_and(|bar| {
            bar.input
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
        })
    }

    fn submit_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bar) = self.bar.as_ref() else {
            return;
        };
        let identity = bar.identity.clone();
        let kind = bar.kind;
        let input = bar.input.clone();
        if !self.accepts_prompt(&identity, cx) {
            self.dismiss_bar();
            cx.emit(EditorPanelEvent::FocusDocument);
            return;
        }
        let text = input.read(cx).value().to_string();
        match kind {
            BarKind::GoToLine { id } => {
                let Some((line, column)) = parse_line_target(&text) else {
                    return;
                };
                self.bar = None;
                cx.emit(EditorPanelEvent::GoToLineRequested { id, line, column });
            }
            BarKind::Rename { id, offset } => {
                let name = text.trim().to_string();
                if name.is_empty() {
                    return;
                }
                self.bar = None;
                cx.emit(EditorPanelEvent::RenameRequested { id, offset, name });
            }
            BarKind::SaveAs { id } => {
                let path = PathBuf::from(text.trim());
                let Some(file) = self.buffers.get(id) else {
                    return;
                };
                let host = file.host.clone();
                if path.as_os_str().is_empty() || !host.is_absolute(&path) {
                    return;
                }
                self.bar = None;
                let check = path.clone();
                HostOps::run_in(
                    host,
                    window,
                    cx,
                    move |host| host.exists(&check),
                    move |editor, exists, window, cx| {
                        if !editor.accepts_prompt(&identity, cx) {
                            return;
                        }
                        if !exists {
                            editor.save_to(id, path, window, cx);
                            return;
                        }
                        let answer = window.prompt(
                            PromptLevel::Warning,
                            &t_fmt(
                                L10nKey::EditorReplaceExisting,
                                &[("path", &path.display().to_string())],
                            ),
                            None,
                            &crate::ui::confirm_answers(
                                t(L10nKey::EditorReplace),
                                t(L10nKey::Cancel),
                            ),
                            cx,
                        );
                        cx.spawn_in(window, async move |view, cx| {
                            let choice = answer.await;
                            let _ = view.update_in(cx, |editor, window, cx| {
                                if !editor.accepts_prompt(&identity, cx) {
                                    return;
                                }
                                if matches!(choice, Ok(0)) {
                                    editor.save_to(id, path, window, cx);
                                } else {
                                    editor.saves_failed(id);
                                    cx.emit(EditorPanelEvent::FocusDocument);
                                }
                            });
                        })
                        .detach();
                    },
                );
                cx.notify();
                cx.emit(EditorPanelEvent::FocusDocument);
            }
        }
    }

    /// Asks about unsaved buffers before `then` goes ahead. Returns `true`
    /// when there was something to ask about: the caller stops there, and the
    /// answer carries the close on — after saving, after discarding, or not
    /// at all.
    pub(crate) fn guard_unsaved(
        &mut self,
        ids: Vec<BufferId>,
        then: AfterUnsaved,
        root: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if ids.is_empty() {
            return false;
        }
        if self.unsaved_prompt_open {
            return true;
        }
        self.unsaved_prompt_open = true;
        let epoch = self.prompt_epoch;
        let identities: Vec<_> = ids
            .iter()
            .filter_map(|id| self.save_identity(*id, cx))
            .collect();
        let names: Vec<_> = ids
            .iter()
            .filter_map(|id| self.buffers.get(*id).map(OpenFile::label))
            .collect();
        let title = match names.as_slice() {
            [name] => t_fmt(L10nKey::EditorUnsavedChanges, &[("name", name)]),
            _ => t_fmt(
                L10nKey::EditorUnsavedChangesMany,
                &[("count", &names.len().to_string())],
            ),
        };
        let mut body: Vec<_> = names.iter().take(8).map(ToString::to_string).collect();
        if names.len() > 8 {
            body.push("…".into());
        }
        let body = (names.len() > 1).then(|| body.join("\n"));
        let root = root.to_path_buf();
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            body.as_deref(),
            &[
                gpui_kit::PromptButton::ok(if names.len() > 1 {
                    t(L10nKey::EditorSaveAll)
                } else {
                    t(L10nKey::Save)
                }),
                gpui_kit::PromptButton::cancel(t(L10nKey::Cancel)),
                gpui_kit::PromptButton::ok(t(L10nKey::EditorDiscard)),
            ],
            cx,
        );
        cx.spawn_in(window, async move |view, cx| {
            let choice = answer.await;
            let _ = view.update_in(cx, |editor, window, cx| {
                if editor.prompt_epoch != epoch {
                    return;
                }
                editor.unsaved_prompt_open = false;
                // A missing/replaced/renamed target is not consent to save or discard.
                if identities.len() != ids.len()
                    || identities
                        .iter()
                        .any(|identity| !editor.accepts_prompt(identity, cx))
                {
                    return;
                }
                match choice {
                    Ok(0) => {
                        editor.waiters.push(SaveWaiter {
                            ids: ids.clone(),
                            then,
                        });
                        for id in ids {
                            editor.save(id, false, &root, window, cx);
                        }
                    }
                    Ok(2) => {
                        cx.emit(EditorPanelEvent::BuffersDiscarded { ids: ids.clone() });
                        cx.emit(EditorPanelEvent::Continue(then));
                    }
                    _ => {
                        cx.emit(EditorPanelEvent::FocusDocument);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        true
    }

    fn take_saved_intents(&mut self) -> Vec<AfterUnsaved> {
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiters)
            .into_iter()
            .partition(|waiter| {
                waiter.ids.iter().all(|id| {
                    self.buffers
                        .get(*id)
                        .is_some_and(|file| !file.dirty && !file.saving && file.untitled.is_none())
                })
            });
        self.waiters = waiting;
        ready.into_iter().map(|waiter| waiter.then).collect()
    }

    pub(crate) fn saves_failed(&mut self, id: BufferId) {
        self.waiters.retain(|waiter| !waiter.ids.contains(&id));
    }

    pub(crate) fn forget_saved_buffer(&mut self, id: BufferId) {
        self.saves_failed(id);
        if self.bar.as_ref().is_some_and(|bar| match bar.kind {
            BarKind::GoToLine { id: target }
            | BarKind::SaveAs { id: target }
            | BarKind::Rename { id: target, .. } => target == id,
        }) {
            self.dismiss_bar();
        }
    }

    pub(crate) fn render_bar(&self, cx: &gpui_kit::App) -> Option<AnyElement> {
        let bar = self.bar.as_ref()?;
        let label = match bar.kind {
            BarKind::GoToLine { .. } => t(L10nKey::EditorGoToLine),
            BarKind::SaveAs { .. } => t(L10nKey::EditorSaveAs),
            BarKind::Rename { .. } => t(L10nKey::LspRenameSymbol),
        };
        Some(
            h_flex()
                .flex_none()
                .w_full()
                .items_center()
                .gap_2()
                .px(px(crate::ui::window_chrome::CONTENT_INSET))
                .py_1()
                .border_b_1()
                .border_color(cx.theme().sidebar_border)
                .text_sm()
                .child(
                    div()
                        .flex_none()
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&bar.input).small()),
                )
                .into_any_element(),
        )
    }
}

enum SaveOutcome {
    Saved(Option<MTime>),
    Conflict(Option<MTime>),
    Failed(std::io::Error),
}

fn write_checked(
    host: &dyn lyrel_core::host::Host,
    target: &Path,
    bytes: &[u8],
    expect: Option<Option<MTime>>,
) -> SaveOutcome {
    if let Some(expected) = expect {
        match host.stat(target) {
            Ok(meta) if meta.mtime != expected => return SaveOutcome::Conflict(meta.mtime),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if expected.is_some() {
                    return SaveOutcome::Conflict(None);
                }
            }
            Err(error) => return SaveOutcome::Failed(error),
        }
    }
    match host.write_file(target, bytes) {
        Ok(meta) => SaveOutcome::Saved(meta.mtime),
        Err(error) => SaveOutcome::Failed(error),
    }
}

#[cfg(test)]
mod tests;
