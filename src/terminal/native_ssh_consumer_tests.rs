// Real native router -> SSH/SFTP -> Linux daemon -> production TerminalView.
// These ignored UI integration tests inspect decoder BGRA, TerminalElement paint
// claims and geometry. They do not certify native-window/GPU output on any OS.
mod native_ssh_graphics_fixture {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/support/native_ssh_graphics.rs"
    ));
}

fn native_ssh_consumer_fixture() -> native_ssh_graphics_fixture::NativeSshFixture {
    let binary = std::env::var_os("LYREL_TEST_SSH_LOCAL_SERVER").expect(
        "native SSH GPUI prerequisite: LYREL_TEST_SSH_LOCAL_SERVER must name the same-commit desktop lyrel-server binary; also provide the explicit LYREL_TEST_SSH_* fixture endpoint, key, fixed host pin, remote home and Linux Node",
    );
    let binary = std::path::PathBuf::from(binary);
    assert!(
        binary.is_file(),
        "LYREL_TEST_SSH_LOCAL_SERVER must be an existing server binary"
    );
    native_ssh_graphics_fixture::NativeSshFixture::start(&binary)
}

fn native_ssh_consumer_init(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
}

fn native_ssh_consumer_spawn(
    fixture: &native_ssh_graphics_fixture::NativeSshFixture,
    shell: crate::daemon::protocol::ShellSpec,
) -> u64 {
    let (mut stream, _) = fixture.connect_pane();
    ClientMsg::Spawn {
        cwd: None,
        size: native_ssh_graphics_fixture::win_size(),
        shell: Some(shell),
        owner: None,
        workspace: None,
        restore: None,
        allow_remote_clipboard_write: false,
        await_claim: false,
    }
    .encode(&mut stream)
    .unwrap();
    let pane = match DaemonMsg::read(&mut stream).unwrap() {
        DaemonMsg::Spawned { pane_id } => pane_id,
        other => panic!("real Linux PTY spawn failed: {other:?}"),
    };
    fixture.track_pane(pane);
    let mut text = Vec::new();
    while !text
        .windows(b"KITTY_READY".len())
        .any(|part| part == b"KITTY_READY")
    {
        match DaemonMsg::read(&mut stream).unwrap() {
            DaemonMsg::Output(bytes) => text.extend(bytes),
            DaemonMsg::Error(error) => panic!("remote sender failed: {error}"),
            DaemonMsg::Exited { .. } => panic!("remote sender exited before KITTY_READY"),
            _ => {}
        }
    }
    ClientMsg::Detach.encode(&mut stream).unwrap();
    pane
}

fn native_ssh_consumer_fresh(
    fixture: &native_ssh_graphics_fixture::NativeSshFixture,
    pane: u64,
    cx: &mut TestAppContext,
    serial: usize,
    decoded_bytes: u64,
) -> (Entity<TerminalView>, gpui_kit::AnyWindowHandle) {
    let (mut stream, _) = fixture.connect_pane_with_capacity(64 << 20, decoded_bytes);
    ClientMsg::Attach {
        pane_id: pane,
        size: native_ssh_graphics_fixture::win_size(),
        allow_remote_clipboard_write: false,
    }
    .encode(&mut stream)
    .unwrap();
    stream.set_read_timeout(None).unwrap();
    // Linux owns this PTY even when the desktop runs on Windows. Reuse the
    // same measured physical-grid-before-mount factory as local recovery.
    let size = TermSize::new(80, 24);
    let (view, window) =
        graphics_build_rooted_consumer_at(cx, pane, stream, Vec::new(), size, PtySource::Raw);
    graphics_assert_production_paint_at(window, &view, cx, size);
    // Unique post-attach query ACK + actual later text, not replay byte count.
    graphics_recovery_stats(&view, cx, serial);
    (view, window)
}

fn native_ssh_consumer_observe_ack(
    fixture: &native_ssh_graphics_fixture::NativeSshFixture,
    pane: u64,
    serial: usize,
) -> std::thread::JoinHandle<Vec<u8>> {
    let (mut stream, _) = fixture.connect_pane();
    // A read-only wire observer must not await desktop texture grants, resize
    // the Linux PTY, or replace the controlling view's small pressure budget.
    // The daemon takes the minimum of observer/controller capacity leases.
    ClientMsg::GraphicsCapacity {
        encoded_bytes: 64 << 20,
        decoded_bytes: 128 << 20,
        offers: false,
    }
    .encode(&mut stream)
    .unwrap();
    ClientMsg::Observe {
        pane_id: pane,
        size: native_ssh_graphics_fixture::win_size(),
    }
    .encode(&mut stream)
    .unwrap();
    // The production observer writer starts only after pane.observe registers
    // its subscription. Receive its first replay frame before sending input,
    // so the live ACK cannot race ahead of this read-only subscription.
    let first = DaemonMsg::read(&mut stream).unwrap();
    assert!(
        !matches!(first, DaemonMsg::Error(_) | DaemonMsg::Exited { .. }),
        "real ACK observer could not subscribe: {first:?}"
    );
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + native_ssh_graphics_fixture::STREAM_WITHIN;
        let done = format!("KITTY_DONE_{serial};");
        let mut first = Some(first);
        let mut text = Vec::new();
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .filter(|wait| !wait.is_zero())
                .expect("real ACK observer deadline expired");
            stream.set_read_timeout(Some(remaining)).unwrap();
            let message = first
                .take()
                .unwrap_or_else(|| DaemonMsg::read(&mut stream).unwrap());
            match message {
                DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => {
                    assert!(
                        text.len() + bytes.len() <= 1 << 20,
                        "ACK observer text exceeds bounded fixture capture"
                    );
                    text.extend(bytes);
                }
                DaemonMsg::Error(error) => panic!("real ACK observer failed: {error}"),
                DaemonMsg::Exited { .. } => {
                    panic!("Linux sender exited before its correlated ACK/text barrier")
                }
                _ => {}
            }
            if text.windows(done.len()).any(|part| part == done.as_bytes()) {
                ClientMsg::Detach.encode(&mut stream).unwrap();
                return text;
            }
        }
    })
}

