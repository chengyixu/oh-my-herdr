//! Read-only, bounded Codex persisted-history/queue API. A paginated thread is
//! complete only after following the native continuation cursor to exhaustion.
use super::super::native_acp::Discovery;
use super::invalid;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io;
use std::path::Path;

fn connect(cwd: &Path) -> io::Result<Discovery> {
    connect_bound(cwd, None)
}
fn connect_bound(cwd: &Path, binding: Option<&(String, String)>) -> io::Result<Discovery> {
    let mut api = Discovery::start_bound(
        "codex",
        &["--no-daemon", "app-server"],
        cwd,
        256 * 1024 * 1024,
        binding,
    )?;
    api.call("initialize",json!({"clientInfo":{"name":"ohmyherdr-native-history","version":"1"},"capabilities":{"experimentalApi":true}}))?;
    api.notify("initialized", json!({}))?;
    Ok(api)
}

pub(crate) fn preflight_target(cwd: &Path, binding: Option<&(String, String)>) -> io::Result<()> {
    let failure = || {
        invalid("Native Codex account/bootstrap is unavailable. Open Codex normally and resolve its authentication before switching; the source was retained")
    };
    let mut api = connect_bound(cwd, binding).map_err(|_| failure())?;
    let account = api
        .call("account/read", json!({"refreshToken":false}))
        .map_err(|_| failure())?;
    if account_ready(&account) {
        Ok(())
    } else {
        Err(failure())
    }
}

fn account_ready(account: &Value) -> bool {
    match account["requiresOpenaiAuth"].as_bool() {
        Some(false) => true,
        Some(true) => matches!(
            account["account"]["type"].as_str(),
            Some("apiKey" | "chatgpt" | "amazonBedrock")
        ),
        _ => false,
    }
}

pub(crate) fn queue_empty(cwd: &Path, id: &str) -> io::Result<bool> {
    let mut api = connect(cwd)?;
    let queue = api.call("thread/queue/list", json!({"threadId":id,"limit":1}))?;
    let items = queue["data"]
        .as_array()
        .ok_or_else(|| invalid("native queue response missing"))?;
    Ok(items.is_empty() && queue.get("nextCursor").is_some_and(Value::is_null))
}

pub(crate) fn history(cwd: &Path, id: &str, path: &Path) -> io::Result<Value> {
    let mut api = connect(cwd)?;
    let response = api.call("thread/read", json!({"threadId":id,"includeTurns":false}))?;
    let thread = &response["thread"];
    if thread["id"] != id || thread["ephemeral"] != false {
        return Err(invalid("native history identity is not a persisted thread"));
    }
    let native_cwd = thread["cwd"]
        .as_str()
        .ok_or_else(|| invalid("native history cwd missing"))?;
    let native_path = thread["path"]
        .as_str()
        .ok_or_else(|| invalid("native history path missing"))?;
    if Path::new(native_cwd).canonicalize()? != cwd.canonicalize()?
        || Path::new(native_path).canonicalize()? != path.canonicalize()?
    {
        return Err(invalid("native history path or working directory changed"));
    }
    let all = collect_items(id, |params| api.call("thread/items/list", params))?;
    Ok(json!({"thread":thread,"items":all,"complete":true}))
}

fn collect_items(
    id: &str,
    mut request: impl FnMut(Value) -> io::Result<Value>,
) -> io::Result<Vec<Value>> {
    let mut all = Vec::new();
    let mut cursor = Value::Null;
    let mut seen = HashSet::new();
    let mut size = 0;
    loop {
        let page =
            request(json!({"threadId":id,"sortDirection":"asc","limit":100,"cursor":cursor}))?;
        let items = page["data"]
            .as_array()
            .ok_or_else(|| invalid("native history page missing"))?;
        for item in items {
            size += serde_json::to_vec(item)?.len();
            if size > 256 * 1024 * 1024 {
                return Err(invalid(
                    "native history exceeds exchange limit; nothing truncated",
                ));
            }
            all.push(item.clone());
        }
        match page
            .get("nextCursor")
            .ok_or_else(|| invalid("native history pagination marker missing"))?
        {
            Value::Null => break,
            Value::String(next) if seen.insert(next.clone()) => cursor = json!(next),
            _ => return Err(invalid("native history pagination did not advance")),
        }
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_target_bootstrap_requires_its_actual_authentication_contract() {
        assert!(account_ready(
            &json!({"requiresOpenaiAuth":false,"account":null})
        ));
        assert!(account_ready(
            &json!({"requiresOpenaiAuth":true,"account":{"type":"apiKey"}})
        ));
        assert!(!account_ready(
            &json!({"requiresOpenaiAuth":true,"account":null})
        ));
        assert!(!account_ready(&json!({"account":{"type":"chatgpt"}})));
    }

    #[test]
    fn native_history_follows_every_cursor_without_cwd_or_latest_session_selection() {
        let mut calls = 0;
        let items = collect_items("exact-source", |params| {
            assert_eq!(params["threadId"], "exact-source");
            assert_eq!(params["sortDirection"], "asc");
            calls += 1;
            Ok(match calls {
                1 => {
                    assert!(params["cursor"].is_null());
                    json!({"data":[{"item":{"id":"oldest"}}],"nextCursor":"opaque-native-cursor"})
                }
                2 => {
                    assert_eq!(params["cursor"], "opaque-native-cursor");
                    json!({"data":[{"item":{"id":"latest"}}],"nextCursor":null})
                }
                _ => panic!("unexpected extra native history request"),
            })
        })
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["item"]["id"], "oldest");
        assert_eq!(items[1]["item"]["id"], "latest");
    }
    #[test]
    fn repeated_or_missing_native_pagination_never_becomes_successful_coverage() {
        let result = collect_items("source", |_| Ok(json!({"data":[],"nextCursor":"stuck"})));
        assert!(result.unwrap_err().to_string().contains("did not advance"));
        assert!(collect_items("source", |_| Ok(json!({"nextCursor":null}))).is_err());
        assert!(collect_items("source", |_| Ok(json!({"data":[]}))).is_err());
    }
}
