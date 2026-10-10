use std::time::{Duration, Instant};

use bytes::Bytes;

use crate::api::schema::{
    AgentBroadcastParams, AgentProfileCreateParams, AgentProfileDeleteParams,
    AgentProfileSetMdParams, AgentPromptParams, AgentRenameParams, AgentReviveParams,
    AgentRosterEntryInfo, AgentRosterStatus, AgentSendKeysParams, AgentSpawnParams,
    AgentStartParams, AgentTarget, PaneReadResult, ResponseResult,
};
use crate::app::App;

use super::responses::{encode_error, encode_error_body, encode_success};

const AGENT_PROMPT_SUBMIT_DELAY: Duration = Duration::from_millis(300);

// Codex's Windows input reader does not surface bracketed paste. It detects the prompt as a
// "paste burst" and, while that burst is buffered, rewrites a following Enter into a newline
// instead of submitting. The burst only flushes after an idle timeout, so any size-based delay is
// a timing guess that fails when ConPTY delivery lags it. Codex flushes a buffered burst
// synchronously when it receives a non-character key, so appending one after the paste gives the
// submission a deterministic paste boundary regardless of prompt size or delivery speed.
#[cfg(windows)]
fn append_codex_paste_boundary(runtime: &crate::terminal::TerminalRuntime, text: &mut Vec<u8>) {
    let keys = match crate::app::api_helpers::encode_api_keys(runtime, &["right".to_string()]) {
        Ok(keys) => keys,
        Err(key) => {
            tracing::warn!(key = %key, "failed to encode Codex paste boundary key");
            return;
        }
    };
    if let Some(key) = keys.into_iter().find(|bytes| !bytes.is_empty()) {
        text.extend_from_slice(&key);
    }
}

impl App {
    pub(super) fn handle_agent_list(&mut self, id: String) -> String {
        encode_success(
            id,
            ResponseResult::AgentList {
                agents: self.collect_agent_infos(),
            },
        )
    }

    pub(super) fn handle_agent_get(&mut self, id: String, target: AgentTarget) -> String {
        self.reconcile_managed_agent_target(&target.target);
        let agent = match self.agent_info_for_target(&target.target) {
            Ok(agent) => agent,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentInfo { agent })
    }

    pub(super) fn handle_agent_focus(&mut self, id: String, target: AgentTarget) -> String {
        let agent = match self.focus_agent_target(&target.target) {
            Ok(agent) => agent,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentInfo { agent })
    }

