//! Past coding-agent sessions on this computer, read off the agents' own
//! history files, so one can be picked and resumed.
//!
//! Only the ends of each file are read: the head carries where the session
//! ran and what it was first asked, the tail the title the agent last gave
//! it. A transcript runs to megabytes, and reading the whole of hundreds of
//! them to list their names would make opening the list the slow part.
//!
//! Parsed files are remembered by path, size and modification time, so after
//! the first scan only the sessions that moved are read again.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::core::cli_agent::CLIAgent;

/// How many sessions are listed, newest first. Past this the list is a
/// search target, not something to scroll, and older sessions are rarely
/// worth resuming.
const MAX_SESSIONS: usize = 500;

/// How much of a file's head and tail is read.
const HEAD_BYTES: u64 = 64 * 1024;
const TAIL_BYTES: u64 = 128 * 1024;

/// How much of a prompt a title keeps.
const TITLE_CHARS: usize = 120;

/// Crosses the control protocol: a remote workspace's server scans its own
/// machine and sends back what it found.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PastSession {
    pub agent: CLIAgent,
    pub id: String,
    /// Where it ran — where it has to be resumed, since agents key their
    /// history by directory.
    pub cwd: Option<PathBuf>,
    pub title: String,
    pub branch: Option<String>,
    /// Last written, in Unix seconds.
    pub updated: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<crate::host::session_operations::SessionScope>,
}

/// Where each agent keeps its history on this computer.
#[derive(Clone, Debug)]
pub struct Roots {
    /// `$PI_CODING_AGENT_DIR`, else `~/.pi/agent`.
    pub pi: PathBuf,
    /// A flat native Pi session store, selected by environment or settings.
    pub pi_sessions: Option<PathBuf>,
    pub pi_scopes: Vec<crate::host::session_operations::SessionScope>,
    /// `~/.omp/agent`.
    pub omp: PathBuf,
    pub omp_scope: crate::host::session_operations::SessionScope,
    pub omp_session_dir: Option<PathBuf>,
    pub omp_scopes: Vec<crate::host::session_operations::SessionScope>,
    /// `$KIMI_CODE_HOME`, else `~/.kimi-code`.
    pub kimi: PathBuf,
}

impl Roots {
    /// Every agent's default, under `home`.
    pub fn under(home: &Path) -> Self {
        Self {
            pi: home.join(".pi").join("agent"),
            pi_sessions: None,
            pi_scopes: Vec::new(),
            omp: home.join(".omp").join("agent"),
            omp_scope: crate::host::session_operations::SessionScope {
                profile: Some("default".into()),
                environment: std::collections::BTreeMap::from([(
                    "HOME".into(),
                    home.to_string_lossy().into_owned(),
                )]),
                ..Default::default()
            },
            omp_session_dir: None,
            omp_scopes: Vec::new(),
            kimi: home.join(".kimi-code"),
        }
    }

    /// The defaults under `home`, moved wherever the environment says an
    /// agent keeps its history instead.
    pub fn from_env(home: &Path) -> std::io::Result<Self> {
        let environment = crate::host::session_operations::SCOPE_ENV
            .iter()
            .copied()
            .chain(["KIMI_CODE_HOME"])
            .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
            .collect();
        Self::from_environment(home, &environment)
    }

