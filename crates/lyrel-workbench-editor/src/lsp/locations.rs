//! Places a language server points at — every reference to a symbol, or a
//! definition with more than one answer — handed to the shell as typed
//! events it lists in the search's Locations tab: arrowing through it
//! previews a place in the file in front, Return goes there.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui_kit::component::{Rope, RopeExt as _};
use gpui_kit::{App, AppContext as _, Context, Window};
use lsp_types::Position;

use super::convert::{self, Encoding};
use super::{Freshen, LspStore};
use crate::code_editor::{BufferId, EditorPanelEvent, EditorPanelState, TabCode};
use lyrel_ui_base::i18n::L10nKey;

/// One place, in the editor's terms: 0-based, the column counted in chars,
/// with the line it is on for the row to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub text: String,
}

/// The longest line text a row shows.
const LINE_PREVIEW: usize = 160;

/// Converts server positions to [`Found`]s. Files open in the editor are
/// measured against their buffer (`open`), the rest against the disk — so
/// this runs off the main thread. Sorted by file, then position, duplicates
/// dropped.
pub fn resolve(
    places: Vec<(PathBuf, Position)>,
    open: HashMap<PathBuf, Rope>,
    enc: Encoding,
) -> Vec<Found> {
    let mut found: Vec<Found> = measure(places, &open, enc).into_iter().flatten().collect();
    found.sort_by(|a, b| (&a.path, a.line, a.column).cmp(&(&b.path, b.line, b.column)));
    found.dedup();
    found
}

/// [`resolve`] without the sorting: one answer per place, in order, `None`
/// for a place past the end of its file.
pub fn measure(
    places: Vec<(PathBuf, Position)>,
    open: &HashMap<PathBuf, Rope>,
    enc: Encoding,
) -> Vec<Option<Found>> {
    let mut read: HashMap<PathBuf, Option<Vec<String>>> = HashMap::new();
    places
        .into_iter()
        .map(|(path, pos)| {
            let line = match open.get(&path) {
                Some(rope) if (pos.line as usize) < rope.lines_len() => {
                    rope.slice_line(pos.line as usize).to_string()
                }
                Some(_) => return None,
                None => read
                    .entry(path.clone())
                    .or_insert_with(|| {
                        std::fs::read_to_string(&path)
                            .ok()
                            .map(|t| t.lines().map(str::to_owned).collect())
                    })
                    .as_ref()
                    .and_then(|lines| lines.get(pos.line as usize).cloned())
                    .unwrap_or_default(),
            };
            let byte = convert::byte_of_column(&line, pos.character, enc);
            let column = line[..byte].chars().count() as u32;
            let text: String = line.trim().chars().take(LINE_PREVIEW).collect();
            Some(Found {
                path,
                line: pos.line,
                column,
                text,
            })
        })
        .collect()
}

impl EditorPanelState {
    /// The open local buffers' texts, for measuring places in them.
    fn lsp_open_texts(&self, cx: &App) -> HashMap<PathBuf, Rope> {
        self.local_files()
            .into_iter()
            .map(|f| (f.path, f.input.read(cx).text().clone()))
            .collect()
    }

    /// Whether the front file's server answers project-wide symbol queries —
    /// what the shell's Symbols tab asks before it offers the query box.
    pub fn lsp_project_symbols_available(&self, code: &TabCode, cx: &mut App) -> bool {
        let Some(f) = self.active_local_file(code) else {
            return false;
        };
        let Some(doc) = LspStore::context(&f.path, f.id, Freshen::Skip, None, cx) else {
            return false;
        };
        doc.caps.workspace_symbol_provider.is_some()
    }

