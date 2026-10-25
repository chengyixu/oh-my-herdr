use std::io;
use std::path::Path;

use serde_json::{json, Value};

use super::{invalid, NativeHistory};

pub(crate) fn read_legacy_file(path: &Path, expected_id: &str) -> io::Result<NativeHistory> {
    let file = std::fs::File::open(path)?;
    let bytes = match crate::platform::read_limited_reader(file, 256 * 1024 * 1024)? {
        crate::platform::LimitedRead::Complete(bytes) => bytes,
        crate::platform::LimitedRead::Empty => return Err(invalid("native session is empty")),
        crate::platform::LimitedRead::Oversized => {
            return Err(invalid("native source exceeds the exchange limit; nothing was truncated"))
        }
    };

    let mut provider = String::new();
    let mut model = String::new();
    let mut messages = Vec::new();
    let mut model_messages = None;
    let mut source_context = None;
    let mut pending_items = Vec::new();
    let mut verified_id = false;

    for line in std::io::BufRead::lines(std::io::Cursor::new(&bytes)) {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(trimmed)
            .map_err(|_| invalid("native source snapshot identity changed"))?;

        let record_type = record.get("type").and_then(Value::as_str).unwrap_or("");
        match record_type {
            "session_meta" => {
                let id = record.pointer("/payload/id").and_then(Value::as_str);
                if id != Some(expected_id) {
                    return Err(invalid("native source snapshot identity changed"));
                }
                verified_id = true;
                if let Some(p) = record.pointer("/payload/model_provider").and_then(Value::as_str) {
                    provider = p.to_string();
                }
            }
            "turn_context" => {
                if let Some(m) = record.pointer("/payload/model").and_then(Value::as_str) {
                    model = m.to_string();
                }
            }
            "response_item" => {
                if let Some(item) = record.get("payload") {
                    pending_items.push(item.clone());
                }
            }
            "compacted" => {
                flush_items(&mut pending_items, &mut messages, &provider, &model)?;
                if let Some(history) = record.pointer("/payload/replacement_history").and_then(Value::as_array) {
                    let decoded_active = decode_items(history, &provider, &model)?;
                    model_messages = Some(decoded_active);
                }
            }
            _ => {
                return Err(invalid("this source has opaque reasoning that this integration cannot transfer; original session retained"));
            }
        }
    }

    if !verified_id {
        return Err(invalid("native source snapshot identity changed"));
    }

    flush_items(&mut pending_items, &mut messages, &provider, &model)?;

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

fn flush_items(
    items: &mut Vec<Value>,
    messages: &mut Vec<Value>,
    provider: &str,
    model: &str,
) -> io::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let drained: Vec<Value> = std::mem::take(items);
    let mut decoded = decode_items(&drained, provider, model)?;
    messages.append(&mut decoded);
    Ok(())
}

fn decode_items(items: &[Value], provider: &str, model: &str) -> io::Result<Vec<Value>> {
    let mut messages = Vec::new();

    for item in items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        match item_type {
            "message" => {
                let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
                let mut content = Vec::new();
                if let Some(text) = item.get("content").and_then(Value::as_str) {
                    content.push(json!({"type": "text", "text": text}));
                } else if let Some(arr) = item.get("content").and_then(Value::as_array) {
                    for part in arr {
                        if let Some(t) = part.get("text").and_then(Value::as_str) {
                            content.push(json!({"type": "text", "text": t}));
                        } else if let Some(t) = part.get("input_text").and_then(Value::as_str) {
                            content.push(json!({"type": "text", "text": t}));
                        } else if let Some(t) = part.get("output_text").and_then(Value::as_str) {
                            content.push(json!({"type": "text", "text": t}));
                        }
                    }
                }
                let mut msg = json!({
                    "role": role,
                    "content": content,
                });
                provenance(&mut msg, provider, model)?;
                messages.push(msg);
            }
            "function_call" => {
                let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
                let name = item.get("name").and_then(Value::as_str).unwrap_or("");
                let args_str = item.get("arguments").and_then(Value::as_str).unwrap_or("{}");
                let args: Value = serde_json::from_str(args_str).unwrap_or(json!({}));
                let mut msg = json!({
                    "role": "assistant",
                    "content": [{
                        "type": "toolCall",
                        "id": call_id,
                        "name": name,
                        "arguments": args,
                    }]
                });
                provenance(&mut msg, provider, model)?;
                messages.push(msg);
            }
            "function_call_output" => {
                let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
                let output = item.get("output").and_then(Value::as_str).unwrap_or("");
                messages.push(json!({
                    "role": "toolResult",
                    "toolCallId": call_id,
                    "content": output,
                }));
            }
            _ => {
                return Err(invalid("this source has opaque reasoning that this integration cannot transfer; original session retained"));
            }
        }
    }

    Ok(messages)
}