    pub fn from_environment(
        home: &Path,
        environment: &std::collections::BTreeMap<String, String>,
    ) -> std::io::Result<Self> {
        let home_keys = if cfg!(windows) {
            ["USERPROFILE", "HOME"]
        } else {
            ["HOME", "USERPROFILE"]
        };
        let selected_home = home_keys
            .into_iter()
            .find_map(|key| environment.get(key).filter(|value| !value.is_empty()));
        let home = selected_home.map_or(home, |value| Path::new(value));
        let mut roots = Self::under(home);
        let cwd = std::env::current_dir().ok();
        if let Some(dir) = environment
            .get("PI_CODING_AGENT_DIR")
            .filter(|value| !value.is_empty())
        {
            roots.pi = if dir == "~" {
                home.to_path_buf()
            } else if let Some(rest) = dir.strip_prefix("~/").or_else(|| dir.strip_prefix("~\\")) {
                home.join(rest)
            } else {
                let dir = PathBuf::from(dir);
                if dir.is_absolute() {
                    dir
                } else {
                    cwd.as_ref().map(|cwd| cwd.join(&dir)).ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "Pi agent directory requires an executing Host working directory",
                        )
                    })?
                }
            };
        }
        roots.pi_sessions = pi_session_dir(
            &roots.pi,
            home,
            cwd.as_deref(),
            environment
                .get("PI_CODING_AGENT_SESSION_DIR")
                .map(String::as_str),
        );
        if let Some(dir) = environment.get("KIMI_CODE_HOME") {
            if !Path::new(dir).is_absolute() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "relative or empty KIMI_CODE_HOME requires the native pane working directory; cannot scan history from daemon defaults",
                ));
            }
            roots.kimi = dir.into();
        }
        let profile = omp_profile(environment)?;
        let base = home.join(
            environment
                .get("PI_CONFIG_DIR")
                .filter(|v| !v.is_empty())
                .map_or(".omp", String::as_str),
        );
        roots.omp = match profile {
            Some(profile) => base.join("profiles").join(profile).join("agent"),
            None => environment
                .get("PI_CODING_AGENT_DIR")
                .filter(|v| !v.is_empty())
                .map_or_else(|| base.join("agent"), PathBuf::from),
        };
        if !roots.omp.is_absolute() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "OMP agent directory must be absolute for history lookup",
            ));
        }
        roots.omp_scope.profile = Some(profile.unwrap_or("default").into());
        roots.omp_scope.environment.extend(
            environment
                .iter()
                .filter(|(key, _)| {
                    crate::host::session_operations::SCOPE_ENV.contains(&key.as_str())
                })
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if profile.is_some()
            || !environment
                .get("PI_CODING_AGENT_DIR")
                .is_some_and(|v| !v.is_empty())
        {
            if let Some(xdg) = environment.get("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
                let mut root = PathBuf::from(xdg).join("omp");
                if let Some(profile) = profile {
                    root = root.join("profiles").join(profile);
                }
                if root.exists() {
                    roots.omp_session_dir = Some(root.join("sessions"));
                }
            }
        }
        Ok(roots)
    }

    fn from_target(
        home: &Path,
        target: &crate::core::agent_hooks::HookTarget,
    ) -> std::io::Result<Self> {
        use crate::core::agent_hooks::HookAgent;
        let mut omp_environment = target.agent_environment(HookAgent::OhMyPi)?;
        // Kimi's configured launch may replace the Host default; validate its own scope below.
        omp_environment.remove("KIMI_CODE_HOME");
        let mut roots = Self::from_environment(home, &omp_environment)?;
        let pi_environment = target.agent_environment(HookAgent::Pi)?;
        roots.pi = target.pi_agent_dir_checked()?;
        roots.pi_sessions = pi_session_dir(
            &roots.pi,
            &target.agent_home(&pi_environment)?,
            std::env::current_dir().ok().as_deref(),
            pi_environment
                .get("PI_CODING_AGENT_SESSION_DIR")
                .map(String::as_str),
        );
        roots.kimi = target
            .kimi_config_path_checked()?
            .parent()
            .expect("Kimi config is below its effective home")
            .to_path_buf();
        Ok(roots)
    }

    /// This computer's agents, under the current user's home.
    pub fn local() -> std::io::Result<Self> {
        let home = home().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "home directory unavailable for agent history",
            )
        })?;
        let launch = crate::core::config::Config::load().agent_launch;
        let host = crate::host::local::LocalHost::new();
        let target = crate::core::agent_hooks::HookTarget::remote(&*host, home.clone())
            .with_launch_commands(&launch);
        let mut roots = Self::from_target(&home, &target)?;
        if let Some(store) = crate::core::machine::observed_store() {
            for pane in store.machine().panes {
                if let Some(agent) = pane.agent
                    && let Some(scope) = agent
                        .session_scope
                        .filter(|scope| scope.session_dir.is_some())
                {
                    let scopes = match agent.agent {
                        CLIAgent::OhMyPi => &mut roots.omp_scopes,
                        CLIAgent::Pi => &mut roots.pi_scopes,
                        _ => continue,
                    };
                    if !scopes.contains(&scope) {
                        scopes.push(scope);
                    }
                }
            }
        }
        Ok(roots)
    }
}

pub(crate) fn omp_profile(
    environment: &std::collections::BTreeMap<String, String>,
) -> std::io::Result<Option<&str>> {
    let profile = environment
        .get("OMP_PROFILE")
        .or_else(|| environment.get("PI_PROFILE"))
        .map(|v| v.trim())
        .filter(|v| !v.is_empty() && *v != "default");
    if let Some(profile) = profile {
        let stem = profile.split('.').next().unwrap_or("").to_ascii_lowercase();
        let reserved = matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
            || (stem.len() == 4
                && (stem.starts_with("com") || stem.starts_with("lpt"))
                && stem.as_bytes()[3].is_ascii_digit());
        if profile.len() > 64
            || !profile.as_bytes()[0].is_ascii_lowercase()
                && !profile.as_bytes()[0].is_ascii_digit()
            || profile.ends_with('.')
            || reserved
            || !profile
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid OMP profile name",
            ));
        }
    }
    Ok(profile)
}

