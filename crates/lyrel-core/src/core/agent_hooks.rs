mod pi;

use std::io;
use std::io::{IsTerminal as _, Read as _};
use std::path::{Path, PathBuf};

use crate::core::cli_agent::{AGENT_EVENT_SENTINEL, CLIAgent};
use crate::host::Host;

mod hermes;
#[cfg(test)]
mod hermes_tests;
mod launch_scope;
mod omp;

pub const LYREL_ENV_MARKER: &str = "LYREL";

const MAX_STDIN: u64 = 64 * 1024;

pub fn run_agent_hook(agent: &str, event: &str) {
    detach_console();
    if std::env::var_os(LYREL_ENV_MARKER).is_none() || CLIAgent::from_slug(agent).is_none() {
        return;
    }
    let mut input = String::new();
    if !std::io::stdin().is_terminal() {
        let _ = std::io::stdin().take(MAX_STDIN).read_to_string(&mut input);
    }
    let Some(event) = effective_event(agent, event, &input) else {
        return;
    };
    let payload = build_hook_payload(agent, event, &input);
    let requests = payload.get("request").is_some() || payload.get("requests").is_some();
    let results = payload.get("result").is_some() || payload.get("opencode_presence").is_some();
    if requests || results {
        let version = crate::client::PaneClient::local().version();
        for (required, feature) in [
            (requests, crate::daemon::protocol::FEATURE_AGENT_REQUESTS),
            (results, crate::daemon::protocol::FEATURE_AGENT_RESULTS),
        ] {
            if required
                && !version
                    .as_ref()
                    .is_ok_and(|version| version.has_feature(feature))
            {
                eprintln!("lyrel: daemon does not support {feature}; update/reconnect the daemon");
                std::process::exit(2);
            }
        }
    }
    let body = payload.to_string();
    if !report_to_daemon(&body) {
        write_to_controlling_tty(&hook_sequence(&body));
    }
}

/// Hand the report to the daemon that owns this pane, over its socket.
///
/// The tty is the channel of last resort. The agent draws on it too, and a
/// tty write is not atomic — on macOS the kernel parks a writer mid-write
/// whenever the output queue is full, which is exactly when an agent is busy
/// redrawing — so the agent's output can land inside the sequence, break it,
/// and leave the rest of it on screen as text. The socket has no such
/// neighbour. It only works where the daemon runs, though: a hook the
/// environment points at no daemon, at a daemon that predates this message,
/// or at a pane it does not run in (see `DaemonPane::report_agent_event`)
/// falls back to the tty.
fn report_to_daemon(body: &str) -> bool {
    let Some(pane) = std::env::var(crate::daemon::pane::LYREL_PANE_ENV)
        .ok()
        .and_then(|p| p.trim().parse::<u64>().ok())
    else {
        return false;
    };
    crate::client::PaneClient::local()
        .report_agent_event(pane, std::process::id(), body)
        .is_ok()
}

#[cfg(not(unix))]
fn detach_console() {
    use windows_sys::Win32::System::Console::FreeConsole;
    unsafe {
        FreeConsole();
    }
}

#[cfg(unix)]
fn detach_console() {}

fn effective_event<'a>(agent: &str, event: &'a str, stdin_json: &str) -> Option<&'a str> {
    if agent == "kimi" {
        let payload: serde_json::Value = serde_json::from_str(stdin_json).ok()?;
        if payload
            .get("agent_id")
            .and_then(|value| value.as_str())
            .is_some_and(|id| id != "main")
        {
            return None;
        }
        // 2.2.0 omits agent_id from TurnStarted. Only the main durable wire is authoritative.
        if event == "turn-started" {
            return (payload["agent_id"] == "main").then_some("tool-complete");
        }
        if event == "tool-start" {
            return (payload["tool_name"] == "AskUserQuestion"
                && payload["tool_input"]["background"] != true)
                .then_some("question-asked");
        }
    }
    Some(event)
}

#[cfg(test)]
fn build_hook_sequence(agent: &str, event: &str, stdin_json: &str) -> Vec<u8> {
    hook_sequence(&build_hook_body(agent, event, stdin_json))
}

/// The report as it goes out on the tty: the body wrapped in the OSC 777
/// notification the daemon's reader looks for.
fn hook_sequence(body: &str) -> Vec<u8> {
    format!("\x1b]777;notify;{AGENT_EVENT_SENTINEL};{body}\x07").into_bytes()
}

/// The report itself, as JSON — see `cli_agent::parse_agent_event_body`.
#[cfg(test)]
fn build_hook_body(agent: &str, event: &str, stdin_json: &str) -> String {
    build_hook_payload(agent, event, stdin_json).to_string()
}

fn build_hook_payload(agent: &str, event: &str, stdin_json: &str) -> serde_json::Value {
    let payload: serde_json::Value =
        serde_json::from_str(stdin_json).unwrap_or(serde_json::json!({}));
    let mut body = serde_json::json!({"v": 1, "agent": agent, "event": event});
    for key in ["session_id", "message", "cwd", "source"] {
        if let Some(value) = payload
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
        {
            body[key] = value.into();
        }
    }
    if let Some(message) = body.get_mut("message")
        && let Some(text) = message.as_str()
    {
        *message = clip_chars(text, MESSAGE_MAX).into();
    }
    if agent == "kimi" {
        kimi_readout(event, &payload, &mut body);
    }
    if matches!(agent, "omp" | "pi" | "opencode" | "hermes")
        && let Some(value) = payload.get("session_scope")
        && let Ok(mut scope) =
            serde_json::from_value::<crate::host::session_operations::SessionScope>(value.clone())
    {
        scope
            .environment
            .retain(|key, _| crate::host::session_operations::SCOPE_ENV.contains(&key.as_str()));
        if let Ok(value) = serde_json::to_value(scope) {
            body["session_scope"] = value;
        }
    }
    if agent == "opencode"
        && let Some(presence) = payload.get("opencode_presence")
    {
        body["opencode_presence"] = presence.clone();
    }
    if let Some(result) = payload.get("result")
        && serde_json::from_value::<crate::core::cli_agent::AgentResult>(result.clone()).is_ok()
    {
        body["result"] = result.clone();
    }
    if let Some(request) = payload.get("request")
        && serde_json::from_value::<crate::core::cli_agent::AgentRequestUpdate>(request.clone())
            .is_ok()
    {
        body["request"] = request.clone();
    }
    if let Some(requests) = payload.get("requests")
        && serde_json::from_value::<Vec<crate::core::cli_agent::AgentRequestUpdate>>(
            requests.clone(),
        )
        .is_ok()
    {
        body["requests"] = requests.clone();
    }
    if agent == "omp" {
        omp::normalize(event, &payload, &mut body);
    }
    body
}

fn kimi_readout(event: &str, payload: &serde_json::Value, body: &mut serde_json::Value) {
    let text = |key: &str| {
        payload
            .get(key)
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
    };
    let request = match event {
        "permission-request" => text("id").map(|id| (id, "permission", false)),
        "permission-result" => text("id").map(|id| (id, "permission", true)),
        "task-started" if payload["kind"] == "question" => {
            text("task_id").map(|id| (id, "question", false))
        }
        "task-notification"
            if payload["source_kind"] == "background_task"
                && matches!(
                    text("notification_type"),
                    Some(
                        "task.completed"
                            | "task.failed"
                            | "task.killed"
                            | "task.timed_out"
                            | "task.lost"
                    )
                ) =>
        {
            text("source_id").map(|id| (id, "question", true))
        }
        "tool-start" | "question-asked"
            if payload["tool_name"] == "AskUserQuestion"
                && payload["tool_input"]["background"] != true =>
        {
            text("tool_call_id").map(|id| (id, "question", false))
        }
        "tool-complete" | "tool-failed"
            if payload["tool_name"] == "AskUserQuestion"
                && payload["tool_input"]["background"] != true =>
        {
            text("tool_call_id").map(|id| (id, "question", true))
        }
        _ => None,
    };
    body["event"] = match request {
        Some((_, "permission", false)) => "permission-request",
        Some((_, "question", false)) => "question-asked",
        _ if matches!(event, "session-start" | "session-end" | "prompt-submit") => event,
        // Stop is before veto/continuation, and terminal hooks may belong to a
        // child with no agent_id. Only the main durable turn.ended is final.
        _ => "tool-complete",
    }
    .into();
    if let Some((id, kind, resolved)) = request {
        body["request"] = serde_json::json!({"id":id,"kind":kind,"resolved":resolved});
    }
    if let Some(message) = text("error_message").or_else(|| payload["error"]["message"].as_str()) {
        body["message"] = clip_chars(message, MESSAGE_MAX).into();
    }
    if let Some(model) = text("model") {
        body["model"] = model.into();
    }
    if event == "session-start" {
        let environment = crate::host::local_agent_environment();
        let home = environment
            .get("KIMI_CODE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                environment
                    .get(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                    .map(|value| PathBuf::from(value).join(".kimi-code"))
            })
            .and_then(|home| {
                if home.is_absolute() {
                    Some(home)
                } else {
                    std::env::current_dir().ok().map(|cwd| cwd.join(home))
                }
            });
        if let Some(home) = home {
            let file = text("session_id")
                .filter(|id| !id.contains(['/', '\\']) && !id.starts_with('.'))
                .and_then(|id| {
                    std::fs::read_dir(home.join("sessions"))
                        .ok()?
                        .filter_map(Result::ok)
                        .map(|project| project.path().join(id).join("state.json"))
                        .find(|file| file.is_file())
                });
            body["session_scope"] = serde_json::json!({"profile":text("profile"),"session_file":file,"environment":{"KIMI_CODE_HOME":home}});
        }
    }
}

/// How much of an agent's message rides back to the terminal. It only ever
/// fills a status line, and every byte of the sequence is a byte another
/// writer on the same tty can cut in on.
const MESSAGE_MAX: usize = 200;

fn clip_chars(text: &str, max: usize) -> &str {
    text.char_indices()
        .nth(max)
        .map_or(text, |(i, _)| &text[..i])
}

#[cfg(unix)]
fn write_to_controlling_tty(bytes: &[u8]) -> bool {
    if write_dev(std::path::Path::new("/dev/tty"), bytes) {
        return true;
    }
    if let Some(dev) = ancestor_tty_device() {
        return write_dev(&dev, bytes);
    }
    false
}

/// Write `bytes` to the terminal device at `path`.
///
/// The device is opened without waiting. A blocking open of a terminal whose
/// other end has gone — the pane closed while the agent was still running its
/// session-end hook — waits for a carrier that never comes, and on macOS it
/// waits holding the lock every lookup under `/dev` needs: the hook hangs, the
/// agent it belongs to cannot finish exiting, and every new terminal, `ps`
/// and `tty` on the machine hangs behind them. Once open, the device goes back
/// to blocking writes, so a full output queue delays the sequence rather than
/// cutting it short; a terminal with no other end fails the write instead.
#[cfg(unix)]
fn write_dev(path: &std::path::Path, bytes: &[u8]) -> bool {
    use std::io::Write as _;
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let Ok(mut tty) = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
    else {
        return false;
    };
    let fd = tty.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return false;
    }
    tty.write_all(bytes).and_then(|_| tty.flush()).is_ok()
}

