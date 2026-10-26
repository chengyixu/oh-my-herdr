use super::{invalid, NativeHistory};
use serde_json::{json, Value};
use std::io;
use std::path::Path;

pub(crate) fn read_legacy_file(path: &Path, expected_id: &str) -> io::Result<NativeHistory> {
    decode(super::read_source_bytes(path)?, expected_id)
}

pub(crate) fn decode(bytes: Vec<u8>, expected_id: &str) -> io::Result<NativeHistory> {
    let records = super::source_records(&bytes)?;
    let mut provider = String::new();
    let mut model = String::new();
    let mut messages = Vec::new();
    let mut active = None::<Vec<Value>>;
    let mut verified = false;
    let mut world_state = Value::Null;
    for row in records {
        match row["type"].as_str() {
            Some("session_meta") if !verified && row["payload"]["id"] == expected_id => {
                if row["payload"]["history_mode"] == "paginated" {
                    return Err(invalid(
                        "paginated Codex source requires a complete native export; source retained",
                    ));
                }
                verified = true;
                provider = row["payload"]["model_provider"]
                    .as_str()
                    .unwrap_or_default()
                    .into();
            }
            Some("turn_context") => {
                model = row["payload"]["model"].as_str().unwrap_or_default().into();
            }
            Some("response_item") if verified => {
                let decoded = decode_items(&[row["payload"].clone()], &provider, &model)?;
                if let Some(active) = &mut active {
                    active.extend(decoded.clone());
                }
                messages.extend(decoded);
            }
            Some("compacted") if verified => {
                let replacement = row["payload"]["replacement_history"]
                    .as_array()
                    .ok_or_else(|| invalid("Codex checkpoint has no explicit active context"))?;
                active = Some(decode_items(replacement, &provider, &model)?);
            }
            // Native events duplicate response items for UI; never duplicate them
            // in the model projection. All raw records remain archived.
            Some("world_state") if verified => {
                // Native prompt/environment checkpoint; retain it as historical
                // configuration without reinterpreting it as target authority.
                world_state = row["payload"]["state"].clone();
            }
            Some("token_usage_record") if verified => {}
            Some("event_msg") if verified => {}
            _ => {
                return Err(invalid(
                    "Codex contains an unknown record or changed identity; source retained",
                ))
            }
        }
    }
    if !verified {
        return Err(invalid("native Codex session identity missing"));
    }
    super::validate_tool_pairs(&messages)?;
    Ok(NativeHistory {
        session_id: expected_id.into(),
        messages,
        model_messages: active,
        source_bytes: bytes,
        source_context: (!world_state.is_null())
            .then(|| json!({"system_prompt":world_state.to_string()})),
    })
}

fn blocks(value: &Value) -> io::Result<Vec<Value>> {
    if let Some(text) = value.as_str() {
        return Ok(vec![json!({"type":"text","text":text})]);
    }
    value
        .as_array()
        .ok_or_else(|| invalid("Codex message content missing"))?
        .iter()
        .map(|part| match part["type"].as_str() {
            Some("input_text" | "output_text" | "text") if part["text"].is_string() => {
                Ok(json!({"type":"text","text":part["text"]}))
            }
            Some("input_image") => {
                let uri = part["image_url"]
                    .as_str()
                    .ok_or_else(|| invalid("Codex image URL missing"))?;
                let (mime, data) = uri
                    .strip_prefix("data:")
                    .and_then(|s| s.split_once(";base64,"))
                    .ok_or_else(|| {
                        invalid("remote/local Codex image requires qualification; source retained")
                    })?;
                let image = json!({"type":"image","mimeType":mime,"data":data});
                super::image_uri(&image)?;
                Ok(image)
            }
            _ => Err(invalid("unsupported Codex content; source retained")),
        })
        .collect()
}

