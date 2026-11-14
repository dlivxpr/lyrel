// Real PTY -> daemon/ordered transport -> production TerminalView coverage of
// Glyph Protocol support negotiation. A real client (omp's pinned
// glyph-protocol.ts) sends `s` and its confirmation `q` in the same write as a
// DA1 sentinel and fails the terminal outright when DA1 comes back first, so
// these cases pin the daemon's reply ordering against the production
// consumer's own Device Attributes answer — not just "some reply exists".
//
// These consumers negotiate through the production rendering handshake; the
// state-only replacement exercises the same handshake without that credential.

/// Attach a consumer stream to `pane` like `daemon.stream`, optionally also
/// declaring the monochrome-render opt-in when the endpoint advertises it.
fn glyph_support_stream(daemon: &GraphicsDaemonProcess, pane: u64, glyph_render: bool) -> Stream {
    let mut stream =
        crate::daemon::transport::connect_endpoint_at(&daemon.endpoint("daemon")).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    crate::daemon::protocol::negotiate_pane_rendering(&mut stream, glyph_render).unwrap();
    ClientMsg::GraphicsCapacity {
        encoded_bytes: 64 << 20,
        decoded_bytes: 128 << 20,
        offers: true,
    }
    .encode(&mut stream)
    .unwrap();
    stream.set_read_timeout(None).unwrap();
    ClientMsg::Attach {
        pane_id: pane,
        size: crate::daemon::protocol::WinSize {
            cols: 80,
            rows: 24,
            cell_w: 10,
            cell_h: 20,
        },
        allow_remote_clipboard_write: false,
    }
    .encode(&mut stream)
    .unwrap();
    stream
}

/// Daemon-layer isolation: a controller attached with both glyph opt-ins must
/// see the frontend-bound `GlyphQuery` frame when the producer writes `q`,
/// and its `GlyphQueryResult` must come back through the PTY as the single
/// composed reply. The `s` probe on the same pane must emit the `fmt=glyf`
/// advertisement. This pins the server collect → credential → dispatch →
/// reply chain without the production consumer's own answer plumbing.
#[test]
fn glyph_support_probe_reaches_the_opted_in_controller() {
    use crate::daemon::protocol::DaemonMsg;

    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let mut stream = glyph_support_stream(&daemon, pane, true);
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let replies = tempfile::TempDir::new().unwrap();

    // `q` is frontend-bound: dispatch arrives on this controller stream, and
    // the fixture captures the reply the daemon writes for our answer.
    let q_path = replies.path().join("q");
    let request = format!(
        "SEND_ANY 1 1 {} 0 {}\n",
        graphics_pty_fixture::hex(b"\x1b_25a1;q;cp=100000\x1b\\"),
        graphics_pty_fixture::hex(q_path.to_string_lossy().as_bytes())
    );
    // The controller must be attached before its frontend-bound query runs;
    // input on a separate connection can overtake Attach on a busy host.
    ClientMsg::Input(request.into_bytes())
        .encode(&mut stream)
        .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let (token, cp) = loop {
        assert!(
            std::time::Instant::now() < deadline,
            "the daemon never framed the query to its opted-in controller"
        );
        match DaemonMsg::read(&mut stream) {
            Ok(DaemonMsg::GlyphQuery { token, cp }) => break (token, cp),
            Ok(_) => continue,
            Err(error) => panic!("controller stream failed: {error}"),
        }
    };
    assert_eq!(cp, 0x100000);
    ClientMsg::GlyphQueryResult {
        token,
        cp,
        system: false,
    }
    .encode(&mut stream)
    .unwrap();

    let wait_reply = |path: &std::path::Path| -> Vec<u8> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if path.with_extension("done").is_file()
                && let Ok(text) = std::fs::read_to_string(path)
            {
                return graphics_pty_fixture::unhex(text.trim());
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the pane never wrote a reply for {}",
                path.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    };
    assert_eq!(
        wait_reply(&q_path),
        b"\x1b_25a1;q;cp=100000;status=\x1b\\".to_vec(),
        "the daemon must write the single composed `q` reply to the PTY"
    );
    // `s` binds at attach; its reply is daemon-written and needs no
    // controller, so it lands on the same capture path a real client reads.
    let s_path = replies.path().join("s");
    let request = format!(
        "SEND_ANY 2 1 {} 0 {}\n",
        graphics_pty_fixture::hex(b"\x1b_25a1;s\x1b\\"),
        graphics_pty_fixture::hex(s_path.to_string_lossy().as_bytes())
    );
    ClientMsg::Input(request.into_bytes())
        .encode(&mut stream)
        .unwrap();
    assert_eq!(
        wait_reply(&s_path),
        b"\x1b_25a1;s;fmt=glyf\x1b\\".to_vec(),
        "an opted-in controller must earn the `fmt=glyf` advertisement"
    );
}

