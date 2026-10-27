use super::cmd_editor::CmdEditor;
use super::history::{self, EntryMeta, History, Scope};
use super::reverse_search::{Action, ReverseSearch};
use crate::ui::host_ops::HostId;
use std::path::{Path, PathBuf};

/// A pane's history and all transitions that keep its indices and scope coherent.
pub(super) struct TerminalHistory {
    data: History,
    frecency: Vec<f64>,
    scope: Scope,
    host: HostId,
    generation: u64,
    cache: Vec<(HostId, Scope, History)>,
    ready: bool,
    ranked_cwd: Option<PathBuf>,
    nav: Option<usize>,
    stash: String,
    prefix: String,
    last_word: Option<LastWordWalk>,
    pending: Option<PendingHistory>,
    search: Option<ReverseSearch>,
}

pub(super) struct HistoryLoad {
    scope: Scope,
    host: HostId,
    generation: u64,
}

impl HistoryLoad {
    pub(super) fn scope(&self) -> &Scope {
        &self.scope
    }
}

#[derive(Clone, Copy)]
pub(super) struct PromptHistory {
    seq: u64,
    at_prompt: bool,
    exit: Option<i32>,
}

impl PromptHistory {
    pub(super) fn new(seq: u64, at_prompt: bool, exit: Option<i32>) -> Self {
        Self { seq, at_prompt, exit }
    }
}

struct PendingHistory {
    line: String,
    cwd: Option<PathBuf>,
    ts: u64,
    seq: u64,
}

struct LastWordWalk {
    entry: usize,
    at: usize,
    word: String,
}

impl TerminalHistory {
    pub(super) fn new() -> Self {
        Self::from_loaded(history::load(&Scope::Local))
    }

    pub(super) fn from_loaded(data: History) -> Self {
        let frecency = history::frecency_scores(&data.entries, &data.counts, &data.cwds, None);
        Self {
            data,
            frecency,
            scope: Scope::Local,
            host: HostId::LOCAL,
            generation: 0,
            cache: Vec::new(),
            ready: true,
            ranked_cwd: None,
            nav: None,
            stash: String::new(),
            prefix: String::new(),
            last_word: None,
            pending: None,
            search: None,
        }
    }

    /// Borrow candidates in place; completion must not clone the history to read it.
    pub(super) fn entries(&self) -> &[String] {
        &self.data.entries
    }

    pub(super) fn frecency(&self) -> &[f64] {
        &self.frecency
    }

    pub(super) fn metadata(&self, line: &str) -> Option<&EntryMeta> {
        self.data.meta.get(line)
    }

    pub(super) fn scope(&self) -> &Scope {
        &self.scope
    }

    pub(super) fn change_scope(
        &mut self,
        scope: Scope,
        host: HostId,
        prompt: PromptHistory,
    ) -> Option<HistoryLoad> {
        if self.scope == scope && self.host == host {
            return None;
        }
        self.flush_pending(prompt);
        // Only complete loads are authoritative cache entries. Moving the
        // containers preserves the existing no-copy scope-switch cost.
        if self.ready {
            self.cache.retain(|(host, cached, _)| *host != self.host || *cached != self.scope);
            self.cache.push((self.host, self.scope.clone(), std::mem::take(&mut self.data)));
            if self.cache.len() > 4 {
                self.cache.remove(0);
            }
        }
        self.scope = scope;
        self.host = host;
        self.generation = self.generation.wrapping_add(1);
        self.data = match self.cache.iter().position(|(host, cached, _)| *host == self.host && *cached == self.scope) {
            Some(i) => {
                self.ready = true;
                self.cache.remove(i).2
            }
            None => {
                self.ready = false;
                History::default()
            }
        };
        self.nav = None;
        self.search = None;
        self.last_word = None;
        self.rerank();
        Some(HistoryLoad {
            scope: self.scope.clone(),
            host,
            generation: self.generation,
        })
    }

