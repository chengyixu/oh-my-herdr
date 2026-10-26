use std::io;
use std::path::Path;

use serde_json::{json, Value};

use super::{invalid, storage, NativeHistory};

pub(crate) fn import_claude_history(
    history: &NativeHistory,
    cwd: &Path,
    claude_dir: &Path,
) -> io::Result<storage::ImportedHistory> {
    let output = std::process::Command::new("claude")
        .arg("--version")
        .output()?;
    if !output.status.success() {
        return Err(invalid(
            "this native Claude version requires qualification before switching; source retained",
        ));
    }
    let version = String::from_utf8_lossy(&output.stdout);
    let trimmed = version.trim();
    if trimmed.is_empty() {
        return Err(invalid(
            "this native Claude version requires qualification before switching; source retained",
        ));
    }

    let identity = storage::Identity::new(history)?;
    let records = records(
        history,
        &identity.id,
        cwd,
        "claude-3-5-sonnet-20241022",
        &identity.stamp,
    )?;

    let cwd_str = cwd
        .to_str()
        .ok_or_else(|| invalid("native Claude cwd is not valid UTF-8"))?;
    let slug: String = cwd_str
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if slug.len() > 200 {
        return Err(invalid(
            "native Claude long-path layout requires qualification before switching",
        ));
    }

    let path = claude_dir
        .join("projects")
        .join(slug)
        .join(format!("{}.jsonl", identity.id));

    let native_session = crate::agent_resume::PersistedAgentSession {
        source: "herdr:claude".into(),
        agent: "claude".into(),
        session_ref: crate::agent_resume::AgentSessionRef::id(identity.id.clone())
            .ok_or_else(|| invalid("invalid native Claude session identity"))?,
    };
    storage::publish(history, records, path, native_session, &identity.id)
}

fn records(
    history: &NativeHistory,
    session_id: &str,
    cwd: &Path,
    target_model: &str,
    stamp: &str,
) -> io::Result<Vec<Value>> {
    let mut records = Vec::new();
    let mut parent: Option<String> = None;

    if let Some(config_msg) = super::historical_source_configuration(history, "claude")? {
        append_messages(
            &mut records,
            &mut parent,
            &[config_msg],
            session_id,
            cwd,
            target_model,
            stamp,
        )?;
    }

    append_messages(
        &mut records,
        &mut parent,
        &history.messages,
        session_id,
        cwd,
        target_model,
        stamp,
    )?;

    if let Some(model_msgs) = &history.model_messages {
        let checkpoint_id =
            storage::uuid_from_seed(&format!("{session_id}:compact:{}", records.len()));
        records.push(json!({
            "type": "system",
            "subtype": "compactMetadata",
            "id": checkpoint_id,
            "parent": parent,
            "sessionId": session_id,
            "cwd": cwd.to_string_lossy(),
            "timestamp": stamp,
            "data": {
                "summary": "Imported native active-context checkpoint; complete recorded source retained",
                "trigger": "preTokens",
                "activeMessages": model_msgs,
            }
        }));
    }

    Ok(records)
}

fn append_messages(
    records: &mut Vec<Value>,
    parent: &mut Option<String>,
    messages: &[Value],
    session_id: &str,
    cwd: &Path,
    target_model: &str,
    stamp: &str,
) -> io::Result<()> {
    for message in messages {
        let msg_id = storage::uuid_from_seed(&format!("{session_id}:msg:{}", records.len() + 1));
        let role = message["role"].as_str().unwrap_or("user");
        let content_blocks = super::message_blocks(message)?;
        let mut claude_content = Vec::new();

        for block in content_blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let text = block["text"].as_str().unwrap_or("");
                    claude_content.push(json!({
                        "type": "text",
                        "text": text,
                    }));
                }
                Some("image") => {
                    let uri = super::image_uri(&block)?;
                    claude_content.push(json!({
                        "type": "image",
                        "source": {
                            "type": "url",
                            "url": uri,
                        }
                    }));
                }
                Some("toolCall") => {
                    let id = block["id"].as_str().unwrap_or("tool-call");
                    let name = super::tool_name(&block)?;
                    let args = block.get("arguments").cloned().unwrap_or(json!({}));
                    claude_content.push(json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": args,
                    }));
                }
                _ => {}
            }
        }

        if role == "toolResult" {
            let call_id = message["toolCallId"].as_str().unwrap_or("");
            let result_content = match message.get("content") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => String::new(),
            };
            claude_content.push(json!({
                "type": "tool_result",
                "tool_use_id": call_id,
                "content": result_content,
            }));
        }

        let record_role = if role == "toolResult" { "user" } else { role };

        let record = json!({
            "type": "message",
            "id": msg_id,
            "parent": *parent,
            "sessionId": session_id,
            "cwd": cwd.to_string_lossy(),
            "timestamp": stamp,
            "message": {
                "role": record_role,
                "content": claude_content,
                "model": target_model,
            }
        });

        *parent = Some(msg_id);
        records.push(record);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn claude_target_retains_recorded_history_and_has_a_native_checkpoint() {
        let history = NativeHistory {
            session_id: "claude-test".into(),
            messages: vec![
                json!({
                    "role": "user",
                    "content": "user question",
                }),
                json!({
                    "role": "assistant",
                    "content": "assistant response",
                }),
            ],
            model_messages: Some(vec![json!({
                "role": "user",
                "content": "active question",
            })]),
            source_bytes: Vec::new(),
            source_context: None,
        };
        let recs = records(
            &history,
            "claude-session",
            Path::new("/tmp"),
            "claude-3-5-sonnet-20241022",
            "2026-10-18T00:00:00Z",
        )
        .unwrap();

        assert_eq!(recs.len(), 3);
        assert_eq!(recs[0]["message"]["role"], "user");
        assert_eq!(recs[1]["message"]["role"], "assistant");
        assert_eq!(recs[2]["type"], "system");
        assert_eq!(recs[2]["subtype"], "compactMetadata");
    }
}