#[cfg(unix)]
fn ancestor_tty_device() -> Option<std::path::PathBuf> {
    use std::process::Command;
    let mut pid = unsafe { libc::getppid() };
    for _ in 0..8 {
        if pid <= 1 {
            break;
        }
        let out = Command::new("ps")
            .args(["-o", "tty=", "-o", "ppid=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let line = String::from_utf8_lossy(&out.stdout);
        let mut fields = line.split_whitespace();
        let tty = fields.next().unwrap_or("");
        let ppid: i32 = fields.next().and_then(|s| s.parse().ok()).unwrap_or(1);
        if !tty.is_empty() && tty != "??" && tty != "?" {
            return Some(std::path::PathBuf::from(format!("/dev/{tty}")));
        }
        pid = ppid;
    }
    None
}

#[cfg(not(unix))]
fn write_to_controlling_tty(bytes: &[u8]) -> bool {
    let procs = crate::daemon::winproc::snapshot();
    let ancestors = ancestor_pids(&procs);

    let name_of = |pid: u32| {
        procs
            .iter()
            .find(|p| p.pid == pid)
            .map(|p| p.name.to_ascii_lowercase())
    };
    let shell = ancestors.iter().copied().find(|&pid| {
        procs
            .iter()
            .find(|p| p.pid == pid)
            .and_then(|p| name_of(p.parent))
            .is_some_and(|n| is_lyrel_host_exe(&n))
    });

    if let Some(pid) = shell {
        if attach_and_write(pid, bytes) {
            return true;
        }
    }

    let mut any = false;
    for pid in ancestors {
        any |= attach_and_write(pid, bytes);
    }
    any
}

#[cfg(any(not(unix), test))]
fn is_lyrel_host_exe(name: &str) -> bool {
    matches!(name, "lyrel-app.exe" | "lyrel-server.exe" | "lyrel.exe")
        || (name.ends_with(".exe")
            && crate::daemon::install::asset::dialect_from_path(name).is_some())
}

#[cfg(not(unix))]
fn attach_and_write(pid: u32, bytes: &[u8]) -> bool {
    use windows_sys::Win32::System::Console::{AttachConsole, FreeConsole};
    unsafe {
        FreeConsole();
        if AttachConsole(pid) == 0 {
            return false;
        }
    }
    let ok = write_conout(bytes);
    unsafe {
        FreeConsole();
    }
    ok
}

#[cfg(not(unix))]
fn write_conout(bytes: &[u8]) -> bool {
    use std::io::Write as _;
    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")
    {
        Ok(mut out) => out.write_all(bytes).and_then(|_| out.flush()).is_ok(),
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn ancestor_pids(procs: &[crate::daemon::winproc::Proc]) -> Vec<u32> {
    let parent_of = |pid: u32| procs.iter().find(|p| p.pid == pid).map(|p| p.parent);
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut cur = std::process::id();
    seen.insert(cur);
    for _ in 0..16 {
        match parent_of(cur) {
            Some(parent) if parent != 0 && seen.insert(parent) => {
                out.push(parent);
                cur = parent;
            }
            _ => break,
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HookAgent {
    OpenCode,
    Pi,
    OhMyPi,
    Kimi,
    Hermes,
}

impl HookAgent {
    pub const ALL: [HookAgent; 5] = [
        HookAgent::OhMyPi,
        HookAgent::Kimi,
        HookAgent::Pi,
        HookAgent::OpenCode,
        HookAgent::Hermes,
    ];

    /// The hooks behind a detected agent process, if it has any.
    ///
    /// Process detection knows far more agents than hooks do — the ones with no
    /// arm here report status some other way, or not at all. The match is
    /// exhaustive on purpose: a newly detected agent has to say which it is.
    pub fn of_detected(agent: CLIAgent) -> Option<HookAgent> {
        match agent {
            CLIAgent::OhMyPi => Some(HookAgent::OhMyPi),
            CLIAgent::Kimi => Some(HookAgent::Kimi),
            CLIAgent::Pi => Some(HookAgent::Pi),
            CLIAgent::OpenCode => Some(HookAgent::OpenCode),
            CLIAgent::Hermes => Some(HookAgent::Hermes),
        }
    }

    /// The events this agent's hooks merge into a shared TOML config, if that
    /// is how it takes them — the third strategy, for the agents whose hooks
    /// live as `[[hooks]]` entries in a config file the user also hand-edits.
    fn toml_hook_events(self) -> Option<&'static [(&'static str, &'static str)]> {
        match self {
            HookAgent::Kimi => Some(KIMI_HOOK_EVENTS),
            _ => None,
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            HookAgent::Hermes => "hermes",
            HookAgent::OpenCode => "opencode",
            HookAgent::Pi => "pi",
            HookAgent::OhMyPi => "omp",
            HookAgent::Kimi => "kimi",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            HookAgent::Hermes => "Hermes",
            HookAgent::OpenCode => "OpenCode",
            HookAgent::Pi => "Pi",
            HookAgent::OhMyPi => "Oh My Pi",
            HookAgent::Kimi => "Kimi Code",
        }
    }

    pub fn target_display(self, target: &HookTarget) -> String {
        match self.target_path(target) {
            Ok(path) => target.abbreviate_home(&path),
            Err(error) => format!("scope unavailable: {error}"),
        }
    }

    fn target_path(self, target: &HookTarget) -> io::Result<PathBuf> {
        Ok(match self {
            HookAgent::Hermes => {
                return hermes::home(target)
                    .map(|home| target.under(&home, &["plugins", "lyrel", "__init__.py"]));
            }
            HookAgent::OpenCode => target.under(
                &target.opencode_config_dir_checked()?,
                &["plugins", "lyrel", "tui.ts"],
            ),
            HookAgent::Pi => return target.pi_extension_path_checked(),
            HookAgent::OhMyPi => return omp::extension_path(target),
            HookAgent::Kimi => return target.kimi_config_path_checked(),
        })
    }

    fn marker(self) -> String {
        format!("agent-hook {}", self.slug())
    }
}

const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

pub struct HookTarget<'a> {
    host: &'a dyn Host,
    home: PathBuf,
    exe: PathBuf,
    launch_scopes:
        std::collections::HashMap<HookAgent, Result<launch_scope::ConfiguredScope, String>>,
}

impl<'a> HookTarget<'a> {
    pub(crate) fn hermes_home(&self) -> io::Result<PathBuf> {
        hermes::home(self)
    }
    pub fn local(host: &'a dyn Host) -> Option<HookTarget<'a>> {
        Self::local_for_exe(host, std::env::current_exe().ok()?)
    }

    /// Build a local target for a known lyrel hook runner.
    ///
    /// Most callers use [`Self::local`]. The standalone CLI is the exception:
    /// it diagnoses hooks but never executes them, so it supplies the
    /// `lyrel-app` it would launch instead of comparing configs to `lyrel`.
    pub fn local_for_exe(host: &'a dyn Host, exe: PathBuf) -> Option<HookTarget<'a>> {
        Some(HookTarget {
            host,
            home: home_dir()?,
            exe,
            launch_scopes: Default::default(),
        })
    }

    pub fn remote(host: &'a dyn Host, home: PathBuf) -> HookTarget<'a> {
        use crate::daemon::install::asset;
        let dialect = crate::daemon::install::RemoteProtocol::of_this_build();
        let home_str = home.to_string_lossy();
        // A Windows server reports its home natively (`C:\Users\me`), and the
        // hook command runs there, so the binary is named the way that
        // machine's installer put it — and spelled natively again.
        let binary = match asset::sftp_path_from_windows(&home_str) {
            Some(sftp_home) => {
                let installed = asset::remote_paths_on(
                    asset::RemotePlatform::Windows,
                    &sftp_home,
                    dialect.control,
                    dialect.protocol,
                )
                .binary;
                asset::windows_native_path(&installed).unwrap_or(installed)
            }
            None => asset::remote_paths(&home_str, dialect.control, dialect.protocol).binary,
        };
        HookTarget {
            host,
            home,
            exe: PathBuf::from(binary),
            launch_scopes: Default::default(),
        }
    }

    fn is_local(&self) -> bool {
        self.host.id().is_local()
    }

    fn under(&self, base: &Path, parts: &[&str]) -> PathBuf {
        let mut p = base.to_path_buf();
        for part in parts {
            p = self.host.join(&p, part);
        }
        p
    }

    fn pi_extension_path_checked(&self) -> io::Result<PathBuf> {
        Ok(self.under(
            &self.pi_agent_dir_checked()?,
            &["extensions", "lyrel", "index.ts"],
        ))
    }

    pub(crate) fn pi_agent_dir_checked(&self) -> io::Result<PathBuf> {
        let environment = self.agent_environment(HookAgent::Pi)?;
        let home = self.agent_home(&environment)?;
        let root = match environment
            .get("PI_CODING_AGENT_DIR")
            .filter(|value| !value.is_empty())
        {
            Some(value) if value == "~" => home,
            Some(value) if value.starts_with("~/") || value.starts_with("~\\") => {
                self.host.join(&home, &value[2..])
            }
            Some(value) => {
                let path = PathBuf::from(value);
                if !self.host.is_absolute(&path) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Pi extension management requires an absolute PI_CODING_AGENT_DIR on the selected Host",
                    ));
                }
                path
            }
            None => self.under(&home, &[".pi", "agent"]),
        };
        Ok(root)
    }

    fn opencode_config_dir_checked(&self) -> io::Result<PathBuf> {
        let environment = self.agent_environment(HookAgent::OpenCode)?;
        let home = self.agent_home(&environment)?;
        let base = match environment
            .get("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
        {
            Some(value) => {
                let path = PathBuf::from(value);
                if !self.host.is_absolute(&path) {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "OpenCode CLI integration requires an absolute XDG_CONFIG_HOME on the selected Host",
                    ));
                }
                path
            }
            None => self.under(&home, &[".config"]),
        };
        Ok(self.under(&base, &["opencode"]))
    }

    pub(crate) fn kimi_config_path_checked(&self) -> io::Result<PathBuf> {
        let environment = self.agent_environment(HookAgent::Kimi)?;
        if let Some(home) = environment.get("KIMI_CODE_HOME") {
            let home = Path::new(home);
            if !self.host.is_absolute(home) {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "relative KIMI_CODE_HOME requires the native pane working directory; cannot manage it from daemon defaults",
                ));
            }
            return Ok(self.host.join(home, "config.toml"));
        }
        Ok(self.under(
            &self.agent_home(&environment)?,
            &[".kimi-code", "config.toml"],
        ))
    }

    fn hook_command(&self, agent: HookAgent, event: &str) -> String {
        if let Some(exe) = self.hook_command_exe() {
            return format!("{exe} agent-hook {} {event}", agent.slug());
        }
        format!(
            "\"{}\" agent-hook {} {event}",
            self.exe.display(),
            agent.slug()
        )
    }

    /// A shell-safe executable path for generated hook commands.
    ///
    /// On Windows, Codex runs hook commands through the session's shell, which
    /// is frequently PowerShell. `pwsh -Command` drops the quotes around a
    /// path containing spaces, so the usual `"C:\Program Files\..."` form is
    /// parsed as `C:\Program` and fails — and even a quoted path without
    /// spaces is a syntax error in PowerShell (invoking a quoted path requires
    /// the `&` call operator). When the executable resolves by its bare file
    /// name from PATH, we can emit the name without any quoting, which every
    /// shell (`cmd.exe`, PowerShell, bash) executes correctly.
    ///
    /// Returns `None` when the executable is not resolvable by name from PATH,
    /// or when the first PATH match is a different binary; callers then fall
    /// back to the quoted full path.
    fn hook_command_exe(&self) -> Option<String> {
        #[cfg(windows)]
        if self.is_local() {
            if let Some(name) = path_resolvable_name(&self.exe) {
                return Some(name);
            }
        }
        None
    }

    fn read(&self, p: &Path) -> io::Result<String> {
        let bytes = self.host.read_file(p, MAX_CONFIG_BYTES)?;
        String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn write(&self, p: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        if let Some(parent) = p.parent() {
            self.host.create_dir(parent, true)?;
        }
        if self.is_local() {
            crate::core::config::write_atomic(p, bytes)?;
        } else {
            self.host.write_file(p, bytes)?;
        }
        Ok(())
    }

    fn abbreviate_home(&self, path: &Path) -> String {
        match path.strip_prefix(&self.home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HooksState {
    NotInstalled,
    Installed,
    Outdated,
}

pub fn hooks_state(target: &HookTarget, agent: HookAgent) -> HooksState {
    if agent == HookAgent::Hermes {
        return hermes::state(target);
    }
    let Ok(path) = agent.target_path(target) else {
        return HooksState::NotInstalled;
    };
    if let Some(events) = agent.toml_hook_events() {
        return toml_hooks_state(target, &path, agent, events);
    }
    if agent == HookAgent::OpenCode
        && target
            .opencode_config_dir_checked()
            .and_then(|config| target.read(&target.under(&config, &["plugins", "lyrel.js"])))
            .is_ok_and(|c| owned_file_is_marked(&c, &agent.marker()))
    {
        return HooksState::Outdated;
    }
    let Some(expected) = owned_file_content(target, agent) else {
        return HooksState::NotInstalled;
    };
    owned_file_state(target, &path, &expected, &agent.marker())
}

/// Configuration-only diagnostics; the GUI owns localized presentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HooksDiagnostic {
    PiExtension,
    OmpExtension,
    KimiHistory,
    HermesObserver { home: PathBuf },
    ScopeUnavailable { error: String },
    OpenCodeUnreadable { path: PathBuf },
    OpenCodeRuntimeDiagnosis { path: PathBuf },
    OpenCodeDisabled { path: PathBuf, pattern: String },
}

pub fn hooks_diagnostic(target: &HookTarget, agent: HookAgent) -> Option<HooksDiagnostic> {
    if let Err(error) = agent.target_path(target) {
        return Some(HooksDiagnostic::ScopeUnavailable {
            error: error.to_string(),
        });
    }
    match agent {
        HookAgent::Pi => Some(HooksDiagnostic::PiExtension),
        HookAgent::OhMyPi => Some(HooksDiagnostic::OmpExtension),
        HookAgent::Kimi => Some(HooksDiagnostic::KimiHistory),
        HookAgent::Hermes => Some(match target.hermes_home() {
            Ok(home) => HooksDiagnostic::HermesObserver { home },
            Err(error) => HooksDiagnostic::ScopeUnavailable {
                error: error.to_string(),
            },
        }),
        HookAgent::OpenCode => opencode_plugin_diagnostic(target),
    }
}

fn opencode_plugin_diagnostic(target: &HookTarget) -> Option<HooksDiagnostic> {
    let config = match target.opencode_config_dir_checked() {
        Ok(config) => config,
        Err(error) => {
            return Some(HooksDiagnostic::ScopeUnavailable {
                error: error.to_string(),
            });
        }
    };
    for name in ["cli.json", "cli.jsonc"] {
        let path = target.under(&config, &[name]);
        let contents = match target.read(&path) {
            Ok(contents) => contents,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return Some(HooksDiagnostic::OpenCodeUnreadable { path }),
        };
        let root: serde_json::Value = match serde_json::from_str(&contents) {
            Ok(root) => root,
            Err(_) => return Some(HooksDiagnostic::OpenCodeRuntimeDiagnosis { path }),
        };
        if let Some(entries) = root.get("plugins").and_then(|v| v.as_array()) {
            for entry in entries.iter().filter_map(|v| v.as_str()) {
                let Some(pattern) = entry.strip_prefix('-') else {
                    continue;
                };
                // OpenCode CLI plugin IDs support wildcard exclusions.
                let mut rest = "lyrel";
                let mut matches = true;
                let mut parts = pattern.split('*').peekable();
                let mut first = true;
                while let Some(part) = parts.next() {
                    if !part.is_empty() {
                        match rest.find(part) {
                            Some(pos) if !first || pos == 0 => rest = &rest[pos + part.len()..],
                            _ => {
                                matches = false;
                                break;
                            }
                        }
                    }
                    if parts.peek().is_none() && !pattern.ends_with('*') && !rest.is_empty() {
                        matches = false;
                    }
                    first = false;
                }
                if matches {
                    return Some(HooksDiagnostic::OpenCodeDisabled {
                        path,
                        pattern: entry.into(),
                    });
                }
            }
        }
    }
    None
}

fn opencode_legacy_path(target: &HookTarget) -> io::Result<PathBuf> {
    Ok(target.under(
        &target.opencode_config_dir_checked()?,
        &["plugins", "lyrel.js"],
    ))
}

/// What an install or uninstall actually did.
///
/// These land in a note the user reads in Settings, and this crate cannot
/// reach `src/ui/i18n`, which owns every user-visible string. Returning a
/// sentence from here meant a Chinese or Japanese UI reported "Installed" in
/// English; returning the outcome lets the caller word it in the user's
/// language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    Installed,
    Removed,
    NothingInstalled,
    NoLyrelHooks,
}

pub fn install_hooks(target: &HookTarget, agent: HookAgent) -> anyhow::Result<HookOutcome> {
    if agent == HookAgent::Hermes {
        return hermes::install(target);
    }
    if agent == HookAgent::OpenCode
        && let Some(HooksDiagnostic::OpenCodeDisabled { path, pattern }) =
            opencode_plugin_diagnostic(target)
    {
        anyhow::bail!(
            "OpenCode CLI plugin lyrel is disabled by {pattern} in {}; authorization is unchanged",
            path.display()
        );
    }
    let path = agent.target_path(target)?;
    if let Some(events) = agent.toml_hook_events() {
        toml_hooks_install(target, &path, agent, events)?;
    } else {
        let content = owned_file_content(target, agent)
            .ok_or_else(|| anyhow::anyhow!("{agent:?} has no owned file"))?;
        owned_file_install(target, &path, &content, &agent.marker())?;
    }
    if agent == HookAgent::OpenCode {
        let legacy = opencode_legacy_path(target)?;
        if target
            .read(&legacy)
            .is_ok_and(|c| owned_file_is_marked(&c, &agent.marker()))
        {
            owned_file_uninstall(target, &legacy, &agent.marker())?;
        }
    }
    Ok(HookOutcome::Installed)
}

pub fn uninstall_hooks(target: &HookTarget, agent: HookAgent) -> anyhow::Result<HookOutcome> {
    if agent == HookAgent::Hermes {
        return hermes::uninstall(target);
    }
    let path = agent.target_path(target)?;
    if agent == HookAgent::OpenCode {
        let current = owned_file_uninstall(target, &path, &agent.marker())?;
        let legacy = opencode_legacy_path(target)?;
        if target
            .read(&legacy)
            .is_ok_and(|c| owned_file_is_marked(&c, &agent.marker()))
        {
            return owned_file_uninstall(target, &legacy, &agent.marker());
        }
        return Ok(current);
    }
    if agent.toml_hook_events().is_some() {
        return toml_hooks_uninstall(target, &path, agent);
    }
    owned_file_uninstall(target, &path, &agent.marker())
}

pub fn refresh_hooks(target: &HookTarget) -> usize {
    let mut refreshed = 0;
    for agent in HookAgent::ALL {
        if hooks_state(target, agent) != HooksState::Outdated {
            continue;
        }
        match install_hooks(target, agent) {
            Ok(summary) => {
                refreshed += 1;
                log::info!(
                    "refreshed stale {} hooks at {}: {summary:?}",
                    agent.display_name(),
                    agent.target_display(target)
                );
            }
            Err(e) => log::warn!(
                "could not refresh stale {} hooks: {e}",
                agent.display_name()
            ),
        }
    }
    refreshed
}

pub fn refresh_remote_hooks(
    host: &dyn Host,
    home: PathBuf,
    commands: &std::collections::HashMap<String, String>,
) -> usize {
    refresh_hooks(&HookTarget::remote(host, home).with_launch_commands(commands))
}

pub fn refresh_hooks_at_launch(commands: &std::collections::HashMap<String, String>) -> usize {
    if cfg!(debug_assertions) {
        return 0;
    }
    let host = crate::host::local::LocalHost::new();
    let Some(target) = HookTarget::local(&*host) else {
        return 0;
    };
    refresh_hooks(&target.with_launch_commands(commands))
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
    #[cfg(not(unix))]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
}

/// Official Kimi Code 2.2.0 TOML hooks. Stop is a blockable pre-final event;
/// main-agent durable turn.ended, not a hook name or silence, settles the turn.
const KIMI_HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "prompt-submit"),
    ("TurnStarted", "turn-started"),
    ("PreToolUse", "tool-start"),
    ("PermissionRequest", "permission-request"),
    ("PermissionResult", "permission-result"),
    ("TaskStarted", "task-started"),
    ("Notification", "task-notification"),
    ("PostToolUse", "tool-complete"),
    ("PostToolUseFailure", "tool-failed"),
    ("Stop", "stop"),
    ("Interrupt", "interrupted"),
    ("StopFailure", "turn-failed"),
    ("SessionEnd", "session-end"),
];

/// Recognize the runner token, not a brand substring in a directory or in
/// arguments. Old products use the same `agent-hook` subcommand, so the
/// subcommand alone is never evidence that an entry belongs to lyrel.
fn lyrel_hook_command(command: &str, marker: &str) -> bool {
    let command = command.trim_start();
    let (exe, args) = if let Some(quote @ ('\'' | '"')) = command.chars().next() {
        let rest = &command[quote.len_utf8()..];
        let Some(end) = rest.find(quote) else {
            return false;
        };
        (&rest[..end], &rest[end + quote.len_utf8()..])
    } else {
        let Some(end) = command.find(char::is_whitespace) else {
            return false;
        };
        (&command[..end], &command[end..])
    };
    if !args.starts_with(char::is_whitespace) {
        return false;
    }
    let Some(event) = args.trim_start().strip_prefix(marker) else {
        return false;
    };
    if !event.starts_with(char::is_whitespace) {
        return false;
    }
    let name = exe.rsplit(['/', '\\']).next().unwrap_or(exe);
    let name = name.strip_suffix(".exe").unwrap_or(name);
    if matches!(name, "lyrel" | "lyrel-app" | "lyrel-server")
        || crate::daemon::install::asset::dialect_from_path(name).is_some()
    {
        return true;
    }
    // A runner may itself have been explicitly renamed. Its own exact path is
    // authoritative; a path merely containing `lyrel` is not. Cache the OS
    // lookup, since shared configurations can hold many hook entries.
    static RUNNER: std::sync::LazyLock<Option<PathBuf>> =
        std::sync::LazyLock::new(|| std::env::current_exe().ok());
    RUNNER.as_deref() == Some(Path::new(exe))
}

fn toml_hooks_state(
    target: &HookTarget,
    path: &Path,
    agent: HookAgent,
    events: &[(&str, &str)],
) -> HooksState {
    let Ok(text) = target.read(path) else {
        return HooksState::NotInstalled;
    };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
        return HooksState::NotInstalled;
    };
    let marker = agent.marker();
    let marked: Vec<&toml_edit::Table> = doc
        .get("hooks")
        .and_then(|h| h.as_array_of_tables())
        .into_iter()
        .flatten()
        .filter(|entry| toml_command_is_marked(entry, &marker))
        .collect();
    if marked.is_empty() {
        return HooksState::NotInstalled;
    }
    // Every marked entry counts towards the total, `event` or no `event`: a
    // hand-edit that drops the key leaves an entry that is ours and is broken,
    // which is exactly what Outdated means. Reporting NotInstalled instead
    // would hide it from `refresh_hooks`, which only ever revisits Outdated.
    let complete = marked.len() == events.len()
        && events.iter().all(|(hook_event, lyrel_event)| {
            let command = target.hook_command(agent, lyrel_event);
            marked.iter().any(|entry| {
                entry.get("event").and_then(|e| e.as_str()) == Some(*hook_event)
                    && entry.get("command").and_then(|c| c.as_str()) == Some(command.as_str())
            })
        });
    if complete {
        HooksState::Installed
    } else {
        HooksState::Outdated
    }
}

fn toml_hooks_install(
    target: &HookTarget,
    path: &Path,
    agent: HookAgent,
    events: &[(&str, &str)],
) -> anyhow::Result<()> {
    let mut doc: toml_edit::DocumentMut = match target.read(path) {
        Ok(text) => text.parse().map_err(|e| {
            anyhow::anyhow!(
                "{} is not valid TOML ({e}); not touching it",
                path.display()
            )
        })?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => toml_edit::DocumentMut::new(),
        Err(e) => return Err(anyhow::anyhow!("read {}: {e}", path.display())),
    };

    // `hooks = []` and no `hooks` key at all say the same thing, but toml_edit
    // keeps an empty inline array and an array of tables apart. Promote the
    // one to the other rather than refusing over a difference that carries no
    // configuration and that the user cannot see.
    if doc
        .get("hooks")
        .and_then(|h| h.as_array())
        .is_some_and(|a| a.is_empty())
    {
        doc.remove("hooks");
    }

    let hooks = doc.entry("hooks").or_insert(toml_edit::Item::ArrayOfTables(
        toml_edit::ArrayOfTables::new(),
    ));
    let Some(list) = hooks.as_array_of_tables_mut() else {
        return Err(anyhow::anyhow!(
            "\"hooks\" in {} is not an array of tables; not touching it",
            path.display()
        ));
    };

    let marker = agent.marker();
    list.retain(|entry| !toml_command_is_marked(entry, &marker));
    for (hook_event, lyrel_event) in events {
        let mut entry = toml_edit::Table::new();
        entry["event"] = toml_edit::value(*hook_event);
        entry["command"] = toml_edit::value(target.hook_command(agent, lyrel_event));
        list.push(entry);
    }

    target.write(path, doc.to_string().as_bytes())
}

fn toml_hooks_uninstall(
    target: &HookTarget,
    path: &Path,
    agent: HookAgent,
) -> anyhow::Result<HookOutcome> {
    let text = match target.read(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(HookOutcome::NothingInstalled);
        }
        Err(e) => return Err(anyhow::anyhow!("read {}: {e}", path.display())),
    };
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| {
        anyhow::anyhow!(
            "{} is not valid TOML ({e}); not touching it",
            path.display()
        )
    })?;

    let marker = agent.marker();
    let mut removed = 0;
    if let Some(list) = doc
        .get_mut("hooks")
        .and_then(|h| h.as_array_of_tables_mut())
    {
        let before = list.len();
        list.retain(|entry| !toml_command_is_marked(entry, &marker));
        removed = before - list.len();
        if list.is_empty() {
            doc.remove("hooks");
        }
    }
    if removed == 0 {
        return Ok(HookOutcome::NoLyrelHooks);
    }
    target.write(path, doc.to_string().as_bytes())?;
    Ok(HookOutcome::Removed)
}

