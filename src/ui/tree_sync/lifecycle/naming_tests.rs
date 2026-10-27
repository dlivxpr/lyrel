use super::super::*;
use super::*;
/// A name typed into the create form is for a workspace the machine has
/// not been told to make yet. Sending it as a rename is sending it into a
/// `NotFound`, which `unsendable` swallows, and the create that follows
/// then names the workspace whatever it rolled — the user's name lost to a
/// generated one every time (#618).
#[gpui_kit::test]
fn a_typed_name_waits_for_the_create_rather_than_racing_it(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        let ws = WorkspaceId::new();
        cx.default_global::<TreeSync>()
            .windows
            .entry(ws)
            .or_default();

        name_new_workspace(cx, ws, "deploy".into());

        let parked = cx.default_global::<TreeSync>().windows[&ws]
            .chosen_name
            .clone()
            .expect("the name rides along with the create instead of chasing it");
        assert_eq!(parked.name, "deploy");
        assert_eq!(
            parked.workspace, ws,
            "and it is parked against the workspace it was typed for, so a window \
                 that walks into a different one cannot spend it (#716)"
        );
    });
}

/// `start_prime` spends the typed name on the create, so a machine that
/// answers with it has settled the matter.
#[gpui_kit::test]
fn the_machine_reading_back_the_typed_name_settles_it(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        let (ws, view) = primed_window(cx, Some("deploy"));
        let epoch = cx.default_global::<TreeSync>().windows[&ws].epoch;

        finish_prime(
            cx,
            ws,
            epoch,
            Ok((WsMirror::default(), Some("deploy".into()), Arrival::Created)),
        );

        assert!(
            cx.default_global::<TreeSync>().windows[&ws]
                .chosen_name
                .is_none(),
            "a name the machine has confirmed is not still owed"
        );
        assert_eq!(
            crate::ui::machine_mirror::display_name(cx, &view).as_deref(),
            Some("deploy"),
            "and it is what the window shows"
        );
    });
}

/// A create ran and came back under a different name — this window's create
/// lost the race to the other pull's, which had rolled a codename before
/// the user finished typing. The typed name is still owed and still has to
/// beat what the pull answered with, which is #618 and the chip #604 wired.
///
/// Reshaped from `a_workspace_the_machine_already_had_still_takes_the_typed_name`,
/// which asserted this override for *every* answer, adopted workspaces
/// included. The override is right; the blanket was not (#716) — see the
/// test below for the half that was wrong.
#[gpui_kit::test]
fn a_create_that_answered_with_another_name_still_takes_the_typed_one(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        let (ws, view) = primed_window(cx, Some("deploy"));
        let epoch = cx.default_global::<TreeSync>().windows[&ws].epoch;

        finish_prime(
            cx,
            ws,
            epoch,
            Ok((
                WsMirror::default(),
                Some("keen-marten".into()),
                Arrival::Created,
            )),
        );

        assert!(
            cx.default_global::<TreeSync>().windows[&ws]
                .chosen_name
                .is_none(),
            "said once, not on every later pull"
        );
        assert_eq!(
            crate::ui::machine_mirror::display_name(cx, &view).as_deref(),
            Some("deploy"),
            "the name the user typed outranks the one the create answered with"
        );
    });
}

/// The other half of that split, and #716's second failure. A client
/// opening a workspace that was already on the machine renamed it to
/// whatever that client had parked — a workspace holding nineteen panes
/// came back named after the arriving user. Nothing was created here, so
/// nothing is owed: the workspace keeps the name it has.
#[gpui_kit::test]
fn a_workspace_the_window_walked_into_keeps_its_own_name(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        let (ws, view) = primed_window(cx, Some("sher1"));
        let epoch = cx.default_global::<TreeSync>().windows[&ws].epoch;

        finish_prime(
            cx,
            ws,
            epoch,
            Ok((
                WsMirror::default(),
                Some("keen-marten".into()),
                Arrival::Adopted,
            )),
        );

        assert_eq!(
            crate::ui::machine_mirror::display_name(cx, &view).as_deref(),
            Some("keen-marten"),
            "the machine's name stands; an arriving client does not rename it"
        );
    });
}

