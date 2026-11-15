//! Deterministic, self-made PDF fixtures for backend and workbench tests.
//!
//! Nothing here is downloaded, and no third-party font, image or fixture file
//! is used: pages are built from standard PDF syntax and the sole image is a
//! generated 16×16 RGB quadrant. The encryption helper writes a Standard
//! `/V 1 /R 2` RC4-40 file with a public test password; RC4 is checked against
//! its published known-answer before use. Available to this crate's unit tests
//! and, through the `test-fixtures` feature, to the workbench's integration
//! tests. It is not compiled into the released library.

#[cfg(test)]
use std::collections::HashMap;

/// Public test value, not a user credential.
pub const TEST_PASSWORD: &str = "fixture-user-279";
pub const WRONG_PASSWORD: &str = "not-the-fixture-password";

const PADDING: [u8; 32] = [
    0x28, 0xbf, 0x4e, 0x5e, 0x4e, 0x75, 0x8a, 0x41, 0x64, 0x00, 0x4e, 0x56, 0xff, 0xfa, 0x01, 0x08,
    0x2e, 0x2e, 0x00, 0xb6, 0xd0, 0x68, 0x3e, 0x80, 0x2f, 0x0c, 0xa9, 0xfe, 0x64, 0x53, 0x69, 0x7a,
];

struct Builder {
    objects: Vec<Option<Vec<u8>>>,
}

impl Builder {
    fn new() -> Self {
        Self {
            objects: Vec::new(),
        }
    }

    fn reserve(&mut self) -> usize {
        self.objects.push(None);
        self.objects.len()
    }

    fn set(&mut self, id: usize, value: impl Into<Vec<u8>>) {
        self.objects[id - 1] = Some(value.into());
    }

    fn add(&mut self, value: impl Into<Vec<u8>>) -> usize {
        let id = self.reserve();
        self.set(id, value);
        id
    }

    fn stream(&mut self, dictionary: &str, bytes: &[u8]) -> usize {
        let mut body = format!("<< {dictionary} /Length {} >>\nstream\n", bytes.len()).into_bytes();
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\nendstream");
        self.add(body)
    }

    fn serialize(&self, root: usize, encryption: Option<(usize, [u8; 16])>) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let mut offsets = Vec::with_capacity(self.objects.len());
        for (index, object) in self.objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            out.extend_from_slice(object.as_ref().expect("assigned object"));
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", self.objects.len() + 1).as_bytes(),
        );
        for offset in &offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        let trailer = match encryption {
            Some((id, file_id)) => format!(
                "trailer\n<< /Size {} /Root {root} 0 R /Encrypt {id} 0 R /ID [<{}> <{}>] >>\n",
                self.objects.len() + 1,
                hex(&file_id),
                hex(&file_id)
            ),
            None => format!(
                "trailer\n<< /Size {} /Root {root} 0 R >>\n",
                self.objects.len() + 1
            ),
        };
        out.extend_from_slice(trailer.as_bytes());
        out.extend_from_slice(format!("startxref\n{xref}\n%%EOF\n").as_bytes());
        out
    }

    /// Encrypt every string/stream in place with the Standard R2 algorithm,
    /// append `/Encrypt`, and serialize. This intentionally mutates the
    /// in-memory builder; the caller drops it afterward.
    fn finish_encrypted(mut self, root: usize, user: &str, owner: &str) -> Vec<u8> {
        let original = self.serialize(root, None);
        let file_id: [u8; 16] = md5(&original);
        let padded_user = padded(user);
        let padded_owner = padded(owner);
        let owner_key = &md5(&padded_owner)[..5];
        let owner_value = rc4(owner_key, &padded_user);
        let mut key_input = padded_user.clone();
        key_input.extend_from_slice(&owner_value);
        key_input.extend_from_slice(&(-4i32).to_le_bytes());
        key_input.extend_from_slice(&file_id);
        let key = &md5(&key_input)[..5];
        let user_value = rc4(key, &PADDING);
        for (index, object) in self.objects.iter_mut().enumerate() {
            let object_number = (index + 1) as u32;
            let mut key_input = key.to_vec();
            key_input.extend_from_slice(&object_number.to_le_bytes()[..3]);
            key_input.extend_from_slice(&0u16.to_le_bytes());
            let object_key = &md5(&key_input)[..10];
            let body = object.take().expect("assigned object");
            *object = Some(encrypt_object(&body, object_key));
        }
        let encrypt_id = self.add(format!(
            "<< /Filter /Standard /V 1 /R 2 /Length 40 /O <{}> /U <{}> /P -4 >>",
            hex(&owner_value),
            hex(&user_value)
        ));
        self.serialize(root, Some((encrypt_id, file_id)))
    }
}

