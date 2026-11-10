//! Capture one immutable document revision before scheduling Host I/O.
use super::*;

pub(super) struct SaveContent {
    pub(super) bytes: Vec<u8>,
    pub(super) written: Fingerprint,
    pub(super) format: TextFormat,
    pub(super) representation: Option<u64>,
    pub(super) revision: Option<u64>,
}
impl SaveContent {
    pub(super) fn capture(
        file: &OpenFile,
        cx: &gpui_kit::App,
    ) -> Result<Self, lyrel_text::EncodeError> {
        if let Some(view) = file.input.markdown() {
            // Source, every terminator, encoding and BOM come from this same snapshot;
            // never read a presentation's possibly stale cached text to acknowledge a save.
            let document = view.read(cx).document().read(cx);
            let snapshot = document.snapshot();
            Ok(Self {
                bytes: snapshot.encode()?,
                written: Fingerprint::of_str(snapshot.text()),
                format: snapshot.format(),
                representation: Some(snapshot.representation_fingerprint()),
                revision: Some(document.revision()),
            })
        } else {
            let input = file.input.read(cx);
            let text = input.text().to_string();
            let text = if text.contains("\r\n") {
                text.replace("\r\n", "\n")
            } else {
                text
            };
            let format = file.format(cx);
            Ok(Self {
                bytes: lyrel_text::encode(&text, &format)?,
                written: Fingerprint::of_str(&text),
                format,
                representation: None,
                revision: None,
            })
        }
    }
}
