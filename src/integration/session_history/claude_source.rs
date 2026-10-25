use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;

use serde_json::{json, Value};

use super::{invalid, NativeHistory};

pub(crate) fn snapshot(
    path: &Path,
    expected_id: &str,
    expected_cwd: &Path,
) -> io::Result<NativeHistory> {
    let file = std::fs::File::open(path)?;
    let bytes = match crate::platform::read_limited_reader(file, 256 * 1024 * 1024)? {
        crate::platform::LimitedRead::Complete(bytes) => bytes,
        crate::platform::LimitedRead::Empty => return Err(invalid("native Claude source is empty")),
        crate::platform::LimitedRead::Oversized => {
            return Err(invalid("native source exceeds the exchange limit; nothing was truncated"))
        }
    };

    let mut records = Vec::new();
    for line in std::io::BufRead::lines(std::io::Cursor::new(&bytes)) {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(trimmed)
            .map_err(|_| invalid("native Claude source contains an invalid record"))?;
        records.push(record);
    }

    if records.is_empty() {
        return Err(invalid("native Claude source is empty"));
    }

    let branch = validate_linear_branch(&records)?;
    let mut messages = Vec::new();
    let mut model_messages = None;
    let mut source_context = None;

    for record in &branch {
        if record.get("sessionId").and_then(Value::as_str) != Some(expected_id) {
            return Err(invalid("native source snapshot identity changed"));
        }
        if let Some(cwd_str) = record.get("cwd").and_then(Value::as_str) {
            if Path::new(cwd_str).canonicalize().ok() != expected_cwd.canonicalize().ok() {
                return Err(invalid("native Claude source cwd missing"));
            }
        }

        let record_type = record.get("type").and_then(Value::as_str).unwrap_or("");
        if record_type == "system" && record.get("subtype").and_then(Value::as_str) == Some("compactMetadata") {
            if let Some(active) = record.pointer("/data/activeMessages").and_then(Value::as_array) {
                model_messages = Some(active.clone());
            }
        } else if record_type == "message" {
            let msg = decode_message(record)?;
            messages.push(msg);
        }
    }

    if let Some(first) = messages.first() {
        if let Some(text) = first.pointer("/content/0/text").and_then(Value::as_str) {
            if text.starts_with("Historical source configuration") {
                if let Some(start_idx) = text.find('{') {
                    if let Ok(ctx) = serde_json::from_str::<Value>(&text[start_idx..]) {
                        source_context = Some(ctx);
                    }
                }
            }
        }
    }

    Ok(NativeHistory {
        session_id: expected_id.to_owned(),
        messages,
        model_messages,
        source_bytes: bytes,
        source_context,
    })
}

fn validate_linear_branch(records: &[Value]) -> io::Result<Vec<Value>> {
    let mut by_id = HashMap::new();
    let mut children_of = HashMap::<String, Vec<String>>::new();
    let mut root_id = None;

    for record in records {
        if let Some(id) = record.get("id").and_then(Value::as_str) {
            by_id.insert(id.to_string(), record.clone());
            let parent = record.get("parent").and_then(Value::as_str);
            match parent {
                Some(p) => children_of.entry(p.to_string()).or_default().push(id.to_string()),
                None => {
                    if root_id.is_some() {
                        return Err(invalid("claude_source_refuses_ambiguous_transcript_branches"));
                    }
                    root_id = Some(id.to_string());
                }
            }
        }
    }

    // Verify linear branch
    for children in children_of.values() {
        if children.len() > 1 {
            return Err(invalid("claude_source_refuses_ambiguous_transcript_branches"));
        }
    }

    // Trace from root to leaf
    let mut ordered = Vec::new();
    let mut current = root_id;
    let mut seen = HashSet::new();

    while let Some(id) = current {
        if !seen.insert(id.clone()) {
            return Err(invalid("native session branch contains a cycle"));
        }
        if let Some(rec) = by_id.get(&id) {
            ordered.push(rec.clone());
        }
        current = children_of.get(&id).and_then(|c| c.first()).cloned();
    }

    if ordered.is_empty() {
        return Ok(records.to_vec());
    }
    Ok(ordered)
}

fn decode_message(record: &Value) -> io::Result<Value> {
    let msg_obj = record.get("message").ok_or_else(|| invalid("native Claude message content is unsupported"))?;
    let role = msg_obj.get("role").and_then(Value::as_str).unwrap_or("user");
    let content_val = msg_obj.get("content").ok_or_else(|| invalid("native Claude message content is unsupported"))?;

    let mut blocks = Vec::new();
    if let Some(text) = content_val.as_str() {
        blocks.push(json!({"type": "text", "text": text}));
    } else if let Some(arr) = content_val.as_array() {
        for item in arr {
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
            match item_type {
                "text" => {
                    blocks.push(json!({
                        "type": "text",
                        "text": item.get("text").and_then(Value::as_str).unwrap_or("")
                    }));
                }
                "image" => {
                    if let Some(url) = item.pointer("/source/url").and_then(Value::as_str) {
                        blocks.push(json!({
                            "type": "image",
                            "data": url,
                        }));
                    }
                }
                "tool_use" => {
                    blocks.push(json!({
                        "type": "toolCall",
                        "id": item.get("id").and_then(Value::as_str).unwrap_or(""),
                        "name": item.get("name").and_then(Value::as_str).unwrap_or(""),
                        "arguments": item.get("input").cloned().unwrap_or(json!({}))
                    }));
                }
                "tool_result" => {
                    return Ok(json!({
                        "role": "toolResult",
                        "toolCallId": item.get("tool_use_id").and_then(Value::as_str).unwrap_or(""),
                        "content": item.get("content").and_then(Value::as_str).unwrap_or("")
                    }));
                }
                _ => {}
            }
        }
    }

    Ok(json!({
        "role": role,
        "content": blocks,
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn claude_source_refuses_ambiguous_transcript_branches() {
        let (_root, _) = crate::platform::create_private_state_file(Path::new("/tmp/test.jsonl")).map(|_| (PathBuf::from("/tmp"), ())).unwrap_or((PathBuf::from("/tmp"), ()));
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
