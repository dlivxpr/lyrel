//! gpui-component's editor hooks, answered by a language server.
//!
//! One [`LspProvider`] per buffer, installed into the buffer's
//! `EditorState::lsp_mut()` while the buffer has a server and taken out when it
//! stops having one. Every request carries positions through
//! [`super::convert`]: gpui-component counts columns in chars, a server in
//! UTF-16 units.
//!
//! The completion, hover and definition hooks run inside the buffer's own
//! update, so they must not read the buffer: the text they need is handed in.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::Result;
use gpui_kit::component::input::{
    CodeActionProvider, CompletionProvider, DefinitionProvider, EditorState, HoverProvider,
};
use gpui_kit::component::{Rope, RopeExt as _};
use gpui_kit::{App, AppContext as _, Entity, SharedString, Task, WeakEntity, Window};
use lsp_types::{
    CodeAction, CodeActionOrCommand, CompletionContext, CompletionResponse, CompletionTextEdit,
    CompletionTriggerKind, InsertTextFormat, LocationLink,
};

use super::convert::{self, Encoding};
use super::{DocContext, Freshen, LspStore};
use crate::ui::app::LyrelApp;

/// Bound the visible menu only after matching and server ordering, so a useful
/// candidate beyond the first page of an unfiltered response is not discarded.
const MAX_COMPLETIONS: usize = 300;

pub(crate) struct LspProvider {
    /// Every open buffer's text when the code-action menu was last filled:
    /// an action picked from it is applied only if what it edits is
    /// unchanged since.
    actions_baseline: std::cell::RefCell<Option<std::collections::HashMap<PathBuf, Rope>>>,
    path: PathBuf,
    /// The buffer this provider serves — always the document's owner.
    input: gpui_kit::EntityId,
    app: WeakEntity<LyrelApp>,
}

pub(super) fn install(
    input: &Entity<EditorState>,
    popups: &Entity<super::popups::EditorPopups>,
    path: &Path,
    app: WeakEntity<LyrelApp>,
    cx: &mut App,
) {
    let provider = Rc::new(LspProvider {
        actions_baseline: Default::default(),
        path: path.to_path_buf(),
        input: input.entity_id(),
        app,
    });
    popups.update(cx, |popups, cx| {
        popups.set_provider(Some(provider.clone()), cx)
    });
    input.update(cx, |state, cx| {
        let lsp = state.lsp_mut();
        // Resolution, import edits and completion commands are app-owned.
        lsp.completion_provider = None;
        lsp.hover_provider = Some(provider.clone());
        lsp.definition_provider = Some(provider.clone());
        let definition = provider.clone();
        lsp.show_document = Some(Rc::new(move |params, window, cx| {
            definition.open_document(params, window, cx)
        }));
        lsp.code_action_providers = vec![provider];
        cx.notify();
    });
}

pub(super) fn uninstall(input: &Entity<EditorState>, cx: &mut App) {
    input.update(cx, |state, cx| {
        let lsp = state.lsp_mut();
        lsp.completion_provider = None;
        lsp.hover_provider = None;
        lsp.definition_provider = None;
        lsp.show_document = None;
        lsp.code_action_providers.clear();
        cx.notify();
    });
}

impl LspProvider {
    fn context(&self, sync: Freshen<'_>, window: &Window, cx: &mut App) -> Option<DocContext> {
        LspStore::context(
            &self.path,
            self.input,
            sync,
            Some(window.window_handle()),
            cx,
        )
    }
    pub(super) fn resolve_completion(
        &self,
        item: lsp_types::CompletionItem,
        text: &Rope,
        cx: &mut App,
    ) -> Task<Result<lsp_types::CompletionItem>> {
        // Resolving asks nothing of the text, which the caller may be in the
        // middle of changing: nothing is sent first.
        let Some(doc) = LspStore::context(&self.path, self.input, Freshen::Skip, None, cx) else {
            return Task::ready(Err(anyhow::anyhow!(
                "language server document is no longer available"
            )));
        };
        let resolvable = doc
            .caps
            .completion_provider
            .as_ref()
            .is_some_and(|c| c.resolve_provider == Some(true));
        let converted = completion_to_editor(item.clone(), text, doc.encoding);
        if !resolvable {
            return Task::ready(Ok(converted));
        }
        let request = doc
            .client
            .request::<lsp_types::request::ResolveCompletionItem>(item.clone());
        let text = text.clone();
        let encoding = doc.encoding;
        cx.background_spawn(async move {
            Ok(match request.await {
                Ok(resolved) => merge_resolved(converted, resolved, &text, encoding),
                Err(e) => {
                    log::debug!("lsp: completionItem/resolve: {e:#}");
                    converted
                }
            })
        })
    }

