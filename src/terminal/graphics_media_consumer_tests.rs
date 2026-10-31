mod owned_media_fixture {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/kitty_media.rs"));
}

fn media_command(id: u32, medium: char, name: &str, offset: u64, size: usize, quiet: u8) -> Vec<u8> {
    use base64::Engine;
    format!("\x1b[6;5H\x1b_Ga=T,i={id},p=9,t={medium},f=32,s=2,v=1,O={offset},S={size},c=4,r=1,C=1,q={quiet};{}\x1b\\",
        base64::engine::general_purpose::STANDARD.encode(name)).into_bytes()
}

#[gpui_kit::test]
fn graphics_consumer_owned_media_large_regions_and_uniform_failures(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    cx.update(|cx| { gpui_kit::component::init(cx); cx.set_global(Config::default()); });
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    let dir = tempfile::tempdir().unwrap();
    let offset = 48 * 1024 * 1024 + 13;
    let object_len = 80 * 1024 * 1024;
    // Deliberately nonaligned, well beyond the image limit, in a larger object.
    // Expected BGRA is independent of sender/decoder implementation.
    let rgba = [255, 0, 0, 255, 0, 255, 0, 255];
    for (serial, medium, offset, object_len) in [
        (1, 'f', offset, object_len),
        (2, 't', offset, object_len),
        (3, 's', offset, object_len),
        // Exact 4 KiB object end and a region crossing 64 KiB both retain all
        // eight requested bytes; neither is an out-of-range read.
        (4, 's', 4088, 4096),
        (5, 's', 65533, 69632),
    ] {
        let file = dir.path().join(format!("tty-graphics-protocol-{serial}.rgba"));
        let shared = (medium == 's').then(|| owned_media_fixture::SharedMedia::new(object_len, offset, &rgba));
        let name = if let Some(shared) = &shared { shared.name.clone() } else {
            owned_media_fixture::region_file(&file, if medium == 't' { offset as u64 + 8 } else { object_len as u64 }, offset as u64, &rgba);
            file.to_str().unwrap().to_owned()
        };
        graphics_send_fixture_with_replies(&view, cx, serial, &media_command(145, medium, &name, offset as u64, if medium == 't' { 0 } else { 8 }, 0), 1);
        graphics_recovery_wait(cx, "production media decode timed out", |cx| {
            view.read_with(cx, |view, _| graphics_visible_images(view).iter().any(|image| image.placement.image_id == 145 && image.data.is_some()))
        });
        graphics_assert_production_paint(window, &view, cx);
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            let image = images.iter().find(|image| image.placement.image_id == 145).unwrap();
            assert_eq!(image.data.as_ref().unwrap().as_bytes(0).unwrap(), [0, 0, 255, 255, 0, 255, 0, 255]);
            assert!(image.painted.load(std::sync::atomic::Ordering::Acquire));
        });
        if medium != 's' { assert_eq!(file.exists(), medium == 'f', "only eligible temporary handoff is removed"); }
        graphics_send_fixture(&view, cx, serial + 10, b"\x1b_Ga=d,d=I,i=145\x1b\\");
    }
    // All actual file-read failures have identical externally visible status;
    // quiet=1 retains errors, quiet=2 suppresses them, image/p identity survives.
    let short = dir.path().join("short.rgba");
    std::fs::write(&short, [1, 2, 3, 4]).unwrap();
    let mut names = vec![dir.path().join("missing.rgba").to_str().unwrap().to_owned(), short.to_str().unwrap().to_owned(), dir.path().to_str().unwrap().to_owned()];
    #[cfg(windows)]
    names.extend([r"\\.\pipe\lyrel-not-owned".into(), r"\\localhost\C$\not-owned".into(), "NUL".into(), format!("{}:stream", short.display())]);
    #[cfg(unix)]
    names.extend(["/dev/zero".into(), "/proc/self/mem".into()]);
    #[cfg(windows)]
    let _inaccessible = {
        use std::os::windows::fs::OpenOptionsExt;
        let path = dir.path().join("inaccessible.rgba");
        std::fs::write(&path, rgba).unwrap();
        names.push(path.to_str().unwrap().to_owned());
        std::fs::OpenOptions::new().read(true).share_mode(0).open(path).unwrap()
    };
    let large = dir.path().join("large-to-end.rgba");
    owned_media_fixture::region_file(&large, object_len as u64, offset as u64, &rgba);
    let shared_short = owned_media_fixture::SharedMedia::new(4096, 13, &rgba);
    let mut failures: Vec<_> = names.into_iter().map(|name| ('f', name, 2, 8)).collect();
    failures.push(('f', large.to_str().unwrap().to_owned(), 0, 0));
    failures.push(('s', shared_short.name.clone(), shared_short.len() - 4, 8));
    failures.push(('s', "malformed/shared/name".into(), 0, 8));
    failures.push(('s', format!("missing-{}", uuid::Uuid::new_v4()), 0, 8));
    let mut serial = 30;
    for (medium, name, region_offset, region_size) in failures {
        for quiet in [0, 1, 2] {
            serial += 1;
            // The fixture prints KITTY_DONE over the start of KITTY_REPLY in
            // the grid. Observe actual PTY output before sending, not that
            // deliberately overwritten presentation of the sender's response.
            let observer = daemon
                .panes()
                .observe(
                    pane,
                    crate::daemon::protocol::WinSize {
                        cols: 80,
                        rows: 24,
                        cell_w: 10,
                        cell_h: 20,
                    },
                )
                .unwrap();
            observer
                .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
                .unwrap();
            let command = media_command(146, medium, &name, region_offset, region_size, quiet);
            let request = format!("SEND {serial} {} {} 1\n", usize::from(quiet != 2), graphics_pty_fixture::hex(&command));
            view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
            let marker = format!("KITTY_DONE_{serial};");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while !view.read_with(cx, |view, _| view.title == marker) {
                assert!(std::time::Instant::now() < deadline, "media refusal stalled terminal: serial={serial}, medium={medium}, name={name:?}, O={region_offset}, S={region_size}, q={quiet}; output={:?}", view.read_with(cx, |view, _| view.scrollback_text()));
                graphics_consumer_progress(cx);
            }
            let (text, _) = graphics_recovery_collect(observer, serial);
            if quiet != 2 {
                let expected = graphics_pty_fixture::hex(b"\x1b_Gi=146,p=9;EBADF:Failed to read image file\x1b\\");
                assert!(text.contains(&format!("KITTY_REPLY {serial} {expected}")), "wrong media refusal: serial={serial}, medium={medium}, name={name:?}, O={region_offset}, S={region_size}, q={quiet}; expected={expected}; PTY output={text:?}; grid={:?}", view.read_with(cx, |view, _| view.scrollback_text()));
            }
            if quiet == 2 {
                assert!(!text.contains(&format!("KITTY_REPLY {serial} ")), "quiet media refusal emitted a response: serial={serial}, medium={medium}, name={name:?}, O={region_offset}, S={region_size}, q={quiet}; PTY output={text:?}");
            }
            assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()), "refused media became visible: serial={serial}, medium={medium}, name={name:?}, O={region_offset}, S={region_size}, q={quiet}");
        }
    }
    // Refusals must not poison ordinary text or a subsequent direct transfer.
    graphics_send_fixture_with_replies(&view, cx, 100, b"after_media_refusal\x1b[8;5H\x1b_Ga=T,i=147,p=1,f=32,s=1,v=1,C=1;/wAA/w==\x1b\\", 1);
    assert!(view.read_with(cx, |view, _| view.scrollback_text().contains("after_media_refusal")));
    graphics_assert_production_paint(window, &view, cx);
}

