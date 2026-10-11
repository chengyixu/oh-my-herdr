use super::*;

#[test]
fn shell_new_controls_use_the_same_client_action_routes_as_keybinds() {
    let mut config = Config::default();
    config.ui.prompt_new_workspace_name = false;
    config.ui.prompt_new_tab_name = true;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("composed frame");

    let new_workspace = state.hits.new_workspace;
    let create_workspace =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: new_workspace.x + 1,
            row: new_workspace.y,
            modifiers: KeyModifiers::empty(),
        })]);
    let [ClientShellAction::Endpoint { request, .. }] = &create_workspace.actions[..] else {
        panic!("new workspace click should use the endpoint API");
    };
    assert!(matches!(
        request.method,
        crate::api::schema::Method::WorkspaceCreate(_)
    ));

    let new_tab = state.hits.new_tab;
    let open_new_tab =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: new_tab.x + 1,
            row: new_tab.y,
            modifiers: KeyModifiers::empty(),
        })]);
    assert!(open_new_tab.actions.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            target: ClientRenameTarget::NewTab { .. },
            ..
        }))
    ));
}

#[test]
fn manual_client_chrome_preferences_round_trip_per_endpoint() {
    let path = std::env::temp_dir().join(format!(
        "herdr-client-shell-prefs-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let config =
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.clone());
    let mut state = ClientShellState::new(config);
    state.sidebar_width = 31;
    state.sidebar_width_manual = true;
    state.sidebar_section_split = 0.7;
    state.sidebar_section_split_manual = true;
    state.sidebar_collapsed = true;
    state.sidebar_collapsed_manual = true;
    state.collapsed_groups.insert("repo-two".into());
    state.collapsed_groups.insert("repo-one".into());
    let profile =
        SavedSshEndpoint::new("Build", "dev@build.example", "agents").expect("saved SSH profile");
    let remote_id = ClientEndpointId::Ssh(profile.id.clone());
    state
        .remote_collapsed_groups
        .insert(remote_id.clone(), HashSet::from(["/repo".to_owned()]));
    state.persist_chrome_preferences(&mut ClientShellInput::default());
    let stored = std::fs::read_to_string(&path).expect("stored client chrome preferences");
    assert!(stored.contains(profile.id.as_str()));
    assert!(stored.contains("repo-one"));
    assert!(!stored.contains(&profile.label));
    assert!(!stored.contains(&profile.target));

    let reloaded_config =
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.clone());
    let reloaded = ClientShellState::new(reloaded_config);
    assert_eq!(reloaded.sidebar_width, 31);
    assert!(reloaded.sidebar_width_manual);
    assert_eq!(reloaded.sidebar_section_split, 0.7);
    assert!(reloaded.sidebar_section_split_manual);
    assert!(reloaded.sidebar_collapsed);
    assert!(reloaded.sidebar_collapsed_manual);
    assert_eq!(
        reloaded.collapsed_groups,
        HashSet::from(["repo-one".to_string(), "repo-two".to_string()])
    );
    assert_eq!(
        reloaded.remote_collapsed_groups.get(&remote_id),
        Some(&HashSet::from(["/repo".to_owned()]))
    );
    let mut reloaded = reloaded;
    reloaded.persist_chrome_preferences(&mut ClientShellInput::default());
    let stored_again = std::fs::read_to_string(&path).expect("restored client chrome preferences");
    assert!(stored_again.contains(profile.id.as_str()));
    assert!(stored_again.contains("/repo"));
    std::fs::remove_file(path).expect("remove client chrome preferences");
}

#[test]
fn tab_bar_renders_endpoint_status_ellipses_and_clamps_to_useful_scroll() {
    let mut projected = snapshot();
    projected.tab_bar_right = vec![
        crate::protocol::ClientShellTabStatusSegment {
            text: "ZOOM".into(),
            accent: true,
        },
        crate::protocol::ClientShellTabStatusSegment {
            text: "host".into(),
            accent: false,
        },
    ];
    projected.tab_bar_right_separator = " · ".into();
    for number in 2..=8 {
        projected.tabs.push(ClientShellTab {
            tab_id: format!("tab_{number}"),
            workspace_id: "ws_1".into(),
            number,
            label: number.to_string(),
            custom_label: false,
            zoomed: false,
            focused: false,
            agent_status: AgentStatus::Idle,
        });
    }
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.mobile_width_threshold = 0;
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let frame = state.compose(106, 20).expect("status and overflow tabs");
    let top = frame.cells[..frame.width as usize]
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(top.contains("ZOOM · host"));
    assert!(top.contains('…'));

    state.tab_scroll = usize::MAX;
    state.reveal_focused_tab = false;
    state.compose(106, 20).expect("clamped tab scroll");
    assert!(state.tab_scroll < 7);
    let manual_scroll = state.tab_scroll;
    let mut replacement = (**state.snapshot.as_ref().expect("snapshot")).clone();
    replacement.revision = 2;
    replacement.tab_bar_right[1].text = "tick".into();
    let mut replacement_surface = surface();
    replacement_surface.projection_revision = 2;
    state.set_snapshot(Box::new(replacement));
    state.set_pane_surface(replacement_surface);
    assert!(!state.reveal_focused_tab);
    state.compose(106, 20).expect("same-width status update");
    assert_eq!(state.tab_scroll, manual_scroll);

    state.compose(45, 20).expect("narrow tabs win over status");
    let narrow = state.compose(45, 20).expect("narrow tab frame");
    let top = narrow.cells[..narrow.width as usize]
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(!top.contains("ZOOM · host"));
}

#[test]
fn inactive_auto_named_tab_label_does_not_stack_terminal_faint() {
    let mut projected = snapshot();
    projected.tabs.push(ClientShellTab {
        tab_id: "tab_2".into(),
        workspace_id: "ws_1".into(),
        number: 2,
        label: "beta".into(),
        custom_label: false,
        zoomed: false,
        focused: false,
        agent_status: AgentStatus::Idle,
    });
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let frame = state.compose(106, 20).expect("tab bar frame");
    let rect = state
        .hits
        .tabs
        .iter()
        .find(|(_, tab_id)| tab_id == "tab_2")
        .expect("inactive tab hit")
        .0;
    let buffer = frame.to_ratatui_buffer().expect("tab bar buffer");
    let (x, y) = cell_symbol_position(&frame, rect, "beta");
    let cell = buffer.cell((x, y)).expect("inactive tab cell");
    assert!(
        !cell.modifier.contains(Modifier::DIM),
        "inactive tab label at ({x},{y}) should not stack terminal faint: {cell:?}"
    );
}

#[test]
fn configured_prefix_is_client_owned_and_renders_its_bar() {
    let config = toml::from_str::<Config>(
        r#"
[keys]
prefix = "ctrl+a"
detach = "prefix+x"
"#,
    )
    .expect("configured keybinds");
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());

    let old_default = state.handle_input_bytes(&[0x02]);
    assert_eq!(
        old_default.requests.len(),
        1,
        "ctrl-b should reach the pane"
    );

    let prefix = state.handle_input_bytes(&[0x01]);
    assert!(prefix.requests.is_empty());
    assert!(prefix.repaint);
    let frame = state.compose(106, 20).expect("prefix frame");
    let text = frame
        .cells
        .chunks(frame.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("PREFIX"), "frame: {text:?}");
    assert!(text.contains("ctrl+a"), "frame: {text:?}");

    let detach = state.handle_input_bytes(b"x");
    assert!(detach.detach);
    assert!(detach.requests.is_empty());
}