fn toml_command_is_marked(entry: &toml_edit::Table, marker: &str) -> bool {
    entry
        .get("command")
        .and_then(|c| c.as_str())
        .is_some_and(|c| lyrel_hook_command(c, marker))
}

/// Returns the bare file name of `exe` when resolving that name from PATH
/// yields the same binary. Returns `None` when the name does not resolve, or
/// when an earlier PATH entry contains a different file with the same name
/// (a bare-name command would then invoke the wrong binary).
#[cfg(windows)]
fn path_resolvable_name(exe: &Path) -> Option<String> {
    let name = exe.file_name()?.to_str()?;
    let path_var = std::env::var_os("PATH")?;
    let exe_canonical = std::fs::canonicalize(exe).ok()?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        let Ok(candidate_canonical) = std::fs::canonicalize(&candidate) else {
            continue;
        };
        if same_windows_path(&candidate_canonical, &exe_canonical) {
            return Some(name.to_string());
        }
        return None;
    }
    None
}

#[cfg(windows)]
fn same_windows_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

fn owned_file_content(target: &HookTarget, agent: HookAgent) -> Option<String> {
    match agent {
        HookAgent::Hermes => Some(hermes::plugin(target)),
        HookAgent::OpenCode => opencode_plugin_js(target),
        HookAgent::Pi => pi::extension(target),
        HookAgent::OhMyPi => omp::extension(target),
        HookAgent::Kimi => None,
    }
}

