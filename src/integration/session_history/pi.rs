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
        message_entry(&config_msg, &mut records, &mut parent, stamp, selection)?;
    }

    for message in &history.messages {
        message_entry(message, &mut records, &mut parent, stamp, selection)?;
    }

    if let Some(model_msgs) = &history.model_messages {
        let mut active_copies = Vec::new();
        for msg in model_msgs {
            active_copies.push(msg.clone());
        }
        let edit_id = format!("compaction-{}", &session_id[..session_id.len().min(8)]);
        records.push(json!({
            "type": "custom",
            "customType": "context_edit",
            "id": edit_id,
            "parentId": parent,
            "data": {
                "activeContextCopies": active_copies,
            }
        }));
    }

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
            "timestamp": stamp,
        }
    });

    if role == "assistant" {
        entry["message"]["modelId"] = json!(selection.model);
        entry["message"]["provider"] = json!(selection.provider);
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
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["type"], "session");
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
            .any(|row| row["type"] == "custom" && row["customType"] == "context_edit"));

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
