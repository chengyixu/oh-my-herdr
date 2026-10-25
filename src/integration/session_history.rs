//! Native recorded-history exchange. No terminal excerpts and no model calls.
//! Unknown content is rejected before a source process can be changed.
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

mod storage;
pub(crate) use storage::ImportedHistory;

const MAX_HISTORY_BYTES: usize = 256 * 1024 * 1024;
static NEXT_IMPORT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct NativeHistory {
    pub(crate) session_id: String,
    pub(crate) messages: Vec<Value>,
    /// Distinct active model context; complete recorded history stays visible.
    model_messages: Option<Vec<Value>>,
    pub(crate) source_bytes: Vec<u8>,
    pub(crate) source_context: Option<Value>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Exact file and leaf identity only. Never select a source by latest cwd/file.
#[cfg(test)]
pub(crate) fn read_pi_history(path: &Path, leaf: Option<&str>) -> io::Result<NativeHistory> {
    let bytes = match crate::platform::read_limited_reader(
        std::fs::File::open(path)?,
        MAX_HISTORY_BYTES,
    )? {
        crate::platform::LimitedRead::Complete(bytes) => bytes,
        crate::platform::LimitedRead::Empty => return Err(invalid("native session is empty")),
        crate::platform::LimitedRead::Oversized => {
            return Err(invalid(
                "native history exceeds the import limit; nothing was truncated",
            ))
        }
    };
    decode_pi_history(bytes, leaf)
}

pub(crate) fn read_pi_snapshot(
    snapshot: &super::native_context::NativeContextSnapshot,
) -> io::Result<NativeHistory> {
    if snapshot.version != 1
        || snapshot.format != "pi.session.v3"
        || snapshot.header["id"] != snapshot.session_id
    {
        return Err(invalid("native context format or session identity changed"));
    }
    let mut bytes = Vec::new();
    for entry in std::iter::once(&snapshot.header).chain(snapshot.branch.iter()) {
        serde_json::to_writer(&mut bytes, entry)?;
        bytes.push(b'\n');
        if bytes.len() > MAX_HISTORY_BYTES {
            return Err(invalid(
                "native history exceeds the import limit; nothing was truncated",
            ));
        }
    }
    // Keep every recorded message on the exact active branch, including the
    // messages before compaction. Summaries add history; they never replace it.
    let mut projected = Vec::new();
    for entry in &snapshot.branch {
        match entry["type"].as_str() {
            Some("message")=>projected.push(entry["message"].clone()),
            Some("custom_message")=>projected.push(json!({"role":"custom","content":entry["content"],"customType":entry["customType"]})),
            Some("compaction")=>projected.push(json!({"role":"compactionSummary","summary":entry["summary"],"systemMessage":entry["systemMessage"],"firstKeptEntryId":entry["firstKeptEntryId"]})),
            Some("branch_summary")=>projected.push(json!({"role":"branchSummary","summary":entry["summary"],"fromId":entry["fromId"]})),
            Some("context_edit")=>return Err(invalid("this source contains explicit context edits; complete editable-history transfer has not been qualified. The original was not changed")),
            Some("model_change"|"thinking_level_change"|"session_info"|"label"|"custom"|"usage")=>{},
            _=>return Err(invalid("native context contains an unknown record; no history was discarded")),
        }
    }
    let mut history = normalize_pi_messages(&snapshot.header, &projected)?;
    let active = normalize_pi_messages(&snapshot.header, &snapshot.model_messages)?;
    if history.messages != active.messages {
        history.model_messages = Some(active.messages);
    }
    history.source_bytes = bytes;
    history.source_context = Some(serde_json::to_value(snapshot)?);
    Ok(history)
}

fn normalize_pi_messages(header: &Value, projected: &[Value]) -> io::Result<NativeHistory> {
    let mut projection_records = vec![header.clone()];
    let mut parent = Value::Null;
    for (index, message) in projected.iter().enumerate() {
        let mut message = message.clone();
        match message["role"].as_str() {
            Some("system") => {
                let body = serde_json::to_string(&message)?;
                message = json!({"role":"developer","content":[{"type":"text","text":format!("Recorded source-harness instructions and tool definitions (historical): {body}")} ]});
            }
            Some("custom" | "bashExecution" | "branchSummary" | "compactionSummary") => {
                let body = serde_json::to_string(&message)?;
                message = json!({"role":"user","content":[{"type":"text","text":format!("Recorded source-harness context: {body}")} ]});
            }
            Some("assistant") => {
                if let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) {
                    for block in blocks {
                        if block["type"] == "thinking" {
                            if block["redacted"] == true || !block["thinking"].is_string() {
                                return Err(invalid("the source has opaque reasoning that this integration cannot transfer; original session retained"));
                            }
                            let reasoning =
                                block["thinking"].as_str().unwrap_or_default().to_string();
                            *block = json!({"type":"text","text":format!("[Recorded source reasoning]\n{reasoning}\n[/Recorded source reasoning]")});
                        }
                    }
                }
            }
            _ => {}
        }
        let id = format!("projected-{index}");
        projection_records
            .push(json!({"type":"message","id":id,"parentId":parent,"message":message}));
        parent = json!(id);
    }
    let mut projected_bytes = Vec::new();
    for record in projection_records {
        serde_json::to_writer(&mut projected_bytes, &record)?;
        projected_bytes.push(b'\n');
    }
    decode_pi_history(projected_bytes, None)
}

