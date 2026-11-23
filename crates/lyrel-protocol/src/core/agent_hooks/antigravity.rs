//! Antigravity (`agy`) observer hooks in the documented global registry.
//!
//! Upstream contract, verified against the embedded hook documentation in
//! `antigravity-cli` 1.5.15 and <https://antigravity.google/docs/hooks/>:
//!
//! - The global registry is `~/.gemini/config/hooks.json`, a JSON object whose
//!   top-level keys name named hooks. Lyrel owns exactly the `"lyrel"` key and
//!   never touches sibling entries, the user `enabled` flag inside its own
//!   entry, or the unrelated `~/.gemini/settings.json` status line.
//! - Tool events (`PreToolUse`, `PostToolUse`) take the grouped
//!   `{"matcher": ..., "hooks": [...]}` entry shape; `SessionStart`,
//!   `PreInvocation`, `PostInvocation` and `Stop` take flat handler objects
//!   directly (upstream issue google-antigravity/antigravity-cli#925: the
//!   grouped form is silently dropped on flat events).
//! - Handlers run `sh -c`/`cmd /c`, receive JSON on stdin and must answer a
//!   JSON object on stdout. `PreToolUse` and `Stop` require a `decision`;
//!   `"allow"` delegates to the next hook or default policy, so the observer
//!   never gates a tool call or a stop.
//! - `PreToolUse` is deliberately not installed: an observer only reports, and
//!   an ordinary tool call must never surface as Waiting.
//!
//! The registry file itself is not proven to be the *installed* runtime
//! contract: a real-run verification of the observed `agy` version's hook
//! shape, continuation and idle semantics stays outstanding until a real
//! `agy` turn can be exercised.

use super::*;

/// The one top-level registry key lyrel owns.
pub const HOOK_NAME: &str = "lyrel";

/// The upstream hook names lyrel installs. Command arguments carry the
/// upstream spelling so the registry reads as the product's own vocabulary.
const EVENTS: &[&str] = &[
    "SessionStart",
    "PreInvocation",
    "PostInvocation",
    "PostToolUse",
    "Stop",
];

/// Every upstream hook name, for schema checks on user entries.
const KNOWN_EVENTS: &[&str] = &[
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "PreInvocation",
    "PostInvocation",
    "Stop",
];

const TIMEOUT: u64 = 10;

pub(super) fn registry_path(target: &HookTarget) -> io::Result<PathBuf> {
    let environment = target.agent_environment(HookAgent::Antigravity)?;
    let home = target.agent_home(&environment)?;
    Ok(target.under(&home, &[".gemini", "config", "hooks.json"]))
}

fn read_registry(
    target: &HookTarget,
    path: &Path,
) -> anyhow::Result<Option<serde_json::Map<String, serde_json::Value>>> {
    let text = match target.read(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(anyhow::anyhow!("read {}: {error}", path.display())),
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Object(object)) => Ok(Some(object)),
        Ok(_) => Err(anyhow::anyhow!(
            "{} is not a JSON hook map; not touching it",
            path.display()
        )),
        Err(error) => Err(anyhow::anyhow!(
            "{} is not valid JSON ({error}); not touching it",
            path.display()
        )),
    }
}

/// Whether `entry` is a tool event's grouped entry — `matcher` and a `hooks`
/// array — the one alternative shape the registry defines.
fn looks_tool(entry: &serde_json::Value) -> bool {
    entry.get("hooks").is_some() || entry.get("matcher").is_some()
}

/// The command handler list under one event entry: the `hooks` array of a
/// grouped tool entry, or the entry itself for a flat handler object.
/// `None` when the shape cannot carry command handlers at all.
fn handlers<'a>(entry: &'a serde_json::Value) -> Option<Vec<&'a serde_json::Value>> {
    if let Some(hooks) = entry.get("hooks") {
        return Some(hooks.as_array()?.iter().collect());
    }
    match entry {
        serde_json::Value::Object(_) => Some(vec![entry]),
        _ => None,
    }
}