fn padded(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = bytes[..bytes.len().min(32)].to_vec();
    out.extend_from_slice(&PADDING);
    out.truncate(32);
    out
}

fn md5(bytes: &[u8]) -> [u8; 16] {
    md5::compute(bytes).0
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn rc4(key: &[u8], bytes: &[u8]) -> Vec<u8> {
    assert!(!key.is_empty());
    let mut state = [0u8; 256];
    for (index, value) in state.iter_mut().enumerate() {
        *value = index as u8;
    }
    let mut j = 0usize;
    for i in 0..256 {
        j = (j + usize::from(state[i]) + usize::from(key[i % key.len()])) & 255;
        state.swap(i, j);
    }
    let (mut i, mut j) = (0usize, 0usize);
    let mut out = Vec::with_capacity(bytes.len());
    for byte in bytes {
        i = (i + 1) & 255;
        j = (j + usize::from(state[i])) & 255;
        state.swap(i, j);
        let index = (usize::from(state[i]) + usize::from(state[j])) & 255;
        out.push(byte ^ state[index]);
    }
    out
}

fn encrypt_object(body: &[u8], object_key: &[u8]) -> Vec<u8> {
    if let Some(stream_at) = find(body, b"\nstream\n") {
        let dict = encrypt_strings(&body[..stream_at], object_key);
        let stream_start = stream_at + b"\nstream\n".len();
        // The generated fixtures always put `/Length` before the stream.
        let length = stream_length(&body[..stream_at]);
        let data_end = (stream_start + length).min(body.len());
        let mut out = dict;
        out.extend_from_slice(b"\nstream\n");
        out.extend_from_slice(&rc4(object_key, &body[stream_start..data_end]));
        out.extend_from_slice(&body[data_end..]);
        out
    } else {
        encrypt_strings(body, object_key)
    }
}

fn stream_length(dict: &[u8]) -> usize {
    let text = String::from_utf8_lossy(dict);
    let Some(index) = text.find("/Length ") else {
        return 0;
    };
    text[index + 8..]
        .split_whitespace()
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

/// Rewrite literal and hex strings as encrypted hex strings.
fn encrypt_strings(bytes: &[u8], key: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 32);
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'(' => {
                let (value, end) = literal_string(bytes, index);
                out.push(b'<');
                out.extend_from_slice(hex(&rc4(key, &value)).as_bytes());
                out.push(b'>');
                index = end;
            }
            b'<' if bytes.get(index + 1) != Some(&b'<')
                && index
                    .checked_sub(1)
                    .and_then(|previous| bytes.get(previous))
                    != Some(&b'<') =>
            {
                let Some(end) = bytes[index + 1..].iter().position(|byte| *byte == b'>') else {
                    out.push(bytes[index]);
                    index += 1;
                    continue;
                };
                let end = index + 1 + end;
                let digits: Vec<u8> = bytes[index + 1..end]
                    .iter()
                    .copied()
                    .filter(|byte| !byte.is_ascii_whitespace())
                    .collect();
                let mut value = Vec::with_capacity(digits.len() / 2 + 1);
                let mut pair = 0;
                while pair < digits.len() {
                    let high = hex_digit(digits[pair]).unwrap_or(0);
                    let low = digits
                        .get(pair + 1)
                        .and_then(|byte| hex_digit(*byte))
                        .unwrap_or(0);
                    value.push((high << 4) | low);
                    pair += 2;
                }
                out.push(b'<');
                out.extend_from_slice(hex(&rc4(key, &value)).as_bytes());
                out.push(b'>');
                index = end + 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    out
}

fn literal_string(bytes: &[u8], start: usize) -> (Vec<u8>, usize) {
    let mut value = Vec::new();
    let mut depth = 1usize;
    let mut index = start + 1;
    while index < bytes.len() && depth > 0 {
        let byte = bytes[index];
        index += 1;
        match byte {
            b'\\' => {
                let Some(&next) = bytes.get(index) else { break };
                index += 1;
                match next {
                    b'n' => value.push(b'\n'),
                    b'r' => value.push(b'\r'),
                    b't' => value.push(b'\t'),
                    b'b' => value.push(0x08),
                    b'f' => value.push(0x0c),
                    b'(' | b')' | b'\\' => value.push(next),
                    b'\r' => {
                        if bytes.get(index) == Some(&b'\n') {
                            index += 1;
                        }
                    }
                    b'\n' => {}
                    b'0'..=b'7' => {
                        let mut digits = vec![next];
                        while digits.len() < 3 {
                            match bytes.get(index) {
                                Some(byte @ b'0'..=b'7') => {
                                    digits.push(*byte);
                                    index += 1;
                                }
                                _ => break,
                            }
                        }
                        let mut number = 0u16;
                        for digit in digits {
                            number = number * 8 + u16::from(digit - b'0');
                        }
                        value.push((number & 0xFF) as u8);
                    }
                    other => value.push(other),
                }
            }
            b'(' => {
                depth += 1;
                value.push(b'(');
            }
            b')' => {
                depth -= 1;
                if depth > 0 {
                    value.push(b')');
                }
            }
            other => value.push(other),
        }
    }
    (value, index)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('(');
    for ch in value.chars() {
        if matches!(ch, '\\' | '(' | ')') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push(')');
    out
}

fn utf16_hex(value: &str) -> String {
    let mut bytes = vec![0xFE, 0xFF];
    for unit in value.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    format!("<{}>", hex(&bytes))
}

struct Base {
    builder: Builder,
    pages: usize,
    font: usize,
    page_ids: Vec<usize>,
    contents: Vec<usize>,
}

impl Base {
    fn new(pages: usize) -> Self {
        let mut builder = Builder::new();
        let pages_id = builder.reserve();
        let font = builder.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".as_bytes());
        Self {
            builder,
            pages: pages_id,
            font,
            page_ids: Vec::with_capacity(pages),
            contents: Vec::with_capacity(pages),
        }
    }

    fn page(&mut self, content: &str, extras: &str) -> usize {
        let content = self.builder.stream("", content.as_bytes());
        let id = self.builder.reserve();
        self.contents.push(content);
        self.page_ids.push(id);
        self.set_page(self.page_ids.len() - 1, extras);
        id
    }

    /// Rebuild one page dictionary; used when annotations are created after the
    /// page objects themselves.
    fn set_page(&mut self, index: usize, extras: &str) {
        let page = self.page_ids[index];
        let content = self.contents[index];
        self.builder.set(
            page,
            format!(
                "<< /Type /Page /Parent {} 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 {} 0 R >> >> /Contents {content} 0 R {extras} >>",
                self.pages, self.font
            )
            .into_bytes(),
        );
    }

    fn finish(self, catalog_extra: &str) -> Vec<u8> {
        let Self {
            mut builder,
            pages,
            page_ids,
            ..
        } = self;
        builder.set(
            pages,
            format!(
                "<< /Type /Pages /Count {} /Kids [{}] >>",
                page_ids.len(),
                page_ids
                    .iter()
                    .map(|id| format!("{id} 0 R"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        );
        let catalog = builder.reserve();
        builder.set(
            catalog,
            format!("<< /Type /Catalog /Pages {pages} 0 R {catalog_extra} >>"),
        );
        builder.serialize(catalog, None)
    }
}

/// Twelve pages with per-page markers and an outline whose destinations are an
/// explicit page, a `/Names /Dests` name tree and a legacy `/Dests` name.
pub fn reading_document() -> Vec<u8> {
    let mut base = Base::new(12);
    let mut page_ids = Vec::new();
    for number in 1..=12 {
        let content = format!(
            "0.1 0.4 0.8 rg 48 640 100 72 re f\n\
             BT /F1 20 Tf 0 0 0 rg 48 740 Td {} Tj 0 -32 Td /F1 12 Tf \
             (fidelity marker; common search target) Tj 0 -24 Td {} Tj ET",
            literal(&format!("lyrel #279 PDF page {number}")),
            literal(&format!("unique-page-{number:02}"))
        );
        let extras = if number == 3 { "/Rotate 90" } else { "" };
        page_ids.push(base.page(&content, extras));
    }
    let Base {
        mut builder,
        pages: pages_id,
        page_ids,
        ..
    } = base;
    let outlines = builder.reserve();
    let first = builder.reserve();
    let second = builder.reserve();
    let third = builder.reserve();
    builder.set(
        outlines,
        format!("<< /Type /Outlines /First {first} 0 R /Last {third} 0 R /Count 3 >>").into_bytes(),
    );
    builder.set(
        first,
        format!(
            "<< /Title (First explicit page) /Parent {outlines} 0 R /Next {second} 0 R \
             /Dest [{} 0 R /XYZ 0 792 null] >>",
            page_ids[0]
        )
        .into_bytes(),
    );
    builder.set(
        second,
        format!(
            "<< /Title (Middle named destination) /Parent {outlines} 0 R /Prev {first} 0 R \
             /Next {third} 0 R /Dest (middle-279) >>"
        )
        .into_bytes(),
    );
    builder.set(
        third,
        format!(
            "<< /Title (Last named action) /Parent {outlines} 0 R /Prev {second} 0 R \
             /A << /S /GoTo /D /last279 >> >>"
        )
        .into_bytes(),
    );
    builder.set(
        pages_id,
        format!(
            "<< /Type /Pages /Count {} /Kids [{}] >>",
            page_ids.len(),
            page_ids
                .iter()
                .map(|id| format!("{id} 0 R"))
                .collect::<Vec<_>>()
                .join(" ")
        )
        .into_bytes(),
    );
    let catalog = builder.reserve();
    builder.set(
        catalog,
        format!(
            "<< /Type /Catalog /Pages {pages_id} 0 R /Outlines {outlines} 0 R \
             /Names << /Dests << /Names [(middle-279) [{} 0 R /XYZ 0 680 null]] >> >> \
             /Dests << /last279 [{} 0 R /Fit] >> >>",
            page_ids[5], page_ids[11]
        )
        .into_bytes(),
    );
    builder.serialize(catalog, None)
}

/// One page whose first widget has an existing appearance, a check box whose
/// `/AP /N` is an `/AS` state dictionary, and an existing Text annotation.
/// When `missing_appearance` is true, the page instead carries two widgets with
/// UTF-16BE values, no `/AP`, and `NeedAppearances true`.
pub fn forms_document(missing_appearance: bool) -> Vec<u8> {
    let mut base = Base::new(1);
    let font = base.font;
    if missing_appearance {
        let (unicode, multiline) = {
            let builder = &mut base.builder;
            let unicode = builder.add(
                format!(
                    "<< /Type /Annot /Subtype /Widget /FT /Tx /T (unicode279) \
                     /V {} /Rect [48 600 360 644] /DA (/F1 18 Tf 0 g) /F 4 >>",
                    utf16_hex("中文表单279")
                )
                .into_bytes(),
            );
            let multiline = builder.add(
                format!(
                    "<< /Type /Annot /Subtype /Widget /FT /Tx /T (multiline279) \
                     /V {} /Rect [48 450 360 550] /DA (/F1 18 Tf 0 g) /Q 2 /Ff 4096 /F 4 >>",
                    utf16_hex("第一行\n第二行")
                )
                .into_bytes(),
            );
            (unicode, multiline)
        };
        base.page(
            "BT /F1 20 Tf 48 740 Td (No AP; Unicode fields must not disappear) Tj ET",
            &format!("/Annots [{unicode} 0 R {multiline} 0 R]"),
        );
        let Base {
            mut builder,
            pages: pages_id,
            page_ids,
            ..
        } = base;
        builder.set(
            pages_id,
            format!("<< /Type /Pages /Count 1 /Kids [{} 0 R] >>", page_ids[0]).into_bytes(),
        );
        let catalog = builder.reserve();
        builder.set(
            catalog,
            format!(
                "<< /Type /Catalog /Pages {pages_id} 0 R \
                 /AcroForm << /Fields [{unicode} 0 R {multiline} 0 R] /NeedAppearances true \
                 /DA (/F1 18 Tf 0 g) /DR << /Font << /F1 {font} 0 R >> >> >> >>"
            )
            .into_bytes(),
        );
        return builder.serialize(catalog, None);
    }

    let (field, check, annotation) = {
        let builder = &mut base.builder;
        let appearance = builder.stream(
            &format!(
                "/Type /XObject /Subtype /Form /BBox [0 0 240 40] \
                 /Resources << /Font << /F1 {font} 0 R >> >>"
            ),
            b"0.85 0.9 1 rg 0 0 240 40 re f BT /F1 16 Tf 0 0 0.8 rg 8 12 Td (FILLED AP 279) Tj ET",
        );
        let yes = builder.stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 24 24]",
            b"0 0.6 0 rg 0 0 24 24 re f 1 1 1 RG 3 w 4 12 m 10 5 l 20 20 l S",
        );
        let off = builder.stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 24 24]",
            b"0.6 0.6 0.6 RG 1 w 1 1 22 22 re S",
        );
        let note = builder.stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 32 32]",
            b"1 0.5 0 rg 0 0 32 32 re f",
        );
        let field = builder.add(
            format!(
                "<< /Type /Annot /Subtype /Widget /FT /Tx /T (filled279) /V (FILLED AP 279) \
                 /Rect [48 600 288 640] /F 4 /AP << /N {appearance} 0 R >> >>"
            )
            .into_bytes(),
        );
        let check = builder.add(
            format!(
                "<< /Type /Annot /Subtype /Widget /FT /Btn /T (check279) /V /Yes /AS /Yes \
                 /Rect [48 540 72 564] /F 4 \
                 /AP << /N << /Yes {yes} 0 R /Off {off} 0 R >> >> >>"
            )
            .into_bytes(),
        );
        let annotation = builder.add(
            format!(
                "<< /Type /Annot /Subtype /Text /Rect [320 600 352 632] \
                 /Contents (Owned note appearance) /F 4 /AP << /N {note} 0 R >> >>"
            )
            .into_bytes(),
        );
        (field, check, annotation)
    };
    base.page(
        "BT /F1 20 Tf 48 740 Td (Owned existing appearance fixture) Tj ET",
        &format!("/Annots [{field} 0 R {check} 0 R {annotation} 0 R]"),
    );
    let Base {
        mut builder,
        pages: pages_id,
        page_ids,
        ..
    } = base;
    builder.set(
        pages_id,
        format!("<< /Type /Pages /Count 1 /Kids [{} 0 R] >>", page_ids[0]).into_bytes(),
    );
    let catalog = builder.reserve();
    builder.set(
        catalog,
        format!(
            "<< /Type /Catalog /Pages {pages_id} 0 R \
             /AcroForm << /Fields [{field} 0 R {check} 0 R] /NeedAppearances false \
             /DR << /Font << /F1 {font} 0 R >> >> >> >>"
        )
        .into_bytes(),
    );
    builder.serialize(catalog, None)
}