/// The identity check, independently of how the pull arrived. A name typed
/// for one workspace must not be spendable on another even when a create
/// did run: the pairing is what makes the name mean anything.
#[gpui_kit::test]
fn a_name_typed_for_another_workspace_is_never_spent_here(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        let (ws, view) = primed_window(cx, Some("deploy"));
        cx.default_global::<TreeSync>()
            .windows
            .get_mut(&ws)
            .expect("primed just above")
            .chosen_name = Some(ChosenName {
            workspace: WorkspaceId::new(),
            name: "deploy".into(),
        });
        let epoch = cx.default_global::<TreeSync>().windows[&ws].epoch;

        finish_prime(
            cx,
            ws,
            epoch,
            Ok((
                WsMirror::default(),
                Some("keen-marten".into()),
                Arrival::Created,
            )),
        );

        assert_eq!(
            crate::ui::machine_mirror::display_name(cx, &view).as_deref(),
            Some("keen-marten"),
            "a name owed to another workspace is not this one's to take"
        );
    });
}

/// The create that actually runs when a window switches workspaces is
/// `pull_workspace`'s, not `start_prime`'s — `hydrate` is what
/// `switch_workspace` orders, and it rolled its own codename with no idea a
/// name had been typed. Whatever the tree comes back saying, the typed name
/// is what the workspace ends up called.
#[gpui_kit::test]
fn a_hydrating_pull_settles_the_typed_name_too(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        crate::ui::windows::WindowRegistry::init(cx);
        let (ws, view) = primed_window(cx, Some("deploy"));
        let epoch = cx.default_global::<TreeSync>().windows[&ws].epoch;
        let pulled = Machine {
            workspaces: vec![lyrel_core::core::machine::Workspace {
                id: ws,
                name: Some("keen-marten".into()),
                ..Default::default()
            }],
            panes: Vec::new(),
        };

        settle_hydration(
            cx,
            ws,
            epoch,
            Adopt::IfEmpty,
            Ok((
                pulled,
                WsMirror::default(),
                Session::default(),
                Arrival::Created,
            )),
        );

        assert_eq!(
            crate::ui::machine_mirror::display_name(cx, &view).as_deref(),
            Some("deploy"),
            "the name the user typed, not the codename the pull rolled"
        );
        assert!(
            cx.default_global::<TreeSync>().windows[&ws]
                .chosen_name
                .is_none(),
            "and it is owed only once"
        );
    });
}

/// The same window arriving at a workspace it did not make, which is the
/// path #716 came in through: `switch_workspace` orders the hydration, the
/// pull finds the workspace already there, and the name parked on the
/// window used to land on it as a rename.
#[gpui_kit::test]
fn a_hydration_that_adopted_leaves_the_name_it_found(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        crate::ui::windows::WindowRegistry::init(cx);
        let (ws, view) = primed_window(cx, Some("sher1"));
        let epoch = cx.default_global::<TreeSync>().windows[&ws].epoch;
        let pulled = Machine {
            workspaces: vec![lyrel_core::core::machine::Workspace {
                id: ws,
                name: Some("keen-marten".into()),
                ..Default::default()
            }],
            panes: Vec::new(),
        };

        settle_hydration(
            cx,
            ws,
            epoch,
            Adopt::IfEmpty,
            Ok((
                pulled,
                WsMirror::default(),
                Session::default(),
                Arrival::Adopted,
            )),
        );

        assert_eq!(
            crate::ui::machine_mirror::display_name(cx, &view).as_deref(),
            Some("keen-marten"),
            "walking in is not naming"
        );
    });
}

/// A window with no name owed reads whatever the machine says, which is
/// the whole of #604 and must survive the arbitration above.
#[gpui_kit::test]
fn a_window_owing_no_name_still_reads_the_machines(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        let (ws, view) = primed_window(cx, None);
        let epoch = cx.default_global::<TreeSync>().windows[&ws].epoch;

        finish_prime(
            cx,
            ws,
            epoch,
            Ok((
                WsMirror::default(),
                Some("keen-marten".into()),
                Arrival::Created,
            )),
        );

        assert_eq!(
            crate::ui::machine_mirror::display_name(cx, &view).as_deref(),
            Some("keen-marten")
        );
    });
}

