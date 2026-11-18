//! `.lyrel/setup`: the project's own "make this checkout runnable" step —
//! install dependencies, write a `.env.local`, seed a database.
//!
//! It is an executable in the repo, any language, run in the new worktree's
//! first pane before the agent starts, so its output is on screen and a
//! failure stops there instead of under an agent that cannot build. lyrel does
//! not run it anywhere else: the `Host` trait cannot run a command, and a
//! pane is where a person can see what a script did.
//!
//! It is code from the repo, so it runs only once someone has approved that
//! exact content ([`Setup::digest`]) for that repo; an edit asks again.
//!
//! The script is told where the main checkout and the worktree are, and
//! `LYREL_PORT` starts a block of ten ports the
//! worktree can use without colliding with its siblings — derived from its
//! path, so no allocator has to remember anything and a worktree recreated at
//! the same place gets the same ports back.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::core::cli_agent::CLIAgent;
use crate::core::shell_quote::quote_for_shell;
use crate::host::{Host, HostId, fnv1a64};

pub const SCRIPT: &[&str] = &[".lyrel", "setup"];

const PORT_FIRST: u16 = 20_000;
const PORT_BLOCKS: u64 = 1_000;
pub const PORT_BLOCK: u16 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setup {
    pub script: PathBuf,
    /// sha256 of the script, hex: what an approval is for.
    pub digest: String,
}

/// The worktree's setup script, if it has one.
pub fn find(host: &dyn Host, worktree: &Path) -> Option<Setup> {
    let script = SCRIPT
        .iter()
        .fold(worktree.to_path_buf(), |p, c| host.join(&p, c));
    let bytes = host.read_file(&script, 1 << 20).ok()?;
    Some(Setup {
        script,
        digest: hex(&Sha256::digest(&bytes)),
    })
}

/// What an approval is filed under: the repo, on the machine it lives on.
pub fn trust_key(host: HostId, main_root: &Path) -> String {
    format!("{}:{}", host.0, main_root.display())
}

/// The first of the worktree's [`PORT_BLOCK`] ports.
pub fn port_base(worktree: &Path) -> u16 {
    let h = fnv1a64(worktree.to_string_lossy().as_bytes());
    PORT_FIRST + (h % PORT_BLOCKS) as u16 * PORT_BLOCK
}

/// The variables the script runs with. `None` unless both paths are
/// absolute: a script doing `rm -rf "$LYREL_WORKTREE_PATH/dist"` must never
/// see an empty or relative value.
pub fn env(
    host: &dyn Host,
    main_root: &Path,
    worktree: &Path,
) -> Option<Vec<(&'static str, String)>> {
    if !host.is_absolute(main_root) || !host.is_absolute(worktree) {
        return None;
    }
    let name = worktree.file_name()?.to_string_lossy().into_owned();
    Some(vec![
        ("LYREL_ROOT_PATH", main_root.display().to_string()),
        ("LYREL_WORKTREE_PATH", worktree.display().to_string()),
        ("LYREL_WORKTREE_NAME", name),
        ("LYREL_PORT", port_base(worktree).to_string()),
    ])
}

/// For a repo without a setup script: the install its lockfile implies, as a
/// suggestion to show — never something run on the user's behalf.
pub fn hint(host: &dyn Host, worktree: &Path) -> Option<&'static str> {
    const LOCKS: &[(&str, &str)] = &[
        ("pnpm-lock.yaml", "pnpm install"),
        ("bun.lock", "bun install"),
        ("bun.lockb", "bun install"),
        ("yarn.lock", "yarn install"),
        ("package-lock.json", "npm ci"),
        ("uv.lock", "uv sync"),
        ("poetry.lock", "poetry install"),
        ("Gemfile.lock", "bundle install"),
        ("composer.lock", "composer install"),
        ("mix.lock", "mix deps.get"),
    ];
    LOCKS
        .iter()
        .find(|(file, _)| host.exists(&host.join(worktree, file)))
        .map(|(_, cmd)| *cmd)
}

