//! Quick launch for the coding agents this machine can run (#955).
//!
//! Launching an agent never types into a pane that already exists. It opens a
//! new shell — a tab in the active tab's directory, or a split when the entry
//! point was asked for one — and types the agent's command line into *that*
//! shell once it is there. The agent then runs the way a hand-typed one does:
//! the daemon detects it from the pane's foreground, status and resume work as
//! usual, and quitting it leaves the shell behind.
//!
//! Which agents are offered: on this computer, those whose launch program is
//! on `PATH`. A remote workspace's `PATH` is not something the `Host` trait can
//! answer — it has no way to run a command or read the far shell's
//! environment — so there the list is the agents seen running in that
//! workspace before (`WindowView::seen_agents`). Either way the order is
//! frecency, recorded the way saved SSH hosts' is.

use crate::ui::notification_history::NotificationWindowExt as _;
use std::collections::HashMap;
use std::sync::LazyLock;

use gpui_kit::{App, Axis, Context, Window};

use crate::core::cli_agent::{CLIAgent, launch_program, program_on_path};
use crate::core::config::{Config, ProfileUsage, unix_now};
use crate::core::session::WorkspaceStore;
use crate::ui::app::{LyrelApp, SpawnWhere};
use crate::ui::i18n::{L10nKey, t_fmt};
use crate::ui::pane::PaneSlot;
use crate::ui::settings::arguments::join_shell_args;
use crate::ui::tab::presentation::SpawnAs;

/// How the keymap spells "launch this agent": `LaunchAgent:omp`.
const LAUNCH_ACTION_PREFIX: &str = "LaunchAgent:";

/// One keymap action name per agent, in [`CLIAgent::ALL`] order.
pub(crate) fn launch_action_names() -> &'static [(CLIAgent, String)] {
    static NAMES: LazyLock<Vec<(CLIAgent, String)>> = LazyLock::new(|| {
        CLIAgent::ALL
            .into_iter()
            .map(|agent| (agent, format!("{LAUNCH_ACTION_PREFIX}{}", agent.slug())))
            .collect()
    });
    &NAMES
}

pub(crate) fn launch_action_name(agent: CLIAgent) -> &'static str {
    launch_action_names()
        .iter()
        .find(|(a, _)| *a == agent)
        .map(|(_, name)| name.as_str())
        .expect("every agent has a launch action")
}

pub(crate) fn agent_for_launch_action(action: &str) -> Option<CLIAgent> {
    action
        .strip_prefix(LAUNCH_ACTION_PREFIX)
        .and_then(CLIAgent::from_slug)
}

/// The agents whose launch program is on `path`, in [`CLIAgent::ALL`] order.
/// An agent with an `agent_launch` override is looked up by the override's
/// program, since that is what the launch will run.
pub(crate) fn installed_on(
    path: &std::ffi::OsStr,
    overrides: &HashMap<String, String>,
) -> Vec<CLIAgent> {
    CLIAgent::ALL
        .into_iter()
        .filter(|agent| {
            launch_program(&agent.launch_command(overrides))
                .is_some_and(|program| program_on_path(&program, path))
        })
        .collect()
}

