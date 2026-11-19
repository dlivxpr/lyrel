//! Editor session value types: which files each tab had open, kept across
//! restarts. Owned here (with the workbench editor that produces them); the
//! root package's `ui::editor_session` keeps the persistent store that reads
//! and writes them.

/// One tab's editor, as it was last seen. `files` and `active` are the left
/// group's — the only group, unless the editor was split.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TabEditor {
    pub files: Vec<std::path::PathBuf>,
    #[serde(default)]
    pub active: usize,
    #[serde(default)]
    pub visible: bool,
    /// The right group of a split editor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<SplitEditor>,
}

/// The right-hand group of a split editor.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SplitEditor {
    pub files: Vec<std::path::PathBuf>,
    #[serde(default)]
    pub active: usize,
    /// Whether this group, not the left one, had the focus.
    #[serde(default)]
    pub focused: bool,
}

// The persistent store: one per process, shared by every window, so two
// windows writing their own tabs never race each other over the file. Kept
// here with the value types it reads and writes.

use gpui_kit::{App, Global};
use lyrel_core::core::machine::TabId;
use std::collections::HashMap;

const FILE: &str = "editor-sessions.json";

/// The most tabs remembered. A tab closed in some other client never tells
/// this one, so its record can only age out.
const MAX_TABS: usize = 256;

const WRITE_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

/// A stored tab record: its editor state plus when it last changed, for
/// choosing what to forget first.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct Entry {
    #[serde(flatten)]
    state: TabEditor,
    /// When this entry last changed, for choosing what to forget first.
    #[serde(default)]
    touched: u64,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct Doc {
    #[serde(default)]
    tabs: HashMap<TabId, Entry>,
}

#[derive(Default)]
struct EditorSessionStore {
    doc: Doc,
    write_scheduled: bool,
}

impl Global for EditorSessionStore {}

fn test_guarded() -> bool {
    cfg!(any(test, feature = "test-support"))
}

fn store(cx: &mut App) -> &mut EditorSessionStore {
    if cx.try_global::<EditorSessionStore>().is_none() {
        cx.set_global(EditorSessionStore {
            doc: load(),
            write_scheduled: false,
        });
    }
    cx.global_mut::<EditorSessionStore>()
}

fn load() -> Doc {
    // A test builds windows like any other run, and must not read — let alone
    // rewrite — the editor state of the person running the suite.
    if test_guarded() {
        return Doc::default();
    }
    let Some(path) = lyrel_core::core::config::config_path(FILE) else {
        return Doc::default();
    };
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            log::warn!(
                "editor sessions: ignoring unreadable {}: {e}",
                path.display()
            );
            Doc::default()
        }),
        Err(_) => Doc::default(),
    }
}

/// What the tab had open, if anything was recorded for it.
pub fn get(cx: &mut App, tab: TabId) -> Option<TabEditor> {
    store(cx).doc.tabs.get(&tab).map(|e| e.state.clone())
}

/// Records a tab's editor. Writing is deferred and coalesced: this is called
/// whenever the set of open files changes, which a burst of opens makes often.
pub fn put(cx: &mut App, tab: TabId, state: TabEditor) {
    let s = store(cx);
    if s.doc.tabs.get(&tab).is_some_and(|e| e.state == state) {
        return;
    }
    s.doc.tabs.insert(
        tab,
        Entry {
            state,
            touched: now_secs(),
        },
    );
    prune(&mut s.doc);
    schedule_write(cx);
}

/// Forgets a tab that was closed.
pub fn remove(cx: &mut App, tab: TabId) {
    if store(cx).doc.tabs.remove(&tab).is_some() {
        schedule_write(cx);
    }
}

fn prune(doc: &mut Doc) {
    if doc.tabs.len() <= MAX_TABS {
        return;
    }
    let mut by_age: Vec<(u64, TabId)> = doc.tabs.iter().map(|(id, e)| (e.touched, *id)).collect();
    by_age.sort_by_key(|(touched, _)| *touched);
    for (_, id) in by_age.into_iter().take(doc.tabs.len() - MAX_TABS) {
        doc.tabs.remove(&id);
    }
}

fn schedule_write(cx: &mut App) {
    if test_guarded() {
        return;
    }
    let s = store(cx);
    if s.write_scheduled {
        return;
    }
    s.write_scheduled = true;
    cx.spawn(async move |cx| {
        cx.background_executor().timer(WRITE_DELAY).await;
        let json = cx.update(|cx| {
            let s = cx.global_mut::<EditorSessionStore>();
            s.write_scheduled = false;
            serde_json::to_vec_pretty(&s.doc)
        });
        let json = match json {
            Ok(json) => json,
            Err(e) => {
                log::warn!("editor sessions: could not serialize: {e}");
                return;
            }
        };
        cx.background_executor()
            .spawn(async move {
                let Some(path) = lyrel_core::core::config::config_path(FILE) else {
                    return;
                };
                if let Err(e) = lyrel_core::core::config::write_atomic(&path, &json) {
                    log::warn!("editor sessions: could not write {}: {e}", path.display());
                }
            })
            .await;
    })
    .detach();
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_document_round_trips_with_tab_ids_as_keys() {
        let mut doc = Doc::default();
        let id = TabId::new();
        doc.tabs.insert(
            id,
            Entry {
                state: TabEditor {
                    files: vec![PathBuf::from("/src/main.rs"), PathBuf::from("/README.md")],
                    active: 1,
                    visible: true,
                    split: Some(SplitEditor {
                        files: vec![PathBuf::from("/src/lib.rs")],
                        active: 0,
                        focused: true,
                    }),
                },
                touched: 7,
            },
        );
        let json = serde_json::to_string(&doc).unwrap();
        let back: Doc = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tabs[&id].state, doc.tabs[&id].state);
    }

    #[test]
    fn a_record_from_before_the_split_still_reads() {
        let old = r#"{"tabs": {}}"#;
        assert!(serde_json::from_str::<Doc>(old).is_ok());
        let state: TabEditor =
            serde_json::from_str(r#"{"files": ["/a.rs"], "active": 0, "visible": true}"#).unwrap();
        assert_eq!(state.split, None);
        assert!(!serde_json::to_string(&state).unwrap().contains("split"));
    }

    #[test]
    fn the_oldest_tabs_are_forgotten_first() {
        let mut doc = Doc::default();
        let mut ids = Vec::new();
        for touched in 0..(MAX_TABS as u64 + 3) {
            let id = TabId::new();
            ids.push(id);
            doc.tabs.insert(
                id,
                Entry {
                    state: TabEditor::default(),
                    touched,
                },
            );
        }
        prune(&mut doc);
        assert_eq!(doc.tabs.len(), MAX_TABS);
        for old in &ids[..3] {
            assert!(!doc.tabs.contains_key(old));
        }
        assert!(doc.tabs.contains_key(ids.last().unwrap()));
    }
}