/// The line a new worktree's first pane types: the setup script under its
/// variables, then the agent — chained with `&&`, so a failed setup stops
/// there, on screen, instead of under an agent that cannot build.
///
/// It opens with a `cd` into `worktree`. The pane is spawned there, but a
/// shell's startup files may move it (`cd ~/src` in a `.zshrc`), and a setup
/// run from the main checkout would install into — and write files into —
/// the wrong tree.
pub fn launch_line(
    worktree: &Path,
    setup: Option<(&Path, &[(&'static str, String)])>,
    agent: Option<String>,
    shell_program: &str,
) -> Option<String> {
    use crate::core::shell_quote::{Quoting, quoting_for};
    let q = |s: &str| quote_for_shell(s, Some(shell_program));
    let quoting = quoting_for(Some(shell_program));
    let chain = |first: String, next: String| match quoting {
        Quoting::PowerShell => format!("{first}; if ($?) {{ {next} }}"),
        _ => format!("{first} && {next}"),
    };
    let setup = setup.map(|(script, env)| {
        let mut parts = vec!["env".to_string()];
        parts.extend(env.iter().map(|(k, v)| format!("{k}={}", q(v))));
        parts.push(q(&script.to_string_lossy()));
        parts.join(" ")
    });
    let run = match (setup, agent) {
        (Some(setup), Some(agent)) => chain(setup, agent),
        (setup, agent) => setup.or(agent)?,
    };
    let path = q(&worktree.to_string_lossy());
    let cd = match quoting {
        Quoting::PowerShell => format!("Set-Location -LiteralPath {path}"),
        Quoting::Cmd => format!("cd /d {path}"),
        Quoting::Posix => format!("cd {path}"),
    };
    Some(chain(cd, run))
}

/// `agent`'s launch line (`overrides` is `Config::agent_launch`), opening on
/// `task` when the agent takes a first message. Kimi tasks fail explicitly:
/// its public `--prompt` mode does not retain the native TUI, and lyrel has
/// not verified an equivalent interactive delivery/readiness contract.
/// Call before creating the worktree or spawning a pane.
pub fn agent_line(
    agent: CLIAgent,
    task: &str,
    overrides: &HashMap<String, String>,
    shell_program: &str,
) -> Result<String, &'static str> {
    let task = if matches!(agent, CLIAgent::OhMyPi | CLIAgent::Hermes) {
        task
    } else {
        task.trim()
    };
    if agent == CLIAgent::Kimi && !task.is_empty() {
        return Err(
            "Kimi Code initial-task delivery to the native TUI is not available in lyrel; \
             no task was submitted. --prompt runs non-interactively and is not a substitute.",
        );
    }
    let mut line = agent.launch_command(overrides);
    if agent == CLIAgent::Hermes && task.trim().is_empty() {
        return Ok(line);
    }
    if agent == CLIAgent::Hermes && !task.trim().is_empty() {
        let mut args = crate::core::cli_agent::parse_command_argv(&line)?;
        let named = (0..args.len())
            .find(|index| CLIAgent::detect_from_argv(&args[..=*index]) == Some(agent))
            .ok_or("Hermes initial task requires an identifiable native executable")?;
        let mut scope = crate::host::session_operations::SessionScope::default();
        scope.shell_program = Some(shell_program.into());
        for assignment in args.drain(..named) {
            let (key, value) = assignment
                .split_once('=')
                .ok_or("Hermes initial task cannot replay a shell wrapper")?;
            if !crate::host::session_operations::SCOPE_ENV.contains(&key) {
                return Err(
                    "Hermes initial task accepts only documented scope environment selectors",
                );
            }
            scope.environment.insert(key.into(), value.into());
        }
        let values = [
            "--profile",
            "-p",
            "--model",
            "-m",
            "--provider",
            "--reasoning",
            "--toolsets",
            "-t",
            "--skills",
            "-s",
            "--max-turns",
            "--run-budget",
            "--in",
            "--source",
        ];
        let switches = [
            "--verbose",
            "-v",
            "--checkpoints",
            "--yolo",
            "--pass-session-id",
            "--ignore-rules",
            "--cli",
            "--tui",
            "--native",
            "--tui-native",
            "--dev",
            "--accept-hooks",
            "--safe-mode",
            "--ignore-user-config",
            "--no-restore-cwd",
        ];
        let mut index = 1;
        if args.get(index).is_some_and(|arg| arg == "chat") {
            args.remove(index);
        }
        while index < args.len() {
            let key = args[index].split('=').next().unwrap();
            if values.contains(&key) {
                index += if args[index].contains('=') { 1 } else { 2 };
                if index > args.len() {
                    return Err("Hermes launch option is missing its value");
                }
            } else if switches.contains(&key) {
                index += 1;
            } else {
                return Err(
                    "Hermes initial task requires an interactive launch without a prior query, session action, or one-shot/output mode",
                );
            }
        }
        args.insert(1, "chat".into());
        // Inline value is essential: argparse otherwise treats a --help-shaped
        // task as another option. On TTY, -q seeds once and returns to the REPL.
        args.push(format!("--query={task}"));
        return crate::host::session_operations::command_line(&args, &scope).map_err(
            |_| "Hermes initial-task arguments cannot be represented safely in the target shell",
        );
    }
    if agent == CLIAgent::Pi && !task.is_empty() {
        let argv = crate::core::cli_agent::command_argv(&line);
        let options = argv
            .iter()
            .take_while(|arg| arg.as_str() != "--")
            .collect::<Vec<_>>();
        if options.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "--print" | "-p" | "--mode=rpc" | "--mode=json"
            )
        }) || options
            .windows(2)
            .any(|pair| pair[0].as_str() == "--mode" && matches!(pair[1].as_str(), "rpc" | "json"))
        {
            return Err(
                "Pi initial tasks require the native TUI; --print and JSON/RPC modes cannot be used as interactive launch overrides.",
            );
        }
        if task.starts_with('@') {
            return Err(
                "Pi interprets leading @ as a file attachment even after --; an initial task beginning with @ cannot be submitted literally through this CLI interface.",
            );
        }
    }
    if agent == CLIAgent::OhMyPi && !task.trim().is_empty() {
        let mut args = crate::core::cli_agent::command_argv(&line);
        let named = (0..args.len())
            .find(|index| CLIAgent::detect_from_argv(&args[..=*index]) == Some(agent))
            .ok_or("OMP initial-task launch requires an identifiable native omp executable")?;
        let mut scope = crate::host::session_operations::SessionScope::default();
        scope.shell_program = Some(shell_program.into());
        for assignment in args.drain(..named) {
            let (key, value) = assignment
                .split_once('=')
                .ok_or("OMP initial-task launch cannot replay a shell wrapper")?;
            if !crate::host::session_operations::SCOPE_ENV.contains(&key) {
                return Err(
                    "OMP initial-task launch accepts only documented scope environment selectors",
                );
            }
            scope.environment.insert(key.into(), value.into());
        }
        let named = 0;
        let values = [
            "--cwd",
            "--profile",
            "--config",
            "--session-dir",
            "--add-dir",
            "--model",
            "--smol",
            "--slow",
            "--plan",
            "--models",
            "--provider",
            "--thinking",
            "--tools",
            "--approval-mode",
            "--service-tier",
            "--provider-session-id",
            "--prompt-cache-key",
            "--extension",
            "-e",
            "--hook",
            "--trusted-extension",
            "--plugin-dir",
            "--skills",
            "--system-prompt",
            "--system-prompt-template",
            "--append-system-prompt",
            "--max-time",
        ];
        let switches = [
            "--allow-home",
            "--no-title",
            "--hide-thinking",
            "--external-thinking",
            "--no-prewalk",
            "--no-tools",
            "--no-lsp",
            "--no-pty",
            "--auto-approve",
            "--yolo",
            "--advisor",
            "--no-extensions",
            "--no-skills",
            "--no-rules",
        ];
        let mut index = named + 1;
        if args.get(index).is_some_and(|arg| arg == "launch") {
            args.remove(index);
        }
        while index < args.len() {
            let key = args[index].split('=').next().unwrap();
            if values.contains(&key) {
                index += if args[index].contains('=') { 1 } else { 2 };
                if index > args.len() {
                    return Err("OMP launch option is missing its value");
                }
            } else if switches.contains(&key) {
                index += 1;
            } else {
                return Err(
                    "OMP initial-task launch must be a native interactive launch without existing prompts, session actions, credentials, or output modes",
                );
            }
        }
        args.insert(named + 1, "launch".into());
        args.extend(["--".into(), task.into()]);
        return crate::host::session_operations::command_line(&args, &scope).map_err(
            |_| "OMP initial-task arguments cannot be represented safely in the target shell",
        );
    }
    if let Some(args) = (!task.is_empty())
        .then(|| agent.prompt_args(task))
        .flatten()
    {
        for arg in args {
            line.push(' ');
            line.push_str(&quote_for_shell(&arg, Some(shell_program)));
        }
    }
    Ok(line)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn first_task_runs_once_in_the_selected_host_shell() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap();
        let task = "第一行 '$HOME'\n第二行 --help";
        for agent in [
            CLIAgent::OhMyPi,
            CLIAgent::Hermes,
            CLIAgent::Pi,
            CLIAgent::OpenCode,
        ] {
            let executable = directory.path().join(agent.slug());
            std::fs::write(&executable, "#!/bin/sh\nprintf '<%s>\\n' \"$@\"\n").unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            let overrides =
                HashMap::from([(agent.slug().into(), executable.display().to_string())]);
            let agent = agent_line(agent, task, &overrides, "/bin/sh").unwrap();
            let line = launch_line(directory.path(), None, Some(agent), "/bin/sh").unwrap();
            let output = std::process::Command::new("/bin/sh")
                .args(["-c", &line])
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            let output = String::from_utf8(output.stdout).unwrap();
            assert_eq!(output.matches(task).count(), 1, "{output}");
        }
    }

    #[test]
    fn hermes_initial_task_is_one_literal_seed_without_exit_mode() {
        for task in ["models", "--help", " 第一行\n第二行 ", "/new"] {
            let line = agent_line(CLIAgent::Hermes, task, &HashMap::new(), "sh").unwrap();
            assert_eq!(
                crate::core::cli_agent::command_argv(&line),
                ["hermes", "chat", &format!("--query={task}")]
            );
        }
        let overrides = HashMap::from([(
            "hermes".into(),
            "hermes chat --profile work --model team --cli".into(),
        )]);
        assert_eq!(
            agent_line(CLIAgent::Hermes, " \n\t", &overrides, "sh").unwrap(),
            "hermes chat --profile work --model team --cli"
        );
        let line = agent_line(CLIAgent::Hermes, "--help", &overrides, "sh").unwrap();
        assert_eq!(
            crate::core::cli_agent::command_argv(&line),
            [
                "hermes",
                "chat",
                "--profile",
                "work",
                "--model",
                "team",
                "--cli",
                "--query=--help"
            ]
        );
        for command in [
            "hermes chat --oneshot",
            "hermes -z old",
            "hermes --quiet",
            "hermes --resume old",
            "hermes chat -q old",
            "hermes sessions",
        ] {
            assert!(
                agent_line(
                    CLIAgent::Hermes,
                    "任务",
                    &HashMap::from([("hermes".into(), command.into())]),
                    "sh"
                )
                .is_err()
            );
        }
    }
    #[test]
    fn pi_initial_task_rejects_noninteractive_overrides_without_silent_mode_changes() {
        let mut overrides = HashMap::new();
        overrides.insert("pi".into(), "pi --mode rpc".into());
        assert!(agent_line(CLIAgent::Pi, "do the task", &overrides, "sh").is_err());
        assert_eq!(
            agent_line(CLIAgent::Pi, "", &overrides, "sh"),
            Ok("pi --mode rpc".into())
        );
        overrides.insert(
            "pi".into(),
            "pi --model provider/model --session-dir /store".into(),
        );
        let line = agent_line(CLIAgent::Pi, "--help\n中文", &overrides, "sh").unwrap();
        let argv = crate::core::cli_agent::command_argv(&line);
        assert_eq!(
            argv,
            [
                "pi",
                "--model",
                "provider/model",
                "--session-dir",
                "/store",
                "--",
                "--help\n中文"
            ]
        );
    }

    #[test]
    fn omp_initial_tasks_are_literal_native_interactive_messages() {
        for task in ["models", "--help", "第一行\n第二行", "launch"] {
            let command = agent_line(CLIAgent::OhMyPi, task, &HashMap::new(), "sh").unwrap();
            let args = crate::core::cli_agent::command_argv(&command);
            assert_eq!(args, vec!["omp", "launch", "--", task]);
        }
        let overrides = HashMap::from([(
            "omp".into(),
            "omp launch --profile work --model native".into(),
        )]);
        let command = agent_line(CLIAgent::OhMyPi, "models", &overrides, "sh").unwrap();
        assert_eq!(
            crate::core::cli_agent::command_argv(&command),
            vec![
                "omp",
                "launch",
                "--profile",
                "work",
                "--model",
                "native",
                "--",
                "models"
            ]
        );
        for line in [
            "omp --print",
            "omp --mode json",
            "omp models",
            "omp --resume old",
        ] {
            assert!(
                agent_line(
                    CLIAgent::OhMyPi,
                    "初始任务",
                    &HashMap::from([("omp".into(), line.into())]),
                    "sh"
                )
                .is_err()
            );
        }
    }

    #[test]
    fn ports_are_stable_blocks_inside_the_range() {
        let a = port_base(Path::new("/r/.lyrel/worktrees/quiet-otter"));
        assert_eq!(a, port_base(Path::new("/r/.lyrel/worktrees/quiet-otter")));
        assert_eq!((a - PORT_FIRST) % PORT_BLOCK, 0);
        assert!(a >= PORT_FIRST && a + PORT_BLOCK <= PORT_FIRST + 10_000);
        assert_ne!(a, port_base(Path::new("/r/.lyrel/worktrees/amber-heron")));
    }

    #[cfg(unix)]
    #[test]
    fn launch_line_chains_setup_before_the_agent_and_quotes_values() {
        let env = vec![
            ("LYREL_ROOT_PATH", "/r/my repo".to_string()),
            ("LYREL_PORT", "20010".to_string()),
        ];
        let wt = Path::new("/r/my repo/.lyrel/worktrees/w");
        let script = Path::new("/r/my repo/.lyrel/worktrees/w/.lyrel/setup");
        assert_eq!(
            launch_line(
                wt,
                Some((script, &env)),
                Some("pi -- 'fix it'".into()),
                "sh"
            )
            .unwrap(),
            "cd '/r/my repo/.lyrel/worktrees/w' && \
             env LYREL_ROOT_PATH='/r/my repo' LYREL_PORT=20010 \
             '/r/my repo/.lyrel/worktrees/w/.lyrel/setup' && pi -- 'fix it'"
        );
        assert_eq!(
            launch_line(wt, None, Some("omp".into()), "sh").unwrap(),
            "cd '/r/my repo/.lyrel/worktrees/w' && omp"
        );
        assert_eq!(launch_line(wt, None, None, "sh"), None);
    }

    #[test]
    fn kimi_initial_task_is_rejected_instead_of_silently_starting_an_empty_session() {
        let overrides = HashMap::from([(
            "kimi".to_string(),
            "kimi --model team-model --agent reviewer --add-dir ../shared".to_string(),
        )]);
        for task in [
            "inspect this checkout",
            "第一行\n第二行",
            "--model attacker-model",
            "fork session_other",
            "/new",
            "$(touch should-not-exist)",
        ] {
            let error = agent_line(CLIAgent::Kimi, task, &overrides, "sh")
                .expect_err("an undeliverable task must not become a bare Kimi launch");
            assert!(error.contains("native TUI"), "{error}");
            assert!(
                !error.contains(task),
                "task contents must not appear in the error"
            );
        }
    }

    #[test]
    fn kimi_without_a_task_preserves_the_configured_native_tui_launch() {
        let overrides = HashMap::from([(
            "kimi".to_string(),
            "KIMI_CODE_HOME=/team/kimi kimi --model team-model --agent reviewer".to_string(),
        )]);
        assert_eq!(
            agent_line(CLIAgent::Kimi, " \n\t", &overrides, "sh").unwrap(),
            "KIMI_CODE_HOME=/team/kimi kimi --model team-model --agent reviewer"
        );
        assert_eq!(
            agent_line(CLIAgent::Kimi, "", &HashMap::new(), "sh").unwrap(),
            "kimi"
        );
    }

    #[test]
    fn env_refuses_relative_paths() {
        let h = crate::host::local::LocalHost::new();
        // Absolute on every platform: `/abs` is not, on Windows.
        let root = std::env::temp_dir();
        let wt = root.join(".lyrel").join("worktrees").join("w");
        assert!(env(&*h, Path::new("rel"), &wt).is_none());
        assert!(env(&*h, &root, Path::new("")).is_none());
        let vars = env(&*h, &root, &wt).unwrap();
        assert_eq!(vars[2], ("LYREL_WORKTREE_NAME", "w".to_string()));
    }

    #[test]
    fn find_digests_the_script_and_hint_reads_lockfiles() {
        let dir = std::env::temp_dir().join(format!("lyrel-setup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".lyrel")).unwrap();
        let h = crate::host::local::LocalHost::new();
        assert!(find(&*h, &dir).is_none());
        assert_eq!(hint(&*h, &dir), None);
        std::fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(hint(&*h, &dir), Some("pnpm install"));
        std::fs::write(dir.join(".lyrel/setup"), "#!/bin/sh\n").unwrap();
        let a = find(&*h, &dir).unwrap();
        assert_eq!(a.digest.len(), 64);
        std::fs::write(dir.join(".lyrel/setup"), "#!/bin/sh\necho x\n").unwrap();
        assert_ne!(find(&*h, &dir).unwrap().digest, a.digest);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
