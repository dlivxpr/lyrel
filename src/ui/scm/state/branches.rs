//! Branch reads and inline branch-entry lifecycle owned by SCM.

use super::*;
use crate::ui::app::LyrelApp;
use crate::ui::host_ops::HostOps;
use gpui_kit::{Context, Focusable as _};
use lyrel_core::core::git::ops::GitOp;

#[derive(Clone, Copy)]
pub(crate) enum BranchInput {
    Create,
    Checkout,
}

struct BranchLoad {
    repo: RepoKey,
    epoch: u64,
    generation: u64,
}

impl ScmPanelState {
    pub(crate) fn branches(&self, repo: &RepoKey) -> &[String] {
        self.branches.get(repo).map_or(&[], |(_, names)| names)
    }

    /// Keep the cheap freshness check allocation-free when a cached list is
    /// current. Only a dispatch captures a repository/Host context.
    pub(crate) fn load_branches(&mut self, repo: &RepoKey, cx: &mut Context<LyrelApp>) {
        let epoch = repo.refresh_epoch(cx);
        if self.branches.get(repo).is_some_and(|(at, _)| *at == epoch)
            || self
                .branches_loading
                .get(repo)
                .is_some_and(|(at, _)| *at == epoch)
        {
            return;
        }
        let Some(context) = repo.context(cx) else {
            return;
        };
        let (repo, host, epoch) = context.into_parts();
        let load = self.begin_branch_load(repo, epoch);
        let root = load.repo.root.clone();
        HostOps::run(
            host,
            cx,
            move |host| {
                lyrel_core::core::git::git(
                    host,
                    &root,
                    &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
                )
            },
            move |this, out, cx| {
                if this.scm.apply_branches(load, out, cx) {
                    cx.notify();
                }
            },
        );
    }

    fn begin_branch_load(&mut self, repo: RepoKey, epoch: u64) -> BranchLoad {
        self.branch_generation = self.branch_generation.wrapping_add(1);
        self.branches_loading
            .insert(repo.clone(), (epoch, self.branch_generation));
        BranchLoad {
            repo,
            epoch,
            generation: self.branch_generation,
        }
    }

    fn apply_branches(&mut self, load: BranchLoad, out: Option<String>, cx: &App) -> bool {
        if self.branches_loading.get(&load.repo) != Some(&(load.epoch, load.generation)) {
            return false;
        }
        self.branches_loading.remove(&load.repo);
        if load.repo.refresh_epoch(cx) == load.epoch
            && HostRegistry::lookup(cx, load.repo.host).is_some()
        {
            // Failure is still an empty list at this epoch, not a retry loop
            // from render. Git-operation errors retain their existing notices.
            let names = out
                .unwrap_or_default()
                .lines()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect();
            self.branches.insert(load.repo, (load.epoch, names));
        }
        true
    }

    pub(crate) fn branch_input(&self, kind: BranchInput) -> Option<&Entity<InputState>> {
        match kind {
            BranchInput::Create => self.new_branch.as_ref(),
            BranchInput::Checkout => self.checkout_branch.as_ref(),
        }
    }

