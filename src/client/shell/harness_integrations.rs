//! Client projection of the selected endpoint's activated integrations.
use super::*;
use crate::api::schema::{EmptyParams, IntegrationHarnessSetParams, Method, ResponseResult};

impl ClientShellState {
    pub(super) fn active_harness_choices(&self) -> Vec<String> {
        self.harness_integrations
            .iter()
            .filter(|h| h.active && h.available)
            .map(|h| h.id.clone())
            .collect()
    }

    pub(super) fn harness_catalog_available(&self) -> bool {
        self.endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == self.active_endpoint_id)
            .and_then(|endpoint| endpoint.methods.as_ref())
            .is_some_and(|methods| methods.contains("integration.harness.list"))
    }

    pub(super) fn queue_harness_integrations(&mut self, outcome: &mut ClientShellInput) {
        // An older server may still offer status hooks without the activated
        // harness catalog. Leave its endpoint connected and explain the gap.
        if self.harness_catalog_available() {
            self.push_endpoint_method_with_kind(
                Method::IntegrationHarnessList(EmptyParams::default()),
                PendingEndpointKind::HarnessIntegrationList,
                outcome,
            );
        }
    }

    pub(super) fn toggle_harness_integration(&mut self, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() else {
            return;
        };
        let Some(integration) = settings
            .harness_integrations
            .get(settings.selected)
            .cloned()
        else {
            return;
        };
        if !integration.available && !integration.active {
            settings.integration_messages = vec![integration
                .installer
                .unwrap_or_else(|| "Install this CLI from its official vendor first.".into())];
            outcome.repaint = true;
            return;
        }
        self.push_endpoint_method_with_kind(
            Method::IntegrationHarnessSet(IntegrationHarnessSetParams {
                target: integration.id,
                active: !integration.active,
            }),
            PendingEndpointKind::HarnessIntegrationSet,
            outcome,
        );
    }

    pub(super) fn handle_harness_integration_result(
        &mut self,
        result: Result<ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        let selected_profile = match self.overlay.as_ref() {
            Some(ClientShellOverlay::Settings(settings))
                if settings.section == ClientSettingsSection::Agents =>
            {
                let count = settings
                    .harness_integrations
                    .iter()
                    .filter(|h| h.active && h.available)
                    .count();
                settings
                    .selected
                    .checked_sub(count)
                    .and_then(|index| settings.profiles.get(index))
                    .map(|profile| profile.role.clone())
            }
            _ => None,
        };
        match result {
            Ok(ResponseResult::HarnessIntegrationList { integrations }) => {
                self.harness_integrations = integrations
            }
            Ok(ResponseResult::HarnessIntegrationSet { integration }) => {
                if let Some(existing) = self
                    .harness_integrations
                    .iter_mut()
                    .find(|entry| entry.id == integration.id)
                {
                    *existing = integration;
                } else {
                    self.harness_integrations.push(integration);
                }
            }
            Ok(_) => self.set_endpoint_error("Unexpected harness integration response"),
            Err(error) => {
                if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                    settings.integration_messages = vec![error.message];
                }
                return (true, Vec::new());
            }
        }
        let choices = self.active_harness_choices();
        if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
            settings.harness_integrations = self.harness_integrations.clone();
            if let Some(role) = selected_profile {
                if let Some(index) = settings
                    .profiles
                    .iter()
                    .position(|profile| profile.role == role)
                {
                    settings.selected = choices.len() + index;
                }
            }
            if let Some(form) = settings.profile_form.as_mut() {
                form.harness_choices = choices.clone();
                form.harness_settings = self.harness_integrations.clone();
            }
        }
        if let Some(ClientShellOverlay::ContextMenu(menu)) = self.overlay.as_mut() {
            if let ClientContextMenuTarget::AgentHarness { harnesses, .. } = &mut menu.target {
                let selected = menu
                    .highlighted
                    .checked_sub(1)
                    .and_then(|index| harnesses.get(index))
                    .cloned();
                *harnesses = choices;
                menu.highlighted = selected
                    .and_then(|id| harnesses.iter().position(|h| *h == id))
                    .map_or(0, |index| index + 1);
            }
        }
        (true, Vec::new())
    }
}
