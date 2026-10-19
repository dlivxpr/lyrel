//! 五种原生统计来源的非秘密关联；请求只在所选 Host 上按当前 launcher 重建。
use super::{Source, discover_hermes, discover_kimi, discover_omp, discover_opencode, discover_pi};
use crate::host::Host;
use crate::host::session_operations::{SessionOperation, SessionScope, durable_scope};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBinding {
    pub host: u64,
    pub agent: String,
    pub selector: String,
    pub scope: String,
    #[serde(default)]
    pub discovered: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_native_scope"
    )]
    native_scope: Option<SessionScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<PathBuf>,
    #[serde(skip)]
    live: Option<std::sync::Arc<Source>>,
    #[serde(skip)]
    service_urls: Vec<String>,
}
impl SourceBinding {
    pub fn key(&self) -> String {
        serde_json::to_string(&(self.host, &self.agent, &self.selector, &self.scope))
            .expect("source identity serializes")
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceBindings {
    added: Vec<SourceBinding>,
    excluded: Vec<SourceBinding>,
}

#[derive(Clone, Debug)]
pub struct SourceFailure {
    pub binding: SourceBinding,
    pub reason: String,
}

#[derive(Default)]
pub struct ResolvedSources {
    pub sources: Vec<Source>,
    pub bindings: Vec<SourceBinding>,
    pub issues: Vec<SourceFailure>,
}

impl SourceBindings {
    pub fn is_discovered(&self, binding: &SourceBinding) -> bool {
        binding.discovered
    }

    pub fn add(
        &mut self,
        host: &dyn Host,
        agent: &str,
        selector: &str,
        launch: &HashMap<String, String>,
    ) -> io::Result<SourceBinding> {
        let mut binding = SourceBinding {
            host: host.id().0,
            agent: agent.into(),
            selector: selector.into(),
            scope: String::new(),
            discovered: false,
            native_scope: None,
            cwd: None,
            live: None,
            service_urls: Vec::new(),
        };
        // Validate before discovery so invalid selectors never read another/default source.
        let source = self.check(host, &binding, launch)?;
        binding = binding_for(host, &source, false)?;
        for existing in &self.added {
            if overlaps(host, existing, &binding) {
                return Err(invalid(
                    "statistics source overlaps an existing association",
                ));
            }
            if existing.agent == "opencode"
                && binding.agent == "opencode"
                && existing.host == binding.host
                && let Ok(verified) = self.check(host, existing, launch)
                && overlaps(host, &binding_for(host, &verified, false)?, &binding)
            {
                return Err(invalid(
                    "statistics source overlaps an existing native service association",
                ));
            }
        }
        for discovered in discover_agent(host, agent, launch)? {
            let mut existing = binding_for(host, &discovered, true)?;
            if self
                .excluded
                .iter()
                .any(|entry| same_binding(entry, &existing))
            {
                continue;
            }
            if existing.agent == "opencode"
                && let Ok(verified) = self.check(host, &existing, launch)
            {
                existing = binding_for(host, &verified, true)?;
            }
            if overlaps(host, &existing, &binding) {
                return Err(invalid(
                    "statistics source overlaps an automatically discovered source",
                ));
            }
        }
        self.added.push(binding.clone());
        Ok(binding)
    }

    /// Only association metadata changes; native files and databases are never written.
    pub fn remove(&mut self, binding: &SourceBinding) {
        self.added.retain(|entry| !same_binding(entry, binding));
        if binding.discovered
            && !self
                .excluded
                .iter()
                .any(|entry| same_binding(entry, binding))
        {
            self.excluded.push(binding.clone());
        }
    }

    pub fn check(
        &self,
        host: &dyn Host,
        binding: &SourceBinding,
        launch: &HashMap<String, String>,
    ) -> io::Result<Source> {
        if binding.host != host.id().0 {
            return Err(invalid("statistics source belongs to another Host"));
        }
        let mut source = if let Some(source) = &binding.live {
            if source.agent != binding.agent
                || source.scope != binding.scope
                || binding_for(host, source, binding.discovered)?.selector != binding.selector
            {
                return Err(invalid("statistics live source does not match its binding"));
            }
            source.as_ref().clone()
        } else {
            rebuild(host, binding, launch)?
        };
        if let Some(request) = source.opencode_request.as_deref() {
            let payload = host
                .opencode_statistics(request)
                .map_err(io::Error::other)?;
            if payload.sessions.is_empty()
                && let Some(reason) = payload.issues.iter().find(|reason| !coverage_only(reason))
            {
                return Err(io::Error::other(reason.clone()));
            }
            let identity: serde_json::Value = serde_json::from_str(&payload.identity)
                .map_err(|_| invalid("OpenCode returned no durable service identity"))?;
            let server: crate::host::session_operations::opencode::OpenCodeServer =
                serde_json::from_value(identity["server"].clone())
                    .map_err(|_| invalid("OpenCode returned no durable service identity"))?;
            if server.urls.is_empty() {
                return Err(invalid("OpenCode returned no durable service URL"));
            }
            for url in &server.urls {
                service_url(url)?;
            }
            super::opencode::parse(&source, payload, chrono::Utc::now().timestamp())?;
            let request = source
                .opencode_request
                .as_mut()
                .expect("checked native request");
            let captured = binding
                .native_scope
                .as_ref()
                .and_then(|scope| scope.opencode.clone());
            let context = request
                .scope
                .opencode
                .get_or_insert_with(|| captured.unwrap_or_default());
            // Keep the live bridge/standalone selector intact; public URLs describe
            // service identity for overlap checks, not permission to replace transport.
            context.server = Some(server);
            return Ok(source);
        }
        if let Some(request) = source.hermes_request.as_deref() {
            let payload = host.hermes_statistics(request).map_err(io::Error::other)?;
            super::hermes::parse(&source, &payload, chrono::Utc::now().timestamp())?;
            return Ok(source);
        }
        source.root = checked_path(host, &binding.selector)?;
        if binding.scope.is_empty() {
            source.scope = format!("{}:{}", binding.agent, source.root.display());
        }
        let meta = host.stat(&source.root)?;
        if !meta.is_dir {
            if binding.agent == "kimi" {
                return Err(invalid(
                    "Kimi statistics requires its native sessions directory",
                ));
            }
            if !source.root.to_string_lossy().ends_with(".jsonl") {
                return Err(invalid(
                    "omp/pi statistics files must be native .jsonl histories",
                ));
            }
        }
        let snapshot = super::scan(
            host,
            std::slice::from_ref(&source),
            &mut super::StatisticsCache::default(),
        )?;
        if snapshot.files.is_empty()
            && let Some(issue) = snapshot.issues().first()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {}", issue.path().display(), issue.reason()),
            ));
        }
        Ok(source)
    }

    /// `discovered` must come from this Host's production discovery, not a cached snapshot.
    /// Cached native sources without requests are explicitly rebound; they never regain defaults.
    pub fn resolve_for_host(
        &self,
        host: &dyn Host,
        discovered: Vec<Source>,
        launch: &HashMap<String, String>,
    ) -> ResolvedSources {
        let mut result = ResolvedSources::default();
        for source in discovered {
            let binding = match binding_for(host, &source, true) {
                Ok(binding) => binding,
                Err(error) => {
                    let binding = display_binding(host, &source, true);
                    if self
                        .excluded
                        .iter()
                        .any(|entry| same_binding(entry, &binding))
                    {
                        continue;
                    }
                    result.bindings.push(binding.clone());
                    result.sources.push(mark_failed(source, error.to_string()));
                    result.issues.push(SourceFailure {
                        binding,
                        reason: error.to_string(),
                    });
                    continue;
                }
            };
            if self
                .excluded
                .iter()
                .any(|entry| same_binding(entry, &binding))
            {
                continue;
            }
            result.bindings.push(binding.clone());
            if (source.agent == "opencode" && source.opencode_request.is_none())
                || (source.agent == "hermes" && source.hermes_request.is_none())
            {
                match rebuild(host, &binding, launch) {
                    Ok(source) => result.sources.push(source),
                    Err(error) => {
                        let reason = error.to_string();
                        result.sources.push(mark_failed(source, reason.clone()));
                        result.issues.push(SourceFailure { binding, reason });
                    }
                }
            } else {
                result.sources.push(source);
            }
        }
        for binding in self.added.iter().filter(|entry| entry.host == host.id().0) {
            if result
                .bindings
                .iter()
                .any(|existing| overlaps(host, existing, binding))
            {
                result.bindings.push(binding.clone());
                result.issues.push(SourceFailure {
                    binding: binding.clone(),
                    reason: "statistics source overlaps a newly discovered source; remove one association".into(),
                });
                continue;
            }
            result.bindings.push(binding.clone());
            match rebuild(host, binding, launch) {
                Ok(source) => result.sources.push(source),
                Err(error) => {
                    let reason = error.to_string();
                    result
                        .sources
                        .push(failed_binding_source(binding, reason.clone()));
                    result.issues.push(SourceFailure {
                        binding: binding.clone(),
                        reason,
                    });
                }
            }
        }
        result
    }
}

