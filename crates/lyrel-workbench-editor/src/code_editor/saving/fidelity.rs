//! Capture one immutable document revision before scheduling Host I/O.
use super::*;

pub struct SaveContent {
    pub bytes: Vec<u8>,
    pub written: Fingerprint,
    pub format: TextFormat,
    pub representation: Option<u64>,
    pub revision: Option<u64>,
}
impl SaveContent {
    pub fn capture(
        file: &OpenFile,
        cx: &gpui_kit::App,
    ) -> Result<Self, lyrel_document::text::EncodeError> {
        if let Some(document) = file.input.source_document(cx) {
            // Source, every terminator, encoding and BOM come from this same snapshot;
            // never read a presentation's possibly stale cached text to acknowledge a save.
            let document = document.read(cx);
            let snapshot = document.snapshot();
            Ok(Self {
                bytes: snapshot.encode()?,
                written: Fingerprint::of_str(snapshot.text()),
                format: snapshot.format(),
                representation: Some(snapshot.representation_fingerprint()),
                revision: Some(document.revision()),
            })
        } else if let Some(document) = file.input.drawio_document(cx) {
            // The engine already serialized: its last committed bytes are the
            // document, fingerprinted once at capture so a later commit can
            // never silently pass as saved.
            let bytes = document
                .bytes()
                .expect("a savable diagram has ready bytes")
                .to_vec();
            let written = Fingerprint::of_bytes(&bytes);
            let revision = Some(document.edit_revision());
            Ok(Self {
                bytes,
                written,
                format: TextFormat::default(),
                representation: None,
                revision,
            })
        } else {
            // Only a text document reaches save: a read-only picture never
            // gets here, so text() and format() are the same capability.
            let text = file
                .input
                .text()
                .expect("save only runs on text documents")
                .read(cx)
                .text()
                .to_string();
            let text = if text.contains("\r\n") {
                text.replace("\r\n", "\n")
            } else {
                text
            };
            let format = file.format(cx).expect("save only runs on text documents");
            Ok(Self {
                bytes: lyrel_document::text::encode(&text, &format)?,
                written: Fingerprint::of_str(&text),
                format,
                representation: None,
                revision: None,
            })
        }
    }
}
