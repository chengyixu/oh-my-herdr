use super::{invalid, NativeHistory};
use serde_json::{json, Value};
use std::io;
use std::path::Path;

pub(crate) fn read_legacy_file(path: &Path, expected_id: &str) -> io::Result<NativeHistory> {
    let bytes = super::read_source_bytes(path)?;
    let records = super::source_records(&bytes)?;
    let native = if records
        .first()
        .is_some_and(|r| r["payload"]["history_mode"] == "paginated")
    {
        let cwd = records[0]["payload"]["cwd"]
            .as_str()
            .ok_or_else(|| invalid("native cwd missing"))?;
        Some(super::codex_native::history(
            Path::new(cwd),
            expected_id,
            path,
        )?)
    } else {
        None
    };
    decode_with_native(bytes, expected_id, native.as_ref())
}

pub(crate) fn decode(bytes: Vec<u8>, expected_id: &str) -> io::Result<NativeHistory> {
    decode_with_native(bytes, expected_id, None)
}

pub(crate) fn decode_with_native(
    bytes: Vec<u8>,
    expected_id: &str,
    native: Option<&Value>,
) -> io::Result<NativeHistory> {
    let records = super::source_records(&bytes)?;
    let mut provider = String::new();
    let mut model = String::new();
    let mut messages = Vec::new();
    let mut active = None::<Vec<Value>>;
    let mut verified = false;
    let mut world_state = Value::Null;
    let mut base_instructions = Value::Null;
    let mut paginated = false;
    for row in records {
        match row["type"].as_str() {
            Some("session_meta") if !verified && row["payload"]["id"] == expected_id => {
                paginated = row["payload"]["history_mode"] == "paginated";
                if paginated && native.is_none() {
                    return Err(invalid(
                        "paginated source requires complete native history export",
                    ));
                }
                base_instructions = row["payload"]["base_instructions"].clone();
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
    let mut context = json!({"system_prompt":json!({"base_instructions":base_instructions,"world_state":world_state}).to_string()});
    if paginated {
        let native = native.ok_or_else(|| invalid("native history export missing"))?;
        if native["complete"] != true || native["thread"]["id"] != expected_id {
            return Err(invalid("native history export incomplete or changed"));
        }
        active = Some(active.unwrap_or_else(|| messages.clone()));
        messages = recorded_native_messages(
            native["items"]
                .as_array()
                .ok_or_else(|| invalid("native history items missing"))?,
        )?;
        context["native_export"] = native.clone();
    }
    Ok(NativeHistory {
        session_id: expected_id.into(),
        messages,
        model_messages: active,
        source_bytes: bytes,
        source_context: (paginated || !world_state.is_null() || !base_instructions.is_null())
            .then_some(context),
    })
}

fn recorded_native_messages(items: &[Value]) -> io::Result<Vec<Value>> {
    let mut messages = Vec::new();
    for entry in items {
        if entry["turnId"].as_str().is_none_or(str::is_empty)
            || entry["item"]["id"].as_str().is_none_or(str::is_empty)
        {
            return Err(invalid("native history item identity missing"));
        }
        let item = &entry["item"];
        match item["type"].as_str() {
            Some("userMessage") => {
                let mut content = Vec::new();
                for block in item["content"]
                    .as_array()
                    .ok_or_else(|| invalid("native user content missing"))?
                {
                    match block["type"].as_str() {
                        Some("text") if block["text"].is_string() => {
                            content.push(json!({"type":"text","text":block["text"]}))
                        }
                        Some("image") => content.extend(blocks(
                            &json!([{"type":"input_image","image_url":block["url"]}]),
                        )?),
                        _ => {
                            return Err(invalid(
                                "native paginated attachment cannot be transferred completely",
                            ))
                        }
                    }
                }
                messages.push(json!({"role":"user","content":content}));
            }
            Some("agentMessage") if item["text"].is_string() => messages
                .push(json!({"role":"assistant","content":[{"type":"text","text":item["text"]}]})),
            // Preserve the complete native UI record, including all arguments,
            // diffs and results, independently of the active model projection.
            Some(
                "commandExecution" | "fileChange" | "mcpToolCall" | "dynamicToolCall"
                | "functionCallOutput" | "plan" | "reasoning" | "contextCompaction",
            ) => {
                if matches!(item["status"].as_str(), Some("inProgress" | "pending")) {
                    return Err(invalid("native history contains unfinished work"));
                }
                messages.push(json!({"role":"user","content":[{"type":"text","text":format!("[Recorded native Codex event]\n{}\n[/Recorded native Codex event]",serde_json::to_string(item)?)}]}));
            }
            _ => {
                return Err(invalid(
                    "unqualified native paginated history item; source retained",
                ))
            }
        }
    }
    Ok(messages)
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
    fn paginated_history_keeps_older_native_pages_out_of_the_active_window() {
        let rows = [
            json!({"type":"session_meta","payload":{"id":"source","history_mode":"paginated"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"current-window"}]}}),
        ];
        let bytes = rows
            .iter()
            .map(|r| format!("{r}\n"))
            .collect::<String>()
            .into_bytes();
        assert!(decode(bytes.clone(), "source").is_err());
        let export = json!({"complete":true,"thread":{"id":"source"},"items":[
            {"turnId":"older","item":{"id":"u1","type":"userMessage","content":[{"type":"text","text":"oldest-native-page"}]}},
            {"turnId":"current","item":{"id":"u2","type":"userMessage","content":[{"type":"text","text":"current-window"}]}}
        ]});
        let history = decode_with_native(bytes.clone(), "source", Some(&export)).unwrap();
        assert!(serde_json::to_string(&history.messages)
            .unwrap()
            .contains("oldest-native-page"));
        let active = serde_json::to_string(&history.model_messages).unwrap();
        assert!(active.contains("current-window") && !active.contains("oldest-native-page"));
        let mut incomplete = export;
        incomplete["complete"] = json!(false);
        assert!(decode_with_native(bytes, "source", Some(&incomplete)).is_err());
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
