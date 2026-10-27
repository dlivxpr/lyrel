//! 负责 shell 选择、集成注入与平台启动环境，向 pane 交付可直接启动的配置。

use std::path::PathBuf;

use portable_pty::CommandBuilder;

use crate::daemon::protocol::{RemoteContext, RemoteKind, ShellSpec};
use crate::daemon::shell_integration;

#[cfg(windows)]
fn default_prog() -> CommandBuilder {
    CommandBuilder::new(crate::core::shells::windows_default_shell())
}

#[cfg(not(windows))]
fn default_prog() -> CommandBuilder {
    default_prog_with_override(detected_shell_override())
}

#[cfg(not(windows))]
fn default_prog_with_override(shell_override: Option<String>) -> CommandBuilder {
    let cmd = CommandBuilder::new_default_prog();
    let portable_shell = cmd.get_shell();
    if let Some(shell) = shell_override.filter(|shell| shell != &portable_shell) {
        return CommandBuilder::new(&shell);
    }
    cmd
}

#[cfg(not(windows))]
fn detected_shell_override() -> Option<String> {
    let path = std::env::var_os(crate::daemon::DETECTED_SHELL_ENV)?;
    usable_shell_path(path)
}

#[cfg(not(windows))]
fn usable_shell_path(path: std::ffi::OsString) -> Option<String> {
    let path = PathBuf::from(path);
    if path.as_os_str().is_empty() || !path.is_file() {
        return None;
    }
    path.into_os_string().into_string().ok()
}

#[cfg(windows)]
fn default_shell_name(_cmd: &CommandBuilder) -> String {
    crate::core::shells::windows_default_shell().to_string()
}

/// The shell `cmd` will actually run. `get_shell` names the login shell
/// whatever `cmd` holds, and the detected-shell override swaps the program
/// for another one: lyrel launched from bash with zsh as the login shell ran
/// bash with zsh's integration, which is none at all.
#[cfg(not(windows))]
fn default_shell_name(cmd: &CommandBuilder) -> String {
    match cmd.get_argv().first() {
        Some(program) if !cmd.is_default_prog() => program.to_string_lossy().into_owned(),
        _ => cmd.get_shell(),
    }
}

#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
struct ChosenShell {
    program: String,
    args: Vec<String>,
    args_are_lyrel_defaults: bool,
}

fn has_custom_args(chosen: Option<&ChosenShell>) -> bool {
    chosen.is_some_and(|c| !c.args.is_empty() && !c.args_are_lyrel_defaults)
}

fn choose_shell(
    spawn_override: Option<ShellSpec>,
    configured: Option<(String, Vec<String>)>,
) -> Option<ChosenShell> {
    spawn_override
        .map(|s| ChosenShell {
            program: s.program,
            args: s.args,
            args_are_lyrel_defaults: s.args_are_lyrel_defaults,
        })
        .or_else(|| {
            configured.map(|(program, args)| ChosenShell {
                program,
                args,
                args_are_lyrel_defaults: false,
            })
        })
}

fn apply_shell_integration(
    cmd: &mut CommandBuilder,
    resolved_program: &str,
    integration: &shell_integration::Injection,
) {
    if integration.replaces_argv || (cmd.is_default_prog() && !integration.args.is_empty()) {
        *cmd = CommandBuilder::new(resolved_program);
    }
    cmd.args(&integration.args);
    for (k, v) in &integration.env {
        cmd.env(k, v);
    }
}

pub(super) struct SpawnConfig {
    pub(super) cmd: CommandBuilder,
    pub(super) initial_cwd: Option<PathBuf>,
    pub(super) integration_dir: Option<PathBuf>,
    pub(super) remote: Option<RemoteContext>,
    /// The shell this pane actually got, after the override and the config have
    /// been resolved against each other. Recorded so the machine tree can name
    /// it — see [`crate::core::machine::PaneRecord::shell`].
    pub(super) shell: Option<ShellSpec>,
}