fn native_ssh_consumer_send(
    fixture: &native_ssh_graphics_fixture::NativeSshFixture,
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    serial: usize,
    bytes: &[u8],
    expected_replies: &[&[u8]],
) {
    // ACK/DONE reports deliberately reuse a fixed saved-cursor area. Grid
    // text is therefore not an ACK log: later reports overwrite earlier ones.
    // Observe their actual ordered PTY Output without changing producer bytes
    // or the geometry/scrolling exercised by the real TerminalView.
    let observer = (!expected_replies.is_empty()).then(|| {
        let pane = view.read_with(cx, |view, _| view.pane_id);
        native_ssh_consumer_observe_ack(fixture, pane, serial)
    });
    let request = format!(
        "SEND {serial} {} {} 7\n",
        expected_replies.len(),
        native_ssh_graphics_fixture::hex(bytes)
    );
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    let done = format!("KITTY_DONE_{serial};");
    graphics_recovery_wait(
        cx,
        "real remote sender query ACK/text barrier did not arrive",
        |cx| view.read_with(cx, |view, _| view.scrollback_text().contains(&done)),
    );
    if let Some(observer) = observer {
        graphics_recovery_wait(
            cx,
            "real ACK observer did not receive the sender's ordered output barrier",
            |_| observer.is_finished(),
        );
        let bytes = observer.join().expect("real ACK observer failed");
        let text = String::from_utf8_lossy(&bytes);
        for reply in expected_replies {
            let marker = format!(
                "KITTY_REPLY {serial} {}",
                native_ssh_graphics_fixture::hex(reply)
            );
            assert!(
                text.contains(&marker),
                "remote application did not receive its correlated reply in real PTY Output: expected {marker}; output={text}"
            );
        }
    }
}

fn native_ssh_consumer_identity(
    fixture: &native_ssh_graphics_fixture::NativeSshFixture,
    pane: u64,
) -> (String, String, u32, String) {
    use lyrel_core::host::Host;
    let host = fixture.connect_control();
    let peer = host.peer();
    assert_eq!(
        peer.protocol_version,
        crate::daemon::protocol::PROTOCOL_VERSION
    );
    assert!(
        !peer.instance.is_empty(),
        "remote daemon must identify its living instance"
    );
    assert_eq!(host.home().to_string_lossy(), fixture.remote_home());
    let status = native_ssh_graphics_fixture::status(&host);
    assert_eq!(status.protocol_version, peer.protocol_version);
    assert!(status.pid > 0 && !status.socket.is_empty());
    let procs = host
        .pane_procs(pane)
        .expect("Linux daemon must describe its actual PTY");
    let context = procs
        .context
        .as_ref()
        .expect("Linux daemon must expose its actual pane context");
    assert!(
        context.local_pty,
        "PTY must belong to the Linux daemon, not local SpawnNativeSsh"
    );
    assert!(
        context.remote.is_none(),
        "ordinary SSH shell bridging is not a native workspace pane"
    );
    let (_, version) = fixture.connect_pane();
    assert_eq!(version.protocol, peer.protocol_version);
    assert!(
        !version.instance.is_empty(),
        "pane protocol must identify its living instance"
    );
    (
        peer.instance.clone(),
        version.instance,
        status.pid,
        status.socket,
    )
}

fn native_ssh_consumer_stat_counter(stats: &str, name: &str) -> u64 {
    let prefix = format!("{name}=");
    stats
        .split_whitespace()
        .find_map(|field| field.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("sender counter {name} missing: {stats}"))
        .parse()
        .unwrap()
}

fn native_ssh_consumer_assert_shared_geometry(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
) {
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let shared: Vec<_> = images.iter().filter(|image| image.placement.image_id == 7).collect();
        assert_eq!(shared.len(), 2, "one direct crop and one real-cell Unicode run, with no duplicate replay");
        assert!(std::sync::Arc::ptr_eq(shared[0].data.as_ref().unwrap(), shared[1].data.as_ref().unwrap()));
        let direct = shared.iter().find(|image| image.placement.placement_id == 11).unwrap();
        assert_eq!((direct.placement.x, direct.placement.w), (1, 2));
        assert_eq!(direct.paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.), (
            Bounds::new(point(px(40.), px(50.)), gpui_kit::size(px(20.), px(10.))),
            Bounds::new(point(px(30.), px(50.)), gpui_kit::size(px(40.), px(10.))),
        ));
        let placeholder = shared.iter().find(|image| image.placement.placement_id == 12).unwrap();
        assert_eq!(placeholder.paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.).0,
            Bounds::new(point(px(40.), px(70.)), gpui_kit::size(px(40.), px(10.))));
        let term = view.terminal.term.lock();
        for col in 4..8 {
            assert_eq!(term.grid[Line(7)][Column(col)].c(), '\u{10eeee}', "the remote Linux text cells, not a fabricated placement list, anchor Unicode graphics");
        }
        assert!(shared.iter().all(|image| image.painted.load(std::sync::atomic::Ordering::Acquire)), "production headless paint claims, not GPU pixels");
    });
}