#[test]
fn prefix_endpoint_action_uses_public_api_with_stable_ids() {
    let mut config = Config::default();
    config.ui.prompt_new_tab_name = false;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));

    assert!(state.handle_input_bytes(&[0x02]).actions.is_empty());
    let create = state.handle_input_bytes(b"c");
    let [ClientShellAction::Endpoint {
        boot_id, request, ..
    }] = &create.actions[..]
    else {
        panic!("expected one endpoint action: {:?}", create.actions);
    };
    assert_eq!(boot_id, "boot-1");
    match &request.method {
        crate::api::schema::Method::TabCreate(params) => {
            assert_eq!(params.workspace_id.as_deref(), Some("ws_1"));
            assert!(params.focus);
        }
        other => panic!("expected tab.create, got {other:?}"),
    }
    assert!(state.pending_requests.contains_key(&request.id));
}

#[test]
fn remote_keybinding_sources_keep_local_commands_off_endpoints_and_apply_server_profiles() {
    let local: Config = toml::from_str(
        r#"
[keys]
prefix = "ctrl+a"
new_tab = "prefix+c"

[[keys.command]]
key = "prefix+c"
command = "local-only"
"#,
    )
    .unwrap();
    let remote_local = ClientShellConfig::from_config(&local)
        .with_keybinding_source(ClientShellKeybindingSource::RemoteLocal);
    assert_eq!(remote_local.keybinds.prefix[0].0, KeyCode::Char('a'));
    assert!(remote_local.keybinds.keybinds.custom_commands.is_empty());
    assert_eq!(
        remote_local.keybinds.keybinds.new_tab.label().as_deref(),
        Some("prefix+c")
    );

    let mut local_state = ClientShellState::new(
        ClientShellConfig::from_config(&local)
            .with_keybinding_source(ClientShellKeybindingSource::Local),
    );
    let mut local_projection = snapshot();
    local_projection
        .commands
        .push(crate::protocol::ClientShellCommand {
            command_id: "cmd_loaded_endpoint".into(),
            binding_label: "prefix+c / prefix+y".into(),
            binding_labels: vec!["prefix+c".into(), "prefix+y".into()],
            action: crate::protocol::ClientShellCommandAction::Shell,
            description: Some("loaded endpoint command".into()),
        });
    local_state.set_snapshot(Box::new(local_projection));
    assert_eq!(
        local_state.config.keybinds.keybinds.custom_commands[0].label,
        "prefix+y"
    );
    assert_eq!(
        local_state
            .config
            .keybinds
            .keybinds
            .new_tab
            .label()
            .as_deref(),
        Some("prefix+c")
    );
    let mut command_outcome = ClientShellInput::default();
    local_state.record_binding(
        crate::input::KeybindMatch::Command(
            local_state.config.keybinds.keybinds.custom_commands[0].clone(),
        ),
        &mut command_outcome,
    );
    let [ClientShellAction::Endpoint { request, .. }] = &command_outcome.actions[..] else {
        panic!("expected surviving endpoint command binding");
    };
    let crate::api::schema::Method::CommandInvoke(params) = &request.method else {
        panic!("expected command invocation");
    };
    assert_eq!(params.command_id, "cmd_loaded_endpoint");

    let mut id_only_projection = snapshot();
    id_only_projection.revision = 2;
    id_only_projection
        .commands
        .push(crate::protocol::ClientShellCommand {
            command_id: "cmd_reloaded_endpoint".into(),
            binding_label: "prefix+c / prefix+y".into(),
            binding_labels: vec!["prefix+c".into(), "prefix+y".into()],
            action: crate::protocol::ClientShellCommandAction::Shell,
            description: Some("loaded endpoint command".into()),
        });
    local_state.mode = ClientShellMode::Prefix;
    local_state.set_snapshot(Box::new(id_only_projection));
    assert_eq!(local_state.mode, ClientShellMode::Prefix);
    assert_eq!(
        local_state.config.keybinds.keybinds.custom_commands[0].command,
        "cmd_reloaded_endpoint"
    );

    let endpoint: Config = toml::from_str(
        r#"
[keys]
prefix = "ctrl+x"
new_tab = "prefix+n"
"#,
    )
    .unwrap();
    let mut state = ClientShellState::new(
        ClientShellConfig::from_config(&local)
            .with_keybinding_source(ClientShellKeybindingSource::Endpoint),
    );
    let mut projection = snapshot();
    projection.server_keybindings_toml = endpoint.local_keybindings_profile_toml().ok();
    projection
        .commands
        .push(crate::protocol::ClientShellCommand {
            command_id: "cmd_remote".into(),
            binding_label: "prefix+z".into(),
            binding_labels: vec!["prefix+z".into()],
            action: crate::protocol::ClientShellCommandAction::Shell,
            description: Some("remote command".into()),
        });
    state.set_snapshot(Box::new(projection));

    assert_eq!(state.config.keybinds.prefix[0].0, KeyCode::Char('x'));
    assert_eq!(
        state.config.keybinds.keybinds.new_tab.label().as_deref(),
        Some("prefix+n")
    );
    assert_eq!(
        state.config.keybinds.keybinds.custom_commands[0].label,
        "prefix+z"
    );
    assert_eq!(
        state.config.keybinds.keybinds.custom_commands[0]
            .description
            .as_deref(),
        Some("remote command")
    );
    assert_eq!(
        state.config.keybinds.keybinds.custom_commands[0].command,
        "cmd_remote"
    );
}

#[test]
fn custom_binding_invokes_only_the_endpoint_manifest_id() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let binding = crate::config::CustomCommandKeybind {
        bindings: crate::config::ActionKeybinds::prefix("z"),
        label: "prefix+z".into(),
        command: "secret-command --token hidden".into(),
        action: crate::config::CustomCommandAction::Shell,
        description: None,
        width: None,
        height: None,
    };
    let mut projection = snapshot();
    projection
        .commands
        .push(crate::protocol::ClientShellCommand {
            command_id: "cmd_0123456789abcdef0123456789abcdef".into(),
            binding_label: binding.label.clone(),
            binding_labels: binding.bindings.labels(),
            action: crate::protocol::ClientShellCommandAction::Shell,
            description: None,
        });
    state.set_snapshot(Box::new(projection));

    let mut outcome = ClientShellInput::default();
    state.record_binding(crate::input::KeybindMatch::Command(binding), &mut outcome);

    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("expected endpoint command invocation");
    };
    let crate::api::schema::Method::CommandInvoke(params) = &request.method else {
        panic!("expected command.invoke");
    };
    assert_eq!(params.command_id, "cmd_0123456789abcdef0123456789abcdef");
    assert_eq!(params.workspace_id.as_deref(), Some("ws_1"));
    assert_eq!(params.tab_id.as_deref(), Some("tab_1"));
    assert_eq!(params.pane_id.as_deref(), Some("pane_1"));
    assert_eq!(params.selection, None);
    assert!(!serde_json::to_string(request)
        .unwrap()
        .contains("secret-command"));
}

#[test]
fn plugin_command_carries_client_owned_selection_coordinates() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let binding = crate::config::CustomCommandKeybind {
        bindings: crate::config::ActionKeybinds::prefix("p"),
        label: "prefix+p".into(),
        command: "plugin.action".into(),
        action: crate::config::CustomCommandAction::PluginAction,
        description: None,
        width: None,
        height: None,
    };
    let mut projection = snapshot();
    projection
        .commands
        .push(crate::protocol::ClientShellCommand {
            command_id: "cmd_plugin".into(),
            binding_label: binding.label.clone(),
            binding_labels: binding.bindings.labels(),
            action: crate::protocol::ClientShellCommandAction::PluginAction,
            description: None,
        });
    state.set_snapshot(Box::new(projection));
    let mut pane_surface = surface();
    pane_surface.panes[0].content_revision = 42;
    state.set_pane_surface(pane_surface);
    let mut selection =
        crate::selection::Selection::absolute_range("pane_1".to_owned(), (2, 3), (4, 5));
    assert!(selection.finish());
    state.selection = Some(selection);

    let mut outcome = ClientShellInput::default();
    state.record_binding(crate::input::KeybindMatch::Command(binding), &mut outcome);

    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("expected endpoint command invocation");
    };
    let crate::api::schema::Method::CommandInvoke(params) = &request.method else {
        panic!("expected command.invoke");
    };
    assert_eq!(
        params.selection,
        Some(crate::api::schema::PaneSelectionReadParams {
            pane_id: "pane_1".into(),
            anchor: crate::api::schema::PaneTextPoint { row: 2, col: 3 },
            cursor: crate::api::schema::PaneTextPoint { row: 4, col: 5 },
            content_revision: Some(42),
        })
    );
}