/// Why a configured shell cannot be run, in one sentence, before anything
/// tries. Without it a missing program arrives at the window wrapped four
/// deep — "daemon refused Spawn: spawn failed: Unable to spawn … (ENOENT: No
/// such file or directory)" — and the one fact that matters is buried in it.
///
/// Only a program given as a path can be checked here; a bare name is resolved
/// through PATH by the OS, and guessing at that would be worse than silence.
fn shell_program_problem(program: &str) -> Option<String> {
    let path = std::path::Path::new(program);
    if path.components().count() < 2 {
        return None;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return Some(format!("no such shell on this machine: {program}"));
    };
    if meta.is_dir() {
        return Some(format!("the configured shell is a directory: {program}"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return Some(format!("the configured shell is not executable: {program}"));
        }
    }
    None
}

pub(super) fn build_spawn_config(
    pane: u64,
    cwd: Option<PathBuf>,
    shell: Option<ShellSpec>,
    workspace: Option<&str>,
) -> anyhow::Result<SpawnConfig> {
    let initial_cwd = initial_working_directory(cwd);
    let configured = choose_shell(shell, crate::core::config::shell_command());
    if let Some(problem) = configured
        .as_ref()
        .and_then(|c| shell_program_problem(&c.program))
    {
        anyhow::bail!(problem);
    }
    let remote = wsl_remote_context(configured.as_ref());
    // Taken before the chosen shell is consumed by the command builder: what
    // goes in the tree is what was resolved here, not the possibly-empty
    // override the caller sent.
    let shell = configured.as_ref().map(|c| ShellSpec {
        program: c.program.clone(),
        args: c.args.clone(),
        args_are_lyrel_defaults: c.args_are_lyrel_defaults,
    });
    let (cmd, integration_dir) = build_shell_command(configured, &initial_cwd, pane, workspace)?;
    Ok(SpawnConfig {
        cmd,
        initial_cwd,
        integration_dir,
        remote,
        shell,
    })
}

fn wsl_remote_context(shell: Option<&ChosenShell>) -> Option<RemoteContext> {
    if !cfg!(windows) {
        return None;
    }
    let chosen = shell?;
    let base = std::path::Path::new(&chosen.program)
        .file_name()?
        .to_str()?
        .to_ascii_lowercase();
    if base.strip_suffix(".exe").unwrap_or(&base) != "wsl" {
        return None;
    }
    Some(RemoteContext {
        kind: RemoteKind::Wsl,
        argv: Vec::new(),
        // No `--distribution` means wsl.exe launches the default distro —
        // name it here, so every consumer of the context (the `\\wsl$`
        // completion route, paste-path rewriting, host labels) gets a real
        // distro instead of an empty placeholder.
        target: shell_integration::wsl_distro(&chosen.args)
            .or_else(crate::core::shells::default_wsl_distro)
            .unwrap_or_default(),
    })
}

fn build_shell_command(
    configured: Option<ChosenShell>,
    initial_cwd: &Option<PathBuf>,
    pane: u64,
    workspace: Option<&str>,
) -> anyhow::Result<(CommandBuilder, Option<PathBuf>)> {
    let mut cmd = match &configured {
        Some(chosen) => {
            let mut c = CommandBuilder::new(&chosen.program);
            c.args(&chosen.args);
            c
        }
        None => default_prog(),
    };
    let resolved_program = match &configured {
        Some(chosen) => chosen.program.clone(),
        None => default_shell_name(&cmd),
    };

    let integration = shell_integration::setup(
        Some(&resolved_program),
        configured.as_ref().map_or(&[][..], |c| c.args.as_slice()),
        has_custom_args(configured.as_ref()),
    );
    if let Some(integration) = &integration {
        apply_shell_integration(&mut cmd, &resolved_program, integration);
    }
    let integration_dir = integration.as_ref().and_then(|i| i.dir.clone());
    let launched = launched_shell_program(&cmd, &resolved_program);
    apply_common_command_setup(&mut cmd, initial_cwd, pane, workspace, &launched);
    Ok((cmd, integration_dir))
}

/// The program the pane is really about to exec.
///
/// `resolved_program` is what lyrel *decided* to run before shell integration
/// had its say, and for a default-prog builder that decision is
/// `CommandBuilder::get_shell()` — the passwd entry — even when
/// `default_prog()` swapped the builder for the shell that launched the daemon.
/// argv survives every rewrite on the way here, including the one an
/// argv-replacing injection performs, so it is the honest answer. An empty argv
/// means the builder is still the untouched default prog, which is precisely
/// the case `resolved_program` already describes.
fn launched_shell_program(cmd: &CommandBuilder, resolved_program: &str) -> String {
    cmd.get_argv()
        .first()
        .map(|argv0| argv0.to_string_lossy().into_owned())
        .unwrap_or_else(|| resolved_program.to_string())
}

fn initial_working_directory(cwd: Option<PathBuf>) -> Option<PathBuf> {
    let fallback = std::env::current_dir()
        .ok()
        .filter(|d| d != std::path::Path::new("/"))
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from));
    let forced = crate::core::config::working_directory_base();
    // A configured "Start in" path that is not a directory can never win the
    // pick below, so every new pane silently lands on the fallback and the
    // mistyped path reads as a lyrel bug (#601). Settings refuses to save one
    // now, but a hand-edited config.json can still hold one — name it, with
    // the reason, at the moment it actually costs something (an explicit cwd
    // that resolves never consults the config, so that case stays quiet).
    if let Some(dir) = &forced {
        if !dir.is_dir() && cwd.as_ref().is_none_or(|d| !d.is_dir()) {
            log::warn!(
                "configured working directory {} is not a directory; new panes fall back",
                dir.display()
            );
        }
    }
    [cwd, forced, fallback]
        .into_iter()
        .flatten()
        .find(|d| d.is_dir())
}

#[cfg(any(target_os = "macos", test))]
fn locale_fallback_is_needed(
    extra_env: &std::collections::HashMap<String, String>,
    mut inherited: impl FnMut(&str) -> Option<String>,
) -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"].iter().all(|key| {
        !extra_env.contains_key(*key) && inherited(key).as_deref().is_none_or(str::is_empty)
    })
}

#[cfg(target_os = "macos")]
const LOCALE_DEFINITION_DIR: &str = "/usr/share/locale";

#[cfg(any(target_os = "macos", test))]
const FALLBACK_CHARACTER_LOCALES: [&str; 2] = ["C.UTF-8", "en_US.UTF-8"];

/// The variable the locale fallback sets.
///
/// It has to be one a shell consults for *every* category. `LC_CTYPE` alone
/// fixes character handling and leaves collation, time and numbers at `C`; a
/// shell that then asks `setlocale(LC_COLLATE, "")` finds no variable to read
/// and bash warns `setlocale: LC_COLLATE: cannot change locale ()` once per
/// category. (zsh and fish swallow the failure, so they look fine while being
/// just as half-configured.) `LC_ALL` would also cover everything, but it wins
/// over every `LC_*` the user's own rc files set afterwards — `LANG` loses to
/// them, which is what a fallback should do.
#[cfg(any(target_os = "macos", test))]
const LOCALE_FALLBACK_KEY: &str = "LANG";

#[cfg(any(target_os = "macos", test))]
fn character_locale(identifier: Option<&str>, exists: impl Fn(&str) -> bool) -> Option<String> {
    identifier
        .and_then(posix_locale_stem)
        .map(|stem| format!("{stem}.UTF-8"))
        .into_iter()
        .chain(FALLBACK_CHARACTER_LOCALES.iter().map(|s| (*s).to_string()))
        .find(|candidate| exists(candidate))
}

#[cfg(any(target_os = "macos", test))]
fn posix_locale_stem(identifier: &str) -> Option<String> {
    let base = identifier.split('@').next()?;
    let mut parts = base.split(['_', '-']).filter(|s| !s.is_empty());
    let language = parts
        .next()
        .filter(|l| (2..=3).contains(&l.len()) && l.chars().all(|c| c.is_ascii_alphabetic()))?;
    let region = parts.next_back().filter(|r| {
        (r.len() == 2 && r.chars().all(|c| c.is_ascii_alphabetic()))
            || (r.len() == 3 && r.chars().all(|c| c.is_ascii_digit()))
    })?;
    Some(format!(
        "{}_{}",
        language.to_ascii_lowercase(),
        region.to_ascii_uppercase()
    ))
}

