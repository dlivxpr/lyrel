const MAX_CONTEXT_BYTES: usize = 24 * 1024;

fn capped(text: &str) -> String {
    if text.len() <= MAX_CONTEXT_BYTES {
        return text.to_string();
    }
    let mut end = MAX_CONTEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[… truncated by lyrel — ask for the rest if needed]",
        &text[..end]
    )
}

pub fn build_selection_prompt(selection: &str, cwd: Option<&str>) -> Option<String> {
    let selection = selection.trim_end();
    if selection.trim().is_empty() {
        return None;
    }
    let mut prompt = String::from(
        "Here is terminal output I selected in another pane; please take a look and help me address it.",
    );
    if let Some(cwd) = cwd.filter(|c| !c.is_empty()) {
        prompt.push_str(&format!(" It came from a shell running in `{cwd}`."));
    }
    prompt.push_str("\n\n```\n");
    prompt.push_str(&capped(selection));
    prompt.push_str("\n```");
    Some(prompt)
}

pub fn build_diff_review_prompt(diff: &str, cwd: Option<&str>) -> Option<String> {
    let diff = diff.trim_end();
    if diff.trim().is_empty() {
        return None;
    }
    let mut prompt = String::from(
        "Please review the following uncommitted changes in this repository: point out bugs, \
         regressions, and anything that looks unintended. Keep it focused — this is a working diff, \
         not a style pass.",
    );
    if let Some(cwd) = cwd.filter(|c| !c.is_empty()) {
        prompt.push_str(&format!(" Repository: `{cwd}`."));
    }
    prompt.push_str("\n\n```diff\n");
    prompt.push_str(&capped(diff));
    prompt.push_str("\n```");
    Some(prompt)
}

/// The prompt an explicit side-panel share sends to the agent pane that asked
/// for the preview (#393): the user's own words plus the absolute path of the
/// screenshot on that pane's Host. `feedback` may be empty when the screenshot
/// alone is the message; both empty is not a share at all.
pub fn build_preview_feedback_prompt(
    feedback: &str,
    screenshot: Option<&str>,
    address: &str,
) -> Option<String> {
    let feedback = feedback.trim();
    let screenshot = screenshot.map(str::trim).filter(|path| !path.is_empty());
    if feedback.is_empty() && screenshot.is_none() {
        return None;
    }
    let mut prompt = String::from(
        "Here is feedback on the page preview you asked me to check. Please use it to fix the \
         page.",
    );
    if !address.is_empty() {
        prompt.push_str(&format!(" Preview: `{address}`."));
    }
    prompt.push_str("\n\n");
    if feedback.is_empty() {
        prompt.push_str("I did not write notes — the screenshot shows the problem.");
    } else {
        prompt.push_str("Feedback:\n");
        prompt.push_str(&capped(feedback));
    }
    if let Some(path) = screenshot {
        prompt.push_str(&format!("\n\nScreenshot (PNG): `{path}`"));
    }
    Some(prompt)
}

pub fn submit_bytes(prompt: &str) -> Vec<u8> {
    let mut bytes = crate::core::paste::bracket(prompt.as_bytes());
    bytes.push(b'\r');
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_prompt_embeds_the_text_and_cwd() {
        let p = build_selection_prompt("error[E0308]: mismatched types", Some("/work/lyrel"))
            .expect("non-empty selection builds");
        assert!(p.contains("error[E0308]"));
        assert!(p.contains("/work/lyrel"));
        assert!(p.contains("```"));
        assert_eq!(build_selection_prompt("   \n", None), None);
    }

    #[test]
    fn diff_prompt_embeds_the_diff() {
        let p = build_diff_review_prompt("--- a/x\n+++ b/x\n+added", None).unwrap();
        assert!(p.contains("```diff"));
        assert!(p.contains("+added"));
        assert_eq!(build_diff_review_prompt("", None), None);
    }

    #[test]
    fn oversized_context_is_truncated_with_a_note() {
        let big = "x".repeat(MAX_CONTEXT_BYTES + 100);
        let p = build_selection_prompt(&big, None).unwrap();
        assert!(p.len() < big.len() + 500);
        assert!(p.contains("truncated by lyrel"));
    }

    #[test]
    fn preview_feedback_names_the_page_and_the_screenshot() {
        let p = build_preview_feedback_prompt(
            "the button overlaps the title",
            Some("/tmp/lyrel/previews/preview-7-1.png"),
            "http://localhost:5173",
        )
        .expect("feedback builds");
        assert!(p.contains("http://localhost:5173"));
        assert!(p.contains("the button overlaps the title"));
        assert!(p.contains("preview-7-1.png"));
        assert_eq!(
            build_preview_feedback_prompt("   ", None, "http://localhost:5173"),
            None,
            "an empty feedback with no screenshot is not a share"
        );
        let shot_only =
            build_preview_feedback_prompt("", Some("/tmp/a.png"), "http://localhost:5173")
                .expect("a screenshot alone is a share");
        assert!(shot_only.contains("the screenshot shows the problem"));
        assert!(shot_only.contains("/tmp/a.png"));
    }

    #[test]
    fn submit_bytes_bracket_and_sanitize() {
        let bytes = submit_bytes("fix this\nplease");
        assert!(bytes.starts_with(b"\x1b[200~"));
        assert!(bytes.ends_with(b"\x1b[201~\r"));
        let sneaky = submit_bytes("a\x1b[201~; rm -rf /\nb");
        let inner = &sneaky[6..sneaky.len() - 7];
        assert!(!inner.contains(&0x1b));
    }
}