/// A window mid-prime with `chosen` parked on it, and a machine this client
/// has pulled, so the mirror has somewhere to write the name.
fn primed_window(
    cx: &mut App,
    chosen: Option<&str>,
) -> (WorkspaceId, crate::core::session::WindowView) {
    lyrel_core::core::config::set_config_dir(
        std::env::temp_dir().join(format!("lyrel-chosen-name-{}", std::process::id())),
    );
    let view = crate::core::session::WindowView::default();
    let ws = view.id;
    WorkspaceStore::install_for_test(
        cx,
        crate::core::session::WindowViews {
            views: vec![view.clone()],
            active: Some(ws),
        },
    );
    crate::ui::machine_mirror::MachineMirrors::install(cx, HostId::LOCAL, Machine::default());
    let state = cx
        .default_global::<TreeSync>()
        .windows
        .entry(ws)
        .or_default();
    state.sync = SyncPhase::Unprimed {
        dirty: false,
        priming: true,
    };
    state.chosen_name = chosen.map(|name| ChosenName {
        workspace: ws,
        name: name.to_string(),
    });
    (ws, view)
}
#[gpui_kit::test]
fn a_superseded_prime_result_does_not_roll_the_mirror_back(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        let ws = WorkspaceId::new();
        let stale_epoch = {
            let state = cx
                .default_global::<TreeSync>()
                .windows
                .entry(ws)
                .or_default();
            state.sync = SyncPhase::Unprimed {
                dirty: false,
                priming: true,
            };
            state.epoch
        };
        let advanced = WsMirror {
            tabs: vec![TreeTab::leaf(7)],
            ..Default::default()
        };
        {
            let state = cx
                .default_global::<TreeSync>()
                .windows
                .get_mut(&ws)
                .unwrap();
            state.epoch += 1;
            state.sync = SyncPhase::Primed(advanced.clone());
        }

        finish_prime(
            cx,
            ws,
            stale_epoch,
            Ok((WsMirror::default(), None, Arrival::Created)),
        );

        match &cx.default_global::<TreeSync>().windows[&ws].sync {
            SyncPhase::Primed(mirror) => assert_eq!(
                *mirror, advanced,
                "the stale pull's empty answer must not replace the advanced mirror"
            ),
            _ => panic!("the mirror was dropped entirely"),
        }
    });
}
/// #604: the answer to the create this window asked for is the only place
/// it will ever hear the workspace's name — a client is left out of the
/// deltas its own ops raise — so priming has to hand that name to the
/// mirror. Dropping it left the window showing an unnamed workspace, and
/// the name it had had all along arrived with the next full pull looking
/// like a rename.
#[gpui_kit::test]
fn priming_teaches_the_mirror_the_name_the_machine_gave_the_workspace(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        lyrel_core::core::config::set_config_dir(
            std::env::temp_dir().join(format!("lyrel-prime-name-{}", std::process::id())),
        );
        let view = crate::core::session::WindowView::default();
        let ws = view.id;
        WorkspaceStore::install_for_test(
            cx,
            crate::core::session::WindowViews {
                views: vec![view.clone()],
                active: Some(ws),
            },
        );
        // A machine this client has pulled, from before the workspace it is
        // about to create existed on it.
        crate::ui::machine_mirror::MachineMirrors::install(cx, HostId::LOCAL, Machine::default());
        cx.default_global::<TreeSync>()
            .windows
            .entry(ws)
            .or_default()
            .sync = SyncPhase::Unprimed {
            dirty: false,
            priming: true,
        };
        let epoch = cx.default_global::<TreeSync>().windows[&ws].epoch;

        finish_prime(
            cx,
            ws,
            epoch,
            Ok((
                WsMirror::default(),
                Some("keen-marten".to_string()),
                Arrival::Created,
            )),
        );

        assert_eq!(
            crate::ui::machine_mirror::display_name(cx, &view).as_deref(),
            Some("keen-marten"),
            "the window knows what its workspace is called as soon as it exists"
        );
    });
}
