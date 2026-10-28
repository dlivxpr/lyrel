use super::*;
use crate::terminal::git_data::ScmData;
use crate::ui::app::test_window::harness;
use gpui_kit::TestAppContext;

fn repo(root: &str) -> RepoKey {
    RepoKey { host: HostId::LOCAL, root: PathBuf::from(root) }
}

fn empty_page() -> CommitPage {
    CommitPage {
        commits: Vec::new(), rows: Vec::new(), max_lanes: 0,
        complete: true, requested: GRAPH_PAGE,
        scope: GraphScope::HeadAndUpstream, truncated_lanes: false, open_lanes: Vec::new(),
    }
}

#[gpui_kit::test]
fn switching_scope_resets_the_page_and_its_size(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (app, mut vcx) = harness(cx);
    app.update(&mut vcx, |app, cx| {
        let repo = repo("/scope");
        app.scm.follow_pane(repo.clone());
        let load = app.scm.begin_graph_load(&repo, repo.refresh_epoch(cx)).unwrap();
        assert!(app.scm.apply_graph(load, Some(empty_page()), cx));
        app.scm.request_more_history();
        app.scm.request_more_history();
        app.scm.set_graph_scope(GraphScope::All, cx);
        assert_eq!(app.scm.graph().scope, GraphScope::All);
        assert_eq!(app.scm.graph().requested, GRAPH_PAGE);
        assert!(app.scm.graph().page.is_none());
        let load = app.scm.begin_graph_load(&repo, repo.refresh_epoch(cx)).unwrap();
        assert!(app.scm.apply_graph(load, Some(empty_page()), cx));
        app.scm.set_graph_scope(GraphScope::All, cx);
        assert!(app.scm.graph().page.is_some(), "the same scope retains its page");
    });
}

#[gpui_kit::test]
fn switching_repositories_drops_the_previous_page(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let mut scm = ScmPanelState::default();
        let first = repo("/first");
        let second = repo("/second");
        scm.follow_pane(first.clone());
        let load = scm.begin_graph_load(&first, 0).unwrap();
        scm.apply_graph(load, Some(empty_page()), cx);
        scm.request_more_history();
        scm.request_more_history();
        scm.follow_pane(second.clone());
        let load = scm.begin_graph_load(&second, 0).unwrap();
        assert!(scm.graph().page.is_none());
        assert_eq!(scm.graph().requested, 0, "depth does not carry between repositories");
        scm.apply_graph(load, Some(empty_page()), cx);
        assert!(scm.begin_graph_load(&second, 0).is_none());
        assert!(scm.graph().page.is_some(), "the same repository keeps its fresh page");
    });
}

#[gpui_kit::test]
fn overlapping_refreshes_cannot_replace_or_release_the_newer_graph(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(ScmData::default());
        let mut scm = ScmPanelState::default();
        let repo = repo("/overlap");
        scm.follow_pane(repo.clone());
        let old = scm.begin_graph_load(&repo, 0).unwrap();
        cx.global_mut::<ScmData>().bump(repo.host, &repo.root);
        let new = scm.begin_graph_load(&repo, repo.refresh_epoch(cx)).unwrap();
        assert!(!scm.apply_graph(old, Some(empty_page()), cx));
        assert!(scm.graph().loading, "the older callback did not release the newer load");
        assert!(scm.graph().page.is_none());
        assert!(scm.apply_graph(new, Some(empty_page()), cx));
        assert_eq!(scm.graph().page_key.as_ref().unwrap().1, 1);
        assert!(!scm.graph().loading);
    });
}

#[gpui_kit::test]
fn rapid_repository_switching_rejects_even_a_return_to_the_original_repo(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let mut scm = ScmPanelState::default();
        let first = repo("/first");
        let second = repo("/second");
        scm.follow_pane(first.clone());
        let old = scm.begin_graph_load(&first, 0).unwrap();
        scm.follow_pane(second.clone());
        let middle = scm.begin_graph_load(&second, 0).unwrap();
        scm.follow_pane(first.clone());
        let latest = scm.begin_graph_load(&first, 0).unwrap();
        assert!(!scm.apply_graph(old, Some(empty_page()), cx));
        assert!(!scm.apply_graph(middle, None, cx));
        assert!(scm.graph().loading);
        assert!(scm.apply_graph(latest, Some(empty_page()), cx));
        assert_eq!(scm.graph().page_key.as_ref().unwrap().0, first);
    });
}

