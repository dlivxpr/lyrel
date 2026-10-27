use super::{Candidate, CandidateKind, CompletionSession, RemotePathRequest, Replacement};
use crate::terminal::cmd_editor::CmdEditor;
use crate::ui::host_ops::HostId;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use std::path::{Path, PathBuf};

#[derive(Clone, PartialEq, Eq)]
pub(in crate::terminal) struct CompletionContext {
    host: HostId,
    cwd: Option<PathBuf>,
    remote: Option<crate::daemon::protocol::RemoteContext>,
}

impl CompletionContext {
    pub(in crate::terminal) fn new(host: HostId, cwd: Option<PathBuf>, remote: Option<crate::daemon::protocol::RemoteContext>) -> Self {
        Self { host, cwd, remote }
    }
}

pub(in crate::terminal) enum CompletionWork {
    Updated,
    Handoff,
    Generators { generation: u64, scripts: Vec<super::PendingGenerator>, cwd: PathBuf },
    Remote(RemoteRequest),
}

pub(in crate::terminal) struct RemoteRequest {
    generation: u64,
    context: CompletionContext,
    line: String,
    cursor: usize,
    path: RemotePathRequest,
    forward: bool,
}

impl RemoteRequest {
    pub(in crate::terminal) fn directory(&self) -> &str { &self.path.dir }
}

/// Owns the complete suggestion lifecycle. The retained view only schedules I/O,
/// supplies the current input/locality, and applies the shell-handoff decision.
#[derive(Default)]
pub(in crate::terminal) struct TerminalCompletion {
    session: Option<CompletionSession>,
    generation: u64,
    context: Option<CompletionContext>,
    remote_inflight: bool,
    notice: Option<String>,
}

impl TerminalCompletion {
    /// History remains the only store; ghosts use recency, not frecency.
    pub(in crate::terminal) fn ghost_suggestion(&self, cmd: &CmdEditor, history: &crate::terminal::history_session::TerminalHistory) -> Option<String> {
        if cmd.is_empty() || cmd.cursor() != cmd.len() { return None }
        let line = cmd.text();
        let mut elsewhere = None;
        for entry in history.entries().iter().rev() {
            if entry.len() <= line.len() || !entry.starts_with(&line) { continue }
            if history.metadata(entry).and_then(|m| m.exit).is_some_and(|exit| exit != 0) { continue }
            if history.directory_affinity(entry) != Some(false) { return Some(entry.clone()) }
            elsewhere.get_or_insert(entry);
        }
        elsewhere.cloned()
    }

