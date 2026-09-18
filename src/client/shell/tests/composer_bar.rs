// Modified by ke: tests for the native composer bar (壳输入栏).
use super::super::composer::{
    COMPOSER_COLS, COMPOSER_ROWS, ComposerHook, ComposerReply, ComposerRequest, PENDING_LIMIT,
};
use super::*;

fn fake_send(request: &ComposerRequest<'_>) -> ComposerReply {
    ComposerReply::Send {
        text: format!("[safe]{}", request.text),
        note: Some("redacted".into()),
    }
}

fn fake_block(_request: &ComposerRequest<'_>) -> ComposerReply {
    ComposerReply::Block {
        note: "processor down".into(),
    }
}

fn toggle(state: &mut ClientShellState) -> ClientShellInput {
    let mut out = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::ToggleComposer),
        &mut out,
    );
    out
}

fn state_with_composer(agent: Option<&str>, hook: ComposerHook) -> ClientShellState {
    let mut snapshot = snapshot();
    if let Some(agent) = agent {
        snapshot.agents.push(crate::protocol::ClientShellAgent {
            pane_id: "pane_1".into(),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: None,
            display_agent: None,
            agent: Some(agent.into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: AgentStatus::Idle,
            state_change_seq: 0,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: true,
        });
    }
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot));
    state.composer_hook = hook;
    let out = toggle(&mut state);
    assert!(out.resize && state.composer.is_some());
    state
}

/// Presses Enter and lets a processor running off the input path finish, the way the main-loop timer would.
fn submit(state: &mut ClientShellState) -> ClientShellInput {
    let mut out = state.handle_input_bytes(b"\r");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while state.composer_pending.is_some() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(2));
        state.tick_composer(std::time::Instant::now(), &mut out);
    }
    assert!(state.composer_pending.is_none(), "processor reply settled");
    out
}

fn fake_slow(request: &ComposerRequest<'_>) -> ComposerReply {
    std::thread::sleep(std::time::Duration::from_millis(150));
    ComposerReply::Send {
        text: format!("[slow]{}", request.text),
        note: None,
    }
}

fn fake_hung(_request: &ComposerRequest<'_>) -> ComposerReply {
    std::thread::sleep(std::time::Duration::from_millis(400));
    ComposerReply::Send {
        text: "late".into(),
        note: None,
    }
}

fn pane_input_requests(out: &ClientShellInput) -> usize {
    out.requests
        .iter()
        .filter(|message| matches!(message, ClientMessage::ClientShellPaneInput { .. }))
        .count()
}

#[test]
fn toggle_reserves_the_right_column() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    assert!(state.composer.is_some(), "the dock is always on screen");
    let pane = state.layout(120, 40).pane_surface;
    let area = state.composer_area(120, 40).expect("dock");
    assert_eq!(area.width, COMPOSER_COLS);
    assert_eq!(area.x, pane.right());
    assert_eq!(area.height, pane.height);
    let closed_focus = state.composer.as_ref().unwrap().focused;
    assert!(!closed_focus);
    let out = toggle(&mut state);
    assert!(state.composer.as_ref().unwrap().focused);
    assert!(out.repaint);
    let area = state.composer_area(120, 40).expect("dock");
    assert_eq!(area.width, COMPOSER_COLS);
    let out = toggle(&mut state);
    assert!(!state.composer.as_ref().unwrap().focused);
    assert!(out.repaint);
}

#[test]
fn expanded_ke_chat_stays_inside_the_right_dock() {
    use super::super::ke_chat::ClientKeChatOverlay;
    let mut state = state_with_composer(None, fake_send);
    let idle = state.composer_area(120, 40).expect("dock");
    assert_eq!(idle.width, COMPOSER_COLS);
    state.set_composer_chat(ClientKeChatOverlay {
        title: "管家".into(),
        body: "codex 空着".into(),
        scroll: 0,
    });
    let open = state.composer_area(120, 40).expect("dock");
    assert_eq!(open, idle, "chat should expand inside the right dock");
    let mut out = ClientShellInput::default();
    state.toggle_composer_chat(&mut out);
    assert!(out.repaint);
    assert_eq!(state.composer_area(120, 40).expect("dock"), idle);
}

