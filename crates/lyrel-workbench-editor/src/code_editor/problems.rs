//! The editor's Problems list: every error, warning and note the language
//! servers have published for the open files, grouped by file.
//!
//! It lives at the foot of the code panel rather than as a right-panel tab:
//! problems belong to the files open in the editor, the status bar's error and
//! warning counts are where people look for them, and a click lands in the
//! editor right above it. The status bar counts toggle it, as does the
//! `ToggleEditorProblems` command.
//!
//! Hints are left out, as VS Code leaves them out: servers use them for
//! inlay-ish suggestions that would bury the real problems.
use std::path::PathBuf;

use gpui_kit::{Context, ScrollHandle};
use lsp_types::DiagnosticSeverity;

use super::EditorPanelState;

#[derive(Default)]
pub struct ProblemsPane {
    pub open: bool,
    scroll: ScrollHandle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// Zero-based, in editor columns.
    pub line: u32,
    pub column: u32,
    pub severity: Severity,
    pub message: String,
    pub source: Option<String>,
}

/// One file's problems, in the order they appear in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileProblems {
    pub path: PathBuf,
    pub problems: Vec<Problem>,
}

/// Sort the store's snapshot into what the list draws: files by path, each
/// file's problems by position, hints dropped, files left with nothing gone.
pub fn collect(snapshot: Vec<(PathBuf, Vec<lsp_types::Diagnostic>)>) -> Vec<FileProblems> {
    let mut files: Vec<FileProblems> = snapshot
        .into_iter()
        .map(|(path, diagnostics)| {
            let mut problems: Vec<Problem> = diagnostics
                .into_iter()
                .filter_map(|d| {
                    let severity = match d.severity {
                        // A server that says nothing means an error (LSP leaves
                        // it to the client; every editor reads it that way).
                        None | Some(DiagnosticSeverity::ERROR) => Severity::Error,
                        Some(DiagnosticSeverity::WARNING) => Severity::Warning,
                        Some(DiagnosticSeverity::INFORMATION) => Severity::Info,
                        _ => return None,
                    };
                    Some(Problem {
                        line: d.range.start.line,
                        column: d.range.start.character,
                        severity,
                        message: d.message,
                        source: d.source,
                    })
                })
                .collect();
            problems.sort_by(|a, b| {
                (a.line, a.column, a.severity).cmp(&(b.line, b.column, b.severity))
            });
            FileProblems { path, problems }
        })
        .filter(|f| !f.problems.is_empty())
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// The first line of a message: the list is one row per problem, and the
/// rest is a hover away in the editor.
pub fn headline(message: &str) -> &str {
    message.lines().next().unwrap_or("").trim_end()
}

impl EditorPanelState {
    pub fn toggle_problems(&mut self, cx: &mut Context<Self>) {
        self.problems.open = !self.problems.open;
        cx.notify();
    }

    pub fn problems_open(&self) -> bool {
        self.problems.open
    }

    pub fn problems_scroll(&self) -> gpui_kit::ScrollHandle {
        self.problems.scroll.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Diagnostic, Position, Range};

    fn d(line: u32, character: u32, severity: Option<DiagnosticSeverity>, msg: &str) -> Diagnostic {
        Diagnostic {
            range: Range::new(
                Position::new(line, character),
                Position::new(line, character + 1),
            ),
            severity,
            message: msg.into(),
            ..Default::default()
        }
    }

    #[test]
    fn problems_are_sorted_by_file_then_position_without_hints() {
        let files = collect(vec![
            (
                PathBuf::from("/p/src/b.rs"),
                vec![
                    d(9, 0, Some(DiagnosticSeverity::WARNING), "w"),
                    d(2, 4, Some(DiagnosticSeverity::ERROR), "e"),
                    d(2, 1, None, "unspecified is an error"),
                    d(5, 0, Some(DiagnosticSeverity::HINT), "hint"),
                ],
            ),
            (
                PathBuf::from("/p/src/a.rs"),
                vec![d(0, 0, Some(DiagnosticSeverity::INFORMATION), "i")],
            ),
            (
                PathBuf::from("/p/src/c.rs"),
                vec![d(0, 0, Some(DiagnosticSeverity::HINT), "only a hint")],
            ),
        ]);
        assert_eq!(files.len(), 2, "a file with only hints has no problems");
        assert_eq!(files[0].path, PathBuf::from("/p/src/a.rs"));
        assert_eq!(files[0].problems[0].severity, Severity::Info);
        let b: Vec<_> = files[1]
            .problems
            .iter()
            .map(|p| (p.line, p.column, p.severity))
            .collect();
        assert_eq!(
            b,
            vec![
                (2, 1, Severity::Error),
                (2, 4, Severity::Error),
                (9, 0, Severity::Warning)
            ]
        );
    }

    #[test]
    fn a_row_shows_the_first_line_of_the_message() {
        assert_eq!(
            headline("mismatched types\nexpected `u8`"),
            "mismatched types"
        );
        assert_eq!(headline(""), "");
    }
}
