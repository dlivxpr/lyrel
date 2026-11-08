//! Resource creation for the native Markdown input surface. Destination I/O
//! always uses the captured document Host; local reads are only drop sources.
use super::super::resources::ResourceContext;
use super::*;
use crate::ui::host_ops::{Host, HostOps};
use crate::ui::i18n::{L10nKey, t};
use std::io;
use std::path::{Path, PathBuf};

const IMAGE_MAX: u64 = 64 * 1024 * 1024;

pub(super) enum IncomingImage {
    Clipboard(Image),
    File(PathBuf),
}

#[derive(Default)]
pub(super) struct InsertionState {
    serial: u64,
    pending: Option<PendingInsertion>,
    error: Option<SharedString>,
}
struct PendingInsertion {
    serial: u64,
    token: (EntityId, u64, u64, u64, Option<EntityId>),
    selection: Selection,
    inputs: Option<Vec<IncomingImage>>,
    cancelled: std::rc::Rc<std::cell::Cell<bool>>,
}
impl InsertionState {
    pub(super) fn status(&self) -> Option<SharedString> {
        self.error.clone().or_else(|| {
            self.pending
                .as_ref()
                .filter(|pending| !pending.cancelled.get())
                .map(|pending| {
                    t(if pending.inputs.is_some() {
                        L10nKey::MarkdownImageChooseLocation
                    } else {
                        L10nKey::MarkdownImageWriting
                    })
                    .into()
                })
        })
    }
}

impl SourcePresentation {
    pub(super) fn cancel_image_insertion(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(pending) = self.assets.pending.take() else {
            return false;
        };
        pending.cancelled.set(true);
        cx.notify();
        true
    }
    pub(super) fn insert_images(
        &mut self,
        inputs: Vec<IncomingImage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .assets
            .pending
            .as_ref()
            .is_some_and(|pending| pending.cancelled.get())
        {
            self.assets.pending = None;
        }
        if inputs.is_empty() || self.assets.pending.is_some() {
            return;
        }
        self.command(Command::FinishComposition, window, cx);
        self.assets.serial = self.assets.serial.wrapping_add(1);
        self.assets.error = None;
        let cancelled = std::rc::Rc::new(std::cell::Cell::new(false));
        self.assets.pending = Some(PendingInsertion {
            serial: self.assets.serial,
            token: self.input_token(cx),
            selection: self.state.selection,
            inputs: Some(inputs),
            cancelled: cancelled.clone(),
        });
        let presentation = cx.entity().downgrade();
        self.document.update(cx, |_, cx| {
            cx.emit(DocumentEvent::RequestAssetLocation {
                presentation,
                cancelled,
            })
        });
        cx.notify();
    }