#[test]
fn typed_text_goes_to_the_composer_not_the_pane() {
    let mut state = state_with_composer(Some("claude"), fake_send);
    let out = state.handle_input_bytes(b"hi");
    assert_eq!(pane_input_requests(&out), 0);
    assert_eq!(
        state.composer.as_ref().map(|c| c.input.as_str()),
        Some("hi")
    );
}

#[test]
fn enter_submits_processed_text_to_an_agent_pane_with_agent_prompt() {
    let mut state = state_with_composer(Some("claude"), fake_send);
    state.handle_input_bytes(b"hi");
    let out = submit(&mut state);
    let [ClientShellAction::Endpoint { request, .. }] = &out.actions[..] else {
        panic!("expected exactly one endpoint request");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::AgentPrompt(params)
            if params.target == "pane_1" && params.text == "[safe]hi"
    ));
    assert_eq!(pane_input_requests(&out), 0);
    let composer = state.composer.as_ref().expect("composer stays open");
    assert!(composer.input.is_empty());
    assert_eq!(composer.notice.as_deref(), Some("redacted"));
}

#[test]
fn enter_on_a_plain_pane_sends_input_with_enter() {
    let mut state = state_with_composer(None, fake_send);
    state.handle_input_bytes(b"ls");
    let out = submit(&mut state);
    let [ClientShellAction::Endpoint { request, .. }] = &out.actions[..] else {
        panic!("expected exactly one endpoint request");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneSendInput(params)
            if params.pane_id == "pane_1" && params.text == "[safe]ls" && params.keys == vec!["Enter".to_string()]
    ));
}

#[test]
fn blocked_text_is_kept_and_nothing_is_sent() {
    let mut state = state_with_composer(Some("claude"), fake_block);
    state.handle_input_bytes(b"secret");
    let out = submit(&mut state);
    assert!(out.actions.is_empty());
    assert_eq!(pane_input_requests(&out), 0);
    let composer = state.composer.as_ref().expect("composer");
    assert_eq!(composer.input.as_str(), "secret");
    assert_eq!(composer.notice.as_deref(), Some("processor down"));
}

#[test]
fn enter_with_an_empty_composer_still_reaches_the_pane() {
    let mut state = state_with_composer(Some("claude"), fake_send);
    let out = state.handle_input_bytes(b"\r");
    assert!(out.actions.is_empty());
    assert_eq!(pane_input_requests(&out), 1);
}

#[test]
fn processor_reply_parsing_fails_closed() {
    assert_eq!(
        super::super::composer::parse_reply("not json", "x"),
        ComposerReply::Block {
            note: "处理进程返回的不是 JSON，未发送".into()
        }
    );
    assert_eq!(
        super::super::composer::parse_reply(r#"{"action":"send","text":"ok"}"#, "x"),
        ComposerReply::Send {
            text: "ok".into(),
            note: None
        }
    );
    assert_eq!(
        super::super::composer::parse_reply(r#"{"action":"done","note":"已交给常驻模型"}"#, "x"),
        ComposerReply::Done {
            note: Some("已交给常驻模型".into())
        }
    );
}

fn fake_done(_request: &ComposerRequest<'_>) -> ComposerReply {
    ComposerReply::Done {
        note: Some("已交给常驻模型".into()),
    }
}

#[test]
fn done_reply_clears_the_bar_and_sends_nothing() {
    let mut state = state_with_composer(Some("claude"), fake_done);
    state.handle_input_bytes(b"@ke check");
    let out = submit(&mut state);
    assert!(out.actions.is_empty());
    assert_eq!(pane_input_requests(&out), 0);
    let composer = state.composer.as_ref().expect("composer");
    assert!(composer.input.is_empty());
    assert_eq!(composer.notice.as_deref(), Some("已交给常驻模型"));
}

#[test]
fn slow_processor_completes_from_the_timer_and_keeps_new_typing() {
    let mut state = state_with_composer(Some("claude"), fake_slow);
    state.handle_input_bytes(b"slow");
    let out = state.handle_input_bytes(b"\r");
    assert!(out.actions.is_empty(), "nothing is sent before the reply");
    assert!(state.composer_pending.is_some());
    assert_eq!(
        state.composer.as_ref().unwrap().notice.as_deref(),
        Some("处理中…")
    );
    state.handle_input_bytes(b"x");
    let again = state.handle_input_bytes(b"\r");
    assert!(
        again.actions.is_empty(),
        "a second submit waits for the first"
    );
    let mut out = ClientShellInput::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while state.composer_pending.is_some() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
        state.tick_composer(std::time::Instant::now(), &mut out);
    }
    let [ClientShellAction::Endpoint { request, .. }] = &out.actions[..] else {
        panic!("expected exactly one endpoint request");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::AgentPrompt(params) if params.text == "[slow]slow"
    ));
    assert_eq!(
        state.composer.as_ref().unwrap().input.as_str(),
        "slowx",
        "text typed while processing is kept"
    );
}

