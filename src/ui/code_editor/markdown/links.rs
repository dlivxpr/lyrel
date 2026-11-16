//! Lyrel-owned ordinary file/URL navigation. Notebook/wiki constructs stay source-only.
use super::super::*;
use lyrel_core::host::Host;
use lyrel_markdown_editor::EditorState as MarkdownEditor;
use std::io;

pub(super) enum Target {
    External(url::Url),
    File(PathBuf, Option<String>),
    Anchor(String),
}

pub(super) fn parent(host: &dyn Host, path: &Path) -> io::Result<PathBuf> {
    if host.separator() == '/' {
        let path = path.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                t(L10nKey::MarkdownLinkInvalidDestination),
            )
        })?;
        let (parent, _) = path.rsplit_once('/').ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                t(L10nKey::MarkdownLinkSaveFirst),
            )
        })?;
        Ok(PathBuf::from(if parent.is_empty() { "/" } else { parent }))
    } else {
        path.parent().map(Path::to_path_buf).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                t(L10nKey::MarkdownLinkSaveFirst),
            )
        })
    }
}

pub(super) fn resolve(
    host: &dyn Host,
    document: Option<&Path>,
    raw: &str,
) -> Result<Target, L10nKey> {
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().any(char::is_control) {
        return Err(L10nKey::MarkdownLinkInvalidDestination);
    }
    if let Some(anchor) = raw.strip_prefix('#') {
        return Ok(Target::Anchor(decode(anchor)?));
    }
    // A drive-letter path is not a URL scheme on a Windows owning Host.
    let windows_path = host.separator() == '\\' && host.is_absolute(Path::new(raw));
    if !windows_path && let Ok(url) = url::Url::parse(raw) {
        match url.scheme() {
            "http" | "https"
                if url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none() =>
            {
                return Ok(Target::External(url));
            }
            "mailto" => return Ok(Target::External(url)),
            "file"
                if host.id().is_local()
                    && url.host_str().is_none_or(|host| host == "localhost") =>
            {
                let path = url
                    .to_file_path()
                    .map_err(|_| L10nKey::MarkdownLinkInvalidDestination)?;
                return Ok(Target::File(path, url.fragment().map(decode).transpose()?));
            }
            _ => return Err(L10nKey::MarkdownLinkUnsupportedScheme),
        }
    }
    if raw.starts_with("//") || raw.starts_with("\\\\") {
        return Err(L10nKey::MarkdownLinkUnsupportedScheme);
    }
    let (path, anchor) = raw
        .split_once('#')
        .map_or((raw, None), |(path, fragment)| (path, Some(fragment)));
    let path = decode(path)?;
    if host.separator() == '/' && (path.contains('\\') || path.contains(':')) {
        return Err(L10nKey::MarkdownLinkRemotePath);
    }
    let path = if host.is_absolute(Path::new(&path)) {
        PathBuf::from(path)
    } else {
        let document = document.ok_or(L10nKey::MarkdownLinkSaveFirst)?;
        let dir = parent(host, document).map_err(|_| L10nKey::MarkdownLinkInvalidDestination)?;
        host.join(&dir, &path)
    };
    Ok(Target::File(path, anchor.map(decode).transpose()?))
}
fn decode(value: &str) -> Result<String, L10nKey> {
    let value = percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .map_err(|_| L10nKey::MarkdownLinkInvalidDestination)?
        .into_owned();
    if value.chars().any(char::is_control) {
        return Err(L10nKey::MarkdownLinkInvalidDestination);
    }
    Ok(value)
}

fn anchor_position(source: &str, anchor: &str) -> Option<Position> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut fence: Option<(char, usize)> = None;
    for (row, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        let first = trimmed.chars().next();
        if matches!(first, Some('`' | '~')) {
            let ch = first.unwrap();
            let count = trimmed.chars().take_while(|c| *c == ch).count();
            if count >= 3 {
                match fence {
                    None => fence = Some((ch, count)),
                    Some((c, n)) if ch == c && count >= n => fence = None,
                    _ => {}
                }
                continue;
            }
        }
        if fence.is_some() {
            continue;
        }
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        if !(1..=6).contains(&level) || !trimmed[level..].starts_with(char::is_whitespace) {
            continue;
        }
        let title = trimmed[level..].trim().trim_end_matches('#').trim_end();
        let slug: String = title
            .chars()
            .flat_map(char::to_lowercase)
            .filter_map(|ch| {
                if ch.is_whitespace() {
                    Some('-')
                } else if ch.is_alphanumeric() || ch == '-' || ch == '_' {
                    Some(ch)
                } else {
                    None
                }
            })
            .collect();
        let count = seen.entry(slug.clone()).or_default();
        let unique = if *count == 0 {
            slug
        } else {
            format!("{slug}-{count}")
        };
        *count += 1;
        if unique == anchor {
            return Some(Position::new(row as u32, 0));
        }
    }
    None
}