fn decode_pi_history(bytes: Vec<u8>, leaf: Option<&str>) -> io::Result<NativeHistory> {
    if !bytes.ends_with(b"\n") {
        return Err(invalid(
            "native session has an unfinished record; wait for it to finish writing",
        ));
    }
    let records = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(serde_json::from_slice::<Value>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid("native history contains an invalid record"))?;
    let header = records
        .first()
        .ok_or_else(|| invalid("native session header is missing"))?;
    if header["type"] != "session" || header["version"] != 3 {
        return Err(invalid(
            "this native session format/version has not been qualified for complete import",
        ));
    }
    let session_id = header["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| invalid("native session identity is missing"))?
        .to_owned();
    let mut nodes = HashMap::new();
    let mut children = HashMap::<&str, usize>::new();
    for record in records.iter().skip(1) {
        let id = record["id"]
            .as_str()
            .ok_or_else(|| invalid("native history entry identity is missing"))?;
        if nodes.insert(id, record).is_some() {
            return Err(invalid(
                "native history contains duplicate entry identities",
            ));
        }
        if let Some(parent) = record["parentId"].as_str() {
            *children.entry(parent).or_default() += 1;
        }
    }
    if leaf.is_none() && children.values().any(|count| *count > 1) {
        return Err(invalid("native session has multiple branches; its exact active leaf must be exported by the running integration"));
    }
    let mut cursor = leaf.or_else(|| {
        records
            .get(1..)
            .and_then(|entries| entries.last())
            .and_then(|record| record["id"].as_str())
    });
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    while let Some(id) = cursor {
        if !seen.insert(id) {
            return Err(invalid("native session branch contains a cycle"));
        }
        let entry = nodes
            .get(id)
            .ok_or_else(|| invalid("native session branch references a missing entry"))?;
        selected.push(*entry);
        cursor = entry["parentId"].as_str();
    }
    selected.reverse();
    let mut messages = Vec::new();
    for entry in selected {
        match entry["type"].as_str() {
            Some("message") => {
                let message = entry.get("message").filter(|message| message.is_object())
                    .ok_or_else(|| invalid("native message body is missing"))?;
                validate_pi_message(message)?;
                messages.push(message.clone());
            }
            Some("model_change" | "thinking_level_change" | "session_info" | "label" | "custom" | "usage") => {}
            // These affect the current model context; do not silently erase them.
            Some("compaction" | "branch_summary" | "custom_message") => return Err(invalid("this session includes context records which require an integration snapshot; the original was not changed")),
            _ => return Err(invalid("native session includes an unsupported context entry")),
        }
    }
    Ok(NativeHistory {
        session_id,
        messages,
        model_messages: None,
        source_bytes: bytes,
        source_context: None,
    })
}

fn message_blocks(message: &Value) -> io::Result<Vec<Value>> {
    match message.get("content") {
        Some(Value::String(text)) => Ok(vec![json!({"type":"text","text":text})]),
        Some(Value::Array(blocks)) => Ok(blocks.clone()),
        _ => Err(invalid("native message content is missing or unsupported")),
    }
}

fn validate_pi_message(message: &Value) -> io::Result<()> {
    if !matches!(
        message["role"].as_str(),
        Some("user" | "assistant" | "toolResult" | "developer")
    ) {
        return Err(invalid(
            "native message role is unsupported for complete import",
        ));
    }
    for block in message_blocks(message)? {
        match block["type"].as_str() {
            Some("text") if block["text"].is_string() => {}
            Some("image") if block["data"].is_string() && block["mimeType"].is_string() => {}
            Some("toolCall") if block["id"].is_string() && block["name"].is_string() && block.get("arguments").is_some() => {}
            Some("thinking") => return Err(invalid("native reasoning/signature import has not been qualified for this pair; the original was not changed")),
            _ => return Err(invalid("native message contains unsupported content; no records were discarded")),
        }
    }
    if message["role"] == "toolResult" && !message["toolCallId"].is_string() {
        return Err(invalid("native tool result identity is missing"));
    }
    Ok(())
}

fn image_uri(block: &Value) -> io::Result<String> {
    let mime = block["mimeType"]
        .as_str()
        .filter(|mime| mime.starts_with("image/"))
        .ok_or_else(|| invalid("native attachment is not a supported image"))?;
    let data = block["data"]
        .as_str()
        .ok_or_else(|| invalid("native image data is missing"))?;
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| invalid("native image data is invalid"))?;
    let format = match mime {
        "image/png" => image::ImageFormat::Png,
        "image/jpeg" => image::ImageFormat::Jpeg,
        "image/webp" => image::ImageFormat::WebP,
        "image/gif" => image::ImageFormat::Gif,
        _ => {
            return Err(invalid(
                "native image format has not been qualified; original session retained",
            ))
        }
    };
    // Base64 validity alone is insufficient: native Codex silently replaces
    // undecodable images with text. Decode privately before any source close,
    // with bounded allocations, and retain the original bytes without reencoding.
    let mut reader = image::ImageReader::with_format(io::Cursor::new(&decoded), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(MAX_HISTORY_BYTES as u64);
    reader.limits(limits);
    reader.decode().map_err(|_| {
        invalid("native image is invalid or exceeds the decoding limit; original session retained")
    })?;
    Ok(format!("data:{mime};base64,{data}"))
}

fn codex_records(
    history: &NativeHistory,
    session_id: &str,
    cwd: &Path,
    stamp: &str,
    target_provider: &str,
) -> io::Result<Vec<Value>> {
    let mut records = codex_context_records(
        &history.messages,
        history.source_context.as_ref(),
        session_id,
        cwd,
        stamp,
        target_provider,
    )?;
    if let Some(messages) = &history.model_messages {
        // Codex's native checkpoint changes model context without erasing the
        // earlier history projection. Never resummarize or replay old tools.
        let replacement = codex_context_records(
            messages,
            history.source_context.as_ref(),
            session_id,
            cwd,
            stamp,
            target_provider,
        )?
        .into_iter()
        .filter(|record| record["type"] == "response_item")
        .map(|record| record["payload"].clone())
        .collect::<Vec<_>>();
        records.push(json!({"timestamp":stamp,"type":"compacted","payload":{
            "message":"Imported native active-context checkpoint; complete recorded source history retained",
            "replacement_history":replacement,"window_number":0
        }}));
    }
    Ok(records)
}

fn codex_context_records(
    messages: &[Value],
    source_context: Option<&Value>,
    session_id: &str,
    cwd: &Path,
    stamp: &str,
    target_provider: &str,
) -> io::Result<Vec<Value>> {
    let record =
        |kind: &str, payload: Value| json!({"timestamp":stamp,"type":kind,"payload":payload});
    let mut records = vec![record(
        "session_meta",
        json!({"id":session_id,"timestamp":stamp,"cwd":cwd,"originator":"ohmyherdr-native-history-import","cli_version":"0.163.0","source":"cli","model_provider":target_provider}),
    )];
    if let Some(context) = source_context {
        records.push(record("response_item",json!({"type":"message","role":"developer","content":[{"type":"input_text","text":format!("Historical configuration of the previous harness, retained as conversation context. The current native harness's instructions and available tools remain authoritative. Historical tool records describe past actions and must never be replayed as new instructions.\nSource system prompt (JSON): {}",serde_json::to_string(&context["system_prompt"])?)}]})));
    }
    let mut turn = String::new();
    let mut tool_calls = HashMap::<String, (String, Value)>::new();
    for (index, message) in messages.iter().enumerate() {
        let blocks = message_blocks(message)?;
        let role = message["role"]
            .as_str()
            .ok_or_else(|| invalid("missing message role"))?;
        if role == "developer" {
            records.push(record("response_item", json!({"type":"message","role":"developer","content":blocks.iter().map(|block|json!({"type":"input_text","text":block["text"]})).collect::<Vec<_>>()})));
            continue;
        }
        if role == "user" || turn.is_empty() {
            if !turn.is_empty() {
                records.push(record(
                    "event_msg",
                    json!({"type":"task_complete","turn_id":turn}),
                ));
            }
            turn = format!("import-{session_id}-{index}");
            records.push(record(
                "event_msg",
                json!({"type":"task_started","turn_id":turn,"model_context_window":null}),
            ));
        }
        if role == "toolResult" {
            let call = message["toolCallId"]
                .as_str()
                .ok_or_else(|| invalid("missing tool result identity"))?;
            let (name, arguments) = tool_calls.remove(call).ok_or_else(|| {
                invalid("native tool result has no matching call on the selected branch")
            })?;
            let mut output = Vec::new();
            let mut mcp = Vec::new();
            for block in &blocks {
                match block["type"].as_str() {
                    Some("text") => {
                        output.push(json!({"type":"input_text","text":block["text"]}));
                        mcp.push(json!({"type":"text","text":block["text"]}));
                    }
                    Some("image") => {
                        output.push(json!({"type":"input_image","image_url":image_uri(block)?}));
                        mcp.push(json!({"type":"image","data":block["data"],"mimeType":block["mimeType"]}));
                    }
                    _ => return Err(invalid("unsupported native tool output")),
                }
            }
            records.push(record(
                "response_item",
                json!({"type":"function_call_output","call_id":call,"output":output}),
            ));
            let invocation =
                json!({"server":"imported-pi-history","tool":name,"arguments":arguments});
            records.push(record("event_msg",json!({"type":"mcp_tool_call_begin","call_id":call,"turn_id":turn,"invocation":invocation})));
            records.push(record("event_msg",json!({"type":"mcp_tool_call_end","call_id":call,"turn_id":turn,"invocation":invocation,"duration":{"secs":0,"nanos":0},"result":{"Ok":{"content":mcp,"isError":message["isError"].as_bool().unwrap_or(false)}}})));
            continue;
        }
        let mut content = Vec::new();
        let mut text = Vec::new();
        let mut images = Vec::new();
        for block in blocks {
            match block["type"].as_str() {
                Some("text") => {
                    content.push(json!({"type":if role=="user" {"input_text"} else {"output_text"},"text":block["text"]}));
                    text.push(
                        block["text"]
                            .as_str()
                            .ok_or_else(|| invalid("invalid native text"))?
                            .to_string(),
                    );
                }
                Some("image") if role == "user" => {
                    let uri = image_uri(&block)?;
                    images.push(uri.clone());
                    content.push(json!({"type":"input_image","image_url":uri}));
                }
                Some("toolCall") if role == "assistant" => {
                    append_message(
                        &mut records,
                        stamp,
                        role,
                        &mut content,
                        &mut text,
                        &mut images,
                    );
                    let id = block["id"]
                        .as_str()
                        .ok_or_else(|| invalid("invalid tool identity"))?
                        .to_string();
                    let name = block["name"]
                        .as_str()
                        .ok_or_else(|| invalid("invalid tool name"))?
                        .to_string();
                    let arguments = block["arguments"].clone();
                    if tool_calls
                        .insert(id.clone(), (name.clone(), arguments.clone()))
                        .is_some()
                    {
                        return Err(invalid("duplicate native tool identity"));
                    }
                    records.push(record("response_item",json!({"type":"function_call","name":name,"arguments":serde_json::to_string(&arguments)?,"call_id":id})));
                }
                _ => {
                    return Err(invalid(
                        "native content cannot be faithfully represented by the target",
                    ))
                }
            }
        }
        append_message(
            &mut records,
            stamp,
            role,
            &mut content,
            &mut text,
            &mut images,
        );
    }
    if !tool_calls.is_empty() {
        return Err(invalid(
            "native session contains unfinished tool calls; wait for completion",
        ));
    }
    if !turn.is_empty() {
        records.push(record(
            "event_msg",
            json!({"type":"task_complete","turn_id":turn}),
        ));
    }
    Ok(records)
}

fn append_message(
    records: &mut Vec<Value>,
    stamp: &str,
    role: &str,
    content: &mut Vec<Value>,
    text: &mut Vec<String>,
    images: &mut Vec<String>,
) {
    if content.is_empty() {
        return;
    }
    records.push(json!({"timestamp":stamp,"type":"response_item","payload":{"type":"message","role":role,"content":std::mem::take(content)}}));
    let body = if role == "user" {
        json!({"type":"user_message","message":text.join("\n"),"images":std::mem::take(images),"local_images":[],"text_elements":[]})
    } else {
        json!({"type":"agent_message","message":text.join("\n")})
    };
    text.clear();
    records.push(json!({"timestamp":stamp,"type":"event_msg","payload":body}));
}

pub(crate) fn verify_codex_import_version(executable: &str) -> io::Result<()> {
    use std::process::Stdio;
    let mut command = crate::noninteractive_process::command(executable);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait()? {
            Some(status) if status.success() => break,
            Some(_) => {
                return Err(invalid(
                    "target native version discovery failed; source unchanged",
                ))
            }
            None if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(invalid(
                    "target native version discovery timed out; source unchanged",
                ));
            }
        }
    }
    let Some(output) = child.stdout.take() else {
        return Err(invalid("target native version discovery has no output"));
    };
    let bytes = match crate::platform::read_limited_reader(output, 4096)? {
        crate::platform::LimitedRead::Complete(bytes) => bytes,
        _ => return Err(invalid("target native version output is invalid")),
    };
    let version =
        std::str::from_utf8(&bytes).map_err(|_| invalid("target native version is invalid"))?;
    if version.trim() != "codex-cli 0.163.0" {
        return Err(invalid("complete native history import is qualified for Codex 0.163.0; this installed version requires qualification before switching"));
    }
    Ok(())
}