/// One page with a generated 16×16 RGB image and no text operators.
pub fn scan_only_document() -> Vec<u8> {
    let mut builder = Builder::new();
    let pages = builder.reserve();
    let page = builder.reserve();
    let mut image = vec![0u8; 16 * 16 * 3];
    for y in 0..16 {
        for x in 0..16 {
            let offset = (y * 16 + x) * 3;
            image[offset] = if x < 8 { 255 } else { 0 };
            image[offset + 1] = if y < 8 { 180 } else { 0 };
            image[offset + 2] = if x >= 8 { 255 } else { 0 };
        }
    }
    let resource = builder.stream(
        "/Type /XObject /Subtype /Image /Width 16 /Height 16 \
         /ColorSpace /DeviceRGB /BitsPerComponent 8",
        &image,
    );
    let content = builder.stream("", b"q 480 0 0 480 48 160 cm /Im0 Do Q");
    builder.set(
        page,
        format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /XObject << /Im0 {resource} 0 R >> >> /Contents {content} 0 R >>"
        )
        .into_bytes(),
    );
    builder.set(
        pages,
        format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>").into_bytes(),
    );
    let catalog = builder.reserve();
    builder.set(
        catalog,
        format!("<< /Type /Catalog /Pages {pages} 0 R >>").into_bytes(),
    );
    builder.serialize(catalog, None)
}

