use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Pi 1.3.0 switches that never consume a following positional task.
pub(crate) const PI_SWITCHES: &[&str] = &[
    "--no-extensions",
    "-ne",
    "--no-mcp",
    "--no-skills",
    "-ns",
    "--no-prompt-templates",
    "-np",
    "--no-themes",
    "--no-context-files",
    "-nc",
    "--no-tools",
    "-nt",
    "--no-builtin-tools",
    "-nbt",
    "--verbose",
    "--approve",
    "-a",
    "--no-approve",
    "-na",
    "--offline",
    "--no-session",
    "--print",
    "-p",
    "--help",
    "-h",
    "--version",
    "-v",
];

/// Pi 1.3.0 native value options preserved when replaying an interactive launch.
pub(crate) const PI_VALUE_OPTIONS: &[&str] = &[
    "--provider",
    "--model",
    "--thinking",
    "--models",
    "--tools",
    "-t",
    "--exclude-tools",
    "-xt",
    "--extension",
    "-e",
    "--skill",
    "--prompt-template",
    "--theme",
    "--use-theme",
    "--system-prompt",
    "--append-system-prompt",
    "--tui-mode",
    "--name",
    "-n",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CLIAgent {
    OhMyPi,
    Kimi,
    Pi,
    OpenCode,
    Hermes,
}

/// Unknown identities lose only their agent annotation, never the containing pane.
/// The wire representation remains the enum's existing JSON string.
pub fn deserialize_optional_agent<'de, D>(deserializer: D) -> Result<Option<CLIAgent>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let identity = Option::<String>::deserialize(deserializer)?;
    Ok(identity.and_then(|name| serde_json::from_value(serde_json::Value::String(name)).ok()))
}

impl CLIAgent {
    pub const ALL: [CLIAgent; 5] = [
        CLIAgent::OhMyPi,
        CLIAgent::Kimi,
        CLIAgent::Pi,
        CLIAgent::OpenCode,
        CLIAgent::Hermes,
    ];

    fn aliases(self) -> &'static [&'static str] {
        match self {
            CLIAgent::OhMyPi => &["omp"],
            // Only the current official Kimi Code product contract is supported;
            // detecting a process name never migrates legacy configuration/data.
            CLIAgent::Kimi => &["kimi"],
            CLIAgent::Pi => &["pi"],
            CLIAgent::OpenCode => &["opencode", "opencode2"],
            CLIAgent::Hermes => &["hermes"],
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            CLIAgent::OpenCode => "opencode",

            CLIAgent::Pi => "pi",

            CLIAgent::Hermes => "hermes",

            CLIAgent::OhMyPi => "omp",
            CLIAgent::Kimi => "kimi",
        }
    }

    pub fn from_slug(name: &str) -> Option<CLIAgent> {
        let name = name.trim();
        CLIAgent::ALL
            .into_iter()
            .find(|a| a.slug().eq_ignore_ascii_case(name))
    }

    pub fn display_name(self) -> &'static str {
        match self {
            CLIAgent::OpenCode => "OpenCode",

            CLIAgent::Pi => "Pi",

            CLIAgent::Hermes => "Hermes",

            CLIAgent::OhMyPi => "Oh My Pi",
            CLIAgent::Kimi => "Kimi Code",
        }
    }

    pub fn fork_label(self) -> Option<&'static str> {
        Some("Fork Session")
    }

    /// What follows the agent's own name in `argv`, with everything that names
    /// a session taken out: the resume/fork subcommands and the flags listed as
    /// stale for this agent, each with its value. `None` when no token in
    /// `argv` names this agent — a wrapper, where there is no telling which
    /// arguments are the agent's.
    pub(crate) fn session_free_tail(self, argv: &[String]) -> Option<Vec<&str>> {
        let names_self = |token: &str| {
            token.split(['/', '\\']).any(|seg| {
                CLIAgent::match_token(&base_stem(seg).to_ascii_lowercase()) == Some(self)
            })
        };
        let argv = &argv[argv.iter().take_while(|t| is_env_assignment(t)).count()..];
        let named = argv.iter().position(|t| names_self(t))?;
        let mut tail: Vec<&str> = argv[named + 1..].iter().map(String::as_str).collect();
        if self == CLIAgent::Pi
            && let Some(boundary) = tail.iter().position(|arg| *arg == "--")
        {
            tail.truncate(boundary);
        }

        // Hermes 的 chat 只选择启动入口，不属于需要重放的参数。
        if self == CLIAgent::Hermes && tail.first().is_some_and(|arg| *arg == "chat") {
            tail.remove(0);
        }

        let stale: &[&str] = match self {
            CLIAgent::Hermes => &["--resume", "-r", "--continue", "-c"],

            CLIAgent::OpenCode => &["--session", "-s", "--continue", "-c", "--fork"],

            CLIAgent::Pi => &[
                "--session",
                "--session-id",
                "--fork",
                "--resume",
                "-r",
                "--continue",
                "-c",
            ],
            // `--resume`, `-r` and `--session` are three spellings of one flag
            // in Oh My Pi; `--session-dir` is a different one and survives.
            CLIAgent::OhMyPi => &[
                "--resume",
                "-r",
                "--session",
                "--restore",
                "--fork",
                "--continue",
                "-c",
            ],
            // `--resume`/`-r` is Kimi's hidden alias for `--session`/`-S`.
            // `--agent`/`--agent-file` bind the main agent at session creation
            // and Kimi rejects either next to `--session` outright; resuming
            // restores the bound agent by itself, so replaying them would only
            // turn a working resume into a startup error.
            CLIAgent::Kimi => &[
                "--session",
                "-S",
                "--resume",
                "-r",
                "--continue",
                "-c",
                "--agent",
                "--agent-file",
            ],
        };
        let mut i = 0;
        while i < tail.len() {
            let t = tail[i];
            if stale.contains(&t)
                || stale
                    .iter()
                    .any(|f| f.len() > 2 && t.starts_with(&format!("{f}=")))
            {
                tail.remove(i);
                if i < tail.len() && !tail[i].starts_with('-') {
                    tail.remove(i);
                }
            } else {
                i += 1;
            }
        }
        Some(tail)
    }

    pub fn accent_rgb(self) -> u32 {
        match self {
            CLIAgent::OpenCode => 0x6E56CF,

            CLIAgent::Pi => 0x0EA5E9,

            CLIAgent::Hermes => 0x8B5CF6,

            CLIAgent::OhMyPi => 0xF97316,
            // The blue flame is Kimi's brand accent; its mark is monochrome.
            CLIAgent::Kimi => 0x027AFF,
        }
    }

    pub fn icon_rgb(self) -> u32 {
        0xFFFFFF
    }

    pub fn icon_path(self) -> &'static str {
        match self {
            CLIAgent::OpenCode => "icons/agents/opencode.svg",

            CLIAgent::Pi => "icons/agents/pi.svg",
            CLIAgent::OhMyPi => "icons/agents/omp.svg",

            CLIAgent::Kimi => "icons/agents/kimi.svg",

            CLIAgent::Hermes => "icons/bot.svg",
        }
    }

    fn match_token(token: &str) -> Option<CLIAgent> {
        CLIAgent::ALL
            .into_iter()
            .find(|a| a.aliases().contains(&token))
    }

    /// The program a launch with no override runs: the first name the agent
    /// is detected by, which is the one its installer puts on `PATH`.
    pub fn binary(self) -> &'static str {
        self.aliases()[0]
    }

    /// The command line a quick launch types for this agent: the user's
    /// `agent_launch` entry when there is one, the bare binary otherwise.
    ///
    /// Keys are matched the way [`CLIAgent::from_slug`] reads them, so a
    /// hand-written `"Claude"` still counts. A blank entry is no override.
    pub fn launch_command(self, overrides: &HashMap<String, String>) -> String {
        overrides
            .iter()
            .find(|(slug, _)| CLIAgent::from_slug(slug) == Some(self))
            .map(|(_, line)| line.trim())
            .filter(|line| !line.is_empty())
            .map_or_else(|| self.binary().to_string(), str::to_string)
    }

    /// The arguments that start an interactive session whose first message is
    /// `prompt`. `None` for an agent whose positional prompt means a one-shot,
    /// non-interactive run, or whose form is not known. In particular, Kimi's
    /// `--prompt` must not substitute for a native-TUI initial task.
    pub fn prompt_args(self, prompt: &str) -> Option<Vec<String>> {
        match self {
            CLIAgent::OpenCode => Some(vec!["--prompt".to_string(), prompt.to_string()]),
            CLIAgent::Pi => Some(vec!["--".into(), prompt.into()]),
            CLIAgent::OhMyPi => Some(vec![
                "launch".to_string(),
                "--".to_string(),
                prompt.to_string(),
            ]),
            CLIAgent::Hermes => Some(vec!["chat".to_string(), format!("--query={prompt}")]),
            _ => None,
        }
    }

    /// `argv` as a command line worth launching this agent with again: its
    /// binary followed by the flags it ran with, minus everything that names
    /// one particular session (a resume, a fork, a session id) and minus
    /// positional arguments such as a prompt, which belong to that one run.
    ///
    /// A run that went through something that does not name the agent — a
    /// wrapper script mapped by `agent_commands` — cannot be taken apart, so
    /// it comes back whole: it is what was typed, and it launched the agent.
    pub fn launch_argv_for_default(self, argv: &[String]) -> Option<Vec<String>> {
        let env = argv.iter().take_while(|t| is_env_assignment(t)).count();
        if argv.len() == env {
            return None;
        }
        let Some(tail) = self.session_free_tail(argv) else {
            return Some(argv.to_vec());
        };
        let mut out: Vec<String> = argv[..env].to_vec();
        out.push(self.binary().to_string());
        let mut takes_value = false;
        for t in tail {
            let is_flag = t.starts_with('-') && t.len() > 1;
            if is_flag {
                out.push(t.to_string());
                takes_value =
                    !t.contains('=') && !(self == CLIAgent::Pi && PI_SWITCHES.contains(&t));
            } else if takes_value {
                out.push(t.to_string());
                takes_value = false;
            }
        }
        Some(out)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn detect_from_argv(argv: &[String]) -> Option<CLIAgent> {
        Self::detect_from_argv_with(argv, &HashMap::new())
    }

    pub fn detect_from_argv_with(
        argv: &[String],
        custom: &HashMap<String, String>,
    ) -> Option<CLIAgent> {
        let mut rest = argv
            .iter()
            .map(String::as_str)
            .skip_while(|t| is_env_assignment(t));

        let launcher = rest.next()?;
        let launcher_stem = base_stem(launcher);

        if let Some(agent) = CLIAgent::match_token(launcher_stem) {
            return Some(agent);
        }
        if let Some(agent) = custom
            .get(&launcher_stem.to_ascii_lowercase())
            .and_then(|slug| CLIAgent::from_slug(slug))
        {
            return Some(agent);
        }

        // A wrapper that is a script runs as its interpreter, so the name the
        // user mapped is the script's, one token in: `bash ~/bin/cc`,
        // `node ~/bin/cc.js`. Only the mapped names are looked for there — the
        // built-in ones already have the scan below.
        if (is_interpreter(launcher_stem) || is_shell(launcher_stem))
            && let Some(agent) = rest
                .clone()
                .find(|a| !a.starts_with('-') && !is_env_assignment(a))
                .and_then(|script| custom.get(&base_stem(script).to_ascii_lowercase()))
                .and_then(|slug| CLIAgent::from_slug(slug))
        {
            return Some(agent);
        }

        if is_interpreter(launcher_stem) {
            for arg in rest {
                if arg.starts_with('-') {
                    continue;
                }
                let mut previous = "";
                for segment in arg.split(['/', '\\']) {
                    let packaged = match (previous, segment) {
                        ("@earendil-works", "pi-coding-agent") => Some(CLIAgent::Pi),
                        ("@oh-my-pi", "pi-coding-agent") => Some(CLIAgent::OhMyPi),
                        _ => None,
                    };
                    if packaged.is_some() {
                        return packaged;
                    }
                    previous = segment;
                    if let Some(agent) =
                        CLIAgent::match_token(&base_stem(segment).to_ascii_lowercase())
                    {
                        return Some(agent);
                    }
                }
            }
        }

        None
    }

    pub fn detect_from_command_with(
        command: &str,
        custom: &HashMap<String, String>,
    ) -> Option<CLIAgent> {
        let argv: Vec<String> = command_argv(command)
            .iter()
            .map(|t| t.to_ascii_lowercase())
            .collect();
        Self::detect_from_argv_with(&argv, custom)
    }
}