pub(crate) fn effective_codex_provider(executable: &str, cwd: &Path) -> io::Result<String> {
    use std::io::{BufRead, BufReader, Read};
    use std::process::Stdio;
    let mut command = crate::noninteractive_process::command(executable);
    command
        .args(["app-server"])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| invalid("native config reader has no input"))?;
    let output = child
        .stdout
        .take()
        .ok_or_else(|| invalid("native config reader has no output"))?;
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::Builder::new()
        .name("native-effective-config".into())
        .spawn(move || {
            for line in BufReader::new(output.take(4 * 1024 * 1024)).lines() {
                let Ok(line) = line else { break };
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    if value["id"] == 1 || value["id"] == 2 {
                        let last = value["id"] == 2;
                        if sender.send(value).is_err() || last {
                            break;
                        }
                    }
                }
            }
        });
    let reader = match reader {
        Ok(reader) => reader,
        Err(error) => {
            drop(input);
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let result = (|| -> io::Result<String> {
        for request in [
            json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"ohmyherdr-native-config","version":"1"}}}),
            json!({"method":"initialized"}),
            json!({"id":2,"method":"config/read","params":{"includeLayers":false,"cwd":cwd}}),
        ] {
            serde_json::to_writer(&mut input, &request)?;
            input.write_all(b"\n")?;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
        loop {
            let response = receiver
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .map_err(|_| {
                    invalid("native effective configuration discovery timed out; source unchanged")
                })?;
            if response.get("error").is_some() {
                return Err(invalid(
                    "native effective configuration was rejected; source unchanged",
                ));
            }
            if response["id"] != 2 {
                continue;
            }
            let config = response
                .pointer("/result/config")
                .ok_or_else(|| invalid("native effective configuration response is missing"))?;
            let value = config
                .get("model_provider")
                .ok_or_else(|| invalid("native effective provider selection is missing"))?;
            return match value {
                // Codex's native config contract uses null for its builtin default.
                Value::Null => Ok("openai".into()),
                Value::String(value) if !value.is_empty() => Ok(value.clone()),
                _ => Err(invalid("native effective provider selection is invalid")),
            };
        }
    })();
    // EOF asks this task-owned app server to shut down without affecting the
    // shared daemon, any user TUI, account configuration or loaded thread.
    drop(input);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            crate::platform::signal_processes(&[child.id()], crate::platform::Signal::Terminate);
            let _ = child.wait();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    reader
        .join()
        .map_err(|_| invalid("native configuration reader stopped"))?;
    result
}