/// Type3 glyphs mapped through `/ToUnicode` to `中文 中文` and `中文`.
pub fn unicode_text_document() -> Vec<u8> {
    let mut builder = Builder::new();
    let pages = builder.reserve();
    let page = builder.reserve();
    let glyph_a = builder.stream("", b"600 0 0 0 500 700 d1 0 0 500 700 re f");
    let glyph_b = builder.stream("", b"600 0 0 0 500 700 d1 0 0 m 500 0 l 250 700 l h f");
    let space = builder.stream("", b"300 0 d0");
    let cmap = builder.stream(
        "",
        b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap \
          /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def \
          /CMapName /Owned279 def /CMapType 2 def \
          1 begincodespacerange <00> <FF> endcodespacerange \
          3 beginbfchar <20> <0020> <41> <4E2D> <42> <6587> endbfchar endcmap \
          CMapName currentdict /CMap defineresource pop end end",
    );
    let widths = (0..35)
        .map(|index| if index == 0 { "300" } else { "600" })
        .collect::<Vec<_>>()
        .join(" ");
    let font = builder.add(
        format!(
            "<< /Type /Font /Subtype /Type3 /Name /F279 /FontBBox [0 0 500 700] \
             /FontMatrix [0.001 0 0 0.001 0 0] \
             /CharProcs << /A {glyph_a} 0 R /B {glyph_b} 0 R /space {space} 0 R >> \
             /Encoding << /Type /Encoding /Differences [32 /space 65 /A /B] >> \
             /FirstChar 32 /LastChar 66 /Widths [{widths}] /Resources << >> \
             /ToUnicode {cmap} 0 R >>"
        )
        .into_bytes(),
    );
    let content = builder.stream(
        "",
        b"BT /F279 24 Tf 48 700 Td (AB AB) Tj 0 -48 Td (AB) Tj ET",
    );
    builder.set(
        page,
        format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F279 {font} 0 R >> >> /Contents {content} 0 R >>"
        )
        .into_bytes(),
    );
    builder.set(
        pages,
        format!("<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>").into_bytes(),
    );
    let catalog = builder.reserve();
    builder.set(
        catalog,
        format!("<< /Type /Catalog /Pages {pages} 0 R >>").into_bytes(),
    );
    builder.serialize(catalog, None)
}