    pub(super) fn handle_agent_rename(&mut self, id: String, params: AgentRenameParams) -> String {
        let agent = match self.rename_agent_target(&params.target, params.name) {
            Ok(agent) => agent,
            Err(err) => return encode_error_body(id, self.agent_rename_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentInfo { agent })
    }

    pub(super) fn handle_agent_start(&mut self, id: String, params: AgentStartParams) -> String {
        let (agent, argv) = match self.start_agent(params) {
            Ok(started) => started,
            Err(err) => return encode_error_body(id, self.agent_start_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentStarted { agent, argv })
    }

    pub(super) fn handle_agent_spawn(&mut self, id: String, params: AgentSpawnParams) -> String {
        match self.spawn_agent(params) {
            Ok(crate::app::agents::AgentSpawnOutcome::Spawned(spawned)) => {
                encode_success(id, ResponseResult::AgentSpawned { spawned: *spawned })
            }
            Ok(crate::app::agents::AgentSpawnOutcome::Pending(pending)) => {
                let pane_id = pending.pane_id().to_string();
                self.cancel_pending_agent_spawn(*pending);
                encode_error(
                    id,
                    "agent_spawn_failed",
                    format!("agent pane {pane_id} busy"),
                )
            }
            Err(err) => encode_error_body(id, self.agent_spawn_error_body(err)),
        }
    }

    pub(super) fn handle_agent_roster_list(&mut self, id: String) -> String {
        if let Err(err) = self.refresh_agent_registry_for_read() {
            return encode_error_body(
                id,
                self.agent_profile_error_body(crate::app::agents::AgentProfileError::LoadFailed(
                    err.to_string(),
                )),
            );
        }
        let entries = self
            .agent_registry
            .roster
            .values()
            .cloned()
            .map(|entry| AgentRosterEntryInfo {
                instance_id: entry.instance_id,
                profile_id: entry.profile_id,
                role: entry.role,
                replica_suffix: entry.replica_suffix,
                display_name: entry.display_name,
                status: match entry.status {
                    crate::agent_registry::AgentStatus::Active => AgentRosterStatus::Active,
                    crate::agent_registry::AgentStatus::Idle => AgentRosterStatus::Idle,
                    crate::agent_registry::AgentStatus::Working => AgentRosterStatus::Working,
                    crate::agent_registry::AgentStatus::Blocked => AgentRosterStatus::Blocked,
                    crate::agent_registry::AgentStatus::Done => AgentRosterStatus::Done,
                    crate::agent_registry::AgentStatus::Unknown => AgentRosterStatus::Unknown,
                    crate::agent_registry::AgentStatus::Terminated => AgentRosterStatus::Terminated,
                },
                harness_override: entry.harness_override,
                last_pane: entry.last_pane,
                last_seen_at: entry.last_seen_at,
            })
            .collect();
        encode_success(id, ResponseResult::AgentRosterList { entries })
    }

    pub(super) fn handle_agent_harness_prepare(
        &mut self,
        id: String,
        target: AgentTarget,
    ) -> String {
        let resolved = match self.resolve_agent_target(&target.target) {
            Ok(target) => target,
            Err(_) => return encode_error(id, "agent_not_found", "agent pane was not found"),
        };
        if let Err(err) = self.refresh_agent_registry_for_read() {
            return encode_error(id, "agent_profile_load_failed", err.to_string());
        }
        let Some(terminal_id) = self.state.workspaces[resolved.ws_idx]
            .terminal_id(resolved.pane_id)
            .cloned()
        else {
            return encode_error(id, "agent_not_found", "agent terminal was not found");
        };
        let Some(terminal) = self.state.terminals.get(&terminal_id) else {
            return encode_error(id, "agent_not_found", "agent terminal was not found");
        };
        if let Some(entry) = terminal
            .live_roster_instance_id()
            .and_then(|instance_id| self.agent_registry.roster.get(instance_id))
        {
            return encode_success(
                id,
                ResponseResult::AgentHarnessSet {
                    instance_id: entry.instance_id.clone(),
                    harness_override: entry.harness_override.clone(),
                },
            );
        }
        let harness = terminal
            .effective_agent_label()
            .or_else(|| {
                terminal
                    .managed_agent_kind()
                    .map(crate::detect::agent_label)
            })
            .unwrap_or("codex")
            .to_string();
        if !matches!(harness.as_str(), "codex" | "pi" | "claude") {
            return encode_error(
                id,
                "invalid_agent_kind",
                "this harness does not support saved native launches",
            );
        }
        let seed = format!("pane-{}", terminal_id.as_str().trim_start_matches("term_"));
        let cwd = self
            .launch_cwd_for_pane_in_workspace(resolved.ws_idx, resolved.pane_id)
            .unwrap_or_else(|| std::path::PathBuf::from("/"));
        let pane_address = self.public_pane_id(resolved.ws_idx, resolved.pane_id);
        let entry = if !self.policy.persist_session {
            match self.update_agent_registry(|registry| {
                registry.adopt_instance(&seed, cwd, &harness, pane_address)
            }) {
                Ok(Some(entry)) => entry,
                Ok(None) => {
                    return encode_error(
                        id,
                        "agent_instance_unavailable",
                        "could not allocate exact agent identity",
                    )
                }
                Err(err) => {
                    return encode_error(id, "agent_profile_persist_failed", err.to_string())
                }
            }
        } else {
            if self.agent_registry.version > crate::agent_registry::REGISTRY_VERSION {
                return encode_error(
                    id,
                    "agent_profile_persist_failed",
                    "registry version is newer than supported",
                );
            }
            match crate::agent_registry::adopt_terminal(&seed, cwd, &harness, pane_address) {
                Ok((entry, registry)) => {
                    self.agent_registry = registry;
                    self.sync_saved_agent_profiles();
                    entry
                }
                Err(err) => {
                    return encode_error(id, "agent_profile_persist_failed", err.to_string())
                }
            }
        };
        if let Some(terminal) = self.state.terminals.get_mut(&terminal_id) {
            terminal
                .bind_agent_roster_identity(entry.display_name.clone(), entry.instance_id.clone());
        }
        self.state.mark_session_dirty();
        self.schedule_session_save();
        encode_success(
            id,
            ResponseResult::AgentHarnessSet {
                instance_id: entry.instance_id,
                harness_override: entry.harness_override,
            },
        )
    }

    pub(super) fn handle_agent_harness_set(
        &mut self,
        id: String,
        params: crate::api::schema::AgentHarnessSetParams,
    ) -> String {
        let harness = match params.harness {
            Some(harness) if matches!(harness.as_str(), "codex" | "pi" | "claude") => Some(harness),
            Some(_) => {
                return encode_error(
                    id,
                    "invalid_agent_kind",
                    "native harness must be codex, pi, or claude",
                )
            }
            None => None,
        };
        let instance_id = params.instance_id;
        let result = self.try_update_agent_registry(|registry| {
            let Some(entry) = registry.roster.get_mut(&instance_id) else {
                return Err(());
            };
            entry.harness_override = harness.clone();
            Ok(())
        });
        match result {
            Ok(Ok(())) => encode_success(
                id,
                ResponseResult::AgentHarnessSet {
                    instance_id,
                    harness_override: harness,
                },
            ),
            Ok(Err(())) => encode_error(
                id,
                "agent_instance_not_found",
                "exact agent instance was not found",
            ),
            Err(err) => encode_error(id, "agent_profile_persist_failed", err.to_string()),
        }
    }

    pub(super) fn handle_agent_revive(&mut self, id: String, params: AgentReviveParams) -> String {
        match self.revive_agent(
            params.instance_id,
            params.tab_id,
            params.cwd_mode,
            params.timeout_ms,
            params.args,
        ) {
            Ok(crate::app::agents::AgentSpawnOutcome::Spawned(revived)) => {
                encode_success(id, ResponseResult::AgentRevived { revived: *revived })
            }
            Ok(crate::app::agents::AgentSpawnOutcome::Pending(pending)) => {
                let pane_id = pending.pane_id().to_string();
                self.cancel_pending_agent_spawn(*pending);
                encode_error(
                    id,
                    "agent_revive_failed",
                    format!("agent pane {pane_id} busy"),
                )
            }
            Err(err) => encode_error_body(id, self.agent_spawn_error_body(err)),
        }
    }

    pub(crate) fn handle_deferred_agent_spawn_api_request(
        &mut self,
        id: String,
        params: AgentSpawnParams,
        respond_to: std::sync::mpsc::Sender<String>,
    ) -> bool {
        match self.spawn_agent(params) {
            Ok(crate::app::agents::AgentSpawnOutcome::Spawned(spawned)) => {
                let _ = respond_to.send(encode_success(
                    id,
                    ResponseResult::AgentSpawned { spawned: *spawned },
                ));
            }
            Ok(crate::app::agents::AgentSpawnOutcome::Pending(pending)) => {
                self.pending_agent_spawns
                    .push(crate::app::agents::DeferredAgentSpawn {
                        id,
                        respond_to,
                        pending: *pending,
                        revived: false,
                    });
                self.sync_pending_agent_spawn_deadline();
            }
            Err(err) => {
                let _ = respond_to.send(encode_error_body(id, self.agent_spawn_error_body(err)));
            }
        }
        true
    }

    pub(crate) fn handle_deferred_agent_revive_api_request(
        &mut self,
        id: String,
        params: AgentReviveParams,
        respond_to: std::sync::mpsc::Sender<String>,
    ) -> bool {
        match self.revive_agent(
            params.instance_id,
            params.tab_id,
            params.cwd_mode,
            params.timeout_ms,
            params.args,
        ) {
            Ok(crate::app::agents::AgentSpawnOutcome::Spawned(revived)) => {
                let _ = respond_to.send(encode_success(
                    id,
                    ResponseResult::AgentRevived { revived: *revived },
                ));
            }
            Ok(crate::app::agents::AgentSpawnOutcome::Pending(pending)) => {
                self.pending_agent_spawns
                    .push(crate::app::agents::DeferredAgentSpawn {
                        id,
                        respond_to,
                        pending: *pending,
                        revived: true,
                    });
                self.sync_pending_agent_spawn_deadline();
            }
            Err(err) => {
                let _ = respond_to.send(encode_error_body(id, self.agent_spawn_error_body(err)));
            }
        }
        true
    }

    pub(crate) fn complete_due_agent_spawns(&mut self, now: Instant) -> bool {
        let completions = self.poll_pending_agent_spawns(now);
        if completions.is_empty() {
            return false;
        }

        for completion in completions {
            let response = match completion.result {
                Ok(spawned) if completion.revived => encode_success(
                    completion.id,
                    ResponseResult::AgentRevived { revived: spawned },
                ),
                Ok(spawned) => {
                    encode_success(completion.id, ResponseResult::AgentSpawned { spawned })
                }
                Err(err) => encode_error_body(completion.id, self.agent_spawn_error_body(err)),
            };
            let _ = completion.respond_to.send(response);
        }
        true
    }

    pub(super) fn handle_agent_profile_edit(
        &mut self,
        id: String,
        params: crate::api::schema::AgentProfileEditParams,
    ) -> String {
        let mut profile = match self.profile(&params.role) {
            Ok(profile) => profile,
            Err(err) => return encode_error_body(id, self.agent_profile_error_body(err)),
        };
        if !profile
            .mds
            .iter()
            .any(|document| document.name == "AGENTS.md")
            && self.policy.persist_session
        {
            match crate::agent_registry::ensure_owned_instructions() {
                Ok(registry) => self.agent_registry = registry,
                Err(err) => {
                    return encode_error(id, "agent_profile_persist_failed", err.to_string())
                }
            }
            profile = match self.profile(&params.role) {
                Ok(profile) => profile,
                Err(err) => return encode_error_body(id, self.agent_profile_error_body(err)),
            };
        }
        if params
            .settings
            .as_ref()
            .is_some_and(|settings| settings.role != params.role)
        {
            return encode_error(
                id,
                "invalid_profile_request",
                "settings must belong to the selected profile",
            );
        }
        if params.documents.len() > 32
            || params
                .documents
                .values()
                .any(|content| content.len() > 256 * 1024 || content.contains('\0'))
        {
            return encode_error(
                id,
                "invalid_agent_instructions",
                "profile documents exceed the size limit or contain NUL",
            );
        }
        for name in params.documents.keys().chain(params.delete_document.iter()) {
            if name != "AGENTS.md"
                && crate::agent_registry::owned_markdown_path(&params.role, name).is_err()
            {
                return encode_error(id, "invalid_md_name", "invalid profile document name");
            }
        }
        if params.delete_document.as_deref() == Some("AGENTS.md")
            || params
                .delete_document
                .as_ref()
                .is_some_and(|name| params.documents.contains_key(name))
        {
            return encode_error(
                id,
                "invalid_profile_request",
                "cannot delete AGENTS.md or save and delete the same document",
            );
        }
        if let Some(settings) = params.settings {
            profile = match self.set_profile(settings) {
                Ok(profile) => profile,
                Err(err) => return encode_error_body(id, self.agent_profile_error_body(err)),
            };
        }
        for (name, content) in params.documents {
            let result = if name == "AGENTS.md" {
                crate::agent_registry::replace_owned_instructions(&params.role, &content)
            } else {
                let path = match crate::agent_registry::owned_markdown_path(&params.role, &name) {
                    Ok(path) => path,
                    Err(err) => return encode_error(id, "invalid_md_name", err.to_string()),
                };
                if path.exists() {
                    crate::agent_registry::replace_owned_markdown(&params.role, &name, &content)
                } else {
                    match crate::agent_registry::create_owned_markdown(
                        &params.role,
                        &name,
                        &content,
                    ) {
                        Ok(path) => match self.set_profile_md(&params.role, &name, path.to_str()) {
                            Ok(updated) => {
                                profile = updated;
                                Ok(())
                            }
                            Err(err) => {
                                let _ = crate::agent_registry::remove_owned_markdown(
                                    &params.role,
                                    &name,
                                );
                                return encode_error_body(id, self.agent_profile_error_body(err));
                            }
                        },
                        Err(err) => Err(err),
                    }
                }
            };
            if let Err(err) = result {
                return encode_error(id, "agent_profile_persist_failed", err.to_string());
            }
        }
        if let Some(name) = params.delete_document {
            if let Err(err) = crate::agent_registry::remove_owned_markdown(&params.role, &name) {
                return encode_error(id, "agent_profile_persist_failed", err.to_string());
            }
            profile = match self.set_profile_md(&params.role, &name, None) {
                Ok(profile) => profile,
                Err(err) => return encode_error_body(id, self.agent_profile_error_body(err)),
            };
        }
        let mut documents = std::collections::BTreeMap::new();
        for document in &profile.mds {
            if document.name == "AGENTS.md"
                || crate::agent_registry::is_owned_markdown_path(
                    &params.role,
                    &document.name,
                    std::path::Path::new(&document.path),
                )
            {
                let content = if document.name == "AGENTS.md" {
                    crate::agent_registry::read_owned_instructions(&params.role)
                } else {
                    crate::agent_registry::read_owned_markdown(&params.role, &document.name)
                };
                match content {
                    Ok(content) => {
                        documents.insert(document.name.clone(), content);
                    }
                    Err(err) => {
                        return encode_error(id, "agent_profile_load_failed", err.to_string())
                    }
                }
            }
        }
        encode_success(
            id,
            ResponseResult::AgentProfileDossier { profile, documents },
        )
    }

    pub(super) fn handle_agent_profile_set_md(
        &mut self,
        id: String,
        params: AgentProfileSetMdParams,
    ) -> String {
        match self.set_profile_md(&params.role, &params.name, params.path.as_deref()) {
            Ok(profile) => encode_success(id, ResponseResult::AgentProfileInfoSet { profile }),
            Err(err) => encode_error_body(id, self.agent_profile_error_body(err)),
        }
    }

    pub(super) fn handle_agent_profile_create(
        &mut self,
        id: String,
        params: AgentProfileCreateParams,
    ) -> String {
        match self.create_profile(params) {
            Ok(profile) => encode_success(id, ResponseResult::AgentProfileInfo { profile }),
            Err(err) => encode_error_body(id, self.agent_profile_error_body(err)),
        }
    }

    pub(super) fn handle_agent_profile_get(
        &mut self,
        id: String,
        params: crate::api::schema::AgentProfileGetParams,
    ) -> String {
        match self.profile(&params.role) {
            Ok(profile) => encode_success(id, ResponseResult::AgentProfileInfo { profile }),
            Err(err) => encode_error_body(id, self.agent_profile_error_body(err)),
        }
    }

    pub(super) fn handle_agent_profile_list(&mut self, id: String) -> String {
        match self.profiles() {
            Ok(profiles) => encode_success(id, ResponseResult::AgentProfileList { profiles }),
            Err(err) => encode_error_body(id, self.agent_profile_error_body(err)),
        }
    }

    pub(super) fn handle_agent_profile_set(
        &mut self,
        id: String,
        params: crate::api::schema::AgentProfileSetParams,
    ) -> String {
        match self.set_profile(params) {
            Ok(profile) => encode_success(id, ResponseResult::AgentProfileInfo { profile }),
            Err(err) => encode_error_body(id, self.agent_profile_error_body(err)),
        }
    }

    pub(super) fn handle_agent_profile_delete(
        &mut self,
        id: String,
        params: AgentProfileDeleteParams,
    ) -> String {
        match self.delete_profile(&params.role) {
            Ok(profile) => encode_success(
                id,
                ResponseResult::AgentProfileDeleted { role: profile.role },
            ),
            Err(err) => encode_error_body(id, self.agent_profile_error_body(err)),
        }
    }

    fn prepare_agent_prompt(
        &self,
        resolved: &crate::app::terminal_targets::TerminalTarget,
        target: &str,
    ) -> Result<
        (&crate::terminal::TerminalRuntime, crate::detect::Agent),
        crate::api::schema::ErrorBody,
    > {
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(resolved.ws_idx)
            .and_then(|workspace| workspace.terminal_id(resolved.pane_id))
            .cloned()
        else {
            return Err(self.agent_target_error_body(
                crate::app::terminal_targets::TerminalTargetError::NotFound {
                    target: target.to_string(),
                },
            ));
        };
        let Some(terminal) = self.state.terminals.get(&terminal_id) else {
            return Err(self.agent_target_error_body(
                crate::app::terminal_targets::TerminalTargetError::NotFound {
                    target: target.to_string(),
                },
            ));
        };
        if terminal.state == crate::detect::AgentState::Blocked {
            return Err(crate::api::schema::ErrorBody {
                code: "agent_blocked".into(),
                message: format!("agent {target} is blocked and requires interactive input"),
            });
        }
        let Some(expected_agent) = terminal.effective_known_agent() else {
            return Err(crate::api::schema::ErrorBody {
                code: "agent_not_ready".into(),
                message: format!("agent {target} is not ready for prompts"),
            });
        };
        if terminal.managed_agent_launch_pending() {
            return Err(crate::api::schema::ErrorBody {
                code: "agent_not_ready".into(),
                message: format!("agent {target} is not ready for prompts"),
            });
        }
        let Some(runtime) = self.lookup_runtime_sender(resolved.ws_idx, resolved.pane_id) else {
            return Err(self.agent_target_error_body(
                crate::app::terminal_targets::TerminalTargetError::NotFound {
                    target: target.to_string(),
                },
            ));
        };
        if !super::super::agents::runtime_hosts_agent(runtime, expected_agent) {
            return Err(crate::api::schema::ErrorBody {
                code: "agent_not_ready".into(),
                message: format!("agent {target} is no longer the pane foreground process"),
            });
        }
        Ok((runtime, expected_agent))
    }

    fn handle_deferred_broadcast(
        &mut self,
        id: String,
        params: AgentBroadcastParams,
        respond_to: std::sync::mpsc::Sender<String>,
    ) -> bool {
        if params.text.is_empty() {
            let _ = respond_to.send(encode_error(
                id,
                "empty_agent_prompt",
                "agent prompt must not be empty",
            ));
            return false;
        }
        let targets = self.agent_targets_for_role(&params.role);
        if targets.is_empty() {
            let _ = respond_to.send(agent_not_found(id, &params.role));
            return false;
        }
        for target in &targets {
            if let Err(error) = self.prepare_agent_prompt(target, &params.role) {
                let _ = respond_to.send(encode_error_body(id, error));
                return false;
            }
        }
        let mut queued = Vec::new();
        for target in targets {
            let Some(pane) = self.public_pane_id(target.ws_idx, target.pane_id) else {
                let _ = respond_to.send(agent_not_found(id, &params.role));
                return false;
            };
            match self.queue_agent_prompt(
                id.clone(),
                AgentPromptParams {
                    target: pane,
                    text: params.text.clone(),
                    wait: None,
                },
            ) {
                Ok((_, agent, completion)) => queued.push((agent, completion)),
                Err(error) => {
                    let _ = respond_to.send(error);
                    return false;
                }
            }
        }
        std::thread::spawn(move || {
            let mut agents = Vec::new();
            for (agent, completion) in queued {
                match completion.recv() {
                    Ok(Ok(())) => agents.push(agent),
                    _ => {
                        let _ = respond_to.send(encode_error(
                            id,
                            "agent_prompt_failed",
                            "ordered broadcast submission failed",
                        ));
                        return;
                    }
                }
            }
            let _ = respond_to.send(encode_success(
                id,
                ResponseResult::AgentBroadcast { agents },
            ));
        });
        true
    }

    pub(crate) fn handle_deferred_agent_api_request(
        &mut self,
        request: crate::api::schema::Request,
        respond_to: std::sync::mpsc::Sender<String>,
    ) -> bool {
        if let crate::api::schema::Method::AgentBroadcast(params) = request.method {
            return self.handle_deferred_broadcast(request.id, params, respond_to);
        }
        let crate::api::schema::Method::AgentPrompt(params) = request.method else {
            return false;
        };
        match self.queue_agent_prompt(request.id, params) {
            Ok((id, agent, completion)) => {
                std::thread::spawn(move || {
                    let response = match completion.recv() {
                        Ok(Ok(())) => encode_success(id, ResponseResult::AgentPrompted { agent }),
                        Ok(Err(err)) if err.kind() == std::io::ErrorKind::TimedOut => {
                            encode_error(id, "timeout", err.to_string())
                        }
                        Ok(Err(err)) => encode_error(id, "agent_prompt_failed", err.to_string()),
                        Err(_) => encode_error(id, "agent_prompt_failed", "pty actor closed"),
                    };
                    let _ = respond_to.send(response);
                });
            }
            Err(response) => {
                let _ = respond_to.send(response);
            }
        }
        true
    }

    fn queue_agent_prompt(
        &mut self,
        id: String,
        params: AgentPromptParams,
    ) -> Result<
        (
            String,
            crate::api::schema::AgentInfo,
            std::sync::mpsc::Receiver<std::io::Result<()>>,
        ),
        String,
    > {
        if params.text.is_empty() {
            return Err(encode_error(
                id,
                "empty_agent_prompt",
                "agent prompt must not be empty",
            ));
        }
        let resolved = match self.resolve_agent_target(&params.target) {
            Ok(resolved) => resolved,
            Err(err) => return Err(encode_error_body(id, self.agent_target_error_body(err))),
        };
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(resolved.ws_idx)
            .and_then(|workspace| workspace.terminal_id(resolved.pane_id))
            .cloned()
        else {
            return Err(agent_not_found(id, &params.target));
        };
        let Some(terminal) = self.state.terminals.get(&terminal_id) else {
            return Err(agent_not_found(id, &params.target));
        };
        if terminal.state == crate::detect::AgentState::Blocked {
            return Err(encode_error(
                id,
                "agent_blocked",
                format!(
                    "agent {} is blocked and requires interactive input",
                    params.target
                ),
            ));
        }
        let Some(expected_agent) = terminal.effective_known_agent() else {
            return Err(agent_not_ready(id, &params.target));
        };
        if terminal.managed_agent_launch_pending() {
            return Err(agent_not_ready(id, &params.target));
        }
        let Some(runtime) = self.lookup_runtime_sender(resolved.ws_idx, resolved.pane_id) else {
            return Err(agent_not_found(id, &params.target));
        };
        if !super::super::agents::runtime_hosts_agent(runtime, expected_agent) {
            return Err(encode_error(
                id,
                "agent_not_ready",
                format!(
                    "agent {} is no longer the pane foreground process",
                    params.target
                ),
            ));
        }
        #[cfg(windows)]
        let submit_deadline = params
            .wait
            .as_ref()
            .and_then(|wait| wait.submission_deadline);
        #[cfg(not(windows))]
        let submit_deadline = None;
        if expected_agent == crate::detect::Agent::GithubCopilot {
            // Copilot ignores synthetic Enter after focus loss until it receives focus gained.
            let focus = match crate::ghostty::encode_focus(crate::ghostty::FocusEvent::Gained) {
                Ok(focus) => focus,
                Err(err) => {
                    return Err(encode_error(id, "agent_prompt_failed", err.to_string()));
                }
            };
            if let Err(err) = runtime.try_send_bytes(Bytes::from(focus)) {
                return Err(encode_error(id, "agent_prompt_failed", err.to_string()));
            }
        }
        let (text, enter) =
            crate::app::api_helpers::encode_api_submission_parts(runtime, &params.text);
        #[cfg(windows)]
        let text = if expected_agent == crate::detect::Agent::Codex {
            let mut text = text;
            append_codex_paste_boundary(runtime, &mut text);
            text
        } else {
            text
        };
        let Some(agent) = self.agent_info(resolved.ws_idx, resolved.pane_id) else {
            return Err(agent_not_found(id, &params.target));
        };
        let completion = runtime
            .queue_user_input_submission(
                Bytes::from(text),
                Bytes::from(enter),
                AGENT_PROMPT_SUBMIT_DELAY,
                submit_deadline,
            )
            .map_err(|err| encode_error(id.clone(), "agent_prompt_failed", err.to_string()))?;
        Ok((id, agent, completion))
    }

    pub(super) fn handle_agent_read(
        &mut self,
        id: String,
        params: crate::api::schema::AgentReadParams,
    ) -> String {
        let resolved = match self.resolve_agent_target(&params.target) {
            Ok(resolved) => resolved,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };
        let Some((pane, workspace_id)) = self.lookup_runtime(resolved.ws_idx, resolved.pane_id)
        else {
            return agent_not_found(id, &params.target);
        };
        let snapshot = crate::app::api_helpers::read_terminal_snapshot(
            pane,
            params.source,
            params.format,
            params.lines,
        );

        encode_success(
            id,
            ResponseResult::PaneRead {
                read: PaneReadResult {
                    pane_id: self
                        .public_pane_id(resolved.ws_idx, resolved.pane_id)
                        .unwrap_or_else(|| params.target.clone()),
                    workspace_id,
                    tab_id: self
                        .public_tab_id(resolved.ws_idx, resolved.tab_idx)
                        .unwrap(),
                    source: params.source,
                    format: params.format,
                    text: snapshot.text,
                    revision: 0,
                    truncated: snapshot.truncated,
                },
            },
        )
    }

    pub(super) fn handle_agent_explain(&mut self, id: String, target: AgentTarget) -> String {
        let resolved = match self.resolve_agent_target(&target.target) {
            Ok(resolved) => resolved,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };
        let Some((pane, _workspace_id)) = self.lookup_runtime(resolved.ws_idx, resolved.pane_id)
        else {
            return agent_not_found(id, &target.target);
        };
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(resolved.ws_idx)
            .and_then(|workspace| workspace.terminal_id(resolved.pane_id))
        else {
            return agent_not_found(id, &target.target);
        };
        let Some(terminal) = self.state.terminals.get(terminal_id) else {
            return agent_not_found(id, &target.target);
        };
        if terminal.full_lifecycle_hook_authority_active() {
            let explain = serde_json::json!({
                "agent": terminal.effective_agent_label().unwrap_or("unknown"),
                "state": crate::detect::manifest::agent_state_label(terminal.state),
                "manifest_source": null,
                "manifest_version": null,
                "cached_remote_version": null,
                "local_override_shadowing_remote": false,
                "remote_update_status": null,
                "remote_update_error": null,
                "matched_rule": null,
                "visible_idle": false,
                "visible_blocker": false,
                "visible_working": false,
                "screen_detection_skipped": true,
                "screen_detection_skip_reason": "full_lifecycle_hook_authority",
                "skip_state_update": false,
                "skipped_update_reason": null,
                "fallback_reason": null,
                "warning": null,
                "evaluated_rules": [],
            });
            return encode_success(id, ResponseResult::AgentExplain { explain });
        }
        let Some(agent) = terminal.effective_known_agent().or(terminal.detected_agent) else {
            return encode_error(
                id,
                "agent_explain_unavailable",
                format!(
                    "agent target {} does not have a detected agent label",
                    target.target
                ),
            );
        };

        let screen = pane.detection_text();
        let osc_title = pane.agent_osc_title();
        let osc_progress = pane.agent_osc_progress();
        let explain = crate::detect::manifest::explain_with_input(
            agent,
            crate::detect::manifest::DetectionInput {
                screen: &screen,
                osc_title: &osc_title,
                osc_progress: &osc_progress,
            },
        );
        let value = crate::detect::manifest::explain_to_json_value(&explain);

        encode_success(id, ResponseResult::AgentExplain { explain: value })
    }

    pub(super) fn handle_agent_send_keys(
        &mut self,
        id: String,
        params: AgentSendKeysParams,
    ) -> String {
        let resolved = match self.resolve_agent_target(&params.target) {
            Ok(resolved) => resolved,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(resolved.ws_idx)
            .and_then(|workspace| workspace.terminal_id(resolved.pane_id))
        else {
            return agent_not_found(id, &params.target);
        };
        let Some(expected_agent) = self
            .state
            .terminals
            .get(terminal_id)
            .and_then(|terminal| terminal.effective_known_agent())
        else {
            return agent_not_ready(id, &params.target);
        };
        let Some(runtime) = self.lookup_runtime_sender(resolved.ws_idx, resolved.pane_id) else {
            return agent_not_found(id, &params.target);
        };
        if !super::super::agents::runtime_hosts_agent(runtime, expected_agent) {
            return agent_not_ready(id, &params.target);
        }
        let encoded = match super::super::api_helpers::encode_api_keys(runtime, &params.keys) {
            Ok(encoded) => encoded,
            Err(key) => {
                return encode_error(id, "invalid_key", format!("unsupported key {key}"));
            }
        };
        let bytes: Vec<u8> = encoded.into_iter().flatten().collect();
        if let Err(err) = runtime.try_send_bytes(Bytes::from(bytes)) {
            return encode_error(id, "agent_send_keys_failed", err.to_string());
        }

        encode_success(id, ResponseResult::Ok {})
    }
}

fn agent_not_ready(id: String, target: &str) -> String {
    encode_error(
        id,
        "agent_not_ready",
        format!("agent {target} is not an active named agent"),
    )
}

fn agent_not_found(id: String, target: &str) -> String {
    encode_error(
        id,
        "agent_not_found",
        format!("agent target {target} not found"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::schema::{AgentStatus, SuccessResponse},
        app::Mode,
        config::Config,
        detect::{Agent, AgentState},
        workspace::Workspace,
    };

    fn app_with_agent() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.workspaces = vec![Workspace::test_new("agent")];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app
    }

    fn start_deferred_agent_prompt(
        app: &mut App,
        id: &str,
        params: AgentPromptParams,
    ) -> std::sync::mpsc::Receiver<String> {
        let (respond_to, response_rx) = std::sync::mpsc::channel();
        assert!(app.handle_deferred_agent_api_request(
            crate::api::schema::Request {
                id: id.into(),
                method: crate::api::schema::Method::AgentPrompt(params),
            },
            respond_to,
        ));
        response_rx
    }

    fn run_deferred_agent_prompt(app: &mut App, id: &str, params: AgentPromptParams) -> String {
        start_deferred_agent_prompt(app, id, params)
            .recv_timeout(Duration::from_secs(1))
            .expect("agent prompt responds after submission")
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_codex_prompt_flushes_paste_burst_before_enter() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::Codex), AgentState::Idle);
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);

        let response = run_deferred_agent_prompt(
            &mut app,
            "req",
            AgentPromptParams {
                target: "reviewer".into(),
                text: "A != B".into(),
                wait: None,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert!(matches!(
            success.result,
            ResponseResult::AgentPrompted { .. }
        ));
        // The non-character key must precede Enter so Codex commits the paste burst first.
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"A != B\x1b[C"));
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\r"));
    }