    pub(super) fn execute_completion_command(
        &self,
        command: lsp_types::Command,
        text: &Rope,
        cx: &mut App,
    ) -> Task<Result<()>> {
        let Some(doc) = LspStore::context(&self.path, self.input, Freshen::Text(text), None, cx)
        else {
            return Task::ready(Err(anyhow::anyhow!(
                "language server document is no longer available"
            )));
        };
        let request = doc.client.request::<lsp_types::request::ExecuteCommand>(
            lsp_types::ExecuteCommandParams {
                command: command.command,
                arguments: command.arguments.unwrap_or_default(),
                work_done_progress_params: Default::default(),
            },
        );
        cx.background_spawn(async move { request.await.map(|_| ()) })
    }
}

fn not_available<T: Default + Send + 'static>() -> Task<Result<T>> {
    Task::ready(Ok(T::default()))
}

impl CompletionProvider for LspProvider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let Some(doc) = self.context(Freshen::Text(text), window, cx) else {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        };
        if doc.caps.completion_provider.is_none() {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        }
        // Read the preceding scalar at the byte cursor, independently of the
        // server's negotiated position encoding.
        let before = text.chars_at(offset).reversed().next();
        let triggers = LspStore::completion_triggers(&self.path, cx);
        let context = match before.map(String::from) {
            Some(c)
                if trigger.trigger_kind == CompletionTriggerKind::TRIGGER_CHARACTER
                    && triggers.contains(&c) =>
            {
                CompletionContext {
                    trigger_kind: CompletionTriggerKind::TRIGGER_CHARACTER,
                    trigger_character: Some(c),
                }
            }
            _ => CompletionContext {
                trigger_kind: CompletionTriggerKind::INVOKED,
                trigger_character: None,
            },
        };
        let params = lsp_types::CompletionParams {
            text_document_position: lsp_types::TextDocumentPositionParams::new(
                lsp_types::TextDocumentIdentifier::new(doc.uri.clone()),
                convert::offset_to_lsp(text, offset, doc.encoding),
            ),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: Some(context),
        };
        let request = doc.client.request::<lsp_types::request::Completion>(params);
        let text = text.clone();
        let encoding = doc.encoding;
        cx.background_spawn(async move {
            let items = match request.await? {
                Some(CompletionResponse::Array(items)) => items,
                Some(CompletionResponse::List(list)) => list.items,
                None => Vec::new(),
            };
            let items = matching_completions(items, &text, offset, encoding);
            Ok(CompletionResponse::Array(items))
        })
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, cx: &mut App) -> bool {
        let Some(last) = new_text.chars().last() else {
            return false;
        };
        if last.is_alphanumeric() || last == '_' {
            return true;
        }
        LspStore::completion_triggers(&self.path, cx).contains(&last.to_string())
    }
}

