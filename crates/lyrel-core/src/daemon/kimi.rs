//! Read-only observer for the official Kimi Code 2.2.0 main-agent durable wire.
//! Stop hooks run before other hooks can veto: only turn.ended settles a turn.

use crate::core::cli_agent::{AgentEvent, parse_agent_event_body};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::PathBuf;

#[derive(Debug)]
pub(super) struct Observer {
    pub session_id: String,
    pub file: PathBuf,
    offset: u64,
    partial: Vec<u8>,
    discard: bool,
    boundary: Vec<u8>,
    turn: Option<u64>,
    ended: Option<u64>,
    seeded: bool,
    seed_end: u64,
    pending: std::collections::BTreeMap<String, String>,
    tasks: std::collections::BTreeMap<String, String>,
    result: Option<serde_json::Value>,
    incompatible: bool,
    recover_active: bool,
}

impl Observer {
    pub fn new(session_id: String, file: PathBuf, recover_active: bool) -> Self {
        let seed_end = std::fs::metadata(&file).map_or(0, |meta| meta.len());
        Self {
            session_id,
            file,
            offset: 0,
            partial: Vec::new(),
            discard: false,
            boundary: Vec::new(),
            turn: None,
            ended: None,
            seeded: seed_end == 0,
            seed_end,
            pending: Default::default(),
            tasks: Default::default(),
            result: None,
            incompatible: false,
            recover_active,
        }
    }

