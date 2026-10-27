use super::*;
use super::super::*;
    #[gpui_kit::test]
    fn a_hydration_that_died_on_a_stale_link_is_owed_back(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            let ws = WorkspaceId::new();
            let epoch = {
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
            owe_rehydration(cx, ws, epoch, Adopt::Replace);
            let state = &cx.default_global::<TreeSync>().windows[&ws];
            assert!(
                matches!(state.sync, SyncPhase::Unprimed { priming: false, .. }),
                "the attempt is over; another one must be able to start"
            );
            assert!(
                state.rehydrate.is_some(),
                "dropping the failure here is what left the window on the home page"
            );

            // A newer attempt has already taken over — the loser must not
            // re-arm a retry behind its back.
            {
                let state = cx
                    .default_global::<TreeSync>()
                    .windows
                    .get_mut(&ws)
                    .unwrap();
                state.rehydrate = None;
                state.epoch += 1;
            }
            owe_rehydration(cx, ws, epoch, Adopt::Replace);
            assert!(
                cx.default_global::<TreeSync>().windows[&ws]
                    .rehydrate
                    .is_none()
            );
        });
    }

    /// The debt an owed pull records is worth nothing without something that
    /// pays it. A pull can fail with the link up and healthy — a `MachineGet`
    /// past its deadline on a slow link, a create that lost its race — and
    /// then no reconnect ever happens to notice, and an empty window has no
    /// edit in it to drive a sync. The window sat there empty, with every tab
    /// still on the machine, until the app was restarted.
    #[gpui_kit::test]
    fn an_owed_pull_is_retried_until_it_is_paid_or_superseded(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            let ws = WorkspaceId::new();
            let epoch = cx
                .default_global::<TreeSync>()
                .windows
                .entry(ws)
                .or_default()
                .epoch;
            owe_rehydration(cx, ws, epoch, Adopt::IfEmpty);
            assert!(
                still_owed(cx, ws, epoch),
                "the retry armed for this debt must still recognise it"
            );

            // Paid: the pull landed, so the retry that is still in flight has
            // to stand down rather than replay the machine over the window.
            cx.default_global::<TreeSync>()
                .windows
                .get_mut(&ws)
                .expect("owed above")
                .rehydrate = None;
            assert!(!still_owed(cx, ws, epoch));

            // Superseded: a newer hydration owns the window now.
            let state = cx
                .default_global::<TreeSync>()
                .windows
                .get_mut(&ws)
                .expect("owed above");
            state.rehydrate = Some(Adopt::IfEmpty);
            state.epoch += 1;
            assert!(!still_owed(cx, ws, epoch));
            assert!(still_owed(cx, ws, epoch + 1));
        });
    }

    #[test]
    fn the_retry_backs_off_and_settles_at_a_cap() {
        let secs = |n| rehydrate_backoff(n).as_secs();
        assert_eq!(secs(1), 2, "the first retry is prompt: a window is empty");
        assert!(
            secs(1) < secs(2) && secs(2) < secs(3),
            "a machine that keeps refusing must be asked less often, not more"
        );
        assert_eq!(secs(REHYDRATE_SETTLED), 30);
        assert_eq!(
            secs(50),
            30,
            "a window left open on an unreachable machine settles at the cap"
        );
    }

    /// Once the backoff stops growing the same failure repeats every thirty
    /// seconds for as long as the window stays open. Reporting each one at full
    /// volume turns one unreachable machine into a log nobody can read past.
    #[test]
    fn a_standing_failure_stops_shouting_once_the_backoff_settles() {
        assert_eq!(
            hydration_log_level(1, log::Level::Warn),
            log::Level::Warn,
            "the first failures are news and must stay news"
        );
        assert_eq!(
            hydration_log_level(REHYDRATE_SETTLED, log::Level::Warn),
            log::Level::Debug
        );
        assert_eq!(
            hydration_log_level(REHYDRATE_SETTLED, log::Level::Info),
            log::Level::Debug,
            "the step down is to debug from wherever it started, not to warn"
        );
    }
    /// The count paces the retry, so it has to mean "failures in a row". Left
    /// standing after the run ends, it makes the next *first* failure wait the
    /// cap on an outage that was already over.
    #[gpui_kit::test]
    fn the_backoff_count_ends_with_the_run_of_failures(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            let _ = lyrel_core::core::config::set_config_dir(
                std::env::temp_dir().join(format!("lyrel-backoff-count-{}", std::process::id())),
            );
            let view = crate::core::session::WindowView::default();
            let ws = view.id;
            WorkspaceStore::install_for_test(
                cx,
                crate::core::session::WindowViews {
                    views: vec![view],
                    active: Some(ws),
                },
            );
            let unprimed = |cx: &mut App| {
                cx.default_global::<TreeSync>()
                    .windows
                    .entry(ws)
                    .or_default()
                    .sync = SyncPhase::Unprimed {
                    dirty: false,
                    priming: true,
                };
            };
            let attempts =
                |cx: &mut App| cx.default_global::<TreeSync>().windows[&ws].rehydrate_attempts;

            unprimed(cx);
            let epoch = cx.default_global::<TreeSync>().windows[&ws].epoch;
            for expected in 1..=3 {
                unprimed(cx);
                owe_rehydration(cx, ws, epoch, Adopt::IfEmpty);
                assert_eq!(
                    attempts(cx),
                    expected,
                    "each failure in the run paces the next"
                );
            }

            // The machine answered. Whatever it was, it is over.
            unprimed(cx);
            finish_prime(
                cx,
                ws,
                epoch,
                Ok((WsMirror::default(), None, Arrival::Created)),
            );
            assert_eq!(
                attempts(cx),
                0,
                "a prime landing is the machine answering, which is the whole question"
            );

            // Abandoned rather than paid: the user filled the window in
            // themselves, so the `Replace` is dropped — and the run is over too.
            {
                let state = cx
                    .default_global::<TreeSync>()
                    .windows
                    .get_mut(&ws)
                    .unwrap();
                state.rehydrate = Some(Adopt::Replace);
                state.owed_over = Vec::new();
                state.rehydrate_attempts = 4;
            }
            assert!(take_rehydrate(cx, ws, &[TabId::new()]).is_none());
            assert_eq!(
                attempts(cx),
                0,
                "a debt nobody owes any more cannot go on pacing the next one"
            );
        });
    }

    /// The retry fires on a timer, so the window it was armed for can be gone
    /// by the time it runs. It has to notice and stand down — and leave the
    /// debt where it is, because a window that is not there is not one that
    /// has been paid.
    #[gpui_kit::test]
    async fn a_retry_that_finds_no_window_stands_down(cx: &mut gpui_kit::TestAppContext) {
        let ws = cx.update(|cx| {
            crate::ui::windows::WindowRegistry::init(cx);
            let ws = WorkspaceId::new();
            let epoch = cx
                .default_global::<TreeSync>()
                .windows
                .entry(ws)
                .or_default()
                .epoch;
            owe_rehydration(cx, ws, epoch, Adopt::IfEmpty);
            ws
        });

        // Well past the first backoff: the armed retry really runs, rather than
        // the test ending while it is still asleep.
        cx.executor().advance_clock(rehydrate_backoff(1) * 2);
        cx.executor().run_until_parked();

        cx.update(|cx| {
            assert!(
                cx.default_global::<TreeSync>().windows[&ws]
                    .rehydrate
                    .is_some(),
                "the debt outlives a retry that found nothing to pay it into"
            );
        });
    }

    #[gpui_kit::test]
    fn a_window_that_filled_up_while_owed_keeps_what_it_has(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            let ws = WorkspaceId::new();
            let stale = [TabId::new(), TabId::new()];
            let arm = |cx: &mut App, adopt, over: &[TabId]| {
                let state = cx
                    .default_global::<TreeSync>()
                    .windows
                    .entry(ws)
                    .or_default();
                state.rehydrate = Some(adopt);
                state.owed_over = over.to_vec();
            };

            arm(cx, Adopt::Replace, &[]);
            assert!(
                take_rehydrate(cx, ws, &[]).is_some(),
                "an empty window is exactly the one that still needs its layout"
            );
            assert!(
                take_rehydrate(cx, ws, &[]).is_none(),
                "the debt is claimed once"
            );

            // #579: the tabs a `Replace` was ordered over are the ones it exists
            // to replace. Reading them as "the user moved on" abandoned the
            // retry on its first attempt for every resync of a window that kept
            // its tabs — a daemon back as a new process, a remote server
            // restarted — and the resync silently did nothing at all.
            arm(cx, Adopt::Replace, &stale);
            assert!(
                take_rehydrate(cx, ws, &stale).is_some(),
                "the dead tabs the resync was ordered over cannot be the reason to drop it"
            );

            arm(cx, Adopt::Replace, &stale);
            assert!(
                take_rehydrate(cx, ws, &stale[..1]).is_some(),
                "closing some of them is not filling the window in either"
            );

            arm(cx, Adopt::Replace, &stale);
            assert!(
                take_rehydrate(cx, ws, &[stale[0], TabId::new()]).is_none(),
                "replaying an older layout over the user's new tabs is worse than not retrying"
            );
            assert!(
                cx.default_global::<TreeSync>().windows[&ws]
                    .rehydrate
                    .is_none(),
                "and the dropped retry must not linger"
            );

            arm(cx, Adopt::IfEmpty, &[]);
            assert!(
                take_rehydrate(cx, ws, &[TabId::new()]).is_some(),
                "IfEmpty polices that itself, and still owes the mirror a pull"
            );
        });
    }