#[test]
fn unavailable_endpoint_method_is_disabled_without_disconnect() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(vec!["pane.focus".into()]));
    let mut outcome = ClientShellInput::default();

    state.push_endpoint_method(
        crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "missing".into(),
        }),
        &mut outcome,
    );

    assert!(outcome.actions.is_empty());
    assert!(outcome.repaint);
    let notice = state
        .visible_endpoint_notice
        .as_ref()
        .expect("unsupported action notice");
    assert_eq!(notice.key.kind, ClientEndpointNoticeKind::Unsupported);
    assert_eq!(notice.key.code, "workspace.focus");
    assert!(notice.body.contains("This server"));
    assert!(state.endpoint_error.is_none());

    let mut repeated = ClientShellInput::default();
    state.push_endpoint_method(
        crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "missing".into(),
        }),
        &mut repeated,
    );
    assert!(repeated.actions.is_empty());
    assert!(!repeated.repaint);

    state.compose(106, 20).expect("endpoint notice frame");
    assert!(!state.hits.notification_toast.is_empty());

    state.handle_input_bytes(b"x");
    assert!(state.visible_endpoint_notice.is_some());

    let hit = state.hits.notification_toast;
    let dismissed =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.x,
            row: hit.y,
            modifiers: KeyModifiers::empty(),
        })]);
    assert!(dismissed.repaint);
    assert!(state.visible_endpoint_notice.is_none());
}

#[test]
fn generic_endpoint_failures_and_control_errors_are_visible() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut outcome = ClientShellInput::default();
    state.push_endpoint_method(
        crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "missing".into(),
        }),
        &mut outcome,
    );
    let request_id = match &outcome.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        other => panic!("expected generic endpoint request, got {other:?}"),
    };
    let rejected_at = std::time::Instant::now();
    let (repaint, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Err(ClientShellEndpointError {
            code: Some("not_found".into()),
            message: "workspace no longer exists".into(),
        }),
    );
    assert!(repaint);
    assert!(actions.is_empty());
    let notice = state
        .visible_endpoint_notice
        .as_ref()
        .expect("rejected action notice");
    assert_eq!(notice.key.kind, ClientEndpointNoticeKind::Rejected);
    assert_eq!(notice.key.code, "workspace.focus:not_found");
    assert_eq!(notice.body, "workspace no longer exists");
    let observed_at = std::time::Instant::now();
    assert!(notice.deadline >= rejected_at + std::time::Duration::from_secs(3));
    assert!(notice.deadline <= observed_at + std::time::Duration::from_secs(3));
    assert!(state.endpoint_error.is_none());

    assert!(state.receive_endpoint_error("Paste rejected: too large".into()));
    let notice = state
        .visible_endpoint_notice
        .as_ref()
        .expect("paste rejection notice");
    assert_eq!(notice.key.kind, ClientEndpointNoticeKind::Rejected);
    assert_eq!(notice.key.code, "paste_rejected");
    assert_eq!(notice.body, "Paste rejected: too large");
    assert!(state.receive_endpoint_error("Paste rejected again".into()));
    assert_eq!(
        state
            .visible_endpoint_notice
            .as_ref()
            .map(|notice| notice.body.as_str()),
        Some("Paste rejected again")
    );
}

#[test]
fn endpoint_timeout_is_a_deduplicated_server_notice() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut outcome = ClientShellInput::default();
    state.push_endpoint_method(
        crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "work".into(),
        }),
        &mut outcome,
    );
    let request_id = match &outcome.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        other => panic!("expected endpoint request, got {other:?}"),
    };

    let (repaint, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Err(ClientShellEndpointError {
            code: Some("endpoint_timeout".into()),
            message: "transport timeout".into(),
        }),
    );

    assert!(repaint);
    assert!(actions.is_empty());
    let notice = state
        .visible_endpoint_notice
        .as_ref()
        .expect("timeout notice");
    assert_eq!(notice.key.kind, ClientEndpointNoticeKind::Timeout);
    assert_eq!(notice.key.code, "workspace.focus");
    assert!(notice.body.contains("workspace.focus"));
    assert!(state.endpoint_error.is_none());
}

#[test]
fn endpoint_notice_dedupe_resets_for_a_new_server_boot() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_endpoint_methods(Some(Vec::new()));
    let method = || {
        crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "work".into(),
        })
    };
    let mut first = ClientShellInput::default();
    state.push_endpoint_method(method(), &mut first);
    assert!(first.repaint);

    let mut next_snapshot = snapshot();
    next_snapshot.boot_id = "boot-2".into();
    state.set_snapshot(Box::new(next_snapshot));
    let mut second = ClientShellInput::default();
    state.push_endpoint_method(method(), &mut second);

    assert!(second.repaint);
    assert_eq!(
        state
            .visible_endpoint_notice
            .as_ref()
            .map(|notice| notice.key.boot_id.as_str()),
        Some("boot-2")
    );
}

#[test]
fn custom_binding_missing_from_endpoint_manifest_is_not_forwarded() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let binding = crate::config::CustomCommandKeybind {
        bindings: crate::config::ActionKeybinds::prefix("z"),
        label: "prefix+z".into(),
        command: "secret-command".into(),
        action: crate::config::CustomCommandAction::Shell,
        description: None,
        width: None,
        height: None,
    };

    let mut outcome = ClientShellInput::default();
    state.record_binding(crate::input::KeybindMatch::Command(binding), &mut outcome);

    assert!(outcome.actions.is_empty());
    assert!(outcome.repaint);
    assert!(state
        .endpoint_error
        .as_deref()
        .is_some_and(|error| error.contains("not available")));
}

#[test]
fn help_overlay_restores_released_search_scroll_and_custom_binding_behavior() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut projection = snapshot();
    projection
        .commands
        .push(crate::protocol::ClientShellCommand {
            command_id: "plugin-action".into(),
            binding_label: "prefix+z".into(),
            binding_labels: vec!["prefix+z".into()],
            action: crate::protocol::ClientShellCommandAction::PluginAction,
            description: Some("run plugin action".into()),
        });
    state.set_snapshot(Box::new(projection));
    state.set_pane_surface(surface());
    let mut open = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::Help),
        &mut open,
    );
    let initial = state.compose(106, 30).expect("help overlay");
    let text = initial
        .cells
        .chunks(initial.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("global"));
    assert!(state.hits.help_max_scroll > 0);
    assert_ne!(state.hits.help_scrollbar, Rect::default());

    state.handle_input_bytes(b"/");
    state.handle_input_bytes(b"plugin");
    let custom = state.compose(106, 30).expect("custom help search");
    let text = custom
        .cells
        .chunks(custom.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("custom"));
    assert!(text.contains("run plugin action"));
    state.handle_input_bytes(b"\x1b");

    state.handle_input_bytes(b"/");
    state.handle_input_bytes(b"does-not-exist");
    let empty = state.compose(106, 30).expect("empty help search");
    let text = empty
        .cells
        .chunks(empty.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("no matching keybinds"));

    state.handle_input_bytes(b"\x1b");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Help(ClientHelpOverlay {
            search_focused: false,
            ref query,
            scroll: 0,
        })) if query.is_empty()
    ));
    state.compose(106, 30).expect("restored help");
    state.handle_input_bytes(b"\x1b[F");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Help(ClientHelpOverlay { scroll, .. }))
            if scroll == state.hits.help_max_scroll
    ));
    state.handle_input_bytes(b"\x1b[H");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Help(ClientHelpOverlay {
            scroll: 0,
            ..
        }))
    ));
    state.handle_input_bytes(b"?");
    assert!(state.overlay.is_none());
}