#[gpui_kit::test]
#[ignore = "requires explicit isolated native SSH-to-Linux fixture and same-commit desktop server; not native GPU evidence"]
fn native_ssh_consumer_linux_pixels_geometry_and_fresh_soft_hard_recovery(cx: &mut TestAppContext) {
    native_ssh_consumer_init(cx);
    let mut fixture = native_ssh_consumer_fixture();
    let host = fixture.connect_control();
    let shell = fixture.install_sender(&host);
    let pane = native_ssh_consumer_spawn(&fixture, shell);
    let identity = native_ssh_consumer_identity(&fixture, pane);
    let (view, window) = native_ssh_consumer_fresh(&fixture, pane, cx, 4600, 128 << 20);
    native_ssh_consumer_send(
        &fixture,
        &view,
        cx,
        4601,
        b"\x1b[2J\x1b[3J\x1b_Ga=t,i=7,f=32,s=4,v=1;/wAA/wD/AP8AAP///////w==\x1b\\",
        &[b"\x1b_Gi=7;OK\x1b\\"],
    );
    assert!(
        view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()),
        "transmit-only cannot paint"
    );
    graphics_assert_production_paint(window, &view, cx);
    native_ssh_consumer_send(&fixture, &view, cx, 4602, "\x1b[6;5H\x1b_Ga=p,i=7,p=11,c=2,r=1,x=1,y=0,w=2,h=1,C=1,q=2\x1b\\\x1b_Ga=p,i=7,p=12,U=1,c=4,r=1,q=2\x1b\\\x1b[8;5H\x1b[38;5;7;58;5;12m\u{10eeee}\u{305}\u{305}\u{10eeee}\u{10eeee}\u{10eeee}\x1b[0m".as_bytes(), &[]);
    let pixels = [
        0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255,
    ];
    graphics_recovery_pixels(&view, cx, 7, &pixels);
    graphics_assert_production_paint(window, &view, cx);
    native_ssh_consumer_assert_shared_geometry(&view, cx);
    let stopped = graphics_recovery_stats(&view, cx, 4603);
    assert!(
        stopped.contains("uploads=1") && stopped.contains("placements=2"),
        "actual sender command accounting: {stopped}"
    );
    // Triple-weak destruction includes real blur/remove/App update, preserving
    // the #45 active TestWindow -> input_handler ownership regression guard.
    graphics_recovery_destroy(view, window, cx);
    let (soft, soft_window) = native_ssh_consumer_fresh(&fixture, pane, cx, 4604, 128 << 20);
    graphics_recovery_pixels(&soft, cx, 7, &pixels);
    graphics_assert_production_paint(soft_window, &soft, cx);
    native_ssh_consumer_assert_shared_geometry(&soft, cx);
    assert_eq!(
        graphics_recovery_stats(&soft, cx, 4605),
        stopped,
        "fresh soft attach must not request a producer resend"
    );
    assert_eq!(native_ssh_consumer_identity(&fixture, pane), identity);
    native_ssh_consumer_send(
        &fixture,
        &soft,
        cx,
        4606,
        b"\x1b[12;5H\x1b_Ga=T,i=8,p=8,c=2,r=1,C=1,f=32,s=1,v=1;/wAA/w==\x1b\\",
        &[b"\x1b_Gi=8,p=8;OK\x1b\\"],
    );
    graphics_recovery_pixels(&soft, cx, 8, &[0, 0, 255, 255]);
    native_ssh_consumer_send(
        &fixture,
        &soft,
        cx,
        4607,
        b"\x1b[12;5H\x1b_Ga=T,i=8,p=8,c=2,r=1,C=1,f=32,s=1,v=1;AAD//w==\x1b\\",
        &[b"\x1b_Gi=8,p=8;OK\x1b\\"],
    );
    graphics_recovery_pixels(&soft, cx, 8, &[255, 0, 0, 255]);
    native_ssh_consumer_send(&fixture, &soft, cx, 4608, b"\x1b_Ga=d,d=I,i=8\x1b\\\x1b[?1049h\x1b[2J\x1b[6;5H\x1b_Ga=T,i=9,p=9,c=2,r=1,C=1,f=32,s=1,v=1;/////w==\x1b\\", &[b"\x1b_Gi=9,p=9;OK\x1b\\"]);
    graphics_recovery_pixels(&soft, cx, 9, &[255, 255, 255, 255]);
    graphics_assert_production_paint(soft_window, &soft, cx);
    soft.read_with(cx, |view, _| {
        assert!(view.terminal.term.lock().mode().contains(Mode::ALT_SCREEN));
        assert!(
            graphics_visible_images(view)
                .iter()
                .all(|image| image.placement.image_id == 9)
        );
    });
    let alt_stopped = graphics_recovery_stats(&soft, cx, 4609);
    graphics_recovery_destroy(soft, soft_window, cx);
    drop(host);
    fixture.restart_router();
    assert_eq!(
        native_ssh_consumer_identity(&fixture, pane),
        identity,
        "router restart must not restart the remote daemon/PTY"
    );
    let (mut stream, _) = fixture.connect_pane();
    ClientMsg::Attach {
        pane_id: pane,
        size: native_ssh_graphics_fixture::win_size(),
        allow_remote_clipboard_write: false,
    }
    .encode(&mut stream)
    .unwrap();
    let mut buffered = Vec::new();
    loop {
        let message = DaemonMsg::read(&mut stream).unwrap();
        assert!(
            !matches!(message, DaemonMsg::Error(_) | DaemonMsg::Exited { .. }),
            "real replay admission failed: {message:?}"
        );
        let offered = matches!(message, DaemonMsg::GraphicsOffer { .. });
        message.encode(&mut buffered).unwrap();
        if offered {
            break;
        }
    }
    // Replay is now stopped at a real resource offer, before the new desktop
    // grants or decodes it. A separate production connection drives live PTY
    // output; its private remote .done proves query ACK + later text completed
    // on Linux before we mount the fresh consumer. No timing/snapshot race.
    use lyrel_core::host::Host;
    let host = fixture.connect_control();
    let live = b"\x1b_Ga=d,d=I,i=9\x1b\\\x1b[?1049l\x1b_Ga=f,i=7,f=32,s=4,v=1,q=2;/////////////////////w==\x1b\\\x1b_Ga=a,i=7,s=1,c=2,q=2\x1b\\\x1b[18;1HREMOTE-MAIN-DURING-HARD";
    let gate = fixture.remote_path("consumer-live-gate");
    let done = fixture.remote_path("consumer-live-gate.done");
    let request = format!(
        "WAIT 4610 0 {} 7 {}\n",
        native_ssh_graphics_fixture::hex(live),
        native_ssh_graphics_fixture::hex(gate.to_str().unwrap().as_bytes())
    );
    let (mut input, _) = fixture.connect_pane();
    ClientMsg::SendInput {
        pane_id: pane,
        bytes: request.into_bytes(),
    }
    .encode(&mut input)
    .unwrap();
    assert!(
        matches!(DaemonMsg::read(&mut input).unwrap(), DaemonMsg::InputAck { pane_id } if pane_id == pane)
    );
    host.write_file(&gate, b"release").unwrap();
    graphics_recovery_wait(
        cx,
        "remote live output must finish its actual query ACK while replay is awaiting the desktop grant",
        |_| host.exists(&done),
    );
    stream.set_read_timeout(None).unwrap();
    let (hard, hard_window) = graphics_build_rooted_consumer_at(
        cx,
        pane,
        stream,
        buffered,
        TermSize::new(80, 24),
        PtySource::Raw,
    );
    graphics_recovery_wait(
        cx,
        "fresh consumer must receive actual post-cutover output, not just a replay prefix",
        |cx| {
            hard.read_with(cx, |view, _| {
                view.scrollback_text().contains("KITTY_DONE_4610;")
                    && view.scrollback_text().contains("REMOTE-MAIN-DURING-HARD")
            })
        },
    );
    let hard_stopped = graphics_recovery_stats(&hard, cx, 4611);
    for name in ["pid", "uploads", "placements"] {
        assert_eq!(
            native_ssh_consumer_stat_counter(&hard_stopped, name),
            native_ssh_consumer_stat_counter(&alt_stopped, name),
            "hard cutover cannot restart or request image retransmission: {name}"
        );
    }
    assert_eq!(
        native_ssh_consumer_stat_counter(&hard_stopped, "bytes"),
        native_ssh_consumer_stat_counter(&alt_stopped, "bytes") + live.len() as u64,
        "only the explicitly ordered live screen/delete/text bytes may be emitted"
    );
    graphics_recovery_pixels(&hard, cx, 7, &[255; 16]);
    graphics_assert_production_paint(hard_window, &hard, cx);
    native_ssh_consumer_assert_shared_geometry(&hard, cx);
    hard.read_with(cx, |view, _| {
        assert!(!view.terminal.term.lock().mode().contains(Mode::ALT_SCREEN));
        assert!(
            !graphics_visible_images(view)
                .iter()
                .any(|image| matches!(image.placement.image_id, 8 | 9)),
            "deleted main image and inactive alternate image cannot leak into the shell"
        );
        assert!(view.scrollback_text().contains("REMOTE-MAIN-DURING-HARD"));
    });
    graphics_recovery_destroy(hard, hard_window, cx);
    fixture.kill_pane(pane);
}

