//! One atomic publication/recovery contract for all native target adapters.
use super::*;

pub(crate) fn uuid_from_seed(seed: &str) -> String {
    let hex = format!("{:x}", Sha256::digest(seed.as_bytes()));
    format!(
        "{}-{}-4{}-a{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

pub(crate) fn entry_identity(session_id: &str, index: usize) -> String {
    uuid_from_seed(&format!("native-entry:{session_id}:{index}"))
}

pub(super) struct Identity {
    pub id: String,
    pub stamp: String,
    pub created: time::OffsetDateTime,
}
impl Identity {
    pub fn new(history: &NativeHistory) -> io::Result<Self> {
        let created = time::OffsetDateTime::now_utc();
        let stamp = created
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(io::Error::other)?;
        let id = uuid_from_seed(&format!(
            "{}:{}:{}:{}",
            history.session_id,
            std::process::id(),
            created.unix_timestamp_nanos(),
            NEXT_IMPORT.fetch_add(1, Ordering::Relaxed)
        ));
        Ok(Self { id, stamp, created })
    }
}

#[derive(Debug)]
pub(crate) struct ImportedHistory {
    pub(crate) path: PathBuf,
    pub(crate) source_archive: PathBuf,
    pub(crate) source_context_archive: Option<PathBuf>,
    pub(crate) native_session: crate::agent_resume::PersistedAgentSession,
    /// A native database session allocated by the target, removed on rollback.
    native_cleanup: Option<(&'static str, PathBuf)>,
    /// A private directory created solely for this import.
    owned_directory: Option<PathBuf>,
    retained: bool,
    pub(crate) messages: usize,
}
impl ImportedHistory {
    pub(super) fn own_native_session(&mut self, program: &'static str, cwd: PathBuf) {
        self.native_cleanup = Some((program, cwd));
    }
    pub(super) fn own_new_directory(&mut self, directory: PathBuf) {
        self.owned_directory = Some(directory);
    }
    pub(crate) fn retain_for_recovery(&mut self) {
        self.retained = true;
    }
}
impl Drop for ImportedHistory {
    fn drop(&mut self) {
        if self.retained {
            return;
        }
        if let Some((program, cwd)) = &self.native_cleanup {
            if let Err(error) = crate::integration::native_process::capture(
                program,
                &["session", "delete", &self.native_session.session_ref.value],
                cwd,
                65536,
            ) {
                tracing::warn!(program, reason=%error, "prepared native session cleanup failed");
            }
        }
        if let Some(directory) = &self.owned_directory {
            let _ = std::fs::remove_dir_all(directory);
            return;
        }
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(&self.source_archive);
        if let Some(path) = &self.source_context_archive {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Publish the encoded target file without replacing anything, beside private
/// archives of the complete original source and its exact exported context.
pub(super) fn publish(
    history: &NativeHistory,
    records: Vec<Value>,
    path: PathBuf,
    native_session: crate::agent_resume::PersistedAgentSession,
    identity: &str,
) -> io::Result<ImportedHistory> {
    let directory = path
        .parent()
        .ok_or_else(|| invalid("native import directory missing"))?;
    std::fs::create_dir_all(directory)?;
    let source_archive = directory.join(format!("ohmyherdr-source-{identity}.jsonl.source"));
    let source_context_archive = history
        .source_context
        .as_ref()
        .map(|_| directory.join(format!("ohmyherdr-source-{identity}.context.json")));
    let mut created = Vec::new();
    let result = (|| -> io::Result<()> {
        let mut original = crate::platform::create_private_state_file(&source_archive)?;
        created.push(source_archive.clone());
        original.write_all(&history.source_bytes)?;
        original.sync_all()?;
        if let (Some(context), Some(path)) = (&history.source_context, &source_context_archive) {
            let mut file = crate::platform::create_private_state_file(path)?;
            created.push(path.clone());
            serde_json::to_writer(&mut file, context)?;
            file.sync_all()?;
        }
        let pending = path.with_extension("jsonl.pending");
        let mut file = crate::platform::create_private_state_file(&pending)?;
        created.push(pending.clone());
        for record in records {
            serde_json::to_writer(&mut file, &record)?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
        drop(file);
        // A complete native file appears atomically and never replaces an
        // existing destination.
        std::fs::hard_link(&pending, &path)?;
        created.push(path.clone());
        std::fs::remove_file(&pending)?;
        crate::platform::sync_parent_directory(directory)?;
        Ok(())
    })();
    if let Err(error) = result {
        for path in created {
            let _ = std::fs::remove_file(path);
        }
        return Err(error);
    }
    Ok(ImportedHistory {
        path,
        source_archive,
        source_context_archive,
        native_session,
        native_cleanup: None,
        owned_directory: None,
        retained: false,
        messages: history.messages.len(),
    })
}