#[test]
fn resize_mode_reuses_endpoint_resize_and_stays_active_until_done() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());

    assert!(state.handle_input_bytes(&[0x02]).actions.is_empty());
    assert!(state.handle_input_bytes(b"r").actions.is_empty());
    assert_eq!(state.mode, ClientShellMode::Resize);

    let modified = state.handle_input_bytes(b"\x1b[1;2D");
    assert!(matches!(
        &modified.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::PaneResize(params)
                    if params.direction == crate::api::schema::PaneDirection::Left
            )
    ));
    assert_eq!(state.mode, ClientShellMode::Resize);

    let resize = state.handle_input_bytes(b"h");
    let [ClientShellAction::Endpoint { request, .. }] = &resize.actions[..] else {
        panic!("resize should use endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneResize(params)
            if params.pane_id.as_deref() == Some("pane_1")
                && params.direction == crate::api::schema::PaneDirection::Left
    ));
    assert_eq!(state.mode, ClientShellMode::Resize);

    assert!(state.handle_input_bytes(b"\r").actions.is_empty());
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn fork_client_settings_header_retains_product_version() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.open_settings_overlay();
    let frame = state.compose(106, 30).unwrap();
    let text = frame
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(text.contains(&crate::build_info::version()));
}

#[test]
fn current_tab_agent_mode_excludes_other_tabs_in_client() {
    let mut snap = snapshot();
    let first = ClientShellAgent {
        pane_id: "pane_1".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: None,
        display_agent: None,
        agent: Some("pi".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Idle,
        state_change_seq: 0,
        state_labels: vec![],
        tokens: vec![],
        focused: true,
    };
    snap.agents.push(first.clone());
    let mut other = first.clone();
    other.pane_id = "other-pane".into();
    other.tab_id = "other-tab".into();
    snap.focused_tab_id = Some(first.tab_id.clone());
    snap.agents.push(other);
    let ids = super::super::agent_sidebar::ordered_agent_pane_ids(
        &snap,
        crate::config::AgentPanelSortConfig::CurrentTab,
    );
    assert_eq!(ids, vec![first.pane_id]);
}

fn harness_integration_fixture(
    id: &str,
    active: bool,
) -> crate::api::schema::HarnessIntegrationInfo {
    crate::api::schema::HarnessIntegrationInfo {
        id: id.into(),
        label: format!("{id} tool"),
        command: id.into(),
        active,
        available: true,
        hook_target: None,
        hook_state: None,
        installer: None,
        profile_instructions: false,
        capabilities: Default::default(),
    }
}

#[test]
fn terminal_harness_picker_and_profiles_follow_activation_including_unfamiliar_ids() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let mut future = harness_integration_fixture("future-cli", true);
    future.capabilities.model = true;
    state.handle_harness_integration_result(Ok(
        crate::api::schema::ResponseResult::HarnessIntegrationList {
            integrations: vec![future, harness_integration_fixture("disabled-cli", false)],
        },
    ));
    state.open_settings_overlay();
    if let Some(ClientShellOverlay::Settings(settings)) = state.overlay.as_mut() {
        settings.section = ClientSettingsSection::Agents;
    }
    state.activate_native_profile(&mut ClientShellInput::default());
    let Some(ClientShellOverlay::Settings(settings)) = state.overlay.as_mut() else {
        panic!("profile form")
    };
    let form = settings.profile_form.as_mut().unwrap();
    assert_eq!(form.harness, "future-cli");
    assert_eq!(form.harness_choices, ["future-cli"]);
    form.existing_role = Some("future-profile".into());
    assert!(form.field_enabled(2));
    assert!(!form.field_enabled(3));
    state.handle_native_profile_result(
        PendingEndpointKind::NativeHarnessPrepare {
            pane_id: "pane_1".into(),
        },
        Ok(crate::api::schema::ResponseResult::AgentHarnessSet {
            instance_id: "reviewer:1".into(),
            harness_override: Some("future-cli".into()),
        }),
    );
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("harness menu")
    };
    assert_eq!(
        menu.items()
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        ["Profile harness (switch now)", "future-cli"]
    );
    let mut outcome = ClientShellInput::default();
    state.activate_context_menu_item(1, &mut outcome);
    let [ClientShellAction::Endpoint { request, .. }] = outcome.actions.as_slice() else {
        panic!("typed harness selection")
    };
    assert!(matches!(
        request.method,
        crate::api::schema::Method::AgentRosterList(_)
    ));
    assert!(state.live_harness_switch.is_some());
    state.handle_harness_integration_result(Ok(
        crate::api::schema::ResponseResult::HarnessIntegrationSet {
            integration: harness_integration_fixture("future-cli", false),
        },
    ));
    assert!(state.active_harness_choices().is_empty());
}

fn integrations_navigation_fixture(legacy: bool) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(if legacy {
        vec![
            "integration.list".into(),
            "integration.install".into(),
            "agent.harness.prepare".into(),
            "agent.harness.set".into(),
        ]
    } else {
        vec![
            "integration.list".into(),
            "integration.install".into(),
            "integration.harness.list".into(),
            "integration.harness.set".into(),
            "agent.harness.prepare".into(),
            "agent.harness.set".into(),
        ]
    }));
    state.open_settings_overlay();
    if let Some(ClientShellOverlay::Settings(settings)) = state.overlay.as_mut() {
        settings.section = ClientSettingsSection::Integrations;
        if legacy {
            settings.integrations = vec![
                crate::api::schema::IntegrationInfo {
                    target: crate::api::schema::IntegrationTarget::Pi,
                    label: "pi".into(),
                    command: "pi".into(),
                    available: true,
                    state: crate::api::schema::IntegrationState::Current,
                },
                crate::api::schema::IntegrationInfo {
                    target: crate::api::schema::IntegrationTarget::Omp,
                    label: "omp".into(),
                    command: "omp".into(),
                    available: true,
                    state: crate::api::schema::IntegrationState::NotInstalled,
                },
            ];
        }
    }
    if !legacy {
        state.handle_harness_integration_result(Ok(
            crate::api::schema::ResponseResult::HarnessIntegrationList {
                integrations: (0..40)
                    .map(|index| harness_integration_fixture(&format!("cli-{index}"), true))
                    .collect(),
            },
        ));
    }
    state.compose(120, 30).unwrap();
    state
}

#[test]
fn integrations_legacy_keyboard_selection_is_visible_and_clickable() {
    let mut state = integrations_navigation_fixture(true);
    state.handle_input_bytes(b"\x1b[B");
    let frame = state.compose(120, 30).unwrap();
    let row = state
        .hits
        .settings_choices
        .iter()
        .find(|(_, index)| *index == 1)
        .expect("legacy integration rows must be clickable")
        .0;
    let index = row.y as usize * frame.width as usize + row.x as usize;
    assert_eq!(
        frame.cells[index].bg,
        crate::protocol::color_to_u32(state.config.palette.surface0)
    );
    let first = state
        .hits
        .settings_choices
        .iter()
        .find(|(_, index)| *index == 0)
        .unwrap()
        .0;
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: first.x + 1,
        row: first.y,
        modifiers: KeyModifiers::empty(),
    })]);
    let Some(ClientShellOverlay::Settings(settings)) = state.overlay.as_ref() else {
        panic!("settings remain open")
    };
    assert_eq!(settings.selected, 0);
}