/// Captured commands keep literal quoted arguments, including Windows paths.
/// A command requiring shell evaluation cannot supply trustworthy replay argv.
pub fn command_argv(command: &str) -> Vec<String> {
    parse_command_argv(command).unwrap_or_default()
}

/// Parse a literal launch command, not an arbitrary shell program. Callers
/// initiating a launch must surface the error rather than use default flags.
pub fn parse_command_argv(command: &str) -> Result<Vec<String>, &'static str> {
    let mut argv = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    let mut call_operator = false;
    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some('\''), '\'') if cfg!(windows) && chars.peek() == Some(&'\'') => {
                chars.next();
                word.push('\'');
            }
            (Some(q), c) if q == c => quote = None,
            (Some('\''), c) => word.push(c),
            (_, '\\') if chars.peek().is_some_and(|c| matches!(c, '\'' | '"')) => {
                word.push(chars.next().unwrap());
                started = true;
            }
            (_, '`') => {
                word.push(chars.next().ok_or("incomplete launch escape")?);
                started = true;
            }
            (_, '$') => {
                return Err(
                    "launch command requires shell variable evaluation; supply literal launch arguments",
                );
            }
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(ch);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    argv.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, '&')
                if argv.is_empty() && !started && !call_operator && chars.peek() != Some(&'&') =>
            {
                call_operator = true;
            }
            (None, ';' | '|' | '&' | '<' | '>') => {
                return Err(
                    "launch command contains shell operators; supply literal launch arguments",
                );
            }
            (None, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err("launch command has unbalanced quotes");
    }
    if started {
        argv.push(word);
    }
    Ok(argv)
}

fn is_env_assignment(token: &str) -> bool {
    match token.split_once('=') {
        Some((key, _)) => {
            let mut bytes = key.bytes();
            bytes
                .next()
                .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        }
        None => false,
    }
}

fn base_stem(token: &str) -> &str {
    let trimmed = token.trim_end_matches(['/', '\\']);
    let name = match trimmed.rfind(['/', '\\']) {
        Some(i) => &trimmed[i + 1..],
        None => trimmed,
    };
    for ext in [
        ".js", ".mjs", ".cjs", ".ts", ".py", ".rb", ".sh", ".exe", ".cmd", ".bat", ".ps1",
    ] {
        if let Some(stem) = name.strip_suffix(ext) {
            return stem;
        }
    }
    name
}

fn is_interpreter(stem: &str) -> bool {
    matches!(
        stem.to_ascii_lowercase().as_str(),
        "node"
            | "nodejs"
            | "bun"
            | "deno"
            | "npx"
            | "pnpm"
            | "yarn"
            | "python"
            | "python3"
            | "ruby"
            | "uv"
            | "uvx"
            | "env"
    )
}

fn is_shell(stem: &str) -> bool {
    matches!(
        stem.to_ascii_lowercase().as_str(),
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish"
    )
}

