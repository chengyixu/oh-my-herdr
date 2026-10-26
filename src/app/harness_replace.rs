//! Server-owned replacement of a harness within an existing pane.
//! Source context exchange is owned by the native integration, not screen reads.
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use super::api::responses::{encode_error, encode_success};
use super::App;
use crate::agent_resume::{AgentSessionRef, PersistedAgentSession};
use crate::api::schema::*;
use crate::integration::native_context::{NativeContextError, NativeContextRegistration};
use crate::integration::session_history::ImportedHistory;
use crate::terminal::TerminalId;

struct Prepared {
    snapshot_id: String,
    imported: ImportedHistory,
}
enum Stage {
    Preparing(Receiver<Result<Prepared, NativeContextError>>),
    Closing(Receiver<Result<(), NativeContextError>>),
    SourceShell,
    TargetReady,
}
pub(crate) struct PendingReplacement {
    id: String,
    response: Sender<String>,
    instance: String,
    name: String,
    role: String,
    pane: String,
    terminal: TerminalId,
    shell_pid: u32,
    source: PersistedAgentSession,
    target: Option<PersistedAgentSession>,
    source_export: NativeContextRegistration,
    prepared: Option<Prepared>,
    stage: Stage,
    deadline: Instant,
    args: Vec<String>,
    binding: Option<(String, String)>,
}

fn exchange_error(code: &str, message: impl Into<String>) -> NativeContextError {
    NativeContextError {
        code: code.into(),
        message: message.into(),
    }
}

impl App {
    pub(crate) fn replacement_pauses_resolved_pane_input(
        &self,
        ws: usize,
        pane: crate::layout::PaneId,
    ) -> bool {
        self.state
            .workspaces
            .get(ws)
            .and_then(|workspace| workspace.terminal_id(pane))
            .and_then(|id| self.state.terminals.get(id))
            .is_some_and(|terminal| terminal.native_replacement_input_paused)
    }
    pub(crate) fn replacement_pauses_pane_input(&self, pane: &str) -> bool {
        self.parse_pane_id(pane)
            .is_some_and(|(ws, pane)| self.replacement_pauses_resolved_pane_input(ws, pane))
    }
    pub(crate) fn replacement_pauses_terminal_input(&self, terminal: &str) -> bool {
        self.state
            .terminals
            .get(terminal)
            .is_some_and(|terminal| terminal.native_replacement_input_paused)
    }
    pub(crate) fn replacement_pauses_api_input(&self, method: &Method) -> bool {
        let target = match method {
            Method::PaneSendText(params) => Some(params.pane_id.as_str()),
            Method::PaneSendKeys(params) => Some(params.pane_id.as_str()),
            Method::PaneSendInput(params) => Some(params.pane_id.as_str()),
            Method::AgentStart(params) => Some(params.pane_id.as_str()),
            Method::AgentPrompt(params) => Some(params.target.as_str()),
            Method::AgentSendKeys(params) => Some(params.target.as_str()),
            Method::AgentHarnessPrompt(params) => Some(params.pane_id.as_str()),
            Method::AgentBroadcast(params) => {
                return self
                    .agent_registry
                    .roster
                    .values()
                    .filter(|entry| entry.role == params.role)
                    .filter_map(|entry| entry.last_pane.as_deref())
                    .any(|pane| self.replacement_pauses_pane_input(pane))
            }
            _ => None,
        };
        target.is_some_and(|target| {
            self.replacement_pauses_pane_input(target)
                || self
                    .resolve_agent_target(target)
                    .ok()
                    .and_then(|target| {
                        self.state.workspaces[target.ws_idx].terminal_id(target.pane_id)
                    })
                    .and_then(|id| self.state.terminals.get(id))
                    .is_some_and(|terminal| terminal.native_replacement_input_paused)
        })
    }

