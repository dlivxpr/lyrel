//! Fenced-code commands share the document's source, selection and history.
use super::*;

impl EditorState {
    /// Opening fence row for the code block containing this presentation's caret.
    pub fn caret_code_fence(&self) -> Option<usize> {
        let (row, _) = self.row_col(self.cursor_offset());
        let mut opening = None;
        for (line, text) in self.content.split('\n').enumerate().take(row + 1) {
            if text.trim_start().starts_with("```") {
                if let Some(start) = opening.take() {
                    if line == row { return Some(start); }
                } else {
                    opening = Some(line);
                }
            }
        }
        opening
    }

    /// Language token only; additional fence metadata remains untouched.
    pub fn caret_code_language(&self) -> Option<String> {
        self.code_block_at(self.caret_code_fence()?).map(|(language, _)| language)
    }

    /// Change only the current block's language token as one undoable source edit.
    pub fn set_caret_code_language(&mut self, language: &str, cx: &mut Context<Self>) {
        if let Some(row) = self.caret_code_fence() {
            self.set_code_lang(row, language, cx);
        }
    }

    /// Copy the current code body without exposing a second editable document.
    pub fn copy_caret_code(&mut self, cx: &mut Context<Self>) {
        if let Some((_, body)) = self.caret_code_fence().and_then(|row| self.code_block_at(row)) {
            self.write_clipboard(self.content[body].to_string(), cx);
        }
    }

    /// Reveal the opening delimiter through normal caret-local source access.
    pub fn reveal_caret_code_fence(&mut self, cx: &mut Context<Self>) {
        if let Some(row) = self.caret_code_fence() {
            let at = self.line_starts()[row];
            self.select_range(at..at, cx);
        }
    }
}
