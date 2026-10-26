use super::{invalid, NativeHistory};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;

pub(crate) fn snapshot(
    path: &Path,
    expected_id: &str,
    expected_cwd: &Path,
) -> io::Result<NativeHistory> {
    decode(super::read_source_bytes(path)?, expected_id, expected_cwd)
}

pub(crate) fn decode(bytes: Vec<u8>, expected_id: &str, cwd: &Path) -> io::Result<NativeHistory> {
    let records = super::source_records(&bytes)?;
    let branch = validate_linear_branch(&records)?;
    let mut messages = Vec::new();
    let mut active = None::<Vec<Value>>;
    let mut source_context = None;
    for row in &branch {
        let kind = row["type"]
            .as_str()
            .ok_or_else(|| invalid("Claude record type missing"))?;
        match kind {
            "user" | "assistant" | "system" => {
                if row["sessionId"] != expected_id {
                    return Err(invalid("native Claude session identity changed"));
                }
                if let Some(path) = row["cwd"].as_str() {
                    if Path::new(path).canonicalize()? != cwd.canonicalize()? {
                        return Err(invalid("native Claude working directory changed"));
                    }
                }
                if kind == "system" {
                    match row["subtype"].as_str() {
                        Some("compact_boundary") => {
                            // Native Claude resumes model context after the latest boundary.
                            // Older records remain in the transcript for UI/history.
                            active = Some(Vec::new());
                        }
                        Some(
                            "turn_duration" | "stop_hook_summary" | "api_error" | "local_command",
                        ) => {}
                        _ => {
                            return Err(invalid(
                                "unqualified Claude system record; source retained",
                            ))
                        }
                    }
                } else {
                    let decoded = decode_message(&row["message"])?;
                    if let Some(active) = &mut active {
                        active.extend(decoded.clone());
                    }
                    messages.extend(decoded);
                }
            }
            "attachment" => {
                if row["sessionId"] != expected_id {
                    return Err(invalid("native attachment identity changed"));
                }
                if row["attachment"]["type"] == "prompt_snapshot" {
                    source_context = Some(
                        json!({"system_prompt":row["attachment"]["systemPrompt"].to_string()}),
                    );
                } else if let Some(rendered) = row["rendered"].as_array() {
                    let mut blocks = Vec::new();
                    for block in rendered {
                        let text = block["content"]
                            .as_str()
                            .ok_or_else(|| invalid("unsupported native attachment rendering"))?;
                        blocks.push(json!({"type":"text","text":text}));
                    }
                    let message = json!({"role":"user","content":blocks});
                    if let Some(active) = &mut active {
                        active.push(message.clone());
                    }
                    messages.push(message);
                } else if row["attachment"]["type"] != "session_context" {
                    return Err(invalid("unsupported native attachment; source retained"));
                }
            }
            "api-request" | "api-request-shape" | "api-request-blob" => {}
            // Native metadata and UI diagnostics are archived but do not enter model context.
            "progress"
            | "file-history-snapshot"
            | "queue-operation"
            | "last-prompt"
            | "summary"
            | "permission-mode"
            | "atis-latch"
            | "cost-state"
            | "mode" => {}
            _ => {
                return Err(invalid(
                    "unqualified Claude transcript record; no records discarded",
                ))
            }
        }
    }
    if messages.is_empty() {
        return Err(invalid("native Claude transcript has no messages"));
    }
    super::validate_tool_pairs(&messages)?;
    Ok(NativeHistory {
        session_id: expected_id.into(),
        messages,
        model_messages: active,
        source_bytes: bytes,
        source_context,
    })
}