/// Keep protocol items untouched for resolve; only inspect their ranges here.
/// Each item's edit start defines its filter prefix, falling back to the word
/// at the actual request cursor when the server does not provide an edit.
fn matching_completions(
    mut items: Vec<lsp_types::CompletionItem>,
    text: &Rope,
    cursor: usize,
    encoding: Encoding,
) -> Vec<lsp_types::CompletionItem> {
    let fallback = completion_prefix_start(text, cursor);
    let point = text.offset_to_point(cursor);
    let line = text.slice_line(point.row).to_string();
    let line_start = cursor - point.column;
    let position = lsp_types::Position::new(
        point.row as u32,
        convert::column_of_byte(&line, point.column, encoding),
    );
    items.retain(|item| {
        let range = match &item.text_edit {
            Some(CompletionTextEdit::Edit(edit)) => Some(edit.range),
            Some(CompletionTextEdit::InsertAndReplace(edit)) => Some(edit.insert),
            None => None,
        };
        let start = range
            .filter(|range| {
                range.start.line == position.line
                    && range.end.line == position.line
                    && range.start <= position
                    && position <= range.end
            })
            .map(|range| {
                line_start + convert::byte_of_column(&line, range.start.character, encoding)
            })
            .unwrap_or(fallback);
        let mut candidate = item
            .filter_text
            .as_ref()
            .unwrap_or(&item.label)
            .chars()
            .flat_map(char::to_lowercase);
        // Match a case-insensitive subsequence, preserving server fuzzy and
        // camelCase candidates without allocating a string per candidate.
        text.chars_at(start)
            .scan(start, |at, ch| {
                if *at >= cursor {
                    return None;
                }
                *at += ch.len_utf8();
                Some(ch)
            })
            .flat_map(char::to_lowercase)
            .all(|ch| candidate.by_ref().any(|next| next == ch))
    });
    items.sort_by(|a, b| {
        a.sort_text
            .as_ref()
            .unwrap_or(&a.label)
            .cmp(b.sort_text.as_ref().unwrap_or(&b.label))
    });
    items.truncate(MAX_COMPLETIONS);
    items
}

pub(super) fn completion_prefix_start(text: &Rope, cursor: usize) -> usize {
    cursor
        - text
            .chars_at(cursor)
            .reversed()
            .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
            .map(char::len_utf8)
            .sum::<usize>()
}

/// A completion item as the editor's menu can insert it: ranges in editor
/// columns, snippets flattened to plain text.
pub(crate) fn completion_to_editor(
    mut item: lsp_types::CompletionItem,
    text: &Rope,
    encoding: Encoding,
) -> lsp_types::CompletionItem {
    let snippet = item.insert_text_format == Some(InsertTextFormat::SNIPPET);
    if snippet {
        item.insert_text = item.insert_text.map(|t| convert::snippet_to_plain(&t));
        item.insert_text_format = Some(InsertTextFormat::PLAIN_TEXT);
    }
    item.text_edit = item.text_edit.map(|edit| match edit {
        CompletionTextEdit::Edit(mut e) => {
            e.range = convert::range_to_editor(text, e.range, encoding);
            if snippet {
                e.new_text = convert::snippet_to_plain(&e.new_text);
            }
            CompletionTextEdit::Edit(e)
        }
        // Declined in the client capabilities, but a server may send one
        // anyway: replacing is what accepting a completion does here.
        CompletionTextEdit::InsertAndReplace(e) => CompletionTextEdit::Edit(lsp_types::TextEdit {
            range: convert::range_to_editor(text, e.replace, encoding),
            new_text: if snippet {
                convert::snippet_to_plain(&e.new_text)
            } else {
                e.new_text
            },
        }),
    });
    item.additional_text_edits = item
        .additional_text_edits
        .map(|edits| additional_edits_to_editor(edits, text, encoding));
    item
}

fn additional_edits_to_editor(
    edits: Vec<lsp_types::TextEdit>,
    text: &Rope,
    encoding: Encoding,
) -> Vec<lsp_types::TextEdit> {
    edits
        .into_iter()
        .map(|e| lsp_types::TextEdit {
            range: convert::range_to_editor(text, e.range, encoding),
            new_text: e.new_text,
        })
        .collect()
}