fn owned_file_is_marked(contents: &str, marker: &str) -> bool {
    // Generated bridges declare their owner in the header, independently of
    // the path they invoke (which can become outdated after an upgrade).
    if contents
        .lines()
        .next()
        .and_then(|line| {
            line.strip_prefix("// lyrel ")
                .or_else(|| line.strip_prefix("/* lyrel "))
                .or_else(|| line.strip_prefix("# lyrel "))
        })
        .is_some_and(|header| {
            header
                .strip_prefix(marker)
                .is_some_and(|rest| rest.starts_with(char::is_whitespace))
        })
    {
        return true;
    }
    false
}

fn owned_file_state(target: &HookTarget, path: &Path, expected: &str, marker: &str) -> HooksState {
    let Ok(contents) = target.read(path) else {
        return HooksState::NotInstalled;
    };
    if contents == expected {
        HooksState::Installed
    } else if owned_file_is_marked(&contents, marker) {
        HooksState::Outdated
    } else {
        HooksState::NotInstalled
    }
}

fn owned_file_install(
    target: &HookTarget,
    path: &Path,
    content: &str,
    marker: &str,
) -> anyhow::Result<()> {
    if let Ok(existing) = target.read(path)
        && !owned_file_is_marked(&existing, marker)
    {
        return Err(anyhow::anyhow!(
            "{} exists but wasn't written by lyrel; not touching it",
            path.display()
        ));
    }
    target.write(path, content.as_bytes())
}

fn owned_file_uninstall(
    target: &HookTarget,
    path: &Path,
    marker: &str,
) -> anyhow::Result<HookOutcome> {
    let contents = match target.read(path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(HookOutcome::NothingInstalled);
        }
        Err(e) => return Err(anyhow::anyhow!("read {}: {e}", path.display())),
    };
    if !owned_file_is_marked(&contents, marker) {
        return Err(anyhow::anyhow!(
            "{} wasn't written by lyrel; not touching it",
            path.display()
        ));
    }
    target.host.remove(path, false)?;
    // Take the directories lyrel generated with it, innermost first, stopping at
    // the one named after lyrel. `remove` is not recursive, so a directory still
    // holding someone else's file simply survives the attempt. Goose nests one
    // level deeper than the rest (`.../lyrel/hooks/hooks.json`), which is why
    // this walks rather than checking a single parent.
    let mut dir = path.parent();
    while let Some(d) = dir {
        if d.file_name().is_some_and(|n| n == "lyrel") {
            let _ = target.host.remove(d, false);
            break;
        }
        if !d
            .parent()
            .and_then(|p| p.file_name())
            .is_some_and(|n| n == "lyrel")
        {
            break;
        }
        let _ = target.host.remove(d, false);
        dir = d.parent();
    }
    Ok(HookOutcome::Removed)
}