/// One page with one glyph run inside a clip rectangle and one outside.
pub fn clipped_text_document() -> Vec<u8> {
    let mut base = Base::new(1);
    base.page(
        "q 0 0 300 300 re W n \
         BT /F1 14 Tf 60 200 Td (inside clip) Tj 340 0 Td (outside clip) Tj ET Q",
        "",
    );
    base.finish("")
}

/// Two text runs drawn out of visual order on the same baseline.
pub fn drawing_order_document() -> Vec<u8> {
    let mut base = Base::new(1);
    base.page(
        "BT /F1 20 Tf 150 700 Td (World) Tj ET \
         BT /F1 20 Tf 48 700 Td (Hello) Tj ET",
        "",
    );
    base.finish("")
}

/// Two pages with a rotated internal link and a safe external URI.
pub fn links_document() -> Vec<u8> {
    let mut base = Base::new(2);
    base.page("BT /F1 20 Tf 48 740 Td (Rotated link page) Tj ET", "");
    base.page("BT /F1 20 Tf 48 740 Td (Destination page) Tj ET", "");
    let page_two = base.page_ids[1];
    let (internal, external, trailer) = {
        let builder = &mut base.builder;
        let internal = builder.add(
            format!(
                "<< /Type /Annot /Subtype /Link /Rect [48 600 288 640] \
                 /Dest [{page_two} 0 R /XYZ 0 792 null] >>"
            )
            .into_bytes(),
        );
        let external = builder.add(
            "<< /Type /Annot /Subtype /Link /Rect [48 500 288 540] \
             /A << /S /URI /URI (https://example.com/lyrel-279) >> >>"
                .as_bytes()
                .to_vec(),
        );
        let trailer = builder.add(
            "<< /Type /Annot /Subtype /Link /Dest (second-279) \
             /Rect [48 600 288 640] >>"
                .as_bytes()
                .to_vec(),
        );
        (internal, external, trailer)
    };
    base.set_page(
        0,
        &format!("/Annots [{internal} 0 R {external} 0 R] /Rotate 90"),
    );
    base.set_page(1, &format!("/Annots [{trailer} 0 R]"));
    base.finish(&format!(
        "/Names << /Dests << /Names [(second-279) [{page_two} 0 R /Fit]] >> >>"
    ))
}

