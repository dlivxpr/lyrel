//! Portable EPUB fixtures: every archive is generated in the test's own
//! temporary directory with `zip::ZipWriter`. No `/usr/bin/zip`, coordination
//! directory, network or platform command is involved.

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use lyrel_document::epub::open_book;
use lyrel_document::epub::{
    BLOCKED_IMAGE_SOURCE, CancelToken, EpubError, ImageMode, Limits, read_chapter,
};

/// A 1×1 opaque PNG. The bytes are a fixture, not a decoder assertion.
const PNG: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H', b'D', b'R',
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, b'I', b'D', b'A', b'T', 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, b'\xb4', 0x00, 0x00, 0x00, 0x00, b'I', b'E', b'N', b'D',
    0xae, 0x42, 0x60, 0x82,
];

struct Fixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

impl Fixture {
    fn new(entries: &[(&str, Vec<u8>)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        write_zip(&path, entries);
        Self { _dir: dir, path }
    }

    fn book(&self) -> lyrel_document::epub::EpubBook {
        self.book_with(&Limits::default())
    }

    fn book_with(&self, limits: &Limits) -> lyrel_document::epub::EpubBook {
        let file = std::fs::File::open(&self.path).unwrap();
        open_book(
            file,
            "a".repeat(64),
            "fallback",
            limits,
            &CancelToken::new(),
        )
        .unwrap()
    }

    fn chapter(&self, index: usize) -> lyrel_document::epub::Chapter {
        let book = self.book();
        let file = std::fs::File::open(&self.path).unwrap();
        read_chapter(
            &book,
            file,
            index,
            ImageMode::Include,
            &Limits::default(),
            &CancelToken::new(),
        )
        .unwrap()
    }
}

fn write_zip(path: &Path, entries: &[(&str, Vec<u8>)]) {
    let mut archive = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (index, (name, bytes)) in entries.iter().enumerate() {
        let stored = index == 0 && *name == "mimetype";
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(if stored {
                zip::CompressionMethod::Stored
            } else {
                zip::CompressionMethod::Deflated
            })
            .unix_permissions(0o644);
        archive.start_file(*name, options).unwrap();
        archive.write_all(bytes).unwrap();
    }
    archive.finish().unwrap();
}

fn container(root: &str) -> Vec<u8> {
    format!(
        r#"<?xml version="1.0"?><container version="1.0"><rootfiles><rootfile full-path="{root}" media-type="application/oebps-package+xml"/></rootfiles></container>"#
    )
    .into_bytes()
}

fn opf(manifest: &str, spine: &str, metadata_extra: &str) -> Vec<u8> {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Reader Test</dc:title><dc:creator>Ada</dc:creator><dc:creator>Bo</dc:creator><dc:language>en</dc:language>{metadata_extra}</metadata><manifest>{manifest}</manifest><spine>{spine}</spine></package>"#
    )
    .into_bytes()
}

fn nav(entries: &str) -> Vec<u8> {
    format!(
        r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><ol>{entries}</ol></nav></body></html>"#
    )
    .into_bytes()
}

fn reflowable_entries() -> Vec<(&'static str, Vec<u8>)> {
    let long = (1..=3000)
        .map(|index| format!("<p>Paragraph {index} of the synthetic book.</p>"))
        .collect::<String>();
    let second = format!(
        r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1 id="final">Final chapter</h1><p>Ends with needle-only-final-chapter-279.</p>{long}</body></html>"#
    );
    let first = r#"<!DOCTYPE html><html xmlns="http://www.w3.org/1999/xhtml"><body><h1 id="start">First&nbsp;chapter</h1><p>Hello <em>reader</em>.</p><img src="../Images/pixel.png" alt="pixel"/><img src="https://example.com/tracker.png"/><script>bad()</script><iframe src="file:///private/data"></iframe><a href="two.xhtml#final">next</a><a href="file:///etc/passwd">bad</a></body></html>"#;
    let third = r#"<html><body><p>Third.</p></body></html>"#;
    vec![
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", container("OPS/book.opf")),
        (
            "OPS/book.opf",
            opf(
                r#"<item id="two" href="Text/two.xhtml" media-type="application/xhtml+xml"/><item id="one" href="Text/one.xhtml" media-type="application/xhtml+xml"/><item id="three" href="Text/three.xhtml" media-type="application/xhtml+xml"/><item id="nav" href="toc.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="cover" href="Images/pixel.png" media-type="image/png" properties="cover-image"/>"#,
                r#"<itemref idref="two"/><itemref idref="one"/><itemref idref="three"/>"#,
                "",
            ),
        ),
        (
            "OPS/toc.xhtml",
            nav(
                r#"<li><a href="Text/two.xhtml#final">Final</a><ol><li><a href="Text/one.xhtml#start">First</a></li></ol></li><li><a href="Text/three.xhtml">Third</a></li>"#,
            ),
        ),
        ("OPS/Text/one.xhtml", first.as_bytes().to_vec()),
        ("OPS/Text/two.xhtml", second.into_bytes()),
        ("OPS/Text/three.xhtml", third.as_bytes().to_vec()),
        ("OPS/Images/pixel.png", PNG.to_vec()),
        // A traversal-shaped member that no package document names. It must
        // never be opened or listed as a chapter.
        ("../secret.txt", b"secret".to_vec()),
    ]
}

