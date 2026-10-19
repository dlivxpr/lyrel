use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::PathBuf;

use super::{HookAgent, HookTarget};
use crate::core::cli_agent::{CLIAgent, parse_command_argv};

pub(super) struct ConfiguredScope {
    environment: BTreeMap<String, String>,
    hermes_profile: Option<String>,
}

impl HookTarget<'_> {
    /// Capture the configured launch selectors without running the launch command.
    pub fn with_launch_commands(mut self, commands: &HashMap<String, String>) -> Self {
        for agent in HookAgent::ALL {
            if let Some(command) = commands
                .get(agent.slug())
                .filter(|command| !command.trim().is_empty())
            {
                self.launch_scopes.insert(
                    agent,
                    parse(agent, command).map_err(|error| error.to_string()),
                );
            }
        }
        self
    }

    pub(crate) fn with_launch_arguments(mut self, agent: HookAgent, args: &[String]) -> Self {
        self.launch_scopes.insert(
            agent,
            parse_arguments(agent, args).map_err(|error| error.to_string()),
        );
        self
    }

    pub(super) fn configured_hermes_profile(&self) -> Option<&str> {
        self.launch_scopes
            .get(&HookAgent::Hermes)
            .and_then(|scope| scope.as_ref().ok())
            .and_then(|scope| scope.hermes_profile.as_deref())
    }

    pub(crate) fn agent_environment(
        &self,
        agent: HookAgent,
    ) -> io::Result<BTreeMap<String, String>> {
        let mut environment = self.host.agent_environment()?;
        if let Some(scope) = self.launch_scopes.get(&agent) {
            let scope = scope
                .as_ref()
                .map_err(|message| io::Error::new(io::ErrorKind::Unsupported, message.clone()))?;
            environment.extend(scope.environment.clone());
            if agent == HookAgent::Hermes
                && let Some(profile) = &scope.hermes_profile
            {
                let home = super::hermes::resolved_home(self, &environment, Some(profile))?;
                environment.insert("HERMES_HOME".into(), home.to_string_lossy().into_owned());
            }
        }
        Ok(environment)
    }

    pub(crate) fn agent_home(&self, environment: &BTreeMap<String, String>) -> io::Result<PathBuf> {
        let keys = if self.host.separator() == '\\' {
            ["USERPROFILE", "HOME"]
        } else {
            ["HOME", "USERPROFILE"]
        };
        let home = keys
            .into_iter()
            .find_map(|key| environment.get(key).filter(|value| !value.is_empty()))
            .map_or_else(|| self.home.clone(), PathBuf::from);
        if !self.host.is_absolute(&home) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "agent home override is not an absolute path on the selected Host",
            ));
        }
        Ok(home)
    }
}

fn parse(agent: HookAgent, command: &str) -> io::Result<ConfiguredScope> {
    let error = || {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "configured agent launch scope is ambiguous or unsupported; use a native executable with literal scope selectors, not a shell wrapper",
        )
    };
    let args = parse_command_argv(command).map_err(|_| error())?;
    parse_arguments(agent, &args)
}

fn parse_arguments(agent: HookAgent, args: &[String]) -> io::Result<ConfiguredScope> {
    let error = || {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "configured agent launch scope is ambiguous or unsupported; use a native executable with literal scope selectors, not a shell wrapper",
        )
    };
    let expected = CLIAgent::from_slug(agent.slug()).ok_or_else(error)?;
    let named = (0..args.len())
        .find(|index| CLIAgent::detect_from_argv(&args[..=*index]) == Some(expected))
        .ok_or_else(error)?;
    let mut environment = BTreeMap::new();
    let mut hermes_profile = None;
    for assignment in &args[..named] {
        let (key, value) = assignment.split_once('=').ok_or_else(error)?;
        if !crate::host::AGENT_ENVIRONMENT_KEYS.contains(&key) || value.contains(['$', '`']) {
            return Err(error());
        }
        environment.insert(key.into(), value.into());
    }
    let values = [
        "--cwd",
        "--profile",
        "--config",
        "--config-file",
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
        "--work-dir",
        "-w",
        "--server",
        "--agent",
        "--prompt",
        "--resume",
        "--session",
        "--restore",
        "--fork",
        "-r",
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
        "--no-session",
        "--standalone",
        "--continue",
        "-c",
    ];
    let mut index = named + 1;
    if args.get(index).is_some_and(|arg| {
        (agent == HookAgent::OhMyPi && arg == "launch")
            || (agent == HookAgent::Hermes && arg == "chat")
    }) {
        index += 1;
    }
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            break;
        }
        let (key, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(key, value)| (key, Some(value)));
        let hermes_value = agent == HookAgent::Hermes
            && [
                "-p",
                "-m",
                "-t",
                "-s",
                "--reasoning",
                "--toolsets",
                "--max-turns",
                "--run-budget",
                "--in",
                "--source",
                "--query",
                "-q",
                "--query-file",
            ]
            .contains(&key);
        let takes_value = if agent == HookAgent::Pi {
            crate::core::cli_agent::PI_VALUE_OPTIONS.contains(&key)
                || matches!(
                    key,
                    "--session"
                        | "--session-id"
                        | "--fork"
                        | "--session-dir"
                        | "--mode"
                        | "--export"
                        | "--resume"
                        | "-r"
                )
        } else {
            values.contains(&key) || hermes_value
        };
        if takes_value {
            let value = if let Some(value) = inline {
                value
            } else {
                index += 1;
                args.get(index).ok_or_else(error)?.as_str()
            };
            if value.contains(['$', '`']) {
                return Err(error());
            }
            if agent == HookAgent::Pi && key == "--session-dir" {
                environment.insert("PI_CODING_AGENT_SESSION_DIR".into(), value.into());
            }
            if key == "--profile" || (agent == HookAgent::Hermes && key == "-p") {
                match agent {
                    HookAgent::OhMyPi => {
                        environment.insert("OMP_PROFILE".into(), value.into());
                    }
                    HookAgent::Hermes => {
                        hermes_profile = Some(value.to_owned());
                    }
                    _ => return Err(error()),
                }
            }
        } else if !switches.contains(&key)
            && !(agent == HookAgent::Pi && crate::core::cli_agent::PI_SWITCHES.contains(&key))
            && !(agent == HookAgent::Hermes
                && [
                    "--verbose",
                    "-v",
                    "--checkpoints",
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
                ]
                .contains(&key))
        {
            return Err(error());
        }
        index += 1;
    }
    Ok(ConfiguredScope {
        environment,
        hermes_profile,
    })
}