    /// Only the owning workbench's successful Saved event calls this for an
    /// untitled document. A chosen name alone is not a successful save.
    pub(crate) fn resume_pending_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(resources) = self.document.read(cx).resources().cloned() else {
            return;
        };
        if resources.path().is_none() {
            return;
        }
        if self
            .assets
            .pending
            .as_ref()
            .is_some_and(|pending| pending.token != self.input_token(cx))
        {
            self.cancel_image_insertion(cx);
            return;
        }
        let Some(pending) = self.assets.pending.as_mut() else {
            return;
        };
        if pending.cancelled.get() {
            self.assets.pending = None;
            return;
        }
        let Some(inputs) = pending.inputs.take() else {
            return;
        };
        let serial = pending.serial;
        let host = resources.host().clone();
        let destination = resources.clone();
        HostOps::run_in(
            host,
            window,
            cx,
            move |host| create_images(host, &destination, inputs),
            move |view, result, window, cx| {
                if !view
                    .assets
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.serial == serial)
                {
                    return;
                }
                let pending = view.assets.pending.take().unwrap();
                if pending.cancelled.get()
                    || pending.token != view.input_token(cx)
                    || !view.focus.is_focused(window)
                    || !resources.is_current(view.document.read(cx).resources())
                {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(mut source) => {
                        let document = view.document.read(cx);
                        let start = document.source().line_start_offset(
                            document
                                .source()
                                .offset_to_point(pending.selection.range().start)
                                .row,
                        );
                        let prefix = document
                            .source()
                            .slice(start..pending.selection.range().start);
                        let suffix = document
                            .source()
                            .slice(pending.selection.range().end..document.source().len());
                        if prefix.chars().all(char::is_whitespace)
                            && suffix
                                .chars()
                                .take_while(|ch| *ch != '\n')
                                .all(char::is_whitespace)
                        {
                            source.push('\n');
                        }
                        let caret = pending.selection.range().start + source.len();
                        view.command(
                            Command::BatchReplaceSelected {
                                edits: vec![(pending.selection.range(), source)],
                                selection: Selection::caret(caret),
                            },
                            window,
                            cx,
                        );
                    }
                    Err(error) => {
                        view.assets.error = Some(
                            format!("{}: {error}", t(L10nKey::MarkdownImageWriteFailed)).into(),
                        );
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    pub(super) fn drop_images(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inputs = paths
            .paths()
            .iter()
            .filter(|path| image_extension(path).is_some())
            .cloned()
            .map(IncomingImage::File)
            .collect();
        self.focus.focus(window, cx);
        self.insert_images(inputs, window, cx);
    }
}

pub(super) fn is_image_path(path: &Path) -> bool {
    image_extension(path).is_some()
}

fn image_extension(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("png"),
        "jpg" | "jpeg" => Some("jpg"),
        "gif" => Some("gif"),
        "webp" => Some("webp"),
        "svg" => Some("svg"),
        "bmp" => Some("bmp"),
        "tiff" | "tif" => Some("tiff"),
        "ico" => Some("ico"),
        "pnm" | "ppm" | "pgm" | "pbm" => Some("pnm"),
        _ => None,
    }
}

fn create_images(
    host: &dyn Host,
    resources: &ResourceContext,
    inputs: Vec<IncomingImage>,
) -> io::Result<String> {
    let path = resources
        .path()
        .ok_or_else(|| io::Error::other("Document has no saved location"))?;
    let directory = host.join(path.parent().unwrap_or_else(|| Path::new("")), "assets");
    host.create_dir(&directory, true)?;
    let mut links = Vec::with_capacity(inputs.len());
    for input in inputs {
        let (stem, extension, bytes) = match input {
            IncomingImage::Clipboard(image) => (
                "screenshot".to_string(),
                image.format.extension().to_string(),
                image.bytes,
            ),
            IncomingImage::File(path) => {
                let extension = image_extension(&path)
                    .ok_or_else(|| io::Error::other("Unsupported image format"))?
                    .to_string();
                let source = lyrel_core::host::local::LocalHost::new();
                let bytes = source.read_file(&path, IMAGE_MAX)?;
                let stem = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("image")
                    .chars()
                    .map(|ch| {
                        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                            ch
                        } else {
                            '-'
                        }
                    })
                    .collect::<String>();
                (
                    if stem.is_empty() {
                        "image".to_string()
                    } else {
                        stem
                    },
                    extension,
                    bytes,
                )
            }
        };
        if bytes.is_empty() || bytes.len() as u64 > IMAGE_MAX {
            return Err(io::Error::other("Empty image or image exceeds 64 MiB"));
        }
        // Exclusive creation, never an exists-then-write check. Only the path
        // successfully reserved by this operation is subsequently written.
        let mut reserved = None;
        for suffix in 0..10_000 {
            let filename = if suffix == 0 {
                format!("{stem}.{extension}")
            } else {
                format!("{stem}-{suffix}.{extension}")
            };
            let target = host.join(&directory, &filename);
            match host.create_file_new(&target) {
                Ok(()) => {
                    reserved = Some((target, filename));
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        let (target, filename) =
            reserved.ok_or_else(|| io::Error::other("No unused image filename"))?;
        host.write_file(&target, &bytes)?;
        links.push(format!("![{stem}](assets/{filename})"));
    }
    Ok(links.join("\n"))
}