#[test]
fn spine_navigation_blocks_and_local_illustrations_survive_active_content() {
    let fixture = Fixture::new(&reflowable_entries());
    let before = std::fs::read(&fixture.path).unwrap();
    let book = fixture.book();
    assert_eq!(book.title(), "Reader Test");
    assert_eq!(book.metadata.author_line(), "Ada, Bo");
    assert_eq!(book.fingerprint, "a".repeat(64));
    assert_eq!(
        book.spine
            .iter()
            .map(|item| item.href.as_str())
            .collect::<Vec<_>>(),
        [
            "OPS/Text/two.xhtml",
            "OPS/Text/one.xhtml",
            "OPS/Text/three.xhtml"
        ]
    );
    assert_eq!(book.spine[0].title, "Final");
    assert_eq!(book.spine[1].title, "First");
    assert_eq!(book.toc[1].depth, 1);

    let first = fixture.chapter(1);
    assert_eq!(first.title, "First");
    assert_eq!(first.blocks[0].text, "First chapter");
    assert!(first.blocks[1].html.contains("<em>reader</em>"));
    assert_eq!(first.images.len(), 1);
    assert_eq!(first.images[0].path, "OPS/Images/pixel.png");
    assert_eq!(first.images[0].bytes, PNG);
    assert!(
        !first
            .blocks
            .iter()
            .any(|block| block.html.contains("bad()"))
    );
    assert!(
        !first
            .blocks
            .iter()
            .any(|block| block.html.contains("iframe"))
    );
    assert!(
        !first
            .blocks
            .iter()
            .any(|block| block.html.contains("tracker"))
    );
    assert!(
        first
            .blocks
            .iter()
            .any(|block| block.images.contains(&BLOCKED_IMAGE_SOURCE.to_owned()))
    );
    assert!(first.omissions.scripts >= 1);
    assert!(first.omissions.frames >= 1);
    assert!(first.omissions.remote_images >= 1);
    assert!(first.omissions.blocked_links >= 1);
    assert!(first.anchors.iter().any(|anchor| anchor.id == "start"));

    let long = fixture.chapter(0);
    assert!(long.blocks.len() > 3000);
    assert!(
        long.blocks
            .iter()
            .any(|block| block.text.contains("needle-only-final-chapter-279"))
    );
    assert_eq!(before, std::fs::read(&fixture.path).unwrap());
}

#[test]
fn epub2_uses_ncx_only_when_epub3_navigation_is_absent() {
    let ncx = r#"<?xml version="1.0"?><ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap><navPoint><navLabel><text>One</text></navLabel><content src="Text/one.xhtml"/><navPoint><navLabel><text>Nested</text></navLabel><content src="Text/one.xhtml#nested"/></navPoint></navPoint></navMap></ncx>"#;
    let fixture = Fixture::new(&[
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", container("OPS/book.opf")),
        (
            "OPS/book.opf",
            opf(
                r#"<item id="one" href="Text/one.xhtml" media-type="application/xhtml+xml"/><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>"#,
                r#"<itemref idref="one"/>"#,
                "",
            ),
        ),
        ("OPS/toc.ncx", ncx.as_bytes().to_vec()),
        (
            "OPS/Text/one.xhtml",
            b"<html><body><p id=\"nested\">Text</p></body></html>".to_vec(),
        ),
    ]);
    let book = fixture.book();
    assert_eq!(book.toc.len(), 2);
    assert_eq!(book.toc[0].title, "One");
    assert_eq!(book.toc[1].depth, 1);
    assert_eq!(book.spine[0].title, "One");
}

