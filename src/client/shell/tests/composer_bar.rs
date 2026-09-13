// Modified by ke: tests for the native composer bar (壳输入栏).
use super::super::composer::{
    ComposerHook, ComposerReply, ComposerRequest, COMPOSER_ROWS, PENDING_LIMIT,
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
fn toggle_reserves_rows_under_the_pane_surface() {
    let mut state = state_with_composer(None, fake_send);
    let open_rows = state.surface_size(120, 40).rows;
    assert!(state
        .composer_area(120, 40)
        .is_some_and(|area| area.height == COMPOSER_ROWS));
    let out = toggle(&mut state);
    assert!(out.resize && state.composer.is_none());
    assert_eq!(state.surface_size(120, 40).rows, open_rows + COMPOSER_ROWS);
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
    assert_eq!(composer.input, "secret");
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
        state.composer.as_ref().unwrap().input,
        "slowx",
        "text typed while processing is kept"
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
    assert_eq!(composer.input, "secret");
    assert_eq!(
        composer.notice.as_deref(),
        Some("处理进程无响应，未发送，原文保留")
    );
}