    pub(crate) fn handle_agent_context_register(
        &mut self,
        id: String,
        params: AgentContextRegisterParams,
    ) -> String {
        let Some(harness) = crate::harness::find(&params.agent) else {
            return encode_error(id, "invalid_agent_kind", "unknown native integration");
        };
        if harness.native_context_version() != Some(params.version)
            || !crate::agent_resume::is_official_agent_source(&params.source, &params.agent)
        {
            return encode_error(
                id,
                "unsupported_native_context",
                "this integration's native context protocol is not supported",
            );
        }
        let Some((ws, pane)) = self.parse_pane_id(&params.pane_id) else {
            return encode_error(id, "pane_not_found", "native context pane was not found");
        };
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(ws)
            .and_then(|workspace| workspace.terminal_id(pane))
            .cloned()
        else {
            return encode_error(
                id,
                "pane_not_found",
                "native context terminal was not found",
            );
        };
        if !self.state.terminals.contains_key(&terminal_id) {
            return encode_error(
                id,
                "pane_not_found",
                "native context terminal was not found",
            );
        }
        let owns_session = self
            .agent_info(ws, pane)
            .and_then(|agent| agent.agent_session)
            .is_some_and(|session| {
                session.source == params.source
                    && session.agent == params.agent
                    && match session.kind {
                        crate::agent_resume::AgentSessionRefKind::Id => {
                            session.value == params.session_id
                        }
                        crate::agent_resume::AgentSessionRefKind::Path => {
                            session.value == params.session_path
                        }
                    }
            });
        let runtime = self.terminal_runtimes.get(&terminal_id);
        let owns_process = runtime.is_some_and(|runtime| {
            super::agents::runtime_hosts_agent(runtime, harness.agent)
                && runtime
                    .child_pid()
                    .and_then(crate::detect::foreground_job)
                    .is_some_and(|job| {
                        job.processes
                            .iter()
                            .any(|process| process.pid == params.process_id)
                    })
        });
        if !owns_session || !owns_process {
            return encode_error(
                id,
                "native_context_not_owned",
                "the reporter does not own this pane's current native session and process",
            );
        }
        if let Err(error) = params.validate_endpoint() {
            return encode_error(id, "invalid_native_context_endpoint", error.to_string());
        }
        if let Some(terminal) = self.state.terminals.get_mut(&terminal_id) {
            if terminal.native_context.as_ref() == Some(&params) {
                return encode_success(id, ResponseResult::Ok {});
            }
            if terminal.native_context.as_ref().is_some_and(|previous| {
                previous.process_id == params.process_id && previous.seq >= params.seq
            }) {
                return encode_error(
                    id,
                    "native_context_superseded",
                    "a newer context registration already owns this native process",
                );
            }
            let baseline = runtime.map(|r| r.native_input_evidence());
            terminal.native_context_input_baseline = baseline;
            terminal.native_context = Some(params);
        }
        encode_success(id, ResponseResult::Ok {})
    }

    pub(crate) fn handle_agent_context_control(
        &mut self,
        id: String,
        params: crate::api::schema::agents::AgentContextControlParams,
    ) -> String {
        let (ws, pane) = match self.parse_current_public_pane_id(&params.context.pane_id) {
            Some(pair) => pair,
            None => return encode_error(id, "pane_not_found", "native source pane was not found"),
        };
        let terminal_id = match self
            .state
            .workspaces
            .get(ws)
            .and_then(|workspace| workspace.terminal_id(pane))
            .cloned()
        {
            Some(t) => t,
            None => {
                return encode_error(id, "pane_not_found", "native source terminal was not found")
            }
        };
        let terminal = match self.state.terminals.get(&terminal_id) {
            Some(t) => t,
            None => {
                return encode_error(id, "pane_not_found", "native source terminal was not found")
            }
        };
        if terminal.native_context.as_ref() != Some(&params.context) {
            return encode_error(
                id,
                "native_context_not_owned",
                "the controller does not own this pane's current native context",
            );
        }
        let runtime = match self.terminal_runtimes.get(&terminal_id) {
            Some(r) => r,
            None => return encode_error(id, "pane_not_found", "native source runtime disappeared"),
        };
        let harness = match crate::harness::find(&params.context.agent) {
            Some(h) => h,
            None => return encode_error(id, "invalid_agent_kind", "unknown native integration"),
        };

        match params.action {
            crate::api::schema::agents::AgentContextControlAction::Inspect => {
                let baseline = terminal
                    .native_context_input_baseline
                    .unwrap_or_else(crate::pane::native_input::NativeInputEvidence::default);
                let current = runtime.native_input_evidence();
                let queue_unchanged = current.queue_proven_unchanged_since(&baseline);
                let output_seq = runtime.content_seq();
                encode_success(
                    id,
                    crate::api::schema::response::ResponseResult::NativeContextControl {
                        input: current,
                        queue_unchanged,
                        output_seq,
                    },
                )
            }
            crate::api::schema::agents::AgentContextControlAction::OpenEditor => {
                let Some(controls) = harness.native_editor_controls() else {
                    return encode_error(
                        id,
                        "unsupported_native_control",
                        "this source has no qualified editor/exit controls",
                    );
                };
                let _ = runtime.try_send_bytes(bytes::Bytes::from_static(controls.0.as_bytes()));
                encode_success(id, ResponseResult::Ok {})
            }
            crate::api::schema::agents::AgentContextControlAction::Exit => {
                let Some(controls) = harness.native_editor_controls() else {
                    return encode_error(
                        id,
                        "unsupported_native_control",
                        "this source has no qualified editor/exit controls",
                    );
                };
                let _ = runtime.try_send_bytes(bytes::Bytes::from_static(controls.1.as_bytes()));
                encode_success(id, ResponseResult::Ok {})
            }
        }
    }