/// OpenCode 2.x CLI-only plugin: loaded in each TUI, not the shared server.
fn opencode_plugin_js(target: &HookTarget) -> Option<String> {
    let exe = serde_json::to_string(&target.exe.display().to_string()).ok()?;
    Some(include_str!("opencode_tui.ts").replace("__EXE__", &exe))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_bridges_share_public_scope_keys_without_cross_agent_policies() {
        let host = crate::host::local::LocalHost::new();
        let target = HookTarget::local_for_exe(&*host, std::env::current_exe().unwrap()).unwrap();
        for (agent, expected) in [
            (
                HookAgent::Pi,
                vec![
                    "HOME",
                    "USERPROFILE",
                    "PI_CODING_AGENT_DIR",
                    "PI_CODING_AGENT_SESSION_DIR",
                    "PI_IMAGE_PROTOCOL",
                ],
            ),
            (
                HookAgent::OhMyPi,
                vec![
                    "HOME",
                    "USERPROFILE",
                    "OMP_PROFILE",
                    "PI_PROFILE",
                    "PI_CONFIG_DIR",
                    "PI_CODING_AGENT_DIR",
                    "PI_CONFIG_FILES",
                    "XDG_CONFIG_HOME",
                    "XDG_DATA_HOME",
                    "XDG_STATE_HOME",
                    "XDG_CACHE_HOME",
                    "PI_FORCE_IMAGE_PROTOCOL",
                    "PI_KITTY_PLACEHOLDERS",
                    "PI_NO_KITTY_PLACEHOLDERS",
                ],
            ),
        ] {
            let source = owned_file_content(&target, agent).unwrap();
            let declaration = source
                .lines()
                .find_map(|line| line.strip_prefix("const SCOPE_ENV = "))
                .unwrap();
            let mut actual: Vec<String> =
                serde_json::from_str(declaration.trim_end_matches(';')).unwrap();
            let mut expected = expected;
            actual.sort();
            expected.sort();
            assert_eq!(actual, expected, "{} bridge scope contract", agent.slug());
            assert!(
                actual
                    .iter()
                    .all(|key| crate::host::session_operations::SCOPE_ENV.contains(&key.as_str()))
            );
            assert!(
                actual
                    .iter()
                    .all(|key| crate::host::is_agent_environment_key(key))
            );
        }
    }

    #[test]
    fn omp_only_finishes_after_its_final_noncontinuing_agent_end() {
        use crate::core::cli_agent::{
            AgentOutcome, AgentSessionState, AgentStatus, parse_agent_event_body,
        };
        let mut state = AgentSessionState::default();
        for payload in [
            r#"{"willContinue":true,"last_assistant":{"stopReason":"error","errorMessage":"retrying"}}"#,
            r#"{"awaitingAsyncWork":true,"willContinue":true}"#,
            r#"{"willContinue":true,"last_assistant":{"stopReason":"stop"}}"#,
        ] {
            let body = build_hook_body("omp", "agent-end", payload);
            state.apply_event(&parse_agent_event_body(body.as_bytes()).unwrap());
            assert_eq!(state.status, AgentStatus::Working);
            assert_eq!(state.turns, 0);
            assert_eq!(state.readout.result, None);
        }
        let body = build_hook_body(
            "omp",
            "agent-end",
            r#"{"last_assistant":{"stopReason":"aborted","errorMessage":"operator cancelled"}}"#,
        );
        state.apply_event(&parse_agent_event_body(body.as_bytes()).unwrap());
        assert_eq!(state.status, AgentStatus::Done);
        assert_eq!(
            state.readout.result.as_ref().unwrap().outcome,
            AgentOutcome::Interrupted
        );
        assert_eq!(state.message.as_deref(), Some("operator cancelled"));
    }

    #[test]
    fn omp_approval_and_plan_review_resolve_only_the_matching_request() {
        use crate::core::cli_agent::{AgentSessionState, AgentStatus, parse_agent_event_body};
        let mut state = AgentSessionState::default();
        for (event, payload) in [
            ("prompt-submit", r#"{"session_id":"main"}"#),
            (
                "permission-request",
                r#"{"request":{"id":"tool-1","kind":"permission"}}"#,
            ),
            (
                "question-asked",
                r#"{"request":{"id":"plan-1","kind":"plan-review"}}"#,
            ),
            (
                "tool-complete",
                r#"{"request":{"id":"tool-1","kind":"permission","resolved":true},"message":"permission denied"}"#,
            ),
        ] {
            state.apply_event(
                &parse_agent_event_body(build_hook_body("omp", event, payload).as_bytes()).unwrap(),
            );
        }
        assert_eq!(state.status, AgentStatus::Waiting);
        assert_eq!(state.pending_requests[0].id, "plan-1");
        state.apply_event(
            &parse_agent_event_body(
                build_hook_body(
                    "omp",
                    "tool-complete",
                    r#"{"request":{"id":"plan-1","kind":"plan-review","resolved":true}}"#,
                )
                .as_bytes(),
            )
            .unwrap(),
        );
        assert_eq!(state.status, AgentStatus::Working);
        assert_eq!(
            state.readout.result, None,
            "a denied tool is not a terminal failed turn"
        );
    }

    #[test]
    fn omp_reports_its_actual_session_scope_without_credential_environment() {
        let body = build_hook_body(
            "omp",
            "session-start",
            r#"{"session_id":"real-source","session_scope":{"profile":"work","session_file":"/isolated/saved.jsonl","session_dir":"/isolated","environment":{"HOME":"/isolated-home","OPENAI_API_KEY":"must-not-leak"}}}"#,
        );
        assert!(!body.contains("must-not-leak"));
        let event = crate::core::cli_agent::parse_agent_event_body(body.as_bytes()).unwrap();
        let scope = event.readout.session_scope.unwrap();
        assert_eq!(scope.profile.as_deref(), Some("work"));
        assert_eq!(
            scope.session_file.as_deref(),
            Some(std::path::Path::new("/isolated/saved.jsonl"))
        );
        assert_eq!(
            scope.environment.get("HOME").map(String::as_str),
            Some("/isolated-home")
        );
        assert!(!scope.environment.contains_key("OPENAI_API_KEY"));
    }

    /// A hook whose pane closed under it gives up instead of waiting on the
    /// terminal's other end for good.
    #[cfg(unix)]
    #[test]
    fn a_terminal_with_its_other_end_gone_fails_the_write_at_once() {
        // Both ends are opened close-on-exec from the start: the other tests
        // spawn processes on threads of their own, and a child that inherited
        // the pane's end would keep it open after it is closed here, so the
        // write would go through.
        let flags = libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC;
        let master = unsafe { libc::posix_openpt(flags) };
        assert!(master >= 0);
        assert_eq!(unsafe { libc::grantpt(master) }, 0);
        assert_eq!(unsafe { libc::unlockpt(master) }, 0);
        let name = unsafe { std::ffi::CStr::from_ptr(libc::ptsname(master)) }
            .to_string_lossy()
            .into_owned();
        let path = std::ffi::CString::new(name.clone()).unwrap();
        let slave = unsafe { libc::open(path.as_ptr(), flags) };
        assert!(slave >= 0);
        // The agent still holds its end; the pane's is gone.
        unsafe { libc::close(master) };

        // A child another test forked a moment ago still holds a copy of the
        // pane's end until it gets to exec, and a write in that moment goes
        // through; one made once it has let go fails. None may wait.
        let mut wrote = Ok(true);
        for _ in 0..50 {
            let (tx, rx) = std::sync::mpsc::channel();
            let name = name.clone();
            std::thread::spawn(move || {
                let _ = tx.send(write_dev(std::path::Path::new(&name), b"x"));
            });
            wrote = rx.recv_timeout(std::time::Duration::from_secs(5));
            if wrote != Ok(true) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        unsafe { libc::close(slave) };
        assert_eq!(wrote, Ok(false));
    }

    #[test]
    fn kimi_permission_replies_resolve_only_the_named_request() {
        use crate::core::cli_agent::{AgentSessionState, AgentStatus, parse_agent_event_body};
        let mut state = AgentSessionState::default();
        for (event, payload) in [
            ("prompt-submit", serde_json::json!({"session_id":"main"})),
            (
                "permission-request",
                serde_json::json!({"session_id":"main","agent_id":"main","id":"approval-1"}),
            ),
            (
                "permission-request",
                serde_json::json!({"session_id":"main","agent_id":"main","id":"approval-2"}),
            ),
            (
                "permission-result",
                serde_json::json!({"session_id":"main","agent_id":"main","id":"approval-1","decision":"approved"}),
            ),
            (
                "tool-failed",
                serde_json::json!({"session_id":"main","tool_call_id":"other","tool_name":"Bash","error":{"message":"retryable tool error"}}),
            ),
        ] {
            let body = build_hook_body("kimi", event, &payload.to_string());
            state.apply_event(&parse_agent_event_body(body.as_bytes()).unwrap());
        }
        assert_eq!(state.status, AgentStatus::Waiting);
        assert_eq!(
            state
                .pending_requests
                .iter()
                .map(|request| request.id.as_str())
                .collect::<Vec<_>>(),
            ["approval-2"]
        );
        assert!(
            state.readout.result.is_none(),
            "a tool failure is not the final turn result"
        );
        let body = build_hook_body(
            "kimi",
            "permission-result",
            r#"{"session_id":"main","agent_id":"main","id":"approval-2","decision":"denied"}"#,
        );
        state.apply_event(&parse_agent_event_body(body.as_bytes()).unwrap());
        assert_eq!(
            state.status,
            AgentStatus::Working,
            "denied tool approval can still be followed by a retry"
        );
        assert!(state.pending_requests.is_empty());
        assert!(state.readout.result.is_none());
    }

    #[test]
    fn kimi_background_question_is_not_resolved_by_its_tool_return() {
        use crate::core::cli_agent::{AgentSessionState, AgentStatus, parse_agent_event_body};
        let mut state = AgentSessionState::default();
        for (event, payload) in [
            ("prompt-submit", serde_json::json!({"session_id":"s"})),
            (
                "task-started",
                serde_json::json!({"session_id":"s","task_id":"question-9","kind":"question"}),
            ),
            (
                "tool-complete",
                serde_json::json!({"session_id":"s","tool_name":"AskUserQuestion","tool_call_id":"tool-9","tool_input":{"background":true}}),
            ),
            (
                "stop",
                serde_json::json!({"session_id":"s","stop_hook_active":false}),
            ),
        ] {
            let body = build_hook_body("kimi", event, &payload.to_string());
            state.apply_event(&parse_agent_event_body(body.as_bytes()).unwrap());
        }
        assert_eq!(state.status, AgentStatus::Waiting);
        assert_eq!(state.pending_requests[0].id, "question-9");
        assert_eq!(state.turns, 0, "pre-veto Stop cannot finish a turn");
        let body = build_hook_body(
            "kimi",
            "task-notification",
            r#"{"session_id":"s","notification_type":"task.completed","source_kind":"background_task","source_id":"question-9"}"#,
        );
        state.apply_event(&parse_agent_event_body(body.as_bytes()).unwrap());
        assert!(state.pending_requests.is_empty());
        assert_eq!(state.status, AgentStatus::Working);
        assert!(state.readout.result.is_none());
    }

    #[test]
    fn kimi_child_and_unscoped_turn_started_hooks_do_not_replace_main_identity() {
        assert_eq!(
            effective_event(
                "kimi",
                "turn-started",
                r#"{"session_id":"s","turn_id":3,"origin_kind":"task"}"#
            ),
            None
        );
        assert_eq!(
            effective_event(
                "kimi",
                "permission-request",
                r#"{"session_id":"child","agent_id":"agent-0","id":"p"}"#
            ),
            None
        );
        assert_eq!(
            effective_event(
                "kimi",
                "turn-started",
                r#"{"session_id":"s","agent_id":"main","turn_id":3}"#
            ),
            Some("tool-complete")
        );
    }

    #[test]
    fn other_agents_carry_no_readout() {
        let stdin = r#"{"session_id":"s","permission_mode":"plan"}"#;
        let seq = build_hook_sequence("kimi", "stop", stdin);
        let ev = crate::core::cli_agent::parse_agent_event(
            seq.strip_prefix(b"\x1b]")
                .unwrap()
                .strip_suffix(b"\x07")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(ev.readout, Default::default());
    }

    /// The exhaustive match keeps every detected agent mapped; this keeps the
    /// other direction honest, so a hooked agent cannot become unreachable
    /// from detection and silently stop being diagnosed.
    #[test]
    fn every_hooked_agent_is_reachable_from_detection() {
        for hooked in HookAgent::ALL {
            assert!(
                CLIAgent::ALL
                    .into_iter()
                    .any(|detected| HookAgent::of_detected(detected) == Some(hooked)),
                "{hooked:?} has hooks but no detected agent maps to it"
            );
        }
    }

    #[test]
    fn every_lyrel_daemon_host_takes_the_console_fast_path() {
        for name in [
            "lyrel-app.exe",
            "lyrel-server.exe",
            "lyrel.exe",
            "lyrel-server-c12p7.exe",
        ] {
            assert!(is_lyrel_host_exe(name), "{name} hosts lyrel shells");
        }
        for name in [
            "explorer.exe",
            "cmd.exe",
            "lyrel",
            "lyrel-app",
            "wt.exe",
            "otherterm-server-c12p7.exe",
            "not-lyrel-server-c12p7.exe",
            "lyrel-server-c12p7.exe.old",
        ] {
            assert!(
                !is_lyrel_host_exe(name),
                "{name} is not a lyrel host process"
            );
        }
    }

    /// Parses a built sequence back the way the terminal's scanner does.
    fn round_trip(
        agent: &str,
        event: &str,
        stdin_json: &str,
    ) -> crate::core::cli_agent::AgentEvent {
        let seq = build_hook_sequence(agent, event, stdin_json);
        crate::core::cli_agent::parse_agent_event(&seq[2..seq.len() - 1]).expect("parses")
    }

    /// What the user typed never rides along: nothing downstream reads it,
    /// and a pasted prompt made the sequence long enough for the agent's own
    /// output to land in the middle of it.
    #[test]
    fn a_submitted_prompt_stays_out_of_the_sequence() {
        let prompt = "x".repeat(64 * 1024);
        let seq = build_hook_sequence(
            "kimi",
            "prompt-submit",
            &serde_json::json!({ "prompt": prompt, "session_id": "s-1" }).to_string(),
        );
        assert!(!String::from_utf8_lossy(&seq).contains("xxxx"));
        assert!(seq.len() < 200, "{} bytes", seq.len());
        let ev = round_trip(
            "kimi",
            "prompt-submit",
            r#"{"prompt":"hi","session_id":"s-1"}"#,
        );
        assert_eq!(
            ev.kind,
            crate::core::cli_agent::AgentEventKind::PromptSubmit
        );
        assert_eq!(ev.message, None, "a prompt is not a message");
    }

    #[test]
    fn a_long_message_is_cut_on_a_character_boundary() {
        let ev = round_trip(
            "omp",
            "permission-request",
            &serde_json::json!({ "message": "允许".repeat(1000) }).to_string(),
        );
        assert_eq!(ev.message.map(|m| m.chars().count()), Some(MESSAGE_MAX));
        assert_eq!(clip_chars("short", MESSAGE_MAX), "short");
    }

    #[test]
    fn every_installed_kimi_event_is_normalized_for_the_consumer() {
        use crate::core::cli_agent::parse_agent_event;
        for (_, event) in KIMI_HOOK_EVENTS {
            let seq = build_hook_sequence("kimi", event, "{}");
            let parsed = parse_agent_event(&seq[2..seq.len() - 1])
                .expect("Kimi adapter normalizes native lifecycle names");
            assert_eq!(parsed.agent, Some(CLIAgent::Kimi));
        }
    }

    #[test]
    fn a_remote_hook_names_the_server_its_own_installer_placed() {
        let host = FakeRemote::shared();
        let dialect = crate::daemon::install::RemoteProtocol::of_this_build();
        let name = crate::daemon::install::asset::binary_name(dialect.control, dialect.protocol);

        let unix = HookTarget::remote(&*host, PathBuf::from("/home/me"));
        assert_eq!(
            unix.exe,
            PathBuf::from(format!("/home/me/.local/share/lyrel/bin/{name}"))
        );

        let windows = HookTarget::remote(&*host, PathBuf::from(r"C:\Users\me"));
        assert_eq!(
            windows.exe,
            PathBuf::from(format!(r"C:\Users\me\AppData\Local\lyrel\bin\{name}.exe"))
        );
    }

    /// A Pi bridge written before Pi changed its package name is ours but
    /// stale, so launch's refresh rewrites it with the current import.
    #[test]
    fn a_pi_bridge_importing_the_old_package_is_refreshed() {
        let host = FakeRemote::shared();
        let base = std::env::temp_dir().join(format!("lyrel-pi-bridge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let target = HookTarget::remote(&*host, base.clone());
        let path = HookAgent::Pi.target_path(&target).unwrap();
        let current = owned_file_content(&target, HookAgent::Pi).expect("pi bridge builds");
        let old = current.replace(
            "@earendil-works/pi-coding-agent",
            "@mariozechner/pi-coding-agent",
        );
        assert_ne!(old, current);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &old).unwrap();

        assert_eq!(hooks_state(&target, HookAgent::Pi), HooksState::Outdated);
        assert_eq!(refresh_hooks(&target), 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), current);
        assert_eq!(hooks_state(&target, HookAgent::Pi), HooksState::Installed);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn ancestor_tty_device_is_none_or_a_dev_path() {
        match ancestor_tty_device() {
            None => {}
            Some(dev) => assert!(
                dev.starts_with("/dev/"),
                "a resolved tty must be an openable device path, got {dev:?}"
            ),
        }
    }

    fn local_host() -> crate::host::SharedHost {
        crate::host::local::LocalHost::new()
    }

    struct FakeRemote(
        crate::host::SharedHost,
        std::collections::BTreeMap<String, String>,
    );

    impl FakeRemote {
        fn shared() -> crate::host::SharedHost {
            std::sync::Arc::new(FakeRemote(local_host(), Default::default()))
        }
    }

    #[test]
    fn offline_pi_can_install_refresh_and_remove_its_integration() {
        let directory = tempfile::tempdir().unwrap();
        let host = FakeRemote::shared();
        let target = HookTarget::remote(&*host, directory.path().into()).with_launch_commands(
            &std::collections::HashMap::from([("pi".into(), "pi --offline -ne".into())]),
        );
        install_hooks(&target, HookAgent::Pi).unwrap();
        assert_eq!(hooks_state(&target, HookAgent::Pi), HooksState::Installed);
        install_hooks(&target, HookAgent::Pi).unwrap();
        uninstall_hooks(&target, HookAgent::Pi).unwrap();
        assert_eq!(
            hooks_state(&target, HookAgent::Pi),
            HooksState::NotInstalled
        );
    }

    #[test]
    fn configured_pi_scope_drives_management_without_touching_default_or_user_files() {
        let directory = tempfile::tempdir().unwrap();
        let host = FakeRemote::shared();
        let target = HookTarget::remote(&*host, directory.path().into()).with_launch_commands(
            &std::collections::HashMap::from([(
                "pi".into(),
                "PI_CODING_AGENT_DIR=~/.pi-custom pi --model native".into(),
            )]),
        );
        let path = HookAgent::Pi.target_path(&target).unwrap();
        assert_eq!(
            path,
            directory
                .path()
                .join(".pi-custom/extensions/lyrel/index.ts")
        );
        install_hooks(&target, HookAgent::Pi).unwrap();
        let user = path.parent().unwrap().join("user.ts");
        std::fs::write(&user, "user-owned").unwrap();
        std::fs::write(
            &path,
            "/* lyrel agent-hook pi bridge — generated by lyrel, do not edit. */\nold bridge",
        )
        .unwrap();
        assert_eq!(hooks_state(&target, HookAgent::Pi), HooksState::Outdated);
        install_hooks(&target, HookAgent::Pi).unwrap();
        uninstall_hooks(&target, HookAgent::Pi).unwrap();
        assert_eq!(std::fs::read_to_string(user).unwrap(), "user-owned");
        assert!(!path.exists());
        assert!(
            !directory
                .path()
                .join(".pi/agent/extensions/lyrel/index.ts")
                .exists()
        );
        let ambiguous = HookTarget::remote(&*host, directory.path().into()).with_launch_commands(
            &std::collections::HashMap::from([("pi".into(), "sh -c 'pi'".into())]),
        );
        assert!(install_hooks(&ambiguous, HookAgent::Pi).is_err());
        assert!(matches!(
            hooks_diagnostic(&ambiguous, HookAgent::Pi),
            Some(HooksDiagnostic::ScopeUnavailable { .. })
        ));
    }

    #[test]
    fn configured_omp_launch_selectors_drive_diagnosis_install_and_uninstall() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("configured home");
        let host = FakeRemote(local_host(), Default::default());
        let line = format!(
            "HOME='{}' omp launch --profile work --model native",
            home.display()
        );
        let target = HookTarget::remote(&host, directory.path().join("wrong-home"))
            .with_launch_commands(&std::collections::HashMap::from([("omp".into(), line)]));
        let path = HookAgent::OhMyPi.target_path(&target).unwrap();
        assert_eq!(
            path,
            home.join(".omp/profiles/work/agent/extensions/lyrel/index.ts")
        );
        assert_eq!(
            hooks_state(&target, HookAgent::OhMyPi),
            HooksState::NotInstalled
        );
        install_hooks(&target, HookAgent::OhMyPi).unwrap();
        assert_eq!(
            hooks_state(&target, HookAgent::OhMyPi),
            HooksState::Installed
        );
        uninstall_hooks(&target, HookAgent::OhMyPi).unwrap();
        assert!(!path.exists());
        let ambiguous = HookTarget::remote(&host, directory.path().into()).with_launch_commands(
            &std::collections::HashMap::from([(
                "omp".into(),
                "sh -c 'omp --profile other'".into(),
            )]),
        );
        assert!(install_hooks(&ambiguous, HookAgent::OhMyPi).is_err());
        assert!(matches!(
            hooks_diagnostic(&ambiguous, HookAgent::OhMyPi),
            Some(HooksDiagnostic::ScopeUnavailable { .. })
        ));
        assert!(
            !directory
                .path()
                .join(".omp/agent/extensions/lyrel/index.ts")
                .exists()
        );
    }

    #[test]
    fn omp_install_update_and_uninstall_stay_in_the_selected_remote_profile() {
        let directory = tempfile::tempdir().unwrap();
        let host = FakeRemote(
            local_host(),
            std::collections::BTreeMap::from([
                ("OMP_PROFILE".into(), "work".into()),
                ("PI_CONFIG_DIR".into(), ".omp-custom".into()),
                (
                    "PI_CODING_AGENT_DIR".into(),
                    "/must-not-win-for-profile".into(),
                ),
            ]),
        );
        let target = HookTarget::remote(&host, directory.path().into());
        let path = HookAgent::OhMyPi.target_path(&target).unwrap();
        assert_eq!(
            path,
            directory
                .path()
                .join(".omp-custom/profiles/work/agent/extensions/lyrel/index.ts")
        );
        install_hooks(&target, HookAgent::OhMyPi).unwrap();
        let user = path.parent().unwrap().join("user.ts");
        std::fs::write(&user, "user-owned").unwrap();
        std::fs::write(
            &path,
            "/* lyrel agent-hook omp bridge — generated by lyrel, do not edit. */\nold bridge",
        )
        .unwrap();
        assert_eq!(
            hooks_state(&target, HookAgent::OhMyPi),
            HooksState::Outdated
        );
        install_hooks(&target, HookAgent::OhMyPi).unwrap();
        uninstall_hooks(&target, HookAgent::OhMyPi).unwrap();
        assert_eq!(std::fs::read_to_string(&user).unwrap(), "user-owned");
        assert!(!path.exists());
        assert!(
            !directory
                .path()
                .join(".omp/agent/extensions/lyrel/index.ts")
                .exists()
        );
    }

    impl Host for FakeRemote {
        fn agent_environment(&self) -> io::Result<std::collections::BTreeMap<String, String>> {
            Ok(self.1.clone())
        }
        fn id(&self) -> crate::host::HostId {
            crate::host::HostId::from_connection_key("ssh-direct:me@box:22")
        }
        fn separator(&self) -> char {
            self.0.separator()
        }
        fn is_absolute(&self, p: &Path) -> bool {
            self.0.is_absolute(p)
        }
        fn read_dir(&self, dir: &Path, root: Option<&Path>) -> io::Result<Vec<crate::host::Entry>> {
            self.0.read_dir(dir, root)
        }
        fn stat(&self, p: &Path) -> io::Result<crate::host::Meta> {
            self.0.stat(p)
        }
        fn read_file(&self, p: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
            self.0.read_file(p, max_bytes)
        }
        fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
            self.0.canonicalize(p)
        }
        fn search(
            &self,
            roots: &[PathBuf],
            query: &str,
            limit: usize,
            max_dirs: usize,
            show_hidden: bool,
        ) -> io::Result<Vec<crate::host::SearchHit>> {
            self.0.search(roots, query, limit, max_dirs, show_hidden)
        }
        fn search_content(
            &self,
            roots: &[PathBuf],
            query: &crate::host::ContentQuery,
            limits: &crate::host::ContentLimits,
        ) -> io::Result<crate::host::ContentResults> {
            self.0.search_content(roots, query, limits)
        }
        fn write_file(&self, p: &Path, bytes: &[u8]) -> io::Result<crate::host::Meta> {
            self.0.write_file(p, bytes)
        }
        fn create_file_new(&self, p: &Path) -> io::Result<()> {
            self.0.create_file_new(p)
        }
        fn create_dir(&self, p: &Path, recursive: bool) -> io::Result<()> {
            self.0.create_dir(p, recursive)
        }
        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            self.0.rename(from, to)
        }
        fn remove(&self, p: &Path, recursive: bool) -> io::Result<()> {
            self.0.remove(p, recursive)
        }
        fn repo_root(&self, p: &Path) -> io::Result<Option<PathBuf>> {
            self.0.repo_root(p)
        }
        fn git(&self, cwd: &Path, args: &[&str]) -> io::Result<crate::host::Output> {
            self.0.git(cwd, args)
        }
        fn shells(&self) -> io::Result<crate::host::ShellInventory> {
            self.0.shells()
        }
        fn watch(&self, dirs: &[PathBuf]) -> io::Result<crate::host::WatchSub> {
            self.0.watch(dirs)
        }
    }

    // Lifecycle tests inspect configs without executing the runner. Supply a
    // product-named path in their sandbox, not Cargo's PATH-resolvable test exe.
    fn local_hook_target<'a>(host: &'a dyn Host, root: &Path) -> HookTarget<'a> {
        let exe = root.join(format!("lyrel-app{}", std::env::consts::EXE_SUFFIX));
        HookTarget::local_for_exe(host, exe).expect("home resolves in tests")
    }

    #[test]
    fn hook_command_quotes_the_exe_path() {
        let host = local_host();
        let target = HookTarget::local(&*host).expect("home resolves in tests");
        let cmd = target.hook_command(HookAgent::Kimi, "stop");
        // On Windows the executable may be emitted as a bare PATH-resolvable
        // name: PowerShell cannot invoke a quoted path without the `&` call
        // operator, so quoting is avoided whenever possible.
        #[cfg(not(windows))]
        assert!(cmd.starts_with('"'));
        assert!(cmd.ends_with("agent-hook kimi stop"));
    }

    #[test]
    fn the_hook_command_names_the_binary_on_that_machine() {
        let host = FakeRemote::shared();
        let target = HookTarget::remote(&*host, PathBuf::from("/home/me"));
        let dialect = crate::daemon::install::RemoteProtocol::of_this_build();
        let name = format!("lyrel-server-c{}p{}", dialect.control, dialect.protocol);
        assert_eq!(
            target.hook_command(HookAgent::Kimi, "stop"),
            format!("\"/home/me/.local/share/lyrel/bin/{name}\" agent-hook kimi stop")
        );

        let local = local_host();
        let here = HookTarget::local(&*local).expect("home resolves in tests");
        let exe = std::env::current_exe().unwrap();
        let command_exe = here
            .hook_command_exe()
            .unwrap_or_else(|| format!("\"{}\"", exe.display()));
        assert_eq!(
            here.hook_command(HookAgent::Kimi, "stop"),
            format!("{command_exe} agent-hook kimi stop")
        );
    }

    #[test]
    fn remote_toml_hooks_do_not_claim_old_product_entries() {
        let sandbox = tempfile::tempdir().unwrap();
        let host = FakeRemote::shared();
        let target = HookTarget::remote(&*host, sandbox.path().to_path_buf());
        for agent in [HookAgent::Kimi] {
            let path = agent.target_path(&target).unwrap();
            let original = "# user settings\n[[hooks]]\nevent = 'Stop'\ncommand = '\"/opt/lyrel/otherterm-server-c12p7\" agent-hook kimi stop'\n";
            target.write(&path, original.as_bytes()).unwrap();
            assert_eq!(hooks_state(&target, agent), HooksState::NotInstalled);
            assert_eq!(refresh_hooks(&target), 0);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            assert_eq!(
                uninstall_hooks(&target, agent).unwrap(),
                HookOutcome::NoLyrelHooks
            );
            install_hooks(&target, agent).unwrap();
            install_hooks(&target, agent).unwrap();
            assert_eq!(hooks_state(&target, agent), HooksState::Installed);
            uninstall_hooks(&target, agent).unwrap();
            let after = std::fs::read_to_string(&path).unwrap();
            let after = after.parse::<toml_edit::DocumentMut>().unwrap();
            let before = original.parse::<toml_edit::DocumentMut>().unwrap();
            if agent == HookAgent::Kimi {
                let entries = after["hooks"].as_array_of_tables().unwrap();
                assert_eq!(entries.len(), 1);
                assert_eq!(
                    entries.get(0).unwrap()["command"].as_str(),
                    before["hooks"]
                        .as_array_of_tables()
                        .unwrap()
                        .get(0)
                        .unwrap()["command"]
                        .as_str()
                );
            }
        }
    }

    #[test]
    fn opencode_scoped_launch_keeps_preview_policy_and_default_config_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("default-home");
        let isolated = directory.path().join("isolated-config");
        let host = FakeRemote::shared();
        let default = HookTarget::remote(&*host, home.clone());
        let default_cli = default.under(&home, &[".config", "opencode", "cli.json"]);
        default
            .write(
                &default_cli,
                br#"{"session":{"image_preview":false},"plugins":["-lyrel"]}"#,
            )
            .unwrap();
        let scoped_cli = default.under(&isolated, &["opencode", "cli.json"]);
        let disabled = r#"{"session":{"image_preview":false},"plugins":["*"]}"#;
        default.write(&scoped_cli, disabled.as_bytes()).unwrap();
        let command = format!(
            "XDG_CONFIG_HOME='{}' opencode --standalone -s ses_fixture --auto",
            isolated.display()
        );
        let target = HookTarget::remote(&*host, home.clone()).with_launch_commands(
            &std::collections::HashMap::from([("opencode".into(), command)]),
        );
        install_hooks(&target, HookAgent::OpenCode).unwrap();
        assert_eq!(
            hooks_state(&target, HookAgent::OpenCode),
            HooksState::Installed
        );
        assert!(
            target
                .read(&target.under(&isolated, &["opencode", "plugins", "lyrel", "tui.ts"]))
                .is_ok()
        );
        assert_eq!(target.read(&scoped_cli).unwrap(), disabled);
        assert_eq!(
            default.read(&default_cli).unwrap(),
            r#"{"session":{"image_preview":false},"plugins":["-lyrel"]}"#
        );
        assert!(!home.join(".config/opencode/plugins/lyrel/tui.ts").exists());
        uninstall_hooks(&target, HookAgent::OpenCode).unwrap();
        assert_eq!(target.read(&scoped_cli).unwrap(), disabled);
        let ambiguous = HookTarget::remote(&*host, home).with_launch_commands(
            &std::collections::HashMap::from([(
                "opencode".into(),
                "XDG_CONFIG_HOME=relative opencode".into(),
            )]),
        );
        assert!(install_hooks(&ambiguous, HookAgent::OpenCode).is_err());
    }

    #[test]
    fn opencode_install_respects_disabled_plugins_and_preserves_user_files() {
        let dir =
            std::env::temp_dir().join(format!("lyrel-opencode-presence-{}", std::process::id()));
        let host = FakeRemote::shared();
        let target = HookTarget::remote(&*host, dir.clone());
        let config = target.under(
            &target.opencode_config_dir_checked().unwrap(),
            &["cli.json"],
        );
        target
            .write(&config, br#"{"plugins":["*","-lyrel","user-plugin"]}"#)
            .unwrap();
        assert!(install_hooks(&target, HookAgent::OpenCode).is_err());
        assert!(matches!(
            hooks_diagnostic(&target, HookAgent::OpenCode),
            Some(HooksDiagnostic::OpenCodeDisabled { .. })
        ));
        assert_eq!(
            target.read(&config).unwrap(),
            r#"{"plugins":["*","-lyrel","user-plugin"]}"#
        );
        target
            .write(&config, br#"{"plugins":["*","user-plugin"]}"#)
            .unwrap();
        let legacy = target.under(
            &target.opencode_config_dir_checked().unwrap(),
            &["plugins", "lyrel.js"],
        );
        target
            .write(
                &legacy,
                b"// lyrel agent-hook opencode bridge old generated file\n",
            )
            .unwrap();
        assert_eq!(
            hooks_state(&target, HookAgent::OpenCode),
            HooksState::Outdated
        );
        install_hooks(&target, HookAgent::OpenCode).unwrap();
        assert_eq!(
            hooks_state(&target, HookAgent::OpenCode),
            HooksState::Installed
        );
        assert!(
            target.read(&legacy).is_err(),
            "upgrade removes only the marked legacy bridge"
        );
        let user = target.under(
            &target.opencode_config_dir_checked().unwrap(),
            &["plugins", "lyrel", "user.ts"],
        );
        target.write(&user, b"user-owned").unwrap();
        uninstall_hooks(&target, HookAgent::OpenCode).unwrap();
        assert_eq!(target.read(&user).unwrap(), "user-owned");
        assert!(target.read(&config).unwrap().contains("user-plugin"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_remote_install_round_trips_through_the_host() {
        let dir = std::env::temp_dir().join(format!("lyrel-remote-hooks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let host = FakeRemote::shared();
        let target = HookTarget::remote(&*host, dir.clone());

        for agent in [HookAgent::Pi, HookAgent::Kimi, HookAgent::OhMyPi] {
            assert_eq!(hooks_state(&target, agent), HooksState::NotInstalled);
            install_hooks(&target, agent).expect("install succeeds");
            assert_eq!(hooks_state(&target, agent), HooksState::Installed);
            let path = agent.target_path(&target).unwrap();
            let dialect = crate::daemon::install::RemoteProtocol::of_this_build();
            assert!(std::fs::read_to_string(&path).unwrap().contains(&format!(
                "lyrel-server-c{}p{}",
                dialect.control, dialect.protocol
            )));
            uninstall_hooks(&target, agent).expect("uninstall succeeds");
            assert_eq!(hooks_state(&target, agent), HooksState::NotInstalled);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn owned_file_round_trip_and_ownership_guard() {
        let dir = std::env::temp_dir().join(format!("lyrel-owned-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.ts");
        let marker = "agent-hook pi";
        let host = local_host();
        let t = local_hook_target(&*host, &dir);
        let content = owned_file_content(&t, HookAgent::Pi).unwrap();

        assert_eq!(
            owned_file_state(&t, &path, &content, marker),
            HooksState::NotInstalled
        );
        owned_file_install(&t, &path, &content, marker).expect("fresh install succeeds");
        assert_eq!(
            owned_file_state(&t, &path, &content, marker),
            HooksState::Installed
        );

        std::fs::write(&path, content.replace(marker, "agent-hook pi --old")).unwrap();
        assert_eq!(
            owned_file_state(&t, &path, &content, marker),
            HooksState::Outdated
        );
        owned_file_install(&t, &path, &content, marker).expect("reinstall over our own file");
        assert_eq!(
            owned_file_state(&t, &path, &content, marker),
            HooksState::Installed
        );

        std::fs::write(&path, "// my own hooks, hands off").unwrap();
        assert!(owned_file_install(&t, &path, &content, marker).is_err());
        assert!(owned_file_uninstall(&t, &path, marker).is_err());
        let old = r#"{"hooks":{"sessionStart":[{"type":"command","bash":"\"/opt/lyrel/otherterm-server\" agent-hook pi session-start"}]}}"#;
        std::fs::write(&path, old).unwrap();
        assert_eq!(
            owned_file_state(&t, &path, &content, marker),
            HooksState::NotInstalled
        );
        assert!(owned_file_install(&t, &path, &content, marker).is_err());
        assert!(owned_file_uninstall(&t, &path, marker).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), old);

        std::fs::write(&path, &content).unwrap();
        owned_file_uninstall(&t, &path, marker).expect("uninstall succeeds");
        assert!(!path.exists());
        owned_file_uninstall(&t, &path, marker).expect("uninstall is idempotent");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Kimi's hooks share `config.toml` with the user's providers and models,
    /// so the merge must leave everything that is not ours — including
    /// comments and formatting — byte-for-byte alone.
    #[test]
    fn kimi_install_preserves_the_user_s_config_toml() {
        let dir =
            std::env::temp_dir().join(format!("lyrel-kimi-hooks-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.toml");
        let user_half = concat!(
            "# my providers\n",
            "default_model = \"kimi-k2\"\n",
            "\n",
            "[[hooks]]\n",
            "event = \"Stop\"\n",
            "command = \"afplay ding.aiff\"\n",
        );
        std::fs::write(&config, user_half).unwrap();
        unsafe { std::env::set_var("KIMI_CODE_HOME", &dir) };

        let host = local_host();
        let t = local_hook_target(&*host, &dir);
        assert_eq!(HookAgent::Kimi.target_path(&t).unwrap(), config);

        assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::NotInstalled);
        install_hooks(&t, HookAgent::Kimi).expect("install succeeds");
        assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Installed);
        install_hooks(&t, HookAgent::Kimi).expect("re-install succeeds");

        let written = std::fs::read_to_string(&config).unwrap();
        assert!(
            written.starts_with(user_half),
            "the user's half of config.toml — comment included — survives untouched"
        );
        let doc: toml_edit::DocumentMut = written.parse().expect("still valid TOML");
        let hooks = doc["hooks"].as_array_of_tables().unwrap();
        assert_eq!(
            hooks
                .iter()
                .filter(|e| toml_command_is_marked(e, "agent-hook kimi"))
                .count(),
            KIMI_HOOK_EVENTS.len(),
            "exactly one lyrel entry per event after two installs"
        );
        for (event, _) in KIMI_HOOK_EVENTS {
            assert!(
                hooks.iter().any(|e| {
                    toml_command_is_marked(e, "agent-hook kimi")
                        && e.get("event").and_then(|v| v.as_str()) == Some(*event)
                }),
                "{event} carries the lyrel hook"
            );
        }

        let healthy = std::fs::read_to_string(&config).unwrap();
        std::fs::write(
            &config,
            healthy.replace("agent-hook kimi stop", "agent-hook kimi stop --stale"),
        )
        .unwrap();
        assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Outdated);
        install_hooks(&t, HookAgent::Kimi).expect("reinstall over an outdated entry succeeds");
        assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Installed);

        uninstall_hooks(&t, HookAgent::Kimi).expect("uninstall succeeds");
        assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::NotInstalled);
        let after = std::fs::read_to_string(&config).unwrap();
        assert!(
            after.contains("afplay ding.aiff"),
            "the user's own Stop hook survives uninstall"
        );
        assert!(!after.contains("agent-hook kimi"));
        uninstall_hooks(&t, HookAgent::Kimi).expect("uninstall is idempotent");

        std::fs::write(&config, "not = valid = toml").unwrap();
        assert!(
            install_hooks(&t, HookAgent::Kimi).is_err(),
            "a config.toml that does not parse is left alone"
        );
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "not = valid = toml",
            "and is not rewritten on the way out"
        );
        assert!(
            uninstall_hooks(&t, HookAgent::Kimi).is_err(),
            "uninstall refuses the same file"
        );
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "not = valid = toml"
        );

        unsafe { std::env::remove_var("KIMI_CODE_HOME") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `config.toml` that parses but spells `hooks` as something other than
    /// an array of tables is a file we do not understand. Every one of these
    /// must come back as a refusal with the file untouched — the one thing
    /// that must never happen to the file holding the user's API keys is a
    /// silent rewrite.
    #[test]
    fn kimi_refuses_a_hooks_key_of_the_wrong_toml_type() {
        let host = FakeRemote::shared();
        let base = std::env::temp_dir().join(format!("lyrel-kimi-shapes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        for (name, text) in [
            ("a_string", "hooks = \"nope\"\n"),
            ("a_table", "[hooks]\nfoo = 1\n"),
            (
                "an_inline_array",
                "hooks = [{ event = \"Stop\", command = \"afplay a.aiff\" }]\n",
            ),
        ] {
            let home = base.join(name);
            let t = HookTarget::remote(&*host, home.clone());
            let config = HookAgent::Kimi.target_path(&t).unwrap();
            std::fs::create_dir_all(config.parent().unwrap()).unwrap();
            std::fs::write(&config, text).unwrap();

            assert_eq!(
                hooks_state(&t, HookAgent::Kimi),
                HooksState::NotInstalled,
                "{name}: nothing of ours is in there"
            );
            assert!(
                install_hooks(&t, HookAgent::Kimi).is_err(),
                "{name}: install refuses"
            );
            assert_eq!(
                uninstall_hooks(&t, HookAgent::Kimi).unwrap(),
                HookOutcome::NoLyrelHooks,
                "{name}: uninstall finds nothing of ours"
            );
            assert_eq!(
                std::fs::read_to_string(&config).unwrap(),
                text,
                "{name}: the file is byte-for-byte what it was"
            );
        }

        // `hooks = []` is the one shape that carries no configuration at all,
        // so it is promoted instead of refused.
        let home = base.join("an_empty_array");
        let t = HookTarget::remote(&*host, home.clone());
        let config = HookAgent::Kimi.target_path(&t).unwrap();
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "model = \"k2\"\nhooks = []\n").unwrap();
        install_hooks(&t, HookAgent::Kimi).expect("an empty inline array is promoted, not refused");
        assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Installed);
        let written = std::fs::read_to_string(&config).unwrap();
        assert!(written.starts_with("model = \"k2\"\n"), "{written}");
        assert!(!written.contains("hooks = []"), "{written}");
        assert_eq!(
            uninstall_hooks(&t, HookAgent::Kimi).unwrap(),
            HookOutcome::Removed
        );
        assert!(
            !std::fs::read_to_string(&config).unwrap().contains("hooks"),
            "the key goes when the last entry in it does"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The rest of the TOML merge contract: a file that does not exist yet, a
    /// second install that changes nothing, entries a hand-edit has mangled,
    /// and an uninstall that has to thread its removals between the user's own
    /// entries and the tables that follow them.
    #[test]
    fn kimi_toml_merge_holds_up_across_the_awkward_shapes() {
        let host = FakeRemote::shared();
        let base = std::env::temp_dir().join(format!("lyrel-kimi-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let marker = "agent-hook kimi";

        // Nothing there at all: install creates the directory and the file.
        let t = HookTarget::remote(&*host, base.join("fresh"));
        let config = HookAgent::Kimi.target_path(&t).unwrap();
        assert!(!config.exists());
        assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::NotInstalled);
        assert_eq!(
            uninstall_hooks(&t, HookAgent::Kimi).unwrap(),
            HookOutcome::NothingInstalled,
            "there is no file to take anything out of"
        );
        install_hooks(&t, HookAgent::Kimi).expect("install writes a fresh config.toml");
        assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Installed);
        let once = std::fs::read_to_string(&config).unwrap();
        install_hooks(&t, HookAgent::Kimi).expect("re-install succeeds");
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            once,
            "a second install is byte-for-byte the first — no churn, no growth"
        );

        // A hand-edit that drops `event` leaves an entry that is ours and is
        // broken. That is Outdated, not NotInstalled: `refresh_hooks` only
        // ever revisits Outdated, so anything else hides the damage.
        let mangled = once.replacen("event = \"SessionStart\"\n", "", 1);
        assert_ne!(mangled, once);
        std::fs::write(&config, &mangled).unwrap();
        assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Outdated);
        install_hooks(&t, HookAgent::Kimi).expect("install repairs it");
        assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Installed);
        assert_eq!(std::fs::read_to_string(&config).unwrap(), once);

        // One marked entry too many is Outdated too, and install prunes it.
        // This one has no `event` at all, so counting only the entries that
        // still name one would find the full roster and call it Installed
        // while a broken ninth entry sat there.
        let mut extra = std::fs::read_to_string(&config).unwrap();
        extra.push_str("\n[[hooks]]\ncommand = \"lyrel agent-hook kimi stop\"\n");
        std::fs::write(&config, &extra).unwrap();
        assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Outdated);
        install_hooks(&t, HookAgent::Kimi).expect("install prunes the stray");
        assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Installed);
        let doc: toml_edit::DocumentMut =
            std::fs::read_to_string(&config).unwrap().parse().unwrap();
        assert_eq!(
            doc["hooks"]
                .as_array_of_tables()
                .unwrap()
                .iter()
                .filter(|e| toml_command_is_marked(e, marker))
                .count(),
            KIMI_HOOK_EVENTS.len()
        );

        // The user's own entry comes first and another table follows ours:
        // uninstall has to take out the middle and leave both ends alone.
        let t = HookTarget::remote(&*host, base.join("sandwich"));
        let config = HookAgent::Kimi.target_path(&t).unwrap();
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        let user_half = concat!(
            "# mine\n",
            "[[hooks]]\n",
            "event = \"Stop\"\n",
            "command = \"afplay a.aiff\"  # ding\n",
            "\n",
            "[providers.moonshot]\n",
            "api_key = \"secret\"\n",
        );
        std::fs::write(&config, user_half).unwrap();
        install_hooks(&t, HookAgent::Kimi).expect("install");
        assert_eq!(hooks_state(&t, HookAgent::Kimi), HooksState::Installed);
        let merged = std::fs::read_to_string(&config).unwrap();
        assert!(
            merged.starts_with(
                "# mine\n[[hooks]]\nevent = \"Stop\"\ncommand = \"afplay a.aiff\"  # ding\n"
            ),
            "the user's entry and its trailing comment come through verbatim:\n{merged}"
        );
        assert!(merged.contains("[providers.moonshot]\napi_key = \"secret\"\n"));
        assert_eq!(
            uninstall_hooks(&t, HookAgent::Kimi).unwrap(),
            HookOutcome::Removed
        );
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            user_half,
            "uninstall puts the file back exactly as the user left it"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn kimi_prefinal_and_unscoped_terminal_hooks_never_invent_success() {
        use crate::core::cli_agent::{AgentEventKind, parse_agent_event_body};
        for event in ["stop", "interrupted", "turn-failed"] {
            let body = build_hook_body("kimi", event, r#"{"session_id":"s"}"#);
            let report = parse_agent_event_body(body.as_bytes()).unwrap();
            assert_eq!(report.kind, AgentEventKind::ToolComplete);
            assert!(report.readout.result.is_none());
        }
    }
}
