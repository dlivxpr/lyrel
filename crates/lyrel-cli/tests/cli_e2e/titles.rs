use super::*;
use lyrel_core::daemon::control::{ControlHello, ControlRequest, ReplyOk};
use lyrel_core::daemon::protocol::FEATURE_VALIDATED_TITLES;

/// What the daemon's authoritative tree says a pane calls itself. This is the
/// fact the CLI's tree/switcher views and every other control consumer read.
fn osc_title(control: &ControlClient, pane: u64) -> Option<String> {
    let ReplyOk::MachineTree(machine) = control
        .request(ControlRequest::MachineGet)
        .expect("read the machine tree")
    else {
        panic!("MachineGet answered with something else");
    };
    machine
        .panes
        .iter()
        .find(|record| record.id == pane)
        .and_then(|record| record.osc_title.clone())
}

fn wait_for_title(control: &ControlClient, pane: u64, want: Option<&str>) {
    let deadline = Instant::now() + SETTLE_WITHIN;
    loop {
        let got = osc_title(control, pane);
        if got.as_deref() == want {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "daemon title fact never became {want:?}; last was {got:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The real raw PTY, the isolated production daemon, and the public control
/// protocol: no GUI and no desktop consumer can supply the title fact. The
/// pinned endpoint advertises the validated-title capability, and every rule
/// the window applies is applied to the daemon's own last-valid-title state.
pub(super) fn validated_titles_reach_daemon_facts_from_a_real_pty(daemon: &Daemon) {
    let version = PaneClient::at(daemon.pane_endpoint())
        .version()
        .expect("the pane endpoint negotiates");
    assert!(
        version.has_feature(FEATURE_VALIDATED_TITLES),
        "this daemon must advertise the title contract it implements"
    );

    let shell = ShellSpec {
        program: std::env::current_exe().unwrap().display().to_string(),
        args: vec![TERMINAL_PTY_FIXTURE_ARG.into()],
        args_are_lyrel_defaults: false,
    };
    let mut pane = PaneClient::at(daemon.pane_endpoint())
        .spawn(
            None,
            WinSize {
                cols: 80,
                rows: 24,
                cell_w: 8,
                cell_h: 16,
            },
            Some(shell),
            Some("validated-titles-e2e".into()),
            None,
        )
        .expect("spawn the raw PTY title fixture");
    pane.set_recv_timeout(Some(SETTLE_WITHIN)).unwrap();
    collect_pane_output_until(&mut pane, b"KITTY_READY");
    let pane_id = pane.pane_id();

    let control = ControlClient::connect_at(
        &daemon.control_endpoint(),
        &ControlHello::host_rpc("e2e-titles", "e2e-titles"),
    )
    .expect("open the control link that owns the machine tree");

    let mut serial = 0;
    let mut write_title = |bytes: &[u8]| {
        serial += 1;
        pane.input(
            format!(
                "TITLE {serial} 0 {} 0\n",
                terminal_pty_fixture::hex(bytes)
            )
            .as_bytes(),
        )
        .expect("write a title through the real PTY");
        collect_pane_output_until(
            &mut pane,
            format!("TITLE_DONE_{serial}").as_bytes(),
        );
    };

    // 255 bytes exactly, with leading/trailing spaces and semicolons that
    // are part of the payload: the fact must keep them, not trim them.
    let at_limit = format!("  {} ;end ", "t".repeat(247));
    assert_eq!(at_limit.len(), 255);
    write_title(format!("\x1b]0;{at_limit}\x1b\\").as_bytes());
    wait_for_title(&control, pane_id, Some(&at_limit));

    // 256 bytes is rejected whole: the previous valid title stands.
    let over = "x".repeat(256);
    write_title(format!("\x1b]2;{over}\x07").as_bytes());
    wait_for_title(&control, pane_id, Some(&at_limit));

    // A multibyte boundary measured in UTF-8 bytes: 85 CJK characters fit
    // exactly; 86 exceed the budget and leave the previous title in place.
    let cjk = "中".repeat(85);
    assert_eq!(cjk.len(), 255);
    write_title(format!("\x1b]2;{cjk}\x1b\\").as_bytes());
    wait_for_title(&control, pane_id, Some(&cjk));
    let cjk_over = "中".repeat(86);
    write_title(format!("\x1b]2;{cjk_over}\x1b\\").as_bytes());
    wait_for_title(&control, pane_id, Some(&cjk));

    // Strict UTF-8, also rejected without disturbing the standing title.
    // ConPTY may rewrite arbitrary invalid bytes before the daemon sees
    // them, so the raw-byte path is pinned where the PTY is byte-exact; the
    // desktop consumer covers the same payload on every platform.
    #[cfg(unix)]
    {
        write_title(b"\x1b]2;bad \xff utf8\x07");
        wait_for_title(&control, pane_id, Some(&cjk));
    }

    // A valid empty payload is the reset, not an ignored report.
    write_title(b"\x1b]2;\x07");
    wait_for_title(&control, pane_id, None);

    // Prompt title -> command start -> rejected title -> command finish: the
    // rejected report must not make the prompt title command-owned.
    write_title(b"\x1b]133;A\x07\x1b]0;prompt title\x07\x1b]133;B\x07");
    wait_for_title(&control, pane_id, Some("prompt title"));
    write_title(
        b"\x1b]133;C;claude\x07\x1b]2;this title is far too long to be accepted \
          and is rejected whole before it could touch the kept prompt title and its \
          own command lifetime metadata, which must survive the finished command \
          exactly as it stood when the prompt wrote it a moment ago, because a \
          rejected report is inert\x07",
    );
    wait_for_title(&control, pane_id, Some("prompt title"));
    write_title(b"\x1b]133;D;0\x07");
    wait_for_title(&control, pane_id, Some("prompt title"));

    // An accepted command title still retires at its `D`.
    write_title(b"\x1b]133;C;ls\x07\x1b]2;ls -la\x07");
    wait_for_title(&control, pane_id, Some("ls -la"));
    write_title(b"\x1b]133;D;0\x07");
    wait_for_title(&control, pane_id, None);
}
