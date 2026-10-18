use super::*;

impl HeadlessServer {
    pub(super) fn handle_client_shell_endpoint_request(
        &mut self,
        client_id: u64,
        boot_id: String,
        mut request: Box<api::schema::Request>,
    ) -> bool {
        let Some(client) = self.clients.get(&client_id) else {
            return false;
        };
        if !matches!(client.mode, ClientConnectionMode::ClientShell) {
            self.remove_client_and_resize_if_needed(client_id);
            return true;
        }
        let request_id = request.id.clone();
        if !crate::server::client_commands::supports_client_shell_method(&request.method) {
            let message = crate::server::client_commands::error_message(
                boot_id,
                request_id,
                "unsupported_endpoint_command",
                "this method is not available through the client shell command lane",
            );
            self.send_to_client(client_id, message);
            return false;
        }
        if boot_id != self.client_shell_boot_id {
            let message = crate::server::client_commands::error_message(
                boot_id,
                request_id,
                "stale_boot",
                "endpoint command targeted an earlier server boot",
            );
            self.send_to_client(client_id, message);
            return false;
        }
        // Pre-native clients use this method as the first context-transfer step
        // before spawning a replica. Keep its public JSON read contract, but stop
        // that retired client flow before it can create another pane.
        if matches!(request.method, api::schema::Method::AgentHarnessContext(_)) {
            self.send_to_client(client_id,crate::server::client_commands::error_message(boot_id,request_id,"legacy_harness_client_upgrade_required","Reopen the updated Ohmyherdr client to switch in the same pane. This older client uses the retired split-pane handoff; no new pane was created"));
            return false;
        }
        let surface_active = client.shell_surface_active;
        if let api::schema::Method::ClientShellSurfaceSet(params) = &request.method {
            let Some((changed, projection_revision)) =
                self.set_client_shell_surface_active(client_id, params.active)
            else {
                return false;
            };
            self.send_to_client(
                client_id,
                crate::server::client_commands::success_message_with_result(
                    boot_id,
                    request_id,
                    api::schema::ResponseResult::ClientShellSurfaceSet {
                        active: params.active,
                        projection_revision,
                    },
                ),
            );
            return changed;
        }
        if client.shell_endpoint_command_in_flight {
            let message = crate::server::client_commands::error_message(
                boot_id,
                request_id,
                "endpoint_busy",
                "this endpoint is still processing another command",
            );
            self.send_to_client(client_id, message);
            return false;
        }
        if !surface_active {
            let message = crate::server::client_commands::error_message(
                boot_id,
                request_id,
                "surface_inactive",
                "this method requires an active client shell surface",
            );
            self.send_to_client(client_id, message);
            return false;
        }

        let api_request_id = format!(
            "endpoint:{}:{client_id}:{request_id}",
            self.client_shell_boot_id
        );
        request.id = api_request_id.clone();
        let (respond_to, response_rx) = std::sync::mpsc::channel();
        if let Err(err) = crate::server::client_commands::spawn_response_waiter(
            client_id,
            boot_id.clone(),
            request_id.clone(),
            response_rx,
            self.server_event_tx.clone(),
        ) {
            let message = crate::server::client_commands::error_message(
                boot_id,
                request_id,
                "server_unavailable",
                format!("failed to start endpoint response bridge: {err}"),
            );
            self.send_to_client(client_id, message);
            return false;
        }
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.shell_endpoint_command_in_flight = true;
            // A later source restore has a new projection revision. Keep this request's lease
            // so a delayed worktree response cannot focus a pane after endpoint switching.
            client.shell_endpoint_command_surface_revision = Some(client.shell_projection_revision);
            let deferred_worktree = matches!(
                &request.method,
                api::schema::Method::WorktreeCreate(_)
                    | api::schema::Method::WorktreeRemove(_)
                    | api::schema::Method::WorktreeList(_)
                    | api::schema::Method::WorktreeOpen(_)
            );
            let deferred_navigation = match &request.method {
                api::schema::Method::WorktreeCreate(params) => params.focus,
                api::schema::Method::WorktreeOpen(params) => params.focus,
                _ => false,
            };
            client.shell_deferred_navigation_request_id =
                deferred_worktree.then(|| api_request_id.clone());
            client.shell_deferred_navigation_response = deferred_navigation.then(Vec::new);
        }
        if let api::schema::Method::AgentReadyWait(params) = &request.method {
            let Some(api_tx) = self.api_tx.clone() else {
                let _ = respond_to.send(crate::api::error_response_json(
                    api_request_id,
                    "server_unavailable",
                    "agent readiness bridge is unavailable".into(),
                ));
                return false;
            };
            let params = params.clone();
            let event_hub = self.app.event_hub.clone();
            let should_quit = self.should_quit.clone();
            let cancel = Arc::new(AtomicBool::new(false));
            if let Some(client) = self.clients.get_mut(&client_id) {
                client.shell_endpoint_wait_cancel = Some(cancel.clone());
            }
            let error_id = api_request_id.clone();
            let error_sender = respond_to.clone();
            if let Err(err) =
                std::thread::Builder::new()
                    .name("endpoint-agent-ready".into())
                    .spawn(move || {
                        let response = api::wait_for_agent_ready_with_cancel(
                            api_request_id.clone(),
                            params,
                            &api_tx,
                            &event_hub,
                            || {
                                Ok(cancel.load(Ordering::Acquire)
                                    || should_quit.load(Ordering::Acquire))
                            },
                        );
                        let response = match response {
                            Ok(Some(response)) => response,
                            Ok(None) => crate::api::error_response_json(
                                api_request_id,
                                "server_unavailable",
                                "agent readiness wait cancelled".into(),
                            ),
                            Err(err) => crate::api::error_response_json(
                                api_request_id,
                                "server_unavailable",
                                err.to_string(),
                            ),
                        };
                        let _ = respond_to.send(response);
                    })
            {
                let _ = error_sender.send(crate::api::error_response_json(
                    error_id,
                    "server_unavailable",
                    err.to_string(),
                ));
            }
            return false;
        }
        let foreground_changed = self.promote_client_to_foreground(client_id);
        foreground_changed
            | self.handle_client_shell_api_request(
                client_id,
                api::ApiRequestMessage {
                    request: *request,
                    respond_to,
                    response_write_complete: None,
                },
            )
    }
}