#[cfg(target_os = "macos")]
fn system_locale_identifier() -> Option<String> {
    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};
    use std::os::raw::c_void;

    type CFLocaleRef = *const c_void;
    unsafe extern "C" {
        fn CFLocaleCopyCurrent() -> CFLocaleRef;
        fn CFLocaleGetIdentifier(locale: CFLocaleRef) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
    }

    unsafe {
        let locale = CFLocaleCopyCurrent();
        if locale.is_null() {
            return None;
        }
        let identifier = CFLocaleGetIdentifier(locale);
        let out =
            (!identifier.is_null()).then(|| CFString::wrap_under_get_rule(identifier).to_string());
        CFRelease(locale);
        out
    }
}

const TERM_PROGRAM_NAME: &str = "lyrel";

/// The config dir this server runs on, not a socket path.
///
/// A client needs *two* endpoints — control and pane — and they are derived
/// from the config dir by rules that already live in this crate
/// (`control_socket_path`, `transport::socket_path_for`), including a hashed
/// fallback when the directory is too long for `sun_path`. Publishing one
/// socket path and letting the client reconstruct the other from it means a
/// second, independent derivation that can and did disagree with the first.
/// Publishing the directory instead means a CLI in this shell resolves both
/// endpoints with the very same functions the server used to open them.
const LYREL_CONFIG_DIR_ENV: &str = "LYREL_CONFIG_DIR";
pub(crate) const LYREL_PANE_ENV: &str = "LYREL_PANE";
const LYREL_WS_ENV: &str = "LYREL_WS";

fn config_dir_env() -> Option<String> {
    crate::core::config::config_dir_path().map(|p| p.display().to_string())
}

/// The shell the pane is running, not the one the passwd entry names.
///
/// Everything that spawns "the user's shell" — tmux's `default-shell`, `sudo
/// -s`, an editor's `!` escape, a coding agent picking a quoting dialect —
/// reads `$SHELL`. A GUI launch inherits the login session's snapshot of it,
/// so a pane told to run fish still advertised `/bin/zsh` and every one of
/// those consumers walked off into the wrong shell (#342). Inside a pane, the
/// shell lyrel was told to run *is* the user's shell, so that is what we name.
///
/// Note this is the opposite direction from `shells::login_shell()`, which
/// deliberately ignores `$SHELL` in favour of passwd: that answers "which
/// shell did the user log in with", which is still the login shell. This
/// answers "which shell is this pane", which is not the same question.
const SHELL_ENV: &str = "SHELL";

/// The absolute path to name in `$SHELL`, or `None` when there is none to
/// promise.
///
/// `$SHELL` is a POSIX contract, so the path rules here are POSIX ones rather
/// than the host's — this only ever describes a pane on a unix daemon (see the
/// caller), and hard-coding them keeps the resolution testable on any host.
///
/// A configured command may be bare (`fish`), because a bare command is what
/// the shell inventory keeps when it wants PATH to decide which install wins.
/// Consumers of `$SHELL` exec it directly, and many of them do so with a PATH
/// of their own, so a bare name there is not the same binary the pane runs.
/// Resolve it the way exec would, against the PATH the pane will inherit, and
/// leave `$SHELL` untouched when that finds nothing — a stale but honest login
/// shell beats a name that resolves somewhere else.
fn shell_env_path(
    program: &str,
    path_var: impl FnOnce() -> Option<String>,
    is_file: impl Fn(&str) -> bool,
) -> Option<String> {
    let program = program.trim();
    if program.is_empty() {
        return None;
    }
    if program.starts_with('/') {
        return Some(program.to_string());
    }
    // Anything else with a separator is relative to a working directory, and
    // the reader's is not necessarily the pane's. Only a bare name is worth
    // resolving.
    if program.contains('/') {
        return None;
    }
    path_var()?
        .split(':')
        .filter(|dir| dir.starts_with('/'))
        .map(|dir| format!("{}/{program}", dir.trim_end_matches('/')))
        .find(|candidate| is_file(candidate))
}

const CAPABILITY_ENV: [&str; 2] = ["TERM", "COLORTERM"];

fn names_capability_env(key: &str) -> bool {
    CAPABILITY_ENV.iter().any(|cap| {
        if cfg!(windows) {
            key.eq_ignore_ascii_case(cap)
        } else {
            key == *cap
        }
    })
}

