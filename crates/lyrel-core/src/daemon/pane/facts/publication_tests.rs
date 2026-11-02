use super::super::test_support::{null_writer, test_state};
use super::super::*;
use super::*;

#[test]
fn observed_facts_prefer_the_sessions_argv_and_carry_its_status() {
    use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};

    let mut st = test_state(true);
    assert_eq!(
        observed_facts(&st.facts),
        ObservedFacts {
            cwd: None,
            osc_title: None,
            agent: None,
            shell: None,
        }
    );

    st.facts.cwd = Some(PathBuf::from("/work/api"));
    st.facts.agent = Some(CLIAgent::OhMyPi);
    st.facts.agent_argv = Some(vec!["omp".into()]);
    st.facts.agent_session = Some(AgentSessionState {
        status: AgentStatus::Working,
        session_id: Some("sess-1".into()),
        launch_argv: Some(vec!["omp".into(), "--model".into(), "opus".into()]),
        ..Default::default()
    });

    let facts = observed_facts(&(&st).facts);
    assert_eq!(facts.cwd.as_deref(), Some("/work/api"));
    let agent = facts.agent.expect("an agent in the foreground is a fact");
    assert_eq!(agent.agent, CLIAgent::OhMyPi);
    assert_eq!(agent.session_id.as_deref(), Some("sess-1"));
    assert_eq!(
        agent.launch_argv.as_deref(),
        Some(&["omp".to_string(), "--model".into(), "opus".into()][..]),
        "the session's own argv outranks the identity poll's capture"
    );
    assert_eq!(agent.status, Some(AgentStatus::Working));

    st.facts.agent_session = None;
    let facts = observed_facts(&(&st).facts);
    assert_eq!(
        facts.agent.unwrap().launch_argv.as_deref(),
        Some(&["omp".to_string()][..])
    );
}

/// The whole path a title takes into the tree: sniffed out of the pane's
/// own output, kept on its state, and written to the record every other
/// viewer reads. Nothing else can name the tabs of a workspace this process
/// does not own — without this the switcher fell back to the agent, and
/// every tab of a workspace running one read "Claude Code".
#[test]
fn a_pane_title_reaches_the_machine_tree() {
    use crate::core::machine::{
        MACHINE_FILE, MachineStore, OBSERVE_SLOT, PaneSeed, publish_observations,
        withdraw_observations,
    };

    const PANE: u64 = 78;
    let _slot = OBSERVE_SLOT.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::TempDir::new().unwrap();
    let store = MachineStore::open(dir.path().join(MACHINE_FILE));
    let ws = store.workspace_create(None, None, None).unwrap();
    store
        .tab_create(ws.id, None, PaneSeed::bare(PANE), None, None)
        .unwrap();
    publish_observations(&store);

    let run = |had: Option<&str>, output: &[u8]| {
        let mut state = test_state(true);
        state.id = PANE;
        state.facts.osc_title = had.map(str::to_string);
        DaemonPane::spawn_reader(
            Arc::new(Mutex::new(state)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(OutputGate::new()),
            Box::new(std::io::Cursor::new(output.to_vec())),
            null_writer(),
            || false,
            ForegroundProbes {
                group: Box::new(|| None),
                remote: Box::new(|| None),
                agent: Box::new(|| Some(None)),
                cwd: Box::new(|| None),
                local_group: |_| Some(true),
            },
            Arc::new(DeathReporter::new(|| {})),
        )
        .join()
        .unwrap();
        store.pane(PANE).expect("the record was seeded").osc_title
    };

    assert_eq!(
        run(None, b"\x1b]0;\xe2\x9c\xb3 fixing the switcher\x07").as_deref(),
        Some("✳ fixing the switcher")
    );
    assert_eq!(
        run(Some("✳ fixing the switcher"), b"\x1b]2;\x07"),
        None,
        "a pane that resets its title clears the one on record"
    );

    withdraw_observations();
}

#[test]
fn output_readiness_cannot_overtake_its_osc_facts_or_wait_for_process_probes() {
    use super::super::test_support::drain;
    use std::time::Duration;

    let mut pane = test_state(true);
    let (sender, receiver) = mpsc::channel();
    attach_subscriber(&mut pane, sender);
    drain(&receiver);
    let (entered_tx, entered) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let state = Arc::new(Mutex::new(pane));
    let bytes = b"\x1b]0;first-osc\x07\x1b]7;file:///work/ready\x07\x1b]133;A\x07OSC_READY\r\n";
    let reader = DaemonPane::spawn_reader(
        state.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(OutputGate::new()),
        Box::new(std::io::Cursor::new(bytes)),
        null_writer(),
        move || {
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            true
        },
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| Some(None)),
            cwd: Box::new(|| None),
            local_group: |_| Some(true),
        },
        Arc::new(DeathReporter::new(|| {})),
    );
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    // The real reader is now paused in a foreground probe, not racing this
    // client. Its marker must already be consumable with the matching OSC facts.
    let output: Vec<_> = receiver.try_iter().filter_map(|message| match message {
        DaemonMsg::Output(bytes) => Some(bytes),
        _ => None,
    }).flatten().collect();
    let facts = {
        let pane = state.lock().unwrap();
        (pane.facts.osc_title().map(str::to_string), pane.facts.cwd().map(Path::to_path_buf))
    };
    // Always release the real worker before assertions, including a RED run.
    release.send(()).unwrap();
    reader.join().unwrap();
    assert_eq!(output, bytes);
    assert_eq!(facts.0.as_deref(), Some("first-osc"));
    assert_eq!(facts.1.as_deref(), Some(Path::new("/work/ready")));
    let pane = state.lock().unwrap();
    assert!(!pane.facts.shell.at_prompt, "relayed prompts stay suppressed after the probe");
    assert!(pane.facts.shell.mark_at_prompt);
}