fn mark_failed(mut source: Source, reason: String) -> Source {
    source.opencode_request = None;
    source.hermes_request = None;
    source.discovery_issue = Some(reason);
    source
}

fn failed_binding_source(binding: &SourceBinding, reason: String) -> Source {
    let root = if binding.agent == "opencode" {
        // Formal OpenCode discovery uses the Host cwd as its root. A damaged legacy
        // binding without cwd retains only a diagnostic selector, never a default home.
        binding
            .cwd
            .clone()
            .unwrap_or_else(|| PathBuf::from(&binding.selector))
    } else {
        PathBuf::from(&binding.selector)
    };
    let mut source = Source::omp(root, binding.scope.clone());
    source.agent = binding.agent.clone();
    mark_failed(source, reason)
}

fn display_binding(host: &dyn Host, source: &Source, discovered: bool) -> SourceBinding {
    SourceBinding {
        host: host.id().0,
        agent: source.agent.clone(),
        selector: source.root.to_string_lossy().into_owned(),
        scope: source.scope.clone(),
        discovered,
        native_scope: None,
        cwd: None,
        live: None,
        service_urls: Vec::new(),
    }
}

fn binding_for(host: &dyn Host, source: &Source, discovered: bool) -> io::Result<SourceBinding> {
    let mut binding = display_binding(host, source, discovered);
    if discovered {
        binding.live = Some(std::sync::Arc::new(source.clone()));
    }
    match source.agent.as_str() {
        "omp" | "pi" | "kimi" => {
            if let Ok(root) = host.canonicalize(&source.root) {
                binding.selector = root.to_string_lossy().into_owned();
            }
        }
        "hermes" => {
            if let Some(request) = source.hermes_request.as_deref() {
                binding.native_scope = Some(safe_scope(&request.scope)?);
                binding.cwd = Some(request.cwd.clone());
            }
        }
        "opencode" => {
            if let Some(request) = source.opencode_request.as_deref() {
                if let Some(server) = request
                    .scope
                    .opencode
                    .as_ref()
                    .and_then(|context| context.server.as_ref())
                {
                    binding.service_urls = server
                        .urls
                        .iter()
                        .map(|url| service_url(url))
                        .collect::<io::Result<_>>()?;
                }
                let mut scope = safe_scope(&request.scope)?;
                for arg in &request.launch_argv {
                    let Some((key, value)) = arg.split_once('=') else {
                        break;
                    };
                    if !native_environment_key(key) {
                        return Err(invalid(
                            "statistics native source contains an unsupported launch environment selector",
                        ));
                    }
                    scope
                        .environment
                        .entry(key.into())
                        .or_insert_with(|| value.into());
                }
                // discovery initially encodes --server in the current launcher only.
                let context = scope.opencode.get_or_insert_with(Default::default);
                if context.connection_argv.is_empty() {
                    let mut args = request.launch_argv.iter();
                    while let Some(arg) = args.next() {
                        if arg == "--server" {
                            let url = args
                                .next()
                                .ok_or_else(|| invalid("OpenCode service URL is missing"))?;
                            context.connection_argv = vec!["--server".into(), service_url(url)?];
                        } else if let Some(url) = arg.strip_prefix("--server=") {
                            context.connection_argv = vec!["--server".into(), service_url(url)?];
                        } else if arg == "--standalone" {
                            context.connection_argv = vec!["--standalone".into()];
                        }
                    }
                }
                binding.selector = connection_selector(&scope)?;
                if binding.selector == "configured"
                    && let Some(url) = binding.service_urls.first()
                {
                    binding.selector = url.clone();
                }
                binding.native_scope = Some(scope);
                binding.cwd = Some(request.cwd.clone());
            } else {
                // Verified snapshots contain public service identity but no live request.
                let identity: serde_json::Value = serde_json::from_str(&source.scope)
                    .map_err(|_| invalid("OpenCode source has no durable service selector; rediscover on the selected Host"))?;
                let url = identity["server"]["urls"]
                    .as_array()
                    .and_then(|urls| urls.first())
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| invalid("OpenCode source has no durable service URL"))?;
                binding.selector = service_url(url)?;
            }
        }
        _ => return Err(invalid("unsupported statistics source agent")),
    }
    Ok(binding)
}

