// Modified by ke: `@ke` replies land in the composer dock, not a full-screen overlay.
use super::super::ke_chat::ClientKeChatOverlay;
use super::*;
use crate::ke::chat_log::{ChatEntry, ChatRole};

fn shell() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state
}

fn entry(ts: f64, role: ChatRole, text: &str, pending: bool) -> ChatEntry {
    ChatEntry {
        ts,
        role,
        text: text.into(),
        pane_id: None,
        pending,
        profile: None,
    }
}

fn ingest(state: &mut ClientShellState, entries: &[ChatEntry]) {
    let _ = state.ke_chat.ingest_for_test(entries);
    let _ = state.tick_ke_chat(std::time::Instant::now());
}

fn history() -> Vec<ChatEntry> {
    vec![
        entry(1.0, ChatRole::User, "哪个 agent 空着", false),
        entry(2.0, ChatRole::Assistant, "codex 空着", false),
    ]
}

fn dock_chat(state: &ClientShellState) -> Option<&ClientKeChatOverlay> {
    state
        .composer
        .as_ref()
        .and_then(|composer| composer.chat.as_ref())
}

#[test]
fn ke_chat_first_ingest_does_not_open_transcript() {
    let mut state = shell();
    ingest(&mut state, &history());
    assert!(state.overlay.is_none(), "startup must not dump history");
    assert!(dock_chat(&state).is_none());
}

#[test]
fn ke_chat_new_assistant_expands_the_dock() {
    let mut state = shell();
    ingest(&mut state, &history());
    let mut next = history();
    next.push(entry(3.0, ChatRole::User, "再问", false));
    next.push(entry(4.0, ChatRole::Assistant, "claude 也空", false));
    ingest(&mut state, &next);
    assert!(state.overlay.is_none(), "replies are not a modal overlay");
    let chat = dock_chat(&state).expect("transcript in the dock");
    assert_eq!(chat.title, "管家");
    assert!(chat.body.contains("claude 也空"), "body: {}", chat.body);
    assert!(chat.body.contains("再问"));
    assert!(state.composer.as_ref().unwrap().chat_expanded);
}

#[test]
fn ke_chat_pending_assistant_does_not_open_transcript() {
    let mut state = shell();
    ingest(&mut state, &history());
    let mut next = history();
    next.push(entry(3.0, ChatRole::Assistant, "正在写", true));
    ingest(&mut state, &next);
    assert!(dock_chat(&state).is_none());
}

#[test]
fn ke_chat_escape_collapses_then_unfocuses() {
    let mut state = shell();
    state.set_composer_chat(ClientKeChatOverlay {
        title: "管家".into(),
        body: "回答".into(),
        scroll: 0,
    });
    if let Some(composer) = state.composer.as_mut() {
        composer.focused = true;
    }
    let out = state.handle_input_bytes(b"\x1b");
    assert!(state.overlay.is_none());
    assert!(
        state
            .composer
            .as_ref()
            .is_some_and(|composer| composer.chat.is_some() && !composer.chat_expanded),
        "first Esc collapses the transcript"
    );
    assert!(out.repaint);
    let out = state.handle_input_bytes(b"\x1b");
    assert!(
        state
            .composer
            .as_ref()
            .is_some_and(|composer| !composer.focused),
        "second Esc gives the keyboard back to the pane"
    );
    assert!(out.repaint);
    assert!(out.actions.is_empty() && out.requests.is_empty());
}

#[test]
fn ke_chat_waits_while_help_overlay_is_open() {
    let mut state = shell();
    ingest(&mut state, &history());
    state.overlay = Some(ClientShellOverlay::Help(ClientHelpOverlay {
        query: TextEditor::default(),
        search_focused: false,
        scroll: 0,
    }));
    let mut next = history();
    next.push(entry(3.0, ChatRole::Assistant, "新回答", false));
    ingest(&mut state, &next);
    assert!(
        matches!(state.overlay, Some(ClientShellOverlay::Help(_))),
        "help must not be replaced"
    );
    assert!(dock_chat(&state).is_none());
    state.overlay = None;
    let _ = state.tick_ke_chat(std::time::Instant::now());
    let chat = dock_chat(&state).expect("queued reply opens in the dock");
    assert!(chat.body.contains("新回答"));
}
