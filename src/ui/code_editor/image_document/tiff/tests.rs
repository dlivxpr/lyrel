//! TIFF 页表与单页解码的回归。夹具在测试内编码，不依赖外部素材。

use super::*;
use crate::ui::code_editor::markdown::images::MAX_PIXELS;
use std::io::Cursor;
use tiff::encoder::TiffEncoder;
use tiff::encoder::colortype::{CMYK8, Gray8, Gray32Float, RGB8, RGB16, RGBA8};

fn rgb_pages(pages: &[(u32, u32, [u8; 3])]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = TiffEncoder::new(Cursor::new(&mut bytes)).expect("tiff starts");
        for (width, height, colour) in pages {
            let image = encoder
                .new_image::<RGB8>(*width, *height)
                .expect("page starts");
            let mut pixels = Vec::with_capacity((*width * *height * 3) as usize);
            for _ in 0..*width * *height {
                pixels.extend_from_slice(colour);
            }
            image.write_data(&pixels).expect("page writes");
        }
    }
    bytes
}

fn open(bytes: Vec<u8>) -> TiffPages {
    TiffPages::open(Arc::from(bytes)).expect("fixture page table opens")
}

#[test]
fn enumerates_pages_and_decodes_distinct_dimensions() {
    let pages = open(rgb_pages(&[
        (40, 20, [200, 10, 10]),
        (20, 60, [10, 200, 10]),
        (70, 10, [10, 10, 200]),
    ]));
    assert_eq!(pages.len(), 3);
    for (page, (width, height, colour)) in [
        (40, 20, [200, 10, 10]),
        (20, 60, [10, 200, 10]),
        (70, 10, [10, 10, 200]),
    ]
    .into_iter()
    .enumerate()
    {
        let decoded = pages.decode(page).expect("page decodes");
        assert_eq!(decoded.dimensions(), (width, height), "page {page}");
        assert_eq!(
            decoded.get_pixel(width / 2, height / 2).0,
            [colour[0], colour[1], colour[2], 255],
            "page {page}"
        );
    }
    assert!(matches!(pages.decode(3), Err(TiffPageError::Missing)));
}

#[test]
fn alpha_and_sixteen_bit_samples_keep_their_meaning() {
    let mut bytes = Vec::new();
    {
        let mut encoder = TiffEncoder::new(Cursor::new(&mut bytes)).expect("tiff starts");
        let image = encoder.new_image::<RGBA8>(2, 1).expect("page starts");
        image
            .write_data(&[10, 20, 30, 128, 1, 2, 3, 0])
            .expect("page writes");
    }
    let pages = open(bytes);
    let decoded = pages.decode(0).expect("rgba page decodes");
    assert_eq!(decoded.get_pixel(0, 0).0, [10, 20, 30, 128]);
    assert_eq!(decoded.get_pixel(1, 0).0, [1, 2, 3, 0]);

    let mut bytes = Vec::new();
    {
        let mut encoder = TiffEncoder::new(Cursor::new(&mut bytes)).expect("tiff starts");
        let image = encoder.new_image::<RGB16>(1, 1).expect("page starts");
        image
            .write_data(&[0x8000u16, 0xFFFF, 0])
            .expect("page writes");
    }
    let pages = open(bytes);
    let decoded = pages.decode(0).expect("16-bit page decodes");
    assert_eq!(decoded.get_pixel(0, 0).0, [128, 255, 0, 255]);
}

#[test]
fn cmyk_and_gray_pages_convert_to_rgba() {
    let mut bytes = Vec::new();
    {
        let mut encoder = TiffEncoder::new(Cursor::new(&mut bytes)).expect("tiff starts");
        let image = encoder.new_image::<CMYK8>(1, 1).expect("page starts");
        image.write_data(&[0, 255, 255, 0]).expect("page writes");
    }
    let decoded = open(bytes).decode(0).expect("cmyk page decodes");
    assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0, 255]);

    let mut bytes = Vec::new();
    {
        let mut encoder = TiffEncoder::new(Cursor::new(&mut bytes)).expect("tiff starts");
        let image = encoder.new_image::<Gray8>(1, 1).expect("page starts");
        image.write_data(&[128]).expect("page writes");
    }
    let decoded = open(bytes).decode(0).expect("gray page decodes");
    assert_eq!(decoded.get_pixel(0, 0).0, [128, 128, 128, 255]);
}

#[test]
fn an_unsupported_sample_layout_is_a_visible_refusal() {
    let mut bytes = Vec::new();
    {
        let mut encoder = TiffEncoder::new(Cursor::new(&mut bytes)).expect("tiff starts");
        let image = encoder.new_image::<Gray32Float>(1, 1).expect("page starts");
        image.write_data(&[0.5f32]).expect("page writes");
    }
    match open(bytes).decode(0) {
        Err(TiffPageError::Unsupported(layout)) => assert!(!layout.is_empty()),
        other => panic!("expected an unsupported-layout error, got {other:?}"),
    }
}

#[test]
fn an_oversized_page_is_refused_by_its_budget() {
    let bytes = rgb_pages(&[(8193, 1, [1, 2, 3])]);
    match open(bytes).decode(0) {
        Err(TiffPageError::Budget { required, limit }) => {
            assert!(required > limit);
            assert_eq!(limit, MAX_PIXELS * 4);
        }
        other => panic!("expected a page budget error, got {other:?}"),
    }
}

#[test]
fn too_many_pages_are_refused_at_open() {
    let mut bytes = Vec::new();
    {
        let mut encoder = TiffEncoder::new(Cursor::new(&mut bytes)).expect("tiff starts");
        for _ in 0..=MAX_PAGES {
            let image = encoder.new_image::<Gray8>(1, 1).expect("page starts");
            image.write_data(&[0]).expect("page writes");
        }
    }
    match TiffPages::open(Arc::from(bytes)) {
        Err(TiffError::Pages { limit }) => assert_eq!(limit, MAX_PAGES),
        Err(other) => panic!("unexpected refusal: {other}"),
        Ok(_) => panic!("a page table over budget opened"),
    }
}

#[test]
fn a_truncated_file_never_panics_and_reports_a_failure() {
    let bytes = rgb_pages(&[(64, 64, [9, 8, 7]), (64, 64, [7, 8, 9])]);
    let cut = bytes[..bytes.len() / 2].to_vec();
    match TiffPages::open(Arc::from(cut)) {
        Err(error) => assert!(!error.to_string().is_empty()),
        Ok(pages) => {
            let error = pages
                .decode(0)
                .expect_err("a truncated page must not decode");
            assert!(!error.to_string().is_empty());
        }
    }
}
