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
pub(super) enum BarKind {
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

pub(super) enum EditorEvent {
    Saved {
        id: BufferId,
        host: HostId,
        directory: Option<PathBuf>,
    },
    Continue(AfterUnsaved),
    Discard(Vec<BufferId>),
    IdentityChanged,
    Focus,
    GoToLine {
        id: BufferId,
        line: u32,
        column: u32,
    },
    Rename {
        id: BufferId,
        offset: usize,
        name: String,
    },
}

/// Typed adapter into the retained workbench entity; implementation only gets
/// editor state. Cross-feature effects leave through semantic events.
pub(super) struct EditorBridge<E> {
    pub(super) access: fn(&mut E) -> &mut EditorPanelState,
    pub(super) event: fn(&mut E, EditorEvent, &mut Window, &mut Context<E>),
}
impl<E> Copy for EditorBridge<E> {}
impl<E> Clone for EditorBridge<E> {
    fn clone(&self) -> Self {
        *self
    }
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

    pub(super) fn retire_prompts(&mut self) {
        self.prompt_epoch = self.prompt_epoch.wrapping_add(1);
        self.unsaved_prompt_open = false;
        self.waiters.clear();
        self.dismiss_bar();
    }

    pub(super) fn save<E: 'static>(
        &mut self,
        id: BufferId,
        force: bool,
        root: &Path,
        window: &mut Window,
        cx: &mut Context<E>,
        bridge: EditorBridge<E>,
    ) {
        self.save_at(id, force, None, root, window, cx, bridge);
    }

