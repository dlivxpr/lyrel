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
    let scope = serde_json::to_string(crate::host::session_operations::OMP_SCOPE_ENV).ok()?;
    Some(
        include_str!("omp.ts")
            .replace("__SCOPE_ENV__", &scope)
            .replace("__EXE__", &exe),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omp_resume_switch_and_rename_publish_only_authoritative_nonempty_titles() {
        let temp = tempfile::tempdir().unwrap();
        let node = std::process::Command::new("node")
            .args(["-p", "process.execPath"])
            .output()
            .expect("Node 22.19+ is required for the OMP integration contract");
        assert!(node.status.success());
        let node = std::path::PathBuf::from(String::from_utf8(node.stdout).unwrap().trim());
        let host = crate::host::local::LocalHost::new();
        let target =
            HookTarget::local_for_exe(&*host, temp.path().join("removed-install")).unwrap();
        std::fs::write(temp.path().join("bridge.ts"), extension(&target).unwrap()).unwrap();
        let utils = temp.path().join("node_modules/@oh-my-pi/pi-utils");
        std::fs::create_dir_all(&utils).unwrap();
        std::fs::write(
            utils.join("package.json"),
            r#"{"type":"module","exports":"./index.js"}"#,
        )
        .unwrap();
        std::fs::write(
            utils.join("index.js"),
            "export const getActiveProfile = () => 'default';",
        )
        .unwrap();
        std::fs::write(temp.path().join("agent-hook"), r#"const fs = require('node:fs'); fs.appendFileSync('reports.jsonl', JSON.stringify({event:process.argv[3],...JSON.parse(fs.readFileSync(0,'utf8'))})+'\n');"#).unwrap();
        let script = r#"
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import bridge from './bridge.ts';
const handlers = new Map(), timers = new Set(), titles = [], warnings = [];
let identity = 'resumed-source', name = 'Real resumed title';
bridge({on(event, handler) { handlers.set(event, handler); }});
const ctx = {
  agent:{kind:'main'},
  sessionManager:{getSessionId:()=>identity,getSessionName:()=>name,getSessionFile:()=>'/owned/source.jsonl',getSessionDir:()=>'/owned',getCwd:()=>'/project'},
  getPendingPlanReview:()=>undefined,
  setInterval(fn) { timers.add(fn); return fn; },
  clearTimer(fn) { timers.delete(fn); },
  ui:{setTitle(title) { titles.push(title); }, notify(message) { warnings.push(message); }}
};
const send = (event) => handlers.get(event)({},ctx);
send('session_start');
assert.deepEqual(titles,['Real resumed title']);
assert.equal(warnings.length,0);
titles.push('program-owned title');
for (const tick of timers) tick();
assert.equal(titles.at(-1),'program-owned title','unchanged metadata must not reclaim a program title');
identity = 'switched-source'; name = 'Real switched title'; send('session_switch');
assert.equal(titles.at(-1),'Real switched title');
assert.equal(timers.size,1,'switch replaces, rather than multiplies, title observers');
name = 'Renamed\x07\x1b title'; for (const tick of timers) tick();
assert.equal(titles.at(-1),'Renamed title','untrusted title controls never reach the terminal');
const count = titles.length;
identity = 'untitled-source'; name = undefined; send('session_switch');
for (const tick of timers) tick();
assert.equal(titles.length,count,'an unnamed session must not overwrite the real program title');
handlers.get('session_start')({}, {...ctx,agent:{kind:'subagent'}});
assert.equal(titles.length,count,'a child cannot name the pane');
assert.equal(timers.size,1);
const reports = readFileSync('reports.jsonl','utf8').trim().split('\n').map(JSON.parse);
assert.deepEqual(reports.map(r=>r.session_id),['resumed-source','switched-source','untitled-source']);
process.stdout.write(reports.map(r=>JSON.stringify(r)).join('\n'));
"#;
        let output = std::process::Command::new(&node)
            .args([
                "--experimental-strip-types",
                "--input-type=module",
                "--eval",
                script,
            ])
            .current_dir(temp.path())
            .env("LYREL", "1")
            .env(crate::core::agent_hooks::LYREL_AGENT_HOOK_EXE_ENV, &node)
            .env("HOME", temp.path())
            .env("USERPROFILE", temp.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut state = crate::core::cli_agent::AgentSessionState::default();
        for line in String::from_utf8(output.stdout).unwrap().lines() {
            let payload: Value = serde_json::from_str(line).unwrap();
            let body = crate::core::agent_hooks::build_hook_body(
                "omp",
                payload["event"].as_str().unwrap(),
                line,
            );
            state.apply_event(
                &crate::core::cli_agent::parse_agent_event_body(body.as_bytes()).unwrap(),
            );
            assert_eq!(state.session_id.as_deref(), payload["session_id"].as_str());
            assert_eq!(
                state
                    .readout
                    .session_scope
                    .as_ref()
                    .unwrap()
                    .session_file
                    .as_deref(),
                Some(std::path::Path::new("/owned/source.jsonl"))
            );
        }
        let failure_script = r#"
import assert from 'node:assert/strict';
import bridge from './bridge.ts';
const handlers = new Map(), warnings = [];
bridge({on(event,handler){handlers.set(event,handler);}});
const ctx = {agent:{kind:'main'}, sessionManager:{getSessionId:()=> 'source',getCwd:()=>'/project',getSessionFile:()=>'/owned/source.jsonl',getSessionDir:()=>'/owned'},ui:{notify(message){warnings.push(message);}}};
handlers.get('agent_start')({},ctx);
handlers.get('turn_start')({},ctx);
assert.equal(warnings.length,1,'delivery failure must remain visible without flooding');
assert.match(warnings[0],/bridge unavailable.*ENOENT/);
"#;
        let failure = std::process::Command::new(&node)
            .args(["--input-type=module", "--eval", failure_script])
            .current_dir(temp.path())
            .env("LYREL", "1")
            .env(
                crate::core::agent_hooks::LYREL_AGENT_HOOK_EXE_ENV,
                temp.path().join("missing-runtime-runner"),
            )
            .output()
            .unwrap();
        assert!(
            failure.status.success(),
            "{}",
            String::from_utf8_lossy(&failure.stderr)
        );
    }
}