/// `glyph_send` for a request that yields more than one reply: the fixture
/// writes the replies comma-separated into the request's gate file.
fn glyph_send_parts(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    serial: usize,
    reply_path: &std::path::Path,
    bytes: &[u8],
    replies: usize,
) -> Vec<Vec<u8>> {
    let path = reply_path.to_string_lossy();
    let request = format!(
        "SEND_ANY {serial} {replies} {} 0 {}\n",
        graphics_pty_fixture::hex(bytes),
        graphics_pty_fixture::hex(path.as_bytes()),
    );
    let input_bytes = request.len();
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    graphics_wait_fixture(view, cx, serial, input_bytes);
    let text = std::fs::read_to_string(reply_path)
        .unwrap_or_else(|error| panic!("no reply file for serial {serial}: {error}"));
    text.trim()
        .split(',')
        .map(graphics_pty_fixture::unhex)
        .collect()
}

/// The `s` probe must be answered in stream order, ahead of the DA1 the same
/// producer write carries next — a real client treats "DA1 first" as "glyph
/// protocol unsupported". `q` then answers the same way against the pane's
/// real glossary and font coverage. Both replies have to leave the single
/// ordered writer before the consumer's own `ESC [ c` answer can arrive.
#[gpui_kit::test]
fn glyph_consumer_support_and_query_replies_precede_the_da1_sentinel(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (view, _window) =
        graphics_rooted_consumer(cx, pane, glyph_support_stream(&daemon, pane, true));
    // ConPTY consumes the first DA1 response as its own terminal identity.
    // Real omp negotiates that before Glyph; prime the fixture the same way
    // without changing the measured q/s + DA1 writes or expected replies.
    #[cfg(windows)]
    graphics_send_fixture(&view, cx, 9, b"\x1b[c");
    let replies = tempfile::TempDir::new().unwrap();
    // First prove the controller is bound: the `q` round trip requires the
    // pane's current subscriber, so a `q`+DA1 pair that both answer shows the
    // subscriber/glyph-state credentials took effect before the `s` probe.
    let parts = glyph_send_parts(
        &view,
        cx,
        1,
        &replies.path().join("q_da1"),
        &[glyph_query(GLYPH_CP), b"\x1b[c".to_vec()].concat(),
        2,
    );
    assert_eq!(
        parts.len(),
        2,
        "q and DA1 each answer exactly once: {parts:?}"
    );
    assert_eq!(
        parts[0],
        format!("\x1b_25a1;q;cp={GLYPH_CP:x};status=system\x1b\\").into_bytes(),
        "the query reply precedes DA1"
    );
    assert_eq!(
        parts[1],
        b"\x1b[?4;6c".to_vec(),
        "DA1 follows the query reply"
    );

    // The pinned client writes `s` and `ESC [ c` together and accepts the
    // protocol only when the format list beats the DA1 back.
    let parts = glyph_send_parts(
        &view,
        cx,
        2,
        &replies.path().join("s_da1"),
        b"\x1b_25a1;s\x1b\\\x1b[c",
        2,
    );
    assert_eq!(
        parts,
        [
            b"\x1b_25a1;s;fmt=glyf\x1b\\".to_vec(),
            b"\x1b[?4;6c".to_vec()
        ],
        "the support reply must list only glyf and precede DA1"
    );

    // Register the client's confirmation codepoint the way the pinned client
    // does — reply=0 so success is silent — then prove the `q` status now
    // carries `glossary`, still ahead of the sentinel.
    let parts = glyph_send_parts(
        &view,
        cx,
        3,
        &replies.path().join("rq_da1"),
        &[
            glyph_register(GLYPH_CP, &glyph_square(1000), 0),
            glyph_query(GLYPH_CP),
            b"\x1b[c".to_vec(),
        ]
        .concat(),
        2,
    );
    assert_eq!(
        parts[0],
        format!("\x1b_25a1;q;cp={GLYPH_CP:x};status=system,glossary\x1b\\").into_bytes(),
        "the registered codepoint reports glossary ahead of DA1"
    );
    assert_eq!(parts[1], b"\x1b[?4;6c".to_vec());
}