    pub(crate) fn begin_harness_replacement(
        &mut self,
        id: String,
        params: AgentHarnessReplaceParams,
        response: Sender<String>,
    ) -> bool {
        let plan = (|| -> Result<PendingReplacement, NativeContextError> {
            self.refresh_agent_registry_for_read()
                .map_err(|error| exchange_error("agent_profile_load_failed", error.to_string()))?;
            let entry = self
                .agent_registry
                .roster
                .get(&params.instance_id)
                .ok_or_else(|| {
                    exchange_error(
                        "agent_switch_stale",
                        "the selected native instance disappeared",
                    )
                })?
                .clone();
            let pane = entry
                .last_pane
                .as_deref()
                .ok_or_else(|| {
                    exchange_error(
                        "agent_switch_stale",
                        "the selected instance has no live pane",
                    )
                })?
                .to_string();
            let (ws, pane_id) = self
                .parse_current_public_pane_id(&pane)
                .ok_or_else(|| exchange_error("agent_switch_stale", "the selected pane changed"))?;
            let terminal_id = self.state.workspaces[ws]
                .terminal_id(pane_id)
                .cloned()
                .ok_or_else(|| {
                    exchange_error("agent_switch_stale", "the selected terminal changed")
                })?;
            if self
                .pending_harness_replacements
                .iter()
                .any(|pending| pending.terminal == terminal_id)
            {
                return Err(exchange_error(
                    "agent_switch_busy",
                    "this pane already has a harness replacement in progress",
                ));
            }
            let terminal = self.state.terminals.get(&terminal_id).ok_or_else(|| {
                exchange_error("agent_switch_stale", "the selected terminal disappeared")
            })?;
            if terminal.live_roster_instance_id() != Some(params.instance_id.as_str()) {
                return Err(exchange_error(
                    "agent_switch_stale",
                    "the exact native instance no longer owns this pane",
                ));
            }
            let info = self.agent_info(ws, pane_id).ok_or_else(|| {
                exchange_error("agent_switch_stale", "the selected agent disappeared")
            })?;
            if !crate::api::agent_accepts_handoff_input(&info) {
                return Err(exchange_error(
                    "source_busy",
                    "finish the current turn and native confirmations before switching",
                ));
            }
            let profile = self.agent_registry.get(&entry.role).ok_or_else(|| {
                exchange_error("agent_profile_not_found", "saved profile disappeared")
            })?;
            let target = params.harness.as_deref().unwrap_or(&profile.harness);
            if info.agent.as_deref() == Some(target) {
                return Err(exchange_error(
                    "same_harness",
                    "this pane already runs the selected integration",
                ));
            }
            let harness = crate::harness::find(target).ok_or_else(|| {
                exchange_error("invalid_agent_kind", "unknown target integration")
            })?;
            if !harness.available() || !harness.active(&self.agent_registry, false) {
                return Err(exchange_error(
                    "inactive_integration",
                    "install and activate the target integration first",
                ));
            }
            let export=terminal.native_context.clone().ok_or_else(||exchange_error("native_context_unavailable","the running integration cannot export its exact native context. Install the updated integration and reload it in this same native session before switching. No pane or session was changed"))?;
            let shell_pid = self
                .terminal_runtimes
                .get(&terminal_id)
                .and_then(|runtime| runtime.child_pid())
                .ok_or_else(|| {
                    exchange_error("agent_switch_stale", "the original shell is unavailable")
                })?;
            let has_parent_shell = self
                .terminal_runtimes
                .get(&terminal_id)
                .and_then(|runtime| runtime.child_pid())
                .and_then(crate::detect::foreground_group_leader_job)
                .is_some_and(|job| {
                    job.processes.iter().any(|process| {
                        process.pid == job.process_group_id
                            && crate::platform::is_pane_shell_process_name(&process.name)
                    })
                });
            if !has_parent_shell {
                return Err(exchange_error("native_replace_requires_shell","this native process replaced its shell; it cannot be closed without destroying the pane. The original was not changed"));
            }
            let source_format = crate::harness::find(&export.agent)
                .and_then(|source| source.native_context_format());
            if !source_format.is_some_and(|format| harness.context_import_from(format)) {
                return Err(exchange_error("native_import_unsupported","this integration pair has no qualified complete-history import. The original pane and session were not changed"));
            }
            let source = info
                .agent_session
                .as_ref()
                .and_then(|session| {
                    crate::agent_resume::session_ref_from_snapshot(
                        &session.source,
                        &session.agent,
                        session.kind,
                        &session.value,
                    )
                })
                .ok_or_else(|| {
                    exchange_error(
                        "native_context_unavailable",
                        "the original native session identity was not captured",
                    )
                })?;
            let export_for_worker = export.clone();
            let cwd = terminal.cwd.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("native-history-import".into())
                .spawn(move || {
                    let result = (|| {
                        let (snapshot_id, snapshot) = export_for_worker.snapshot()?;
                        if snapshot.cwd.canonicalize().ok() != cwd.canonicalize().ok()
                            || snapshot.cwd.canonicalize().is_err()
                        {
                            return Err(exchange_error(
                                "source_changed",
                                "the native working directory changed before import",
                            ));
                        }
                        let imported =
                            harness.import_native_context(&snapshot).map_err(|error| {
                                exchange_error("native_import_failed", error.to_string())
                            })?;
                        Ok(Prepared {
                            snapshot_id,
                            imported,
                        })
                    })();
                    let _ = tx.send(result);
                })
                .map_err(|error| exchange_error("native_import_failed", error.to_string()))?;
            Ok(PendingReplacement {
                id: id.clone(),
                response: response.clone(),
                instance: params.instance_id,
                name: entry.display_name,
                role: entry.role,
                pane,
                terminal: terminal_id,
                shell_pid,
                source,
                target: None,
                source_export: export,
                prepared: None,
                stage: Stage::Preparing(rx),
                deadline: Instant::now() + Duration::from_secs(30),
                args: Vec::new(),
                binding: None,
            })
        })();
        match plan {
            Ok(plan) => {
                if let Some(terminal) = self.state.terminals.get_mut(&plan.terminal) {
                    terminal.native_replacement_input_paused = true;
                }
                self.pending_harness_replacements.push(plan);
                self.harness_replace_deadline = Some(Instant::now());
            }
            Err(error) => {
                let _ = response.send(encode_error(id, &error.code, error.message));
            }
        }
        true
    }