fn validate_linear_branch(records: &[Value]) -> io::Result<Vec<Value>> {
    let relevant = |row: &Value| {
        matches!(
            row["type"].as_str(),
            Some("user" | "assistant" | "system" | "message" | "attachment")
        )
    };
    let id = |row: &Value| {
        row["uuid"]
            .as_str()
            .or_else(|| row["id"].as_str())
            .map(str::to_owned)
    };
    let mut nodes = HashMap::new();
    let mut children = HashMap::<String, String>::new();
    let mut roots = Vec::new();
    for row in records.iter().filter(|r| relevant(r)) {
        let key = id(row).ok_or_else(|| invalid("Claude native record identity missing"))?;
        if nodes.insert(key, row).is_some() {
            return Err(invalid("duplicate Claude transcript identity"));
        }
        if row["isSidechain"] == true {
            return Err(invalid("ambiguous Claude sidechain"));
        }
    }
    for row in records.iter().filter(|r| relevant(r)) {
        let key = id(row).ok_or_else(|| invalid("Claude native record identity missing"))?;
        if let Some(parent) = row["parentUuid"]
            .as_str()
            .or_else(|| row["parent"].as_str())
        {
            if !nodes.contains_key(parent) {
                return Err(invalid("native Claude transcript parent missing"));
            }
            if children.insert(parent.into(), key).is_some() {
                return Err(invalid("ambiguous Claude transcript branches"));
            }
        } else {
            if !roots.is_empty() && row["subtype"] != "compact_boundary" {
                return Err(invalid("ambiguous Claude transcript roots"));
            }
            roots.push(key);
        }
    }
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for root in roots {
        let mut current = Some(root);
        while let Some(key) = current {
            if !seen.insert(key.clone()) {
                return Err(invalid("Claude transcript cycle"));
            }
            ordered.push(
                (*nodes
                    .get(&key)
                    .ok_or_else(|| invalid("Claude transcript parent missing"))?)
                .clone(),
            );
            current = children.get(&key).cloned();
        }
    }
    if seen.len() != nodes.len() {
        return Err(invalid("Claude transcript cycle or detached branch"));
    }
    ordered.extend(records.iter().filter(|r| !relevant(r)).cloned());
    Ok(ordered)
}

fn content_blocks(value: &Value) -> io::Result<Vec<Value>> {
    if let Some(text) = value.as_str() {
        return Ok(vec![json!({"type":"text","text":text})]);
    }
    let array = value
        .as_array()
        .ok_or_else(|| invalid("Claude content is unsupported"))?;
    array.iter().map(|block| match block["type"].as_str() {
        Some("text") if block["text"].is_string() => Ok(block.clone()),
        Some("image") if block["source"]["type"] == "base64" => {
            let image = json!({"type":"image","data":block["source"]["data"],"mimeType":block["source"]["media_type"]});
            super::image_uri(&image)?; Ok(image)
        }
        Some("tool_use") if block["id"].is_string() && block["name"].is_string() && block["input"].is_object() =>
            Ok(json!({"type":"toolCall","id":block["id"],"name":block["name"],"arguments":block["input"]})),
        Some("thinking") if block["thinking"].is_string() =>
            Ok(json!({"type":"text","text":format!("[Recorded source reasoning]\n{}\n[/Recorded source reasoning]",block["thinking"].as_str().unwrap_or_default())})),
        _ => Err(invalid("Claude contains opaque or unsupported content; source retained")),
    }).collect()
}

