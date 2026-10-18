//! The client requests one server-owned, same-pane native replacement.
//! Native conversation import and process transitions never run in the UI.
use super::*;
use crate::api::schema::*;

impl ClientShellState {
    pub(super) fn begin_live_harness_switch(
        &mut self,
        instance_id: String,
        target: Option<String>,
        outcome: &mut ClientShellInput,
    ) {
        if self.live_harness_switch.is_some() {
            self.set_endpoint_error("A harness switch is already in progress");
            outcome.repaint = true;
            return;
        }
        if target.as_deref().is_some_and(|id| {
            !self
                .active_harness_choices()
                .iter()
                .any(|active| active == id)
        }) {
            self.set_endpoint_error("Activate and install the target integration first");
            outcome.repaint = true;
            return;
        }
        let method = Method::AgentHarnessReplace(AgentHarnessReplaceParams {
            instance_id: instance_id.clone(),
            harness: target.clone(),
        });
        if !self.supports_endpoint_method(&method) {
            let message="The connected server does not support same-pane native history replacement (agent.harness.replace). Update its runtime before switching; no pane or session was changed";
            self.set_endpoint_error(message);
            self.push_endpoint_notice(
                ClientEndpointNoticeKind::Unsupported,
                "agent.harness.replace",
                "Same-pane switch unavailable",
                message,
            );
            outcome.repaint = true;
            return;
        }
        self.live_harness_switch = Some(LiveHarnessSwitch {
            instance_id: instance_id.clone(),
            target,
            endpoint_id: self.active_endpoint_id.clone(),
            old_pane: None,
        });
        if !self.push_endpoint_method_with_kind(
            method,
            PendingEndpointKind::HarnessSwitch {
                instance_id,
                step: HarnessSwitchStep::Replace,
            },
            outcome,
        ) {
            self.live_harness_switch = None;
        }
    }

    pub(super) fn handle_live_harness_switch_result(
        &mut self,
        instance_id: String,
        _step: HarnessSwitchStep,
        result: Result<ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        let Some(_plan) = self.live_harness_switch.take().filter(|plan| {
            plan.instance_id == instance_id && plan.endpoint_id == self.active_endpoint_id
        }) else {
            return (false, Vec::new());
        };
        match result {
            Ok(ResponseResult::AgentHarnessReplaced {..})=>{
                self.endpoint_error=None;
                self.endpoint_error_deadline=None;
            },
            Ok(_)=>self.set_endpoint_error("The server returned an unexpected native replacement result; inspect the original pane before retrying"),
            Err(error)=>self.set_endpoint_error(format!("Harness switch stopped: {}",error.message)),
        }
        (true, Vec::new())
    }
}