    pub(crate) fn poll_harness_replacements(&mut self, now: Instant) -> bool {
        if self
            .harness_replace_deadline
            .is_none_or(|deadline| now < deadline)
        {
            return false;
        }
        let pending = std::mem::take(&mut self.pending_harness_replacements);
        let mut changed = false;
        for mut plan in pending {
            let result = self.advance_harness_replacement(&mut plan, now);
            match result {
                Ok(Some(result)) => {
                    if let Some(terminal) = self.state.terminals.get_mut(&plan.terminal) {
                        terminal.native_replacement_input_paused = false;
                    }
                    let _ = plan.response.send(encode_success(plan.id, result));
                    changed = true;
                }
                Ok(None) => self.pending_harness_replacements.push(plan),
                Err(error) => {
                    // Never send exit keys to a target which failed or needs input.
                    // If the original shell is available, recover the exact source
                    // snapshot; otherwise retain both native files and surface error.
                    if !matches!(plan.stage, Stage::Preparing(_))
                        && self
                            .terminal_runtimes
                            .get(&plan.terminal)
                            .and_then(|runtime| runtime.child_pid())
                            == Some(plan.shell_pid)
                        && self.replacement_shell_available(&plan.terminal)
                    {
                        if let Ok((args, binding)) =
                            self.replacement_launch_arguments(&plan.role, &plan.source)
                        {
                            let _ = self.start_replacement_in_shell(
                                &plan.pane,
                                &plan.instance,
                                &plan.name,
                                plan.source.clone(),
                                &args,
                                binding,
                            );
                        }
                    }
                    if let Some(terminal) = self.state.terminals.get_mut(&plan.terminal) {
                        terminal.native_replacement_recovery_session = None;
                        terminal.native_replacement_input_paused = false;
                    }
                    let _ = plan
                        .response
                        .send(encode_error(plan.id, &error.code, error.message));
                    changed = true;
                }
            }
        }
        self.harness_replace_deadline = (!self.pending_harness_replacements.is_empty())
            .then(|| now + Duration::from_millis(50));
        changed
    }

