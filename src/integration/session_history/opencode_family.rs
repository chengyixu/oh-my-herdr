use std::io;
use std::path::Path;

use serde_json::{json, Value};

use super::{invalid, storage, NativeHistory};

pub(crate) struct Ids {
    timestamp: u64,
    counter: u64,
    suffix: String,
}

impl Ids {
    pub(crate) fn new(timestamp: u64) -> Self {
        use sha2::{Digest, Sha256};
        let hash = format!("{:x}", Sha256::digest(timestamp.to_le_bytes()));
        let suffix = hash[..14].to_string();
        Self {
            timestamp,
            counter: 1,
            suffix,
        }
    }

    pub(crate) fn next(&mut self, prefix: &str) -> String {
        let val = (self.timestamp * 0x1000) + self.counter;
        self.counter += 1;
        let hex_val = val & 0xffff_ffff_ffff;
        format!("{prefix}_{hex_val:012x}{}", self.suffix)
    }
}

pub(crate) fn import_family_history(
    history: &NativeHistory,
    cwd: &Path,
    program: &str,
) -> io::Result<storage::ImportedHistory> {
    let agent_name = if program.contains("kilo") {
        "kilo"
    } else {
        "opencode"
    };

    let mut acp = super::super::native_acp::Discovery::start(program, &["acp"], cwd)?;
    let init_res = acp.call(
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {
                "name": "ohmyherdr-native-import",
                "version": "1"
            }
        }),
    )?;
    let version = init_res
        .pointer("/agentInfo/version")
        .and_then(Value::as_str)
        .unwrap_or("");
    if version != "7.8.8" && version != "1.19.3" {
        return Err(invalid(
            "this native OpenCode/Kilo version requires qualification; source retained",
        ));
    }

    let result = acp.call(
        "session/new",
        json!({
            "cwd": cwd.to_string_lossy(),
            "mcpServers": []
        }),
    )?;
    let session_id = result
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("native session identity was not allocated"))?
        .to_string();

    let model_id = result
        .pointer("/configOptions/model")
        .and_then(Value::as_str)
        .unwrap_or("default");

    let (exchange_dir, _) = super::super::native_process::private_directory()?;
    let seed_export =
        super::super::native_process::capture(program, &["export", &session_id], cwd, 65536)?;
    let seed_json: Value = serde_json::from_slice(&seed_export).map_err(|_| {
        invalid("allocated native import seed already contains conversation records")
    })?;
    if let Some(msgs) = seed_json.get("messages").and_then(Value::as_array) {
        if !msgs.is_empty() {
            return Err(invalid(
                "allocated native import seed already contains conversation records",
            ));
        }
    }

    let identity = storage::Identity::new(history)?;
    let timestamp_ms = identity.created.unix_timestamp() as u64 * 1000;
    let mut ids = Ids::new(timestamp_ms);

    let doc = document(
        history,
        &session_id,
        cwd,
        model_id,
        &mut ids,
        &identity.stamp,
    )?;
    let import_file = exchange_dir.join("native-history.jsonl");
    let doc_bytes = serde_json::to_vec(&doc)?;
    std::fs::write(&import_file, &doc_bytes)?;

    let _import_output = super::super::native_process::capture(
        program,
        &["import", &session_id, import_file.to_str().unwrap()],
        cwd,
        MAX_HISTORY_BYTES,
    )?;

    let after_export = super::super::native_process::capture(
        program,
        &["export", &session_id],
        cwd,
        MAX_HISTORY_BYTES,
    )?;
    let exported: Value = serde_json::from_slice(&after_export).map_err(|_| {
        invalid("native import did not retain the complete encoded conversation; source retained")
    })?;
    if exported.pointer("/info/id").and_then(Value::as_str) != Some(&session_id) {
        return Err(invalid("allocated native session identity changed"));
    }

    let native_session = crate::agent_resume::PersistedAgentSession {
        source: format!("herdr:{agent_name}"),
        agent: agent_name.into(),
        session_ref: crate::agent_resume::AgentSessionRef::id(session_id.clone())
            .ok_or_else(|| invalid("invalid allocated native identity"))?,
    };

    let mut imported = storage::publish(
        history,
        doc["messages"].as_array().cloned().unwrap_or_default(),
        import_file,
        native_session,
        &session_id,
    )?;
    imported.own_new_directory(exchange_dir);
    imported.own_native_session(
        Box::leak(agent_name.to_string().into_boxed_str()),
        cwd.to_path_buf(),
    );
    Ok(imported)
}

const MAX_HISTORY_BYTES: usize = 256 * 1024 * 1024;