    pub(crate) fn begin_branch_input(
        &mut self,
        kind: BranchInput,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
    ) {
        let placeholder = match kind {
            BranchInput::Create => L10nKey::ScmCreateBranch,
            BranchInput::Checkout => L10nKey::ScmCheckoutBranch,
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t(placeholder)));
        let handle = input.read(cx).focus_handle(cx);
        self.new_branch = None;
        self.checkout_branch = None;
        match kind {
            BranchInput::Create => self.new_branch = Some(input),
            BranchInput::Checkout => self.checkout_branch = Some(input),
        }
        window.focus(&handle, cx);
        cx.notify();
    }

    pub(crate) fn cancel_branch_input(&mut self, kind: BranchInput) {
        self.take_branch_input(kind);
    }

    fn take_branch_input(&mut self, kind: BranchInput) -> Option<Entity<InputState>> {
        match kind {
            BranchInput::Create => self.new_branch.take(),
            BranchInput::Checkout => self.checkout_branch.take(),
        }
    }

    pub(crate) fn submit_branch_input(&mut self, kind: BranchInput, cx: &App) -> Option<GitOp> {
        let input = self.take_branch_input(kind)?;
        let name = input.read(cx).value().trim().to_string();
        if name.is_empty() {
            return None;
        }
        Some(match kind {
            BranchInput::Create => GitOp::CreateBranch {
                name,
                start: None,
                checkout: true,
            },
            BranchInput::Checkout => GitOp::CheckoutBranch { name },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn a_retired_host_read_cannot_release_or_replace_its_reconnected_read(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(|cx| {
            let repo = RepoKey {
                host: HostId::LOCAL,
                root: PathBuf::from("/branch-read"),
            };
            let mut scm = ScmPanelState::default();
            let old = scm.begin_branch_load(repo.clone(), 0);
            scm.forget_lost_hosts(&[]);
            let new = scm.begin_branch_load(repo.clone(), 0);
            assert!(!scm.apply_branches(old, Some("obsolete\n".into()), cx));
            assert!(scm.apply_branches(new, Some("main\n topic \n".into()), cx));
            assert_eq!(scm.branches(&repo), ["main", "topic"]);
            let failed = scm.begin_branch_load(repo.clone(), 0);
            assert!(scm.apply_branches(failed, None, cx));
            assert!(scm.branches(&repo).is_empty());
        });
    }

    #[gpui_kit::test]
    fn an_epoch_change_keeps_the_previous_list_until_a_current_read_lands(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(|cx| {
            use crate::terminal::git_data::ScmData;
            cx.set_global(ScmData::default());
            let repo = RepoKey {
                host: HostId::LOCAL,
                root: PathBuf::from("/branch-epoch"),
            };
            let mut scm = ScmPanelState::default();
            let initial = scm.begin_branch_load(repo.clone(), 0);
            scm.apply_branches(initial, Some("main\n".into()), cx);
            let old = scm.begin_branch_load(repo.clone(), 0);
            cx.global_mut::<ScmData>().bump(repo.host, &repo.root);
            scm.apply_branches(old, Some("obsolete\n".into()), cx);
            assert_eq!(scm.branches(&repo), ["main"]);
            let current = scm.begin_branch_load(repo.clone(), repo.refresh_epoch(cx));
            scm.apply_branches(current, Some("main\ntopic\n".into()), cx);
            assert_eq!(scm.branches(&repo), ["main", "topic"]);
        });
    }

    #[gpui_kit::test]
    fn reopening_a_repository_refreshes_branches_and_rejects_retired_results(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use crate::terminal::git_data::{ScmData, ScmWatcher};
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .current_dir(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "--quiet", "--initial-branch=main"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "initial",
        ]);
        let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
        let repo = RepoKey {
            host: HostId::LOCAL,
            root: dir.path().to_path_buf(),
        };
        for clear_host in [false, true] {
            let retired = app.update(&mut vcx, |app, cx| {
                cx.default_global::<ScmData>().subscriptions().declare(
                    8100,
                    ScmWatcher::Panel,
                    Some((repo.host, repo.root.clone())),
                );
                let epoch = repo.refresh_epoch(cx);
                let initial = app.scm.begin_branch_load(repo.clone(), epoch);
                app.scm.apply_branches(initial, Some("main\n".into()), cx);
                app.scm.begin_branch_load(repo.clone(), epoch)
            });
            vcx.update(|_, cx| {
                let data = cx.global_mut::<ScmData>();
                if clear_host {
                    data.clear_host(repo.host);
                } else {
                    data.release_window(8100);
                }
            });
            let branch = if clear_host {
                "reconnected"
            } else {
                "reopened"
            };
            git(&["branch", branch]);
            app.update(&mut vcx, |app, cx| {
                cx.global_mut::<ScmData>().subscriptions().declare(
                    8100,
                    ScmWatcher::Panel,
                    Some((repo.host, repo.root.clone())),
                );
                app.scm
                    .apply_branches(retired, Some("obsolete\n".into()), cx);
                assert_eq!(
                    app.scm.branches(&repo),
                    ["main"],
                    "the retired result cannot land"
                );
                // Exercise the actual cache freshness fast path and Host reader.
                app.scm.load_branches(&repo, cx);
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                app.update(&mut vcx, |app, cx| app.scm.load_branches(&repo, cx));
                vcx.run_until_parked();
                if app.read_with(&vcx, |app, _| {
                    app.scm.branches(&repo).iter().any(|name| name == branch)
                }) {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "reopened branch list stayed stale"
                );
                std::thread::yield_now();
            }
            app.read_with(&vcx, |app, _| {
                assert!(
                    !app.scm
                        .branches(&repo)
                        .iter()
                        .any(|name| name == "obsolete")
                )
            });
            vcx.update(|_, cx| cx.global_mut::<ScmData>().release_window(8100));
        }
    }
}