    pub(in crate::terminal) fn is_open(&self) -> bool { self.session.is_some() }
    pub(in crate::terminal) fn has_work(&self) -> bool { self.session.is_some() || self.remote_inflight }
    pub(in crate::terminal) fn has_selection(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.selected().is_some())
    }
    pub(in crate::terminal) fn items(&self) -> Vec<&Candidate> {
        self.session.as_ref().map(|s| s.filtered.iter().map(|&i| &s.all[i]).collect()).unwrap_or_default()
    }
    pub(in crate::terminal) fn selected_index(&self) -> Option<usize> {
        self.session.as_ref().and_then(|s| s.index)
    }
    pub(in crate::terminal) fn notice_text(&self) -> Option<String> {
        self.notice.clone().or_else(|| self.remote_inflight.then(|| t(L10nKey::CompletionListingRemote).to_string()))
    }
    pub(in crate::terminal) fn dismiss_notice(&mut self) -> bool { self.notice.take().is_some() }

    pub(in crate::terminal) fn close(&mut self) {
        self.session = None;
        self.remote_inflight = false;
        self.generation = self.generation.wrapping_add(1);
    }

    pub(in crate::terminal) fn observe_context(&mut self, context: CompletionContext) -> bool {
        if self.context.as_ref() == Some(&context) { return false }
        let changed = self.has_work() || self.notice.is_some();
        self.close();
        self.notice = None;
        self.context = Some(context);
        changed
    }

    pub(in crate::terminal) fn tab(
        &mut self,
        cmd: &mut CmdEditor,
        context: CompletionContext,
        local_cwd: Option<&Path>,
        share_cwd: Option<&Path>,
        remote_cwd: Option<&str>,
        shell: Option<&str>,
        enabled: bool,
        forward: bool,
    ) -> CompletionWork {
        self.observe_context(context.clone());
        if !enabled { self.close(); return CompletionWork::Handoff }
        if self.is_open() {
            self.tab_step(cmd, shell, forward);
            return CompletionWork::Updated;
        }
        if self.remote_inflight { return CompletionWork::Updated }
        let line = cmd.text();
        let cursor = cmd.cursor();
        let completion = match share_cwd {
            Some(cwd) => super::complete_foreign(&line, cursor, cwd),
            None => super::complete(&line, cursor, local_cwd, shell),
        };
        let Some(completion) = completion else {
            return match remote_cwd.and_then(|cwd| super::remote_path_request(&line, cursor, cwd)) {
                Some(path) => CompletionWork::Remote(self.begin_remote(cmd, context, path, forward)),
                None => CompletionWork::Handoff,
            };
        };
        let (start, end) = completion.candidates.first().map(|c| (c.start, c.end)).unwrap_or_else(|| {
            let chars: Vec<char> = line.chars().collect();
            let mut start = cursor.min(chars.len());
            while start > 0 && !chars[start - 1].is_whitespace() { start -= 1; }
            (start, cursor)
        });
        let generation = self.offer(cmd, start, end, completion.candidates, completion.pending.len(), shell);
        match (generation, local_cwd) {
            (Some(generation), Some(cwd)) if !completion.pending.is_empty() => CompletionWork::Generators {
                generation, scripts: completion.pending, cwd: cwd.to_path_buf(),
            },
            _ => CompletionWork::Updated,
        }
    }

    fn begin_remote(&mut self, cmd: &CmdEditor, context: CompletionContext, path: RemotePathRequest, forward: bool) -> RemoteRequest {
        self.observe_context(context.clone());
        self.close();
        self.remote_inflight = true;
        RemoteRequest { generation: self.generation, context, line: cmd.text(), cursor: cmd.cursor(), path, forward }
    }

    /// A stale answer must not clear a successor's loading indicator or error.
    /// Failed listings retain a notice; successful empty listings hand Tab back.
    pub(in crate::terminal) fn remote_results(
        &mut self,
        request: RemoteRequest,
        context: CompletionContext,
        cmd: &mut CmdEditor,
        active: bool,
        listed: std::io::Result<Vec<crate::daemon::protocol::SftpEntry>>,
        shell: Option<&str>,
    ) -> Option<bool> {
        self.observe_context(context.clone());
        if self.generation != request.generation || request.context != context || !self.remote_inflight {
            return None;
        }
        self.remote_inflight = false;
        if !active || cmd.text() != request.line || cmd.cursor() != request.cursor { self.close(); return None }
        let entries = match listed {
            Ok(entries) => entries,
            Err(error) => {
                self.notice = Some(t_fmt(L10nKey::CompletionRemoteListingFailed, &[("error", &error.to_string())]));
                return Some(!request.forward);
            }
        };
        let entries: Vec<super::RemoteEntry> = entries.into_iter().map(|entry| super::RemoteEntry {
            is_dir: entry.kind == crate::daemon::protocol::SftpEntryKind::Dir || entry.target_is_dir,
            name: entry.name,
        }).collect();
        let candidates = super::remote_path_candidates(&request.path, &entries);
        if candidates.is_empty() { return Some(!request.forward) }
        self.offer(cmd, request.path.word_start, request.path.cursor, candidates, 0, shell);
        None
    }

    pub(in crate::terminal) fn offer(&mut self, cmd: &mut CmdEditor, start: usize, end: usize, candidates: Vec<Candidate>, pending: usize, shell: Option<&str>) -> Option<u64> {
        self.close();
        if pending == 0 && candidates.len() == 1 {
            Self::insert(cmd, &candidates[0], candidates[0].start, shell);
            return None;
        }
        let line = cmd.text();
        let word: String = line.chars().skip(start).take(end - start).collect();
        let session = CompletionSession::new(start, word.clone(), candidates, pending);
        if pending == 0
            && let Some(prefix) = session.common_prefix()
            && prefix.chars().count() > word.chars().count()
            && crate::core::shell_quote::quote_for_shell(&prefix, shell) == prefix
        {
            Self::replace(cmd, start, end, &prefix);
        }
        self.session = Some(session);
        Some(self.generation)
    }

    pub(in crate::terminal) fn merge(&mut self, generation: u64, context: CompletionContext, cmd: &CmdEditor, results: Vec<crate::terminal::generator::Parsed>) -> bool {
        self.observe_context(context);
        if self.generation != generation { return false }
        let Some(session) = self.session.as_mut() else { return false };
        let chars: Vec<char> = cmd.text().chars().collect();
        let cursor = cmd.cursor().min(chars.len());
        let start = session.word_start;
        if cursor < start { self.close(); return true }
        let word: String = chars[start..cursor].iter().collect();
        let candidates = results.into_iter().map(|p| Candidate {
            text: p.text, kind: CandidateKind::Value, start, end: cursor,
            description: p.description, icon: None,
        }).collect();
        session.generator_answered();
        session.merge(candidates, &word);
        if session.is_spent() { self.close(); }
        true
    }

    pub(in crate::terminal) fn select(&mut self, forward: bool) {
        if let Some(session) = self.session.as_mut() { session.select(forward); }
    }

    fn tab_step(&mut self, cmd: &mut CmdEditor, shell: Option<&str>, forward: bool) {
        if forward && let Some(session) = self.session.as_ref() {
            let cursor = cmd.cursor().min(cmd.len());
            if let Some(prefix) = session.common_prefix()
                && prefix.chars().count() > cursor.saturating_sub(session.word_start)
            {
                if session.filtered.len() == 1 { self.accept(cmd, shell); return }
                if crate::core::shell_quote::quote_for_shell(&prefix, shell) == prefix {
                    Self::replace(cmd, session.word_start, cursor, &prefix);
                    return;
                }
            }
        }
        self.select(forward);
    }

    pub(in crate::terminal) fn accept(&mut self, cmd: &mut CmdEditor, shell: Option<&str>) {
        let session = self.session.take();
        self.close();
        if let Some(session) = session && let Some(candidate) = session.selected() {
            Self::insert(cmd, candidate, session.word_start, shell);
        }
    }

    fn insert(cmd: &mut CmdEditor, candidate: &Candidate, start: usize, shell: Option<&str>) {
        let cursor = cmd.cursor().min(cmd.len());
        let mut text = crate::core::shell_quote::quote_for_shell(&candidate.text, shell);
        if candidate.is_dir() {
            if !text.ends_with('/') { text.push('/'); }
        } else if cursor == cmd.len() { text.push(' '); }
        Self::replace(cmd, start, cursor, &text);
    }

    fn replace(cmd: &mut CmdEditor, start: usize, end: usize, text: &str) {
        let (line, cursor) = Replacement { orig: cmd.text(), start, end, text: text.to_string() }.apply();
        cmd.set_with_cursor(&line, cursor);
    }

    pub(in crate::terminal) fn refilter(&mut self, cmd: &CmdEditor) {
        if self.remote_inflight { self.close(); }
        let Some(session) = self.session.as_mut() else { return };
        let chars: Vec<char> = cmd.text().chars().collect();
        let cursor = cmd.cursor().min(chars.len());
        let keep = cursor >= session.word_start
            && chars[session.word_start..cursor].iter().all(|c| !c.is_whitespace())
            && session.refilter(&chars[session.word_start..cursor].iter().collect::<String>());
        if !keep { self.close(); }
    }
}