fn rebuild(
    host: &dyn Host,
    binding: &SourceBinding,
    launch: &HashMap<String, String>,
) -> io::Result<Source> {
    if binding.host != host.id().0 {
        return Err(invalid("statistics source belongs to another Host"));
    }
    let path = PathBuf::from(&binding.selector);
    let scope = if binding.scope.is_empty() {
        format!("{}:{}", binding.agent, binding.selector)
    } else {
        binding.scope.clone()
    };
    match binding.agent.as_str() {
        "omp" | "pi" | "kimi" => {
            absolute_path(host, &binding.selector)?;
            Ok(match binding.agent.as_str() {
                "omp" => Source::omp(path, scope),
                "pi" => Source::pi(path, scope),
                _ => Source::kimi(path, scope),
            })
        }
        "opencode" => {
            let selector = service_url(&binding.selector)?;
            let mut source = discover_opencode(host, launch)?
                .into_iter()
                .next()
                .ok_or_else(|| invalid("OpenCode Host discovery returned no launcher"))?;
            let request = source
                .opencode_request
                .as_mut()
                .ok_or_else(|| invalid("OpenCode Host launcher is unavailable"))?;
            let mut native = binding
                .native_scope
                .as_ref()
                .map(safe_scope)
                .transpose()?
                .unwrap_or_default();
            let context = native.opencode.get_or_insert_with(Default::default);
            context.connection_argv = vec!["--server".into(), selector];
            apply_native_scope(host, request, native, binding.cwd.as_deref())?;
            source.scope = scope;
            Ok(source)
        }
        "hermes" => {
            let root = checked_path(host, &binding.selector)?;
            if !host.stat(&root)?.is_dir {
                return Err(invalid(
                    "Hermes statistics requires its resolved native profile home, not a database file",
                ));
            }
            host.read_dir(&root, None)?;
            let mut source = discover_hermes(host, launch)?
                .into_iter()
                .next()
                .ok_or_else(|| invalid("Hermes Host discovery returned no launcher"))?;
            let request = std::sync::Arc::make_mut(
                source
                    .hermes_request
                    .as_mut()
                    .ok_or_else(|| invalid("Hermes Host launcher is unavailable"))?,
            );
            let mut native = binding
                .native_scope
                .as_ref()
                .map(safe_scope)
                .transpose()?
                .unwrap_or_else(|| request.scope.clone());
            let profile = profile_for(&root)?;
            if native
                .profile
                .as_deref()
                .is_some_and(|selected| selected != profile)
                && binding.native_scope.is_some()
            {
                return Err(invalid(
                    "Hermes profile does not match its selected native home",
                ));
            }
            native.profile = Some(profile.into());
            native
                .environment
                .insert("HERMES_HOME".into(), root.to_string_lossy().into_owned());
            apply_native_scope(host, request, native, binding.cwd.as_deref())?;
            source.root = root;
            source.scope = scope;
            Ok(source)
        }
        _ => Err(invalid("unsupported statistics source agent")),
    }
}

