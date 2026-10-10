// Client-only native profile state/actions. All durable data is owned by the
// selected server and goes through advertised JSON API methods.
use super::native_profiles::{AgentProfileForm, AgentProfileMarkdown, ProfileAction};
use super::*;
use crate::api::schema::*;
use crossterm::event::{KeyCode, KeyEvent};

impl ClientShellState {
    pub(super) fn queue_native_profiles(&mut self, outcome: &mut ClientShellInput) {
        self.push_endpoint_method_with_kind(
            Method::AgentProfileList(EmptyParams::default()),
            PendingEndpointKind::NativeProfiles,
            outcome,
        );
    }
    pub(super) fn open_native_agents(&mut self, outcome: &mut ClientShellInput) {
        self.open_settings_overlay();
        self.select_settings_section(ClientSettingsSection::Agents, outcome);
    }
    pub(super) fn load_native_profile(&mut self, role: String, outcome: &mut ClientShellInput) {
        self.open_settings_overlay();
        if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
            settings.section = ClientSettingsSection::Agents;
        }
        self.push_endpoint_method_with_kind(
            Method::AgentProfileEdit(AgentProfileEditParams {
                role,
                settings: None,
                documents: Default::default(),
                delete_document: None,
            }),
            PendingEndpointKind::NativeProfileLoad,
            outcome,
        );
    }
    pub(super) fn activate_native_profile(&mut self, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_ref() else {
            return;
        };
        if settings.profile_form.is_some() {
            self.save_native_profile(outcome);
            return;
        }
        let selected = settings.selected;
        if selected >= 3 {
            if let Some(profile) = settings.profiles.get(selected - 3) {
                self.load_native_profile(profile.role.clone(), outcome);
            }
            return;
        }
        let cwd = self
            .snapshot
            .as_deref()
            .and_then(|s| {
                s.panes
                    .iter()
                    .find(|p| Some(p.pane_id.as_str()) == s.focused_pane_id.as_deref())
            })
            .and_then(|p| p.cwd.clone())
            .unwrap_or_else(|| "/".into());
        if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
            settings.profile_form = Some(Box::new(AgentProfileForm {
                existing_role: None,
                role: String::new(),
                harness: ["codex", "pi", "claude"][selected].into(),
                native_cwd: cwd,
                model: String::new(),
                effort: String::new(),
                apikey_ref: String::new(),
                allowlist: String::new(),
                additional_markdown: Vec::new(),
                linked_markdown: Vec::new(),
                instructions: "# Agent instructions".into(),
                instructions_cursor: 0,
                instructions_scroll: 0,
                selected_markdown: None,
                pending_markdown_name: None,
                selected_field: 0,
            }));
        }
        outcome.repaint = true;
    }
    pub(super) fn save_native_profile(&mut self, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_ref() else {
            return;
        };
        let Some(form) = settings.profile_form.as_ref() else {
            return;
        };
        if form.is_new() {
            self.push_endpoint_method_with_kind(
                Method::AgentProfileCreate(AgentProfileCreateParams {
                    role: form.role.clone(),
                    harness: form.harness.clone(),
                    native_cwd: form.native_cwd.clone(),
                    instructions: Some(form.instructions.clone()),
                }),
                PendingEndpointKind::NativeProfileSave,
                outcome,
            );
            return;
        }
        let allowlist = if form.allowlist.trim().is_empty() {
            None
        } else {
            match serde_json::from_str(&form.allowlist) {
                Ok(value) => Some(value),
                Err(_) => {
                    self.set_endpoint_error("Tools must be JSON containing a tools array");
                    return;
                }
            }
        };
        let effort = match form.effort.as_str() {
            "low" => Some(AgentProfileEffort::Low),
            "medium" => Some(AgentProfileEffort::Medium),
            "high" => Some(AgentProfileEffort::High),
            _ => None,
        };
        let caps = crate::agent_registry::native_harness_capabilities(&form.harness);
        let patch = AgentProfileSetParams {
            role: form.role.clone(),
            harness: Some(form.harness.clone()),
            native_cwd: Some(form.native_cwd.clone()),
            model: (!form.model.trim().is_empty()).then(|| form.model.clone()),
            effort,
            apikey_ref: (caps.api_key_ref && !form.apikey_ref.is_empty())
                .then(|| form.apikey_ref.clone()),
            allowlist: if caps.tool_allowlist { allowlist } else { None },
            clear_model: form.model.is_empty(),
            clear_effort: form.effort.is_empty(),
            clear_apikey_ref: !caps.api_key_ref || form.apikey_ref.is_empty(),
            clear_allowlist: !caps.tool_allowlist || form.allowlist.is_empty(),
        };
        let documents = std::iter::once(("AGENTS.md".into(), form.instructions.clone()))
            .chain(
                form.additional_markdown
                    .iter()
                    .map(|d| (d.name.clone(), d.content.clone())),
            )
            .collect();
        self.push_endpoint_method_with_kind(
            Method::AgentProfileEdit(AgentProfileEditParams {
                role: form.role.clone(),
                settings: Some(patch),
                documents,
                delete_document: None,
            }),
            PendingEndpointKind::NativeProfileSave,
            outcome,
        );
    }
    pub(super) fn route_native_profile_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() else {
            return false;
        };
        if settings.section != ClientSettingsSection::Agents {
            return false;
        }
        if let Some(form) = settings.profile_form.as_mut() {
            let action =
                super::native_profiles::edit_form(form, KeyEvent::new(key.code, key.modifiers));
            match action {
                Some(ProfileAction::Cancel) => settings.profile_form = None,
                Some(ProfileAction::SaveAgentProfile) => self.save_native_profile(outcome),
                Some(ProfileAction::CreateAgentProfileMarkdown { role, name }) => {
                    self.push_endpoint_method_with_kind(
                        Method::AgentProfileEdit(AgentProfileEditParams {
                            role,
                            settings: None,
                            documents: [(name, String::new())].into_iter().collect(),
                            delete_document: None,
                        }),
                        PendingEndpointKind::NativeProfileLoad,
                        outcome,
                    );
                }
                Some(ProfileAction::DeleteAgentProfileMarkdown { role, name }) => {
                    self.push_endpoint_method_with_kind(
                        Method::AgentProfileEdit(AgentProfileEditParams {
                            role,
                            settings: None,
                            documents: Default::default(),
                            delete_document: Some(name),
                        }),
                        PendingEndpointKind::NativeProfileLoad,
                        outcome,
                    );
                }
                None => {}
            }
            outcome.repaint = true;
            return true;
        }
        let role = settings
            .selected
            .checked_sub(3)
            .and_then(|i| settings.profiles.get(i))
            .map(|p| p.role.clone());
        match key.code {
            KeyCode::Char('d') if role.is_some() => self.push_endpoint_method_with_kind(
                Method::AgentProfileDelete(AgentProfileDeleteParams {
                    role: role.unwrap_or_default(),
                }),
                PendingEndpointKind::NativeProfileSave,
                outcome,
            ),
            KeyCode::Char('s') if role.is_some() => self.push_endpoint_method_with_kind(
                Method::AgentSpawn(AgentSpawnParams {
                    role: role.unwrap_or_default(),
                    kind: None,
                    tab_id: self
                        .snapshot
                        .as_deref()
                        .and_then(|s| s.focused_tab_id.clone()),
                    cwd_mode: "tab".into(),
                    timeout_ms: None,
                    args: Vec::new(),
                }),
                PendingEndpointKind::Generic,
                outcome,
            ),
            _ => return false,
        };
        outcome.repaint = true;
        true
    }
    pub(super) fn prepare_native_harness(
        &mut self,
        pane_id: String,
        outcome: &mut ClientShellInput,
    ) {
        self.push_endpoint_method_with_kind(
            Method::AgentHarnessPrepare(AgentTarget { target: pane_id }),
            PendingEndpointKind::NativeHarnessPrepare,
            outcome,
        );
    }
    pub(super) fn handle_native_profile_result(
        &mut self,
        kind: PendingEndpointKind,
        result: Result<ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                    settings.profile_error = Some(error.message);
                }
                return (true, Vec::new());
            }
        };
        let mut next = ClientShellInput::default();
        match result {
            ResponseResult::AgentProfileList { profiles } => {
                self.native_profiles = profiles.clone();
                self.native_profile_endpoint = Some(self.active_endpoint_id.clone());
                if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                    settings.profiles = profiles;
                }
            }
            ResponseResult::AgentProfileDeleted { .. } => {
                self.queue_native_profiles(&mut next);
            }
            ResponseResult::AgentProfileInfo { profile } => {
                self.load_native_profile(profile.role, &mut next)
            }
            ResponseResult::AgentProfileDossier { profile, documents } => {
                let retained_draft = match self.overlay.as_mut() {
                    Some(ClientShellOverlay::Settings(settings)) => settings
                        .profile_form
                        .take()
                        .filter(|form| form.role == profile.role),
                    _ => None,
                };
                let linked_markdown = profile
                    .mds
                    .iter()
                    .filter(|md| !documents.contains_key(&md.name))
                    .map(|md| md.name.clone())
                    .collect::<Vec<_>>();
                let instructions = documents.get("AGENTS.md").cloned().unwrap_or_default();
                let additional_markdown = documents
                    .into_iter()
                    .filter(|(name, _)| name != "AGENTS.md")
                    .map(|(name, content)| AgentProfileMarkdown {
                        name,
                        path: String::new(),
                        cursor: content.len(),
                        content,
                        scroll: 0,
                    })
                    .collect();
                if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                    settings.profile_error = None;
                    settings.profile_form = Some(Box::new(AgentProfileForm {
                        existing_role: Some(profile.role.clone()),
                        role: profile.role,
                        harness: profile.harness,
                        native_cwd: profile.native_cwd,
                        model: profile.model.unwrap_or_default(),
                        effort: profile
                            .effort
                            .map(|e| match e {
                                AgentProfileEffort::Low => "low",
                                AgentProfileEffort::Medium => "medium",
                                AgentProfileEffort::High => "high",
                            })
                            .unwrap_or("")
                            .into(),
                        apikey_ref: profile.apikey_ref.unwrap_or_default(),
                        allowlist: profile.allowlist.map(|v| v.to_string()).unwrap_or_default(),
                        additional_markdown,
                        linked_markdown,
                        instructions_cursor: instructions.len(),
                        instructions_scroll: 0,
                        instructions,
                        selected_markdown: None,
                        pending_markdown_name: None,
                        selected_field: 7,
                    }));
                    if matches!(kind, PendingEndpointKind::NativeProfileLoad) {
                        if let Some(mut draft) = retained_draft {
                            if let Some(loaded) = settings.profile_form.as_ref() {
                                let mut documents = loaded.additional_markdown.clone();
                                for document in &mut documents {
                                    if let Some(previous) = draft
                                        .additional_markdown
                                        .iter()
                                        .find(|old| old.name == document.name)
                                    {
                                        *document = previous.clone();
                                    }
                                }
                                draft.additional_markdown = documents;
                            }
                            draft.pending_markdown_name = None;
                            settings.profile_form = Some(draft);
                        }
                    }
                    if matches!(kind, PendingEndpointKind::NativeProfileSave) {
                        settings.profile_form = None;
                        self.queue_native_profiles(&mut next);
                    }
                }
            }
            ResponseResult::AgentHarnessSet {
                instance_id,
                harness_override,
            } => {
                self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
                    target: ClientContextMenuTarget::AgentHarness { instance_id },
                    x: self.hits.panes.first().map(|p| p.rect.x).unwrap_or(0),
                    y: self.hits.panes.first().map(|p| p.rect.y).unwrap_or(0),
                    highlighted: match harness_override.as_deref() {
                        Some("codex") => 1,
                        Some("pi") => 2,
                        Some("claude") => 3,
                        _ => 0,
                    },
                }));
            }
            _ => self.set_endpoint_error("Unexpected native agent response"),
        }
        (true, next.actions)
    }
}