fn click(state: &mut ClientShellState, column: u16, row: u16) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })])
}

#[test]
fn clicking_a_panel_row_with_input_opens_the_composer_prefilled() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let (panel, _) = super::super::ke_panel::parse_panel(
        r#"{"ts":1.0,"title":"ke","rows":[
            {"text":"claude ok","level":"ok"},
            {"text":"继续上一个任务","level":"info","input":"@ke 继续"}
        ]}"#,
    )
    .expect("panel");
    state.ke_panel.set_panel_for_test(Some(panel));
    state.compose(120, 40).expect("frame");
    let [(rect, input)] = &state.hits.ke_panel_rows[..] else {
        panic!(
            "exactly one clickable panel row, got {:?}",
            state.hits.ke_panel_rows
        );
    };
    assert_eq!(input, "@ke 继续");
    let (rect, plain_row) = (*rect, rect.y - 1);
    assert!(
        state.composer.as_ref().is_some_and(|c| !c.focused),
        "dock starts unfocused"
    );

    let out = click(&mut state, rect.x + 1, plain_row);
    assert!(
        !state.composer.as_ref().unwrap().focused,
        "display-only rows do nothing"
    );
    assert_eq!(pane_input_requests(&out), 0);

    let out = click(&mut state, rect.x + 1, rect.y);
    assert!(out.repaint, "the dock focuses");
    assert_eq!(pane_input_requests(&out), 0, "nothing is sent by the click");
    assert!(out.actions.is_empty());
    assert_eq!(
        state.composer.as_ref().map(|c| c.input.as_str()),
        Some("@ke 继续")
    );

    state.compose(120, 40).expect("frame with composer");
    let (rect, _) = state.hits.ke_panel_rows[0].clone();
    state.handle_input_bytes(b" now");
    let out = click(&mut state, rect.x + 1, rect.y);
    assert!(!out.resize, "already open: only the text changes");
    assert_eq!(
        state.composer.as_ref().map(|c| c.input.as_str()),
        Some("@ke 继续"),
        "a click replaces what was in the bar"
    );
}

#[test]
fn unresponsive_processor_times_out_as_a_block() {
    let mut state = state_with_composer(Some("claude"), fake_hung);
    state.handle_input_bytes(b"secret");
    state.handle_input_bytes(b"\r");
    assert!(state.composer_pending.is_some());
    let mut out = ClientShellInput::default();
    state.tick_composer(
        std::time::Instant::now() + PENDING_LIMIT + std::time::Duration::from_secs(1),
        &mut out,
    );
    assert!(out.actions.is_empty());
    assert!(state.composer_pending.is_none());
    let composer = state.composer.as_ref().unwrap();
    assert_eq!(composer.input.as_str(), "secret");
    assert_eq!(
        composer.notice.as_deref(),
        Some("处理进程无响应，未发送，原文保留")
    );
}