    fn advance_harness_replacement(
        &mut self,
        plan: &mut PendingReplacement,
        now: Instant,
    ) -> Result<Option<ResponseResult>, NativeContextError> {
        let (ws, pane) = self
            .parse_current_public_pane_id(&plan.pane)
            .ok_or_else(|| exchange_error("agent_switch_stale", "the original pane disappeared"))?;
        if self.state.workspaces[ws].terminal_id(pane) != Some(&plan.terminal) {
            return Err(exchange_error(
                "agent_switch_stale",
                "the original terminal binding changed",
            ));
        }
        if self
            .terminal_runtimes
            .get(&plan.terminal)
            .and_then(|runtime| runtime.child_pid())
            != Some(plan.shell_pid)
        {
            return Err(exchange_error(
                "agent_switch_stale",
                "the original shell was replaced; no new process was started there",
            ));
        }
        if now >= plan.deadline {
            return Err(exchange_error("native_switch_timeout","the native switch did not reach a ready prompt. Its source history is saved; inspect the same pane before retrying"));
        }
        match &mut plan.stage {
            Stage::Preparing(rx) => {
                let mut prepared = match rx.try_recv() {
                    Ok(result) => result?,
                    Err(TryRecvError::Empty) => return Ok(None),
                    Err(_) => {
                        return Err(exchange_error(
                            "native_import_failed",
                            "the native importer stopped; original unchanged",
                        ))
                    }
                };
                let terminal = self.state.terminals.get(&plan.terminal).ok_or_else(|| {
                    exchange_error("agent_switch_stale", "source terminal disappeared")
                })?;
                if terminal.native_context.as_ref() != Some(&plan.source_export)
                    || terminal.live_roster_instance_id() != Some(plan.instance.as_str())
                {
                    return Err(exchange_error(
                        "source_changed",
                        "the original native session changed during import; nothing was closed",
                    ));
                }
                let target = prepared.imported.native_session.clone();
                let (args, binding) = self
                    .replacement_launch_arguments(&plan.role, &target)
                    .map_err(|error| exchange_error("native_launch_failed", error))?;
                let recovery = crate::agent_registry::NativeHarnessRecovery {
                    source: plan.source.clone(),
                    target: target.clone(),
                    source_archive: prepared.imported.source_archive.clone(),
                    source_context_archive: prepared.imported.source_context_archive.clone(),
                    target_path: prepared.imported.path.clone(),
                    phase: "prepared".into(),
                };
                self.update_agent_registry(|registry| {
                    registry
                        .native_harness_recovery
                        .insert(plan.instance.clone(), recovery);
                })
                .map_err(|error| {
                    exchange_error("agent_profile_persist_failed", error.to_string())
                })?;
                prepared.imported.retain_for_recovery();
                // Recover the exact selected branch, even if the source file later
                // moved its active leaf. The original source file remains intact.
                plan.source.session_ref = AgentSessionRef::path(
                    prepared
                        .imported
                        .source_archive
                        .to_string_lossy()
                        .into_owned(),
                )
                .ok_or_else(|| {
                    exchange_error("native_import_failed", "invalid source recovery path")
                })?;
                if let Some(terminal) = self.state.terminals.get_mut(&plan.terminal) {
                    terminal.native_replacement_recovery_session = Some(plan.source.clone());
                }
                self.state.mark_session_dirty();
                self.schedule_session_save();
                plan.target = Some(target);
                plan.args = args;
                plan.binding = binding;
                plan.prepared = Some(prepared);
                let export = plan.source_export.clone();
                let snapshot = plan
                    .prepared
                    .as_ref()
                    .map(|prepared| prepared.snapshot_id.clone())
                    .ok_or_else(|| {
                        exchange_error("native_import_failed", "missing context checkpoint")
                    })?;
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::Builder::new()
                    .name("native-context-close".into())
                    .spawn(move || {
                        let _ = tx.send(export.close_if_unchanged(&snapshot));
                    })
                    .map_err(|error| exchange_error("native_close_failed", error.to_string()))?;
                plan.stage = Stage::Closing(rx);
                plan.deadline = now + Duration::from_secs(20);
            }
            Stage::Closing(rx) => {
                match rx.try_recv() {
                    Ok(result) => result?,
                    Err(TryRecvError::Empty) => return Ok(None),
                    Err(_) => {
                        return Err(exchange_error(
                            "native_close_failed",
                            "native close acknowledgement was lost; inspect the original pane",
                        ))
                    }
                };
                plan.stage = Stage::SourceShell;
            }
            Stage::SourceShell => {
                if !self.replacement_shell_available(&plan.terminal) {
                    return Ok(None);
                }
                let target = plan.target.clone().ok_or_else(|| {
                    exchange_error("native_launch_failed", "target identity missing")
                })?;
                self.start_replacement_in_shell(
                    &plan.pane,
                    &plan.instance,
                    &plan.name,
                    target,
                    &plan.args,
                    plan.binding.clone(),
                )
                .map_err(|error| exchange_error("native_launch_failed", error))?;
                plan.stage = Stage::TargetReady;
                plan.deadline = now + Duration::from_secs(35);
            }
            Stage::TargetReady => {
                let target = plan.target.as_ref().ok_or_else(|| {
                    exchange_error("native_launch_failed", "target identity missing")
                })?;
                let Some(agent) = self.agent_info(ws, pane) else {
                    return Ok(None);
                };
                if self
                    .state
                    .terminals
                    .get(&plan.terminal)
                    .and_then(|terminal| terminal.live_roster_instance_id())
                    != Some(plan.instance.as_str())
                    || agent.agent.as_deref() != Some(&target.agent)
                    || agent
                        .agent_session
                        .as_ref()
                        .is_none_or(|session| session.value != target.session_ref.value)
                    || !crate::api::agent_accepts_handoff_input(&agent)
                {
                    return Ok(None);
                }
                self.update_agent_registry(|registry| {
                    if let Some(entry) = registry.roster.get_mut(&plan.instance) {
                        entry.harness_override = Some(target.agent.clone());
                        entry.last_pane = Some(plan.pane.clone());
                        entry.status = crate::agent_registry::AgentStatus::Idle;
                    }
                    if let Some(recovery) = registry.native_harness_recovery.get_mut(&plan.instance)
                    {
                        recovery.phase = "committed".into();
                    }
                })
                .map_err(|error| {
                    exchange_error("agent_profile_persist_failed", error.to_string())
                })?;
                if let Some(terminal) = self.state.terminals.get_mut(&plan.terminal) {
                    terminal.native_replacement_recovery_session = None;
                }
                let pane = self.pane_info(ws, pane).ok_or_else(|| {
                    exchange_error("agent_switch_stale", "original pane disappeared")
                })?;
                let imported = plan.prepared.as_ref().ok_or_else(|| {
                    exchange_error("native_import_failed", "missing import checkpoint")
                })?;
                self.state.mark_session_dirty();
                self.schedule_session_save();
                return Ok(Some(ResponseResult::AgentHarnessReplaced {
                    pane,
                    source_session: plan.source_export.session_id.clone(),
                    target_session: target.session_ref.value.clone(),
                    messages: imported.imported.messages,
                }));
            }
        }
        Ok(None)
    }
}
