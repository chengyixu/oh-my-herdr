use std::time::{Duration, Instant};

use super::{App, SESSION_SAVE_DEBOUNCE};

enum SessionSaveJob {
    Clear,
    Save {
        snapshot: crate::persist::SessionSnapshot,
        history: Option<crate::persist::SessionHistorySnapshot>,
    },
}

impl App {
    pub(super) fn sync_saved_agent_profiles(&mut self) {
        self.state.agent_harness_overrides = self
            .agent_registry
            .roster
            .values()
            .filter_map(|entry| {
                entry
                    .harness_override
                    .as_ref()
                    .map(|harness| (entry.instance_id.clone(), harness.clone()))
            })
            .collect();
        self.state.saved_agent_profiles = self
            .agent_registry
            .profiles
            .values()
            .map(|profile| crate::app::state::SavedAgentProfile {
                role: profile.role.clone(),
                native_cwd: profile.native_cwd.display().to_string(),
                harness: profile.harness.clone(),
                replicas_assigned: profile.replicas_assigned,
            })
            .collect();
    }

    pub(super) fn update_agent_registry<T>(
        &mut self,
        mutation: impl FnOnce(&mut crate::agent_registry::AgentRegistry) -> T,
    ) -> std::io::Result<T> {
        let result = if !self.policy.persist_session {
            mutation(&mut self.agent_registry)
        } else {
            if self.agent_registry.version > crate::agent_registry::REGISTRY_VERSION {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    format!(
                        "agent registry version {} is newer than supported version {}",
                        self.agent_registry.version,
                        crate::agent_registry::REGISTRY_VERSION
                    ),
                ));
            }
            let (result, registry) = crate::agent_registry::update(mutation)?;
            self.agent_registry = registry;
            result
        };
        self.sync_saved_agent_profiles();
        self.state.mark_session_dirty();
        self.schedule_session_save();
        Ok(result)
    }

    pub(super) fn try_update_agent_registry<T, E>(
        &mut self,
        mutation: impl FnOnce(&mut crate::agent_registry::AgentRegistry) -> Result<T, E>,
    ) -> std::io::Result<Result<T, E>> {
        let (result, registry) = if !self.policy.persist_session {
            let mut registry = self.agent_registry.clone();
            match mutation(&mut registry) {
                Ok(result) => (result, registry),
                Err(err) => return Ok(Err(err)),
            }
        } else {
            if self.agent_registry.version > crate::agent_registry::REGISTRY_VERSION {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    format!(
                        "agent registry version {} is newer than supported version {}",
                        self.agent_registry.version,
                        crate::agent_registry::REGISTRY_VERSION
                    ),
                ));
            }
            match crate::agent_registry::try_update(mutation)? {
                Ok(committed) => committed,
                Err(err) => return Ok(Err(err)),
            }
        };
        self.agent_registry = registry;
        self.sync_saved_agent_profiles();
        self.state.mark_session_dirty();
        self.schedule_session_save();
        Ok(Ok(result))
    }

    pub(super) fn refresh_agent_registry_for_read(&mut self) -> std::io::Result<()> {
        if self.policy.persist_session {
            self.agent_registry = crate::agent_registry::load_for_read()?;
        }
        self.sync_saved_agent_profiles();
        Ok(())
    }
    pub(super) fn schedule_session_save(&mut self) {
        if self.policy.persist_session {
            self.pane_exit_checkpoint_pending = false;
            self.session_save_deadline = Some(Instant::now() + SESSION_SAVE_DEBOUNCE);
        }
    }

    pub(crate) fn sync_session_save_schedule(&mut self) {
        if self.reap_finished_session_save() {
            self.state.mark_session_dirty();
        }
        if self.state.session_dirty {
            self.state.session_dirty = false;
            self.schedule_session_save();
        }
    }

    fn reap_finished_session_save(&mut self) -> bool {
        if self
            .session_save_thread
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
        {
            if let Some(thread) = self.session_save_thread.take() {
                return !matches!(thread.join(), Ok(Ok(())));
            }
        }
        false
    }

    fn capture_session_save_job(&self) -> SessionSaveJob {
        if self.state.workspaces.is_empty() {
            SessionSaveJob::Clear
        } else {
            let snapshot = crate::persist::capture(
                &self.state.workspaces,
                &self.state.terminals,
                &self.terminal_runtimes,
                self.state.active,
                self.state.selected,
            );
            let history = self.persist_pane_history.then(|| {
                crate::persist::capture_history(
                    &snapshot,
                    &self.state.workspaces,
                    &self.terminal_runtimes,
                )
            });
            SessionSaveJob::Save { snapshot, history }
        }
    }

    pub(crate) fn start_background_session_save(&mut self) {
        if !self.policy.persist_session {
            self.session_save_deadline = None;
            return;
        }

        if self.reap_finished_session_save() {
            self.state.mark_session_dirty();
            self.session_save_deadline = Some(Instant::now() + SESSION_SAVE_DEBOUNCE);
        }
        if self.session_save_thread.is_some() {
            self.session_save_deadline = Some(Instant::now() + Duration::from_millis(250));
            return;
        }

        let job = self.capture_session_save_job();
        self.pane_exit_checkpoint_pending = false;
        self.session_save_deadline = None;
        let writer = self.session_writer.clone();
        match std::thread::Builder::new()
            .name("herdr-session-save".into())
            .spawn(move || run_session_save_job(job, &writer))
        {
            Ok(thread) => self.session_save_thread = Some(thread),
            Err(err) => {
                tracing::warn!(err = %err, "failed to spawn session save thread; saving inline");
                if let Err(err) =
                    run_session_save_job(self.capture_session_save_job(), &self.session_writer)
                {
                    tracing::warn!(err=%err,"session save failed; retrying");
                    self.session_save_deadline = Some(Instant::now() + SESSION_SAVE_DEBOUNCE);
                }
            }
        }
    }

    pub(crate) fn save_session_now(&mut self) {
        if let Some(thread) = self.session_save_thread.take() {
            let _ = thread.join();
        }

        if !self.policy.persist_session {
            self.session_save_deadline = None;
            return;
        }

        match run_session_save_job(self.capture_session_save_job(), &self.session_writer) {
            Ok(()) => {
                self.pane_exit_checkpoint_pending = false;
                self.session_save_deadline = None;
            }
            Err(err) => {
                tracing::warn!(err=%err,"session save failed; retrying");
                self.state.mark_session_dirty();
                self.session_save_deadline = Some(Instant::now() + SESSION_SAVE_DEBOUNCE);
            }
        }
    }

    pub(crate) fn checkpoint_session_before_pane_exit(&mut self) {
        if !self.policy.persist_session
            || (self.pane_exit_checkpoint_pending && !self.state.session_dirty)
        {
            return;
        }
        self.save_session_now();
        self.pane_exit_checkpoint_pending = true;
        self.state.session_dirty = false;
    }

    pub(crate) fn finish_checkpointed_pane_exit(&mut self) {
        if self.pane_exit_checkpoint_pending {
            self.state.session_dirty = false;
            self.session_save_deadline = Some(Instant::now() + SESSION_SAVE_DEBOUNCE);
        }
    }

    pub(crate) fn save_session_on_shutdown(&mut self) {
        if self.pane_exit_checkpoint_pending && !self.state.session_dirty {
            self.session_save_deadline = None;
            return;
        }
        self.save_session_now();
    }
}

fn run_session_save_job(
    job: SessionSaveJob,
    writer: &std::sync::Mutex<crate::persist::SessionWriter>,
) -> std::io::Result<()> {
    let mut writer = match writer.lock() {
        Ok(writer) => writer,
        Err(err) => {
            tracing::warn!(err = %err, "session writer is poisoned; refusing to modify session");
            return Err(std::io::Error::other("session writer poisoned"));
        }
    };
    match job {
        SessionSaveJob::Clear => writer.try_clear(),
        SessionSaveJob::Save { snapshot, history } => writer.try_save(&snapshot, history.as_ref()),
    }
}
