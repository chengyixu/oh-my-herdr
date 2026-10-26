use super::{invalid, storage, NativeHistory};
use serde_json::{json, Value};
use std::io;
use std::path::Path;

pub(crate) fn import_claude_history(
    history: &NativeHistory,
    cwd: &Path,
    claude_dir: &Path,
) -> io::Result<storage::ImportedHistory> {
    let version = super::super::native_process::capture("claude", &["--version"], cwd, 4096)?;
    if !matches!(
        String::from_utf8_lossy(&version).trim(),
        "2.1.302 (Claude Code)" | "2.1.310 (Claude Code)"
    ) {
        return Err(invalid(
            "installed Claude version requires native import qualification; source retained",
        ));
    }
    let identity = storage::Identity::new(history)?;
    let records = records(
        history,
        &identity.id,
        cwd,
        "imported-source-history",
        &identity.stamp,
    )?;
    let canonical = cwd.canonicalize()?;
    let slug: String = canonical
        .to_str()
        .ok_or_else(|| invalid("Claude cwd is not UTF-8"))?
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if slug.len() > 200 {
        return Err(invalid("Claude long-path layout requires qualification"));
    }
    let path = claude_dir
        .join("projects")
        .join(slug)
        .join(format!("{}.jsonl", identity.id));
    let session = crate::agent_resume::PersistedAgentSession {
        source: "herdr:claude".into(),
        agent: "claude".into(),
        session_ref: crate::agent_resume::AgentSessionRef::id(identity.id.clone())
            .ok_or_else(|| invalid("invalid Claude identity"))?,
    };
    storage::publish(history, records, path, session, &identity.id)
}

fn records(
    history: &NativeHistory,
    session_id: &str,
    cwd: &Path,
    model: &str,
    stamp: &str,
) -> io::Result<Vec<Value>> {
    super::validate_tool_pairs(&history.messages)?;
    super::validate_native_tool_names(&history.messages)?;
    let mut records = Vec::new();
    let mut parent = None;
    if let Some(config) = super::historical_source_configuration(history, "claude")? {
        append_messages(
            &mut records,
            &mut parent,
            &[config],
            session_id,
            cwd,
            model,
            stamp,
        )?;
    }
    append_messages(
        &mut records,
        &mut parent,
        &history.messages,
        session_id,
        cwd,
        model,
        stamp,
    )?;
    if let Some(active) = &history.model_messages {
        super::validate_tool_pairs(active)?;
        let id = storage::uuid_from_seed(&format!("{session_id}:checkpoint"));
        records.push(json!({"type":"system","subtype":"compact_boundary","uuid":id,"parentUuid":parent,
            "sessionId":session_id,"cwd":cwd,"version":"2.1.310","isSidechain":false,"timestamp":stamp,
            "content":"Imported native active context; complete history retained",
            "compactMetadata":{"trigger":"auto","preTokens":0}}));
        parent = Some(id);
        let start = records.len();
        append_messages(
            &mut records,
            &mut parent,
            active,
            session_id,
            cwd,
            model,
            stamp,
        )?;
        if let Some(row) = records.get_mut(start) {
            row["isCompactSummary"] = json!(true);
        }
    }
    Ok(records)
}

fn claude_blocks(message: &Value) -> io::Result<Vec<Value>> {
    super::message_blocks(message)?.iter().map(|block| match block["type"].as_str() {
        Some("text") if block["text"].is_string()=>Ok(block.clone()),
        Some("image")=> { super::image_uri(block)?; Ok(json!({"type":"image","source":{"type":"base64","media_type":block["mimeType"],"data":block["data"]}})) },
        Some("toolCall")=>Ok(json!({"type":"tool_use","id":block["id"],"name":super::tool_name(block)?,"input":block["arguments"]})),
        _=>Err(invalid("unsupported Claude import content; source retained")),
    }).collect()
}

fn append_messages(
    records: &mut Vec<Value>,
    parent: &mut Option<String>,
    messages: &[Value],
    session_id: &str,
    cwd: &Path,
    model: &str,
    stamp: &str,
) -> io::Result<()> {
    for message in messages {
        let id = storage::uuid_from_seed(&format!("{session_id}:msg:{}", records.len()));
        let role = message["role"]
            .as_str()
            .ok_or_else(|| invalid("import message role missing"))?;
        let (native_role, content) = match role {
            "toolResult" => (
                "user",
                vec![
                    json!({"type":"tool_result","tool_use_id":message["toolCallId"],"content":claude_blocks(message)?,"is_error":message["isError"].as_bool().unwrap_or(false)}),
                ],
            ),
            "developer" => ("user", claude_blocks(message)?),
            "user" | "assistant" => (role, claude_blocks(message)?),
            _ => return Err(invalid("unsupported Claude import role")),
        };
        let mut msg = json!({"role":native_role,"content":content});
        if native_role == "assistant" {
            msg["id"] = json!(format!("msg_{id}"));
            msg["type"] = json!("message");
            msg["model"] = message
                .get("model")
                .filter(|v| v.is_string())
                .cloned()
                .unwrap_or(json!(model));
            msg["stop_reason"] = json!("end_turn");
            msg["stop_sequence"] = Value::Null;
            msg["usage"] = json!({"input_tokens":0,"output_tokens":0});
        }
        records.push(json!({"type":native_role,"uuid":id,"parentUuid":*parent,"isSidechain":false,
            "userType":"external","cwd":cwd,"sessionId":session_id,"version":"2.1.310","timestamp":stamp,"message":msg}));
        *parent = Some(id);
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

        assert_eq!(recs.len(), 4);
        assert_eq!(recs[0]["type"], "user");
        assert!(recs[0]["uuid"].is_string());
        assert_eq!(recs[0]["message"]["role"], "user");
        assert_eq!(recs[1]["message"]["role"], "assistant");
        assert_eq!(recs[2]["type"], "system");
        assert_eq!(recs[2]["subtype"], "compact_boundary");
    }
}