fn pane_environment(
    extra_env: &std::collections::HashMap<String, String>,
    // Only Windows has a use for it — see the `COLORFGBG` block below.
    #[cfg_attr(not(windows), allow(unused_variables))] dark: bool,
    pane: u64,
    workspace: Option<&str>,
    shell: &str,
) -> Vec<(String, String)> {
    let version = env!("CARGO_PKG_VERSION");
    let mut env = vec![
        ("TERM".to_string(), "xterm-256color".to_string()),
        ("COLORTERM".to_string(), "truecolor".to_string()),
        (
            crate::core::agent_hooks::LYREL_ENV_MARKER.to_string(),
            version.to_string(),
        ),
        ("TERM_PROGRAM".to_string(), TERM_PROGRAM_NAME.to_string()),
        ("TERM_PROGRAM_VERSION".to_string(), version.to_string()),
        // lyrel renders OSC 8 links, but `supports-hyperlinks` (Claude Code,
        // most Node CLIs) only trusts a fixed TERM_PROGRAM list and strips
        // them otherwise; this is the override it honours. It is checked
        // before the isTTY test, so such a tool now writes OSC 8 into
        // `> file` and `| less` too. `FORCE_HYPERLINK=0` in the `env` config
        // or the shell's rc turns it back off.
        ("FORCE_HYPERLINK".to_string(), "1".to_string()),
        (LYREL_PANE_ENV.to_string(), pane.to_string()),
    ];
    #[cfg(windows)]
    if !extra_env
        .keys()
        .any(|key| key.eq_ignore_ascii_case("COLORFGBG"))
    {
        // The bundled ConPTY forwards OSC 11 queries, and `daemon::conpty`
        // refuses to start a pane without that pinned pair — so there is no
        // in-box conhost case left to cover for: a pane running at all is a
        // pane whose queries get answered. Keep the conventional hint for
        // applications that never learned to ask. An explicit user override
        // still wins, below.
        let colorfgbg = if dark { "15;0" } else { "0;15" };
        env.push(("COLORFGBG".to_string(), colorfgbg.to_string()));
    }
    if let Some(ws) = workspace {
        env.push((LYREL_WS_ENV.to_string(), ws.to_string()));
    }
    if let Some(dir) = config_dir_env() {
        env.push((LYREL_CONFIG_DIR_ENV.to_string(), dir));
    }
    // Names the pane's own history file; the integration snippet is what
    // decides to use it, once the user's rc has said where their history was.
    if let Some((key, value)) = crate::daemon::history::env_for(pane, shell) {
        env.push((key, value));
    }
    // Windows is deliberately left out. `SHELL` is not a native concept there —
    // neither cmd nor PowerShell reads it — and the tools that do read it are
    // the POSIX emulations: MSYS/Git Bash, Cygwin, and WSL, which take `WSLENV`
    // exports as Linux paths. All of them want a POSIX path, and every shell
    // this branch could name is a Windows one, so setting it would point them
    // at something they cannot exec. That also covers the WSL pane, whose
    // program is `wsl.exe` rather than a shell at all: the distro's own login
    // shell is the right answer inside it, and leaving `SHELL` alone is how
    // that answer survives.
    //
    // Resolution runs against the PATH the pane will actually inherit, so a
    // user who redirects PATH in their `env` block gets the binary that PATH
    // resolves rather than the daemon's.
    if !cfg!(windows)
        && let Some(path) = shell_env_path(
            shell,
            || {
                extra_env
                    .get("PATH")
                    .cloned()
                    .or_else(|| std::env::var("PATH").ok())
            },
            |candidate| std::path::Path::new(candidate).is_file(),
        )
    {
        env.push((SHELL_ENV.to_string(), path));
    }
    env.extend(
        extra_env
            .iter()
            .filter(|(k, _)| !names_capability_env(k))
            .map(|(k, v)| (k.clone(), v.clone())),
    );
    env
}

