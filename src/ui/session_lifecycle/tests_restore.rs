use super::{leaf_shares_the_window_daemon, native_ssh_pane_alive, pane_free_for, split_machine_held};
    /// The machine stops the panes it holds; a native SSH pane in a remote
    /// window lives in this computer's daemon and is ended from here, because
    /// nothing on the window's machine could.
    #[test]
    fn a_remote_windows_native_ssh_pane_is_ended_here_and_the_rest_by_its_machine() {
        let local = crate::terminal::PaneRoute::Local;
        let leaves = vec![(4, local.clone(), false), (5, local.clone(), true)];

        let (held, here) = split_machine_held(true, &leaves);
        let ids = |v: &[(u64, crate::terminal::PaneRoute)]| {
            v.iter().map(|(id, _)| *id).collect::<Vec<_>>()
        };
        assert_eq!(ids(&held), vec![4]);
        assert_eq!(ids(&here), vec![5]);

        let (held, here) = split_machine_held(false, &leaves);
        assert_eq!(
            ids(&held),
            vec![4, 5],
            "a local window's machine holds its SSH panes too"
        );
        assert!(here.is_empty());
    }
    #[test]
    fn a_layout_that_came_back_short_says_how_short() {
        crate::ui::i18n::set_locale("en");
        for n in [1usize, 2, 9] {
            let text =
                crate::ui::i18n::t_plural(crate::ui::i18n::L10nKey::AppTabsNotRestored, n, &[]);
            assert!(text.contains(&n.to_string()), "{n}: {text}");
            assert!(!text.contains("{count}"), "the placeholder leaked: {text}");
        }
        // Singular is not "1 tabs".
        let one = crate::ui::i18n::t_plural(crate::ui::i18n::L10nKey::AppTabsNotRestored, 1, &[]);
        assert!(one.contains("1 tab "), "{one}");
    }
    #[test]
    fn restore_only_attaches_panes_the_workspace_owns_or_nobody_claims() {
        let ours = crate::core::session::WorkspaceId::new();
        let theirs = crate::core::session::WorkspaceId::new();
        let alive: std::collections::HashMap<u64, Option<String>> = [
            (1, Some(ours.to_string())),
            (2, Some(theirs.to_string())),
            (3, None),
            (5, Some("lyrel-cli".to_string())),
        ]
        .into_iter()
        .collect();

        assert!(
            pane_free_for(Some(&alive), 1, ours),
            "our own pane attaches"
        );
        assert!(
            !pane_free_for(Some(&alive), 2, ours),
            "another workspace's pane must spawn fresh instead"
        );
        assert!(
            pane_free_for(Some(&alive), 3, ours),
            "an unowned pane is legacy"
        );
        assert!(
            pane_free_for(Some(&alive), 5, ours),
            "an owner that names no workspace is not a rival's claim: older CLIs \
             wrote their own name there, and respawning strands the live pane"
        );
        assert!(
            pane_free_for(None, 4, ours),
            "a failed List says nothing about pane 4; the attach itself must decide, \
             because respawning on a transient RPC error destroys a live session"
        );
    }

    #[test]
    fn a_dead_pane_keeps_its_id_so_its_screen_can_be_asked_for() {
        let ours = crate::core::session::WorkspaceId::new();
        let theirs = crate::core::session::WorkspaceId::new();
        let alive: std::collections::HashMap<u64, Option<String>> =
            [(1, Some(ours.to_string())), (2, Some(theirs.to_string()))]
                .into_iter()
                .collect();

        // The restart case: every pane the window held is missing from the new
        // daemon's listing. Their ids are the only handle on the screens it
        // still has stored, so being dead must not erase them — this is what
        // made a restarted server come back to a row of blank shells.
        assert!(
            pane_free_for(Some(&alive), 4, ours),
            "a dead pane's id has to survive; the restore is keyed on it"
        );

        // What being free does not mean: helping yourself to a pane that is
        // alive and belongs to another workspace, whose screen is not this
        // window's to show either.
        assert!(
            !pane_free_for(Some(&alive), 2, ours),
            "another workspace's pane is not ours to restore from"
        );
        assert!(pane_free_for(Some(&alive), 1, ours), "our own pane is ours");
    }

    #[test]
    fn a_native_ssh_leaf_in_a_remote_window_is_not_looked_up_in_the_remote_daemon() {
        assert!(!leaf_shares_the_window_daemon(true, true));
        assert!(leaf_shares_the_window_daemon(true, false));
        assert!(leaf_shares_the_window_daemon(false, true));
        assert!(leaf_shares_the_window_daemon(false, false));
    }
    #[test]
    fn a_native_ssh_pane_the_server_no_longer_has_is_dialled_again() {
        let mut alive = std::collections::HashMap::new();
        alive.insert(4u64, None);
        assert!(native_ssh_pane_alive(Some(&alive), 4));
        assert!(
            !native_ssh_pane_alive(Some(&alive), 9),
            "gone: dial, don't attach"
        );
        assert!(
            !native_ssh_pane_alive(None, 4),
            "no listing: dial, don't guess"
        );
    }
    #[gpui_kit::test]
    fn a_connecting_pane_saves_the_agent_it_is_rebuilding(cx: &mut gpui_kit::TestAppContext) {
        use crate::core::cli_agent::CLIAgent;
        use crate::core::session::SessionPane;
        use crate::ui::pane::{Pane, PaneSlot};
        use crate::ui::pending_pane::{PendingPane, PendingSpawn};
        use gpui_kit::AppContext as _;

        cx.update(|cx| {
            let pending = cx.new(|cx| {
                PendingPane::new(
                    "build-box",
                    PendingSpawn {
                        workspace: None,
                        working_directory: Some(std::path::PathBuf::from("/work")),
                        restore_pane: Some(7),
                        shell: None,
                        agent: Some(CLIAgent::OhMyPi),
                        agent_session_id: Some("sid-abc".to_string()),
                        agent_launch_argv: Some(vec!["omp".to_string()]),
                        agent_session_scope: None,
                        run_on_land: None,
                        owner: None,
                        font_size: 14.0,
                    },
                    cx,
                )
            });
            let saved = super::pane_to_session(&Pane::leaf(PaneSlot::Connecting(pending)), cx);
            let SessionPane::Leaf {
                pane_id,
                agent,
                agent_session_id,
                agent_launch_argv,
                ..
            } = saved
            else {
                panic!("a leaf saves as a leaf");
            };
            assert_eq!(pane_id, Some(7), "the id it is re-attaching to");
            assert_eq!(agent, Some(CLIAgent::OhMyPi));
            assert_eq!(agent_session_id.as_deref(), Some("sid-abc"));
            assert_eq!(agent_launch_argv, Some(vec!["omp".to_string()]));
        });
    }