pub(super) fn follow(
    app: &mut LyrelApp,
    id: BufferId,
    source: &Entity<MarkdownEditor>,
    raw: &str,
    window: &mut Window,
    cx: &mut Context<LyrelApp>,
) {
    let Some(file) = app.editor.buffers.get(id) else {
        return;
    };
    let host = file.host.clone();
    let path = file.untitled.is_none().then(|| file.path.clone());
    let revision = source.read(cx).document().read(cx).revision();
    let target = match resolve(&*host, path.as_deref(), raw) {
        Ok(target) => target,
        Err(reason) => {
            window.notify_app(
                t_fmt(L10nKey::MarkdownLinkOpenFailed, &[("reason", t(reason))]),
                cx,
            );
            return;
        }
    };
    let (path, anchor) = match target {
        Target::External(url) => {
            cx.open_url(url.as_str());
            return;
        }
        Target::Anchor(anchor) => {
            let position = anchor_position(source.read(cx).text(), &anchor);
            if let Some(position) = position {
                let offset = source
                    .read(cx)
                    .offset_for_line_column(position.line as usize, position.character as usize);
                source.update(cx, |view, cx| view.select_range(offset..offset, cx));
            } else {
                window.notify_app(t(L10nKey::MarkdownLinkAnchorNotFound), cx);
            }
            return;
        }
        Target::File(path, anchor) => (path, anchor),
    };
    let Some((tab, group)) = app.tabs.iter().find_map(|tab| {
        let code = tab.code.as_ref()?;
        std::iter::once(code.group)
            .chain(code.split.as_ref().map(|other| other.id))
            .find_map(|group| {
                file.presentations
                    .markdown_existing(tab.id(), group)
                    .filter(|view| view == source)
                    .map(|_| (tab.id(), group))
            })
    }) else {
        return;
    };
    let target = buffers::OpenTarget {
        workspace: app.workspace,
        tab,
        group: Some(group),
        window_host: app.spawn_host(cx),
        origin: Some((id, revision)),
        origin_epoch: app.editor_resource_epoch(id, cx),
    };
    if let Some(anchor) = anchor {
        let requested = path.clone();
        let destination_host = host.clone();
        // Decode on the same owning Host, never by opening a local lookalike.
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| -> io::Result<Position> {
                let bytes = h.read_file(&requested, 4 * 1024 * 1024)?;
                let source = lyrel_text::decode_source(&bytes)?.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        t(L10nKey::MarkdownLinkInvalidDestination),
                    )
                })?;
                anchor_position(&source.text, &anchor).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        t(L10nKey::MarkdownLinkAnchorNotFound),
                    )
                })
            },
            move |app, result, window, cx| {
                if !app.editor_open_target_valid(target, cx) {
                    return;
                }
                match result {
                    Ok(cursor) => open(
                        app,
                        destination_host,
                        &path,
                        Some(cursor),
                        target,
                        window,
                        cx,
                    ),
                    Err(error) => {
                        HostOps::notify_err(window, cx, t(L10nKey::MarkdownFollowLink), &error)
                    }
                }
            },
        );
    } else {
        open(app, host, &path, None, target, window, cx);
    }
}

fn open(
    app: &mut LyrelApp,
    host: SharedHost,
    path: &Path,
    cursor: Option<Position>,
    target: buffers::OpenTarget,
    window: &mut Window,
    cx: &mut Context<LyrelApp>,
) {
    if !app.editor_open_target_valid(target, cx) {
        return;
    }
    let reused = app.editor.buffers.open(
        host,
        path,
        cursor,
        target,
        window,
        cx,
        |app| &mut app.editor.buffers,
        EditorPanelState::buffer_edited,
        LyrelApp::editor_open_target_valid,
        LyrelApp::editor_opened,
    );
    if let Some(id) = reused {
        app.editor_show_open_target(target, id, true, window, cx);
    }
}