/// Whether every command handler this named-hook entry defines is lyrel's.
/// Non-event keys (`enabled`, anything unknown) never carry commands and never
/// make an entry foreign; a handler list that is not wholly ours does.
fn entry_is_ours(entry: &serde_json::Value, marker: &str) -> bool {
    let Some(object) = entry.as_object() else {
        return false;
    };
    let mut any = false;
    for (key, value) in object {
        if !KNOWN_EVENTS.contains(&key.as_str()) && !looks_tool(value) {
            continue;
        }
        for item in value.as_array().into_iter().flatten() {
            let Some(list) = handlers(item) else {
                return false;
            };
            for handler in list {
                let Some(command) = handler.get("command").and_then(|c| c.as_str()) else {
                    return false;
                };
                any = true;
                if !lyrel_hook_command(command, marker) {
                    return false;
                }
            }
        }
    }
    any
}

/// The named-hook entry this build installs, in the exact shape each fired
/// event's contract accepts: flat handlers for non-tool events and one
/// grouped `matcher` entry for `PostToolUse`. `enabled` is never written —
/// a user-set flag is a user configuration to preserve.
fn expected_entry(target: &HookTarget) -> serde_json::Map<String, serde_json::Value> {
    let agent = HookAgent::Antigravity;
    let mut entry = serde_json::Map::new();
    for &upstream in EVENTS {
        let handler = serde_json::json!({
            "type": "command",
            "command": target.hook_command(agent, upstream),
            "timeout": TIMEOUT,
        });
        entry.insert(
            upstream.into(),
            if upstream == "PostToolUse" {
                serde_json::json!([{"matcher": "*", "hooks": [handler]}])
            } else {
                serde_json::json!([handler])
            },
        );
    }
    entry
}

/// Whether the registry's `"lyrel"` entry already carries the command for
/// every event this build installs. Extra keys the user kept (`enabled`,
/// further handlers that are also ours) do not make the entry stale.
fn entry_is_current(entry: &serde_json::Value, target: &HookTarget) -> bool {
    let agent = HookAgent::Antigravity;
    for &upstream in EVENTS {
        let command = target.hook_command(agent, upstream);
        let found = entry
            .get(upstream)
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .any(|item| {
                handlers(item).into_iter().flatten().any(|handler| {
                    handler.get("command").and_then(|c| c.as_str()) == Some(command.as_str())
                })
            });
        if !found {
            return false;
        }
    }
    true
}

pub(super) fn state(target: &HookTarget) -> HooksState {
    let Ok(path) = registry_path(target) else {
        return HooksState::NotInstalled;
    };
    let registry = match read_registry(target, &path) {
        Ok(Some(registry)) => registry,
        _ => return HooksState::NotInstalled,
    };
    let Some(entry) = registry.get(HOOK_NAME) else {
        return HooksState::NotInstalled;
    };
    let marker = HookAgent::Antigravity.marker();
    if !entry_is_ours(entry, &marker) {
        return HooksState::NotInstalled;
    }
    if entry_is_current(entry, target) {
        HooksState::Installed
    } else {
        HooksState::Outdated
    }
}

pub(super) fn install(target: &HookTarget) -> anyhow::Result<HookOutcome> {
    let path = registry_path(target)?;
    let marker = HookAgent::Antigravity.marker();
    let mut registry = read_registry(target, &path)?.unwrap_or_default();
    if let Some(entry) = registry.get(HOOK_NAME)
        && !entry_is_ours(entry, &marker)
    {
        anyhow::bail!(
            "\"{HOOK_NAME}\" in {} exists but wasn't written by lyrel; not touching it",
            path.display()
        );
    }
    // Only lyrel-owned event keys are replaced; `enabled` and everything else
    // the user kept inside the entry survive reinstall and update.
    if let Some(entry) = registry
        .get_mut(HOOK_NAME)
        .and_then(serde_json::Value::as_object_mut)
    {
        for upstream in EVENTS {
            entry.remove(*upstream);
        }
    }
    let expected = expected_entry(target);
    let entry = registry
        .entry(HOOK_NAME)
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    entry
        .as_object_mut()
        .expect("lyrel entry is an object")
        .extend(expected);
    target.write(&path, serde_json::to_string_pretty(&registry)?.as_bytes())?;
    Ok(HookOutcome::Installed)
}

