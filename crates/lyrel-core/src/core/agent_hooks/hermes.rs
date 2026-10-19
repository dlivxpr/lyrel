//! Hermes 0.22.2 public plain-CLI observer bridge.
use super::*;

const MARKER: &str = "agent-hook hermes";
const MANIFEST: &str = "# lyrel agent-hook hermes v2\nname: lyrel\nversion: 2.0.0\ndescription: lyrel plain CLI state and session identity\n";

pub(super) fn home(target: &HookTarget) -> io::Result<PathBuf> {
    let environment = target.agent_environment(HookAgent::Hermes)?;
    resolved_home(target, &environment, target.configured_hermes_profile())
}

pub(super) fn resolved_home(
    target: &HookTarget,
    environment: &std::collections::BTreeMap<String, String>,
    profile: Option<&str>,
) -> io::Result<PathBuf> {
    let mut root = if let Some(home) = environment.get("HERMES_HOME").filter(|v| !v.is_empty()) {
        PathBuf::from(home)
    } else {
        let windows = target.host.separator() == '\\';
        let user_home = target.agent_home(environment)?;
        let suffix = environment
            .get("HERMES_DATA_DIR_SUFFIX")
            .map(String::as_str)
            .unwrap_or("");
        if windows {
            let base = environment
                .get("LOCALAPPDATA")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| target.under(&user_home, &["AppData", "Local"]));
            target.host.join(&base, &format!("hermes{suffix}"))
        } else {
            target.host.join(&user_home, &format!(".hermes{suffix}"))
        }
    };
    if !target.host.is_absolute(&root) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Hermes home must be an absolute path on the selected Host",
        ));
    }
    // Explicit profile homes are authoritative. A root home follows Hermes'
    // documented sticky active_profile selector, without changing that file.
    let spelling = root.to_string_lossy();
    if spelling
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .nth(1)
        == Some("profiles")
    {
        if profile.is_none() {
            return Ok(root);
        }
        let spelling = spelling.trim_end_matches(['/', '\\']);
        let parent = spelling.rfind(['/', '\\']).unwrap();
        let grandparent = spelling[..parent].rfind(['/', '\\']).unwrap();
        root = PathBuf::from(&spelling[..grandparent]);
    }
    let selector = target.under(&root, &["active_profile"]);
    let name = if let Some(profile) = profile {
        profile.to_owned()
    } else {
        match target.read(&selector) {
            Ok(name) => name,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(root),
            Err(error) => return Err(error),
        }
    };
    let name = name
        .trim_start_matches('\u{feff}')
        .trim()
        .to_ascii_lowercase();
    if name.is_empty() || name == "default" {
        return Ok(root);
    }
    if name.len() > 64
        || !name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Hermes active_profile contains an invalid profile id",
        ));
    }
    let profile = target.under(&root, &["profiles", &name]);
    let tombstone = target.under(&root, &["profiles", ".deleted", &name]);
    match target.host.stat(&tombstone) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Hermes active profile has been deleted",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if !target.host.stat(&profile)?.is_dir {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "Hermes active profile is not a directory",
        ));
    }
    for marker in [
        "config.yaml",
        ".env",
        "SOUL.md",
        "profile.yaml",
        "auth.json",
        "state.db",
    ] {
        match target.host.stat(&target.host.join(&profile, marker)) {
            Ok(meta) if !meta.is_dir || meta.is_symlink => return Ok(profile),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Hermes active profile has no identity marker",
    ))
}

fn files(target: &HookTarget) -> io::Result<[(PathBuf, String); 2]> {
    let root = home(target)?;
    Ok([
        (
            target.under(&root, &["plugins", "lyrel", "__init__.py"]),
            plugin(target),
        ),
        (
            target.under(&root, &["plugins", "lyrel", "plugin.yaml"]),
            MANIFEST.into(),
        ),
    ])
}

pub(super) fn state(target: &HookTarget) -> HooksState {
    let Ok(files) = files(target) else {
        return HooksState::NotInstalled;
    };
    let states = files.map(|(path, content)| owned_file_state(target, &path, &content, MARKER));
    if states.iter().all(|s| *s == HooksState::Installed) {
        HooksState::Installed
    } else if states.iter().any(|s| *s != HooksState::NotInstalled) {
        HooksState::Outdated
    } else {
        HooksState::NotInstalled
    }
}

pub(super) fn install(target: &HookTarget) -> anyhow::Result<HookOutcome> {
    let files = files(target)?;
    // Check every file before modifying the bundle; never overwrite user code.
    for (path, _) in &files {
        match target.read(path) {
            Ok(contents) if !owned_file_is_marked(&contents, MARKER) => anyhow::bail!(
                "{} exists but wasn't written by lyrel; not touching it",
                path.display()
            ),
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    for (path, content) in files {
        owned_file_install(target, &path, &content, MARKER)?;
    }
    Ok(HookOutcome::Installed)
}

pub(super) fn uninstall(target: &HookTarget) -> anyhow::Result<HookOutcome> {
    let files = files(target)?;
    for (path, _) in &files {
        match target.read(path) {
            Ok(contents) if !owned_file_is_marked(&contents, MARKER) => anyhow::bail!(
                "{} wasn't written by lyrel; not touching it",
                path.display()
            ),
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    let mut removed = false;
    for (path, _) in files {
        removed |= owned_file_uninstall(target, &path, MARKER)? == HookOutcome::Removed;
    }
    Ok(if removed {
        HookOutcome::Removed
    } else {
        HookOutcome::NothingInstalled
    })
}

pub(super) fn plugin(target: &HookTarget) -> String {
    // JSON string syntax is also a Python string literal for these paths except
    // surrogate pairs; ensure Unicode is emitted literally by serde_json.
    let runner = serde_json::to_string(&target.exe.to_string_lossy()).unwrap();
    include_str!("hermes.py").replace("__LYREL_RUNNER__", &runner)
}