/// Encrypt the reading fixture with the public Standard R2 test password.
pub fn encrypted_reading_document() -> Vec<u8> {
    encrypt_serialized(reading_document(), TEST_PASSWORD)
}

/// Encrypt the missing-appearance form fixture; the decrypted field values and
/// geometry must still be available.
pub fn encrypted_forms_document() -> Vec<u8> {
    encrypt_serialized(forms_document(true), TEST_PASSWORD)
}

/// Re-build a serialized document with our own encryption helper. The input is
/// one of this module's files, so its simple xref layout is the one we wrote.
fn encrypt_serialized(original: Vec<u8>, user: &str) -> Vec<u8> {
    let mut builder = Builder::new();
    let xref_at = find(&original, b"\nxref\n").expect("fixture xref") + 1;
    // Only the xref/trailer is ASCII. Byte offsets refer to the original PDF,
    // not a lossy UTF-8 rendering of its binary header and object streams.
    let xref = std::str::from_utf8(&original[xref_at..]).expect("ASCII xref");
    let mut lines = xref.lines();
    assert_eq!(lines.next(), Some("xref"));
    let mut header = lines.next().expect("xref header").split_whitespace();
    assert_eq!(header.next(), Some("0"));
    let count: usize = header.next().expect("xref count").parse().unwrap();
    lines.next().expect("free object zero");
    let offsets: Vec<usize> = lines
        .take(count - 1)
        .map(|line| line[..10].parse().expect("xref byte offset"))
        .collect();
    let root: usize = xref
        .split("/Root ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse().ok())
        .unwrap();
    for (index, offset) in offsets.iter().enumerate() {
        let end = offsets.get(index + 1).copied().unwrap_or(xref_at);
        let body = &original[*offset..end];
        let prefix = format!("{} 0 obj\n", index + 1);
        let body = body.strip_prefix(prefix.as_bytes()).expect("object prefix");
        let object_end = find(body, b"\nendobj").expect("object end");
        builder.add(body[..object_end].to_vec());
    }
    builder.finish_encrypted(root, user, "fixture-owner-279")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc4_matches_the_published_known_answer() {
        assert_eq!(
            hex(&rc4(b"Key", b"Plaintext")),
            "bbf316e8d940af0ad3".to_string()
        );
    }

    #[test]
    fn generated_fixtures_keep_their_expected_shapes() {
        let files: HashMap<&str, Vec<u8>> = [
            ("reading", reading_document()),
            ("forms", forms_document(false)),
            ("missing", forms_document(true)),
            ("scan", scan_only_document()),
            ("unicode", unicode_text_document()),
            ("clipped", clipped_text_document()),
            ("order", drawing_order_document()),
            ("links", links_document()),
        ]
        .into_iter()
        .collect();
        for (name, bytes) in &files {
            assert!(bytes.starts_with(b"%PDF-1.7"), "{name}");
            assert!(bytes.ends_with(b"%%EOF\n"), "{name}");
        }
        let encrypted = encrypted_reading_document();
        assert!(encrypted.windows(9).any(|window| window == b"/Encrypt "));
        assert!(String::from_utf8_lossy(&encrypted).contains("/R 2"));
    }
}