/// Adds resolution-only fields to an item already converted to editor columns.
///
/// The server receives the original protocol item. Keep the converted primary
/// edit rather than converting its echoed range again; only newly resolved
/// additional edits still need protocol-to-editor conversion. Callers cache
/// this result and must not pass it back through `resolve_completion`.
pub(crate) fn merge_resolved(
    mut item: lsp_types::CompletionItem,
    resolved: lsp_types::CompletionItem,
    text: &Rope,
    encoding: Encoding,
) -> lsp_types::CompletionItem {
    item.documentation = resolved.documentation.or(item.documentation);
    item.detail = resolved.detail.or(item.detail);
    item.command = resolved.command.or(item.command);
    item.additional_text_edits = Some(
        resolved
            .additional_text_edits
            .map(|edits| additional_edits_to_editor(edits, text, encoding))
            .or(item.additional_text_edits)
            .unwrap_or_default(),
    );
    item
}

impl HoverProvider for LspProvider {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<lsp_types::Hover>>> {
        let Some(doc) = self.context(Freshen::Text(text), window, cx) else {
            return not_available();
        };
        if doc.caps.hover_provider.is_none() {
            return not_available();
        }
        let params = lsp_types::HoverParams {
            text_document_position_params: lsp_types::TextDocumentPositionParams::new(
                lsp_types::TextDocumentIdentifier::new(doc.uri.clone()),
                convert::offset_to_lsp(text, offset, doc.encoding),
            ),
            work_done_progress_params: Default::default(),
        };
        let request = doc
            .client
            .request::<lsp_types::request::HoverRequest>(params);
        let text = text.clone();
        let encoding = doc.encoding;
        cx.background_spawn(async move {
            let Some(mut hover) = request.await? else {
                return Ok(None);
            };
            if hover_is_empty(&hover.contents) {
                return Ok(None);
            }
            hover.range = hover
                .range
                .map(|r| convert::range_to_editor(&text, r, encoding));
            hover.contents = fence_language_strings(hover.contents);
            Ok(Some(hover))
        })
    }
}

fn hover_is_empty(contents: &lsp_types::HoverContents) -> bool {
    use lsp_types::{HoverContents, MarkedString};
    let blank = |m: &MarkedString| match m {
        MarkedString::String(s) => s.trim().is_empty(),
        MarkedString::LanguageString(l) => l.value.trim().is_empty(),
    };
    match contents {
        HoverContents::Scalar(m) => blank(m),
        HoverContents::Array(a) => a.iter().all(blank),
        HoverContents::Markup(m) => m.value.trim().is_empty(),
    }
}

/// The popover renders Markdown and shows a bare `{ language, value }` pair as
/// plain text; as a fenced block it gets its code styling.
fn fence_language_strings(contents: lsp_types::HoverContents) -> lsp_types::HoverContents {
    use lsp_types::{HoverContents, MarkedString};
    let fence = |m: MarkedString| match m {
        MarkedString::LanguageString(l) => {
            MarkedString::String(format!("```{}\n{}\n```", l.language, l.value))
        }
        other => other,
    };
    match contents {
        HoverContents::Scalar(m) => HoverContents::Scalar(fence(m)),
        HoverContents::Array(a) => HoverContents::Array(a.into_iter().map(fence).collect()),
        markup => markup,
    }
}

impl DefinitionProvider for LspProvider {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>> {
        let Some(doc) = self.context(Freshen::Text(text), window, cx) else {
            return not_available();
        };
        let request = definition_request(&doc, text, offset);
        let text = text.clone();
        let encoding = doc.encoding;
        cx.background_spawn(async move {
            let mut links = request.await?;
            // Only the origin is in this buffer; the target is converted when
            // it is followed, against the file it is in.
            for link in &mut links {
                link.origin_selection_range = link
                    .origin_selection_range
                    .map(|r| convert::range_to_editor(&text, r, encoding));
            }
            Ok(links)
        })
    }
}