/// The program a launch command line runs: its first word that is not an
/// environment assignment.
pub fn launch_program(command: &str) -> Option<String> {
    command_argv(command)
        .into_iter()
        .find(|t| !is_env_assignment(t))
}

/// The wrapper names `agent_launch` implies, in the shape `agent_commands`
/// takes: `{"claude": "cc --fast"}` means a `cc` in a pane is Claude Code.
///
/// This is how a pane opened by a quick launch through a wrapper is still
/// recognised without the user also writing an `agent_commands` entry: the
/// launch typed exactly this command, so its program is known to be that
/// agent. A program that is already an agent's own name needs no entry, and
/// an interpreter (`npx`, `node`, `env`, …) or a shell is never mapped
/// wholesale — `npx` is not Claude Code just because Claude Code is one of the
/// things it can run.
pub fn launch_aliases(overrides: &HashMap<String, String>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (slug, line) in overrides {
        let Some(agent) = CLIAgent::from_slug(slug) else {
            continue;
        };
        let Some(program) = launch_program(line) else {
            continue;
        };
        let stem = base_stem(&program).to_ascii_lowercase();
        if stem.is_empty()
            || CLIAgent::match_token(&stem).is_some()
            || is_interpreter(&stem)
            || is_shell(&stem)
        {
            continue;
        }
        out.insert(stem, agent.slug().to_string());
    }
    out
}

/// Whether `program` names an executable file, either directly (it has a
/// directory in it) or through one of the directories in `path`.
///
/// On Windows a bare name is also tried with the extensions an npm or winget
/// install leaves behind, since `claude` there is really `claude.cmd`.
pub fn program_on_path(program: &str, path: &std::ffi::OsStr) -> bool {
    let program = program.trim();
    if program.is_empty() {
        return false;
    }
    if program.contains(['/', '\\']) {
        // The launch is typed into a shell, which expands a leading `~/`.
        let home = program
            .strip_prefix("~/")
            .and_then(|rest| Some(std::path::PathBuf::from(std::env::var_os("HOME")?).join(rest)));
        return is_executable(home.as_deref().unwrap_or(std::path::Path::new(program)));
    }
    let exts: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat", ".ps1"]
    } else {
        &[""]
    };
    std::env::split_paths(path).any(|dir| {
        exts.iter()
            .any(|ext| is_executable(&dir.join(format!("{program}{ext}"))))
    })
}

