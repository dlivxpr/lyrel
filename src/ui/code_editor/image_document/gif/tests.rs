//! 上游 `GifPlayback` 回归在 Lyrel 播放器上的执行。
//!
//! 这些是解码器层测试：生产工作台的计时、可见性与文档生命周期由
//! `image_document/tests.rs` 覆盖。夹具在测试内生成，不依赖外部素材。

use super::*;

fn gif_bytes(
    width: u16,
    height: u16,
    frames: &[(u16, Vec<u8>, gif::DisposalMethod, u16, bool)],
    repeat: gif::Repeat,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = gif::Encoder::new(
            &mut bytes,
            width,
            height,
            &[255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0],
        )
        .expect("global palette encodes");
        encoder.set_repeat(repeat).expect("repeat encodes");
        for &(left, ref pixels, disposal, delay, interlaced) in frames {
            let frame = gif::Frame {
                left,
                width: (pixels.len() / usize::from(height)).max(1) as u16,
                height,
                delay,
                dispose: disposal,
                transparent: Some(3),
                interlaced,
                buffer: std::borrow::Cow::Borrowed(pixels),
                ..Default::default()
            };
            encoder.write_frame(&frame).expect("frame encodes");
        }
    }
    bytes
}

fn playback(bytes: Vec<u8>) -> GifPlayback {
    GifPlayback::new(Arc::from(bytes)).expect("fixture opens")
}

/// 上游 `gif_playback_composites_disposal_and_respects_finite_loops`：
/// 四帧、`Keep/Background/Previous`、透明与 finite 1 的两轮播放。
#[test]
fn composites_disposal_and_respects_finite_loops() {
    use gif::DisposalMethod::{Background, Keep, Previous};
    let bytes = gif_bytes(
        3,
        1,
        &[
            (0, vec![0, 0, 0], Keep, 5, false),
            (1, vec![1], Background, 5, false),
            (0, vec![2], Previous, 5, false),
            (2, vec![1], Keep, 5, false),
        ],
        gif::Repeat::Finite(1),
    );
    let mut playback = playback(bytes);
    let red = [255, 0, 0, 255];
    let green = [0, 255, 0, 255];
    let blue = [0, 0, 255, 255];
    let transparent = [0, 0, 0, 0];
    for _ in 0..2 {
        for expected in [
            [red, red, red],
            [red, green, red],
            [blue, transparent, red],
            [red, transparent, green],
        ] {
            let (frame, delay) = playback.next_frame().expect("frame decodes").unwrap();
            assert_eq!(frame.dimensions(), (3, 1));
            assert_eq!(delay, Duration::from_millis(50));
            for (pixel, expected) in frame.pixels().zip(expected) {
                assert_eq!(pixel.0, expected);
            }
        }
    }
    assert!(playback.next_frame().expect("past the loop").is_none());
    assert!(playback.next_frame().expect("still finished").is_none());
}

/// 上游 `gif_playback_has_no_total_frame_limit`：2101 帧的无限动画跨两圈
/// 仍继续解码，没有被累计帧数截断。
#[test]
fn has_no_total_frame_limit() {
    let frame = (0, vec![0, 1, 2], gif::DisposalMethod::Keep, 2, false);
    let frames = vec![frame; 2101];
    let bytes = gif_bytes(3, 1, &frames, gif::Repeat::Infinite);
    let mut playback = playback(bytes);
    for _ in 0..4202 {
        assert!(playback.next_frame().expect("frame decodes").is_some());
        assert_eq!(playback.canvas.dimensions(), (3, 1));
        assert!(playback.restore.is_none());
    }
}

/// interlace 的流按 pass 顺序写入，播放器必须把它放回源行位置。
#[test]
fn interlaced_rows_land_in_source_order() {
    let width = 4u16;
    let height = 8u16;
    // 每行使用不同调色板索引，交错顺序为 0,4,2,6,1,3,5,7。
    let mut stream = Vec::new();
    for y in [0u16, 4, 2, 6, 1, 3, 5, 7] {
        let index = (y % 4) as u8;
        stream.extend(std::iter::repeat_n(index, usize::from(width)));
    }
    let bytes = gif_bytes(
        width,
        height,
        &[(0, stream, gif::DisposalMethod::Keep, 5, true)],
        gif::Repeat::Finite(0),
    );
    let mut playback = playback(bytes);
    let (frame, _) = playback.next_frame().expect("frame decodes").unwrap();
    let palette = [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [0, 0, 0, 0],
    ];
    for y in 0..usize::from(height) {
        let expected = palette[y % 4];
        for x in 0..usize::from(width) {
            assert_eq!(
                frame.get_pixel(x as u32, y as u32).0,
                expected,
                "row {y} column {x}"
            );
        }
    }
}

/// 0/1 centisecond 的延时不能形成零时长忙循环；其余按下限换算。
#[test]
fn short_delays_become_one_hundred_milliseconds() {
    for (encoded, expected) in [(0, 100), (1, 100), (2, 20), (10, 100)] {
        let bytes = gif_bytes(
            1,
            1,
            &[(0, vec![0], gif::DisposalMethod::Keep, encoded, false)],
            gif::Repeat::Finite(0),
        );
        let mut playback = playback(bytes);
        let (_, delay) = playback.next_frame().expect("frame decodes").unwrap();
        assert_eq!(delay, Duration::from_millis(expected), "delay {encoded}");
    }
}

/// 截断的帧数据必须报错，而不是返回半张画布。
#[test]
fn truncated_frame_data_is_an_error() {
    let bytes = gif_bytes(
        32,
        32,
        &[
            (0, vec![0; 32 * 32], gif::DisposalMethod::Keep, 5, false),
            (0, vec![1; 32 * 32], gif::DisposalMethod::Keep, 5, false),
        ],
        gif::Repeat::Finite(0),
    );
    let mut cut = bytes.clone();
    cut.truncate(bytes.len() * 3 / 4);
    let mut playback = playback(cut);
    let mut saw_error = false;
    for _ in 0..2 {
        match playback.next_frame() {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(error) => {
                assert!(
                    matches!(error, GifError::Truncated | GifError::Decode(_)),
                    "{error}"
                );
                saw_error = true;
                break;
            }
        }
    }
    assert!(saw_error, "a truncated animation must fail visibly");
}

/// 画布超出合成预算时在打开阶段拒绝，不降分辨率继续。
#[test]
fn an_oversized_canvas_is_refused_at_open() {
    let mut bytes = gif_bytes(
        1,
        1,
        &[(0, vec![0], gif::DisposalMethod::Keep, 5, false)],
        gif::Repeat::Finite(0),
    );
    // 只改逻辑屏幕尺寸：3000×2000 RGBA 的 canvas+restore+output 已超过 64 MiB。
    bytes[6..8].copy_from_slice(&3000u16.to_le_bytes());
    bytes[8..10].copy_from_slice(&2000u16.to_le_bytes());
    match GifPlayback::new(Arc::from(bytes)) {
        Err(GifError::Budget { required, limit }) => {
            assert!(required > limit);
            assert_eq!(limit, COMPOSITE_BYTES);
        }
        Err(other) => panic!("unexpected refusal: {other}"),
        Ok(_) => panic!("an over-budget animation opened"),
    }
}