/// Write a new target session; never overwrite a source or an existing session.
/// The complete original is archived for recovery and independent fidelity checks.
pub(crate) fn import_codex_history(
    history: &NativeHistory,
    cwd: &Path,
    codex_home: &Path,
    target_provider: &str,
) -> io::Result<ImportedHistory> {
    let identity = storage::Identity::new(history)?;
    let records = codex_records(history, &identity.id, cwd, &identity.stamp, target_provider)?;
    let now = identity.created;
    let path = codex_home
        .join("sessions")
        .join(format!(
            "{:04}/{:02}/{:02}",
            now.year(),
            u8::from(now.month()),
            now.day()
        ))
        .join(format!(
            "rollout-{:04}-{:02}-{:02}T{:02}-{:02}-{:02}-{}.jsonl",
            now.year(),
            u8::from(now.month()),
            now.day(),
            now.hour(),
            now.minute(),
            now.second(),
            identity.id
        ));
    let native_session = crate::agent_resume::PersistedAgentSession {
        source: "herdr:codex".into(),
        agent: "codex".into(),
        session_ref: crate::agent_resume::AgentSessionRef::id(identity.id.clone())
            .ok_or_else(|| invalid("invalid imported native identity"))?,
    };
    storage::publish(history, records, path, native_session, &identity.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    const IMAGE_DATA: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAIAAAAmkwkpAAAAE0lEQVR4nGPUCqljgAEmOAsvBwAx6AEEzQ2cZwAAAABJRU5ErkJggg==";
    fn source() -> Vec<Value> {
        let mut records = vec![json!({"type":"session","version":3,"id":"source-id","cwd":"/tmp"})];
        let mut parent = Value::Null;
        for index in 0..300 {
            for role in ["user", "assistant"] {
                let id = format!("{role}-{index}");
                records.push(json!({"type":"message","id":id,"parentId":parent,"message":{"role":role,"content":[{"type":"text","text":format!("{role}-{index}-{}","x".repeat(200))}]}}));
                parent = json!(id);
            }
        }
        let mut images = vec![json!({"type":"image","mimeType":"image/png","data":IMAGE_DATA})];
        for (format, mime) in [
            (image::ImageFormat::Jpeg, "image/jpeg"),
            (image::ImageFormat::WebP, "image/webp"),
            (image::ImageFormat::Gif, "image/gif"),
        ] {
            use base64::Engine;
            let mut encoded = io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgb8(4, 4)
                .write_to(&mut encoded, format)
                .unwrap();
            images.push(json!({"type":"image","mimeType":mime,"data":base64::engine::general_purpose::STANDARD.encode(encoded.into_inner())}));
        }
        records[1]["message"]["content"]
            .as_array_mut()
            .unwrap()
            .extend(images.clone());
        records.push(json!({"type":"message","id":"call","parentId":parent,"message":{"role":"assistant","content":[{"type":"text","text":"before-tool-call"},{"type":"toolCall","id":"call-1","name":"read","arguments":{"path":"kept-arguments"}}]}}));
        records.push(json!({"type":"message","id":"result","parentId":"call","message":{"role":"toolResult","toolCallId":"call-1","isError":false,"content":[{"type":"text","text":format!("kept-output-{}","z".repeat(30000))}]}}));
        records.last_mut().unwrap()["message"]["content"]
            .as_array_mut()
            .unwrap()
            .extend(images);
        records
    }
    fn bytes(records: &[Value]) -> Vec<u8> {
        records
            .iter()
            .map(|v| serde_json::to_string(v).unwrap() + "\n")
            .collect::<String>()
            .into_bytes()
    }
    #[cfg(unix)]
    #[test]
    fn an_unqualified_target_version_is_refused_before_import() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::path::PathBuf::from(format!("/tmp/ohversion-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("native-version");
        std::fs::write(&path, "#!/bin/sh\nprintf 'codex-cli 0.164.0\\n'\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = verify_codex_import_version(path.to_str().unwrap());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("requires qualification before switching"));
        assert!(!root.join("sessions").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_native_images_are_rejected_before_the_import_is_published() {
        let block = json!({"type":"image","mimeType":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aBnEAAAAASUVORK5CYII="});
        assert!(
            image_uri(&block).is_err(),
            "a bad PNG checksum must not become an omitted native model image"
        );
        assert!(image_uri(
            &json!({"type":"image","mimeType":"image/unqualified","data":IMAGE_DATA})
        )
        .is_err());
    }

    #[test]
    fn an_instruction_only_native_session_has_no_fabricated_turn() {
        let records = source();
        let history = decode_pi_history(bytes(&records[..1]), None).unwrap();
        assert!(history.messages.is_empty());
        let target = codex_records(
            &history,
            "00000000-0000-4000-a000-000000000000",
            Path::new("/tmp"),
            "2026-10-18T00:00:00Z",
            "openai",
        )
        .unwrap();
        assert_eq!(target.len(), 1);
        assert_eq!(target[0]["type"], "session_meta");
    }

    #[test]
    fn complete_history_preserves_oldest_newest_and_full_tool_output() {
        let input = bytes(&source());
        let history = decode_pi_history(input.clone(), None).unwrap();
        assert_eq!(history.messages.len(), 602);
        assert_eq!(history.source_bytes, input);
        let records = codex_records(
            &history,
            "00000000-0000-4000-a000-000000000000",
            Path::new("/tmp"),
            "2026-10-18T00:00:00Z",
            "target-native-provider",
        )
        .unwrap();
        let serialized = serde_json::to_string(&records).unwrap();
        let text_position = records
            .iter()
            .position(|r| {
                r["type"] == "response_item"
                    && r["payload"]["content"]
                        .to_string()
                        .contains("before-tool-call")
            })
            .unwrap();
        let call_position = records
            .iter()
            .position(|r| r["payload"]["type"] == "function_call")
            .unwrap();
        assert!(
            text_position < call_position,
            "text and tool calls must keep the original order"
        );
        for text in [
            "user-0-",
            "assistant-299-",
            "kept-arguments",
            &format!("kept-output-{}", "z".repeat(30000)),
        ] {
            assert!(serialized.contains(text), "history omitted {text:.30}");
        }
    }
    #[cfg(unix)]
    #[test]
    #[ignore = "requires installed Codex; qualifies effective project config without a model call"]
    fn native_config_discovery_respects_native_provider_authority() {
        let _lock = crate::integration::integration_env_lock();
        let root =
            std::path::PathBuf::from(format!("/var/tmp/ohnativeconfig-{}", std::process::id()));
        let home = root.join("native-home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(root.join(".codex")).unwrap();
        assert!(std::process::Command::new("git")
            .args(["-c", "init.templateDir=", "init", "--quiet"])
            .arg(&root)
            .status()
            .unwrap()
            .success());
        std::fs::write(home.join("config.toml"),format!("model_provider = 'home-native'\nmodel = 'native-config-test'\n[model_providers.home-native]\nname = 'Home'\nbase_url = 'http://127.0.0.1:9/v1'\nwire_api = 'responses'\n[model_providers.project-native]\nname = 'Project'\nbase_url = 'http://127.0.0.1:9/v1'\nwire_api = 'responses'\n[projects.{}]\ntrust_level = 'trusted'\n",serde_json::to_string(root.canonicalize().unwrap().to_str().unwrap()).unwrap())).unwrap();
        std::fs::write(
            root.join(".codex/config.toml"),
            "model_provider = 'project-native'\n",
        )
        .unwrap();
        let previous = std::env::var_os("CODEX_HOME");
        std::env::set_var("CODEX_HOME", &home);
        let result = effective_codex_provider("codex", &root);
        match previous {
            Some(value) => std::env::set_var("CODEX_HOME", value),
            None => std::env::remove_var("CODEX_HOME"),
        };
        // Codex deliberately denies model_provider in project layers. The
        // adapter must follow the native resolved authority, not merge TOML itself.
        assert_eq!(result.unwrap(), "home-native");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    #[ignore = "requires the installed native Codex CLI; no model calls or credentials"]
    fn native_codex_hydrates_complete_imported_history() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        let root = std::path::PathBuf::from(format!("/var/tmp/ohhistory-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("source.jsonl");
        std::fs::write(&path, bytes(&source())).unwrap();
        let history = read_pi_history(&path, None).unwrap();
        let home = root.join("codex");
        let imported = import_codex_history(&history, &root, &home, "openai").unwrap();
        assert_eq!(imported.messages, 602);
        assert!(imported.path.exists());
        assert_eq!(
            std::fs::read(&imported.source_archive).unwrap(),
            history.source_bytes
        );
        let mut process = Command::new("codex")
            .args(["app-server"])
            .env("CODEX_HOME", &home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut input = process.stdin.take().unwrap();
        let output = process.stdout.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    if sender.send(value).is_err() {
                        break;
                    }
                }
            }
        });
        writeln!(input,"{}",json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"ohmyherdr-history-qualification","version":"1"},"capabilities":{"experimentalApi":true}}})).unwrap();
        let init = receiver
            .recv_timeout(std::time::Duration::from_secs(15))
            .unwrap();
        assert!(init.get("error").is_none(), "{init}");
        writeln!(input, "{}", json!({"method":"initialized"})).unwrap();
        writeln!(input,"{}",json!({"id":2,"method":"thread/read","params":{"threadId":imported.native_session.session_ref.value,"includeTurns":true}})).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
        let response = loop {
            let value = receiver
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()));
            match value {
                Ok(value) if value["id"] == 2 => break Ok(value),
                Ok(_) => continue,
                Err(error) => break Err(error),
            }
        };
        let _ = process.kill();
        let _ = process.wait();
        drop(input);
        reader.join().unwrap();
        let response = response.unwrap();
        assert!(response.get("error").is_none(), "{response}");
        let hydrated = serde_json::to_string(&response).unwrap();
        for text in [
            "user-0-",
            "assistant-299-",
            "kept-arguments",
            &format!("kept-output-{}", "z".repeat(30000)),
        ] {
            assert!(
                hydrated.contains(text),
                "native importer omitted {text:.30}; evidence {}",
                root.display()
            );
        }
        assert!(
            hydrated.contains(IMAGE_DATA),
            "native import lost image bytes"
        );
        let turns = response
            .pointer("/result/thread/turns")
            .unwrap()
            .as_array()
            .unwrap();
        let first_user = &turns[0]["items"][0];
        assert!(
            first_user["content"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["type"] == "image"),
            "native importer lost the user attachment: {first_user}"
        );
        let last_tool = turns.last().unwrap()["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["type"] == "mcpToolCall")
            .unwrap();
        assert!(
            last_tool["result"]["content"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["type"] == "image"),
            "native importer lost the tool-result attachment: {last_tool}"
        );
        assert_eq!(
            response
                .pointer("/result/thread/turns")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            300
        );
        let _ = std::fs::remove_dir_all(root);
    }
    fn compacted_snapshot() -> super::super::native_context::NativeContextSnapshot {
        let mut branch = source();
        let current_tool_result = branch.last().unwrap()["message"].clone();
        let current_tool_call = branch[branch.len() - 2]["message"].clone();
        branch.push(json!({"type":"compaction","id":"compact","parentId":"result","summary":"summary is additional context","firstKeptEntryId":"result"}));
        let mut summary = branch[1]["message"].clone();
        summary["content"][0]["text"] = json!("summary only");
        let model_messages = vec![summary, current_tool_call, current_tool_result];
        super::super::native_context::NativeContextSnapshot {
            version: 1,
            format: "pi.session.v3".into(),
            session_id: "source-id".into(),
            session_path: "/tmp/source.jsonl".into(),
            leaf_id: Some("compact".into()),
            cwd: "/tmp".into(),
            header: branch.remove(0),
            branch,
            projection: json!({"messages":model_messages}),
            model_messages,
            system_prompt: "source instructions".into(),
        }
    }

    #[test]
    #[ignore = "requires native Codex; captures a local fixed-response request, no remote model or credentials"]
    fn native_codex_preserves_active_checkpoint_and_complete_recorded_history() {
        let root =
            std::path::PathBuf::from(format!("/var/tmp/ohcheckpoint-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let home = root.join("codex");
        let history = read_pi_snapshot(&compacted_snapshot()).unwrap();
        let imported = import_codex_history(&history, &root, &home, "capture").unwrap();
        let output = std::process::Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/scripts/verify_native_context_checkpoint.py"
            ))
            .arg("--home")
            .arg(&home)
            .arg("--session")
            .arg(&imported.native_session.session_ref.value)
            .arg("--cwd")
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "native checkpoint verification failed; retained evidence at {}\n{}\n{}",
            root.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        drop(imported);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_snapshot_keeps_precompaction_history_and_rejects_edited_or_opaque_context() {
        let mut snapshot = compacted_snapshot();
        let history = read_pi_snapshot(&snapshot).unwrap();
        assert!(serde_json::to_string(&history.messages)
            .unwrap()
            .contains("user-0-"));
        assert!(serde_json::to_string(&history.messages)
            .unwrap()
            .contains("summary is additional context"));
        let records = codex_records(
            &history,
            "target",
            Path::new("/tmp"),
            "2026-10-18T00:00:00Z",
            "openai",
        )
        .unwrap();
        let checkpoint = records.iter().find(|record| record["type"] == "compacted")
            .expect("active native model context must be checkpointed independently of complete visible history");
        let active = serde_json::to_string(&checkpoint["payload"]["replacement_history"]).unwrap();
        assert!(active.contains("summary only"));
        assert!(
            !active.contains("user-0-"),
            "compacted source records were resurrected in model context"
        );
        snapshot.branch.push(json!({"type":"context_edit","id":"edit","parentId":"compact","targetId":"user-0","replacement":null}));
        assert!(read_pi_snapshot(&snapshot).is_err());
        snapshot.branch.pop();
        snapshot.branch[1]["message"]["content"] =
            json!([{"type":"thinking","redacted":true,"thinkingSignature":"opaque"}]);
        assert!(read_pi_snapshot(&snapshot).is_err());
    }

    #[test]
    fn incomplete_unknown_and_ambiguous_context_fails_without_truncation() {
        let mut input = bytes(&source());
        input.pop();
        assert!(decode_pi_history(input, None).is_err());
        let mut records = source();
        records.last_mut().unwrap()["message"]["content"] =
            json!([{"type":"unknown-attachment","value":"do not lose"}]);
        assert!(decode_pi_history(bytes(&records), None).is_err());
        let mut records = source();
        records.push(json!({"type":"message","id":"other-branch","parentId":"user-0","message":{"role":"assistant","content":"unrelated branch"}}));
        assert!(decode_pi_history(bytes(&records), None).is_err());
        let branch = decode_pi_history(bytes(&records), Some("result")).unwrap();
        assert!(!serde_json::to_string(&branch.messages)
            .unwrap()
            .contains("unrelated branch"));
    }
}