#[test]
fn integrations_legacy_enter_installs_only_selected_hook() {
    let mut state = integrations_navigation_fixture(true);
    state.handle_input_bytes(b"\x1b[B");
    let outcome = state.handle_input_bytes(b"\r");
    let [ClientShellAction::Endpoint { request, .. }] = outcome.actions.as_slice() else {
        panic!("one selected hook request")
    };
    assert!(matches!(
        request.method,
        crate::api::schema::Method::IntegrationInstall(
            crate::api::schema::IntegrationInstallParams {
                target: crate::api::schema::IntegrationTarget::Omp,
            }
        )
    ));
}

#[test]
fn integrations_native_and_legacy_support_mouse_wheel_selection() {
    for legacy in [false, true] {
        let mut state = integrations_navigation_fixture(legacy);
        let popup = state.hits.settings_popup;
        state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: popup.x + 5,
            row: popup.y + 7,
            modifiers: KeyModifiers::empty(),
        })]);
        let Some(ClientShellOverlay::Settings(settings)) = state.overlay.as_ref() else {
            panic!("settings")
        };
        assert_eq!(
            settings.selected, 1,
            "wheel down must move selection for legacy={legacy}"
        );
        state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: popup.x + 5,
            row: popup.y + 7,
            modifiers: KeyModifiers::empty(),
        })]);
        let Some(ClientShellOverlay::Settings(settings)) = state.overlay.as_ref() else {
            panic!("settings")
        };
        assert_eq!(settings.selected, 0);
    }
}

#[test]
fn integrations_legacy_hook_list_reaches_last_row_in_small_terminal() {
    let mut state = integrations_navigation_fixture(true);
    let statuses = crate::api::schema::IntegrationTarget::ALL
        .iter()
        .map(|target| crate::api::schema::IntegrationInfo {
            target: *target,
            label: format!("{target:?}"),
            command: "tool".into(),
            available: true,
            state: crate::api::schema::IntegrationState::NotInstalled,
        })
        .collect::<Vec<_>>();
    if let Some(ClientShellOverlay::Settings(settings)) = state.overlay.as_mut() {
        settings.integrations = statuses;
    }
    for _ in 1..crate::api::schema::IntegrationTarget::ALL.len() {
        state.handle_input_bytes(b"\x1b[B");
    }
    let frame = state.compose(106, 20).expect("clamped integrations popup");
    let Some((rect, selected)) = state
        .hits
        .settings_choices
        .iter()
        .find(|(_, index)| *index == crate::api::schema::IntegrationTarget::ALL.len() - 1)
    else {
        panic!("last hook row must scroll into view")
    };
    assert_eq!(
        *selected,
        crate::api::schema::IntegrationTarget::ALL.len() - 1
    );
    let text = frame
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(text.contains("Grok"));
    assert!(rect.y >= state.hits.settings_popup.y);
}

#[test]
fn integrations_missing_catalog_reports_runtime_upgrade_instead_of_empty_picker() {
    let mut state = integrations_navigation_fixture(true);
    let frame = state.compose(120, 30).unwrap();
    let text = frame
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(
        text.contains("runtime upgrade required"),
        "the old-server fallback must explain missing activation"
    );
    state.cancel_settings_overlay();
    let mut outcome = ClientShellInput::default();
    state.prepare_native_harness("pane_1".into(), &mut outcome);
    assert!(
        outcome.actions.is_empty(),
        "old servers cannot provide an activation-backed picker"
    );
    assert!(state
        .endpoint_error
        .as_deref()
        .is_some_and(|error| error.contains("running server") && error.contains("handoff")));
    assert!(!matches!(
        state.overlay,
        Some(ClientShellOverlay::ContextMenu(_))
    ));
}

#[test]
fn terminal_integration_lists_keep_the_last_selection_visible_and_mouse_addressable() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.handle_harness_integration_result(Ok(
        crate::api::schema::ResponseResult::HarnessIntegrationList {
            integrations: (0..45)
                .map(|index| harness_integration_fixture(&format!("cli-{index}"), true))
                .collect(),
        },
    ));
    state.open_settings_overlay();
    if let Some(ClientShellOverlay::Settings(settings)) = state.overlay.as_mut() {
        settings.section = ClientSettingsSection::Integrations;
        settings.selected = 44;
    }
    let frame = state.compose(120, 24).unwrap();
    let text = frame
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(text.contains("cli-44 tool"));
    assert!(state
        .hits
        .settings_choices
        .iter()
        .any(|(_, index)| *index == 44));
    state.handle_native_profile_result(
        PendingEndpointKind::NativeHarnessPrepare {
            pane_id: "pane_1".into(),
        },
        Ok(crate::api::schema::ResponseResult::AgentHarnessSet {
            instance_id: "reviewer:1".into(),
            harness_override: Some("cli-44".into()),
        }),
    );
    let frame = state.compose(120, 16).unwrap();
    let text = frame
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(text.contains("cli-44"));
    assert!(state
        .hits
        .context_menu_rows
        .iter()
        .any(|(_, index)| *index == 45));
}

#[test]
fn native_profile_client_edits_multiline_documents_and_saves_typed_api_patch() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.open_settings_overlay();
    let profile = crate::api::schema::AgentProfileInfo {
        role: "reviewer".into(),
        native_cwd: "/repo".into(),
        native_cwd_seeded: true,
        mds: Vec::new(),
        harness: "pi".into(),
        model: None,
        effort: None,
        apikey_ref: None,
        allowlist: None,
        replicas_assigned: 0,
        created_at: 0,
        last_spawned_at: None,
    };
    if let Some(ClientShellOverlay::Settings(settings)) = state.overlay.as_mut() {
        settings.section = ClientSettingsSection::Agents;
    }
    state.handle_native_profile_result(
        PendingEndpointKind::NativeProfileLoad,
        Ok(crate::api::schema::ResponseResult::AgentProfileDossier {
            profile,
            documents: [("AGENTS.md".into(), "first\nsecond".into())]
                .into_iter()
                .collect(),
        }),
    );
    assert!(state.insert_overlay_text("\nthird"));
    let frame = state.compose(120, 40).unwrap();
    let text = frame
        .cells
        .iter()
        .map(|c| c.symbol.as_str())
        .collect::<String>();
    assert!(text.contains("third"));
    assert!(frame
        .cells
        .iter()
        .any(|cell| cell.modifier & Modifier::REVERSED.bits() != 0));
    let mut save = ClientShellInput::default();
    state.save_native_profile(&mut save);
    let [ClientShellAction::Endpoint { request, .. }] = save.actions.as_slice() else {
        panic!("save routes through native endpoint API")
    };
    let crate::api::schema::Method::AgentProfileEdit(params) = &request.method else {
        panic!("typed dossier save")
    };
    assert_eq!(params.documents["AGENTS.md"], "first\nsecond\nthird");
    let before = match state.overlay.as_ref() {
        Some(ClientShellOverlay::Settings(s)) => s.profile_form.clone(),
        _ => None,
    };
    state.handle_native_profile_result(
        PendingEndpointKind::NativeProfileSave,
        Err(ClientShellEndpointError {
            code: Some("agent_profile_persist_failed".into()),
            message: "disk unavailable".into(),
        }),
    );
    match state.overlay.as_ref() {
        Some(ClientShellOverlay::Settings(s)) => assert_eq!(s.profile_form, before),
        _ => panic!("failed save keeps draft"),
    };
}