#[gpui_kit::test]
fn graphics_consumer_regular_symlink_target_and_unmarked_temp_are_preserved(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    cx.update(|cx| { gpui_kit::component::init(cx); cx.set_global(Config::default()); });
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    let dir = tempfile::tempdir().unwrap();
    let target_dir = dir.path().join("target");
    std::fs::create_dir(&target_dir).unwrap();
    let target = target_dir.join("sender-owned.rgba");
    std::fs::write(&target, [0, 0, 255, 255, 255, 255, 255, 255]).unwrap();
    let link = dir.path().join("link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target_dir, &link).unwrap();
    #[cfg(windows)]
    {
        // A junction needs no developer-mode symlink privilege. All targets
        // are fixture-owned; no network path or user's link is touched.
        let status = std::process::Command::new("cmd.exe").args(["/d", "/c", "mklink", "/J"])
            .arg(&link).arg(&target_dir).stdout(std::process::Stdio::null()).status().unwrap();
        assert!(status.success(), "owned junction creation is a Windows media prerequisite");
    }
    let name = link.join("sender-owned.rgba");
    graphics_send_fixture_with_replies(&view, cx, 1, &media_command(145, 't', name.to_str().unwrap(), 0, 8, 0), 1);
    graphics_recovery_wait(cx, "resolved media target must decode", |cx| {
        view.read_with(cx, |view, _| graphics_visible_images(view).iter().any(|image| image.data.is_some()))
    });
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(images[0].data.as_ref().unwrap().as_bytes(0).unwrap(), [255, 0, 0, 255, 255, 255, 255, 255]);
    });
    assert_eq!(std::fs::read(&target).unwrap(), [0, 0, 255, 255, 255, 255, 255, 255]);
    assert!(link.exists());
}