fn decode_message(message: &Value) -> io::Result<Vec<Value>> {
    let role = message["role"]
        .as_str()
        .ok_or_else(|| invalid("Claude message role missing"))?;
    if !matches!(role, "user" | "assistant") {
        return Err(invalid("unsupported Claude role"));
    }
    let raw = &message["content"];
    let mut result = Vec::new();
    // Tool results are user blocks in Claude. Keep every result and adjacent text;
    // a return from the middle of the array used to drop the remaining blocks.
    if let Some(blocks) = raw.as_array() {
        for block in blocks {
            if block["type"] == "tool_result" {
                let id = block["tool_use_id"]
                    .as_str()
                    .ok_or_else(|| invalid("tool result identity missing"))?;
                result.push(json!({"role":"toolResult","toolCallId":id,"isError":block["is_error"].as_bool().unwrap_or(false),"content":content_blocks(&block["content"])?}));
            } else {
                let mut msg = json!({"role":role,"content":content_blocks(&json!([block]))?});
                if role == "assistant" {
                    msg["model"] = message["model"].clone();
                }
                result.push(msg);
            }
        }
    } else {
        result.push(json!({"role":role,"content":content_blocks(raw)?}));
    }
    Ok(result)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn native_roles_and_checkpoint_keep_post_compaction_messages_and_tools() {
        let root = std::env::temp_dir().join(format!("native-claude-red-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("source.jsonl");
        let rows = [
            json!({"type":"user","uuid":"u","parentUuid":null,"sessionId":"s","cwd":root,"message":{"role":"user","content":"oldest"}}),
            json!({"type":"system","subtype":"compact_boundary","uuid":"c","parentUuid":"u","sessionId":"s","cwd":root,"compactMetadata":{"trigger":"auto","preTokens":4000}}),
            json!({"type":"user","uuid":"s","parentUuid":"c","sessionId":"s","cwd":root,"isCompactSummary":true,"message":{"role":"user","content":"summary"}}),
            json!({"type":"assistant","uuid":"a","parentUuid":"s","sessionId":"s","cwd":root,"message":{"role":"assistant","model":"source-model","content":[{"type":"tool_use","id":"t","name":"read","input":{"path":"full"}}]}}),
            json!({"type":"user","uuid":"r","parentUuid":"a","sessionId":"s","cwd":root,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":[{"type":"text","text":"tail"}]}]}}),
        ];
        std::fs::write(
            &path,
            rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
        )
        .unwrap();
        let h = snapshot(&path, "s", &root).unwrap();
        assert!(serde_json::to_string(&h.messages)
            .unwrap()
            .contains("oldest"));
        let active = serde_json::to_string(&h.model_messages).unwrap();
        assert!(
            active.contains("summary") && active.contains("tail") && !active.contains("oldest"),
            "{active}"
        );
        assert_eq!(h.messages.last().unwrap()["toolCallId"], "t");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_parent_links_order_late_flushed_attachments_before_the_answer() {
        let rows = [
            json!({"type":"user","uuid":"u","parentUuid":null}),
            json!({"type":"assistant","uuid":"a","parentUuid":"environment"}),
            json!({"type":"attachment","uuid":"environment","parentUuid":"u","rendered":[{"content":"native environment"}]}),
        ];
        let ordered = validate_linear_branch(&rows).unwrap();
        assert_eq!(
            ordered
                .iter()
                .map(|r| r["uuid"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["u", "environment", "a"]
        );
    }

    #[test]
    fn claude_source_refuses_ambiguous_transcript_branches() {
        let records = vec![
            json!({"type": "message", "id": "msg-1", "sessionId": "s-1", "cwd": "/tmp"}),
            json!({"type": "message", "id": "msg-2a", "parent": "msg-1", "sessionId": "s-1", "cwd": "/tmp"}),
            json!({"type": "message", "id": "msg-2b", "parent": "msg-1", "sessionId": "s-1", "cwd": "/tmp"}),
        ];
        assert!(validate_linear_branch(&records).is_err());
    }

    #[test]
    fn claude_native_source_keeps_recorded_prefix_and_separates_compacted_context() {
        let records = vec![
            json!({
                "type": "message",
                "id": "msg-1",
                "sessionId": "s-1",
                "cwd": "/tmp",
                "message": {"role": "user", "content": [{"type": "text", "text": "hello"}]}
            }),
            json!({
                "type": "system",
                "subtype": "compactMetadata",
                "id": "compact-1",
                "parent": "msg-1",
                "sessionId": "s-1",
                "cwd": "/tmp",
                "data": {"activeMessages": [{"role": "user", "content": "active"}]}
            }),
        ];
        let branch = validate_linear_branch(&records).unwrap();
        assert_eq!(branch.len(), 2);
    }
}