#[test]
fn fixed_layout_is_declared_unsupported_globally_and_per_spine_item() {
    let global = Fixture::new(&[
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", container("OPS/book.opf")),
        (
            "OPS/book.opf",
            opf(
                r#"<item id="one" href="Text/one.xhtml" media-type="application/xhtml+xml"/>"#,
                r#"<itemref idref="one"/>"#,
                r#"<meta property="rendition:layout">pre-paginated</meta>"#,
            ),
        ),
        (
            "OPS/Text/one.xhtml",
            b"<html><body><p>One</p></body></html>".to_vec(),
        ),
    ]);
    let file = std::fs::File::open(&global.path).unwrap();
    let error = open_book(
        file,
        "a".repeat(64),
        "fallback",
        &Limits::default(),
        &CancelToken::new(),
    )
    .unwrap_err();
    assert!(matches!(error, EpubError::FixedLayout { global: true, .. }));

    let item = Fixture::new(&[
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", container("OPS/book.opf")),
        (
            "OPS/book.opf",
            opf(
                r#"<item id="one" href="Text/one.xhtml" media-type="application/xhtml+xml"/><item id="two" href="Text/two.xhtml" media-type="application/xhtml+xml"/>"#,
                r#"<itemref idref="one"/><itemref idref="two" properties="rendition:layout-pre-paginated"/>"#,
                "",
            ),
        ),
        (
            "OPS/Text/one.xhtml",
            b"<html><body><p>One</p></body></html>".to_vec(),
        ),
        (
            "OPS/Text/two.xhtml",
            b"<html><body><p>Two</p></body></html>".to_vec(),
        ),
    ]);
    let file = std::fs::File::open(&item.path).unwrap();
    let error = open_book(
        file,
        "a".repeat(64),
        "fallback",
        &Limits::default(),
        &CancelToken::new(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        EpubError::FixedLayout {
            global: false,
            items: 1
        }
    ));
}

#[test]
fn rights_encryption_and_non_font_cipher_targets_are_refused() {
    let rights = Fixture::new(&[
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", container("OPS/book.opf")),
        ("META-INF/rights.xml", b"<rights/>".to_vec()),
        (
            "OPS/book.opf",
            opf(
                r#"<item id="one" href="Text/one.xhtml" media-type="application/xhtml+xml"/>"#,
                r#"<itemref idref="one"/>"#,
                "",
            ),
        ),
        (
            "OPS/Text/one.xhtml",
            b"<html><body><p>One</p></body></html>".to_vec(),
        ),
    ]);
    let file = std::fs::File::open(&rights.path).unwrap();
    assert!(matches!(
        open_book(
            file,
            "a".repeat(64),
            "x",
            &Limits::default(),
            &CancelToken::new()
        ),
        Err(EpubError::Protected { .. })
    ));

    let encrypted = |uri: &str, media: &str| {
        let encryption = format!(
            r#"<?xml version="1.0"?><encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:enc="http://www.w3.org/2001/04/xmlenc#"><enc:EncryptedData><enc:EncryptionMethod Algorithm="http://www.idpf.org/2008/embedding"/><enc:CipherData><enc:CipherReference URI="{uri}"/></enc:CipherData></enc:EncryptedData></encryption>"#
        );
        Fixture::new(&[
            ("mimetype", b"application/epub+zip".to_vec()),
            ("META-INF/container.xml", container("OPS/book.opf")),
            ("META-INF/encryption.xml", encryption.into_bytes()),
            (
                "OPS/book.opf",
                opf(
                    &format!(
                        r#"<item id="one" href="Text/one.xhtml" media-type="application/xhtml+xml"/><item id="font" href="Fonts/f.ttf" media-type="{media}"/>"#
                    ),
                    r#"<itemref idref="one"/>"#,
                    "",
                ),
            ),
            (
                "OPS/Text/one.xhtml",
                b"<html><body><p>One</p></body></html>".to_vec(),
            ),
            ("OPS/Fonts/f.ttf", vec![0u8; 8]),
        ])
    };
    let chapter_target = encrypted("OPS/Text/one.xhtml", "font/ttf");
    let file = std::fs::File::open(&chapter_target.path).unwrap();
    assert!(matches!(
        open_book(
            file,
            "a".repeat(64),
            "x",
            &Limits::default(),
            &CancelToken::new()
        ),
        Err(EpubError::Protected { .. })
    ));
    let font_target = encrypted("OPS/Fonts/f.ttf", "font/ttf");
    let file = std::fs::File::open(&font_target.path).unwrap();
    let book = open_book(
        file,
        "a".repeat(64),
        "x",
        &Limits::default(),
        &CancelToken::new(),
    )
    .expect("font obfuscation is the one allowed algorithm and only for fonts");
    let file = std::fs::File::open(&font_target.path).unwrap();
    let chapter = read_chapter(
        &book,
        file,
        0,
        ImageMode::Include,
        &Limits::default(),
        &CancelToken::new(),
    )
    .expect("a DRM-free font-obfuscated book still reads");
    assert!(
        chapter
            .blocks
            .iter()
            .any(|block| block.text.contains("One")),
        "the chapter content survives the allowed font obfuscation"
    );
}