/// `s` and `q` are bound to the pane's current coverage credential: a
/// rendering controller is answered `fmt=glyf` and routed `GlyphQuery`,
/// while a controller that only subscribed to the retained projection — the
/// CLI `run` shape, decode-only with no font stack — is asked neither. Its
/// probes stay silent, proved here by the DA1 the same write carries next:
/// one reply for a three-message write, and it is the sentinel. Each phase
/// is a real production consumer: the replacement's attach demotes the
/// previous controller.
#[gpui_kit::test]
fn glyph_consumer_support_answer_tracks_the_controllers_render_opt_in(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let replies = tempfile::TempDir::new().unwrap();

    // A controller that declared the render opt-in is answered `fmt=glyf`
    // before the DA1 its own write carried.
    let (view, _window) =
        graphics_rooted_consumer(cx, pane, glyph_support_stream(&daemon, pane, true));
    // ConPTY's initial DA1 belongs to the host, not the application stdin.
    #[cfg(windows)]
    graphics_send_fixture(&view, cx, 9, b"\x1b[c");
    let parts = glyph_send_parts(
        &view,
        cx,
        10,
        &replies.path().join("rendering_da1"),
        b"\x1b_25a1;s\x1b\\\x1b[c",
        2,
    );
    assert_eq!(
        parts[0],
        b"\x1b_25a1;s;fmt=glyf\x1b\\".to_vec(),
        "the render-opted controller is advertised glyf first"
    );
    assert_eq!(parts[1], b"\x1b[?4;6c".to_vec());

    // A replacement controller that subscribed to the projection but never
    // declared it renders v1.12 holds no coverage credential: `s` stays
    // silent, and its `q` is never routed to a peer that could only drop it.
    // The DA1 is the write's only reply.
    let (view, _window) =
        graphics_rooted_consumer(cx, pane, glyph_support_stream(&daemon, pane, false));
    let parts = glyph_send_parts(
        &view,
        cx,
        11,
        &replies.path().join("state_only_da1"),
        &[
            b"\x1b_25a1;s\x1b\\".to_vec(),
            glyph_query(GLYPH_CP),
            b"\x1b[c".to_vec(),
        ]
        .concat(),
        1,
    );
    assert_eq!(
        parts,
        [b"\x1b[?4;6c".to_vec()],
        "without the render opt-in `s` and `q` stay silent; only DA1 replies"
    );

    // Controller replacement again — rendering declared — re-enables the
    // advertisement for the pane it now controls.
    let (view, _window) =
        graphics_rooted_consumer(cx, pane, glyph_support_stream(&daemon, pane, true));
    let parts = glyph_send_parts(
        &view,
        cx,
        12,
        &replies.path().join("rendering_again_da1"),
        b"\x1b_25a1;s\x1b\\\x1b[c",
        2,
    );
    assert_eq!(
        parts[0],
        b"\x1b_25a1;s;fmt=glyf\x1b\\".to_vec(),
        "the render opt-in binds at attach, not at pane lifetime"
    );
    assert_eq!(parts[1], b"\x1b[?4;6c".to_vec());
}

/// The retained-projection opt-in is a decode/transport credential, not a
/// coverage answerer: a `negotiate_pane` controller — the CLI `run`
/// backend's exact shape, which decodes `GlyphState` but drops a
/// `GlyphQuery` kind it has no arm for — must never be handed the query at
/// all. Producer order makes silence observable: `q` precedes the marker
/// text in one write, so a `GlyphQuery` frame arriving before that marker
/// would mean the daemon routed coverage to a peer that cannot answer.
#[test]
fn glyph_support_query_never_reaches_a_state_only_controller() {
    use crate::daemon::protocol::DaemonMsg;

    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let mut stream = glyph_support_stream(&daemon, pane, false);
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();

    // The controller must be attached before its stream position is read:
    // input on the same connection cannot overtake Attach.
    let request = format!(
        "SEND_ANY 1 0 {} 0\n",
        graphics_pty_fixture::hex(b"\x1b_25a1;q;cp=100000\x1b\\")
    );
    ClientMsg::Input(request.into_bytes())
        .encode(&mut stream)
        .unwrap();

    // The fixture completes the request with `KITTY_DONE_1` output — the
    // ordered marker after the unanswered `q`. Any `GlyphQuery` seen first
    // means the daemon asked a coverage question of a peer that can only
    // drop it; the producer would wait forever for a reply.
    let mut seen = Vec::new();
    let marker = b"KITTY_DONE_1";
    while !seen
        .windows(marker.len())
        .any(|window| window == marker.as_slice())
    {
        match DaemonMsg::read(&mut stream) {
            Ok(DaemonMsg::GlyphQuery { .. }) => {
                panic!("a decode-only controller was handed the coverage query")
            }
            Ok(DaemonMsg::Output(bytes)) | Ok(DaemonMsg::Snapshot(bytes)) => {
                seen.extend(bytes)
            }
            Ok(DaemonMsg::Exited { code }) => {
                panic!("the producer exited before its marker: {code:?}")
            }
            Ok(_) => continue,
            Err(error) => panic!("controller stream failed: {error}"),
        }
    }
}