    pub fn poll(&mut self) -> io::Result<Vec<AgentEvent>> {
        if self.incompatible {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Kimi durable wire requires verified protocol 1.5",
            ));
        }
        let mut file = File::open(&self.file)?;
        let len = file.metadata()?.len();
        let mut replaced = len < self.offset;
        if !replaced && !self.boundary.is_empty() {
            file.seek(SeekFrom::Start(self.offset - self.boundary.len() as u64))?;
            let mut previous = vec![0; self.boundary.len()];
            file.read_exact(&mut previous)?;
            replaced = previous != self.boundary;
        }
        if replaced {
            self.offset = 0;
            self.partial.clear();
            self.discard = false;
            self.boundary.clear();
            self.pending.clear();
            self.tasks.clear();
            if !self.seeded {
                self.seed_end = self.seed_end.min(len);
            }
            // Keep the high-water turn identity: old records after a rewrite must
            // not overwrite a newer result or increment completed turns twice.
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let initial = !self.seeded;
        let mut events = Vec::new();
        let mut bytes = [0; 16 * 1024];
        let mut remaining = if initial {
            self.seed_end.saturating_sub(self.offset).min(1024 * 1024) as usize
        } else {
            1024 * 1024
        };
        while remaining > 0 {
            let available = bytes.len().min(remaining);
            let count = file.read(&mut bytes[..available])?;
            if count == 0 {
                break;
            }
            remaining -= count;
            self.offset += count as u64;
            for byte in &bytes[..count] {
                if *byte == b'\n' {
                    if !self.discard {
                        if let Ok(record) =
                            serde_json::from_slice::<serde_json::Value>(&self.partial)
                        {
                            if record["type"] == "metadata" && record["protocol_version"] != "1.5" {
                                self.incompatible = true;
                                return Err(io::Error::new(
                                    io::ErrorKind::Unsupported,
                                    "Kimi durable wire protocol differs from verified 2.2.0 protocol 1.5",
                                ));
                            }
                            self.record(&record, &mut events);
                        }
                    }
                    self.partial.clear();
                    self.discard = false;
                } else if !self.discard {
                    if self.partial.len() < 256 * 1024 {
                        self.partial.push(*byte);
                    } else {
                        self.partial.clear();
                        self.discard = true;
                    }
                }
            }
        }
        let boundary_len = self.offset.min(32) as usize;
        self.boundary.resize(boundary_len, 0);
        file.seek(SeekFrom::Start(self.offset - boundary_len as u64))?;
        file.read_exact(&mut self.boundary)?;
        if initial {
            // A large history may take several ticks to reach the live tail.
            // Only the last active turn is relevant at attachment, never every
            // old success. Do not infer completion from EOF or from silence.
            events.clear();
            if self.offset >= self.seed_end {
                self.seeded = true;
                if self.recover_active && self.result.is_some() {
                    events.push(self.progress());
                } else if !self.pending.is_empty()
                    || !self.tasks.is_empty()
                    || self
                        .turn
                        .is_some_and(|turn| self.ended.is_none_or(|ended| turn > ended))
                {
                    events.push(self.event("prompt-submit", None));
                }
            }
        }
        Ok(events)
    }

    fn record(&mut self, record: &serde_json::Value, events: &mut Vec<AgentEvent>) {
        if record["agentId"] != "main" {
            return;
        }
        match record["type"].as_str() {
            Some("interaction.request") => {
                let Some(id) = record["id"].as_str() else {
                    return;
                };
                let kind = match record["kind"].as_str() {
                    Some("approval") => "permission",
                    Some("question" | "user_tool") => "question",
                    _ => return,
                };
                self.pending.insert(id.into(), kind.into());
                events.push(self.event("tool-complete", None));
                return;
            }
            Some("interaction.resolved") => {
                if let Some(id) = record["id"].as_str() {
                    self.pending.remove(id);
                }
                events.push(self.progress());
                return;
            }
            Some("task.started" | "task.terminated") => {
                let Some(id) = record["info"]["taskId"].as_str() else {
                    return;
                };
                if record["type"] == "task.started" {
                    self.tasks.insert(
                        id.into(),
                        record["info"]["kind"].as_str().unwrap_or("process").into(),
                    );
                } else {
                    self.tasks.remove(id);
                }
                events.push(self.progress());
                return;
            }
            _ => {}
        }
        let Some(turn) = record["turnId"].as_u64() else {
            return;
        };
        match record["type"].as_str() {
            Some("turn.prompt") if self.turn.is_none_or(|current| turn > current) => {
                self.turn = Some(turn);
                self.result = None;
                events.push(self.event("prompt-submit", None));
            }
            Some("turn.ended")
                if self.turn == Some(turn) && self.ended.is_none_or(|ended| turn > ended) =>
            {
                let outcome = match record["reason"].as_str() {
                    Some("completed") => "succeeded",
                    Some("failed") => "failed",
                    Some("cancelled") => "interrupted",
                    Some("blocked") => "rejected",
                    _ => return,
                };
                self.ended = Some(turn);
                let reason = record["error"]["message"]
                    .as_str()
                    .or_else(|| record["stopReason"].as_str())
                    .or_else(|| record["reason"].as_str());
                self.result = Some(serde_json::json!({"outcome":outcome,"reason":reason}));
                events.push(self.progress());
            }
            _ => {}
        }
    }

    fn progress(&self) -> AgentEvent {
        let settled = self.result.is_some()
            && self.turn == self.ended
            && self.tasks.is_empty()
            && self.pending.is_empty();
        self.event(
            if settled { "stop" } else { "tool-complete" },
            self.result.clone().filter(|_| settled),
        )
    }

    fn event(&self, kind: &str, result: Option<serde_json::Value>) -> AgentEvent {
        parse_agent_event_body(serde_json::json!({
            "v":1,"agent":"kimi","event":kind,"session_id":self.session_id,"result":result,
            "source":"kimi-durable-wire",
            "requests":self.pending.iter().chain(self.tasks.iter().filter(|(_,kind)| kind.as_str() == "question")).map(|(id,kind)| serde_json::json!({"id":id,"kind":kind,"resolved":false})).collect::<Vec<_>>()
        }).to_string().as_bytes()).expect("known Kimi bridge event")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cli_agent::{AgentOutcome, AgentSessionState, AgentStatus};
    use std::io::Write;

    #[test]
    fn kimi_blocked_stop_and_subagents_cannot_finish_the_main_turn() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("wire.jsonl");
        std::fs::write(
            &file,
            "{\"type\":\"metadata\",\"protocol_version\":\"1.5\"}\n",
        )
        .unwrap();
        let mut observer = Observer::new("session-main".into(), file.clone(), false);
        let mut state = AgentSessionState::default();
        assert!(observer.poll().unwrap().is_empty());
        let mut append = |value: serde_json::Value| {
            writeln!(
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&file)
                    .unwrap(),
                "{value}"
            )
            .unwrap();
            for event in observer.poll().unwrap() {
                state.apply_event(&event);
            }
            state.clone()
        };
        assert_eq!(
            append(serde_json::json!({"type":"turn.prompt","agentId":"main","turnId":7})).status,
            AgentStatus::Working
        );
        assert_eq!(append(serde_json::json!({"type":"turn.ended","agentId":"agent-0","turnId":7,"reason":"completed"})).status, AgentStatus::Working);
        // Stop-veto continuation is a context append, not a turn boundary.
        assert_eq!(append(serde_json::json!({"type":"context.append","agentId":"main","origin":{"kind":"system_trigger","name":"stop_hook"}})).status, AgentStatus::Working);
        let ended = append(
            serde_json::json!({"type":"turn.ended","agentId":"main","turnId":7,"reason":"failed","error":{"message":"model rejected"}}),
        );
        assert_eq!(ended.status, AgentStatus::Done);
        assert_eq!(
            ended.readout.result.as_ref().unwrap().outcome,
            AgentOutcome::Failed
        );
        assert_eq!(
            ended.readout.result.as_ref().unwrap().reason.as_deref(),
            Some("model rejected")
        );
        assert_eq!(append(serde_json::json!({"type":"turn.ended","agentId":"main","turnId":6,"reason":"completed"})).readout.result, ended.readout.result);
    }

    #[test]
    fn kimi_rebind_and_truncation_do_not_replay_old_success() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("wire.jsonl");
        std::fs::write(&file, "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":2}\n{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":2,\"reason\":\"completed\"}\n").unwrap();
        let mut observer = Observer::new("session-new".into(), file.clone(), false);
        assert!(
            observer.poll().unwrap().is_empty(),
            "old restored completion does not create a new completed turn"
        );
        std::fs::write(
            &file,
            "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":3}\n",
        )
        .unwrap();
        let events = observer.poll().unwrap();
        let mut state = AgentSessionState::default();
        for event in events {
            state.apply_event(&event);
        }
        assert_eq!(state.status, AgentStatus::Working);
        assert_eq!(state.session_id.as_deref(), Some("session-new"));
        assert_eq!(state.turns, 0);
    }

    #[test]
    fn kimi_durable_requests_and_background_work_settle_without_late_hook_races() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("wire.jsonl");
        std::fs::write(&file, "").unwrap();
        let mut observer = Observer::new("session-main".into(), file.clone(), false);
        let mut state = AgentSessionState::default();
        let mut append = |value: serde_json::Value| {
            writeln!(
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&file)
                    .unwrap(),
                "{value}"
            )
            .unwrap();
            for event in observer.poll().unwrap() {
                state.apply_event(&event);
            }
            state.clone()
        };
        append(serde_json::json!({"type":"turn.prompt","agentId":"main","turnId":0}));
        append(
            serde_json::json!({"type":"interaction.request","agentId":"main","id":"p1","kind":"approval"}),
        );
        append(
            serde_json::json!({"type":"interaction.request","agentId":"main","id":"p2","kind":"approval"}),
        );
        let waiting =
            append(serde_json::json!({"type":"interaction.resolved","agentId":"main","id":"p1"}));
        assert_eq!(waiting.status, AgentStatus::Waiting);
        assert_eq!(
            waiting
                .pending_requests
                .iter()
                .map(|request| request.id.as_str())
                .collect::<Vec<_>>(),
            ["p2"]
        );
        append(
            serde_json::json!({"type":"task.started","agentId":"main","info":{"taskId":"background","kind":"process"}}),
        );
        append(serde_json::json!({"type":"interaction.resolved","agentId":"main","id":"p2"}));
        assert_eq!(append(serde_json::json!({"type":"turn.ended","agentId":"main","turnId":0,"reason":"completed"})).status, AgentStatus::Working);
        let done = append(
            serde_json::json!({"type":"task.terminated","agentId":"main","info":{"taskId":"background","kind":"process"}}),
        );
        assert_eq!(done.status, AgentStatus::Done);
        assert_eq!(
            done.readout.result.unwrap().outcome,
            AgentOutcome::Succeeded
        );
    }

    #[test]
    fn kimi_handoff_recovers_the_real_end_of_a_previously_active_turn() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("wire.jsonl");
        std::fs::write(&file, "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":8}\n{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":8,\"reason\":\"cancelled\"}\n").unwrap();
        let mut observer = Observer::new("session-active-at-handoff".into(), file, true);
        let mut state = AgentSessionState::default();
        for event in observer.poll().unwrap() {
            state.apply_event(&event);
        }
        assert_eq!(state.status, AgentStatus::Done);
        assert_eq!(
            state.readout.result.unwrap().outcome,
            AgentOutcome::Interrupted
        );
    }
}