fn apply_native_scope(
    host: &dyn Host,
    request: &mut SessionOperation,
    scope: SessionScope,
    cwd: Option<&Path>,
) -> io::Result<()> {
    if let Some(cwd) = cwd {
        absolute_path(host, &cwd.to_string_lossy())?;
        request.cwd = cwd.into();
    }
    request.expected_host = host.id().0;
    request.scope = scope;
    Ok(())
}

fn serialize_native_scope<S: serde::Serializer>(
    scope: &Option<SessionScope>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let scope = scope
        .as_ref()
        .map(safe_scope)
        .transpose()
        .map_err(<S::Error as serde::ser::Error>::custom)?;
    let value = scope.map(|scope| serde_json::json!({
        "profile": scope.profile,
        "environment": scope.environment,
        "opencode": scope.opencode.map(|context| serde_json::json!({"connection_argv": context.connection_argv})),
    }));
    value.serialize(serializer)
}

fn safe_scope(scope: &SessionScope) -> io::Result<SessionScope> {
    // Validate before durable_scope can remove unknown fields: malformed persisted
    // selectors must fail rather than silently inherit an unrelated Host default.
    if scope
        .environment
        .keys()
        .any(|key| !native_environment_key(key))
    {
        return Err(invalid(
            "statistics native source contains an unsupported environment selector",
        ));
    }
    let mut safe = durable_scope(scope);
    safe.session_dir = None;
    safe.session_file = None;
    safe.shell_program = None;
    if let Some(context) = safe.opencode.as_mut() {
        context.session = None;
        match context.connection_argv.as_slice() {
            [] => {}
            [standalone] if standalone == "--standalone" => {}
            [flag, url] if flag == "--server" => {
                service_url(url)?;
            }
            _ => return Err(invalid("invalid OpenCode service selector")),
        }
    }
    Ok(safe)
}