pub fn scan(roots: &Roots) -> Vec<PastSession> {
    let mut files = Vec::new();
    for (index, scope) in roots.pi_scopes.iter().enumerate() {
        if let Some(dir) = &scope.session_dir {
            files.extend(files_in(dir, is_jsonl).into_iter().map(|(path, meta)| {
                let mut file = found(CLIAgent::Pi, path, &meta);
                file.pi_scope_index = Some(index);
                file
            }));
        }
    }
    files.extend(match &roots.pi_sessions {
        Some(dir) => files_in(dir, is_jsonl)
            .into_iter()
            .map(|(path, meta)| found(CLIAgent::Pi, path, &meta))
            .collect(),
        None => pi_files(&roots.pi.join("sessions"), CLIAgent::Pi),
    });
    for (index, scope) in roots.omp_scopes.iter().enumerate() {
        if let Some(dir) = &scope.session_dir {
            files.extend(pi_files(dir, CLIAgent::OhMyPi).into_iter().map(|mut file| {
                file.omp_scope_index = Some(index);
                file
            }));
        }
    }
    files.extend(pi_files(
        roots
            .omp_session_dir
            .as_deref()
            .unwrap_or(&roots.omp.join("sessions")),
        CLIAgent::OhMyPi,
    ));
    let mut seen_paths = std::collections::HashSet::new();
    files.retain(|file| seen_paths.insert(file.path.clone()));
    files.extend(kimi_files(&roots.kimi));
    files.sort_by_key(|f| std::cmp::Reverse(f.modified));
    files.truncate(MAX_SESSIONS);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    let mut seen = HashMap::with_capacity(files.len());
    for file in files {
        let session = cache
            .get(&file.path)
            .filter(|c| c.len == file.len && c.modified == file.modified)
            .map(|c| c.session.clone())
            .unwrap_or_else(|| read(&file));
        seen.insert(
            file.path.clone(),
            Cached {
                len: file.len,
                modified: file.modified,
                session: session.clone(),
            },
        );
        out.extend(session.map(|mut s| {
            if matches!(s.agent, CLIAgent::Pi | CLIAgent::OhMyPi) {
                let mut scope = crate::host::session_operations::SessionScope {
                    session_file: Some(file.path.clone()),
                    session_dir: file.path.parent().map(Path::to_path_buf),
                    ..Default::default()
                };
                let root = if s.agent == CLIAgent::OhMyPi {
                    let reported = file
                        .omp_scope_index
                        .and_then(|index| roots.omp_scopes.get(index))
                        .unwrap_or(&roots.omp_scope);
                    scope.profile.clone_from(&reported.profile);
                    scope.environment.clone_from(&reported.environment);
                    &roots.omp
                } else {
                    if let Some(reported) = file
                        .pi_scope_index
                        .and_then(|index| roots.pi_scopes.get(index))
                    {
                        scope.environment.clone_from(&reported.environment);
                    }
                    &roots.pi
                };
                if (s.agent == CLIAgent::Pi && file.pi_scope_index.is_none())
                    || (file.omp_scope_index.is_none()
                        && scope.profile.as_deref() == Some("default"))
                {
                    scope.environment.insert(
                        "PI_CODING_AGENT_DIR".into(),
                        root.to_string_lossy().into_owned(),
                    );
                }
                s.scope = Some(scope);
            }
            if s.agent == CLIAgent::Kimi {
                let scope = s.scope.get_or_insert_default();
                scope.session_file = Some(file.path.clone());
                scope.environment.insert(
                    "KIMI_CODE_HOME".into(),
                    roots.kimi.to_string_lossy().into_owned(),
                );
            }
            s
        }));
    }
    *cache = seen;
    drop(cache);
    out.sort_by_key(|s| std::cmp::Reverse(s.updated));
    out.truncate(MAX_SESSIONS);
    out
}

/// How a session is named in `hidden_agent_sessions`: ids are only unique
/// within one agent's history.
pub fn session_key(agent: CLIAgent, id: &str) -> String {
    format!("{}:{id}", agent.slug())
}

/// This user's home, where the agents keep their history.
pub fn home() -> Option<PathBuf> {
    let keys = if cfg!(windows) {
        ["USERPROFILE", "HOME"]
    } else {
        ["HOME", "USERPROFILE"]
    };
    keys.into_iter()
        .find_map(|key| std::env::var_os(key).filter(|home| !home.is_empty()))
        .map(PathBuf::from)
}

/// Native Pi 1.3.0 storage precedence. `cwd` is the executing Host's project,
/// never the GUI machine's directory. Project sessionDir is trust-independent.
pub(crate) fn pi_session_dir(
    agent_dir: &Path,
    home: &Path,
    cwd: Option<&Path>,
    environment: Option<&str>,
) -> Option<PathBuf> {
    let setting = |path: PathBuf| -> Option<String> {
        let bytes = std::fs::read(path).ok()?;
        let value: Value = serde_json::from_slice(&bytes).ok()?;
        value
            .get("sessionDir")?
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let selected = environment
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| cwd.and_then(|cwd| setting(cwd.join(".pi/settings.json"))))
        .or_else(|| setting(agent_dir.join("settings.json")))?;
    let path = if selected == "~" {
        home.to_path_buf()
    } else if let Some(rest) = selected
        .strip_prefix("~/")
        .or_else(|| selected.strip_prefix("~\\"))
    {
        home.join(rest)
    } else {
        PathBuf::from(selected)
    };
    if path.is_absolute() {
        Some(path)
    } else {
        cwd.map(|cwd| cwd.join(path))
    }
}

struct Cached {
    len: u64,
    modified: SystemTime,
    /// `None` for a file that holds no session worth listing — remembered
    /// too, so it is not read again until it changes.
    session: Option<PastSession>,
}

static CACHE: LazyLock<Mutex<HashMap<PathBuf, Cached>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct Found {
    agent: CLIAgent,
    path: PathBuf,
    len: u64,
    modified: SystemTime,
    omp_scope_index: Option<usize>,
    pi_scope_index: Option<usize>,
}