    /// Shows what a server found: one place is gone to at once, several
    /// are listed for the shell to pick from.
    pub fn lsp_show_places(
        &mut self,
        heading: L10nKey,
        places: Vec<(PathBuf, Position)>,
        encoding: Encoding,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if places.is_empty() {
            return;
        }
        let open = self.lsp_open_texts(cx);
        cx.spawn_in(window, async move |editor, cx| {
            let found = cx
                .background_spawn(async move { resolve(places, open, encoding) })
                .await;
            let _ = editor.update(cx, |editor, cx| match found.as_slice() {
                [] => {}
                [one] => {
                    editor.open_location_at(one.path.clone(), one.line + 1, one.column + 1, cx)
                }
                many => cx.emit(EditorPanelEvent::ShowLocations {
                    heading,
                    root,
                    locations: many.to_vec(),
                }),
            });
        })
        .detach();
    }

    /// ⇧F12: every reference to the symbol under the cursor.
    pub fn lsp_find_references(
        &mut self,
        code: &TabCode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.active_local_file(code) else {
            return;
        };
        let (text, offset) = {
            let state = f.input.read(cx);
            (state.text().clone(), state.cursor())
        };
        let Some(doc) = LspStore::context(
            &f.path,
            f.id,
            Freshen::Text(&text),
            Some(window.window_handle()),
            cx,
        ) else {
            return;
        };
        if doc.caps.references_provider.is_none() {
            return;
        }
        let request =
            doc.client
                .request::<lsp_types::request::References>(lsp_types::ReferenceParams {
                    text_document_position: lsp_types::TextDocumentPositionParams::new(
                        lsp_types::TextDocumentIdentifier::new(doc.uri.clone()),
                        convert::offset_to_lsp(&text, offset, doc.encoding),
                    ),
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                    context: lsp_types::ReferenceContext {
                        include_declaration: true,
                    },
                });
        let (encoding, root) = (doc.encoding, doc.root.clone());
        cx.spawn_in(window, async move |editor, cx| {
            let locations = match request.await {
                Ok(locations) => locations.unwrap_or_default(),
                Err(e) => {
                    log::debug!("lsp: references: {e:#}");
                    return;
                }
            };
            let places = locations
                .into_iter()
                .filter_map(|l| Some((super::uri_to_path(&l.uri)?, l.range.start)))
                .collect();
            let _ = editor.update_in(cx, |editor, window, cx| {
                editor.lsp_show_places(
                    L10nKey::SearchHeadingReferences,
                    places,
                    encoding,
                    root,
                    window,
                    cx,
                )
            });
        })
        .detach();
    }

    /// What the Symbols tab asks as its query changes: the front file's
    /// language server, for symbols anywhere in the project. The answered
    /// hits go to the shell as [`EditorPanelEvent::ShowWorkspaceSymbols`];
    /// `None`-worthy states (no front file, no server) are the shell's
    /// question, asked through [`Self::lsp_project_symbols_available`].
    pub fn lsp_workspace_query(
        &mut self,
        path: &Path,
        requester: BufferId,
        query: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use std::sync::atomic::{AtomicU64, Ordering};
        // The latest query wins: an answer to an older one is dropped.
        static LATEST: AtomicU64 = AtomicU64::new(0);
        let seq = LATEST.fetch_add(1, Ordering::Relaxed) + 1;
        if query.trim().is_empty() {
            cx.emit(EditorPanelEvent::ShowWorkspaceSymbols {
                root: PathBuf::new(),
                hits: Vec::new(),
            });
            return;
        }
        let Some(doc) = LspStore::context(path, requester, Freshen::Skip, None, cx) else {
            return;
        };
        let client = doc.client.clone();
        let (encoding, root) = (doc.encoding, doc.root.clone());
        let open = self.lsp_open_texts(cx);
        let front = path.to_path_buf();
        cx.spawn_in(window, async move |editor, cx| {
            // Typing fast asks once, for what was typed last.
            cx.background_executor()
                .timer(std::time::Duration::from_millis(150))
                .await;
            if LATEST.load(Ordering::Relaxed) != seq {
                return;
            }
            let answer = client
                .request::<lsp_types::request::WorkspaceSymbolRequest>(
                    lsp_types::WorkspaceSymbolParams {
                        query,
                        work_done_progress_params: Default::default(),
                        partial_result_params: Default::default(),
                    },
                )
                .await;
            // The front file's own symbols are listed above these already.
            let symbols = match answer {
                Ok(Some(symbols)) => workspace_symbols(symbols)
                    .into_iter()
                    .filter(|s| s.path != front)
                    .collect::<Vec<_>>(),
                Ok(None) => Vec::new(),
                Err(e) => {
                    log::debug!("lsp: workspace/symbol: {e:#}");
                    return;
                }
            };
            let hits = cx
                .background_spawn(async move {
                    let places = symbols
                        .iter()
                        .map(|s| (s.path.clone(), s.position))
                        .collect();
                    measure(places, &open, encoding)
                        .into_iter()
                        .zip(symbols)
                        .filter_map(|(found, symbol)| Some((found?, symbol)))
                        .collect::<Vec<_>>()
                })
                .await;
            if LATEST.load(Ordering::Relaxed) != seq {
                return;
            }
            let _ = editor.update(cx, |_, cx| {
                cx.emit(EditorPanelEvent::ShowWorkspaceSymbols { root, hits });
            });
        })
        .detach();
    }
}

