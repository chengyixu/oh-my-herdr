//! Capability-bound transport to a native harness's own active context exporter.
//! Registrations are ephemeral and never enter persisted session/configuration data.
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use interprocess::local_socket::traits::Stream as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NativeContextRegistration {
    pub pane_id: String,
    pub source: String,
    pub agent: String,
    pub session_id: String,
    pub session_path: String,
    pub endpoint: String,
    pub token: String,
    pub process_id: u32,
    pub seq: u64,
    pub version: u32,
}

impl std::fmt::Debug for NativeContextRegistration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeContextRegistration")
            .field("pane_id", &self.pane_id)
            .field("source", &self.source)
            .field("agent", &self.agent)
            .field("session_id", &self.session_id)
            .field("endpoint", &self.endpoint)
            .field("process_id", &self.process_id)
            .field("version", &self.version)
            .field("token", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct NativeContextSnapshot {
    pub version: u32,
    pub format: String,
    pub session_id: String,
    pub session_path: String,
    pub leaf_id: Option<String>,
    pub cwd: PathBuf,
    pub header: Value,
    pub branch: Vec<Value>,
    /// Native SDK projection for the next model request, before target encoding.
    pub model_messages: Vec<Value>,
    pub projection: Value,
    pub system_prompt: String,
}

#[derive(Debug)]
pub(crate) struct NativeContextError {
    pub code: String,
    pub message: String,
}
impl NativeContextError {
    fn transport(_: impl std::fmt::Display) -> Self {
        Self { code: "native_context_unavailable".into(), message: "The native context exporter is unavailable; check the original pane before retrying".into() }
    }
}

impl NativeContextRegistration {
    pub(crate) fn matches_session(&self, session: &crate::api::schema::AgentSessionInfo) -> bool {
        if session.source != self.source || session.agent != self.agent {
            return false;
        }
        match session.kind {
            crate::agent_resume::AgentSessionRefKind::Id => session.value == self.session_id,
            crate::agent_resume::AgentSessionRefKind::Path => {
                session.value == self.session_path || {
                    let expected = Path::new(&self.session_path).canonicalize();
                    let reported = Path::new(&session.value).canonicalize();
                    matches!((expected,reported),(Ok(a),Ok(b)) if a==b)
                }
            }
        }
    }

    pub(crate) fn validate_endpoint(&self) -> io::Result<()> {
        if self.version != 1
            || self.process_id == 0
            || crate::agent_resume::AgentSessionRef::id(self.session_id.clone()).is_none()
            || crate::agent_resume::AgentSessionRef::path(self.session_path.clone()).is_none()
            || !(16..=128).contains(&self.token.len())
            || !self
                .token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid native context registration",
            ));
        }
        crate::platform::validate_private_context_endpoint(Path::new(&self.endpoint))
    }

    fn request(
        &self,
        action: &str,
        snapshot_id: Option<&str>,
    ) -> Result<Value, NativeContextError> {
        self.validate_endpoint()
            .map_err(NativeContextError::transport)?;
        let mut stream = crate::ipc::connect_local_stream(Path::new(&self.endpoint))
            .map_err(NativeContextError::transport)?;
        stream
            .set_recv_timeout(Some(Duration::from_secs(25)))
            .map_err(NativeContextError::transport)?;
        stream
            .set_send_timeout(Some(Duration::from_secs(5)))
            .map_err(NativeContextError::transport)?;
        let command = json!({"token":self.token,"action":action,"snapshot_id":snapshot_id});
        serde_json::to_writer(&mut stream, &command).map_err(NativeContextError::transport)?;
        stream
            .write_all(b"\n")
            .map_err(NativeContextError::transport)?;
        let mut bytes = Vec::new();
        stream
            .take(256 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(NativeContextError::transport)?;
        if bytes.len() > 256 * 1024 * 1024 {
            return Err(NativeContextError {
                code: "context_too_large".into(),
                message: "Native context exceeded the exchange limit; nothing was truncated".into(),
            });
        }
        let response: Value =
            serde_json::from_slice(&bytes).map_err(NativeContextError::transport)?;
        if response["ok"] != true {
            return Err(NativeContextError {
                code: response
                    .pointer("/error/code")
                    .and_then(Value::as_str)
                    .unwrap_or("native_context_unavailable")
                    .chars()
                    .take(128)
                    .collect(),
                message: response
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("Native context could not be exported; the source was not closed")
                    .chars()
                    .take(512)
                    .collect(),
            });
        }
        Ok(response)
    }

    pub(crate) fn snapshot(&self) -> Result<(String, NativeContextSnapshot), NativeContextError> {
        let response = self.request("snapshot", None)?;
        let id = response["snapshot_id"]
            .as_str()
            .filter(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| NativeContextError::transport("invalid context identity"))?
            .to_owned();
        let snapshot: NativeContextSnapshot = serde_json::from_value(response["snapshot"].clone())
            .map_err(NativeContextError::transport)?;
        if snapshot.version != 1
            || snapshot.session_id != self.session_id
            || snapshot.session_path != self.session_path
        {
            return Err(NativeContextError {
                code: "source_changed".into(),
                message: "The native session identity changed; the source was not closed".into(),
            });
        }
        Ok((id, snapshot))
    }

    pub(crate) fn release(&self) {
        let _ = self.request("release", None);
    }

    pub(crate) fn ready(&self) -> Result<(), NativeContextError> {
        if crate::harness::find(&self.agent).is_some_and(|h| h.native_editor_controls().is_none()) {
            return self.snapshot().map(|_| ());
        }
        let response = self.request("ready", None)?;
        if response["ready"] != true {
            return Err(NativeContextError::transport(
                "native prompt not acknowledged",
            ));
        }
        Ok(())
    }

    pub(crate) fn close_if_unchanged(&self, snapshot_id: &str) -> Result<(), NativeContextError> {
        let response = self.request("close", Some(snapshot_id))?;
        if response["closing"] != true {
            return Err(NativeContextError::transport(
                "native close was not acknowledged",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    #[test]
    fn native_path_aliases_match_but_another_file_in_the_same_cwd_never_does() {
        let root =
            std::env::temp_dir().join(format!("native-context-identity-{}", std::process::id()));
        std::fs::create_dir_all(root.join("child")).unwrap();
        let path = root.join("exact.jsonl");
        let other = root.join("other.jsonl");
        std::fs::write(&path, b"original\n").unwrap();
        std::fs::write(&other, b"original\n").unwrap();
        let context:NativeContextRegistration=serde_json::from_value(json!({"pane_id":"w1:p1","source":"herdr:pi","agent":"pi","session_id":"exact-id","session_path":path,"endpoint":"/unused","token":"test-only-capability","process_id":1,"seq":1,"version":1})).unwrap();
        let mut reported = crate::api::schema::AgentSessionInfo {
            source: "herdr:pi".into(),
            agent: "pi".into(),
            kind: crate::agent_resume::AgentSessionRefKind::Path,
            value: root.join("child/../exact.jsonl").to_string_lossy().into(),
        };
        assert!(context.matches_session(&reported));
        reported.value = other.to_string_lossy().into();
        assert!(!context.matches_session(&reported));
        std::fs::remove_dir_all(root).unwrap();
    }
}