#[gpui_kit::test]
#[ignore = "requires explicit isolated native SSH-to-Linux fixture and same-commit desktop server; not native GPU evidence"]
fn native_ssh_consumer_remote_pressure_notices_scroll_and_recover_without_resend(
    cx: &mut TestAppContext,
) {
    native_ssh_consumer_init(cx);
    let fixture = native_ssh_consumer_fixture();
    let host = fixture.connect_control();
    let shell = fixture.install_sender(&host);
    let pane = native_ssh_consumer_spawn(&fixture, shell);
    let identity = native_ssh_consumer_identity(&fixture, pane);
    let (prior, prior_window) = native_ssh_consumer_fresh(&fixture, pane, cx, 4630, 8);
    native_ssh_consumer_send(&fixture, &prior, cx, 4631, "\x1b[2J\x1b[3J\x1b[6;5H\x1b_Ga=T,i=31,p=1,c=2,r=1,C=1,f=32,s=1,v=1;/wAA/w==\x1b\\\x1b_Ga=T,i=32,p=2,U=1,c=2,r=1,f=32,s=1,v=1;AAD//w==\x1b\\\x1b[8;5H\x1b[38;5;32;58;5;2m\u{10eeee}\u{305}\u{305}\u{10eeee}\x1b[0m".as_bytes(), &[b"\x1b_Gi=31,p=1;OK\x1b\\", b"\x1b_Gi=32,p=2;OK\x1b\\"]);
    graphics_recovery_pixels(&prior, cx, 31, &[0, 0, 255, 255]);
    graphics_recovery_pixels(&prior, cx, 32, &[255, 0, 0, 255]);
    graphics_assert_production_paint(prior_window, &prior, cx);
    let placeholder_area = prior.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let image = images
            .iter()
            .find(|image| image.placement.image_id == 32)
            .unwrap();
        let area = image.paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.);
        assert_eq!(
            area.0,
            Bounds::new(point(px(45.), px(70.)), gpui_kit::size(px(10.), px(10.)))
        );
        area
    });
    native_ssh_consumer_send(&fixture, &prior, cx, 4632, &b"\r\n".repeat(80), &[]);
    native_ssh_consumer_send(
        &fixture,
        &prior,
        cx,
        4633,
        b"\x1b[10;5H\x1b_Ga=T,i=33,p=3,c=2,r=1,C=1,f=32,s=1,v=1;AP8A/w==\x1b\\",
        &[b"\x1b_Gi=33,p=3;OK\x1b\\"],
    );
    graphics_recovery_pixels(&prior, cx, 33, &[0, 255, 0, 255]);
    prior.update(cx, |view, cx| {
        view.scroll(10_000, None, &Modifiers::default(), cx)
    });
    graphics_assert_production_paint(prior_window, &prior, cx);
    prior.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let oldest = images
            .iter()
            .find(|image| image.placement.image_id == 31)
            .unwrap();
        assert!(oldest.evicted && oldest.data.is_none());
        assert!(
            oldest.painted.load(std::sync::atomic::Ordering::Acquire),
            "oldest remote history paints its real notice area"
        );
        assert!(
            !images
                .iter()
                .find(|image| image.placement.image_id == 32)
                .unwrap()
                .evicted
        );
    });
    prior.update(cx, |view, cx| {
        view.scroll(-10_000, None, &Modifiers::default(), cx)
    });
    native_ssh_consumer_send(
        &fixture,
        &prior,
        cx,
        4634,
        b"\x1b[12;5H\x1b_Ga=T,i=34,p=4,c=2,r=1,C=1,f=32,s=1,v=1;/////w==\x1b\\",
        &[b"\x1b_Gi=34,p=4;OK\x1b\\"],
    );
    graphics_recovery_pixels(&prior, cx, 34, &[255, 255, 255, 255]);
    native_ssh_consumer_send(
        &fixture,
        &prior,
        cx,
        4635,
        b"\x1b_Ga=d,d=I,i=31\x1b\\\x1b[18;1HREMOTE-PRESSURE-SHELL-ALIVE",
        &[],
    );
    graphics_assert_production_paint(prior_window, &prior, cx);
    let stopped = graphics_recovery_stats(&prior, cx, 4636);
    graphics_recovery_destroy(prior, prior_window, cx);
    let (fresh, fresh_window) = native_ssh_consumer_fresh(&fixture, pane, cx, 4637, 8);
    graphics_recovery_pixels(&fresh, cx, 33, &[0, 255, 0, 255]);
    graphics_recovery_pixels(&fresh, cx, 34, &[255, 255, 255, 255]);
    assert_eq!(native_ssh_consumer_identity(&fixture, pane), identity);
    assert_eq!(
        graphics_recovery_stats(&fresh, cx, 4638),
        stopped,
        "pressure recovery must not bypass the budget with a producer resend"
    );
    fresh.update(cx, |view, cx| {
        view.scroll(10_000, None, &Modifiers::default(), cx)
    });
    graphics_assert_production_paint(fresh_window, &fresh, cx);
    fresh.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert!(
            !images.iter().any(|image| image.placement.image_id == 31),
            "application deletion must remove even an evicted notice"
        );
        let notice = images
            .iter()
            .find(|image| image.placement.image_id == 32)
            .unwrap();
        assert!(
            notice.evicted && notice.data.is_none(),
            "fresh recovery cannot resurrect evicted remote pixels"
        );
        assert!(
            notice.painted.load(std::sync::atomic::Ordering::Acquire),
            "the restored Unicode cells must paint a localized notice, not the private-use glyph"
        );
        let offset = view.terminal.term.lock().display_offset() as i32;
        assert_eq!(
            notice.paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, offset, 1.),
            placeholder_area
        );
        let text = view.scrollback_text();
        assert!(text.contains("REMOTE-PRESSURE-SHELL-ALIVE"));
        assert!(
            !text.contains("Image evicted") && !text.contains("图片已淘汰"),
            "localized notices are paint-only and cannot manufacture terminal text"
        );
    });
    graphics_recovery_destroy(fresh, fresh_window, cx);
    fixture.kill_pane(pane);
}