fn saved_profile_click_fixture() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut snap = snapshot();
    snap.tabs[0].tab_id = "tab_in_selected_space".into();
    snap.focused_tab_id = Some("tab_in_selected_space".into());
    snap.workspaces[0].active_tab_id = "tab_in_selected_space".into();
    snap.panes[0].tab_id = "tab_in_selected_space".into();
    state.set_snapshot(Box::new(snap));
    state.set_pane_surface(surface());
    state.config.agent_panel_sort = crate::config::AgentPanelSortConfig::Agents;
    state.sidebar_width = 36;
    state.sidebar_width_manual = true;
    state.handle_native_profile_result(
        PendingEndpointKind::NativeProfiles,
        Ok(crate::api::schema::ResponseResult::AgentProfileList {
            profiles: vec![crate::api::schema::AgentProfileInfo {
                role: "reviewer".into(),
                native_cwd: "/different/native-cwd".into(),
                native_cwd_seeded: true,
                mds: Vec::new(),
                harness: "pi".into(),
                model: None,
                effort: None,
                apikey_ref: None,
                allowlist: None,
                replicas_assigned: 0,
                created_at: 0,
                last_spawned_at: None,
            }],
        }),
    );
    state.compose(106, 30).unwrap();
    state
}

#[test]
fn saved_profile_sidebar_click_restores_space_and_native_cwd_spawn_actions() {
    for (index, cwd_mode) in [(0, "tab"), (1, "agent")] {
        let mut state = saved_profile_click_fixture();
        let rect = state.hits.saved_profiles[0].0;
        let click = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 1,
            row: rect.y,
            modifiers: KeyModifiers::empty(),
        })]);
        assert!(
            click.actions.is_empty(),
            "opening the launch menu must not request the settings editor"
        );
        let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
            panic!("saved agent click should open its launch menu")
        };
        let labels = menu
            .items()
            .into_iter()
            .map(|item| item.label)
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            [
                "Use space cwd",
                "Use agent native cwd",
                "Native harness...",
                "Edit profile",
                "Delete profile"
            ]
        );
        state.compose(106, 30).unwrap();
        let row = state.hits.context_menu_rows[index].0;
        let selected = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: row.x + 1,
            row: row.y,
            modifiers: KeyModifiers::empty(),
        })]);
        let [ClientShellAction::Endpoint { request, .. }] = selected.actions.as_slice() else {
            panic!("one spawn request")
        };
        let crate::api::schema::Method::AgentSpawn(params) = &request.method else {
            panic!("saved agent must spawn, not load its editor")
        };
        assert_eq!(params.role, "reviewer");
        assert_eq!(params.tab_id.as_deref(), Some("tab_in_selected_space"));
        assert_eq!(params.cwd_mode, cwd_mode);
        assert_eq!(
            params.kind, None,
            "launch settings must come from the saved profile"
        );
        assert!(state.overlay.is_none());
    }
}

#[test]
fn saved_profile_sidebar_edit_actions_are_explicit_and_do_not_spawn() {
    for (index, expected_field) in [(2, 0), (3, 7)] {
        let mut state = saved_profile_click_fixture();
        let rect = state.hits.saved_profiles[0].0;
        state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: rect.x + 1,
            row: rect.y,
            modifiers: KeyModifiers::empty(),
        })]);
        assert!(matches!(
            state.overlay,
            Some(ClientShellOverlay::ContextMenu(_))
        ));
        let mut outcome = ClientShellInput::default();
        state.activate_context_menu_item(index, &mut outcome);
        let [ClientShellAction::Endpoint { request, .. }] = outcome.actions.as_slice() else {
            panic!("explicit profile edit")
        };
        let crate::api::schema::Method::AgentProfileEdit(params) = &request.method else {
            panic!("editing must not spawn an agent")
        };
        assert_eq!(params.role, "reviewer");
        let profile = state.native_profiles[0].clone();
        state.handle_native_profile_result(
            if index == 2 {
                PendingEndpointKind::NativeProfileHarness
            } else {
                PendingEndpointKind::NativeProfileLoad
            },
            Ok(crate::api::schema::ResponseResult::AgentProfileDossier {
                profile,
                documents: [("AGENTS.md".into(), "Instructions".into())]
                    .into_iter()
                    .collect(),
            }),
        );
        let Some(ClientShellOverlay::Settings(settings)) = state.overlay.as_ref() else {
            panic!("explicit editor")
        };
        assert_eq!(
            settings.profile_form.as_ref().unwrap().selected_field,
            expected_field
        );
    }
}

#[test]
fn live_harness_menu_highlights_the_running_cli_instead_of_a_saved_override() {
    let mut projected = snapshot();
    projected.agents.push(ClientShellAgent {
        pane_id: "pane_1".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: Some("reviewer".into()),
        display_agent: None,
        agent: Some("pi".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Idle,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: true,
    });
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.harness_integrations = vec![
        harness_integration_fixture("pi", true),
        harness_integration_fixture("droid", true),
    ];
    state.handle_native_profile_result(
        PendingEndpointKind::NativeHarnessPrepare {
            pane_id: "pane_1".into(),
        },
        Ok(crate::api::schema::ResponseResult::AgentHarnessSet {
            instance_id: "reviewer:1".into(),
            harness_override: Some("droid".into()),
        }),
    );
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("live harness menu")
    };
    assert_eq!(menu.highlighted, 1);
}

#[test]
fn choosing_live_harness_starts_portable_handoff_without_changing_the_running_agent() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.harness_integrations = vec![harness_integration_fixture("droid", true)];
    state.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
        target: ClientContextMenuTarget::AgentHarness {
            instance_id: "reviewer:1".into(),
            harnesses: vec!["pi".into(), "droid".into()],
        },
        x: 0,
        y: 0,
        highlighted: 2,
    }));
    let mut outcome = ClientShellInput::default();
    state.activate_context_menu_item(2, &mut outcome);
    let [ClientShellAction::Endpoint { request, .. }] = outcome.actions.as_slice() else {
        panic!("one handoff preflight request")
    };
    assert!(matches!(
        request.method,
        crate::api::schema::Method::AgentRosterList(_)
    ));
    assert!(state.overlay.is_none());
}