fn unix(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Parse the session `file` holds, whichever agent wrote it.
fn read(file: &Found) -> Option<PastSession> {
    let updated = unix(file.modified);
    match file.agent {
        CLIAgent::Pi | CLIAgent::OhMyPi => {
            let (head, tail) = ends(&file.path, file.len).ok()?;
            parse_pi(file.agent, &head, &tail, updated)
        }
        CLIAgent::Kimi => {
            let text = std::fs::read_to_string(&file.path).ok()?;
            let dir = file.path.parent()?.file_name()?.to_str()?;
            parse_kimi(dir, &text, updated)
        }
        _ => None,
    }
}

fn found(agent: CLIAgent, path: PathBuf, meta: &std::fs::Metadata) -> Found {
    Found {
        agent,
        path,
        len: meta.len(),
        modified: meta.modified().unwrap_or(UNIX_EPOCH),
        omp_scope_index: None,
        pi_scope_index: None,
    }
}

/// The files directly in `dir` that `keep` accepts by name.
fn files_in(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<(PathBuf, std::fs::Metadata)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(&keep))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            meta.is_file().then(|| (e.path(), meta))
        })
        .collect()
}

/// The directories directly in `dir`.
fn dirs_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect()
}

fn is_jsonl(name: &str) -> bool {
    name.ends_with(".jsonl")
}

/// Pi and Oh My Pi: `sessions/<cwd>/<time>_<id>.jsonl`. Oh My Pi keeps a
/// session's subagent transcripts in a directory beside it.
fn pi_files(sessions: &Path, agent: CLIAgent) -> Vec<Found> {
    files_in(sessions, is_jsonl)
        .into_iter()
        .chain(
            dirs_in(sessions)
                .into_iter()
                .flat_map(|project| files_in(&project, is_jsonl)),
        )
        .map(|(path, meta)| found(agent, path, &meta))
        .collect()
}

/// 按原生 OMP scope 枚举会话文件，不套用历史列表的最近 500 条上限。
pub(crate) fn omp_session_paths(
    scope: &crate::host::session_operations::SessionScope,
    cwd: &Path,
) -> std::io::Result<impl Iterator<Item = PathBuf>> {
    let root = if let Some(dir) = &scope.session_dir {
        if dir.is_absolute() {
            dir.clone()
        } else {
            cwd.join(dir)
        }
    } else {
        let home_keys = if cfg!(windows) {
            ["USERPROFILE", "HOME"]
        } else {
            ["HOME", "USERPROFILE"]
        };
        let home = home_keys
            .into_iter()
            .find_map(|key| {
                scope
                    .environment
                    .get(key)
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
            })
            .or_else(home)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "home directory unavailable for OMP session lookup",
                )
            })?;
        let mut environment = scope.environment.clone();
        // 此入口只定位 OMP，不让另一个 agent 的独立 home 选择器影响它。
        environment.remove("KIMI_CODE_HOME");
        if let Some(profile) = &scope.profile {
            environment.insert("OMP_PROFILE".into(), profile.clone());
        }
        if let Some(dir) = environment
            .get_mut("PI_CODING_AGENT_DIR")
            .filter(|value| !value.is_empty())
        {
            let path = if dir.as_str() == "~" {
                home.clone()
            } else if let Some(rest) = dir.strip_prefix("~/").or_else(|| dir.strip_prefix("~\\")) {
                home.join(rest)
            } else if Path::new(dir.as_str()).is_absolute() {
                PathBuf::from(&*dir)
            } else {
                cwd.join(&*dir)
            };
            *dir = path.to_string_lossy().into_owned();
        }
        let roots = Roots::from_environment(&home, &environment)?;
        roots
            .omp_session_dir
            .unwrap_or_else(|| roots.omp.join("sessions"))
    };
    Ok(pi_files(&root, CLIAgent::OhMyPi)
        .into_iter()
        .map(|file| file.path))
}

/// Kimi Code: `sessions/wd_<dir>_<hash>/<session>/state.json`.
fn kimi_files(root: &Path) -> Vec<Found> {
    dirs_in(&root.join("sessions"))
        .into_iter()
        .flat_map(|project| dirs_in(&project))
        .flat_map(|session| files_in(&session, |n| n == "state.json"))
        .map(|(path, meta)| found(CLIAgent::Kimi, path, &meta))
        .collect()
}

/// The file's first [`HEAD_BYTES`] and last [`TAIL_BYTES`], as text. The two
/// overlap on a short file, which is harmless: the head is read for the first
/// of things and the tail for the last.
fn ends(path: &Path, len: u64) -> std::io::Result<(String, String)> {
    let mut file = File::open(path)?;
    let mut head = Vec::new();
    (&mut file).take(HEAD_BYTES).read_to_end(&mut head)?;
    let mut tail = Vec::new();
    if len > HEAD_BYTES {
        file.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))?;
        file.take(TAIL_BYTES).read_to_end(&mut tail)?;
    }
    Ok((
        String::from_utf8_lossy(&head).into_owned(),
        String::from_utf8_lossy(&tail).into_owned(),
    ))
}