fn native_ssh_consumer_assert_animation_graph(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    expected: &[u8],
) -> Vec<lyrel_core::core::kitty_graphics::Placement> {
    graphics_recovery_pixels(view, cx, 147, expected);
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let shared: Vec<_> = images.iter().filter(|image| image.placement.image_id == 147).collect();
        assert_eq!(shared.len(), 4, "direct, relative, Unicode prototype and virtual-relative child share one selected version");
        for image in &shared {
            assert_eq!(image.data.as_ref().unwrap().as_bytes(0).unwrap(), expected);
            assert!(std::sync::Arc::ptr_eq(shared[0].data.as_ref().unwrap(), image.data.as_ref().unwrap()));
        }
        let direct = shared.iter().find(|image| image.placement.placement_id == 1).unwrap();
        let relative = shared.iter().find(|image| image.placement.placement_id == 2).unwrap();
        let virtual_root = shared.iter().find(|image| image.placement.placement_id == 3).unwrap();
        let virtual_child = shared.iter().find(|image| image.placement.placement_id == 4).unwrap();
        assert_eq!(relative.placement.parent, Some(direct.placement.key));
        assert_eq!(virtual_child.placement.parent, Some(virtual_root.placement.key));
        assert!(virtual_root.placement.virtual_placement);
        assert_eq!((direct.anchor_row, direct.anchor_col), (5, 4));
        assert_eq!((relative.anchor_row, relative.anchor_col), (7, 7));
        assert_eq!((virtual_child.anchor_row, virtual_child.anchor_col), (11, 7));
        shared.iter().map(|image| image.placement.clone()).collect()
    })
}