    #[tokio::test]
    async fn a_false_process_exit_makes_a_named_live_agent_unreachable_by_name() {
        // Reproduces the registration loss reported on #3225 by rszrszrsz:
        // a live agent pane with an assigned name stops resolving by that name
        // while its process keeps running, and renaming is the only recovery.
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let observed_at = std::time::Instant::now();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_detected_state(Some(Agent::Pi), AgentState::Working);
        terminal.set_agent_name("reviewer".into());

        let found = app.handle_agent_get(
            "req:before".into(),
            AgentTarget {
                target: "reviewer".into(),
            },
        );
        assert!(
            serde_json::from_str::<SuccessResponse>(&found).is_ok(),
            "the assigned name must resolve while the agent is running: {found}"
        );

        // One process-exit observation, then the same agent is observed alive
        // again on the next probe - the process never actually went away.
        app.handle_internal_event(crate::events::AppEvent::StateChanged {
            pane_id,
            agent: Some(Agent::Pi),
            state: AgentState::Idle,
            visible_blocker: false,
            visible_working: false,
            process_exited: true,
            observed_at,
        });
        app.handle_internal_event(crate::events::AppEvent::AgentProcessDetected {
            pane_id,
            agent: Agent::Pi,
            observed_at: observed_at + std::time::Duration::from_secs(1),
        });

        let terminal = &app.state.terminals[&terminal_id];
        assert_eq!(
            terminal.detected_agent,
            Some(Agent::Pi),
            "the agent process is still there"
        );

        let after = app.handle_agent_get(
            "req:after".into(),
            AgentTarget {
                target: "reviewer".into(),
            },
        );
        assert!(
            serde_json::from_str::<SuccessResponse>(&after).is_ok(),
            "a live agent must stay reachable by its assigned name: {after}"
        );
    }