fn is_executable(path: &std::path::Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub const AGENT_EVENT_SENTINEL: &str = "lyrel://cli-agent";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentStatus {
    #[default]
    Idle,
    Working,
    Waiting,
    Done,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionState {
    #[serde(default = "AgentSessionState::default_status")]
    pub status: AgentStatus,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub launch_argv: Option<Vec<String>>,
    #[serde(default)]
    pub rich: bool,
    #[serde(default)]
    pub cwd: Option<std::path::PathBuf>,
    #[serde(default)]
    pub activity: u64,
    /// Unanswered human-input requests, keyed by kind and upstream identity.
    #[serde(default)]
    pub pending_requests: Vec<AgentRequestUpdate>,
    #[serde(default)]
    pub request_resume_status: AgentStatus,
    /// A real final boundary received while human input was still unresolved.
    #[serde(default)]
    pub pending_stop: bool,
    /// How many turns this session has finished: bumped each time it settles
    /// into `Done`. The status alone cannot tell a client that attaches to a
    /// `Done` pane whether that is the turn it already showed the reader or a
    /// later one that finished while nobody was watching (#870).
    #[serde(default)]
    pub turns: u64,
    #[serde(default)]
    pub readout: AgentReadout,
}

/// What an agent says about how it is set up — what the message composer's
/// toolbar shows. Each field is only ever what the agent reported; a field
/// it has not reported stays `None` rather than being guessed at.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentReadout {
    /// The agent's own word for it: `default`, `acceptEdits`, `plan`,
    /// `bypassPermissions`, …
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// The model id, as the agent's transcript records it.
    #[serde(default)]
    pub model: Option<String>,
    /// The reasoning effort level (`low`, `medium`, `high`, …); empty when
    /// the agent runs a model that takes none.
    #[serde(default)]
    pub effort: Option<String>,
    /// Captured by the running agent, never inferred from the GUI environment.
    #[serde(default)]
    pub session_scope: Option<crate::host::session_operations::SessionScope>,
    /// Client-local OpenCode view, never inferred from the shared service environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opencode_presence: Option<OpenCodePresence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<AgentResult>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentOutcome {
    Succeeded,
    Failed,
    Interrupted,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentResult {
    pub outcome: AgentOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentRequestKind {
    Permission,
    Question,
    PlanReview,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRequestUpdate {
    pub id: String,
    pub kind: AgentRequestKind,
    #[serde(default)]
    pub resolved: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCodePresence {
    #[serde(default)]
    pub foreground_session_id: Option<String>,
    /// Exact non-secret CLI connection options of this TUI; no credentials or prompt.
    #[serde(default)]
    pub connection_argv: Vec<String>,
    #[serde(default)]
    pub sessions: Vec<OpenCodeSessionPresence>,
    /// Transport/plugin errors do not finish server-side execution.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCodeSessionPresence {
    pub session_id: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub outcome: Option<AgentOutcome>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub forms: Vec<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

impl AgentReadout {
    /// Take what `newer` reports and keep the rest: one event rarely carries
    /// every field, and a missing one says nothing about its value.
    pub fn merge(&mut self, newer: &AgentReadout) {
        fn take<T: Clone>(slot: &mut Option<T>, newer: &Option<T>) {
            if newer.is_some() {
                slot.clone_from(newer);
            }
        }
        take(&mut self.permission_mode, &newer.permission_mode);
        take(&mut self.model, &newer.model);
        take(&mut self.effort, &newer.effort);
        take(&mut self.session_scope, &newer.session_scope);
        take(&mut self.opencode_presence, &newer.opencode_presence);
        take(&mut self.result, &newer.result);
    }
}

impl AgentStatus {
    pub fn dot_rgb(self) -> Option<u32> {
        match self {
            AgentStatus::Idle => None,
            AgentStatus::Working => Some(0x3B82F6),
            AgentStatus::Waiting => Some(0xF59E0B),
            AgentStatus::Done => Some(0x9CA3AF),
        }
    }
}

impl AgentSessionState {
    fn default_status() -> AgentStatus {
        AgentStatus::Idle
    }

    /// Whether authoritative events report a turn or human input in flight.
    pub fn mid_turn(&self) -> bool {
        matches!(self.status, AgentStatus::Working | AgentStatus::Waiting)
    }

    pub fn apply_event(&mut self, ev: &AgentEvent) {
        // A status set before the first hook came from a desktop notification
        // — Crush's "turn completed", say — which says the pane wants a look,
        // not that a question is open. Taken for one, the first tool report
        // turned it into a turn running, and nothing would ever end it.
        if !self.rich && self.status == AgentStatus::Waiting {
            self.status = AgentStatus::Idle;
            self.message = None;
        }
        self.rich = true;
        if ev.agent == Some(CLIAgent::OpenCode)
            && let Some(presence) = &ev.readout.opencode_presence
        {
            self.readout.merge(&ev.readout);
            self.session_id = presence.foreground_session_id.clone();
            if let Some(error) = &presence.error {
                self.message = Some(error.clone());
                return;
            }
            self.pending_requests = presence
                .sessions
                .iter()
                .flat_map(|session| {
                    session
                        .permissions
                        .iter()
                        .map(|id| AgentRequestUpdate {
                            id: format!("{}:{id}", session.session_id),
                            kind: AgentRequestKind::Permission,
                            resolved: false,
                        })
                        .chain(session.forms.iter().map(|id| AgentRequestUpdate {
                            id: format!("{}:{id}", session.session_id),
                            kind: AgentRequestKind::Question,
                            resolved: false,
                        }))
                })
                .collect();
            let waiting = presence
                .sessions
                .iter()
                .any(|s| !s.permissions.is_empty() || !s.forms.is_empty());
            let active = presence.sessions.iter().any(|s| s.active);
            let finished = presence.sessions.iter().any(|s| s.outcome.is_some());
            let next = if waiting {
                AgentStatus::Waiting
            } else if active {
                AgentStatus::Working
            } else if finished {
                AgentStatus::Done
            } else {
                AgentStatus::Idle
            };
            if next == AgentStatus::Done && self.status != AgentStatus::Done {
                self.turns = self.turns.wrapping_add(1);
            }
            self.status = next;
            self.readout.result = if waiting || active {
                None
            } else {
                presence
                    .sessions
                    .iter()
                    .filter_map(|s| {
                        s.outcome.map(|outcome| AgentResult {
                            outcome,
                            reason: s.reason.clone(),
                        })
                    })
                    .max_by_key(|r| match r.outcome {
                        AgentOutcome::Succeeded => 0,
                        AgentOutcome::Interrupted => 1,
                        AgentOutcome::Rejected => 2,
                        AgentOutcome::Failed => 3,
                    })
            };
            self.message = if active {
                presence
                    .sessions
                    .iter()
                    .filter(|s| s.active)
                    .find_map(|s| s.reason.clone())
            } else {
                self.readout.result.as_ref().and_then(|r| {
                    r.reason.clone().or_else(|| match r.outcome {
                        AgentOutcome::Succeeded => None,
                        AgentOutcome::Failed => Some("execution failed".into()),
                        AgentOutcome::Interrupted => Some("execution interrupted".into()),
                        AgentOutcome::Rejected => Some("permission rejected".into()),
                    })
                })
            };
            return;
        }
        if let Some(id) = &ev.session_id {
            if self.session_id.as_ref() != Some(id) {
                self.readout.session_scope = None;
            }
            self.session_id = Some(id.clone());
        }
        if let Some(cwd) = &ev.cwd {
            self.cwd = Some(cwd.clone());
        }
        if ev.kind == AgentEventKind::SessionStart {
            self.pending_requests.clear();
            self.pending_stop = false;
            self.request_resume_status = AgentStatus::Idle;
            self.readout.result = None;
        }
        if let Some(requests) = &ev.requests {
            if self.pending_requests.is_empty() && !requests.is_empty() {
                self.request_resume_status = self.status;
            }
            self.pending_requests = requests.iter().filter(|r| !r.resolved).cloned().collect();
            if self.pending_requests.is_empty() && self.status == AgentStatus::Waiting {
                self.status = self.request_resume_status;
                self.message = None;
            }
        }
        if let Some(request) = &ev.request {
            let existing = self
                .pending_requests
                .iter()
                .position(|r| r.id == request.id && r.kind == request.kind);
            if request.resolved {
                if let Some(index) = existing {
                    self.pending_requests.remove(index);
                }
                if self.pending_requests.is_empty() && self.status == AgentStatus::Waiting {
                    self.status = self.request_resume_status;
                    self.message = None;
                }
            } else {
                if self.pending_requests.is_empty() {
                    self.request_resume_status = self.status;
                }
                if existing.is_none() {
                    self.pending_requests.push(request.clone());
                }
            }
        }
        self.readout.merge(&ev.readout);
        match ev.kind {
            AgentEventKind::SessionStart => {
                self.status = AgentStatus::Idle;
                self.message = None;
            }
            AgentEventKind::PromptSubmit => {
                self.readout.result = None;
                self.pending_stop = false;
                self.status = AgentStatus::Working;
                self.message = None;
            }
            AgentEventKind::PermissionRequest | AgentEventKind::QuestionAsked => {
                if ev.request.is_none() {
                    if self.pending_requests.is_empty() {
                        self.request_resume_status = self.status;
                    }
                    self.pending_requests.push(AgentRequestUpdate {
                        id: format!("lyrel:unidentified:{}", self.pending_requests.len()),
                        kind: if ev.kind == AgentEventKind::PermissionRequest {
                            AgentRequestKind::Permission
                        } else {
                            AgentRequestKind::Question
                        },
                        resolved: false,
                    });
                }
                self.message = ev.message.clone();
            }
            AgentEventKind::Notification => {
                if ev.message.is_some() {
                    self.message = ev.message.clone();
                }
            }
            AgentEventKind::ToolComplete => {
                self.activity = self.activity.wrapping_add(1);
            }
            AgentEventKind::Stop => {
                if !self.pending_requests.is_empty() {
                    self.pending_stop = true;
                    self.status = AgentStatus::Waiting;
                    return;
                }
                if self.status != AgentStatus::Done {
                    self.turns = self.turns.wrapping_add(1);
                }
                self.status = AgentStatus::Done;
                self.message = ev
                    .message
                    .clone()
                    .or_else(|| self.readout.result.as_ref().and_then(|r| r.reason.clone()));
            }
            AgentEventKind::SessionEnd => {
                self.pending_requests.clear();
                self.pending_stop = false;
                self.request_resume_status = AgentStatus::Idle;
                self.status = AgentStatus::Idle;
                self.message = None;
                self.cwd = None;
            }
        }
        if self.pending_requests.is_empty() && std::mem::take(&mut self.pending_stop) {
            if self.status != AgentStatus::Done {
                self.turns = self.turns.wrapping_add(1);
            }
            self.status = AgentStatus::Done;
            self.message = self
                .readout
                .result
                .as_ref()
                .and_then(|result| result.reason.clone());
        }
        if !self.pending_requests.is_empty() {
            if self.status != AgentStatus::Waiting {
                self.request_resume_status = self.status;
            }
            self.status = AgentStatus::Waiting;
            if self
                .pending_requests
                .iter()
                .any(|r| r.id.starts_with("lyrel:unidentified:"))
            {
                self.message = Some("request identity unavailable; waiting cannot be correlated until the session changes".into());
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentEventKind {
    SessionStart,
    PromptSubmit,
    PermissionRequest,
    QuestionAsked,
    ToolComplete,
    Notification,
    Stop,
    SessionEnd,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentEvent {
    pub agent: Option<CLIAgent>,
    pub kind: AgentEventKind,
    pub session_id: Option<String>,
    pub message: Option<String>,
    pub cwd: Option<std::path::PathBuf>,
    /// What started the session, on a session start that says: `startup`,
    /// `resume`, `clear` (Codex's `/new`), `compact`.
    pub source: Option<String>,
    pub readout: AgentReadout,
    pub request: Option<AgentRequestUpdate>,
    pub requests: Option<Vec<AgentRequestUpdate>>,
}

pub fn parse_agent_event(payload: &[u8]) -> Option<AgentEvent> {
    let rest = payload.strip_prefix(b"777;notify;")?;
    let rest = rest.strip_prefix(AGENT_EVENT_SENTINEL.as_bytes())?;
    parse_agent_event_body(rest.strip_prefix(b";")?)
}

/// The JSON body of an agent event, without the OSC around it — what a hook
/// hands the daemon over its socket.
pub fn parse_agent_event_body(json: &[u8]) -> Option<AgentEvent> {
    #[derive(Deserialize)]
    struct Wire {
        #[serde(default)]
        #[allow(dead_code)]
        v: u32,
        #[serde(default)]
        agent: Option<String>,
        event: String,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        request: Option<AgentRequestUpdate>,
        #[serde(default)]
        requests: Option<Vec<AgentRequestUpdate>>,
        #[serde(flatten)]
        readout: AgentReadout,
    }

    let w: Wire = serde_json::from_slice(json).ok()?;
    if w.agent
        .as_deref()
        .is_some_and(|name| CLIAgent::from_slug(name).is_none())
    {
        return None;
    }
    let kind = serde_json::from_value::<AgentEventKind>(serde_json::Value::String(w.event)).ok()?;
    let nonempty = |s: Option<String>| s.filter(|s| !s.trim().is_empty());
    Some(AgentEvent {
        agent: w.agent.as_deref().and_then(CLIAgent::from_slug),
        kind,
        session_id: nonempty(w.session_id),
        message: nonempty(w.message),
        cwd: nonempty(w.cwd).map(std::path::PathBuf::from),
        source: nonempty(w.source),
        request: w.request.filter(|r| !r.id.trim().is_empty()),
        requests: w.requests,
        readout: w.readout,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_final_turn_waits_for_plan_review_then_keeps_its_actual_result() {
        let mut state = AgentSessionState::default();
        for body in [
            r#"{"agent":"omp","event":"prompt-submit"}"#,
            r#"{"agent":"omp","event":"question-asked","request":{"id":"review","kind":"plan-review"}}"#,
            r#"{"agent":"omp","event":"stop","result":{"outcome":"succeeded"}}"#,
        ] {
            state.apply_event(&parse_agent_event_body(body.as_bytes()).unwrap());
        }
        assert_eq!(state.status, AgentStatus::Waiting);
        assert_eq!(state.turns, 0);
        state.apply_event(&parse_agent_event_body(br#"{"agent":"omp","event":"tool-complete","request":{"id":"review","kind":"plan-review","resolved":true}}"#).unwrap());
        assert_eq!(state.status, AgentStatus::Done);
        assert_eq!(state.turns, 1);
        assert_eq!(
            state.readout.result.as_ref().unwrap().outcome,
            AgentOutcome::Succeeded
        );
    }
    #[test]
    fn authoritative_request_snapshot_can_finish_a_previously_waiting_turn() {
        let mut state = AgentSessionState::default();
        for body in [
            r#"{"agent":"kimi","event":"prompt-submit"}"#,
            r#"{"agent":"kimi","event":"permission-request","request":{"id":"approval","kind":"permission"}}"#,
            r#"{"agent":"kimi","event":"stop","requests":[],"result":{"outcome":"failed","reason":"provider failed"}}"#,
        ] {
            state.apply_event(&parse_agent_event_body(body.as_bytes()).unwrap());
        }
        assert_eq!(state.status, AgentStatus::Done);
        assert!(state.pending_requests.is_empty());
        assert_eq!(
            state.readout.result.as_ref().unwrap().outcome,
            AgentOutcome::Failed
        );
    }

    #[test]
    fn resolving_an_idle_or_finished_dialog_restores_its_previous_status() {
        for before in [AgentStatus::Idle, AgentStatus::Done] {
            let mut state = AgentSessionState {
                status: before,
                rich: true,
                ..Default::default()
            };
            for body in [
                r#"{"agent":"pi","event":"question-asked","request":{"id":"dialog","kind":"question"}}"#,
                r#"{"agent":"pi","event":"tool-complete","request":{"id":"dialog","kind":"question","resolved":true}}"#,
            ] {
                state.apply_event(&parse_agent_event_body(body.as_bytes()).unwrap());
            }
            assert_eq!(state.status, before);
            assert_eq!(
                state.turns, 0,
                "closing a dialog is not a completed model turn"
            );
        }
    }
    #[test]
    fn retired_names_and_hook_reports_do_not_claim_a_terminal() {
        for name in ["claude", "codex", "gemini", "kimi-code", "muse-bin-1.0"] {
            assert_eq!(
                CLIAgent::detect_from_command_with(name, &HashMap::new()),
                None
            );
        }
        assert!(
            parse_agent_event_body(br#"{"agent":"claude","event":"stop","session_id":"old"}"#)
                .is_none()
        );
        assert!(parse_agent_event_body(br#"{"agent":"future-agent","event":"stop"}"#).is_none());
        assert_eq!(
            CLIAgent::detect_from_command_with("omp", &HashMap::new()),
            Some(CLIAgent::OhMyPi)
        );
        assert_eq!(
            CLIAgent::detect_from_command_with("pi", &HashMap::new()),
            Some(CLIAgent::Pi)
        );
    }

    #[test]
    fn replying_to_one_request_does_not_release_another_or_finish_the_turn() {
        let mut state = AgentSessionState::default();
        let event = |value: serde_json::Value| {
            parse_agent_event_body(value.to_string().as_bytes()).unwrap()
        };
        state.apply_event(&event(
            serde_json::json!({"agent":"omp","event":"prompt-submit","session_id":"main"}),
        ));
        for id in ["one", "two"] {
            state.apply_event(&event(serde_json::json!({"agent":"omp","event":"permission-request","request":{"id":id,"kind":"permission"}})));
        }
        state.apply_event(&event(serde_json::json!({"agent":"omp","event":"tool-complete","request":{"id":"one","kind":"permission","resolved":true}})));
        assert_eq!(state.status, AgentStatus::Waiting);
        assert_eq!(state.pending_requests[0].id, "two");
        state.apply_event(&event(
            serde_json::json!({"agent":"omp","event":"tool-complete"}),
        ));
        state.apply_event(&event(serde_json::json!({"agent":"omp","event":"stop"})));
        assert_eq!(
            state.status,
            AgentStatus::Waiting,
            "a stop cannot erase unresolved human input"
        );
        assert_eq!(state.turns, 0);
        state.apply_event(&event(serde_json::json!({"agent":"omp","event":"tool-complete","request":{"id":"two","kind":"permission","resolved":true}})));
        assert_eq!(
            state.status,
            AgentStatus::Done,
            "the real final boundary was retained while input remained pending"
        );
        state.apply_event(&event(serde_json::json!({"agent":"omp","event":"stop","result":{"outcome":"interrupted","reason":"operator cancelled"}})));
        assert_eq!(state.status, AgentStatus::Done);
        assert_eq!(
            state.readout.result.as_ref().unwrap().outcome,
            AgentOutcome::Interrupted
        );
    }

    #[test]
    fn switching_sessions_discards_old_requests_and_results() {
        let mut state = AgentSessionState::default();
        for body in [
            r#"{"agent":"omp","event":"session-start","session_id":"old"}"#,
            r#"{"agent":"omp","event":"question-asked","request":{"id":"review","kind":"plan-review"}}"#,
            r#"{"agent":"omp","event":"session-start","session_id":"new"}"#,
        ] {
            state.apply_event(&parse_agent_event_body(body.as_bytes()).unwrap());
        }
        assert_eq!(state.session_id.as_deref(), Some("new"));
        assert_eq!(state.status, AgentStatus::Idle);
        assert!(state.pending_requests.is_empty());
        assert!(state.readout.result.is_none());
    }

    #[test]
    fn a_session_scope_survives_sparse_events_but_not_a_different_identity() {
        let mut state = AgentSessionState::default();
        let first = parse_agent_event_body(br#"{"agent":"omp","event":"session-start","session_id":"one","session_scope":{"session_file":"/store/one.jsonl","environment":{"HOME":"/isolated-home"}}}"#).unwrap();
        state.apply_event(&first);
        let sparse = parse_agent_event_body(
            br#"{"agent":"omp","event":"tool-complete","session_id":"one"}"#,
        )
        .unwrap();
        state.apply_event(&sparse);
        assert_eq!(
            state
                .readout
                .session_scope
                .as_ref()
                .and_then(|scope| scope.session_file.as_deref()),
            Some(std::path::Path::new("/store/one.jsonl"))
        );
        let other = parse_agent_event_body(
            br#"{"agent":"omp","event":"session-start","session_id":"two"}"#,
        )
        .unwrap();
        state.apply_event(&other);
        assert!(state.readout.session_scope.is_none());
    }

    #[test]
    fn opencode_tabs_aggregate_without_replacing_the_foreground_identity() {
        let mut state = AgentSessionState::default();
        let snapshot = |active: bool, permissions: &[&str], outcome: &str| {
            parse_agent_event_body(
                serde_json::json!({
                    "agent": "opencode", "event": "tool-complete",
                    "opencode_presence": {
                        "foreground_session_id": "front",
                        "connection_argv": ["--server", "https://example.test"],
                        "sessions": [
                            {"session_id": "front", "active": false},
                            {"session_id": "background", "active": active,
                             "permissions": permissions, "outcome": outcome}
                        ]
                    }
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap()
        };
        state.apply_event(&snapshot(true, &["approval-a", "approval-b"], "succeeded"));
        assert_eq!(state.session_id.as_deref(), Some("front"));
        assert_eq!(state.status, AgentStatus::Waiting);
        state.apply_event(&snapshot(true, &["approval-b"], "succeeded"));
        assert_eq!(
            state.status,
            AgentStatus::Waiting,
            "one reply must not clear another request"
        );
        state.apply_event(&snapshot(true, &[], "succeeded"));
        assert_eq!(
            state.status,
            AgentStatus::Working,
            "old outcome cannot end active work"
        );
        state.apply_event(&snapshot(false, &[], "failed"));
        assert_eq!(state.status, AgentStatus::Done);
        assert_eq!(
            state.readout.result.as_ref().unwrap().outcome,
            AgentOutcome::Failed
        );
    }

    #[test]
    fn opencode_reconnect_restores_requests_and_transport_loss_does_not_finish_work() {
        let snapshot = |presence| {
            parse_agent_event_body(
                serde_json::json!({
                    "agent": "opencode", "event": "tool-complete", "opencode_presence": presence
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap()
        };
        let mut first = AgentSessionState::default();
        let mut second = AgentSessionState::default();
        let active = snapshot(serde_json::json!({
            "foreground_session_id": "shared", "sessions": [
                {"session_id": "shared", "active": true, "outcome": "succeeded", "forms": ["question-one", "question-two"]}
            ]
        }));
        first.apply_event(&active);
        second.apply_event(&active);
        assert_eq!(first.status, AgentStatus::Waiting);
        assert_eq!(
            second.status,
            AgentStatus::Waiting,
            "a shared session has multiple views, not one owner"
        );
        first.apply_event(&snapshot(serde_json::json!({
            "foreground_session_id": "shared", "error": "connection lost"
        })));
        assert_eq!(first.status, AgentStatus::Waiting);
        assert_eq!(first.readout.result, None);
        first.apply_event(&snapshot(serde_json::json!({
            "foreground_session_id": "forked", "sessions": [
                {"session_id": "forked", "active": true},
                {"session_id": "shared", "active": false, "outcome": "interrupted"}
            ]
        })));
        assert_eq!(first.session_id.as_deref(), Some("forked"));
        assert_eq!(first.status, AgentStatus::Working);
        assert_eq!(second.session_id.as_deref(), Some("shared"));
        assert_eq!(second.status, AgentStatus::Waiting);
    }

    /// A later event that leaves a field out keeps what an earlier one said.
    #[test]
    fn the_readout_keeps_what_later_events_leave_out() {
        let mut state = AgentSessionState::default();
        let event = |readout: AgentReadout| AgentEvent {
            agent: Some(CLIAgent::OhMyPi),
            kind: AgentEventKind::ToolComplete,
            session_id: None,
            message: None,
            cwd: None,
            source: None,
            request: None,
            requests: None,
            readout,
        };
        state.apply_event(&event(AgentReadout {
            permission_mode: Some("plan".into()),
            model: Some("omp-opus-5-5".into()),
            ..Default::default()
        }));
        state.apply_event(&event(AgentReadout {
            effort: Some("high".into()),
            ..Default::default()
        }));
        assert_eq!(state.readout.permission_mode.as_deref(), Some("plan"));
        assert_eq!(state.readout.model.as_deref(), Some("omp-opus-5-5"));
        assert_eq!(state.readout.effort.as_deref(), Some("high"));
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn detects_native_binaries() {
        for (command, agent) in [
            ("omp", CLIAgent::OhMyPi),
            ("kimi", CLIAgent::Kimi),
            ("pi", CLIAgent::Pi),
            ("opencode", CLIAgent::OpenCode),
            ("hermes", CLIAgent::Hermes),
        ] {
            assert_eq!(CLIAgent::detect_from_argv(&argv(&[command])), Some(agent));
        }
    }

    #[test]
    fn strips_leading_env_assignments() {
        assert_eq!(
            CLIAgent::detect_from_argv(&argv(&["FOO=1", "BAR=baz", "omp"])),
            Some(CLIAgent::OhMyPi)
        );
    }

    #[test]
    fn detects_node_wrapped_pi_by_package_dir() {
        assert_eq!(
            CLIAgent::detect_from_argv(&argv(&[
                "node",
                "/usr/lib/node_modules/@earendil-works/pi-coding-agent/dist/cli.js"
            ])),
            Some(CLIAgent::Pi)
        );
    }

    #[test]
    fn detects_npx_package_form() {
        assert_eq!(
            CLIAgent::detect_from_argv(&argv(&["npx", "@earendil-works/pi-coding-agent"])),
            Some(CLIAgent::Pi)
        );
        assert_eq!(
            CLIAgent::detect_from_argv(&argv(&["npx", "@oh-my-pi/pi-coding-agent"])),
            Some(CLIAgent::OhMyPi)
        );
    }

    #[test]
    fn non_interpreter_does_not_match_on_arguments() {
        assert_eq!(CLIAgent::detect_from_argv(&argv(&["cat", "pi.md"])), None);
        assert_eq!(
            CLIAgent::detect_from_argv(&argv(&["vim", "pi-coding-agent/notes.txt"])),
            None
        );
        assert_eq!(CLIAgent::detect_from_argv(&argv(&["less", "aider"])), None);
    }

    #[test]
    fn unrelated_commands_are_none() {
        assert_eq!(CLIAgent::detect_from_argv(&argv(&["zsh"])), None);
        assert_eq!(
            CLIAgent::detect_from_argv(&argv(&["node", "server.js"])),
            None
        );
        assert_eq!(CLIAgent::detect_from_argv(&argv(&[])), None);
    }

    #[test]
    fn every_agent_has_metadata() {
        for a in CLIAgent::ALL {
            assert!(!a.display_name().is_empty());
            assert!(!a.aliases().is_empty());
            assert!(a.accent_rgb() <= 0xFFFFFF);
            assert_eq!(CLIAgent::from_slug(a.slug()), Some(a));
        }
    }

    #[test]
    fn only_the_unbranded_agents_use_the_fallback_glyph() {
        assert_eq!(CLIAgent::Hermes.icon_path(), "icons/bot.svg");
        for agent in [
            CLIAgent::OhMyPi,
            CLIAgent::Kimi,
            CLIAgent::Pi,
            CLIAgent::OpenCode,
        ] {
            assert_ne!(agent.icon_path(), "icons/bot.svg");
        }
    }

    #[test]
    fn detects_supported_agents_by_command() {
        for (cmd, agent) in [
            ("/usr/bin/omp", CLIAgent::OhMyPi),
            ("/usr/bin/kimi", CLIAgent::Kimi),
            ("pi", CLIAgent::Pi),
            ("hermes", CLIAgent::Hermes),
            ("opencode.exe", CLIAgent::OpenCode),
        ] {
            assert_eq!(CLIAgent::detect_from_argv(&argv(&[cmd])), Some(agent));
        }
    }

    /// Oh My Pi is a Pi fork, but it is its own binary with its own config
    /// directory and its own flags — one must never be detected as the other.
    #[test]
    fn oh_my_pi_and_pi_stay_distinct() {
        assert_eq!(
            CLIAgent::detect_from_argv(&argv(&["omp"])),
            Some(CLIAgent::OhMyPi)
        );
        assert_eq!(
            CLIAgent::detect_from_argv(&argv(&["pi"])),
            Some(CLIAgent::Pi)
        );
        assert_eq!(CLIAgent::from_slug("omp"), Some(CLIAgent::OhMyPi));
        assert_eq!(CLIAgent::from_slug("oh-my-pi"), None);
        assert_ne!(CLIAgent::Pi.icon_path(), CLIAgent::OhMyPi.icon_path());
    }

    #[test]
    fn custom_rules_map_wrappers_to_agents() {
        let custom: HashMap<String, String> = [("cc".to_string(), "omp".to_string())].into();
        assert_eq!(
            CLIAgent::detect_from_argv_with(&argv(&["/home/x/bin/cc", "-c"]), &custom),
            Some(CLIAgent::OhMyPi)
        );
        let bogus: HashMap<String, String> = [("cc".to_string(), "hal9000".to_string())].into();
        assert_eq!(
            CLIAgent::detect_from_argv_with(&argv(&["cc"]), &bogus),
            None
        );
        assert_eq!(
            CLIAgent::detect_from_argv_with(&argv(&["node", "cc/cli.js"]), &custom),
            None
        );
        let shadow: HashMap<String, String> = [("pi".to_string(), "omp".to_string())].into();
        assert_eq!(
            CLIAgent::detect_from_argv_with(&argv(&["pi"]), &shadow),
            Some(CLIAgent::Pi)
        );
    }

    #[test]
    fn detects_from_typed_command_lines() {
        let none = HashMap::new();
        assert_eq!(
            CLIAgent::detect_from_command_with("omp --resume abc", &none),
            Some(CLIAgent::OhMyPi)
        );
        assert_eq!(
            CLIAgent::detect_from_command_with("omp.exe", &none),
            Some(CLIAgent::OhMyPi)
        );
        assert_eq!(
            CLIAgent::detect_from_command_with(
                r"C:\Users\x\AppData\Roaming\npm\omp.cmd --model opus",
                &none
            ),
            Some(CLIAgent::OhMyPi)
        );
        assert_eq!(
            CLIAgent::detect_from_command_with("OMP", &none),
            Some(CLIAgent::OhMyPi)
        );
        assert_eq!(
            CLIAgent::detect_from_command_with(r#"& "C:\tools\pi.exe""#, &none),
            Some(CLIAgent::Pi)
        );
        assert_eq!(
            CLIAgent::detect_from_command_with(
                r"node C:\x\node_modules\@oh-my-pi\pi-coding-agent\cli.js",
                &none
            ),
            Some(CLIAgent::OhMyPi)
        );
        assert_eq!(
            CLIAgent::detect_from_command_with("npx.cmd kimi", &none),
            Some(CLIAgent::Kimi)
        );
        assert_eq!(
            CLIAgent::detect_from_command_with("notepad omp.txt", &none),
            None
        );
        assert_eq!(CLIAgent::detect_from_command_with("cat pi.md", &none), None);
        assert_eq!(CLIAgent::detect_from_command_with("", &none), None);
        let custom: HashMap<String, String> = [("cc".to_string(), "omp".to_string())].into();
        assert_eq!(
            CLIAgent::detect_from_command_with("cc -c", &custom),
            Some(CLIAgent::OhMyPi)
        );
    }

    #[test]
    fn command_argv_preserves_case_for_flag_replay() {
        assert_eq!(
            command_argv("omp --dangerously-skip-permissions"),
            ["omp", "--dangerously-skip-permissions"]
        );
        assert_eq!(
            command_argv("omp --model Opus --resume Abc-123"),
            ["omp", "--model", "Opus", "--resume", "Abc-123"]
        );
        assert_eq!(
            command_argv(r#"& "C:\Tools\omp.exe" --continue"#),
            [r"C:\Tools\omp.exe", "--continue"]
        );
        assert_eq!(
            command_argv(
                r#"& "C:\Program Files\工具\omp.exe" --profile "work project" --session-dir 'C:\会话 store'"#
            ),
            [
                r"C:\Program Files\工具\omp.exe",
                "--profile",
                "work project",
                "--session-dir",
                r"C:\会话 store"
            ]
        );
        assert!(parse_command_argv("omp --model 'unfinished").is_err());
        assert!(parse_command_argv("omp; unexpected-command").is_err());
        assert_eq!(command_argv("  "), [""; 0]);
    }

    #[test]
    fn parses_sentinel_events() {
        let ev = parse_agent_event(
            br#"777;notify;lyrel://cli-agent;{"v":1,"agent":"omp","event":"permission-request","session_id":"abc-123","message":"Claude needs your permission to use Bash"}"#,
        )
        .expect("well-formed sentinel event");
        assert_eq!(ev.agent, Some(CLIAgent::OhMyPi));
        assert_eq!(ev.kind, AgentEventKind::PermissionRequest);
        assert_eq!(ev.session_id.as_deref(), Some("abc-123"));
        assert!(ev.message.as_deref().unwrap().contains("permission"));

        assert_eq!(parse_agent_event(b"777;notify;Build;done"), None);
        assert_eq!(
            parse_agent_event(br#"777;notify;lyrel://cli-agent;{"event":"quantum-leap"}"#),
            None
        );
        assert_eq!(
            parse_agent_event(b"777;notify;lyrel://cli-agent;{oops"),
            None
        );
    }

    /// A desktop notification before any hook marks the pane waiting; the
    /// first hook report must not read that as a question being answered.
    #[test]
    fn a_hook_after_a_notification_does_not_start_a_turn() {
        let mut s = AgentSessionState {
            status: AgentStatus::Waiting,
            message: Some("Agent's turn completed".into()),
            ..Default::default()
        };
        s.apply_event(&AgentEvent {
            agent: Some(CLIAgent::Pi),
            kind: AgentEventKind::ToolComplete,
            session_id: Some("sid".into()),
            message: None,
            cwd: None,
            source: None,
            request: None,
            requests: None,
            readout: Default::default(),
        });
        assert_eq!(s.status, AgentStatus::Idle);
        assert_eq!(s.message, None);
        assert!(s.rich);
        assert_eq!(s.activity, 1);
    }

    #[test]
    fn tool_completions_count_even_when_the_status_holds_still() {
        let ev = |kind| AgentEvent {
            agent: Some(CLIAgent::OhMyPi),
            kind,
            session_id: None,
            message: None,
            cwd: None,
            source: None,
            request: None,
            requests: None,
            readout: Default::default(),
        };

        let mut s = AgentSessionState::default();
        s.apply_event(&ev(AgentEventKind::PromptSubmit));
        assert_eq!(s.activity, 0, "a turn starting is not tool activity");

        for n in 1..=3 {
            s.apply_event(&ev(AgentEventKind::ToolComplete));
            assert_eq!(s.status, AgentStatus::Working, "the status holds still…");
            assert_eq!(s.activity, n, "…while the counter is what moves");
        }

        s.apply_event(&ev(AgentEventKind::Stop));
        s.apply_event(&ev(AgentEventKind::ToolComplete));
        assert_eq!(
            s.status,
            AgentStatus::Done,
            "and still doesn't resurrect the turn"
        );
        assert_eq!(s.activity, 4);

        s.apply_event(&ev(AgentEventKind::SessionEnd));
        assert_eq!(s.activity, 4);
    }

    #[test]
    fn an_unidentified_request_stays_waiting_with_a_diagnostic() {
        let mut state = AgentSessionState::default();
        for body in [
            r#"{"agent":"omp","event":"prompt-submit"}"#,
            r#"{"agent":"omp","event":"permission-request"}"#,
            r#"{"agent":"omp","event":"tool-complete"}"#,
            r#"{"agent":"omp","event":"stop"}"#,
        ] {
            state.apply_event(&parse_agent_event_body(body.as_bytes()).unwrap());
        }
        assert_eq!(state.status, AgentStatus::Waiting);
        assert!(
            state
                .message
                .as_deref()
                .unwrap()
                .contains("request identity unavailable")
        );
        assert_eq!(state.turns, 0);
    }

    #[test]
    fn each_finished_turn_is_counted_once() {
        let ev = |kind| AgentEvent {
            agent: Some(CLIAgent::OhMyPi),
            kind,
            session_id: None,
            message: None,
            cwd: None,
            source: None,
            request: None,
            requests: None,
            readout: Default::default(),
        };

        let mut s = AgentSessionState::default();
        s.apply_event(&ev(AgentEventKind::PromptSubmit));
        assert_eq!(s.turns, 0, "a turn starting has not finished anything");

        s.apply_event(&ev(AgentEventKind::Stop));
        assert_eq!(s.turns, 1);
        s.apply_event(&ev(AgentEventKind::Stop));
        assert_eq!(s.turns, 1, "a repeated stop is the same turn");
        s.apply_event(&ev(AgentEventKind::Notification));
        assert_eq!(s.turns, 1);

        s.apply_event(&ev(AgentEventKind::PromptSubmit));
        s.apply_event(&ev(AgentEventKind::Stop));
        assert_eq!(s.turns, 2, "a second turn is a second count");

        s.apply_event(&ev(AgentEventKind::SessionEnd));
        assert_eq!(s.turns, 2);
    }

    #[test]
    fn session_state_tracks_and_releases_the_agent_cwd() {
        use std::path::PathBuf;

        let ev = |kind, cwd: Option<&str>| AgentEvent {
            agent: Some(CLIAgent::OhMyPi),
            kind,
            session_id: None,
            message: None,
            cwd: cwd.map(PathBuf::from),
            source: None,
            request: None,
            requests: None,
            readout: Default::default(),
        };

        let mut s = AgentSessionState::default();
        s.apply_event(&ev(AgentEventKind::SessionStart, Some("/repo")));
        assert_eq!(s.cwd.as_deref(), Some(std::path::Path::new("/repo")));

        s.apply_event(&ev(
            AgentEventKind::ToolComplete,
            Some("/repo/.omp/worktrees/fix-x"),
        ));
        assert_eq!(
            s.cwd.as_deref(),
            Some(std::path::Path::new("/repo/.omp/worktrees/fix-x"))
        );

        s.apply_event(&ev(AgentEventKind::Stop, None));
        assert_eq!(
            s.cwd.as_deref(),
            Some(std::path::Path::new("/repo/.omp/worktrees/fix-x"))
        );

        s.apply_event(&ev(AgentEventKind::SessionEnd, None));
        assert_eq!(s.cwd, None, "session end releases the cwd claim");
    }

    #[test]
    fn status_metadata_is_consistent() {
        assert_eq!(AgentStatus::Idle.dot_rgb(), None);
        for st in [
            AgentStatus::Working,
            AgentStatus::Waiting,
            AgentStatus::Done,
        ] {
            assert!(st.dot_rgb().is_some());
        }
        assert_eq!(
            serde_json::to_string(&AgentStatus::Waiting).unwrap(),
            "\"waiting\""
        );
    }

    fn overrides(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_launch_is_the_bare_binary_unless_overridden() {
        let overrides = overrides(&[
            ("omp", "  omp --model custom "),
            ("pi", "  "),
            ("claude", "do-not-run"),
        ]);
        assert_eq!(
            CLIAgent::OhMyPi.launch_command(&overrides),
            "omp --model custom"
        );
        assert_eq!(CLIAgent::Pi.launch_command(&overrides), "pi");
        assert!(!launch_aliases(&overrides).contains_key("do-not-run"));
    }

    #[test]
    fn every_agent_launches_as_a_name_it_is_detected_by() {
        let none = HashMap::new();
        for agent in CLIAgent::ALL {
            let argv = command_argv(&agent.launch_command(&none));
            assert_eq!(
                CLIAgent::detect_from_argv(&argv),
                Some(agent),
                "{agent:?} would launch as something detection does not know"
            );
        }
    }

    #[test]
    fn a_wrapper_named_by_a_launch_override_is_detected_as_its_agent() {
        let aliases = launch_aliases(&overrides(&[("omp", "FOO=1 cc --fast")]));
        // Run as a binary, and run as a script under its interpreter.
        for run in [
            argv(&["cc", "--fast"]),
            argv(&["/Users/me/bin/cc", "--fast"]),
            argv(&["/bin/bash", "/Users/me/bin/cc", "--fast"]),
            argv(&["node", "/Users/me/bin/cc.js", "--fast"]),
        ] {
            assert_eq!(
                CLIAgent::detect_from_argv_with(&run, &aliases),
                Some(CLIAgent::OhMyPi),
                "{run:?}"
            );
        }
        // Without the override the same wrapper is nobody.
        assert_eq!(
            CLIAgent::detect_from_argv_with(&argv(&["cc", "--fast"]), &HashMap::new()),
            None
        );
    }

    #[test]
    fn a_launch_override_never_maps_an_interpreter_or_a_real_agent_name() {
        let aliases = launch_aliases(&overrides(&[
            ("omp", "npx @earendil-works/pi-coding-agent"),
            ("pi", "bash -lc pi"),
            // Already detected as itself; mapping it would add nothing.
            ("kimi", "kimi --yolo"),
            ("not-an-agent", "cc"),
        ]));
        assert!(aliases.is_empty(), "{aliases:?}");
        // A shell running something else is still nobody.
        assert_eq!(
            CLIAgent::detect_from_argv_with(
                &argv(&["bash", "./build.sh"]),
                &launch_aliases(&overrides(&[("omp", "cc")]))
            ),
            None
        );
    }

    #[test]
    fn pi_default_launch_never_remembers_a_literal_initial_task_as_flags() {
        assert_eq!(
            CLIAgent::Pi.launch_argv_for_default(&argv(&[
                "pi",
                "--model",
                "provider/model",
                "--",
                "--help\n中文"
            ])),
            Some(argv(&["pi", "--model", "provider/model"]))
        );
        assert_eq!(
            CLIAgent::Pi.launch_argv_for_default(&argv(&[
                "pi",
                "--no-extensions",
                "old initial task"
            ])),
            Some(argv(&["pi", "--no-extensions"]))
        );
    }

    #[test]
    fn launch_args_drop_the_session_and_the_prompt_but_keep_the_flags() {
        assert_eq!(
            CLIAgent::OhMyPi.launch_argv_for_default(&argv(&[
                "omp", "--resume", "old", "--model", "custom", "prompt"
            ])),
            Some(argv(&["omp", "--model", "custom"]))
        );
        assert_eq!(
            CLIAgent::Pi.launch_argv_for_default(&argv(&["wrapper", "--fast"])),
            Some(argv(&["wrapper", "--fast"]))
        );
        assert_eq!(CLIAgent::Pi.launch_argv_for_default(&[]), None);
    }

    #[test]
    fn program_on_path_finds_only_executables() {
        let dir = tempfile::TempDir::new().unwrap();
        let bin = dir.path().join("omp");
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::write(dir.path().join("pi"), "not executable").unwrap();
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(!program_on_path("pi", dir.path().as_os_str()));
        }
        std::fs::create_dir(dir.path().join("kimi")).unwrap();
        let path =
            std::env::join_paths([std::path::Path::new("/nonexistent"), dir.path()]).unwrap();
        assert!(program_on_path("omp", &path));
        assert!(
            !program_on_path("kimi", &path),
            "a directory is not a program"
        );
        assert!(!program_on_path("aider", &path));
        assert!(program_on_path(
            bin.to_str().unwrap(),
            std::ffi::OsStr::new("")
        ));
        assert!(!program_on_path("", &path));
    }
}