fn native_environment_key(key: &str) -> bool {
    [
        "HOME",
        "USERPROFILE",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "OPENCODE_DB",
        "HERMES_HOME",
        "HERMES_RUNTIME_DIR",
    ]
    .contains(&key)
}

fn connection_selector(scope: &SessionScope) -> io::Result<String> {
    match scope
        .opencode
        .as_ref()
        .map(|context| context.connection_argv.as_slice())
    {
        Some([flag, url]) if flag == "--server" => service_url(url),
        Some([flag]) if flag == "--standalone" => Ok("standalone".into()),
        Some([]) => Ok("configured".into()),
        _ => Err(invalid("invalid OpenCode service selector")),
    }
}

fn discover_agent(
    host: &dyn Host,
    agent: &str,
    launch: &HashMap<String, String>,
) -> io::Result<Vec<Source>> {
    match agent {
        "omp" => discover_omp(host, launch),
        "pi" => discover_pi(host, launch),
        "kimi" => discover_kimi(host, launch),
        "opencode" => discover_opencode(host, launch),
        "hermes" => discover_hermes(host, launch),
        _ => Err(invalid("unsupported statistics source agent")),
    }
}

fn same_binding(a: &SourceBinding, b: &SourceBinding) -> bool {
    a.host == b.host && a.agent == b.agent && a.selector == b.selector && a.scope == b.scope
}