fn document(
    history: &NativeHistory,
    session_id: &str,
    cwd: &Path,
    model_id: &str,
    ids: &mut Ids,
    stamp: &str,
) -> io::Result<Value> {
    let mut messages = Vec::new();

    if let Some(config_msg) = super::historical_source_configuration(history, "opencode")? {
        let msg_id = ids.next("msg");
        let prt_id = ids.next("prt");
        let text = text(&config_msg)?;
        messages.push(json!({
            "info": {
                "id": msg_id,
                "sessionID": session_id,
                "role": "user",
                "time": {"created": stamp}
            },
            "parts": [{
                "id": prt_id,
                "messageID": msg_id,
                "sessionID": session_id,
                "type": "text",
                "text": text
            }]
        }));
    }

    for msg in &history.messages {
        let msg_id = ids.next("msg");
        let role = msg["role"].as_str().unwrap_or("user");
        let blocks = super::message_blocks(msg)?;
        let mut parts = Vec::new();

        for b in blocks {
            let prt_id = ids.next("prt");
            match b.get("type").and_then(Value::as_str) {
                Some("text") => {
                    parts.push(json!({
                        "id": prt_id,
                        "messageID": msg_id,
                        "sessionID": session_id,
                        "type": "text",
                        "text": b["text"].as_str().unwrap_or("")
                    }));
                }
                Some("image") => {
                    let uri = super::image_uri(&b)?;
                    parts.push(json!({
                        "id": prt_id,
                        "messageID": msg_id,
                        "sessionID": session_id,
                        "type": "file",
                        "mime": "image/png",
                        "url": uri
                    }));
                }
                Some("toolCall") => {
                    let call_id = b["id"].as_str().unwrap_or("");
                    let name = super::tool_name(&b)?;
                    let args = b.get("arguments").cloned().unwrap_or(json!({}));
                    parts.push(json!({
                        "id": prt_id,
                        "messageID": msg_id,
                        "sessionID": session_id,
                        "type": "tool",
                        "callID": call_id,
                        "tool": name,
                        "state": {
                            "status": "completed",
                            "input": args,
                            "time": {"start": stamp, "end": stamp}
                        }
                    }));
                }
                _ => {}
            }
        }

        if role == "toolResult" {
            let call_id = msg["toolCallId"].as_str().unwrap_or("");
            let result_text = match msg.get("content") {
                Some(Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => String::new(),
            };
            let prt_id = ids.next("prt");
            parts.push(json!({
                "id": prt_id,
                "messageID": msg_id,
                "sessionID": session_id,
                "type": "tool",
                "callID": call_id,
                "state": {
                    "status": "completed",
                    "output": result_text,
                    "time": {"start": stamp, "end": stamp}
                }
            }));
        }

        messages.push(json!({
            "info": {
                "id": msg_id,
                "sessionID": session_id,
                "role": if role == "toolResult" { "user" } else { role },
                "modelID": model_id,
                "time": {"created": stamp}
            },
            "parts": parts
        }));
    }

    if let Some(model_msgs) = &history.model_messages {
        let parent_msg_id = ids.next("msg");
        let parent_prt_id = ids.next("prt");
        messages.push(json!({
            "info": {
                "id": parent_msg_id,
                "sessionID": session_id,
                "role": "user",
                "time": {"created": stamp}
            },
            "parts": [{
                "id": parent_prt_id,
                "messageID": parent_msg_id,
                "sessionID": session_id,
                "type": "compaction",
                "auto": true
            }]
        }));

        let summary_msg_id = ids.next("msg");
        let summary_prt_id = ids.next("prt");
        messages.push(json!({
            "info": {
                "id": summary_msg_id,
                "sessionID": session_id,
                "parentID": parent_msg_id,
                "role": "assistant",
                "summary": true,
                "modelID": model_id,
                "time": {"created": stamp}
            },
            "parts": [{
                "id": summary_prt_id,
                "messageID": summary_msg_id,
                "sessionID": session_id,
                "type": "text",
                "text": "",
                "metadata": {
                    "ohmyherdrNativeCheckpoint": {
                        "version": 1
                    }
                }
            }]
        }));

        for active in model_msgs {
            let active_msg_id = ids.next("msg");
            let active_prt_id = ids.next("prt");
            let active_role = active["role"].as_str().unwrap_or("user");
            let active_text = match active.get("content") {
                Some(Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => String::new(),
            };
            messages.push(json!({
                "info": {
                    "id": active_msg_id,
                    "sessionID": session_id,
                    "parentID": summary_msg_id,
                    "role": active_role,
                    "modelID": model_id,
                    "time": {"created": stamp}
                },
                "parts": [{
                    "id": active_prt_id,
                    "messageID": active_msg_id,
                    "sessionID": session_id,
                    "type": "text",
                    "text": active_text
                }]
            }));
        }
    }

    Ok(json!({
        "info": {
            "id": session_id,
            "title": "Imported conversation",
            "directory": cwd.to_string_lossy(),
            "time": {"created": stamp}
        },
        "messages": messages
    }))
}

fn text(message: &Value) -> io::Result<String> {
    let blocks = super::message_blocks(message)?;
    let mut out = String::new();
    for b in blocks {
        if let Some(t) = b.get("text").and_then(Value::as_str) {
            out.push_str(t);
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn native_family_ids_are_monotonic_and_older_than_the_next_prompt() {
        let mut ids = Ids::new(1729209600000);
        let first = ids.next("msg");
        let second = ids.next("msg");
        let third = ids.next("prt");

        assert!(first.starts_with("msg_"));
        assert!(second.starts_with("msg_"));
        assert!(third.starts_with("prt_"));
        assert!(first < second);
        assert_eq!(first.len(), 4 + 12 + 14);
    }
}