fn provenance(message: &mut Value, provider: &str, model: &str) -> io::Result<()> {
    if !provider.is_empty() {
        message["provider"] = json!(provider);
    }
    if !model.is_empty() {
        message["model"] = json!(model);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn codex_source_refuses_unknown_opaque_or_unfinished_records() {
        let item = json!({"type": "unknown_future_record"});
        assert!(decode_items(&[item], "source", "source-model").is_err());
    }

    #[test]
    fn codex_source_preserves_messages_namespaces_arguments_and_full_output() {
        let temp_dir = std::env::temp_dir().join(format!("oh-codex-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let path = temp_dir.join("test-rollout.jsonl");

        let lines = vec![
            json!({"type": "session_meta", "payload": {"id": "source-id", "cli_version": "0.163.0", "model_provider": "native-provider"}}),
            json!({"type": "turn_context", "payload": {"model": "native-source-model"}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": "oldest-codex-source"}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "assistant", "content": "recorded-native-answer"}}),
            json!({"type": "response_item", "payload": {"type": "function_call", "call_id": "call-source", "name": "functions.read", "arguments": "{\"path\":\"exact-source-arguments\"}"}}),
            json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": "call-source", "output": format!("native-result-{}", "z".repeat(30_000))}}),
        ];

        let mut content = String::new();
        for l in lines {
            content.push_str(&l.to_string());
            content.push('\n');
        }
        std::fs::write(&path, &content).unwrap();

        let history = read_legacy_file(&path, "source-id").unwrap();
        let text = serde_json::to_string(&history.messages).unwrap();
        assert!(text.contains("oldest-codex-source"));
        assert!(text.contains("recorded-native-answer"));
        assert!(text.contains("exact-source-arguments"));
        assert!(text.contains(&format!("native-result-{}", "z".repeat(30_000))));

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn native_codex_legacy_file_keeps_full_records_and_uses_its_checkpoint() {
        let temp_dir = std::env::temp_dir().join(format!("oh-codex-chk-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let path = temp_dir.join("checkpoint-rollout.jsonl");

        let lines = vec![
            json!({"type": "session_meta", "payload": {"id": "chk-id", "cli_version": "0.163.0", "model_provider": "native-provider"}}),
            json!({"type": "turn_context", "payload": {"model": "native-source-model"}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": "oldest-recorded-native-message"}}),
            json!({"type": "compacted", "payload": {"replacement_history": [{"type": "message", "role": "user", "content": "exact-native-checkpoint"}]}}),
        ];

        let mut content = String::new();
        for l in lines {
            content.push_str(&l.to_string());
            content.push('\n');
        }
        std::fs::write(&path, &content).unwrap();

        let history = read_legacy_file(&path, "chk-id").unwrap();
        assert!(history.model_messages.is_some());
        let active_text = serde_json::to_string(&history.model_messages).unwrap();
        assert!(active_text.contains("exact-native-checkpoint"));

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