#[gpui_kit::test]
#[ignore = "requires isolated real Linux native SSH; decoded/paint contracts, not GPU/FPS acceptance"]
fn native_ssh_consumer_animation_mutations_visibility_and_detached_latest_graph(cx: &mut TestAppContext) {
    native_ssh_consumer_init(cx);
    let fixture = native_ssh_consumer_fixture();
    let host = fixture.connect_control();
    let pane = native_ssh_consumer_spawn(&fixture, fixture.install_sender(&host));
    let identity = native_ssh_consumer_identity(&fixture, pane);
    let (view, window) = native_ssh_consumer_fresh(&fixture, pane, cx, 14700, 128 << 20);
    native_ssh_consumer_send(&fixture, &view, cx, 14701,
        concat!("\x1b[2J\x1b[3J\x1b[6;5H\x1b_Ga=T,i=147,p=1,c=4,r=1,C=1,f=32,s=2,v=1;/wAA/wD/AP8=\x1b\\",
        "\x1b_Ga=f,i=147,f=32,s=2,v=1,z=60000;AAD///////8=\x1b\\",
        "\x1b_Ga=p,i=147,p=2,P=147,Q=1,H=3,V=2,c=4,r=1,C=1\x1b\\",
        "\x1b_Ga=p,i=147,p=3,U=1,c=4,r=1\x1b\\",
        "\x1b[10;5H\x1b[38;5;147;58;5;3m\u{10eeee}\u{305}\u{305}\u{10eeee}\u{10eeee}\u{10eeee}\x1b[0m",
        "\x1b_Ga=p,i=147,p=4,P=147,Q=3,H=3,V=2,c=4,r=1,C=1\x1b\\").as_bytes(),
        &[b"\x1b_Gi=147,p=1;OK\x1b\\", b"\x1b_Gi=147;OK\x1b\\", b"\x1b_Gi=147,p=2;OK\x1b\\", b"\x1b_Gi=147,p=3;OK\x1b\\", b"\x1b_Gi=147,p=4;OK\x1b\\"]);
    let root_bgra = [0, 0, 255, 255, 0, 255, 0, 255];
    native_ssh_consumer_assert_animation_graph(&view, cx, &root_bgra);
    // Partial frame clones frame two, then overwrites its first pixel. Edit
    // updates that exact immutable version; composition copies from frame one.
    native_ssh_consumer_send(&fixture, &view, cx, 14702,
        b"\x1b_Ga=f,i=147,c=2,X=1,s=1,v=1,z=60000;/wD//w==\x1b\\\x1b_Ga=a,i=147,s=1,c=3\x1b\\",
        &[b"\x1b_Gi=147;OK\x1b\\", b"\x1b_Gi=147;OK\x1b\\"]);
    native_ssh_consumer_assert_animation_graph(&view, cx, &[255, 0, 255, 255, 255, 255, 255, 255]);
    native_ssh_consumer_send(&fixture, &view, cx, 14703,
        b"\x1b_Ga=f,i=147,r=3,X=1,x=1,s=1,v=1;AP///w==\x1b\\\x1b_Ga=c,i=147,r=1,c=3,X=1,Y=0,w=1,h=1,x=0,y=0,C=1\x1b\\",
        &[b"\x1b_Gi=147;OK\x1b\\", b"\x1b_Gi=147;OK\x1b\\"]);
    let composed = [0, 255, 0, 255, 255, 255, 0, 255];
    let graph = native_ssh_consumer_assert_animation_graph(&view, cx, &composed);
    graphics_assert_production_paint(window, &view, cx);
    // Hold old render handles across an edit/selection; stale pixel claims may
    // never be revived by a later paint or fresh reader.
    let stale = view.read_with(cx, |view, _| graphics_visible_images(view));
    native_ssh_consumer_send(&fixture, &view, cx, 14704, b"\x1b_Ga=a,i=147,s=1,c=2\x1b\\", &[b"\x1b_Gi=147;OK\x1b\\"]);
    view.read_with(cx, |view, _| assert!(stale.iter().all(|image| !view.terminal.images().claim_for_paint(image))));
    drop(stale);
    native_ssh_consumer_assert_animation_graph(&view, cx, &[255, 0, 0, 255, 255, 255, 255, 255]);
    // A second actual RemoteTerminal/rooted view contributes visibility but is
    // an Observe subscription: no resize/control ownership takeover.
    let (mut stream, _) = fixture.connect_pane();
    ClientMsg::Observe { pane_id: pane, size: native_ssh_graphics_fixture::win_size() }.encode(&mut stream).unwrap();
    stream.set_read_timeout(None).unwrap();
    let (observer, observer_window) = graphics_build_rooted_consumer_at(cx, pane, stream, Vec::new(), TermSize::new(80, 24), PtySource::Raw);
    graphics_recovery_stats(&view, cx, 14705);
    graphics_assert_production_paint(observer_window, &observer, cx);
    native_ssh_consumer_assert_animation_graph(&observer, cx, &[255, 0, 0, 255, 255, 255, 255, 255]);
    view.read_with(cx, |view, _| view.terminal.viewport_reporter().set_displayed(false));
    native_ssh_consumer_send(&fixture, &view, cx, 14706, b"\x1b_Ga=a,i=147,s=3,c=2,r=2,z=60000,v=4\x1b\\", &[b"\x1b_Gi=147;OK\x1b\\"]);
    graphics_playback_wait(&observer, cx, |p| p.state == 3 && !p.paused);
    observer.read_with(cx, |view, _| view.terminal.viewport_reporter().set_displayed(false));
    let paused = graphics_playback_wait(&view, cx, |p| p.state == 3 && p.paused);
    assert!(paused.remaining_ms > 0 && paused.remaining_ms <= 60000);
    assert_eq!((paused.loops, paused.completed), (3, 0));
    // Explicit edits/delete remain authoritative while every GUI is hidden.
    native_ssh_consumer_send(&fixture, &view, cx, 14707, b"\x1b_Ga=d,d=f,i=147,r=2\x1b\\", &[]);
    native_ssh_consumer_assert_animation_graph(&view, cx, &composed);
    let hidden = graphics_playback_wait(&view, cx, |p| p.paused);
    assert_eq!((hidden.loops, hidden.completed), (paused.loops, paused.completed));
    assert_eq!(hidden.remaining_ms, paused.remaining_ms, "hidden mutation preserves the suspended remaining gap");
    graphics_recovery_destroy(observer, observer_window, cx);
    graphics_recovery_destroy(view, window, cx);
    let (resumed, resumed_window) = native_ssh_consumer_fresh(&fixture, pane, cx, 14770, 128 << 20);
    let restored = graphics_playback_wait(&resumed, cx, |p| p.state == 3 && !p.paused);
    assert_eq!((restored.loops, restored.completed), (hidden.loops, hidden.completed));
    assert!(restored.remaining_ms > 0 && restored.remaining_ms <= hidden.remaining_ms, "fresh consumer resumes the remaining interval, not a new full interval or a catch-up loop");
    native_ssh_consumer_assert_animation_graph(&resumed, cx, &composed);
    resumed.read_with(cx, |view, _| view.terminal.viewport_reporter().set_displayed(false));
    graphics_playback_wait(&resumed, cx, |p| p.paused);
    graphics_recovery_destroy(resumed, resumed_window, cx);
    // Truly zero consumers: append, root edit, select and remove a separate
    // graph while no desktop store/window exists. Gate uses a private Linux
    // file after the sender's actual query ACK, never an observing viewport.
    fixture.detached(&host, pane, 14708,
        b"\x1b_Ga=f,i=147,f=32,s=2,v=1,q=2,z=60000;/////wAAAP8=\x1b\\\x1b_Ga=f,i=147,r=1,X=1,s=1,v=1,q=2;AQID/w==\x1b\\\x1b_Ga=a,i=147,s=1,c=3,q=2\x1b\\\x1b[16;1H\x1b_Ga=T,i=148,p=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\\x1b_Ga=d,d=I,i=148,q=2\x1b\\DETACHED-SSH-LATEST");
    let (fresh, fresh_window) = native_ssh_consumer_fresh(&fixture, pane, cx, 14709, 128 << 20);
    let latest = [255, 255, 255, 255, 0, 0, 0, 255];
    assert_eq!(native_ssh_consumer_assert_animation_graph(&fresh, cx, &latest), graph, "latest frame changes no parent/prototype/placement geometry");
    fresh.read_with(cx, |view, _| {
        assert!(view.scrollback_text().contains("DETACHED-SSH-LATEST"));
        assert!(graphics_visible_images(view).iter().all(|image| image.placement.image_id != 148));
    });
    let stopped = graphics_recovery_stats(&fresh, cx, 14710);
    graphics_recovery_destroy(fresh, fresh_window, cx);
    let (fresh, fresh_window) = native_ssh_consumer_fresh(&fixture, pane, cx, 14711, 128 << 20);
    native_ssh_consumer_assert_animation_graph(&fresh, cx, &latest);
    assert_eq!(graphics_recovery_stats(&fresh, cx, 14712), stopped, "silent sender must not retransmit root or frames on fresh recovery");
    assert_eq!(native_ssh_consumer_identity(&fixture, pane), identity);
    // Uppercase final-frame deletion releases the full relative/virtual graph.
    native_ssh_consumer_send(&fixture, &fresh, cx, 14713, b"\x1b_Ga=d,d=f,i=147,r=3\x1b\\\x1b_Ga=d,d=f,i=147,r=2\x1b\\\x1b_Ga=d,d=F,i=147,r=1\x1b\\", &[]);
    graphics_assert_production_paint(fresh_window, &fresh, cx);
    fresh.read_with(cx, |view, _| assert!(graphics_visible_images(view).is_empty()));
    graphics_recovery_destroy(fresh, fresh_window, cx);
    let (last, last_window) = native_ssh_consumer_fresh(&fixture, pane, cx, 14714, 128 << 20);
    last.read_with(cx, |view, _| assert!(graphics_visible_images(view).is_empty(), "deleted frames/prototypes cannot resurrect"));
    graphics_recovery_destroy(last, last_window, cx);
    fixture.kill_pane(pane);
}