#[test]
fn portable_handoff_refuses_busy_source_without_changing_any_pane() {
    use crate::api::schema::*;
    let mut projected = snapshot();
    projected.agents.push(ClientShellAgent {
        pane_id: "pane_1".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: Some("reviewer".into()),
        display_agent: None,
        agent: Some("pi".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Working,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: true,
    });
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.harness_integrations = vec![harness_integration_fixture("droid", true)];
    state.begin_live_harness_switch(
        "reviewer:1".into(),
        Some("droid".into()),
        &mut ClientShellInput::default(),
    );
    let (_, actions) = state.handle_live_harness_switch_result(
        "reviewer:1".into(),
        HarnessSwitchStep::Roster,
        Ok(ResponseResult::AgentRosterList {
            entries: vec![AgentRosterEntryInfo {
                instance_id: "reviewer:1".into(),
                profile_id: "reviewer".into(),
                role: "reviewer".into(),
                replica_suffix: String::new(),
                display_name: "reviewer".into(),
                status: AgentRosterStatus::Working,
                harness_override: None,
                last_pane: Some("pane_1".into()),
                last_seen_at: None,
            }],
        }),
    );
    assert!(actions.is_empty());
    assert!(state.live_harness_switch.is_none());
    assert!(state
        .endpoint_error
        .as_deref()
        .is_some_and(|error| error.contains("working")));
}

#[test]
fn portable_handoff_keeps_original_until_new_agent_accepts_context() {
    use crate::api::schema::*;
    let mut projected = snapshot();
    projected.agents.push(ClientShellAgent {
        pane_id: "pane_1".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: Some("reviewer".into()),
        display_agent: None,
        agent: Some("pi".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Idle,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: true,
    });
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.harness_integrations = vec![harness_integration_fixture("droid", true)];
    let mut started = ClientShellInput::default();
    state.begin_live_harness_switch("reviewer:1".into(), Some("droid".into()), &mut started);
    assert_eq!(started.actions.len(), 1);
    let roster = ResponseResult::AgentRosterList {
        entries: vec![AgentRosterEntryInfo {
            instance_id: "reviewer:1".into(),
            profile_id: "reviewer".into(),
            role: "reviewer".into(),
            replica_suffix: String::new(),
            display_name: "reviewer".into(),
            status: AgentRosterStatus::Active,
            harness_override: None,
            last_pane: Some("pane_1".into()),
            last_seen_at: None,
        }],
    };
    let (_, context) = state.handle_live_harness_switch_result(
        "reviewer:1".into(),
        HarnessSwitchStep::Roster,
        Ok(roster),
    );
    let [ClientShellAction::Endpoint { request, .. }] = context.as_slice() else {
        panic!("saved profile context read")
    };
    assert!(
        matches!(&request.method, Method::AgentHarnessContext(AgentHarnessContextParams { instance_id }) if instance_id == "reviewer:1")
    );
    let progress = state.compose(120, 30).unwrap();
    assert!(progress
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>()
        .contains("Swit"));
    let (_, read) = state.handle_live_harness_switch_result(
        "reviewer:1".into(),
        HarnessSwitchStep::Context,
        Ok(ResponseResult::AgentHarnessContext {
            harness: "pi".into(),
            documents: [("AGENTS.md".into(), "Saved directions".into())]
                .into_iter()
                .collect(),
        }),
    );
    let [ClientShellAction::Endpoint { request, .. }] = read.as_slice() else {
        panic!("bounded transcript read")
    };
    assert!(
        matches!(&request.method, Method::PaneRead(PaneReadParams { pane_id, lines: Some(200), .. }) if pane_id == "pane_1")
    );
    let (_, spawn) = state.handle_live_harness_switch_result(
        "reviewer:1".into(),
        HarnessSwitchStep::Read,
        Ok(ResponseResult::PaneRead {
            read: PaneReadResult {
                pane_id: "pane_1".into(),
                workspace_id: "ws_1".into(),
                tab_id: "tab_1".into(),
                source: ReadSource::Recent,
                format: ReadFormat::Text,
                text: "prior work and instructions".into(),
                revision: 1,
                truncated: false,
            },
        }),
    );
    let [ClientShellAction::Endpoint { request, .. }] = spawn.as_slice() else {
        panic!("new harness spawn")
    };
    let Method::AgentSpawn(params) = &request.method else {
        panic!("agent spawn")
    };
    assert_eq!(params.kind.as_deref(), Some("droid"));
    assert_eq!(params.role, "reviewer");
    assert_eq!(params.tab_id.as_deref(), Some("tab_1"));
    assert_eq!(params.cwd_mode, "agent");
    let agent: AgentInfo = serde_json::from_value(serde_json::json!({
        "terminal_id":"term_new", "name":"reviewer-replica-1", "agent":"droid",
        "agent_status":"idle", "workspace_id":"ws_1", "tab_id":"tab_1", "pane_id":"pane_new",
        "focused":true, "revision":2, "interactive_ready":true
    }))
    .unwrap();
    let (_, ready) = state.handle_live_harness_switch_result(
        "reviewer:1".into(),
        HarnessSwitchStep::Spawn,
        Ok(ResponseResult::AgentSpawned {
            spawned: SpawnedAgentInfo {
                agent: agent.clone(),
                argv: vec!["droid".into()],
                name: "reviewer-replica-1".into(),
                pane_id: "pane_new".into(),
                split: false,
            },
        }),
    );
    assert!(
        matches!(&ready[0], ClientShellAction::Endpoint { request, .. } if matches!(request.method, Method::AgentReadyWait(_)))
    );
    let (_, prompt) = state.handle_live_harness_switch_result(
        "reviewer:1".into(),
        HarnessSwitchStep::Ready,
        Ok(ResponseResult::AgentInfo {
            agent: agent.clone(),
        }),
    );
    let [ClientShellAction::Endpoint { request, .. }] = prompt.as_slice() else {
        panic!("portable prompt")
    };
    let Method::AgentPrompt(params) = &request.method else {
        panic!("native prompt")
    };
    assert_eq!(params.target, "pane_new");
    assert!(params.text.contains("prior work and instructions"));
    assert!(params.text.contains("Saved directions"));
    assert!(params.text.contains("not a complete native session"));
    let failure = ClientShellEndpointError {
        code: Some("agent_prompt_failed".into()),
        message: "target refused context".into(),
    };
    let (_, after_failure) = state.handle_live_harness_switch_result(
        "reviewer:1".into(),
        HarnessSwitchStep::Prompt,
        Err(failure),
    );
    assert!(after_failure.is_empty());
    assert!(state.live_harness_switch.is_none());
    assert!(state
        .endpoint_error
        .as_deref()
        .is_some_and(|error| error.contains("Original session remains available")));
}

#[test]
fn switching_back_reuses_an_existing_native_session_and_carries_recent_context() {
    use crate::api::schema::*;
    let mut projected = snapshot();
    for (pane_id, tab_id, name, harness) in [
        ("pane_1", "tab_1", "reviewer-replica-1", "droid"),
        ("pane_old", "tab_2", "reviewer", "pi"),
    ] {
        projected.agents.push(ClientShellAgent {
            pane_id: pane_id.into(),
            workspace_id: "ws_1".into(),
            tab_id: tab_id.into(),
            name: Some(name.into()),
            display_agent: None,
            agent: Some(harness.into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: AgentStatus::Idle,
            state_change_seq: 2,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: pane_id == "pane_1",
        });
    }
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.harness_integrations = vec![harness_integration_fixture("pi", true)];
    state.begin_live_harness_switch(
        "new:2".into(),
        Some("pi".into()),
        &mut ClientShellInput::default(),
    );
    let (_, context) = state.handle_live_harness_switch_result(
        "new:2".into(),
        HarnessSwitchStep::Roster,
        Ok(ResponseResult::AgentRosterList {
            entries: vec![
                AgentRosterEntryInfo {
                    instance_id: "new:2".into(),
                    profile_id: "reviewer".into(),
                    role: "reviewer".into(),
                    replica_suffix: "-replica-1".into(),
                    display_name: "reviewer-replica-1".into(),
                    status: AgentRosterStatus::Idle,
                    harness_override: Some("droid".into()),
                    last_pane: Some("pane_1".into()),
                    last_seen_at: None,
                },
                AgentRosterEntryInfo {
                    instance_id: "old:1".into(),
                    profile_id: "reviewer".into(),
                    role: "reviewer".into(),
                    replica_suffix: String::new(),
                    display_name: "reviewer".into(),
                    status: AgentRosterStatus::Idle,
                    harness_override: None,
                    last_pane: Some("pane_old".into()),
                    last_seen_at: None,
                },
            ],
        }),
    );
    assert!(
        matches!(&context[0], ClientShellAction::Endpoint { request, .. } if matches!(request.method, Method::AgentHarnessContext(_)))
    );
    let (_, read) = state.handle_live_harness_switch_result(
        "new:2".into(),
        HarnessSwitchStep::Context,
        Ok(ResponseResult::AgentHarnessContext {
            harness: "pi".into(),
            documents: Default::default(),
        }),
    );
    assert!(
        matches!(&read[0], ClientShellAction::Endpoint { request, .. } if matches!(request.method, Method::PaneRead(_)))
    );
    let (_, ready) = state.handle_live_harness_switch_result(
        "new:2".into(),
        HarnessSwitchStep::Read,
        Ok(ResponseResult::PaneRead {
            read: PaneReadResult {
                pane_id: "pane_1".into(),
                workspace_id: "ws_1".into(),
                tab_id: "tab_1".into(),
                source: ReadSource::Recent,
                format: ReadFormat::Text,
                text: "new work in Droid".into(),
                revision: 3,
                truncated: false,
            },
        }),
    );
    let [ClientShellAction::Endpoint { request, .. }] = ready.as_slice() else {
        panic!("reused native session readiness")
    };
    assert!(
        matches!(&request.method, Method::AgentReadyWait(params) if params.target == "pane_old")
    );
    assert!(state.live_harness_switch.as_ref().unwrap().reuse_existing);
}

#[test]
fn portable_handoff_archives_exact_source_only_after_context_delivery_and_override() {
    use crate::api::schema::*;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.live_harness_switch = Some(LiveHarnessSwitch {
        instance_id: "old:1".into(),
        target: Some("droid".into()),
        endpoint_id: ClientEndpointId::Local,
        role: Some("reviewer".into()),
        old_pane: Some("pane_1".into()),
        tab_id: Some("tab_1".into()),
        previous_harness: Some("pi".into()),
        transcript: Some("prior work".into()),
        documents: Default::default(),
        new_pane: Some("pane_new".into()),
        reuse_existing: false,
        new_name: Some("reviewer-replica-1".into()),
        target_instance_id: None,
    });
    let agent: AgentInfo = serde_json::from_value(serde_json::json!({
        "terminal_id":"term_new", "name":"reviewer-replica-1", "agent":"droid",
        "agent_status":"idle", "workspace_id":"ws_1", "tab_id":"tab_1", "pane_id":"pane_new",
        "focused":true, "revision":2, "interactive_ready":true
    }))
    .unwrap();
    let (_, roster) = state.handle_live_harness_switch_result(
        "old:1".into(),
        HarnessSwitchStep::Prompt,
        Ok(ResponseResult::AgentPrompted { agent }),
    );
    assert!(
        matches!(&roster[0], ClientShellAction::Endpoint { request, .. } if matches!(request.method, Method::AgentRosterList(_)))
    );
    let (_, set) = state.handle_live_harness_switch_result(
        "old:1".into(),
        HarnessSwitchStep::NewRoster,
        Ok(ResponseResult::AgentRosterList {
            entries: vec![AgentRosterEntryInfo {
                instance_id: "new:2".into(),
                profile_id: "reviewer".into(),
                role: "reviewer".into(),
                replica_suffix: "-replica-1".into(),
                display_name: "reviewer-replica-1".into(),
                status: AgentRosterStatus::Idle,
                harness_override: None,
                last_pane: Some("pane_new".into()),
                last_seen_at: None,
            }],
        }),
    );
    let [ClientShellAction::Endpoint { request, .. }] = set.as_slice() else {
        panic!("exact new instance override")
    };
    assert!(
        matches!(&request.method, Method::AgentHarnessSet(params) if params.instance_id == "new:2" && params.harness.as_deref() == Some("droid"))
    );
    let (_, archive) = state.handle_live_harness_switch_result(
        "old:1".into(),
        HarnessSwitchStep::Override,
        Ok(ResponseResult::AgentHarnessSet {
            instance_id: "new:2".into(),
            harness_override: Some("droid".into()),
        }),
    );
    let [ClientShellAction::Endpoint { request, .. }] = archive.as_slice() else {
        panic!("archive exact original")
    };
    assert!(matches!(&request.method, Method::AgentHarnessCommit(params)
        if params.source_instance_id == "old:1" && params.source_pane_id == "pane_1" && params.target_instance_id == "new:2" && params.target_pane_id == "pane_new"));
}

#[test]
fn saved_profile_sidebar_menu_cannot_spawn_on_a_different_endpoint() {
    let mut state = saved_profile_click_fixture();
    let rect = state.hits.saved_profiles[0].0;
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    let remote = SavedSshEndpoint::new("Remote", "dev@remote.example", "agents").unwrap();
    state.active_endpoint_id = ClientEndpointId::Ssh(remote.id);
    let mut outcome = ClientShellInput::default();
    state.activate_context_menu_item(0, &mut outcome);
    assert!(outcome.actions.is_empty());
    assert!(state.overlay.is_none());
}

#[test]
fn saved_profile_sidebar_scrollbar_right_click_never_opens_a_launch_menu() {
    let mut state = saved_profile_click_fixture();
    let row = state.hits.saved_profiles[0].0;
    state.hits.agent_scrollbar = Rect::new(row.right() - 1, row.y, 1, 1);
    let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: row.right() - 1,
        row: row.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(outcome.actions.is_empty());
    assert!(state.overlay.is_none());
}

#[test]
fn saved_profile_settings_spawn_uses_activated_integration_count() {
    for count in [0, 1, 5] {
        let mut state = saved_profile_click_fixture();
        state.handle_harness_integration_result(Ok(
            crate::api::schema::ResponseResult::HarnessIntegrationList {
                integrations: (0..count)
                    .map(|index| harness_integration_fixture(&format!("tool-{index}"), true))
                    .collect(),
            },
        ));
        state.open_settings_overlay();
        if let Some(ClientShellOverlay::Settings(settings)) = state.overlay.as_mut() {
            settings.section = ClientSettingsSection::Agents;
            settings.selected = count;
        }
        let spawned = state.handle_input_bytes(b"s");
        let [ClientShellAction::Endpoint { request, .. }] = spawned.actions.as_slice() else {
            panic!(
                "spawn shortcut must address the selected saved profile for {count} integrations"
            )
        };
        let crate::api::schema::Method::AgentSpawn(params) = &request.method else {
            panic!("spawn API")
        };
        assert_eq!(params.role, "reviewer");
        assert_eq!(params.tab_id.as_deref(), Some("tab_in_selected_space"));
        assert_eq!(params.cwd_mode, "tab");
        assert!(
            state.overlay.is_none(),
            "spawning returns to the agent pane"
        );
    }
}

#[test]
fn saved_profile_roster_is_client_owned_and_excludes_live_pane_targets() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.config.agent_panel_sort = crate::config::AgentPanelSortConfig::Agents;
    state.sidebar_width = 36;
    state.sidebar_width_manual = true;
    let profile = crate::api::schema::AgentProfileInfo {
        role: "reviewer".into(),
        native_cwd: "/repo/reviewer".into(),
        native_cwd_seeded: true,
        mds: Vec::new(),
        harness: "pi".into(),
        model: None,
        effort: None,
        apikey_ref: None,
        allowlist: None,
        replicas_assigned: 0,
        created_at: 0,
        last_spawned_at: None,
    };
    state.handle_native_profile_result(
        PendingEndpointKind::NativeProfiles,
        Ok(crate::api::schema::ResponseResult::AgentProfileList {
            profiles: vec![profile],
        }),
    );
    let frame = state.compose(106, 30).unwrap();
    let text = frame
        .cells
        .iter()
        .map(|c| c.symbol.as_str())
        .collect::<String>();
    assert!(text.contains("reviewer (/repo/reviewer)"));
    assert!(state.hits.agents.is_empty());
    assert_eq!(state.hits.saved_profiles.len(), 1);
    state.reset_endpoint_projection();
    assert!(state.native_profiles.is_empty());
}