#[test]
fn ke_slash_help_stays_in_the_dock_and_sends_nothing() {
    let mut state = state_with_composer(Some("claude"), fake_send);
    state.handle_input_bytes(b"/ke");
    let out = submit(&mut state);
    assert!(out.actions.is_empty());
    assert_eq!(pane_input_requests(&out), 0);
    let composer = state.composer.as_ref().expect("composer");
    assert!(composer.input.is_empty());
    assert!(composer.chat_expanded);
    assert_eq!(
        composer.chat.as_ref().map(|chat| chat.title.as_str()),
        Some("壳命令")
    );
    assert!(
        composer
            .chat
            .as_ref()
            .is_some_and(|chat| chat.body.contains("/ke model"))
    );
}

#[test]
fn ke_slash_update_shows_the_install_command() {
    let mut state = state_with_composer(None, fake_send);
    state.handle_input_bytes(b"/ke update");
    let out = submit(&mut state);
    assert!(out.actions.is_empty());
    let composer = state.composer.as_ref().expect("composer");
    assert_eq!(
        composer.chat.as_ref().map(|chat| chat.title.as_str()),
        Some("壳更新")
    );
    assert!(composer.chat.as_ref().is_some_and(|chat| {
        chat.body.contains(crate::build_info::KE_VERSION) && chat.body.contains("ke-install")
    }));
}

#[test]
fn slash_escape_sends_the_rest_to_the_processor() {
    let mut state = state_with_composer(Some("claude"), fake_send);
    state.handle_input_bytes(b"//clear");
    let out = submit(&mut state);
    let [ClientShellAction::Endpoint { request, .. }] = &out.actions[..] else {
        panic!("expected exactly one endpoint request");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::AgentPrompt(params)
            if params.target == "pane_1" && params.text == "[safe]/clear"
    ));
}

#[test]
fn other_slash_commands_still_go_to_the_pane() {
    let mut state = state_with_composer(Some("claude"), fake_send);
    state.handle_input_bytes(b"/compact");
    let out = submit(&mut state);
    let [ClientShellAction::Endpoint { request, .. }] = &out.actions[..] else {
        panic!("expected exactly one endpoint request");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::AgentPrompt(params)
            if params.text == "[safe]/compact"
    ));
}

#[test]
fn ke_chat_binding_prefills_mention() {
    let mut state = state_with_composer(None, fake_send);
    let mut out = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::KeChat),
        &mut out,
    );
    assert_eq!(
        state
            .composer
            .as_ref()
            .map(|composer| composer.input.as_str()),
        Some("@ke ")
    );
    assert!(
        state
            .composer
            .as_ref()
            .is_some_and(|composer| composer.focused)
    );
}

#[test]
fn long_composer_text_grows_the_mobile_dock() {
    let mut state = state_with_composer(None, fake_send);
    state.handle_input_bytes(&[b'x'; 160]);
    let tall = state.composer_area(40, 40).expect("dock").height;
    assert!(
        tall > COMPOSER_ROWS,
        "wrapped input should reserve more than the idle two rows, got {tall}"
    );
    assert!(tall <= COMPOSER_ROWS + super::super::composer::INPUT_MAX_ROWS);
}

#[test]
fn double_slash_opens_a_command_palette() {
    let mut state = state_with_composer(Some("claude"), fake_send);
    let idle = state.composer_area(40, 40).expect("dock").height;
    state.handle_input_bytes(b"//");
    let tall = state.composer_area(40, 40).expect("dock").height;
    assert!(
        tall > idle,
        "typing // should grow the mobile dock for the slash palette, idle={idle} tall={tall}"
    );
    let desktop = state.composer_area(120, 40).expect("right dock");
    assert_eq!(desktop.width, COMPOSER_COLS);
}

#[test]
fn slash_palette_tab_completes_a_pane_command() {
    let mut state = state_with_composer(Some("claude"), fake_send);
    state.handle_input_bytes(b"//cl");
    let mut out = ClientShellInput::default();
    assert!(state.composer_handle_key(
        &crate::input::TerminalKey::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE
        ),
        &mut out
    ));
    assert_eq!(
        state
            .composer
            .as_ref()
            .map(|composer| composer.input.as_str()),
        Some("//clear")
    );
}