fn decode_items(items: &[Value], provider: &str, model: &str) -> io::Result<Vec<Value>> {
    let mut messages = Vec::new();
    for item in items {
        let mut message = match item["type"].as_str() {
            Some("message") => {
                let role = item["role"]
                    .as_str()
                    .ok_or_else(|| invalid("Codex message role missing"))?;
                if !matches!(role, "user" | "assistant" | "developer" | "system") {
                    return Err(invalid("unsupported Codex message role"));
                }
                json!({"role":if role=="system" {"developer"} else {role},"content":blocks(&item["content"])?})
            }
            Some("function_call") => {
                let id = item["call_id"]
                    .as_str()
                    .ok_or_else(|| invalid("Codex tool identity missing"))?;
                let name = item["name"]
                    .as_str()
                    .ok_or_else(|| invalid("Codex tool name missing"))?;
                let args = serde_json::from_str::<Value>(
                    item["arguments"]
                        .as_str()
                        .ok_or_else(|| invalid("Codex arguments missing"))?,
                )
                .map_err(|_| invalid("Codex tool arguments are not valid JSON; source retained"))?;
                json!({"role":"assistant","content":[{"type":"toolCall","id":id,"name":name,"namespace":item["namespace"],"arguments":args}]})
            }
            Some("custom_tool_call") => {
                let id = item["call_id"]
                    .as_str()
                    .ok_or_else(|| invalid("native custom tool identity missing"))?;
                let name = item["name"]
                    .as_str()
                    .ok_or_else(|| invalid("native custom tool name missing"))?;
                let input = item["input"]
                    .as_str()
                    .ok_or_else(|| invalid("native custom tool input missing"))?;
                json!({"role":"assistant","content":[{"type":"toolCall","id":id,"name":name,"nativeType":"custom_tool_call","arguments":{"input":input}}]})
            }
            Some("function_call_output" | "custom_tool_call_output") => {
                let id = item["call_id"]
                    .as_str()
                    .ok_or_else(|| invalid("Codex tool result identity missing"))?;
                json!({"role":"toolResult","toolCallId":id,"content":blocks(&item["output"])?})
            }
            Some("reasoning") if item["encrypted_content"].is_null() => {
                let mut content = Vec::new();
                for part in item["summary"].as_array().into_iter().flatten() {
                    let text = part["text"]
                        .as_str()
                        .ok_or_else(|| invalid("opaque Codex reasoning"))?;
                    content.push(json!({"type":"text","text":format!("[Recorded source reasoning]\n{text}\n[/Recorded source reasoning]")}));
                }
                json!({"role":"assistant","content":content})
            }
            _ => {
                return Err(invalid(
                    "Codex contains opaque reasoning or unsupported content; source retained",
                ))
            }
        };
        if message["role"] == "assistant" {
            message["provider"] = json!(provider);
            message["model"] = json!(model);
        }
        messages.push(message);
    }
    Ok(messages)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn native_events_checkpoint_and_structured_output_are_lossless() {
        let root = std::env::temp_dir().join(format!("native-codex-red-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("source.jsonl");
        let rows = [
            json!({"type":"session_meta","payload":{"id":"s","cwd":root,"cli_version":"0.164.0"}}),
            json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"turn"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"oldest"}]}}),
            json!({"type":"compacted","payload":{"replacement_history":[{"type":"message","role":"user","content":[{"type":"input_text","text":"summary"}]}]}}),
            json!({"type":"response_item","payload":{"type":"function_call","call_id":"t","namespace":"functions","name":"read","arguments":"{\"path\":\"kept\"}"}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"t","output":[{"type":"input_text","text":"tail"}]}}),
            json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"turn"}}),
        ];
        std::fs::write(
            &path,
            rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
        )
        .unwrap();
        let h = read_legacy_file(&path, "s").unwrap();
        let active = serde_json::to_string(&h.model_messages).unwrap();
        assert!(
            active.contains("summary") && active.contains("tail") && !active.contains("oldest"),
            "{active}"
        );
        assert_eq!(h.messages[1]["content"][0]["namespace"], "functions");
        assert_eq!(h.messages[2]["content"][0]["text"], "tail");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_custom_tools_preserve_the_exact_grammar_input() {
        let input = "*** Begin Patch\n*** Add File: file.txt\n+full content\n*** End Patch";
        let items = [
            json!({"type":"custom_tool_call","call_id":"patch","name":"apply_patch","input":input}),
            json!({"type":"custom_tool_call_output","call_id":"patch","output":"complete result"}),
        ];
        let messages = decode_items(&items, "source", "model").unwrap();
        assert_eq!(messages[0]["content"][0]["arguments"]["input"], input);
        assert_eq!(messages[1]["content"][0]["text"], "complete result");
        assert_eq!(messages[0]["content"][0]["nativeType"], "custom_tool_call");
    }

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