    /// Reject both scope round-trips and a Host replacement before its next poll.
    pub(super) fn accept_load(&mut self, load: HistoryLoad, host: HostId, data: History) -> bool {
        if load.generation != self.generation || load.scope != self.scope
            || load.host != self.host || load.host != host
        {
            return false;
        }
        self.data = data;
        self.ready = true;
        self.nav = None;
        self.last_word = None;
        self.rerank();
        if let Some(search) = self.search.as_mut() {
            search.refresh(&self.data.entries, &self.frecency);
        }
        true
    }

    fn rerank(&mut self) {
        self.frecency = history::frecency_scores(
            &self.data.entries,
            &self.data.counts,
            &self.data.cwds,
            self.ranked_cwd.as_deref().and_then(Path::to_str),
        );
    }

    pub(super) fn rank_for_cwd(&mut self, cwd: Option<&Path>) {
        if self.ranked_cwd.as_deref() != cwd {
            self.ranked_cwd = cwd.map(Path::to_path_buf);
            self.rerank();
        }
    }

    pub(super) fn record(&mut self, line: &str, cwd: Option<PathBuf>, prompt: PromptHistory) {
        if !line.trim().is_empty() {
            let now = unix_now();
            *self.data.counts.entry(line.to_string()).or_insert(0) += 1;
            if let Some(dir) = cwd.as_ref().and_then(|p| p.to_str()) {
                self.data.cwds.entry(line.to_string()).or_default().insert(dir.to_string());
            }
            self.data.meta.insert(line.to_string(), EntryMeta { ts: Some(now), exit: None });
            self.data.entries.retain(|h| h != line);
            self.data.entries.push(line.to_string());
            self.flush_pending(prompt);
            self.pending = Some(PendingHistory { line: line.to_string(), cwd: cwd.clone(), ts: now, seq: prompt.seq });
            self.ranked_cwd = cwd;
            self.rerank();
        }
        // Dedup can move every index, including the last-word walk.
        self.nav = None;
        self.last_word = None;
        self.stash.clear();
        self.prefix.clear();
    }

    pub(super) fn poll_prompt(&mut self, prompt: PromptHistory) -> bool {
        if self.pending.as_ref().is_some_and(|p| prompt.at_prompt && prompt.seq > p.seq) {
            self.flush_pending(prompt);
            true
        } else {
            false
        }
    }

    pub(super) fn flush_pending(&mut self, prompt: PromptHistory) {
        let Some(p) = self.pending.take() else { return };
        let exit = (prompt.seq > p.seq && prompt.at_prompt).then_some(prompt.exit).flatten();
        if exit.is_some() && let Some(meta) = self.data.meta.get_mut(&p.line) {
            meta.exit = exit;
        }
        history::append(&self.scope, &p.line, p.cwd.as_deref(), p.ts, exit);
    }

    pub(super) fn reset_navigation(&mut self) {
        self.nav = None;
    }

    pub(super) fn reset_last_word(&mut self) {
        self.last_word = None;
    }

    #[cfg(test)]
    pub(super) fn is_navigating(&self) -> bool {
        self.nav.is_some()
    }

    #[cfg(test)]
    pub(super) fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Capture only text before the cursor as prefix, but restore the whole draft.
    pub(super) fn previous(&mut self, cmd: &mut CmdEditor) -> bool {
        let from = match self.nav {
            None => {
                let line = cmd.text();
                self.prefix = line[..cmd.cursor_byte()].to_string();
                self.stash = line;
                self.data.entries.len()
            }
            Some(i) => i,
        };
        let Some(next) = (0..from).rev().find(|&i| self.data.entries[i].starts_with(&self.prefix)) else {
            return false;
        };
        self.nav = Some(next);
        cmd.set(&self.data.entries[next]);
        true
    }

