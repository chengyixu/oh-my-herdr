//! Activation of a launch-owned native context bridge, separate from UI state.
use super::App;
use crate::integration::native_context::{NativeContextError, NativeContextRegistration};
use crate::terminal::TerminalId;

impl App {
    pub(crate) fn ensure_managed_native_context(
        &mut self,
        terminal: &TerminalId,
        pane: &str,
    ) -> Result<Option<NativeContextRegistration>, NativeContextError> {
        let error = |message: &str| NativeContextError {
            code: "native_context_unavailable".into(),
            message: message.into(),
        };
        if let Some(context) = self
            .state
            .terminals
            .get(terminal)
            .and_then(|t| t.native_context.clone())
        {
            let current = self
                .parse_current_public_pane_id(pane)
                .and_then(|(ws, p)| self.agent_info(ws, p))
                .and_then(|a| a.agent_session);
            if current
                .as_ref()
                .is_none_or(|session| context.matches_session(session))
            {
                return Ok(Some(context));
            }
            if let Some(state) = self.state.terminals.get_mut(terminal) {
                state.native_context = None;
            }
        }
        #[cfg(unix)]
        {
            if !self.managed_sources.contains_key(terminal) {
                return Ok(None);
            }
            let (ws, pid) = self
                .parse_current_public_pane_id(pane)
                .ok_or_else(|| error("source pane changed"))?;
            let Some(info) = self.agent_info(ws, pid) else {
                return Ok(None);
            };
            if !crate::api::agent_accepts_handoff_input(&info) {
                return Ok(None);
            }
            let Some(session) = info.agent_session else {
                return Ok(None);
            };
            let Some(harness) = crate::harness::find(&session.agent) else {
                return Ok(None);
            };
            let Some(job) = self
                .terminal_runtimes
                .get(terminal)
                .and_then(|r| r.child_pid())
                .and_then(crate::detect::foreground_job)
            else {
                return Ok(None);
            };
            let owners: Vec<_> = job
                .processes
                .iter()
                .filter(|p| crate::detect::parse_agent_label(&p.name) == Some(harness.agent))
                .collect();
            if owners.len() != 1 {
                return Err(error("native source process ownership is ambiguous"));
            }
            let cwd = self
                .state
                .terminals
                .get(terminal)
                .ok_or_else(|| error("source terminal changed"))?
                .cwd
                .clone();
            let path = crate::integration::session_history::exact_source_path(
                &session.agent,
                &session.value,
                &cwd,
            )
            .map_err(|_| error("the exact reported native session file could not be resolved"))?;
            let baseline = self
                .state
                .terminals
                .get(terminal)
                .and_then(|t| t.native_context_input_baseline);
            let lease = self
                .managed_sources
                .get_mut(terminal)
                .ok_or_else(|| error("source launch lease disappeared"))?;
            let context = lease
                .activate(
                    &session.agent,
                    pane,
                    &session.value,
                    &path,
                    owners[0].pid,
                    baseline,
                )
                .map_err(|_| error("source service activation failed"))?;
            context
                .validate_endpoint()
                .map_err(|_| error("native source service is unavailable"))?;
            if let Some(t) = self.state.terminals.get_mut(terminal) {
                t.native_context = Some(context.clone());
            }
            Ok(Some(context))
        }
        #[cfg(not(unix))]
        {
            let _ = (terminal, pane, error);
            Ok(None)
        }
    }
}