pub(super) fn uninstall(target: &HookTarget) -> anyhow::Result<HookOutcome> {
    let path = registry_path(target)?;
    let marker = HookAgent::Antigravity.marker();
    let Some(mut registry) = read_registry(target, &path)? else {
        return Ok(HookOutcome::NothingInstalled);
    };
    let Some(entry) = registry.get(HOOK_NAME) else {
        return Ok(HookOutcome::NoLyrelHooks);
    };
    if !entry_is_ours(entry, &marker) {
        anyhow::bail!(
            "\"{HOOK_NAME}\" in {} wasn't written by lyrel; not touching it",
            path.display()
        );
    }
    registry.remove(HOOK_NAME);
    target.write(&path, serde_json::to_string_pretty(&registry)?.as_bytes())?;
    Ok(HookOutcome::Removed)
}

/// The upstream hook event that fired, as the payload itself says, falling
/// back to the event name the registry command carried. `hookEventName` is
/// authoritative when a user moves a lyrel handler between events: the report
/// then describes the event that actually ran, not the command's spelling.
pub fn upstream_event(arg: &str, stdin_json: &str) -> String {
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(stdin_json) else {
        return arg.to_string();
    };
    payload
        .get("hookEventName")
        .and_then(|name| name.as_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(arg)
        .to_string()
}

/// The lyrel event an Antigravity hook reports, or `None` when the event must
/// not change pane state at all.
pub(super) fn effective(arg: &str, stdin_json: &str) -> Option<&'static str> {
    match upstream_event(arg, stdin_json).as_str() {
        "SessionStart" => Some("session-start"),
        "PreInvocation" | "PostInvocation" => Some("prompt-submit"),
        "PostToolUse" => Some("tool-complete"),
        "Stop" => {
            // Done is settled only when upstream says the agent is completely
            // finished. `fullyIdle` absent is not idle: the loop may continue
            // or background work may still run, so the observation stays
            // Working — never an inferred Done.
            let settled = serde_json::from_str::<serde_json::Value>(stdin_json)
                .ok()
                .and_then(|payload| payload.get("fullyIdle").and_then(|v| v.as_bool()))
                .unwrap_or(false);
            Some(if settled { "stop" } else { "prompt-submit" })
        }
        // PreToolUse and every other event report nothing; a tool call is not
        // a turn boundary and can never become Waiting from an observer.
        _ => None,
    }
}

/// The stdout answer `agy` blocks on, given the event the hook ran for.
/// Responses are passive by contract: `{}` where output is optional,
/// `{"decision": "allow"}` where a decision is required — delegating to the
/// next hook or default policy rather than gating a tool call or a stop.
/// Anything lyrel does not recognize gets the safe shape, never an error.
pub fn response(upstream: &str) -> &'static str {
    match upstream {
        "PreToolUse" | "Stop" => r#"{"decision":"allow"}"#,
        _ => "{}",
    }
}

/// Fold the hook stdin payload into the report: identity, model, the
/// authoritative transcript path, and a settled outcome where upstream
/// supplies one. `lastUserInput` is never forwarded — what the user typed
/// does not belong in a status report.
pub(super) fn normalize(event: &str, payload: &serde_json::Value, body: &mut serde_json::Value) {
    let text = |key: &str| {
        payload
            .get(key)
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
    };
    if let Some(id) = text("conversationId") {
        body["session_id"] = id.into();
    }
    if let Some(model) = text("modelName") {
        body["model"] = model.into();
    }
    if event == "session-start"
        && let Some(transcript) = text("transcriptPath")
    {
        body["session_scope"] = serde_json::json!({
            "session_file": transcript,
            "environment": {},
        });
    }
    if event == "stop" {
        let error = text("error");
        let reason = text("terminationReason");
        // `terminationReason` is why the loop terminated, not a verdict: only
        // a reported error or an explicit cancellation is an outcome.
        let (outcome, detail) = match reason {
            Some("USER_CANCELED") => (Some("interrupted"), error.or(Some("canceled by user"))),
            Some("ERROR") => (Some("failed"), error.or(Some("execution error"))),
            _ => (None, None),
        };
        if let Some(outcome) = outcome {
            let mut result = serde_json::json!({"outcome": outcome});
            if let Some(detail) = detail {
                result["reason"] = clip_chars(detail, MESSAGE_MAX).into();
            }
            body["result"] = result;
            body["message"] = clip_chars(detail.unwrap_or(outcome), MESSAGE_MAX).into();
        }
    }
}
