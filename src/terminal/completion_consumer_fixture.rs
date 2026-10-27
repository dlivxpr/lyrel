use super::*;
use crate::daemon::protocol::{SftpEntry, SftpEntryKind};

pub(super) fn remote_request(view: &mut TerminalView) -> RemoteRequest {
    let context = view.completion_context();
    match view.completion.tab(view.input.editor_mut(), context, None, None, Some("/work"), Some("bash"), true, true) {
        CompletionWork::Remote(request) => request,
        _ => panic!("the path needs a listing from the remote filesystem"),
    }
}

pub(super) fn entries(names: &[&str], dirs: bool) -> Vec<SftpEntry> {
    names.iter().map(|name| SftpEntry {
        name: name.to_string(),
        kind: if dirs { SftpEntryKind::Dir } else { SftpEntryKind::File },
        size: 0, mtime: 0, permissions: 0, target_is_dir: false,
    }).collect()
}

pub(super) fn shown(view: &TerminalView) -> Vec<String> {
    view.completion.items().iter().map(|candidate| candidate.text.clone()).collect()
}

pub(super) fn generator_request(view: &mut TerminalView, cwd: &std::path::Path) -> (u64, usize) {
    let context = view.completion_context();
    match view.completion.tab(view.input.editor_mut(), context, Some(cwd), None, None, Some("bash"), true, true) {
        CompletionWork::Generators { generation, scripts, .. } => (generation, scripts.len()),
        _ => panic!("git's signature must request its real branch generators"),
    }
}

pub(super) fn parsed(words: &[&str]) -> Vec<crate::terminal::generator::Parsed> {
    words.iter().map(|word| crate::terminal::generator::Parsed {
        text: word.to_string(), description: None,
    }).collect()
}