/// The most project symbols (LSP `workspace/symbol`) listed for one query.
const MAX_WORKSPACE_SYMBOLS: usize = 200;

/// One project symbol, before its column is measured. "Workspace" is the
/// protocol's word for the server's project root — not a lyrel workspace.
pub struct WorkspaceHit {
    pub name: String,
    pub kind: lsp_types::SymbolKind,
    pub container: Option<String>,
    pub path: PathBuf,
    pub position: Position,
}

/// A `workspace/symbol` answer in either of its shapes. A symbol known only
/// by its file (no range yet) lands on the file's first line.
#[allow(deprecated)]
pub fn workspace_symbols(answer: lsp_types::WorkspaceSymbolResponse) -> Vec<WorkspaceHit> {
    use lsp_types::{OneOf, WorkspaceSymbolResponse};
    let hits: Vec<WorkspaceHit> = match answer {
        WorkspaceSymbolResponse::Flat(symbols) => symbols
            .into_iter()
            .filter_map(|s| {
                Some(WorkspaceHit {
                    path: super::uri_to_path(&s.location.uri)?,
                    position: s.location.range.start,
                    name: s.name,
                    kind: s.kind,
                    container: s.container_name,
                })
            })
            .collect(),
        WorkspaceSymbolResponse::Nested(symbols) => symbols
            .into_iter()
            .filter_map(|s| {
                let (uri, position) = match s.location {
                    OneOf::Left(l) => (l.uri, l.range.start),
                    OneOf::Right(l) => (l.uri, Position::default()),
                };
                Some(WorkspaceHit {
                    path: super::uri_to_path(&uri)?,
                    position,
                    name: s.name,
                    kind: s.kind,
                    container: s.container_name,
                })
            })
            .collect(),
    };
    hits.into_iter().take(MAX_WORKSPACE_SYMBOLS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_are_measured_against_open_buffers_and_the_disk() {
        let dir = tempfile::TempDir::new().unwrap();
        let on_disk = dir.path().join("b.rs");
        std::fs::write(&on_disk, "fn b() {\n    let 🎉 = a();\n}\n").unwrap();
        let open_path = dir.path().join("a.rs");
        let open = HashMap::from([(open_path.clone(), Rope::from("pub fn é_a() {}\n"))]);
        let found = resolve(
            vec![
                // `a` after the emoji: UTF-16 column 13, char column 12.
                (on_disk.clone(), Position::new(1, 13)),
                (open_path.clone(), Position::new(0, 7)),
                (open_path.clone(), Position::new(0, 7)),
                (open_path.clone(), Position::new(9, 0)),
            ],
            open,
            Encoding::Utf16,
        );
        assert_eq!(
            found,
            [
                Found {
                    path: open_path,
                    line: 0,
                    column: 7,
                    text: "pub fn é_a() {}".into(),
                },
                Found {
                    path: on_disk,
                    line: 1,
                    column: 12,
                    text: "let 🎉 = a();".into(),
                },
            ]
        );
    }
}