/// `agents` most-used-first by frecency score; agents with the same score
/// keep the order they came in.
pub(crate) fn by_frecency(
    agents: impl IntoIterator<Item = CLIAgent>,
    usage: &HashMap<String, ProfileUsage>,
    now: u64,
) -> Vec<CLIAgent> {
    let score = |agent: &CLIAgent| usage.get(agent.slug()).map(|u| u.score(now)).unwrap_or(0.0);
    let mut agents: Vec<CLIAgent> = agents.into_iter().collect();
    agents.sort_by(|a, b| {
        score(b)
            .partial_cmp(&score(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    agents
}

/// The agent "New Agent Tab" opens: of `offered`, the one launched or seen
/// most recently; the first offered when none has been used yet.
pub(crate) fn most_recent(
    offered: &[CLIAgent],
    usage: &HashMap<String, ProfileUsage>,
) -> Option<CLIAgent> {
    let last_used = |agent: &CLIAgent| usage.get(agent.slug()).map_or(0, |u| u.last_used);
    offered
        .iter()
        .copied()
        .filter(|agent| last_used(agent) > 0)
        // `max_by_key` keeps the last of equals; reversed, that is the first.
        .rev()
        .max_by_key(last_used)
        .or_else(|| offered.first().copied())
}

/// The `agent_launch` line "Set Current Launch Args as Default" writes for a
/// pane that `agent` is running in with `argv`.
pub(crate) fn launch_line_to_save(agent: CLIAgent, argv: &[String]) -> Option<String> {
    agent
        .launch_argv_for_default(argv)
        .map(|argv| join_shell_args(&argv))
}

/// Make `line` the command `agent` is launched with from now on.
pub(crate) fn remember_launch_line(cfg: &mut Config, agent: CLIAgent, line: String) {
    // One entry per agent, whatever case an existing key was written in.
    cfg.agent_launch
        .retain(|slug, _| CLIAgent::from_slug(slug) != Some(agent));
    cfg.agent_launch.insert(agent.slug().to_string(), line);
}

/// How long a new shell is given to reach its first prompt before a queued
/// command is typed anyway.
const PROMPT_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
const PROMPT_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// Type `command` into the shell `slot` holds — now if it is up, or the
/// moment it lands if it is still connecting.
///
/// A shell that is up is not yet at its prompt: typed straight in, the line
/// was echoed by the tty above everything the shell prints while it starts (a
/// banner, `fastfetch`) and read only afterwards. It waits for the shell's
/// first prompt instead — up to [`PROMPT_WAIT`], for a shell with no
/// integration to say when that is.
pub(crate) fn run_when_ready(slot: &PaneSlot, command: String, cx: &mut App) {
    match slot {
        PaneSlot::Ready(view) => {
            let view = view.downgrade();
            cx.spawn(async move |cx| {
                let deadline = std::time::Instant::now() + PROMPT_WAIT;
                loop {
                    let ready = view
                        .read_with(cx, |view, _| view.terminal.at_prompt())
                        .unwrap_or(true);
                    if ready || std::time::Instant::now() >= deadline {
                        break;
                    }
                    cx.background_executor().timer(PROMPT_POLL).await;
                }
                let Ok((host, shell)) = view.read_with(cx, |view, cx| {
                    let shell = view.shell_spec().map(|spec| spec.program).or_else(|| {
                        // Only local panes inherit the desktop's shell setting.
                        (view.host_id().is_local() && view.ssh_spec().is_none())
                            .then(|| {
                                cx.global::<Config>()
                                    .shell
                                    .as_ref()
                                    .map(|shell| shell.program.clone())
                            })
                            .flatten()
                    });
                    (view.host(cx), shell)
                }) else {
                    return;
                };
                let _ = view.update_in(cx, |_view, window, cx| {
                    let Some(host) = host else {
                        let error = "the launch host is disconnected".to_string();
                        window.notify_app(
                            crate::ui::notification_history::Notification::error(t_fmt(
                                L10nKey::AppAgentLaunchRejected,
                                &[("error", &error)],
                            ))
                            .autohide(false),
                            cx,
                        );
                        return;
                    };
                    crate::ui::host_ops::HostOps::run_in(
                        host,
                        window,
                        cx,
                        move |host| {
                            lyrel_core::host::session_operations::scoped_launch_command(
                                host,
                                &command,
                                shell.as_deref(),
                            )
                            .map_err(|error| error.to_string())
                        },
                        |view, result, window, cx| match result {
                            Ok(command) => view.run_command_line(&command),
                            Err(error) => {
                                log::error!("agent launch rejected: {error}");
                                window.notify_app(
                                    crate::ui::notification_history::Notification::error(t_fmt(
                                        L10nKey::AppAgentLaunchRejected,
                                        &[("error", &error)],
                                    ))
                                    .autohide(false),
                                    cx,
                                );
                            }
                        },
                    );
                });
            })
            .detach();
        }
        PaneSlot::Connecting(pending) => {
            pending.update(cx, |pending, _| pending.run_when_ready(command));
        }
    }
}

impl LyrelApp {
    /// The agents this window's quick launch offers, most likely first.
    pub(crate) fn offered_agents(&self, cx: &App) -> Vec<CLIAgent> {
        let cfg = cx.global::<Config>();
        let candidates = match WorkspaceStore::all(cx)
            .get(self.workspace)
            .filter(|view| view.is_remote())
        {
            Some(view) => view
                .seen_agents
                .iter()
                .filter_map(|slug| CLIAgent::from_slug(slug))
                .collect(),
            None => installed_on(
                &std::env::var_os("PATH").unwrap_or_default(),
                &cfg.agent_launch,
            ),
        };
        by_frecency(candidates, &cfg.agent_frecency, unix_now())
    }

    /// Open a new shell for `agent` and start it there.
    pub(crate) fn launch_agent(
        &mut self,
        agent: CLIAgent,
        at: SpawnWhere,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = agent.launch_command(&cx.global::<Config>().agent_launch);
        let slot = match at {
            SpawnWhere::NewTab => {
                let cwd = self.tabs.get(self.tabs.active_index()).and_then(|t| {
                    t.pane
                        .focused_or_first(window, cx)
                        .and_then(|leaf| leaf.read(cx).spawnable_cwd())
                });
                self.new_tab_slot(cwd, None, window, cx)
            }
            SpawnWhere::Split => {
                self.split_slot(Axis::Horizontal, Some(SpawnAs::Shell(None)), window, cx)
            }
        };
        // Nothing opened (the spawn failed, or this workspace cannot host a
        // shell right now), and the reason is already on screen. The command
        // goes nowhere rather than into whatever pane was focused before.
        let Some(slot) = slot else {
            log::warn!("no pane opened for {}; not launching it", agent.slug());
            return;
        };
        run_when_ready(&slot, command, cx);
        // Recency only: the run is counted when the pane reports the agent
        // actually running, so a launch and its detection are one use, not two.
        self.update_config(cx, |cfg| {
            cfg.agent_frecency
                .entry(agent.slug().to_string())
                .or_default()
                .last_used = unix_now();
        });
    }

    /// Reopen `agent`'s session `session_id` in a new tab in `cwd`, the
    /// directory it ran in — an agent keys its history by directory, and
    /// `--resume` elsewhere finds nothing. The resume carries the flags the
    /// agent is configured to launch with (`agent_launch`), so a session
    /// started with them comes back with them.
    ///
    /// With `fork`, the session is branched instead: the agent starts a new
    /// one from its history and the original is left as it was.
    pub(crate) fn resume_session(
        &mut self,
        agent: CLIAgent,
        session_id: &str,
        cwd: Option<std::path::PathBuf>,
        host_id: crate::ui::host_ops::HostId,
        scope: Option<lyrel_core::host::session_operations::SessionScope>,
        fork: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use lyrel_core::host::session_operations::{SessionOperation, SessionOperationKind};
        let Some(cwd) = cwd else {
            window.notify_app(crate::ui::i18n::t(L10nKey::AppPaneNoKnownDirectory), cx);
            return;
        };
        if self.spawn_host(cx) != host_id {
            window.notify_app(
                crate::ui::notification_history::Notification::error(
                    "the selected history belongs to another host",
                ),
                cx,
            );
            return;
        }
        let Some(host) = crate::ui::host_registry::HostRegistry::lookup(cx, host_id) else {
            window.notify_app(crate::ui::i18n::t(L10nKey::RemoteNoticeDisconnected), cx);
            return;
        };
        let launch = agent.launch_command(&cx.global::<Config>().agent_launch);
        let launch_argv = match crate::core::cli_agent::parse_command_argv(&launch) {
            Ok(argv) if !argv.is_empty() => argv,
            _ => {
                window.notify_app(
                    crate::ui::notification_history::Notification::error(
                        "the configured agent launch is not a literal argv command",
                    ),
                    cx,
                );
                return;
            }
        };
        let mut scope = scope.unwrap_or_default();
        if host_id.is_local() {
            scope.shell_program = Some(
                cx.global::<Config>()
                    .shell
                    .as_ref()
                    .map(|shell| shell.program.clone())
                    .unwrap_or_else(crate::core::shells::login_shell),
            );
        } else {
            // A new remote pane uses that Host's default, not a saved shell
            // selector or the Windows desktop's platform fallback.
            scope.shell_program = None;
        }
        let request = SessionOperation {
            expected_host: host_id.0,
            agent,
            kind: if fork {
                SessionOperationKind::Fork
            } else {
                SessionOperationKind::Resume
            },
            source_id: session_id.into(),
            cwd,
            launch_argv,
            scope,
        };
        crate::ui::host_ops::HostOps::run_in(
            host,
            window,
            cx,
            move |host| host.session_operation(&request),
            |app, result, window, cx| match result {
                Ok(prepared) => app.open_prepared_session(prepared, window, cx),
                Err(error) => app.notify_session_operation_error(error, window, cx),
            },
        );
    }

    pub(crate) fn open_prepared_session(
        &mut self,
        prepared: lyrel_core::host::session_operations::PreparedSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.spawn_host(cx).0 != prepared.host_id {
            self.notify_session_operation_error(
                prepared.opening_failed("the selected workspace moved to another host"),
                window,
                cx,
            );
            return;
        }
        let Some(command) = prepared.command.clone() else {
            self.notify_session_operation_error(
                prepared.opening_failed(
                    "the session exists but its launch context could not be prepared",
                ),
                window,
                cx,
            );
            return;
        };
        let Some(slot) = self.new_tab_slot(Some(prepared.cwd.clone()), None, window, cx) else {
            self.notify_session_operation_error(
                prepared.opening_failed("the prepared session's new pane could not be opened"),
                window,
                cx,
            );
            return;
        };
        if let PaneSlot::Connecting(pending) = &slot {
            // The failed connection card retries this exact prepared identity,
            // not the source fork operation.
            pending.update(cx, |pending, _| {
                pending.resume_agent(
                    Some(prepared.agent),
                    prepared.target_id.clone(),
                    Some(prepared.launch_argv.clone()),
                    Some(prepared.scope.clone()),
                    false,
                );
            });
        }
        run_when_ready(&slot, command, cx);
    }

    pub(crate) fn notify_session_operation_error(
        &mut self,
        error: lyrel_core::host::session_operations::SessionOperationError,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::ui::notification_history::Notification;
        let mut note = Notification::error(error.to_string()).autohide(false);
        if let Some(prepared) = error.retained {
            note = note
                .title(crate::ui::i18n::t(L10nKey::TryAgain))
                .on_click(cx.listener(move |app, _, window, cx| {
                    app.retry_prepared_session((*prepared).clone(), window, cx);
                }));
        }
        window.notify_app(note, cx);
    }

    fn retry_prepared_session(
        &mut self,
        prepared: lyrel_core::host::session_operations::PreparedSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use lyrel_core::host::session_operations::{SessionOperation, SessionOperationKind};
        let Some(target_id) = prepared.target_id else {
            return;
        };
        let Some(host) = crate::ui::host_registry::HostRegistry::lookup(
            cx,
            crate::ui::host_ops::HostId(prepared.host_id),
        ) else {
            window.notify_app(crate::ui::i18n::t(L10nKey::RemoteNoticeDisconnected), cx);
            return;
        };
        let mut scope = prepared.scope;
        scope.session_file = prepared.session_file;
        let request = SessionOperation {
            expected_host: prepared.host_id,
            agent: prepared.agent,
            kind: SessionOperationKind::Resume,
            source_id: target_id,
            cwd: prepared.cwd,
            launch_argv: prepared.launch_argv,
            scope,
        };
        crate::ui::host_ops::HostOps::run_in(
            host,
            window,
            cx,
            move |host| host.session_operation(&request),
            |app, result, window, cx| match result {
                Ok(prepared) => app.open_prepared_session(prepared, window, cx),
                Err(error) => app.notify_session_operation_error(error, window, cx),
            },
        );
    }

    /// "New Agent Tab": launch the agent used most recently.
    pub(crate) fn new_agent_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let offered = self.offered_agents(cx);
        match most_recent(&offered, &cx.global::<Config>().agent_frecency) {
            Some(agent) => self.launch_agent(agent, SpawnWhere::NewTab, window, cx),
            None => {
                let remote = WorkspaceStore::remote_ref(cx, self.workspace).is_some();
                window.notify_app(
                    crate::ui::i18n::t(if remote {
                        L10nKey::AppNoAgentSeenHere
                    } else {
                        L10nKey::AppNoAgentOnPath
                    }),
                    cx,
                );
            }
        }
    }

    /// A pane in this window started running `agent`.
    pub(crate) fn note_agent_detected(&mut self, agent: CLIAgent, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| {
            let entry = cfg
                .agent_frecency
                .entry(agent.slug().to_string())
                .or_default();
            entry.count = entry.count.saturating_add(1);
            entry.last_used = unix_now();
        });
        if WorkspaceStore::remote_ref(cx, self.workspace).is_some() {
            WorkspaceStore::record_agent_seen(cx, self.workspace, agent);
        }
    }

    /// "Set Current Launch Args as Default": make the focused pane's agent
    /// launch the way this one was started.
    pub(crate) fn save_agent_launch_args(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self
            .tabs
            .get(self.tabs.active_index())
            .and_then(|t| t.pane.focused_or_first(window, cx))
        else {
            return;
        };
        let view = view.read(cx);
        let Some(agent) = view.agent() else {
            window.notify_app(crate::ui::i18n::t(L10nKey::AppPaneNoCodingAgent), cx);
            return;
        };
        let name = agent.display_name();
        let Some(line) = view
            .agent_session()
            .and_then(|s| s.launch_argv)
            .and_then(|argv| launch_line_to_save(agent, &argv))
        else {
            window.notify_app(
                t_fmt(L10nKey::AppAgentLaunchArgsUnknown, &[("name", name)]),
                cx,
            );
            return;
        };
        let saved = line.clone();
        self.update_config(cx, |cfg| remember_launch_line(cfg, agent, saved));
        window.notify_app(
            t_fmt(
                L10nKey::AppAgentLaunchSaved,
                &[("name", name), ("command", &line)],
            ),
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(pairs: &[(&str, u32, u64)]) -> HashMap<String, ProfileUsage> {
        pairs
            .iter()
            .map(|(slug, count, last_used)| {
                (
                    slug.to_string(),
                    ProfileUsage {
                        count: *count,
                        last_used: *last_used,
                    },
                )
            })
            .collect()
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    const NOW: u64 = 1_800_000_000;
    #[gpui_kit::test]
    fn refused_launch_is_visible_and_actionable(cx: &mut gpui_kit::TestAppContext) {
        use crate::ui::notification_history::{Severity, history};
        let (app, mut window, _streams) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
        window.update(|window, cx| window.notify_app("launch fixture ready", cx));
        app.update_in(&mut window, |app, window, cx| {
            let view = app
                .tabs
                .get(0)
                .unwrap()
                .pane
                .focused_or_first(window, cx)
                .unwrap();
            run_when_ready(
                &PaneSlot::Ready(view),
                "PI_IMAGE_PROTOCOL=none SECRET=hidden pi".into(),
                cx,
            );
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
        loop {
            window.executor().advance_clock(PROMPT_POLL);
            window.run_until_parked();
            let received = window.read(|cx| {
                history(cx).read(cx).records().any(|record| {
                    record.severity() == Severity::Error
                        && record.message().contains("agent_launch")
                })
            });
            if received {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "launch refusal must be an actionable error notification"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    const DAY: u64 = 86_400;

    #[test]
    fn agents_are_offered_most_used_first() {
        let used = usage(&[
            ("pi", 3, NOW - DAY),
            ("omp", 12, NOW - 2 * DAY),
            // Used a lot, but so long ago that it has decayed below Pi.
            ("kimi", 20, NOW - 200 * DAY),
        ]);
        let offered = by_frecency(
            [
                CLIAgent::Hermes,
                CLIAgent::Kimi,
                CLIAgent::Pi,
                CLIAgent::OhMyPi,
                CLIAgent::OpenCode,
            ],
            &used,
            NOW,
        );
        assert_eq!(
            offered,
            vec![
                CLIAgent::OhMyPi,
                CLIAgent::Pi,
                CLIAgent::Kimi,
                // Never used: they keep the order they were found in.
                CLIAgent::Hermes,
                CLIAgent::OpenCode,
            ]
        );
    }

    #[test]
    fn new_agent_tab_opens_the_most_recently_used_agent() {
        let offered = [CLIAgent::OhMyPi, CLIAgent::Pi, CLIAgent::Kimi];
        // OhMyPi is used more, but Pi was the last one run or launched.
        let used = usage(&[
            ("omp", 40, NOW - DAY),
            ("pi", 1, NOW - 60),
            ("hermes", 5, NOW),
        ]);
        assert_eq!(most_recent(&offered, &used), Some(CLIAgent::Pi));
        // Hermes was more recent still, but it is not offered here.
        // Nothing used yet: the first one offered.
        assert_eq!(
            most_recent(&offered, &HashMap::new()),
            Some(CLIAgent::OhMyPi)
        );
        assert_eq!(most_recent(&[], &used), None);
        // A launch that only stamped recency counts as the most recent.
        let launched = usage(&[("omp", 40, NOW - DAY), ("kimi", 0, NOW)]);
        assert_eq!(most_recent(&offered, &launched), Some(CLIAgent::Kimi));
    }

    #[test]
    fn saved_launch_args_survive_a_round_trip_through_the_command_line() {
        let run = argv(&[
            "/opt/homebrew/bin/omp",
            "--append-system-prompt",
            "be terse, don't guess",
            "--add-dir",
            "/Users/me/My Projects",
            "--model",
            "opus",
            "--resume",
            "abc-123",
        ]);
        let line = launch_line_to_save(CLIAgent::OhMyPi, &run).unwrap();
        assert_eq!(
            crate::ui::settings::arguments::split_shell_args(&line).unwrap(),
            argv(&[
                "omp",
                "--append-system-prompt",
                "be terse, don't guess",
                "--add-dir",
                "/Users/me/My Projects",
                "--model",
                "opus",
            ])
        );
        // And the saved line is recognised as the agent it launches.
        assert_eq!(
            CLIAgent::detect_from_command_with(&line, &HashMap::new()),
            Some(CLIAgent::OhMyPi)
        );
    }

    #[test]
    fn set_current_launch_args_as_default_writes_the_agent_launch_entry() {
        let mut cfg = Config::default();
        cfg.agent_launch
            .insert("OMP".to_string(), "omp --old".to_string());
        cfg.agent_launch
            .insert("pi".to_string(), "pi --model o3".to_string());
        let line = launch_line_to_save(
            CLIAgent::OhMyPi,
            &argv(&["omp", "--dangerously-skip-permissions", "--continue"]),
        )
        .unwrap();
        remember_launch_line(&mut cfg, CLIAgent::OhMyPi, line);

        let json: serde_json::Value = serde_json::to_value(&cfg.0).unwrap();
        assert_eq!(
            json["agent_launch"],
            serde_json::json!({
                "omp": "omp --dangerously-skip-permissions",
                "pi": "pi --model o3",
            })
        );
        assert_eq!(
            CLIAgent::OhMyPi.launch_command(&cfg.agent_launch),
            "omp --dangerously-skip-permissions"
        );
    }

    #[test]
    fn every_agent_has_a_bindable_launch_action() {
        for agent in CLIAgent::ALL {
            let name = launch_action_name(agent);
            assert_eq!(agent_for_launch_action(name), Some(agent), "{name}");
        }
        assert_eq!(agent_for_launch_action("LaunchAgent:nobody"), None);
        assert_eq!(agent_for_launch_action("NewAgentTab"), None);
    }

    #[test]
    fn only_agents_whose_launch_program_is_on_path_are_offered() {
        let dir = tempfile::TempDir::new().unwrap();
        for name in ["pi", "cc"] {
            let path = dir.path().join(name);
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let path = dir.path().as_os_str();
        assert_eq!(installed_on(path, &HashMap::new()), vec![CLIAgent::Pi]);
        // With an override, the override's program is what has to be there.
        let overrides: HashMap<String, String> = [
            ("omp".to_string(), "cc --fast".to_string()),
            ("pi".to_string(), "pi-nightly".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(installed_on(path, &overrides), vec![CLIAgent::OhMyPi]);
    }

    #[test]
    fn retired_launch_shortcuts_are_inert_and_supported_ones_still_resolve() {
        assert_eq!(agent_for_launch_action("LaunchAgent:claude"), None);
        assert_eq!(agent_for_launch_action("LaunchAgent:codex"), None);
        assert_eq!(
            agent_for_launch_action("LaunchAgent:omp"),
            Some(CLIAgent::OhMyPi)
        );
        assert_eq!(
            agent_for_launch_action("LaunchAgent:pi"),
            Some(CLIAgent::Pi)
        );
    }
}