fn apply_common_command_setup(
    cmd: &mut CommandBuilder,
    initial_cwd: &Option<PathBuf>,
    pane: u64,
    workspace: Option<&str>,
    shell: &str,
) {
    if let Some(dir) = initial_cwd {
        cmd.cwd(dir);
        // A new pane inherits the directory the last one *reported*, which is
        // the logical path its shell was showing — `/tmp/x`, not the
        // `/private/tmp/x` the kernel resolves it to. Handing that to `cwd()`
        // alone loses the distinction: the child starts in the resolved
        // directory and its shell falls back to `getcwd()`, so one tab read
        // `/tmp/x` while every tab opened from it read `/private/tmp/x` — the
        // same directory under two names, side by side in the sidebar.
        //
        // `PWD` is how a shell is told which of a directory's names it arrived
        // by; it is what `cd` sets, and what Terminal.app and iTerm2 pass for
        // this reason. POSIX has the shell verify it — a `PWD` that does not
        // name the directory the process is really in is discarded, confirmed
        // here against zsh, bash, sh and fish — so this can correct the name
        // and cannot invent one.
        //
        // Unix only: a Windows shell that reads `PWD` at all wants it in its
        // own idiom (`/c/x` under Git Bash), so a native path would just be
        // discarded by the same check.
        #[cfg(unix)]
        cmd.env("PWD", dir);
    }
    // The AppImage runtime exports `ARGV0` (the path the image was launched
    // by) into lyrel's own environment, and the pane would inherit it. zsh
    // treats an exported `ARGV0` as the argv[0] of every external command it
    // runs, so each program in the pane is told it is `lyrel-….AppImage` — and
    // multiplexers like Volta's shims, which pick the tool from argv[0], fail
    // with "Could not find executable" (#1100). Nothing outside a single
    // command line ever wants it set, so the pane starts without it; the
    // configured `env` block, applied below, can still put it back.
    cmd.env_remove("ARGV0");

    let extra_env = crate::core::config::extra_env();

    // Windows hands every process a private copy of the environment at spawn
    // time and never updates it, so a daemon that has been running since before
    // an installer touched `HKCU\Environment` would give a brand-new pane its
    // startup `PATH` (#333). Re-read both hives here and pin the merge onto the
    // command; the pane-specific variables below (and the configured overrides
    // they carry) are applied afterwards and still win.
    #[cfg(windows)]
    for (k, v) in crate::daemon::windows_env::refreshed_pane_environment(&extra_env) {
        cmd.env(k, v);
    }

    let dark = crate::core::machine::appearance().dark;
    for (k, v) in pane_environment(&extra_env, dark, pane, workspace, shell) {
        cmd.env(k, v);
    }

    #[cfg(target_os = "macos")]
    if locale_fallback_is_needed(&extra_env, |key| std::env::var(key).ok())
        && let Some(locale) = character_locale(system_locale_identifier().as_deref(), |name| {
            std::path::Path::new(LOCALE_DEFINITION_DIR)
                .join(name)
                .is_dir()
        })
    {
        cmd.env(LOCALE_FALLBACK_KEY, locale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A pane inherits the directory the last one *reported* — the logical
    /// path, `/tmp/x` and not the `/private/tmp/x` the kernel resolves it to.
    /// `cwd()` alone loses that: the shell falls back to `getcwd()` and one
    /// tab reads `/tmp/x` while the tab opened from it reads `/private/tmp/x`.
    /// `PWD` is what carries the name across, and what the shell checks.
    /// An AppImage launch leaves `ARGV0` in the daemon's environment; a zsh
    /// that inherits it renames every command it runs, which breaks Volta's
    /// shims (#1100).
    #[test]
    fn the_appimage_runtimes_argv0_does_not_reach_the_pane() {
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.env("ARGV0", "./lyrel-26.9.5-linux-x86_64.AppImage");
        apply_common_command_setup(&mut cmd, &None, 1, None, "sh");
        assert_eq!(cmd.get_env("ARGV0"), None);
    }

    #[cfg(unix)]
    #[test]
    fn an_inherited_directory_keeps_the_name_it_was_reached_by() {
        let mut cmd = CommandBuilder::new("/bin/sh");
        // `CommandBuilder` starts from this process's environment, so `PWD`
        // already names wherever the daemon happens to be. The pane's own
        // directory has to win over it.
        let inherited = cmd.get_env("PWD").map(std::ffi::OsStr::to_owned);
        let dir = Some(std::path::PathBuf::from("/tmp"));
        apply_common_command_setup(&mut cmd, &dir, 1, None, "sh");
        assert_ne!(cmd.get_env("PWD").map(std::ffi::OsStr::to_owned), inherited);
        assert_eq!(
            cmd.get_env("PWD"),
            Some(std::ffi::OsStr::new("/tmp")),
            "the shell is told which name it arrived by"
        );
    }

    #[test]
    fn a_shell_that_cannot_run_is_named_once_not_wrapped_four_deep() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("not-a-shell");
        let problem = shell_program_problem(&missing.to_string_lossy()).expect("a problem");
        assert!(problem.contains("no such shell"), "{problem}");
        assert!(problem.contains("not-a-shell"), "{problem}");
        // One sentence: none of the layers this used to arrive wrapped in.
        assert!(!problem.contains("ENOENT"), "{problem}");
        assert!(!problem.contains("Unable to spawn"), "{problem}");

        // A directory is a different mistake and says so.
        let as_dir = dir.path().to_string_lossy().to_string();
        assert!(
            shell_program_problem(&as_dir)
                .expect("a problem")
                .contains("directory")
        );

        // A bare name goes through PATH; guessing at it here would be worse
        // than saying nothing.
        assert_eq!(shell_program_problem("zsh"), None);

        // Something real passes. The test binary is the one path guaranteed
        // to exist and carry the execute bit on every platform this runs on —
        // `/bin/sh` named a file that is simply absent on Windows, so the
        // check meant to prove a good shell passes proved the opposite there.
        let real = std::env::current_exe().expect("the running test binary");
        assert_eq!(shell_program_problem(&real.to_string_lossy()), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_shell_without_the_execute_bit_says_so() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("not-executable");
        std::fs::write(&path, b"#!/bin/sh\n").expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let problem = shell_program_problem(&path.to_string_lossy()).expect("a problem");
        assert!(problem.contains("not executable"), "{problem}");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert_eq!(shell_program_problem(&path.to_string_lossy()), None);
    }

    #[test]
    fn initial_working_directory_skips_paths_that_are_not_directories() {
        let real = std::env::temp_dir();
        assert!(real.is_dir(), "temp dir should exist");

        assert_eq!(
            initial_working_directory(Some(real.clone())),
            Some(real.clone())
        );

        for bogus in [
            "/home/someone/definitely-not-here",
            "/c/Users/definitely-not-here",
        ] {
            let got = initial_working_directory(Some(PathBuf::from(bogus)));
            assert_ne!(
                got.as_deref(),
                Some(Path::new(bogus)),
                "{bogus} is not a directory here and must not be handed to spawn"
            );
            if let Some(d) = got {
                assert!(d.is_dir(), "fallback {d:?} must be a real directory");
            }
        }

        let file = real.join("lyrel-iwd-probe");
        std::fs::write(&file, b"x").expect("write probe file");
        let got = initial_working_directory(Some(file.clone()));
        assert_ne!(got.as_deref(), Some(file.as_path()));
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn choose_shell_prefers_override_then_config_then_default() {
        let over = ShellSpec {
            program: "fish".into(),
            args: vec!["-l".into()],
            args_are_lyrel_defaults: true,
        };
        let cfg = ("zsh".to_string(), vec!["-i".to_string()]);

        assert_eq!(
            choose_shell(Some(over.clone()), Some(cfg.clone())),
            Some(ChosenShell {
                program: "fish".to_string(),
                args: vec!["-l".to_string()],
                args_are_lyrel_defaults: true,
            })
        );
        assert_eq!(
            choose_shell(None, Some(cfg.clone())),
            Some(ChosenShell {
                program: "zsh".to_string(),
                args: vec!["-i".to_string()],
                args_are_lyrel_defaults: false,
            })
        );
        assert_eq!(choose_shell(None, None), None);
    }

    #[test]
    fn only_user_authored_args_block_shell_integration() {
        let chosen = |args: Vec<&str>, lyrel: bool| ChosenShell {
            program: r"C:\Program Files\Git\bin\bash.exe".to_string(),
            args: args.into_iter().map(str::to_string).collect(),
            args_are_lyrel_defaults: lyrel,
        };

        assert!(!has_custom_args(Some(&chosen(vec!["-i", "-l"], true))));
        assert!(has_custom_args(Some(&chosen(vec!["-i", "-l"], false))));
        assert!(!has_custom_args(Some(&chosen(vec![], false))));
        assert!(!has_custom_args(Some(&chosen(vec![], true))));
        assert!(!has_custom_args(None));
    }

    #[cfg(windows)]
    #[test]
    fn wsl_panes_are_tagged_as_a_foreign_filesystem() {
        let spec = |program: &str, args: Vec<&str>| ChosenShell {
            program: program.to_string(),
            args: args.into_iter().map(str::to_string).collect(),
            args_are_lyrel_defaults: true,
        };

        let ctx = wsl_remote_context(Some(&spec(
            "wsl.exe",
            vec!["--distribution", "Ubuntu-24.04", "--cd", "~"],
        )))
        .expect("wsl.exe must be tagged");
        assert_eq!(ctx.kind, RemoteKind::Wsl);
        assert_eq!(ctx.target, "Ubuntu-24.04");
        assert!(ctx.argv.is_empty());

        assert_eq!(
            wsl_remote_context(Some(&spec("wsl.exe", vec!["-d", "Debian"])))
                .expect("short flag")
                .target,
            "Debian"
        );
        assert_eq!(
            wsl_remote_context(Some(&spec("wsl.exe", vec![])))
                .expect("default distro is still WSL")
                .target,
            crate::core::shells::default_wsl_distro().unwrap_or_default(),
            "no --distribution resolves to the machine's default distro"
        );
        assert_eq!(
            wsl_remote_context(Some(&spec("wsl.exe", vec!["--distribution=Arch"])))
                .expect("joined flag")
                .target,
            "Arch"
        );
        assert!(wsl_remote_context(Some(&spec(r"C:\Windows\System32\WSL.EXE", vec![]))).is_some());

        let from_config = choose_shell(None, Some(("wsl.exe".to_string(), Vec::new())));
        assert_eq!(
            wsl_remote_context(from_config.as_ref()).map(|c| c.kind),
            Some(RemoteKind::Wsl),
            "a configured wsl.exe is as much a WSL pane as a dropdown one"
        );

        assert!(wsl_remote_context(Some(&spec("powershell.exe", vec![]))).is_none());
        assert!(
            wsl_remote_context(Some(&spec(r"C:\Program Files\Git\bin\bash.exe", vec![]))).is_none()
        );
        assert!(wsl_remote_context(None).is_none());
    }

    #[test]
    fn arg_based_integration_rebuilds_default_shell_builder() {
        let mut cmd = CommandBuilder::new_default_prog();
        let injection = shell_integration::Injection {
            env: std::collections::HashMap::new(),
            args: vec!["-C".to_string(), "echo ready".to_string()],
            replaces_argv: false,
            dir: None,
        };

        apply_shell_integration(&mut cmd, "/bin/fish", &injection);

        assert!(!cmd.is_default_prog(), "argv can now be appended safely");
        let argv: Vec<_> = cmd
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(argv, vec!["/bin/fish", "-C", "echo ready"]);
    }

    #[test]
    fn env_only_integration_keeps_default_login_shell_builder() {
        let mut cmd = CommandBuilder::new_default_prog();
        let mut env = std::collections::HashMap::new();
        env.insert("ZDOTDIR".to_string(), "/tmp/lyrel-zdotdir-test".to_string());
        let injection = shell_integration::Injection {
            env,
            args: Vec::new(),
            replaces_argv: false,
            dir: None,
        };

        apply_shell_integration(&mut cmd, "/bin/zsh", &injection);

        assert!(
            cmd.is_default_prog(),
            "zsh still launches as the login shell"
        );
        assert_eq!(
            cmd.get_env("ZDOTDIR").and_then(|value| value.to_str()),
            Some("/tmp/lyrel-zdotdir-test")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn detected_shell_override_uses_explicit_command_builder() {
        let portable_shell = CommandBuilder::new_default_prog().get_shell();
        let detected_shell = format!("{portable_shell}-detected");
        let cmd = default_prog_with_override(Some(detected_shell.clone()));

        assert!(!cmd.is_default_prog());
        let argv: Vec<_> = cmd
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(argv, vec![detected_shell.clone()]);
        assert_eq!(default_shell_name(&cmd), detected_shell);
    }

    #[cfg(not(windows))]
    #[test]
    fn no_detected_shell_keeps_portable_login_default() {
        let cmd = default_prog_with_override(None);

        assert!(cmd.is_default_prog());
        assert!(!default_shell_name(&cmd).is_empty());
    }

    #[test]
    fn pane_environment_advertises_the_terminal_under_the_standard_names() {
        let env: std::collections::HashMap<_, _> = pane_environment(
            &std::collections::HashMap::new(),
            false,
            7,
            Some("ws-main"),
            "/bin/zsh",
        )
        .into_iter()
        .collect();
        let version = env!("CARGO_PKG_VERSION");

        assert_eq!(env.get("TERM_PROGRAM").map(String::as_str), Some("lyrel"));
        assert_eq!(env.get("FORCE_HYPERLINK").map(String::as_str), Some("1"));
        assert_eq!(
            env.get("TERM_PROGRAM_VERSION").map(String::as_str),
            Some(version)
        );
        assert_eq!(
            env.get(crate::core::agent_hooks::LYREL_ENV_MARKER)
                .map(String::as_str),
            Some(version)
        );
        assert_eq!(
            env.get("TERM").map(String::as_str),
            Some("xterm-256color"),
            "terminfo name is what the pane's decoder actually implements"
        );
    }

    #[test]
    fn pane_environment_hands_the_shell_its_own_address() {
        let env: std::collections::HashMap<_, _> = pane_environment(
            &std::collections::HashMap::new(),
            false,
            42,
            Some("ws-main"),
            "/bin/zsh",
        )
        .into_iter()
        .collect();

        assert_eq!(
            env.get(LYREL_PANE_ENV).map(String::as_str),
            Some("42"),
            "a CLI inside the pane needs its own pane id for address-free verbs"
        );
        assert_eq!(
            env.get(LYREL_WS_ENV).map(String::as_str),
            Some("ws-main"),
            "the workspace the spawn was filed under rides into the shell"
        );
        match config_dir_env() {
            Some(dir) => assert_eq!(
                env.get(LYREL_CONFIG_DIR_ENV),
                Some(&dir),
                "the shell is told which config dir this server runs on, so a CLI \
                 there resolves both endpoints the same way the server opened them"
            ),
            None => assert!(
                !env.contains_key(LYREL_CONFIG_DIR_ENV),
                "no resolvable config dir must not inject an empty LYREL_CONFIG_DIR"
            ),
        }

        let unfiled: std::collections::HashMap<_, _> = pane_environment(
            &std::collections::HashMap::new(),
            false,
            42,
            None,
            "/bin/zsh",
        )
        .into_iter()
        .collect();
        assert!(
            !unfiled.contains_key(LYREL_WS_ENV),
            "a pane outside any workspace must not claim one"
        );
    }

    #[cfg(unix)]
    #[test]
    fn pane_environment_names_the_shell_the_pane_actually_runs() {
        // The reported bug: a pane running fish still advertised the passwd
        // login shell, so tmux, `sudo -s` and agents inside it all picked zsh.
        let env: std::collections::HashMap<_, _> = pane_environment(
            &std::collections::HashMap::new(),
            false,
            1,
            None,
            "/opt/homebrew/bin/fish",
        )
        .into_iter()
        .collect();

        assert_eq!(
            env.get(SHELL_ENV).map(String::as_str),
            Some("/opt/homebrew/bin/fish"),
            "$SHELL must name the shell the pane is actually running"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unconfigured_pane_still_announces_the_shell_it_runs() {
        // No configured shell means the pane runs the login shell, and naming
        // it is both harmless and correct: it is what the pane is running.
        let cmd = build_shell_command(None, &Some(PathBuf::from("/tmp")), 1, None)
            .expect("build default shell command")
            .0;
        let shell = cmd
            .get_env(SHELL_ENV)
            .and_then(|v| v.to_str())
            .map(str::to_string)
            .expect("the default shell resolves to an absolute path");

        assert!(
            shell.starts_with('/'),
            "$SHELL must be an absolute path, got {shell}"
        );
        assert_eq!(
            shell,
            default_shell_name(&CommandBuilder::new_default_prog()),
            "an unconfigured pane announces the very shell it was about to run"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_configured_shell_variable_outranks_the_shell_lyrel_launched() {
        // Same precedence contract as TERM_PROGRAM: lyrel describes the pane,
        // the user's own `env` block gets the last word.
        let configured = [("SHELL".to_string(), "/bin/ksh".to_string())]
            .into_iter()
            .collect();

        let applied: std::collections::HashMap<_, _> =
            pane_environment(&configured, false, 1, None, "/opt/homebrew/bin/fish")
                .into_iter()
                .collect();

        assert_eq!(applied.get(SHELL_ENV).map(String::as_str), Some("/bin/ksh"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_panes_are_left_without_a_shell_variable() {
        let applied = pane_environment(
            &std::collections::HashMap::new(),
            false,
            1,
            None,
            r"C:\Program Files\PowerShell\7\pwsh.exe",
        );

        assert!(
            !applied
                .iter()
                .any(|(key, _)| key.eq_ignore_ascii_case(SHELL_ENV)),
            "$SHELL is a POSIX contract: the MSYS, Cygwin and WSL tools that \
             read it on Windows want a POSIX path, and a Windows one would \
             point them at a shell they cannot exec"
        );
    }

    #[test]
    fn shell_env_path_only_ever_answers_an_absolute_path() {
        let never = |_: &str| false;
        let path = || Some("/usr/local/bin:/usr/bin:relative/bin".to_string());

        assert_eq!(
            shell_env_path("/opt/homebrew/bin/fish", path, never).as_deref(),
            Some("/opt/homebrew/bin/fish"),
            "an absolute program is already the answer, PATH untouched"
        );
        assert_eq!(
            shell_env_path("fish", path, |c| c == "/usr/local/bin/fish").as_deref(),
            Some("/usr/local/bin/fish"),
            "a bare command must be resolved the way exec would resolve it"
        );
        assert_eq!(
            shell_env_path("fish", path, never),
            None,
            "an unresolvable bare command must leave $SHELL alone rather than \
             hand a consumer a name it may resolve differently"
        );
        assert_eq!(
            shell_env_path("./fish", path, |_| true),
            None,
            "a cwd-relative program is meaningless to a process with another cwd"
        );
        assert_eq!(shell_env_path("  ", path, |_| true), None);
        assert_eq!(
            shell_env_path("fish", || None, |_| true),
            None,
            "no PATH to search means nothing to promise"
        );
    }

    #[test]
    fn the_shell_we_announce_is_the_argv_we_are_about_to_exec() {
        let mut configured = CommandBuilder::new("/opt/homebrew/bin/fish");
        configured.args(["-l"]);
        assert_eq!(
            launched_shell_program(&configured, "/bin/zsh"),
            "/opt/homebrew/bin/fish",
            "a builder with an argv has already overruled whatever lyrel resolved"
        );

        assert_eq!(
            launched_shell_program(&CommandBuilder::new_default_prog(), "/bin/zsh"),
            "/bin/zsh",
            "an untouched default prog is exactly what resolved_program describes"
        );
    }

    #[test]
    fn shell_env_path_ignores_relative_path_entries() {
        // A relative PATH entry resolves against the *pane's* cwd, which is not
        // ours, so it can never yield the absolute path $SHELL must hold.
        assert_eq!(
            shell_env_path("fish", || Some("bin:.".to_string()), |_| true),
            None
        );
    }

    #[test]
    fn pane_environment_lets_configured_env_override_identity_but_not_capability() {
        let configured = [
            ("TERM_PROGRAM", "iTerm.app"),
            ("TERM_PROGRAM_VERSION", "3.5.0"),
            ("TERM", "dumb"),
            ("COLORTERM", ""),
            ("EDITOR", "hx"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();

        let applied: std::collections::HashMap<_, _> =
            pane_environment(&configured, false, 1, None, "/bin/zsh")
                .into_iter()
                .collect();

        assert_eq!(
            applied.get("TERM_PROGRAM").map(String::as_str),
            Some("iTerm.app")
        );
        assert_eq!(
            applied.get("TERM_PROGRAM_VERSION").map(String::as_str),
            Some("3.5.0")
        );
        assert_eq!(applied.get("EDITOR").map(String::as_str), Some("hx"));
        assert_eq!(
            applied.get("TERM").map(String::as_str),
            Some("xterm-256color")
        );
        assert_eq!(
            applied.get("COLORTERM").map(String::as_str),
            Some("truecolor")
        );
    }

    #[cfg(windows)]
    #[test]
    fn pane_environment_advertises_light_and_dark_backgrounds() {
        let empty = std::collections::HashMap::new();
        let light: std::collections::HashMap<_, _> =
            pane_environment(&empty, false, 1, None, "pwsh.exe")
                .into_iter()
                .collect();
        let dark: std::collections::HashMap<_, _> =
            pane_environment(&empty, true, 1, None, "pwsh.exe")
                .into_iter()
                .collect();

        assert_eq!(light.get("COLORFGBG").map(String::as_str), Some("0;15"));
        assert_eq!(dark.get("COLORFGBG").map(String::as_str), Some("15;0"));
    }

    #[cfg(windows)]
    #[test]
    fn configured_colorfgbg_wins_case_insensitively() {
        let configured = [("ColorFgBg".to_string(), "3;4".to_string())]
            .into_iter()
            .collect();
        let applied = pane_environment(&configured, false, 1, None, "pwsh.exe");

        assert!(!applied.iter().any(|(key, _)| key == "COLORFGBG"));
        assert!(
            applied
                .iter()
                .any(|(key, value)| key == "ColorFgBg" && value == "3;4")
        );
    }

    #[cfg(windows)]
    #[test]
    fn pane_environment_capability_keys_cannot_be_overridden_by_recasing() {
        let configured = [("Term", "dumb"), ("ColorTerm", ""), ("term_program", "x")]
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();

        let applied = pane_environment(&configured, false, 1, None, "pwsh.exe");

        assert!(
            !applied.iter().any(|(k, _)| k == "Term" || k == "ColorTerm"),
            "a recased capability key must be filtered out, or it would land \
             in the same case-folded slot and win by coming later"
        );
        let get = |key: &str| {
            applied
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("TERM"), Some("xterm-256color"));
        assert_eq!(get("COLORTERM"), Some("truecolor"));
        assert_eq!(get("term_program"), Some("x"));
    }

    #[test]
    fn locale_fallback_respects_inherited_and_configured_environments() {
        let environment = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect::<std::collections::HashMap<_, _>>()
        };
        let fallback_is_needed = |extra: &[(&str, &str)], parent: &[(&str, &str)]| {
            let extra = environment(extra);
            let parent = environment(parent);
            locale_fallback_is_needed(&extra, |key| parent.get(key).cloned())
        };

        assert!(fallback_is_needed(&[], &[]));
        assert!(fallback_is_needed(&[], &[("LANG", "")]));
        assert!(!fallback_is_needed(&[], &[("LANG", "en_US.UTF-8")]));
        assert!(!fallback_is_needed(&[], &[("LC_ALL", "C")]));
        assert!(!fallback_is_needed(&[("LC_CTYPE", "UTF-8")], &[]));
        assert!(!fallback_is_needed(&[("LC_CTYPE", "")], &[]));
        assert!(!fallback_is_needed(
            &[("LANG", "")],
            &[("LANG", "en_US.UTF-8")]
        ));
    }

    #[test]
    fn locale_fallback_sets_a_variable_that_backs_every_category() {
        let injected =
            std::iter::once((LOCALE_FALLBACK_KEY.to_string(), "en_US.UTF-8".to_string()))
                .collect::<std::collections::HashMap<_, _>>();

        // Whatever we inject has to satisfy the check that gated it, or every
        // pane would keep re-deriving a fallback that is already in place.
        assert!(!locale_fallback_is_needed(&injected, |_| None));

        // And it has to back every category, not just character handling — see
        // LOCALE_FALLBACK_KEY. LC_CTYPE here would leave a shell warning about
        // LC_COLLATE and friends on every launch.
        assert_eq!(LOCALE_FALLBACK_KEY, "LANG");
    }

    #[test]
    fn posix_locale_stem_drops_script_and_keyword_subtags() {
        let stem = |id: &str| posix_locale_stem(id);

        assert_eq!(stem("en_US").as_deref(), Some("en_US"));
        assert_eq!(stem("en-US").as_deref(), Some("en_US"));
        assert_eq!(stem("zh_Hans_CN").as_deref(), Some("zh_CN"));
        assert_eq!(
            stem("zh_Hans_CN@calendar=gregorian").as_deref(),
            Some("zh_CN")
        );
        assert_eq!(stem("es_419").as_deref(), Some("es_419"));
        assert_eq!(stem("EN_us").as_deref(), Some("en_US"));

        assert_eq!(stem("zh"), None);
        assert_eq!(stem("zh_Hans"), None);
        assert_eq!(stem(""), None);
        assert_eq!(stem("@calendar=gregorian"), None);
    }

    #[test]
    fn character_locale_only_returns_installed_locales() {
        let installed = |names: &'static [&'static str]| move |n: &str| names.contains(&n);

        assert_eq!(
            character_locale(Some("zh_Hans_CN"), installed(&["zh_CN.UTF-8", "C.UTF-8"])),
            Some("zh_CN.UTF-8".to_string())
        );
        assert_eq!(
            character_locale(Some("en_CN"), installed(&["C.UTF-8", "en_US.UTF-8"])),
            Some("C.UTF-8".to_string())
        );
        assert_eq!(
            character_locale(Some("en_CN"), installed(&["en_US.UTF-8"])),
            Some("en_US.UTF-8".to_string())
        );
        assert_eq!(
            character_locale(None, installed(&["C.UTF-8", "en_US.UTF-8"])),
            Some("C.UTF-8".to_string())
        );
        assert_eq!(character_locale(Some("zh_Hans_CN"), installed(&[])), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn derived_character_locale_is_installed_on_this_machine() {
        let exists = |name: &str| {
            std::path::Path::new(LOCALE_DEFINITION_DIR)
                .join(name)
                .is_dir()
        };
        let locale = character_locale(system_locale_identifier().as_deref(), exists)
            .expect("macOS always ships en_US.UTF-8");
        assert!(exists(&locale), "derived {locale} is not installed");
        assert!(
            locale.ends_with(".UTF-8"),
            "derived {locale} is not a UTF-8 locale"
        );
    }

    #[test]
    fn spawned_shell_carries_the_lyrel_marker() {
        let cmd = build_shell_command(None, &Some(PathBuf::from("/tmp")), 42, Some("ws-main"))
            .expect("build default shell command")
            .0;
        let lyrel = cmd
            .get_env(crate::core::agent_hooks::LYREL_ENV_MARKER)
            .and_then(|v| v.to_str());
        assert_eq!(
            lyrel,
            Some(env!("CARGO_PKG_VERSION")),
            "the daemon must inject LYREL into every spawned shell"
        );
        assert_eq!(
            cmd.get_env(LYREL_PANE_ENV).and_then(|v| v.to_str()),
            Some("42"),
            "the daemon must tell every spawned shell which pane it is"
        );
        assert_eq!(
            cmd.get_env(LYREL_WS_ENV).and_then(|v| v.to_str()),
            Some("ws-main"),
            "the daemon must tell every spawned shell which workspace filed it"
        );
        if let Some(dir) = config_dir_env() {
            assert_eq!(
                cmd.get_env(LYREL_CONFIG_DIR_ENV).and_then(|v| v.to_str()),
                Some(dir.as_str()),
                "the daemon must tell every spawned shell which config dir it serves"
            );
        }
    }
}