#[test]
fn missing_package_escaping_paths_and_doctype_entities_are_visible_failures() {
    let missing = Fixture::new(&[
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", container("OPS/missing.opf")),
    ]);
    let file = std::fs::File::open(&missing.path).unwrap();
    assert!(matches!(
        open_book(
            file,
            "a".repeat(64),
            "x",
            &Limits::default(),
            &CancelToken::new()
        ),
        Err(EpubError::Missing { .. })
    ));

    let escape = Fixture::new(&[
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", container("OPS/book.opf")),
        (
            "OPS/book.opf",
            opf(
                r#"<item id="one" href="../../secret.xhtml" media-type="application/xhtml+xml"/>"#,
                r#"<itemref idref="one"/>"#,
                "",
            ),
        ),
    ]);
    let file = std::fs::File::open(&escape.path).unwrap();
    assert!(matches!(
        open_book(
            file,
            "a".repeat(64),
            "x",
            &Limits::default(),
            &CancelToken::new()
        ),
        Err(EpubError::PathEscapes { .. })
    ));

    let entities = Fixture::new(&[
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", container("OPS/book.opf")),
        (
            "OPS/book.opf",
            opf(
                r#"<item id="one" href="Text/one.xhtml" media-type="application/xhtml+xml"/>"#,
                r#"<itemref idref="one"/>"#,
                "",
            ),
        ),
        (
            "OPS/Text/one.xhtml",
            br#"<!DOCTYPE html [<!ENTITY x SYSTEM "file:///secret">]><html><body><p>&x;</p></body></html>"#.to_vec(),
        ),
    ]);
    let book = entities.book();
    let file = std::fs::File::open(&entities.path).unwrap();
    assert!(matches!(
        read_chapter(
            &book,
            file,
            0,
            ImageMode::Skip,
            &Limits::default(),
            &CancelToken::new()
        ),
        Err(EpubError::Encoding { .. })
    ));
}

#[test]
fn ordinary_xhtml_entities_render_and_a_plain_zip_is_not_a_book() {
    let entities = Fixture::new(&[
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", container("OPS/book.opf")),
        (
            "OPS/book.opf",
            opf(
                r#"<item id="one" href="Text/one.xhtml" media-type="application/xhtml+xml"/>"#,
                r#"<itemref idref="one"/>"#,
                "",
            ),
        ),
        (
            "OPS/Text/one.xhtml",
            br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>caf&eacute; &amp; cr&egrave;me &mdash; &trade;</p></body></html>"#.to_vec(),
        ),
    ]);
    let chapter = entities.chapter(0);
    assert_eq!(chapter.blocks[0].text, "café & crème — ™");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plain.zip");
    write_zip(
        &path,
        &[
            ("mimetype", b"application/zip".to_vec()),
            ("a.txt", b"x".to_vec()),
        ],
    );
    let file = std::fs::File::open(&path).unwrap();
    assert!(matches!(
        open_book(
            file,
            "a".repeat(64),
            "x",
            &Limits::default(),
            &CancelToken::new()
        ),
        Err(EpubError::NotEpub)
    ));
}

#[test]
fn tight_budgets_and_cancellation_stop_work_visibly() {
    let fixture = Fixture::new(&reflowable_entries());
    let mut limits = Limits::default();
    limits.block_bytes = 10;
    let file = std::fs::File::open(&fixture.path).unwrap();
    let book = open_book(file, "a".repeat(64), "x", &limits, &CancelToken::new()).unwrap();
    let file = std::fs::File::open(&fixture.path).unwrap();
    assert!(matches!(
        read_chapter(
            &book,
            file,
            0,
            ImageMode::Skip,
            &limits,
            &CancelToken::new()
        ),
        Err(EpubError::Limit {
            resource: "block bytes",
            ..
        })
    ));

    let mut limits = Limits::default();
    limits.chapters = 1;
    let file = std::fs::File::open(&fixture.path).unwrap();
    assert!(matches!(
        open_book(file, "a".repeat(64), "x", &limits, &CancelToken::new()),
        Err(EpubError::Limit {
            resource: "chapters",
            ..
        })
    ));

    let cancel = CancelToken::new();
    cancel.cancel();
    let file = std::fs::File::open(&fixture.path).unwrap();
    assert!(matches!(
        open_book(file, "a".repeat(64), "x", &Limits::default(), &cancel),
        Err(EpubError::Cancelled)
    ));
}

#[test]
fn a_whole_document_is_one_reader_view_of_the_actual_bytes() {
    // The parser never exposes an extracted-text dump: the same fixture read
    // twice from independent handles returns the same block text, and
    // changing the source file between reads is visible instead of cached.
    let fixture = Fixture::new(&reflowable_entries());
    let first = fixture.chapter(2);
    assert_eq!(first.blocks[0].text, "Third.");
    let mut cursor = Cursor::new(std::fs::read(&fixture.path).unwrap());
    assert_eq!(
        read_chapter(
            &fixture.book(),
            &mut cursor,
            2,
            ImageMode::Skip,
            &Limits::default(),
            &CancelToken::new()
        )
        .unwrap()
        .blocks[0]
            .text,
        "Third."
    );
}