#[gpui_kit::test]
fn failed_loading_waits_for_a_new_refresh_identity(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(ScmData::default());
        let mut scm = ScmPanelState::default();
        let repo = repo("/failure");
        scm.follow_pane(repo.clone());
        let load = scm.begin_graph_load(&repo, 0).unwrap();
        assert!(scm.apply_graph(load, None, cx));
        assert!(!scm.graph().loading);
        assert!(scm.begin_graph_load(&repo, 0).is_none(), "a failed read is not a render retry loop");
        cx.global_mut::<ScmData>().bump(repo.host, &repo.root);
        assert!(scm.begin_graph_load(&repo, repo.refresh_epoch(cx)).is_some());
    });
}

#[gpui_kit::test]
fn a_scope_change_rejects_the_old_query_even_when_it_finishes_last(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (app, mut vcx) = harness(cx);
    app.update(&mut vcx, |app, cx| {
        let repo = repo("/query");
        app.scm.follow_pane(repo.clone());
        let old = app.scm.begin_graph_load(&repo, repo.refresh_epoch(cx)).unwrap();
        app.scm.set_graph_scope(GraphScope::All, cx);
        let new = app.scm.begin_graph_load(&repo, repo.refresh_epoch(cx)).unwrap();
        assert!(app.scm.apply_graph(new, Some(empty_page()), cx));
        assert!(!app.scm.apply_graph(old, None, cx));
        assert_eq!(app.scm.graph().page_key.as_ref().unwrap().2, GraphScope::All);
        assert!(app.scm.graph().failed_key.is_none());
    });
}

#[gpui_kit::test]
fn closing_and_reopening_the_same_commit_rejects_the_retired_detail(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (app, mut vcx) = harness(cx);
    app.update(&mut vcx, |app, cx| {
        let repo = repo("/detail");
        app.scm.follow_pane(repo.clone());
        app.scm.open_detail(repo.clone(), "deadbeef".into(), None, cx);
        let (old, _) = app.scm.begin_detail_load(cx).unwrap();
        app.scm.close_detail(cx);
        app.scm.open_detail(repo, "deadbeef".into(), None, cx);
        let (new, _) = app.scm.begin_detail_load(cx).unwrap();
        assert!(!app.scm.apply_detail(old, None, Some(Vec::new()), cx));
        assert!(app.scm.detail().unwrap().loading);
        assert!(app.scm.apply_detail(new, None, None, cx));
        let detail = app.scm.detail().unwrap();
        assert!(detail.loaded);
        assert!(detail.files_failed, "failed files must not be presented as zero files changed");
        assert!(app.scm.begin_detail_load(cx).is_none(), "missing commit/files is a terminal read result");
    });
}

#[gpui_kit::test]
fn a_detail_refresh_cannot_land_after_its_repository_epoch_changes(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (app, mut vcx) = harness(cx);
    app.update(&mut vcx, |app, cx| {
        let repo = repo("/detail-epoch");
        app.scm.follow_pane(repo.clone());
        app.scm.open_detail(repo.clone(), "deadbeef".into(), None, cx);
        let (old, _) = app.scm.begin_detail_load(cx).unwrap();
        cx.default_global::<ScmData>().bump(repo.host, &repo.root);
        assert!(app.scm.apply_detail(old, None, Some(Vec::new()), cx));
        assert!(!app.scm.detail().unwrap().loaded);
        assert!(app.scm.begin_detail_load(cx).is_some(), "a new epoch may retry the unsettled detail");
    });
}

#[test]
fn load_more_growth_stops_at_the_cap() {
    assert_eq!(next_page_request(0), GRAPH_PAGE * 2);
    assert_eq!(next_page_request(GRAPH_PAGE), GRAPH_PAGE * 2);
    assert_eq!(next_page_request(MAX_GRAPH_COMMITS - 1), MAX_GRAPH_COMMITS);
    assert_eq!(next_page_request(MAX_GRAPH_COMMITS), MAX_GRAPH_COMMITS);
}

#[test]
fn the_filter_matches_what_a_row_can_show() {
    use lyrel_core::core::git::log::{OffsetTs, Signature};
    let who = Signature { name: "Ada Lovelace".into(), email: "a@b.invalid".into(), at: OffsetTs { unix: 1, offset_minutes: 0 } };
    let commit = Commit {
        oid: "c0ffee1234".into(), parents: Default::default(), summary: "feat(ui): the graph".into(),
        body: "rationale".into(), author: who.clone(), committer: who, refs: Vec::new(),
    };
    for hit in ["graph", "GRAPH", "feat", "ada", "Lovelace", "c0ffee"] {
        assert!(matches_query(&commit, &hit.to_lowercase()), "{hit:?} should match");
    }
    assert!(!matches_query(&commit, "rationale"));
    assert!(!matches_query(&commit, "ffee"));
}
