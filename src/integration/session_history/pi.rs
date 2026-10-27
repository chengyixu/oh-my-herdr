use std::io;
use std::path::Path;

use serde_json::{json, Value};

use super::{invalid, storage, NativeHistory};

#[derive(Debug, Clone)]
pub(crate) struct Selection {
    pub(crate) provider: String,
    pub(crate) model: String,
}

pub(crate) fn import_pi_history(
    history: &NativeHistory,
    cwd: &Path,
    pi_dir: &Path,
) -> io::Result<storage::ImportedHistory> {
    let script = include_str!("../assets/pi/native-config.mjs");
    let output = std::process::Command::new("node")
        .args(["-e", script, &pi_dir.to_string_lossy()])
        .output()?;
    if !output.status.success() {
        return Err(invalid("native Pi settings response was invalid"));
    }
    let settings: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| invalid("native Pi settings response was invalid"))?;
    let provider = settings["provider"]
        .as_str()
        .ok_or_else(|| invalid("native Pi default provider missing"))?
        .to_string();
    let model = settings["model"]
        .as_str()
        .ok_or_else(|| invalid("native Pi default model missing"))?
        .to_string();
    let selection = Selection { provider, model };

    let identity = storage::Identity::new(history)?;
    super::validate_tool_pairs(&history.messages)?;
    super::validate_native_tool_names(&history.messages)?;
    let records = pi_records(history, &identity.id, cwd, &identity.stamp, &selection)?;

    let cwd_str = cwd
        .to_str()
        .ok_or_else(|| invalid("native Pi cwd is not valid UTF-8"))?;
    let slug = cwd_str
        .trim_start_matches('/')
        .trim_start_matches('\\')
        .replace(['/', '\\', ':'], "--");
    let path = pi_dir
        .join("sessions")
        .join(format!("--{slug}--"))
        .join(format!("{}.jsonl", identity.id));

    let native_session = crate::agent_resume::PersistedAgentSession {
        source: "herdr:pi".into(),
        agent: "pi".into(),
        session_ref: crate::agent_resume::AgentSessionRef::path(
            path.to_string_lossy().into_owned(),
        )
        .ok_or_else(|| invalid("native Pi target session path is invalid"))?,
    };
    storage::publish(history, records, path, native_session, &identity.id)
}

fn pi_records(
    history: &NativeHistory,
    session_id: &str,
    cwd: &Path,
    stamp: &str,
    selection: &Selection,
) -> io::Result<Vec<Value>> {
    let mut records = Vec::new();
    records.push(json!({
        "type": "session",
        "version": 3,
        "id": session_id,
        "cwd": cwd.to_string_lossy(),
        "timestamp": stamp,
    }));

    let mut parent = Value::Null;
    if let Some(config_msg) = super::historical_source_configuration(history, "pi")? {
        let id = storage::entry_identity(session_id, records.len());
        records.push(json!({"type":"custom_message","customType":"ohmyherdr-source-configuration",
            "id":id,"parentId":parent,"timestamp":stamp,"display":false,"content":config_msg["content"]}));
        parent = json!(id);
    }

    for message in &history.messages {
        message_entry(message, &mut records, &mut parent, stamp, selection)?;
    }

    if let Some(model_msgs) = &history.model_messages {
        super::validate_tool_pairs(model_msgs)?;
        // Native append-only context edits keep original history visible while
        // omitting its older projection. The active checkpoint follows in order.
        let ids: Vec<_> = records
            .iter()
            .filter(|row| row["type"] == "message")
            .map(|row| row["id"].clone())
            .collect();
        for target in ids {
            let id = storage::entry_identity(session_id, records.len());
            records.push(json!({"type":"context_edit","id":id,"parentId":parent,"timestamp":stamp,"targetId":target,"replacement":null}));
            parent = json!(id);
        }
        for msg in model_msgs {
            message_entry(msg, &mut records, &mut parent, stamp, selection)?;
        }
    }

    // Preserve historical assistant provenance without letting it select the
    // target's model. Native model_change is the selection authority on resume.
    let id = storage::entry_identity(session_id, records.len());
    records.push(
        json!({"type":"model_change","id":id,"parentId":parent,"timestamp":stamp,
        "provider":selection.provider,"modelId":selection.model}),
    );
    Ok(records)
}

