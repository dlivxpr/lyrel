use serde_json::{Value, json};

use super::HookTarget;

pub(super) fn extension_path(target: &HookTarget) -> std::io::Result<std::path::PathBuf> {
    let environment = target.agent_environment(super::HookAgent::OhMyPi)?;
    let profile = crate::core::agent_history::omp_profile(&environment)?;
    let home = target.agent_home(&environment)?;
    let config = environment
        .get("PI_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map_or(".omp", String::as_str);
    let base = if target.host.is_absolute(std::path::Path::new(config)) {
        config.into()
    } else {
        target.under(&home, &[config])
    };
    let agent = if let Some(profile) = profile {
        target.under(&base, &["profiles", profile, "agent"])
    } else if let Some(path) = environment
        .get("PI_CODING_AGENT_DIR")
        .filter(|v| !v.is_empty())
    {
        let path = std::path::PathBuf::from(path);
        if !target.host.is_absolute(&path) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "relative PI_CODING_AGENT_DIR requires the native launch working directory; refusing to install in an inferred scope",
            ));
        }
        path
    } else {
        target.under(&base, &["agent"])
    };
    Ok(target.under(&agent, &["extensions", "lyrel", "index.ts"]))
}

pub(super) fn extension(target: &HookTarget) -> Option<String> {
    let exe = serde_json::to_string(&target.exe.display().to_string()).ok()?;
    Some(include_str!("omp.ts").replace("__EXE__", &exe))
}

/// OMP 18.12.1 extension finality, not Pi's agent_settled contract.
pub(super) fn normalize(event: &str, payload: &Value, body: &mut Value) {
    if event != "agent-end" {
        return;
    }
    if payload.get("willContinue").and_then(Value::as_bool) == Some(true)
        || payload.get("awaitingAsyncWork").and_then(Value::as_bool) == Some(true)
    {
        body["event"] = json!("prompt-submit");
        body.as_object_mut().unwrap().remove("result");
        return;
    }
    body["event"] = json!("stop");
    let last = payload.get("last_assistant");
    let reason = last
        .and_then(|m| m.get("errorMessage"))
        .and_then(Value::as_str);
    let outcome = match last
        .and_then(|m| m.get("stopReason"))
        .and_then(Value::as_str)
    {
        Some("error") => "failed",
        Some("aborted") => "interrupted",
        Some("stop") => "succeeded",
        Some("length") => "failed",
        _ => return, // A final boundary alone does not prove success.
    };
    body["result"] = json!({"outcome": outcome, "reason": reason.or_else(|| (last.and_then(|m| m.get("stopReason")).and_then(Value::as_str) == Some("length")).then_some("model output limit reached"))});
}