#[test]
fn ke_model_opens_the_form_overlay() {
    let mut state = state_with_composer(None, fake_send);
    state.handle_input_bytes(b"/ke model");
    let _ = submit(&mut state);
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
            section: ClientSettingsSection::KeModel,
            ..
        }))
    ));
}

#[test]
fn ke_model_plan_cycle_fills_ark() {
    let mut state = state_with_composer(None, fake_send);
    state.handle_input_bytes(b"/ke model");
    let _ = submit(&mut state);
    let mut out = ClientShellInput::default();
    assert!(state.route_settings_key(
        &crate::input::TerminalKey::new(
            crossterm::event::KeyCode::Right,
            crossterm::event::KeyModifiers::NONE
        ),
        &mut out
    ));
    let Some(ClientShellOverlay::Settings(settings)) = state.overlay.as_ref() else {
        panic!("model settings");
    };
    assert_eq!(settings.section, ClientSettingsSection::KeModel);
    assert_eq!(settings.ke_model.name.as_str(), "ark");
    assert!(settings.ke_model.base_url.as_str().contains("ark.cn-beijing"));
    assert_eq!(settings.ke_model.api_key_env.as_str(), "ARK_API_KEY");
    assert_eq!(settings.ke_model.plan_label(), "火山方舟 / Coding Plan");
}

#[test]
fn settings_includes_the_model_tab() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.open_settings_overlay();
    let mut out = ClientShellInput::default();
    state.select_settings_section(ClientSettingsSection::KeModel, &mut out);
    state.compose(106, 30).expect("settings model tab");
    assert!(
        state
            .hits
            .settings_tabs
            .iter()
            .any(|(_, section)| *section == ClientSettingsSection::KeModel),
        "settings must expose a model tab"
    );
    assert!(
        state.hits.settings_choices.len() >= 8,
        "model tab should list the form fields"
    );
}

#[test]
fn dragging_the_composer_divider_changes_the_right_dock_width() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let path = std::env::temp_dir().join(format!(
        "ke-composer-width-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let config =
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.clone());
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(120, 40).expect("desktop composer");
    let divider = state.hits.composer_divider;
    assert!(
        divider.width == 1 && divider.height > 2,
        "right dock should expose a 1-col divider, got {divider:?}"
    );
    assert_eq!(state.composer_width, COMPOSER_COLS);
    assert_eq!(state.composer_area(120, 40).expect("dock").width, COMPOSER_COLS);

    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: divider.x,
        row: divider.y + 2,
        modifiers: crossterm::event::KeyModifiers::empty(),
    })]);
    let drag_column = divider.x.saturating_sub(8);
    let resize = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: drag_column,
        row: divider.y + 2,
        modifiers: crossterm::event::KeyModifiers::empty(),
    })]);
    assert!(state.composer_width > COMPOSER_COLS);
    assert!(state.composer_width_manual);
    assert!(resize.resize);
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: drag_column,
        row: divider.y + 2,
        modifiers: crossterm::event::KeyModifiers::empty(),
    })]);
    assert!(state.chrome_drag.is_none());
    let saved = state.composer_width;

    let reloaded = ClientShellState::new(
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.clone()),
    );
    assert_eq!(reloaded.composer_width, saved);
    assert!(reloaded.composer_width_manual);

    state.set_pane_surface(surface());
    state.compose(120, 40).expect("resized composer");
    let reset_divider = state.hits.composer_divider;
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: reset_divider.x,
        row: reset_divider.y + 2,
        modifiers: crossterm::event::KeyModifiers::empty(),
    })]);
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: reset_divider.x,
        row: reset_divider.y + 2,
        modifiers: crossterm::event::KeyModifiers::empty(),
    })]);
    assert_eq!(state.composer_width, COMPOSER_COLS);
    assert!(!state.composer_width_manual);
    let _ = std::fs::remove_file(path);
}
