use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{invalid, storage, NativeHistory};

struct Seed {
    session_id: String,
    directory: PathBuf,
}

impl Drop for Seed {
    fn drop(&mut self) {
        // If not committed/published, clean up
    }
}

pub(crate) fn import_grok_history(
    history: &NativeHistory,
    cwd: &Path,
    grok_dir: &Path,
) -> io::Result<storage::ImportedHistory> {
    // Grok does not support images in native import
    for message in &history.messages {
        for block in super::message_blocks(message)? {
            if block.get("type").and_then(Value::as_str) == Some("image") {
                return Err(invalid(
                    "Grok image/non-text import is not qualified; source retained",
                ));
            }
        }
    }

    let seed = allocate_seed_session(cwd, grok_dir)?;
    let session_id = seed.session_id.clone();
    let session_dir = seed.directory.clone();

    let identity = storage::Identity::new(history)?;
    let chat_lines = chat_records(history, &session_id, cwd, &identity.stamp)?;
    let updates_lines = updates(history, &session_id, &identity.stamp)?;

    let chat_path = session_dir.join("chat_history.jsonl");
    let updates_path = session_dir.join("updates.jsonl");
    let summary_path = session_dir.join("summary.json");
    let canonical_path = session_dir.join("ohmyherdr-canonical.json");

    let chat_bytes = jsonl(&chat_lines)?;
    let updates_bytes = jsonl(&updates_lines)?;

    let chat_sha = format!("{:x}", Sha256::digest(&chat_bytes));
    let updates_sha = format!("{:x}", Sha256::digest(&updates_bytes));

    let summary = json!({
        "sessionId": session_id,
        "title": "Imported conversation",
        "created": identity.stamp,
        "updated": identity.stamp,
        "chatSha256": chat_sha,
        "updatesSha256": updates_sha,
    });

    let canonical = json!({
        "version": 1,
        "sourceSession": history.session_id,
        "active": session_id,
        "chatSha256": chat_sha,
        "updatesSha256": updates_sha,
    });

    std::fs::write(&chat_path, &chat_bytes)?;
    std::fs::write(&updates_path, &updates_bytes)?;
    std::fs::write(&summary_path, serde_json::to_vec_pretty(&summary)?)?;
    std::fs::write(&canonical_path, serde_json::to_vec_pretty(&canonical)?)?;

    let native_session = crate::agent_resume::PersistedAgentSession {
        source: "herdr:grok".into(),
        agent: "grok".into(),
        session_ref: crate::agent_resume::AgentSessionRef::id(session_id.clone())
            .ok_or_else(|| invalid("native Grok identity invalid"))?,
    };

    let mut imported =
        storage::publish(history, chat_lines, chat_path, native_session, &session_id)?;
    imported.own_new_directory(session_dir);
    Ok(imported)
}

fn allocate_seed_session(cwd: &Path, grok_dir: &Path) -> io::Result<Seed> {
    let mut acp = super::super::native_acp::Discovery::start("grok", &["acp"], cwd)?;
    let result = acp.call("session/new", json!({"cwd": cwd.to_string_lossy()}))?;
    let session_id = result
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("native Grok did not allocate an exact session"))?
        .to_string();

    let encoded_cwd = urlencoding_path(cwd);
    let session_dir = grok_dir
        .join("sessions")
        .join(&encoded_cwd)
        .join(&session_id);
    if !session_dir.is_dir() {
        return Err(invalid("native Grok did not allocate an exact session"));
    }

    Ok(Seed {
        session_id,
        directory: session_dir,
    })
}

fn urlencoding_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    s.replace('/', "%2F").replace('\\', "%5C")
}

fn jsonl(records: &[Value]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    for rec in records {
        serde_json::to_writer(&mut out, rec)?;
        out.write_all(b"\n")?;
    }
    Ok(out)
}

fn chat_records(
    history: &NativeHistory,
    session_id: &str,
    _cwd: &Path,
    stamp: &str,
) -> io::Result<Vec<Value>> {
    let mut records = Vec::new();
    let mut prompt_index = 0u64;

    if let Some(config_msg) = super::historical_source_configuration(history, "grok")? {
        records.push(json!({
            "sessionId": session_id,
            "role": "system",
            "prompt_index": prompt_index,
            "content": text_blocks(&config_msg)?,
            "timestamp": stamp,
        }));
        prompt_index += 1;
    }

    for message in &history.messages {
        let role = message["role"].as_str().unwrap_or("user");
        let content = text_blocks(message)?;
        records.push(json!({
            "sessionId": session_id,
            "role": role,
            "prompt_index": prompt_index,
            "content": content,
            "timestamp": stamp,
        }));
        prompt_index += 1;
    }

    Ok(records)
}

fn updates(history: &NativeHistory, session_id: &str, stamp: &str) -> io::Result<Vec<Value>> {
    let mut records = Vec::new();
    for (i, msg) in history.messages.iter().enumerate() {
        records.push(json!({
            "sessionId": session_id,
            "seq": i,
            "role": msg["role"],
            "timestamp": stamp,
        }));
    }
    Ok(records)
}

fn text_blocks(message: &Value) -> io::Result<String> {
    let blocks = super::message_blocks(message)?;
    let mut full = String::new();
    for b in blocks {
        if let Some(text) = b.get("text").and_then(Value::as_str) {
            full.push_str(text);
        }
    }
    if full.is_empty() {
        return Err(invalid("native text missing"));
    }
    Ok(full)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn grok_checkpoint_keeps_complete_tool_payload_and_result() {
        let history = NativeHistory {
            session_id: "grok-history".into(),
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
            model_messages: None,
            source_bytes: Vec::new(),
            source_context: None,
        };

        let lines = chat_records(
            &history,
            "grok-sess",
            Path::new("/tmp"),
            "2026-10-18T00:00:00Z",
        )
        .unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["role"], "user");
        assert_eq!(lines[1]["role"], "assistant");
    }
}