/// Complete JSON records in `text`. A cut at either end leaves a partial
/// line, which does not parse and is skipped.
fn records(text: &str) -> impl DoubleEndedIterator<Item = Value> + '_ {
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
}

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The first line of `text` worth reading as a title, or `None` for text
/// the harness wrote rather than the user.
fn prompt_title(text: &str) -> Option<String> {
    let text = strip_injected(text);
    if text.is_empty() || text.starts_with('<') || text.starts_with("Caveat:") {
        return None;
    }
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut title: String = line.chars().take(TITLE_CHARS).collect();
    if line.chars().count() > TITLE_CHARS {
        title.push('…');
    }
    Some(title)
}

/// `text` without the context blocks agents put in front of what was typed
/// (`<system-reminder>…</system-reminder>` and its kin).
fn strip_injected(text: &str) -> &str {
    const TAGS: [&str; 5] = [
        "system-reminder",
        "system_reminder",
        "hook_context",
        "loaded_context",
        "session_context",
    ];
    let mut text = text.trim();
    'strip: loop {
        for tag in TAGS {
            let open = format!("<{tag}>");
            let close = format!("</{tag}>");
            if let Some(rest) = text.strip_prefix(open.as_str())
                && let Some(end) = rest.find(close.as_str())
            {
                text = rest[end + close.len()..].trim_start();
                continue 'strip;
            }
        }
        return text;
    }
}

/// The text of a message's `content`: a string, or the first text part of a
/// list (`{"type": "text" | "input_text", "text"}`, or a bare `{"text"}`).
fn content_text(content: &Value) -> Option<&str> {
    match content {
        Value::String(s) => Some(s),
        Value::Array(parts) => parts.iter().find_map(|p| {
            let kind = p.get("type").and_then(Value::as_str);
            matches!(kind, None | Some("text" | "input_text"))
                .then(|| p.get("text").and_then(Value::as_str))
                .flatten()
        }),
        _ => None,
    }
}

/// A title an agent wrote, or `None` for the placeholder it writes before
/// it has one.
fn real_title(title: Option<&str>) -> Option<String> {
    title
        .filter(|t| !matches!(*t, "New Session" | "Untitled"))
        .map(str::to_string)
}

/// Pi and Oh My Pi: a `session` header (after Oh My Pi's fixed-width
/// `title` line, when it has one), then the conversation.
pub(crate) fn parse_pi(
    agent: CLIAgent,
    head: &str,
    tail: &str,
    updated: u64,
) -> Option<PastSession> {
    let mut id = None;
    let mut cwd = None;
    let mut titled = None;
    let mut first_prompt = None;
    for record in records(head) {
        match str_field(&record, "type") {
            Some("title") if titled.is_none() => titled = real_title(str_field(&record, "title")),
            Some("session") if id.is_none() => {
                if agent == CLIAgent::OhMyPi && record.get("seanceSource").is_some() {
                    return None;
                }
                id = str_field(&record, "id").map(str::to_string);
                cwd = str_field(&record, "cwd").map(PathBuf::from);
                if titled.is_none() {
                    titled = real_title(str_field(&record, "title"));
                }
            }
            Some("message") if first_prompt.is_none() => {
                let message = record.get("message");
                if message.and_then(|m| str_field(m, "role")) == Some("user") {
                    first_prompt = message
                        .and_then(|m| m.get("content"))
                        .and_then(content_text)
                        .and_then(prompt_title);
                }
            }
            Some("session_init") if agent == CLIAgent::OhMyPi => return None,
            _ => {}
        }
    }
    // A name given in the session outranks the title line, which Oh My Pi
    // rewrites in place and so is already the newest.
    let mut named = None;
    let mut summary = None;
    for record in records(tail).rev().chain(records(head).rev()) {
        match str_field(&record, "type") {
            Some("session_info") if named.is_none() => {
                named = str_field(&record, "name").map(str::to_string);
            }
            Some("compaction") if summary.is_none() => {
                let field = if agent == CLIAgent::Pi {
                    "summary"
                } else {
                    "shortSummary"
                };
                summary = str_field(&record, field).and_then(prompt_title);
            }
            _ => {}
        }
    }
    Some(PastSession {
        scope: None,
        agent,
        id: id?,
        cwd,
        title: named.or(titled).or(summary).or(first_prompt)?,
        branch: None,
        updated,
    })
}