    fn save_at<E: 'static>(
        &mut self,
        id: BufferId,
        force: bool,
        save_as: Option<PathBuf>,
        root: &Path,
        window: &mut Window,
        cx: &mut Context<E>,
        bridge: EditorBridge<E>,
    ) {
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
            self.save_as(id, root, window, cx, bridge);
            return;
        }
        // Markdown hard breaks and unknown syntax are source: ordinary saves do
        // not apply code cleanup. Explicit workbench cleanup remains undoable.
        if file.input.code().is_some() {
            let text = file.input.read(cx).text().to_string();
            if let Some(fixed) = lyrel_text::apply_save_rules(&text, &file.config) {
                let input = file.input.clone();
                replace_buffer_text(&input, &fixed, window, cx);
            }
        }
        let captured = match fidelity::SaveContent::capture(self.buffers.get(id).unwrap(), cx) {
            Ok(captured) => captured,
            Err(error) => {
                self.offer_utf8(id, error.unmappable, save_as, window, cx, bridge);
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
        let fidelity::SaveContent { bytes, written, format, representation, revision } = captured;
        let host = file.host.clone();
        let target = save_as.clone().unwrap_or_else(|| file.path.clone());
        let directory = target.parent().map(Path::to_path_buf);
        HostOps::run_in(
            host,
            window,
            cx,
            move |host| write_checked(host, &target, &bytes, expect),
            move |view, outcome, window, cx| {
                let editor = (bridge.access)(view);
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
                            if editor.buffers.at(identity.host, target).is_some_and(|other| other != id) {
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
                            let source_only = file.path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("mdx"));
                            file.presentations.set_source_only(source_only, &file.input, cx);
                        }
                        let file = editor.buffers.get_mut(id).unwrap();
                        file.disk_mtime = mtime;
                        file.saved = written;
                        file.saved_format = format;
                        file.saved_representation = representation;
                        file.conflict = None;
                        editor.buffers.note_edit(id, cx);
                        if editor.buffers.get(id).is_some_and(|file| {
                            file.dirty || revision.is_some_and(|saved_revision| {
                                file.input.markdown().is_some_and(|view| view.read(cx).document().read(cx).revision() != saved_revision)
                            })
                        }) {
                            // New edits/representation changes retire this close.
                            // A later save is not consent to close that newer work.
                            editor.saves_failed(id);
                        }
                        (bridge.event)(
                            view,
                            EditorEvent::Saved {
                                id,
                                host: identity.host,
                                directory,
                            },
                            window,
                            cx,
                        );
                        if save_as.is_some() {
                            (bridge.event)(view, EditorEvent::IdentityChanged, window, cx);
                        }
                        let editor = (bridge.access)(view);
                        if let Some(target) = next_target {
                            editor.save_to(id, target, window, cx, bridge);
                        } else if pending {
                            editor.save(id, false, Path::new(""), window, cx, bridge);
                        } else {
                            let ready = editor.take_saved_intents();
                            for intent in ready {
                                (bridge.event)(view, EditorEvent::Continue(intent), window, cx);
                            }
                        }
                    }
                    SaveOutcome::Conflict(observed) => {
                        file.conflict = Some(DiskConflict::Changed(observed));
                        editor.saves_failed(id);
                        editor.ask_overwrite(id, window, cx, bridge);
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

    fn ask_overwrite<E: 'static>(
        &mut self,
        id: BufferId,
        window: &mut Window,
        cx: &mut Context<E>,
        bridge: EditorBridge<E>,
    ) {
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
            let _ = view.update_in(cx, |view, window, cx| {
                let editor = (bridge.access)(view);
                if !editor.accepts_prompt(&identity, cx) {
                    return;
                }
                if matches!(choice, Ok(0)) {
                    editor.save(id, true, Path::new(""), window, cx, bridge);
                } else {
                    (bridge.event)(view, EditorEvent::Focus, window, cx);
                }
            });
        })
        .detach();
    }

    fn offer_utf8<E: 'static>(
        &mut self,
        id: BufferId,
        unmappable: char,
        target: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<E>,
        bridge: EditorBridge<E>,
    ) {
        let Some(identity) = self.save_identity(id, cx) else {
            return;
        };
        let file = self.buffers.get(id).unwrap();
        let name = file.label();
        let encoding = file.format(cx).encoding_label();
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
            let _ = view.update_in(cx, |view, window, cx| {
                let editor = (bridge.access)(view);
                if !editor.accepts_prompt(&identity, cx) {
                    return;
                }
                if matches!(choice, Ok(0)) {
                    let file = editor.buffers.get_mut(id).unwrap();
                    file.set_format(
                        TextFormat {
                            line_ending: file.format(cx).line_ending,
                            ..TextFormat::default()
                        },
                        cx,
                    );
                    editor.save_at(id, false, target, Path::new(""), window, cx, bridge);
                } else {
                    (bridge.event)(view, EditorEvent::Focus, window, cx);
                }
            });
        })
        .detach();
    }

    pub(super) fn save_as<E: 'static>(
        &mut self,
        id: BufferId,
        root: &Path,
        window: &mut Window,
        cx: &mut Context<E>,
        bridge: EditorBridge<E>,
    ) {
        let Some(identity) = self.save_identity(id, cx) else {
            return;
        };
        let file = self.buffers.get(id).unwrap();
        let suggested = if file.untitled.is_some() {
            let extension = if file.input.markdown().is_some() {
                "md"
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
                bridge,
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
            let _ = view.update_in(cx, |view, window, cx| {
                let editor = (bridge.access)(view);
                if !editor.accepts_prompt(&identity, cx) {
                    return;
                }
                if let Ok(Ok(Some(path))) = result {
                    editor.save_to(id, path, window, cx, bridge);
                } else {
                    editor.saves_failed(id);
                    (bridge.event)(view, EditorEvent::Focus, window, cx);
                }
            });
        })
        .detach();
    }

    fn save_to<E: 'static>(
        &mut self,
        id: BufferId,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<E>,
        bridge: EditorBridge<E>,
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
        self.save_at(id, true, Some(path), Path::new(""), window, cx, bridge);
    }

    pub(super) fn open_bar<E: 'static>(
        &mut self,
        kind: BarKind,
        initial: String,
        window: &mut Window,
        cx: &mut Context<E>,
        bridge: EditorBridge<E>,
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
                    .map(|file| file.input.read(cx).text().lines_len())
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
        let sub = cx.subscribe_in(&input, window, move |view, _, event, window, cx| {
            let editor = (bridge.access)(view);
            match event {
                InputEvent::PressEnter { .. } => {
                    if let Some(event) = editor.submit_bar(window, cx, bridge) {
                        (bridge.event)(view, event, window, cx);
                    }
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
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.bar = Some(EditorBar {
            kind,
            identity,
            input,
            _sub: sub,
        });
        cx.notify();
    }

    pub(super) fn dismiss_bar(&mut self) {
        if let Some(bar) = self.bar.take()
            && let BarKind::SaveAs { id } = bar.kind
        {
            self.saves_failed(id);
        }
    }
    pub(super) fn has_bar(&self) -> bool {
        self.bar.is_some()
    }

    pub(super) fn bar_has_focus(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        self.bar.as_ref().is_some_and(|bar| {
            bar.input
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
        })
    }

    fn submit_bar<E: 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<E>,
        bridge: EditorBridge<E>,
    ) -> Option<EditorEvent> {
        let bar = self.bar.as_ref()?;
        let identity = bar.identity.clone();
        if !self.accepts_prompt(&identity, cx) {
            self.dismiss_bar();
            return Some(EditorEvent::Focus);
        }
        let bar = self.bar.as_ref()?;
        let text = bar.input.read(cx).value().to_string();
        match bar.kind {
            BarKind::GoToLine { id } => {
                let (line, column) = parse_line_target(&text)?;
                self.bar = None;
                Some(EditorEvent::GoToLine { id, line, column })
            }
            BarKind::Rename { id, offset } => {
                let name = text.trim().to_string();
                if name.is_empty() {
                    return None;
                }
                self.bar = None;
                Some(EditorEvent::Rename { id, offset, name })
            }
            BarKind::SaveAs { id } => {
                let path = PathBuf::from(text.trim());
                let identity = bar.identity.clone();
                let host = self.buffers.get(id)?.host.clone();
                if path.as_os_str().is_empty() || !host.is_absolute(&path) {
                    return None;
                }
                self.bar = None;
                let check = path.clone();
                HostOps::run_in(
                    host,
                    window,
                    cx,
                    move |host| host.exists(&check),
                    move |view, exists, window, cx| {
                        let editor = (bridge.access)(view);
                        if !editor.accepts_prompt(&identity, cx) {
                            return;
                        }
                        if !exists {
                            editor.save_to(id, path, window, cx, bridge);
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
                            let _ = view.update_in(cx, |view, window, cx| {
                                let editor = (bridge.access)(view);
                                if !editor.accepts_prompt(&identity, cx) {
                                    return;
                                }
                                if matches!(choice, Ok(0)) {
                                    editor.save_to(id, path, window, cx, bridge);
                                } else {
                                    editor.saves_failed(id);
                                    (bridge.event)(view, EditorEvent::Focus, window, cx);
                                }
                            });
                        })
                        .detach();
                    },
                );
                cx.notify();
                Some(EditorEvent::Focus)
            }
        }
    }

    pub(super) fn guard_unsaved<E: 'static>(
        &mut self,
        ids: Vec<BufferId>,
        then: AfterUnsaved,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<E>,
        bridge: EditorBridge<E>,
    ) -> bool {
        if ids.is_empty() {
            return false;
        }
        if self.unsaved_prompt_open {
            return true;
        }
        self.unsaved_prompt_open = true;
        let epoch = self.prompt_epoch;
        let identities: Vec<_> = ids.iter().filter_map(|id| self.save_identity(*id, cx)).collect();
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
            let _ = view.update_in(cx, |view, window, cx| {
                let editor = (bridge.access)(view);
                if editor.prompt_epoch != epoch {
                    return;
                }
                editor.unsaved_prompt_open = false;
                // A missing/replaced/renamed target is not consent to save or discard.
                if identities.len() != ids.len() || identities.iter().any(|identity| !editor.accepts_prompt(identity, cx)) {
                    return;
                }
                match choice {
                    Ok(0) => {
                        editor.waiters.push(SaveWaiter {
                            ids: ids.clone(),
                            then,
                        });
                        for id in ids {
                            (bridge.access)(view).save(id, false, &root, window, cx, bridge);
                        }
                    }
                    Ok(2) => {
                        (bridge.event)(view, EditorEvent::Discard(ids), window, cx);
                        (bridge.event)(view, EditorEvent::Continue(then), window, cx);
                    }
                    _ => {
                        (bridge.event)(view, EditorEvent::Focus, window, cx);
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

    pub(super) fn saves_failed(&mut self, id: BufferId) {
        self.waiters.retain(|waiter| !waiter.ids.contains(&id));
    }

    pub(super) fn forget_saved_buffer(&mut self, id: BufferId) {
        self.saves_failed(id);
        if self.bar.as_ref().is_some_and(|bar| match bar.kind {
            BarKind::GoToLine { id: target }
            | BarKind::SaveAs { id: target }
            | BarKind::Rename { id: target, .. } => target == id,
        }) {
            self.dismiss_bar();
        }
    }

    pub(super) fn render_bar(&self, cx: &gpui_kit::App) -> Option<AnyElement> {
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