fn overlaps(host: &dyn Host, a: &SourceBinding, b: &SourceBinding) -> bool {
    if a.host != b.host || a.agent != b.agent {
        return false;
    }
    if a.agent == "opencode" {
        let a_selector = service_url(&a.selector).ok();
        let b_selector = service_url(&b.selector).ok();
        return a.selector == b.selector
            || matches!((&a_selector, &b_selector), (Some(a), Some(b)) if a == b)
            || a.service_urls
                .iter()
                .any(|url| Some(url) == b_selector.as_ref() || b.service_urls.contains(url))
            || b.service_urls
                .iter()
                .any(|url| Some(url) == a_selector.as_ref());
    }
    let normalize = |value: &str| {
        let canonical = host
            .canonicalize(Path::new(value))
            .unwrap_or_else(|_| PathBuf::from(value));
        let text = canonical.to_string_lossy().replace('\\', "/");
        let text = text.trim_end_matches('/');
        if host.separator() == '\\' {
            text.to_lowercase()
        } else {
            text.into()
        }
    };
    let a = normalize(&a.selector);
    let b = normalize(&b.selector);
    a == b
        || a.strip_prefix(&b)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || b.strip_prefix(&a)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn service_url(value: &str) -> io::Result<String> {
    let url: http::Uri = value.parse().map_err(|_| {
        invalid("OpenCode statistics requires a native http(s) service URL, not a database path")
    })?;
    if !matches!(url.scheme_str(), Some("http" | "https"))
        || url
            .authority()
            .is_none_or(|authority| authority.as_str().contains('@'))
        || url.query().is_some()
        || value.contains(['#', '\0', '\n', '\r'])
    {
        return Err(invalid(
            "OpenCode service URL must not contain credentials or private parameters",
        ));
    }
    let scheme = url.scheme_str().expect("checked scheme");
    let authority = url
        .authority()
        .expect("checked authority")
        .as_str()
        .to_lowercase();
    let default_port = if scheme == "http" { ":80" } else { ":443" };
    let authority = authority.strip_suffix(default_port).unwrap_or(&authority);
    Ok(format!(
        "{scheme}://{authority}{}",
        if url.path().is_empty() {
            "/"
        } else {
            url.path()
        }
    ))
}

fn profile_for(path: &Path) -> io::Result<&str> {
    let value = path
        .to_str()
        .ok_or_else(|| invalid("Hermes profile home is not valid UTF-8"))?;
    let mut parts = value.trim_end_matches(['/', '\\']).rsplit(['/', '\\']);
    let name = parts.next().unwrap_or("");
    if parts.next() != Some("profiles") {
        return Ok("default");
    }
    if name.len() > 64
        || !name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte))
    {
        return Err(invalid("Hermes native profile name is invalid"));
    }
    Ok(name)
}

fn coverage_only(reason: &str) -> bool {
    reason.starts_with("OpenCode experimental.session.stats:")
        || reason.starts_with("OpenCode native aggregate and message metering differ;")
        || reason == "OpenCode statistics session window reached"
        || reason == "OpenCode statistics deadline reached"
        || reason == "OpenCode statistics message/deadline/response window reached"
        || reason == "OpenCode statistics repeated session cursor"
}

fn absolute_path(host: &dyn Host, value: &str) -> io::Result<()> {
    if value.is_empty() || value.contains(['\0', '\n', '\r']) || !host.is_absolute(Path::new(value))
    {
        return Err(invalid(
            "statistics source requires an absolute Host-native path",
        ));
    }
    Ok(())
}

fn checked_path(host: &dyn Host, value: &str) -> io::Result<PathBuf> {
    absolute_path(host, value)?;
    host.canonicalize(Path::new(value))
}

fn invalid(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, reason)
}

#[cfg(test)]
#[path = "sources_tests.rs"]
mod tests;