/// Kimi Code's native session metadata; subagent wires are never listed.
pub(crate) fn parse_kimi(dir: &str, text: &str, updated: u64) -> Option<PastSession> {
    let state: Value = serde_json::from_str(text).ok()?;
    if state.get("archived").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let title = real_title(str_field(&state, "title"))
        .or_else(|| str_field(&state, "lastPrompt").and_then(prompt_title))?;
    Some(PastSession {
        scope: None,
        agent: CLIAgent::Kimi,
        id: str_field(&state, "id").unwrap_or(dir).to_string(),
        cwd: str_field(&state, "cwd")
            .or_else(|| str_field(&state, "workDir"))
            .map(PathBuf::from),
        title,
        branch: None,
        updated: state
            .get("updatedAt")
            .and_then(Value::as_u64)
            .map(|ms| ms / 1000)
            .unwrap_or(updated),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(v: serde_json::Value) -> String {
        format!("{v}\n")
    }

    #[test]
    fn omp_profile_history_keeps_effective_home_and_custom_session_directory() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let sessions = directory.path().join("custom sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join("one.jsonl"), line(serde_json::json!({"type":"session","id":"native","cwd":"/project","title":"中文历史"}))).unwrap();
        let environment = std::collections::BTreeMap::from([
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("PI_CONFIG_DIR".into(), ".omp-custom".into()),
            ("OMP_PROFILE".into(), "work".into()),
            (
                "PI_CODING_AGENT_DIR".into(),
                directory
                    .path()
                    .join("ignored-for-named-profile")
                    .to_string_lossy()
                    .into_owned(),
            ),
        ]);
        let mut roots = Roots::from_environment(&home, &environment).unwrap();
        assert_eq!(roots.omp, home.join(".omp-custom/profiles/work/agent"));
        roots.omp_session_dir = Some(sessions.clone());
        let session = scan(&roots)
            .into_iter()
            .find(|s| s.agent == CLIAgent::OhMyPi)
            .unwrap();
        assert_eq!(session.title, "中文历史");
        let scope = session.scope.unwrap();
        assert_eq!(scope.profile.as_deref(), Some("work"));
        assert_eq!(scope.session_file, Some(sessions.join("one.jsonl")));
        assert_eq!(scope.environment.get("HOME"), environment.get("HOME"));
        let mut default = environment.clone();
        default.insert("OMP_PROFILE".into(), String::new());
        default.insert("PI_PROFILE".into(), "must-not-win".into());
        assert_eq!(
            Roots::from_environment(&home, &default)
                .unwrap()
                .omp_scope
                .profile
                .as_deref(),
            Some("default")
        );
    }

    #[test]
    fn kimi_history_rejects_cwd_relative_homes_without_an_executing_pane() {
        let home = tempfile::tempdir().unwrap();
        for selected in ["", ".", "relative-home"] {
            let environment =
                std::collections::BTreeMap::from([("KIMI_CODE_HOME".into(), selected.into())]);
            assert!(
                Roots::from_environment(home.path(), &environment).is_err(),
                "{selected:?} must not scan the daemon's cwd or default home"
            );
        }
    }

    #[test]
    fn configured_pi_and_kimi_history_are_listed_without_an_observed_pane() {
        use crate::core::agent_hooks::HookTarget;
        let home = tempfile::tempdir().unwrap();
        let host = crate::host::local::LocalHost::new();
        let q = |value: &Path| {
            crate::core::shell_quote::quote_for_shell(&value.to_string_lossy(), Some("sh"))
        };
        let selectors = format!("HOME={} USERPROFILE={}", q(home.path()), q(home.path()));
        let pi_store = home.path().join("pi-sessions");
        let kimi_home = home.path().join("kimi-home");
        let target = HookTarget::remote(&*host, home.path().into()).with_launch_commands(&HashMap::from([
            ("omp".into(), format!("{selectors} omp")),
            ("pi".into(), format!("{selectors} PI_CODING_AGENT_DIR=~/pi-private pi --offline --session-dir {}", q(&pi_store))),
            ("kimi".into(), format!("{selectors} KIMI_CODE_HOME={} kimi --model native", q(&kimi_home))),
        ]));
        std::fs::create_dir_all(&pi_store).unwrap();
        std::fs::write(
            pi_store.join("pi-source.jsonl"),
            lines(&[
                json!({"type":"session","id":"configured-pi","cwd":"/project"}),
                json!({"type":"message","message":{"role":"user","content":"scoped task"}}),
            ]),
        )
        .unwrap();
        let kimi_file = kimi_home.join("sessions/wd_project/configured-kimi/state.json");
        std::fs::create_dir_all(kimi_file.parent().unwrap()).unwrap();
        std::fs::write(&kimi_file, json!({"id":"configured-kimi","cwd":"/project","title":"scoped Kimi","updatedAt":1792320000123u64}).to_string()).unwrap();
        let sessions = scan(&Roots::from_target(home.path(), &target).unwrap());
        let pi = sessions
            .iter()
            .find(|session| session.id == "configured-pi")
            .unwrap();
        let kimi = sessions
            .iter()
            .find(|session| session.id == "configured-kimi")
            .unwrap();
        assert_eq!(
            pi.scope.as_ref().unwrap().session_dir.as_deref(),
            Some(pi_store.as_path())
        );
        assert_eq!(
            pi.scope
                .as_ref()
                .unwrap()
                .environment
                .get("PI_CODING_AGENT_DIR"),
            Some(
                &home
                    .path()
                    .join("pi-private")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        assert_eq!(
            kimi.scope.as_ref().unwrap().session_file.as_deref(),
            Some(kimi_file.as_path())
        );
        assert_eq!(
            kimi.scope
                .as_ref()
                .unwrap()
                .environment
                .get("KIMI_CODE_HOME"),
            Some(&kimi_home.to_string_lossy().into_owned())
        );
    }

    #[test]
    fn kimi_history_keeps_the_selected_home_and_native_timestamp() {
        let home = tempfile::tempdir().unwrap();
        let roots = Roots::under(home.path());
        let file = roots
            .kimi
            .join("sessions/wd_project_123/session-k/state.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, serde_json::json!({
            "id":"session-k", "cwd":"/project", "title":"New Session", "lastPrompt":"检查主会话",
            "updatedAt":1792320000123u64, "archived":false
        }).to_string()).unwrap();
        let selected = scan(&roots)
            .into_iter()
            .find(|session| session.agent == CLIAgent::Kimi)
            .unwrap();
        assert_eq!(selected.id, "session-k");
        assert_eq!(selected.title, "检查主会话");
        assert_eq!(selected.updated, 1792320000);
        let scope = selected.scope.unwrap();
        assert_eq!(scope.session_file.as_deref(), Some(file.as_path()));
        assert_eq!(
            scope.environment.get("KIMI_CODE_HOME"),
            Some(&roots.kimi.to_string_lossy().into_owned())
        );
    }

    #[test]
    fn identical_omp_ids_in_separate_stores_keep_the_selected_file_and_scope() {
        let home = tempfile::tempdir().unwrap();
        let one = Roots::under(&home.path().join("one"));
        let two = Roots::under(&home.path().join("two"));
        for roots in [&one, &two] {
            let dir = roots.omp.join("sessions/project");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("saved.jsonl"), line(serde_json::json!({
                "type": "session", "id": "same-native-id", "cwd": "/project", "title": "selected history"
            }))).unwrap();
        }
        let first = scan(&one)
            .into_iter()
            .find(|s| s.agent == CLIAgent::OhMyPi)
            .unwrap();
        let second = scan(&two)
            .into_iter()
            .find(|s| s.agent == CLIAgent::OhMyPi)
            .unwrap();
        assert_eq!(first.id, second.id);
        let first_scope = first.scope.unwrap();
        let second_scope = second.scope.unwrap();
        assert_eq!(
            first_scope.session_file,
            Some(one.omp.join("sessions/project/saved.jsonl"))
        );
        assert_eq!(
            second_scope.session_file,
            Some(two.omp.join("sessions/project/saved.jsonl"))
        );
        assert_eq!(first_scope.profile.as_deref(), Some("default"));
        assert_eq!(
            first_scope.environment.get("PI_CODING_AGENT_DIR"),
            Some(&one.omp.to_string_lossy().into_owned())
        );
    }

    #[test]
    fn pi_history_includes_recorded_pane_private_storage_and_config_root() {
        let home = tempfile::tempdir().unwrap();
        let mut roots = Roots::under(home.path());
        let store = home.path().join("private-sessions");
        let config = home
            .path()
            .join("private-config")
            .to_string_lossy()
            .into_owned();
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(
            store.join("source.jsonl"),
            lines(&[
                json!({"type":"session","id":"private-source","cwd":"/project"}),
                json!({"type":"message","message":{"role":"user","content":"private task"}}),
            ]),
        )
        .unwrap();
        roots
            .pi_scopes
            .push(crate::host::session_operations::SessionScope {
                session_dir: Some(store.clone()),
                environment: std::collections::BTreeMap::from([(
                    "PI_CODING_AGENT_DIR".into(),
                    config.clone(),
                )]),
                ..Default::default()
            });
        let session = scan(&roots)
            .into_iter()
            .find(|session| session.id == "private-source")
            .unwrap();
        let scope = session.scope.unwrap();
        assert_eq!(scope.session_file, Some(store.join("source.jsonl")));
        assert_eq!(scope.environment.get("PI_CODING_AGENT_DIR"), Some(&config));
    }

    #[test]
    fn pi_storage_precedence_uses_the_executing_home_and_project_directory() {
        let home = tempfile::tempdir().unwrap();
        let agent = home.path().join("agent");
        let cwd = home.path().join("project");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::create_dir_all(cwd.join(".pi")).unwrap();
        std::fs::write(
            agent.join("settings.json"),
            r#"{"sessionDir":"~/global-sessions"}"#,
        )
        .unwrap();
        assert_eq!(
            pi_session_dir(&agent, home.path(), Some(&cwd), None),
            Some(home.path().join("global-sessions"))
        );
        std::fs::write(
            cwd.join(".pi/settings.json"),
            r#"{"sessionDir":"project-sessions"}"#,
        )
        .unwrap();
        assert_eq!(
            pi_session_dir(&agent, home.path(), Some(&cwd), None),
            Some(cwd.join("project-sessions"))
        );
        assert_eq!(
            pi_session_dir(&agent, home.path(), Some(&cwd), Some("~/env-sessions")),
            Some(home.path().join("env-sessions"))
        );
    }

    #[test]
    fn pi_history_reads_flat_custom_storage_without_mixing_the_default_store() {
        let home = tempfile::tempdir().unwrap();
        let mut roots = Roots::under(home.path());
        let store = home.path().join("会话 store");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(
            store.join("source.jsonl"),
            lines(&[
                json!({"type":"session","id":"same-id","cwd":"/project"}),
                json!({"type":"message","message":{"role":"user","content":"custom store task"}}),
            ]),
        )
        .unwrap();
        roots.pi_sessions = Some(store.clone());
        let session = scan(&roots)
            .into_iter()
            .find(|s| s.agent == CLIAgent::Pi)
            .unwrap();
        assert_eq!(
            (session.id.as_str(), session.title.as_str()),
            ("same-id", "custom store task")
        );
        assert_eq!(
            session.scope.unwrap().session_file,
            Some(store.join("source.jsonl"))
        );
    }

    use serde_json::json;

    fn lines(records: &[serde_json::Value]) -> String {
        records.iter().map(|r| line(r.clone())).collect()
    }

    #[test]
    fn injected_context_in_front_of_a_prompt_is_not_its_title() {
        assert_eq!(
            prompt_title("<system-reminder>\ntoday is Monday\n</system-reminder>\nfix the build"),
            Some("fix the build".into())
        );
        assert_eq!(prompt_title("<command-name>/clear</command-name>"), None);
    }

    #[test]
    fn pi_history_uses_the_published_compaction_summary_and_latest_name() {
        let head = lines(&[
            json!({"type":"session","version":3,"id":"pi-source","cwd":"/project"}),
            json!({"type":"message","message":{"role":"user","content":"original task"}}),
        ]);
        let tail = lines(&[
            json!({"type":"compaction","summary":"保留的上下文","shortSummary":"wrong omp field"}),
        ]);
        assert_eq!(
            parse_pi(CLIAgent::Pi, &head, &tail, 7).unwrap().title,
            "保留的上下文"
        );
        let named = format!(
            "{tail}{}",
            line(json!({"type":"session_info","name":"Explicit name"}))
        );
        assert_eq!(
            parse_pi(CLIAgent::Pi, &head, &named, 7).unwrap().title,
            "Explicit name"
        );
    }

    #[test]
    fn pi_and_oh_my_pi_read_the_header_after_an_optional_title_line() {
        let pi = lines(&[
            json!({"type": "session", "version": 3, "id": "p1", "cwd": "/pi"}),
            json!({"type": "custom_message", "content": "extension text"}),
            json!({"type": "message", "message": {"role": "user", "content": "write docs"}}),
        ]);
        let s = parse_pi(CLIAgent::Pi, &pi, "", 0).unwrap();
        assert_eq!((s.id.as_str(), s.title.as_str()), ("p1", "write docs"));
        let named = line(json!({"type": "session_info", "name": "Docs pass"}));
        assert_eq!(
            parse_pi(CLIAgent::Pi, &pi, &named, 0).unwrap().title,
            "Docs pass"
        );

        let omp = format!(
            "{}{pi}",
            line(json!({"type": "title", "v": 1, "title": "Auto title", "source": "auto"}))
        );
        let s = parse_pi(CLIAgent::OhMyPi, &omp, "", 0).unwrap();
        assert_eq!(
            (s.agent, s.title.as_str()),
            (CLIAgent::OhMyPi, "Auto title")
        );
    }

    #[test]
    fn kimi_reads_both_state_shapes_and_leaves_archived_ones_out() {
        let v2 = json!({"id": "session_k1", "cwd": "/k", "title": "New Session",
            "lastPrompt": "port the tests", "updatedAt": 1})
        .to_string();
        let s = parse_kimi("session_k1", &v2, 0).unwrap();
        assert_eq!(
            (s.id.as_str(), s.title.as_str()),
            ("session_k1", "port the tests")
        );
        let v0 = json!({"title": "Porting", "workDir": "/k0"}).to_string();
        let s = parse_kimi("dir-id", &v0, 0).unwrap();
        assert_eq!(
            (s.id.as_str(), s.cwd.as_deref()),
            ("dir-id", Some(Path::new("/k0")))
        );
        let archived = json!({"id": "x", "title": "t", "archived": true}).to_string();
        assert!(parse_kimi("x", &archived, 0).is_none());
    }

    #[test]
    fn history_lists_supported_sessions_without_touching_retired_data() {
        let home = tempfile::tempdir().unwrap();
        let retired = home.path().join(".claude/projects/old/old.jsonl");
        std::fs::create_dir_all(retired.parent().unwrap()).unwrap();
        let old = "retired user data must remain unchanged\n";
        std::fs::write(&retired, old).unwrap();
        let pi = home.path().join(".pi/agent/sessions/project/current.jsonl");
        std::fs::create_dir_all(pi.parent().unwrap()).unwrap();
        std::fs::write(&pi, concat!(
            "{\"type\":\"session\",\"id\":\"current-pi\",\"cwd\":\"/work\"}\n",
            "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"continue the real work\"}}\n"
        )).unwrap();
        let found = scan(&Roots::under(home.path()));
        assert_eq!(
            found
                .iter()
                .map(|s| (s.agent, s.id.as_str(), s.title.as_str()))
                .collect::<Vec<_>>(),
            vec![(CLIAgent::Pi, "current-pi", "continue the real work")]
        );
        assert_eq!(std::fs::read_to_string(&retired).unwrap(), old);
    }
}