fn message_entry(
    message: &Value,
    records: &mut Vec<Value>,
    parent: &mut Value,
    stamp: &str,
    selection: &Selection,
) -> io::Result<()> {
    let id = storage::entry_identity("pi", records.len());
    let role = message["role"].as_str().unwrap_or("user");
    let content = message["content"].clone();

    let mut entry = json!({
        "type": "message",
        "id": id,
        "parentId": *parent,
        "message": {
            "role": role,
            "content": content,
            "timestamp": 0,
        }
    });
    entry["timestamp"] = json!(stamp);
    if role == "developer" {
        entry["message"]["role"] = json!("user");
    }
    if role == "toolResult" {
        entry["message"]["toolCallId"] = message["toolCallId"].clone();
        entry["message"]["toolName"] = message
            .get("toolName")
            .cloned()
            .unwrap_or(json!("imported_tool"));
        entry["message"]["isError"] = message.get("isError").cloned().unwrap_or(json!(false));
    }

    if role == "assistant" {
        entry["message"]["model"] = message
            .get("model")
            .filter(|v| v.is_string())
            .cloned()
            .unwrap_or(json!(selection.model));
        entry["message"]["provider"] = message
            .get("provider")
            .filter(|v| v.is_string())
            .cloned()
            .unwrap_or(json!(selection.provider));
        entry["message"]["api"] = json!("openai-responses");
        entry["message"]["stopReason"] = json!("stop");
        entry["message"]["usage"] = json!({"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}});
    }

    *parent = json!(id);
    records.push(entry);
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn pi_target_instruction_only_history_has_no_fabricated_conversation_turn() {
        let history = NativeHistory {
            session_id: "instruction-only".into(),
            messages: Vec::new(),
            model_messages: None,
            source_bytes: Vec::new(),
            source_context: None,
        };
        let selection = Selection {
            provider: "target-provider".into(),
            model: "target-model".into(),
        };
        let records = pi_records(
            &history,
            "target-id",
            Path::new("/owned-cwd"),
            "2026-10-18T00:00:00Z",
            &selection,
        )
        .unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["type"], "session");
        assert_eq!(records[1]["type"], "model_change");
        assert!(!records.iter().any(|row| row["type"] == "message"));
    }

    #[test]
    fn pi_target_preserves_complete_records_and_separates_active_context() {
        let history = NativeHistory {
            session_id: "complete-source".into(),
            messages: vec![
                json!({
                    "role": "user",
                    "content": "oldest-recorded-message",
                }),
                json!({
                    "role": "assistant",
                    "content": format!("untruncated-result-{}", "z".repeat(30_000)),
                }),
            ],
            model_messages: Some(vec![json!({
                "role": "user",
                "content": "exact-active-checkpoint",
            })]),
            source_bytes: Vec::new(),
            source_context: None,
        };
        let selection = Selection {
            provider: "target-provider".into(),
            model: "target-model".into(),
        };
        let records = pi_records(
            &history,
            "target-id",
            Path::new("/owned-cwd"),
            "2026-10-18T00:00:00Z",
            &selection,
        )
        .unwrap();

        assert!(records
            .iter()
            .any(|row| row["message"]["content"] == "oldest-recorded-message"));
        assert!(serde_json::to_string(&records)
            .unwrap()
            .contains(&format!("untruncated-result-{}", "z".repeat(30_000))));
        assert!(records
            .iter()
            .any(|row| row["type"] == "context_edit" && row["replacement"].is_null()));

        let serialized = serde_json::to_string(&records).unwrap();
        assert!(serialized.contains("exact-active-checkpoint"));
    }

    #[test]
    fn imported_pi_context_round_trips_without_recorded_history_duplication() {
        let history = NativeHistory {
            session_id: "round-trip".into(),
            messages: vec![json!({
                "role": "user",
                "content": "oldest-recorded-message",
            })],
            model_messages: Some(vec![json!({
                "role": "user",
                "content": "exact-active-checkpoint",
            })]),
            source_bytes: Vec::new(),
            source_context: Some(json!({"instructions": "source instructions"})),
        };
        let selection = Selection {
            provider: "target-provider".into(),
            model: "target-model".into(),
        };
        let records = pi_records(
            &history,
            "target-id",
            Path::new("/owned-cwd"),
            "2026-10-18T00:00:00Z",
            &selection,
        )
        .unwrap();

        let visible = serde_json::to_string(&records).unwrap();
        assert!(visible.contains("oldest-recorded-message"));
        assert!(visible.contains("Historical source configuration"));
    }
}