pub(super) fn render_profiles(
    buffer: &mut Buffer,
    area: Rect,
    settings: &ClientSettingsOverlay,
    palette: &Palette,
) -> Vec<(Rect, usize)> {
    use super::render::put_text;
    let style = Style::default().fg(palette.text).bg(palette.panel_bg);
    let mut hits = Vec::new();
    if let Some(form) = settings.profile_form.as_ref() {
        let mut fields = Vec::new();
        if form.is_new() {
            fields.push((0, format!("role: {}", form.role)));
            fields.push((1, format!("native harness: {} (←→)", form.harness)));
            fields.push((2, format!("native cwd: {}", form.native_cwd)));
        } else {
            fields.extend([
                (0, format!("native harness: {} (←→)", form.harness)),
                (1, format!("native cwd: {}", form.native_cwd)),
                (2, format!("model: {}", form.model)),
                (3, format!("effort: {} (←→)", form.effort)),
            ]);
            if form.field_enabled(4) {
                fields.push((4, format!("API key ref (env:NAME): {}", form.apikey_ref)));
            }
            if form.field_enabled(5) {
                fields.push((
                    5,
                    format!(
                        "{}: {}",
                        if form.harness == "claude" {
                            "auto-approved tools"
                        } else {
                            "built-in tools"
                        },
                        form.allowlist
                    ),
                ));
            }
            fields.push((
                6,
                format!(
                    "documents: {} (a add, d delete, ←→)",
                    form.pending_markdown_name
                        .as_deref()
                        .unwrap_or(form.active_document_name())
                ),
            ));
        }
        let mut y = area.y;
        for (field, label) in fields {
            if y >= area.bottom() {
                break;
            }
            let rect = Rect::new(area.x, y, area.width, 1);
            let st = if form.selected_field == field {
                style.bg(palette.surface0)
            } else {
                style
            };
            buffer.set_style(rect, st);
            put_text(buffer, rect.x, rect.y, rect.width, &label, st);
            hits.push((rect, field));
            y += 1;
        }
        if y < area.bottom() {
            put_text(
                buffer,
                area.x,
                y,
                area.width,
                form.active_document_name(),
                style.fg(palette.accent),
            );
            y += 1;
        }
        let document = Rect::new(area.x, y, area.width, area.bottom().saturating_sub(y));
        super::native_profiles::render_document(buffer, document, form, style);
        hits.push((document, form.instructions_field()));
    } else {
        for (index, label) in ["New Codex profile", "New Pi profile", "New Claude profile"]
            .into_iter()
            .map(String::from)
            .chain(
                settings
                    .profiles
                    .iter()
                    .map(|p| format!("{} ({}) · {}", p.role, p.native_cwd, p.harness)),
            )
            .enumerate()
            .skip(
                settings
                    .selected
                    .saturating_sub(area.height.saturating_sub(1) as usize),
            )
            .take(area.height as usize)
        {
            let rect = Rect::new(area.x, area.y + hits.len() as u16, area.width, 1);
            let st = if index == settings.selected {
                style.bg(palette.surface0)
            } else {
                style
            };
            buffer.set_style(rect, st);
            put_text(buffer, rect.x, rect.y, rect.width, &label, st);
            hits.push((rect, index));
        }
    }
    hits
}