    pub(super) fn next(&mut self, cmd: &mut CmdEditor) -> bool {
        let Some(i) = self.nav else { return false };
        if let Some(next) = (i + 1..self.data.entries.len()).find(|&j| self.data.entries[j].starts_with(&self.prefix)) {
            self.nav = Some(next);
            cmd.set(&self.data.entries[next]);
        } else {
            self.nav = None;
            self.prefix.clear();
            cmd.set(&std::mem::take(&mut self.stash));
        }
        true
    }

    pub(super) fn insert_last_word(&mut self, cmd: &mut CmdEditor) -> bool {
        let resumed = self.last_word.take().filter(|walk| {
            let len = walk.word.chars().count();
            cmd.cursor() == walk.at + len && cmd.selection().is_none()
                && cmd.text().chars().skip(walk.at).take(len).eq(walk.word.chars())
        });
        let start = match &resumed {
            Some(walk) => walk.entry.checked_sub(1),
            None => self.data.entries.len().checked_sub(1),
        };
        let Some(mut entry) = start else { self.last_word = resumed; return false };
        let word = loop {
            if let Some(word) = self.data.entries[entry].split_whitespace().next_back() {
                break word.to_string();
            }
            let Some(older) = entry.checked_sub(1) else { self.last_word = resumed; return false };
            entry = older;
        };
        if let Some(walk) = resumed {
            cmd.clear_selection();
            cmd.set_cursor(walk.at);
            cmd.extend_to(walk.at + walk.word.chars().count());
            cmd.delete_selection();
        }
        cmd.insert_str(&word);
        let at = cmd.cursor() - word.chars().count();
        self.last_word = Some(LastWordWalk { entry, at, word });
        self.nav = None;
        true
    }

    /// Ghost text uses recency, cwd affinity and successful exit, not frecency.
    pub(super) fn ghost_suggestion(&self, cmd: &CmdEditor) -> Option<String> {
        if cmd.is_empty() || cmd.cursor() != cmd.len() { return None }
        let line = cmd.text();
        let cwd = self.ranked_cwd.as_deref().and_then(Path::to_str);
        let mut elsewhere = None;
        for entry in self.data.entries.iter().rev() {
            if entry.len() <= line.len() || !entry.starts_with(&line) { continue }
            if self.data.meta.get(entry).and_then(|m| m.exit).is_some_and(|exit| exit != 0) { continue }
            let here = cwd.is_some_and(|dir| self.data.cwds.get(entry).is_some_and(|dirs| dirs.contains(dir)));
            if here || cwd.is_none() { return Some(entry.clone()) }
            elsewhere.get_or_insert(entry);
        }
        elsewhere.cloned()
    }

    pub(super) fn search(&self) -> Option<&ReverseSearch> { self.search.as_ref() }

    pub(super) fn selected_line(&self) -> Option<&str> {
        self.search.as_ref()?.selected_line(&self.data.entries)
    }

    pub(super) fn start_search(&mut self) {
        if self.search.is_none() {
            self.search = Some(ReverseSearch::new(self.entries(), self.frecency()));
        }
    }

    pub(super) fn cancel_search(&mut self) { self.search = None; }

    pub(super) fn push_search_query(&mut self, text: &str) -> bool {
        let Some(search) = self.search.as_mut() else { return false };
        search.push_query(text, &self.data.entries, &self.frecency);
        true
    }

    pub(super) fn search_key(&mut self, ks: &gpui_kit::Keystroke) -> Action {
        let m = &ks.modifiers;
        if !m.control && !m.platform && !m.alt
            && let Some(ch) = ks.key_char.as_deref()
            && !ch.is_empty() && ch.chars().all(|c| c >= '\u{20}' && c != '\u{7f}')
        {
            self.push_search_query(ch);
            return Action::Redraw;
        }
        let Some(search) = self.search.as_mut() else { return Action::Redraw };
        let action = search.handle_key(ks, &self.data.entries, &self.frecency);
        if !matches!(action, Action::Redraw) { self.search = None; }
        action
    }
}

pub(super) fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