    #[tokio::test]
    async fn agent_prompt_sends_text_then_delays_enter() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Working);
        let (runtime, mut rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80, 24, 0, b"", 2,
            );
        runtime.test_process_pty_bytes(b"\x1b[?2004h");
        app.state.insert_test_runtime(pane_id, runtime);

        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();
        let bracketed_started = std::time::Instant::now();
        let response_rx = start_deferred_agent_prompt(
            &mut app,
            "req",
            AgentPromptParams {
                target: public_pane_id,
                text: "A != B".into(),
                wait: None,
            },
        );
        assert!(response_rx.try_recv().is_err());
        let response = response_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("agent prompt responds after submission");
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::AgentPrompted { agent, .. } = success.result else {
            panic!("expected prompted response");
        };
        assert_eq!(agent.name.as_deref(), Some("reviewer"));
        assert_eq!(
            rx.try_recv().unwrap(),
            Bytes::from_static(b"\x1b[200~A != B\x1b[201~")
        );
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\r"));
        assert!(bracketed_started.elapsed() >= AGENT_PROMPT_SUBMIT_DELAY);

        app.lookup_runtime_sender(0, pane_id)
            .unwrap()
            .test_process_pty_bytes(b"\x1b[?2004l");
        let raw_started = std::time::Instant::now();
        let raw = run_deferred_agent_prompt(
            &mut app,
            "req-raw",
            AgentPromptParams {
                target: "reviewer".into(),
                text: "A != B".into(),
                wait: None,
            },
        );
        let raw: SuccessResponse = serde_json::from_str(&raw).unwrap();
        assert!(matches!(raw.result, ResponseResult::AgentPrompted { .. }));
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"A != B"));
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\r"));
        assert!(raw_started.elapsed() >= AGENT_PROMPT_SUBMIT_DELAY);

        let rejected = run_deferred_agent_prompt(
            &mut app,
            "req-label",
            AgentPromptParams {
                target: "opencode".into(),
                text: "wrong target".into(),
                wait: None,
            },
        );
        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&rejected).unwrap();
        assert_eq!(error.error.code, "agent_not_found");
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn agent_prompt_rejects_blocked_agent_without_writing() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::GithubCopilot), AgentState::Blocked);
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);

        let response = run_deferred_agent_prompt(
            &mut app,
            "req",
            AgentPromptParams {
                target: "reviewer".into(),
                text: "unrelated prompt".into(),
                wait: None,
            },
        );

        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "agent_blocked");
        assert!(
            tokio::time::timeout(
                AGENT_PROMPT_SUBMIT_DELAY + Duration::from_millis(100),
                rx.recv()
            )
            .await
            .is_err(),
            "blocked prompt wrote or scheduled terminal input"
        );
    }

    #[tokio::test]
    async fn agent_prompt_focuses_copilot_before_submitting() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::GithubCopilot), AgentState::Idle);
        let (runtime, mut rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80, 24, 0, b"", 3,
            );
        runtime.test_process_pty_bytes(b"\x1b[?2004h");
        app.state.insert_test_runtime(pane_id, runtime);

        let response = run_deferred_agent_prompt(
            &mut app,
            "req",
            AgentPromptParams {
                target: "reviewer".into(),
                text: "A != B".into(),
                wait: None,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert!(matches!(
            success.result,
            ResponseResult::AgentPrompted { .. }
        ));
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\x1b[I"));
        assert_eq!(
            rx.try_recv().unwrap(),
            Bytes::from_static(b"\x1b[200~A != B\x1b[201~")
        );
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\r"));
    }

    #[tokio::test]
    async fn agent_send_keys_validates_every_key_before_writing() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::Pi), AgentState::Idle);
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);

        let rejected = app.handle_agent_send_keys(
            "req-invalid".into(),
            AgentSendKeysParams {
                target: "reviewer".into(),
                keys: vec!["enter".into(), "not-a-key".into()],
            },
        );
        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&rejected).unwrap();
        assert_eq!(error.error.code, "invalid_key");
        assert!(rx.try_recv().is_err());

        let sent = app.handle_agent_send_keys(
            "req-valid".into(),
            AgentSendKeysParams {
                target: "reviewer".into(),
                keys: vec!["up".into(), "enter".into()],
            },
        );
        let success: SuccessResponse = serde_json::from_str(&sent).unwrap();
        assert!(matches!(success.result, ResponseResult::Ok {}));
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\x1b[A\r"));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn agent_prompt_rejects_managed_agent_while_startup_is_pending() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        let now = std::time::Instant::now();
        terminal.begin_managed_agent(
            "reviewer".into(),
            Agent::OpenCode,
            None,
            now,
            std::time::Duration::from_secs(3),
            std::time::Duration::from_secs(10),
        );
        terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Idle);
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);

        let response = run_deferred_agent_prompt(
            &mut app,
            "req-pending",
            AgentPromptParams {
                target: "reviewer".into(),
                text: "A != B".into(),
                wait: None,
            },
        );
        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "agent_not_ready");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn agent_focus_marks_already_focused_done_agent_seen() {
        let mut app = app_with_agent();
        app.state.outer_terminal_focus = Some(false);

        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(Agent::Pi), AgentState::Idle);
        app.state.workspaces[0].tabs[0]
            .panes
            .get_mut(&pane_id)
            .unwrap()
            .seen = false;
        app.state.workspaces[0].tabs[0].layout.focus_pane(pane_id);

        let response = app.handle_agent_focus(
            "req".into(),
            AgentTarget {
                target: app.public_pane_id(0, pane_id).unwrap(),
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::AgentInfo { agent } = success.result else {
            panic!("expected agent info response");
        };
        assert_eq!(agent.agent_status, AgentStatus::Idle);
    }

    #[test]
    fn agent_rename_does_not_replace_the_pane_label() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_manual_label("shell-pane".into());
        terminal.set_detected_state(Some(Agent::Pi), AgentState::Idle);
        let target = app.public_pane_id(0, pane_id).unwrap();

        for name in [Some("reviewer".to_string()), None] {
            let response = app.handle_agent_rename(
                "req".into(),
                AgentRenameParams {
                    target: target.clone(),
                    name,
                },
            );
            let success: SuccessResponse = serde_json::from_str(&response).unwrap();
            assert!(matches!(success.result, ResponseResult::AgentInfo { .. }));
            assert_eq!(
                app.state.terminals[&terminal_id].manual_label.as_deref(),
                Some("shell-pane")
            );
        }
    }
}