#[gpui_kit::test]
#[ignore = "requires real Linux native SSH, writable /dev/shm; decoder/production paint, not native GPU evidence"]
fn native_ssh_consumer_linux_file_temp_shm_pixels_and_silent_recovery(cx: &mut TestAppContext) {
    native_ssh_consumer_init(cx);
    let fixture = native_ssh_consumer_fixture();
    let host = fixture.connect_control();
    let desktop = tempfile::tempdir().unwrap();
    let sentinel = desktop.path().join(format!("tty-graphics-protocol-{}.rgba", desktop.path().file_name().unwrap().to_string_lossy()));
    std::fs::write(&sentinel, [91, 72, 53, 255]).unwrap();
    let shell = fixture.install_script(&host, "kitty_ssh_media.mjs", include_bytes!("../../tests/support/kitty_ssh_media.mjs"), vec!["--desktop-path".into(), sentinel.to_string_lossy().into_owned()]);
    let pane = native_ssh_consumer_spawn(&fixture, shell);
    let identity = native_ssh_consumer_identity(&fixture, pane);
    // This sender intentionally does not implement STAT. Mount its real stream
    // directly and wait for its own correlated output after remote media ACKs.
    let (mut stream, _) = fixture.connect_pane();
    ClientMsg::Attach { pane_id: pane, size: native_ssh_graphics_fixture::win_size(), allow_remote_clipboard_write: false }.encode(&mut stream).unwrap();
    stream.set_read_timeout(None).unwrap();
    let (view, window) = graphics_build_rooted_consumer_at(cx, pane, stream, Vec::new(), TermSize::new(80, 24), PtySource::Raw);
    fixture.input(pane, b"RUN\n".to_vec());
    graphics_recovery_wait(cx, "real Linux media sender never received its file/temp/shm ACKs", |cx| view.read_with(cx, |view, _| view.scrollback_text().contains("KITTY_REMOTE_MEDIA_OK")));
    for id in 1471..=1473 { graphics_recovery_pixels(&view, cx, id, &[56, 34, 12, 255]); }
    graphics_recovery_pixels(&view, cx, 1474, &[255, 0, 255, 255]);
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(std::fs::read(&sentinel).unwrap(), [91, 72, 53, 255]);
    graphics_recovery_destroy(view, window, cx);
    let (mut stream, _) = fixture.connect_pane();
    ClientMsg::Attach { pane_id: pane, size: native_ssh_graphics_fixture::win_size(), allow_remote_clipboard_write: false }.encode(&mut stream).unwrap();
    stream.set_read_timeout(None).unwrap();
    let (fresh, window) = graphics_build_rooted_consumer_at(cx, pane, stream, Vec::new(), TermSize::new(80, 24), PtySource::Raw);
    for id in 1471..=1473 { graphics_recovery_pixels(&fresh, cx, id, &[56, 34, 12, 255]); }
    graphics_recovery_pixels(&fresh, cx, 1474, &[255, 0, 255, 255]);
    graphics_assert_production_paint(window, &fresh, cx);
    assert_eq!(native_ssh_consumer_identity(&fixture, pane), identity);
    assert_eq!(std::fs::read(&sentinel).unwrap(), [91, 72, 53, 255]);
    graphics_recovery_destroy(fresh, window, cx);
    fixture.input(pane, b"QUIT\n".to_vec());
    fixture.kill_pane(pane);
}