impl LspProvider {
    fn open_document(
        &self,
        params: &lsp_types::ShowDocumentParams,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let Some(path) = super::uri_to_path(&params.uri) else {
            // An `http:` target and the like: gpui-component opens it.
            return false;
        };
        let position = params
            .selection
            .map(|range| range.start)
            .unwrap_or_default();
        let encoding = LspStore::context(&self.path, self.input, Freshen::Skip, None, cx)
            .map(|d| d.encoding)
            .unwrap_or_default();
        let app = self.app.clone();
        // Deferred: this runs inside the buffer's update, and opening a file
        // — even this same one — updates buffers.
        window.defer(cx, move |window, cx| {
            let _ = app.update(cx, |app, cx| {
                app.lsp_open_location(path, position, encoding, window, cx)
            });
        });
        true
    }
}

/// `textDocument/definition` at `offset`, answered as location links
/// whatever shape the server replied in.
pub(crate) fn definition_request(
    doc: &DocContext,
    text: &Rope,
    offset: usize,
) -> impl std::future::Future<Output = Result<Vec<LocationLink>>> + use<> {
    let params = lsp_types::GotoDefinitionParams {
        text_document_position_params: lsp_types::TextDocumentPositionParams::new(
            lsp_types::TextDocumentIdentifier::new(doc.uri.clone()),
            convert::offset_to_lsp(text, offset, doc.encoding),
        ),
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    let supported = doc.caps.definition_provider.is_some();
    let request = doc
        .client
        .request::<lsp_types::request::GotoDefinition>(params);
    async move {
        if !supported {
            return Ok(Vec::new());
        }
        use lsp_types::GotoDefinitionResponse as R;
        let to_link = |l: lsp_types::Location| LocationLink {
            origin_selection_range: None,
            target_uri: l.uri,
            target_range: l.range,
            target_selection_range: l.range,
        };
        Ok(match request.await? {
            Some(R::Scalar(l)) => vec![to_link(l)],
            Some(R::Array(ls)) => ls.into_iter().map(to_link).collect(),
            Some(R::Link(links)) => links,
            None => Vec::new(),
        })
    }
}

impl CodeActionProvider for LspProvider {
    fn id(&self) -> SharedString {
        "lsp".into()
    }

    fn code_actions(
        &self,
        state: Entity<EditorState>,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<CodeAction>>> {
        let text = state.read(cx).text().clone();
        let Some(doc) = self.context(Freshen::Text(&text), window, cx) else {
            return not_available();
        };
        if doc.caps.code_action_provider.is_none() {
            return not_available();
        }
        *self.actions_baseline.borrow_mut() = Some(LspStore::snapshot(cx));
        let lsp_range = convert::offsets_to_lsp_range(&text, range, doc.encoding);
        // The diagnostics the range touches, which is what turns a server's
        // quick fixes on.
        let diagnostics = doc
            .diagnostics
            .iter()
            .filter(|d| {
                d.range.start.line <= lsp_range.end.line && d.range.end.line >= lsp_range.start.line
            })
            .cloned()
            .collect();
        let params = lsp_types::CodeActionParams {
            text_document: lsp_types::TextDocumentIdentifier::new(doc.uri.clone()),
            range: lsp_range,
            context: lsp_types::CodeActionContext {
                diagnostics,
                only: None,
                trigger_kind: Some(lsp_types::CodeActionTriggerKind::INVOKED),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let request = doc
            .client
            .request::<lsp_types::request::CodeActionRequest>(params);
        cx.background_spawn(async move {
            Ok(request
                .await?
                .unwrap_or_default()
                .into_iter()
                .map(|a| match a {
                    CodeActionOrCommand::CodeAction(a) => a,
                    CodeActionOrCommand::Command(command) => CodeAction {
                        title: command.title.clone(),
                        command: Some(command),
                        ..Default::default()
                    },
                })
                .filter(|a| a.disabled.is_none())
                .collect())
        })
    }

    fn perform_code_action(
        &self,
        _state: Entity<EditorState>,
        action: CodeAction,
        _push_to_history: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        // Inside the buffer's update: it cannot be read, and the actions on
        // offer were computed against text that was already sent.
        let Some(doc) = self.context(Freshen::Skip, window, cx) else {
            return Task::ready(Ok(()));
        };
        let resolvable = matches!(
            &doc.caps.code_action_provider,
            Some(lsp_types::CodeActionProviderCapability::Options(o))
                if o.resolve_provider == Some(true)
        );
        // Taken when the menu was filled, which is also when an action that
        // came with its edit was computed. Without one there is nothing to
        // vouch for the open buffers, and an edit to any of them is refused.
        let baseline = self
            .actions_baseline
            .borrow_mut()
            .take()
            .unwrap_or_default();
        let client = doc.client.clone();
        let encoding = doc.encoding;
        window.spawn(cx, async move |cx| {
            let action = if action.edit.is_none() && action.data.is_some() && resolvable {
                client
                    .request::<lsp_types::request::CodeActionResolveRequest>(action.clone())
                    .await
                    .unwrap_or(action)
            } else {
                action
            };
            if let Some(edit) = action.edit {
                let applied = cx.update(|_, cx| {
                    super::apply_workspace_edit(edit, encoding, Some(baseline), cx)
                })?;
                if !applied {
                    // Its command would act on the same stale ground.
                    return Ok(());
                }
            }
            if let Some(command) = action.command {
                // Whatever this changes comes back as `workspace/applyEdit`.
                client
                    .request::<lsp_types::request::ExecuteCommand>(
                        lsp_types::ExecuteCommandParams {
                            command: command.command,
                            arguments: command.arguments.unwrap_or_default(),
                            work_done_progress_params: Default::default(),
                        },
                    )
                    .await?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{CompletionItem, Position, Range as LspRange, TextEdit};

    #[test]
    fn completion_matching_precedes_limit_and_uses_filter_text_and_server_order() {
        let text = Rope::from("let message = \"中文🎉\"; value.len");
        let mut items = (0..MAX_COMPLETIONS + 10)
            .map(|i| CompletionItem {
                label: format!("split_{i}"),
                sort_text: Some(format!("{i:04}")),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        items.extend([
            CompletionItem {
                label: "length_with_docs()".into(),
                filter_text: Some("len".into()),
                sort_text: Some("2".into()),
                ..Default::default()
            },
            CompletionItem {
                label: "len()".into(),
                filter_text: Some("len".into()),
                sort_text: Some("1".into()),
                ..Default::default()
            },
            CompletionItem {
                label: "len_but_server_says_not_to_match".into(),
                filter_text: Some("split_off".into()),
                ..Default::default()
            },
        ]);
        let items = matching_completions(items, &text, text.len(), Encoding::Utf16);
        assert_eq!(
            items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["len()", "length_with_docs()"]
        );
    }

    #[test]
    fn completion_matching_uses_unicode_protocol_edit_range_without_converting_item() {
        let text = Rope::from("中🎉 receiver.长度");
        let range = LspRange::new(Position::new(0, 4), Position::new(0, 15));
        let item = CompletionItem {
            label: "Length".into(),
            filter_text: Some("receiver.长度".into()),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: "receiver.长度()".into(),
            })),
            ..Default::default()
        };
        let items = matching_completions(vec![item.clone()], &text, text.len(), Encoding::Utf16);
        assert_eq!(items, vec![item]);
        assert_eq!(
            completion_prefix_start(&text, text.len()),
            "中🎉 receiver.".len()
        );
        let converted = completion_to_editor(items[0].clone(), &text, Encoding::Utf16);
        let Some(CompletionTextEdit::Edit(edit)) = converted.text_edit else {
            panic!("expected primary edit");
        };
        assert_eq!(
            edit.range,
            LspRange::new(Position::new(0, 3), Position::new(0, 14))
        );
    }

    #[test]
    fn empty_completion_prefix_preserves_candidates_and_label_fallback_is_case_insensitive() {
        let items = vec![
            CompletionItem {
                label: "Length".into(),
                ..Default::default()
            },
            CompletionItem {
                label: "split_off".into(),
                ..Default::default()
            },
        ];
        let empty = Rope::from("中🎉 value.");
        assert_eq!(
            matching_completions(items.clone(), &empty, empty.len(), Encoding::Utf16).len(),
            2
        );
        let prefix = Rope::from("中🎉 value.le");
        let filtered = matching_completions(items, &prefix, prefix.len(), Encoding::Utf16);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].label, "Length");
    }

    #[test]
    fn fuzzy_server_completions_survive_client_refinement() {
        let text = Rope::from("fb");
        let items = ["foo_bar", "FooBar"]
            .into_iter()
            .map(|label| CompletionItem {
                label: label.into(),
                filter_text: Some(label.into()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: LspRange::new(Position::new(0, 0), Position::new(0, 2)),
                    new_text: label.into(),
                })),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        let matched = matching_completions(items.clone(), &text, text.len(), Encoding::Utf16);
        for item in items {
            assert!(matched.contains(&item));
        }
    }

    #[test]
    fn a_snippet_completion_arrives_as_plain_text_in_editor_columns() {
        let text = Rope::from("let 🎉 = fo\n");
        let item = CompletionItem {
            label: "foo".into(),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                // UTF-16: `let ` 4, emoji 2, ` = ` 3 → `fo` at 9..11.
                range: LspRange::new(Position::new(0, 9), Position::new(0, 11)),
                new_text: "foo(${1:x})$0".into(),
            })),
            ..Default::default()
        };
        let item = completion_to_editor(item, &text, Encoding::Utf16);
        let Some(CompletionTextEdit::Edit(edit)) = item.text_edit else {
            panic!("expected a plain edit");
        };
        assert_eq!(edit.new_text, "foo(x)");
        assert_eq!(
            edit.range,
            LspRange::new(Position::new(0, 8), Position::new(0, 10))
        );
        assert_eq!(item.insert_text_format, Some(InsertTextFormat::PLAIN_TEXT));
    }

    #[test]
    fn resolving_adds_imports_in_editor_columns_and_keeps_the_edit() {
        let text = Rope::from("use é;\nfoo\n");
        let item = CompletionItem {
            label: "foo".into(),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: LspRange::new(Position::new(1, 0), Position::new(1, 3)),
                new_text: "foo".into(),
            })),
            ..Default::default()
        };
        let resolved = CompletionItem {
            label: "foo".into(),
            documentation: Some(lsp_types::Documentation::String("Does foo.".into())),
            // Echoed back as sent; must not be converted a second time.
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: LspRange::new(Position::new(9, 9), Position::new(9, 9)),
                new_text: "wrong".into(),
            })),
            additional_text_edits: Some(vec![TextEdit {
                // After `use é;` — UTF-16 column 6 is char column 6 here.
                range: LspRange::new(Position::new(0, 6), Position::new(0, 6)),
                new_text: "\nuse bar::foo;".into(),
            }]),
            ..Default::default()
        };
        let merged = merge_resolved(item.clone(), resolved, &text, Encoding::Utf16);
        assert_eq!(merged.text_edit, item.text_edit);
        assert!(merged.documentation.is_some());
        assert_eq!(
            merged.additional_text_edits.unwrap()[0].new_text,
            "\nuse bar::foo;"
        );
        // Nothing to add: said so, so accepting does not ask again.
        let bare = merge_resolved(item.clone(), item, &text, Encoding::Utf16);
        assert_eq!(bare.additional_text_edits, Some(vec![]));
    }

    #[test]
    fn language_strings_in_a_hover_become_fenced_code() {
        let contents = lsp_types::HoverContents::Scalar(lsp_types::MarkedString::LanguageString(
            lsp_types::LanguageString {
                language: "rust".into(),
                value: "fn main()".into(),
            },
        ));
        let lsp_types::HoverContents::Scalar(lsp_types::MarkedString::String(s)) =
            fence_language_strings(contents)
        else {
            panic!("expected a markdown string");
        };
        assert_eq!(s, "```rust\nfn main()\n```");
        assert!(hover_is_empty(&lsp_types::HoverContents::Array(vec![])));
    }
}
