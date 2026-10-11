use crate::api::schema::{
    IntegrationInfo, IntegrationInstallResult, IntegrationState, IntegrationUninstallResult,
    ResponseResult,
};
use crate::app::App;

use super::responses::{encode_error, encode_success};

impl App {
    pub(super) fn harness_integrations(&self) -> Vec<crate::api::schema::HarnessIntegrationInfo> {
        let hooks = crate::integration::integration_recommendations();
        crate::harness::HARNESSES
            .iter()
            .map(|h| {
                let hook = h
                    .hook
                    .and_then(|target| hooks.iter().find(|entry| entry.target == target));
                let hook_state = hook.map(|entry| match entry.state {
                    crate::integration::IntegrationStatusKind::Current => IntegrationState::Current,
                    crate::integration::IntegrationStatusKind::Outdated => {
                        IntegrationState::Outdated
                    }
                    crate::integration::IntegrationStatusKind::NotInstalled => {
                        IntegrationState::NotInstalled
                    }
                });
                crate::api::schema::HarnessIntegrationInfo {
                    id: h.id.into(),
                    label: h.name.into(),
                    command: h.executable.into(),
                    available: h.available(),
                    active: h.active(
                        &self.agent_registry,
                        hook_state.is_some_and(|state| state != IntegrationState::NotInstalled),
                    ),
                    hook_target: h.hook,
                    hook_state,
                    installer: h.installer.map(Into::into),
                    profile_instructions: h.instructions != crate::harness::Instructions::Native,
                    capabilities: crate::agent_registry::native_harness_capabilities(h.id),
                }
            })
            .collect()
    }

    pub(super) fn handle_integration_harness_list(&mut self, id: String) -> String {
        if let Err(err) = self.refresh_agent_registry_for_read() {
            return encode_error(id, "integration_load_failed", err.to_string());
        }
        encode_success(
            id,
            ResponseResult::HarnessIntegrationList {
                integrations: self.harness_integrations(),
            },
        )
    }

    pub(super) fn handle_integration_harness_set(
        &mut self,
        id: String,
        params: crate::api::schema::IntegrationHarnessSetParams,
    ) -> String {
        let Some(harness) = crate::harness::find(&params.target) else {
            return encode_error(id, "invalid_integration", "unknown terminal integration");
        };
        if params.active && !harness.available() {
            return encode_error(
                id,
                "integration_cli_missing",
                harness
                    .installer
                    .unwrap_or("install this CLI from its official vendor first"),
            );
        }
        if let Err(err) = self.update_agent_registry(|registry| {
            registry
                .harness_activations
                .insert(harness.id.into(), params.active);
        }) {
            return encode_error(id, "integration_persist_failed", err.to_string());
        }
        let Some(integration) = self
            .harness_integrations()
            .into_iter()
            .find(|entry| entry.id == harness.id)
        else {
            return encode_error(id, "invalid_integration", "unknown terminal integration");
        };
        encode_success(id, ResponseResult::HarnessIntegrationSet { integration })
    }

    pub(super) fn handle_integration_list(&self, id: String) -> String {
        let integrations = crate::integration::integration_recommendations()
            .into_iter()
            .map(|recommendation| IntegrationInfo {
                target: recommendation.target,
                label: recommendation.label.to_owned(),
                command: recommendation.command.to_owned(),
                available: recommendation.available,
                state: match recommendation.state {
                    crate::integration::IntegrationStatusKind::NotInstalled => {
                        IntegrationState::NotInstalled
                    }
                    crate::integration::IntegrationStatusKind::Current => IntegrationState::Current,
                    crate::integration::IntegrationStatusKind::Outdated => {
                        IntegrationState::Outdated
                    }
                },
            })
            .collect();
        encode_success(id, ResponseResult::IntegrationList { integrations })
    }

    pub(super) fn handle_integration_install(
        &mut self,
        id: String,
        params: crate::api::schema::IntegrationInstallParams,
    ) -> String {
        let target = params.target;
        let messages = match crate::integration::install_target(target) {
            Ok(messages) => messages,
            Err(err) => return encode_error(id, "integration_install_failed", err.to_string()),
        };
        self.state.integration_recommendations = crate::integration::integration_recommendations();

        encode_success(
            id,
            ResponseResult::IntegrationInstall {
                target,
                details: IntegrationInstallResult { messages },
            },
        )
    }

    pub(super) fn handle_integration_uninstall(
        &mut self,
        id: String,
        params: crate::api::schema::IntegrationUninstallParams,
    ) -> String {
        let target = params.target;
        let messages = match crate::integration::uninstall_target(target) {
            Ok(messages) => messages,
            Err(err) => return encode_error(id, "integration_uninstall_failed", err.to_string()),
        };
        self.state.integration_recommendations = crate::integration::integration_recommendations();

        encode_success(
            id,
            ResponseResult::IntegrationUninstall {
                target,
                details: IntegrationUninstallResult { messages },
            },
        )
    }
}
