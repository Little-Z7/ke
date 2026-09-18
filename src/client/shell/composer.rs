//! Modified by ke: native composer dock (壳输入栏).
//!
//! The two-row bar is always on screen under the pane surface. Keys go to the focused pane until
//! the dock is focused (click it, or prefix+i). `/ke` commands run locally; `//…` drops one `/`
//! and goes to the pane. `@ke` replies expand a transcript above the input; click the header or Esc
//! to collapse it. On Enter other text is handed to a local processor over a local socket, then
//! submitted with `agent.prompt` or `pane.send_input`. With an empty unfocused bar, Enter / Esc /
//! Backspace still reach the pane.

use super::*;
use crossterm::event::{MouseButton, MouseEventKind};

pub(super) const COMPOSER_ROWS: u16 = 2;
/// Extra rows for an expanded `@ke` transcript sitting above the input.
pub(super) const CHAT_ROWS: u16 = 8;
/// Replies faster than this finish inside the key handler; slower ones complete from the timer.
const SYNC_WAIT: std::time::Duration = std::time::Duration::from_millis(30);
/// A processor that has not answered by now is treated as a block.
pub(super) const PENDING_LIMIT: std::time::Duration = std::time::Duration::from_millis(5000);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ClientComposer {
    pub(super) input: String,
    pub(super) notice: Option<String>,
    /// When false the bar is still drawn, but keys go to the focused pane.
    pub(super) focused: bool,
    pub(super) chat: Option<super::ke_chat::ClientKeChatOverlay>,
    pub(super) chat_expanded: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ComposerRequest<'a> {
    pub(super) pane_id: &'a str,
    pub(super) agent: Option<&'a str>,
    pub(super) text: &'a str,
    /// Processor socket resolved from `[ke] composer_socket` / `KE_COMPOSER_SOCKET`; `None` means
    /// no processor is configured and the text passes through unchanged.
    pub(super) socket: Option<&'a std::path::Path>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ComposerReply {
    Send {
        text: String,
        note: Option<String>,
    },
    Block {
        note: String,
    },
    /// The processor took the input over itself (e.g. `@ke` for the resident model): nothing is sent
    /// to the pane and the bar is cleared.
    Done {
        note: Option<String>,
    },
}

pub(super) type ComposerHook = fn(&ComposerRequest<'_>) -> ComposerReply;

/// A submit whose processor reply has not arrived yet.
#[derive(Debug)]
pub(super) struct ComposerPending {
    rx: std::sync::mpsc::Receiver<ComposerReply>,
    pane_id: String,
    agent: Option<String>,
    text: String,
    started: std::time::Instant,
}

pub(super) fn call_composer_hook(request: &ComposerRequest<'_>) -> ComposerReply {
    let Some(path) = request.socket else {
        return ComposerReply::Send {
            text: request.text.to_owned(),
            note: None,
        };
    };
    match exchange(path, request) {
        Ok(reply) => reply,
        Err(err) => ComposerReply::Block {
            note: format!("处理进程不可用（{err}），未发送"),
        },
    }
}

fn exchange(
    path: &std::path::Path,
    request: &ComposerRequest<'_>,
) -> std::io::Result<ComposerReply> {
    use std::io::{BufRead, Write};
    let mut stream = crate::ipc::connect_local_stream(path)?;
    let mut line = serde_json::json!({
        "pane_id": request.pane_id,
        "agent": request.agent,
        "text": request.text,
    })
    .to_string();
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    let mut reply = String::new();
    std::io::BufReader::new(&mut stream).read_line(&mut reply)?;
    Ok(parse_reply(&reply, request.text))
}

pub(super) fn parse_reply(line: &str, original: &str) -> ComposerReply {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return ComposerReply::Block {
            note: "处理进程返回的不是 JSON，未发送".into(),
        };
    };
    let note = value
        .get("note")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    match value.get("action").and_then(serde_json::Value::as_str) {
        Some("send") => ComposerReply::Send {
            text: value
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(original)
                .to_owned(),
            note,
        },
        Some("done") => ComposerReply::Done { note },
        _ => ComposerReply::Block {
            note: note.unwrap_or_else(|| "处理进程拦下了这条输入".into()),
        },
    }
}

impl ClientShellState {
    pub(super) fn composer_accepts_text(&self) -> bool {
        self.composer
            .as_ref()
            .is_some_and(|composer| composer.focused)
            && self.overlay.is_none()
            && self.mode == ClientShellMode::Terminal
            && self.popup_terminal_id.is_none()
            && !self.popup_pending
            && self.focused_pane_id().is_some()
    }

    pub(super) fn composer_reserved_rows(&self) -> u16 {
        let Some(composer) = self.composer.as_ref() else {
            return 0;
        };
        COMPOSER_ROWS.saturating_add(if composer.chat_expanded { CHAT_ROWS } else { 0 })
    }

    /// Prefix+i focuses or unfocuses the dock. The bar itself stays on screen.
    pub(super) fn toggle_composer(&mut self, outcome: &mut ClientShellInput) {
        let composer = self.composer.get_or_insert_with(ClientComposer::default);
        composer.focused = !composer.focused;
        outcome.repaint = true;
        outcome.resize = true;
    }

    /// Opens the bar if needed and replaces its text (coordinator panel click). The text is not
    /// sent: the user still reviews it and presses Enter, so it goes through the processor as usual.
    pub(super) fn composer_open_with(&mut self, text: &str, outcome: &mut ClientShellInput) {
        let composer = self.composer.get_or_insert_with(ClientComposer::default);
        composer.focused = true;
        composer.input.clear();
        composer.notice = None;
        self.composer_insert(text);
        outcome.repaint = true;
    }

    pub(super) fn set_composer_chat(&mut self, chat: super::ke_chat::ClientKeChatOverlay) -> bool {
        let composer = self.composer.get_or_insert_with(ClientComposer::default);
        let same = composer.chat.as_ref() == Some(&chat) && composer.chat_expanded;
        composer.chat = Some(chat);
        composer.chat_expanded = true;
        !same
    }

    pub(super) fn toggle_composer_chat(&mut self, outcome: &mut ClientShellInput) {
        let Some(composer) = self.composer.as_mut() else {
            return;
        };
        if composer.chat.is_none() {
            return;
        }
        composer.chat_expanded = !composer.chat_expanded;
        outcome.repaint = true;
        outcome.resize = true;
    }

    pub(super) fn composer_insert(&mut self, text: &str) {
        if let Some(composer) = self.composer.as_mut() {
            composer.input.extend(text.chars().map(|character| {
                if matches!(character, '\r' | '\n') {
                    ' '
                } else {
                    character
                }
            }));
            composer.notice = None;
        }
    }

    pub(super) fn composer_handle_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if !self.composer_accepts_text()
            || !matches!(
                key.kind,
                crossterm::event::KeyEventKind::Press | crossterm::event::KeyEventKind::Repeat
            )
        {
            return false;
        }
        let empty = self
            .composer
            .as_ref()
            .is_none_or(|composer| composer.input.is_empty());
        let plain = key
            .modifiers
            .difference(crossterm::event::KeyModifiers::SHIFT)
            .is_empty();
        match key.code {
            KeyCode::Enter if plain && !empty => {
                self.submit_composer(outcome);
                true
            }
            KeyCode::Esc if !empty => {
                if let Some(composer) = self.composer.as_mut() {
                    composer.input.clear();
                    composer.notice = None;
                }
                outcome.repaint = true;
                true
            }
            KeyCode::Esc => {
                if self
                    .composer
                    .as_ref()
                    .is_some_and(|composer| composer.chat_expanded)
                {
                    self.toggle_composer_chat(outcome);
                    true
                } else if self
                    .composer
                    .as_ref()
                    .is_some_and(|composer| composer.focused)
                {
                    if let Some(composer) = self.composer.as_mut() {
                        composer.focused = false;
                    }
                    outcome.repaint = true;
                    true
                } else {
                    false
                }
            }
            KeyCode::Backspace if !empty => {
                if let Some(composer) = self.composer.as_mut() {
                    composer.input.pop();
                }
                outcome.repaint = true;
                true
            }
            KeyCode::Char('u')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL)
                    && !empty =>
            {
                if let Some(composer) = self.composer.as_mut() {
                    composer.input.clear();
                }
                outcome.repaint = true;
                true
            }
            KeyCode::Char(character) if plain => {
                let text = key
                    .generated_text
                    .clone()
                    .unwrap_or_else(|| character.to_string());
                self.composer_insert(&text);
                outcome.repaint = true;
                true
            }
            _ => false,
        }
    }

    pub(super) fn composer_target(&self) -> Option<(String, Option<String>)> {
        let pane_id = self.focused_pane_id()?;
        let agent = self
            .snapshot
            .as_deref()
            .and_then(|snapshot| {
                snapshot
                    .agents
                    .iter()
                    .find(|agent| agent.pane_id == pane_id)
            })
            .and_then(|agent| agent.agent.clone().or_else(|| agent.display_agent.clone()));
        Some((pane_id, agent))
    }

    pub(super) fn composer_target_label(&self) -> String {
        match self.composer_target() {
            Some((pane_id, Some(agent))) => format!("{agent} {pane_id}"),
            Some((pane_id, None)) => pane_id,
            None => "无窗格".into(),
        }
    }

    pub(super) fn submit_composer(&mut self, outcome: &mut ClientShellInput) {
        if self.composer_pending.is_some() {
            if let Some(composer) = self.composer.as_mut() {
                composer.notice = Some("上一条还在处理，稍候再发".into());
            }
            outcome.repaint = true;
            return;
        }
        let Some(original) = self
            .composer
            .as_ref()
            .map(|composer| composer.input.trim().to_owned())
            .filter(|text| !text.is_empty())
        else {
            return;
        };
        if let Some(command) = crate::ke::slash::parse_ke_command(&original) {
            self.run_ke_slash(command, outcome);
            return;
        }
        let Some((pane_id, agent)) = self.composer_target() else {
            return;
        };
        let text = crate::ke::slash::passthrough_after_slash_escape(&original)
            .map(str::to_owned)
            .unwrap_or_else(|| original.clone());
        // The processor runs off the input path: a fast reply finishes here, a slow one is picked up
        // by `tick_composer` from the main-loop timer, so the UI never blocks on the socket.
        let hook = self.composer_hook;
        // Resolved per submit so a processor started after ke launched is picked up.
        let socket = self.config.ke.composer_socket_path();
        let (tx, rx) = std::sync::mpsc::channel();
        let job = (pane_id.clone(), agent.clone(), text.clone(), socket);
        std::thread::spawn(move || {
            let (pane_id, agent, text, socket) = job;
            let _ = tx.send(hook(&ComposerRequest {
                pane_id: &pane_id,
                agent: agent.as_deref(),
                text: &text,
                socket: socket.as_deref(),
            }));
        });
        match rx.recv_timeout(SYNC_WAIT) {
            Ok(reply) => self.finish_composer_submit(pane_id, agent, &original, reply, outcome),
            Err(_) => {
                self.composer_pending = Some(ComposerPending {
                    rx,
                    pane_id,
                    agent,
                    text: original,
                    started: std::time::Instant::now(),
                });
                if let Some(composer) = self.composer.as_mut() {
                    composer.notice = Some("处理中…".into());
                }
                outcome.repaint = true;
            }
        }
    }

    /// Picks up a slow processor reply; called from the main-loop timer.
    pub(crate) fn tick_composer(
        &mut self,
        now: std::time::Instant,
        outcome: &mut ClientShellInput,
    ) {
        let Some(pending) = self.composer_pending.as_ref() else {
            return;
        };
        let reply = match pending.rx.try_recv() {
            Ok(reply) => reply,
            Err(std::sync::mpsc::TryRecvError::Empty)
                if now.saturating_duration_since(pending.started) < PENDING_LIMIT =>
            {
                return;
            }
            Err(_) => ComposerReply::Block {
                note: "处理进程无响应，未发送，原文保留".into(),
            },
        };
        let Some(pending) = self.composer_pending.take() else {
            return;
        };
        self.finish_composer_submit(
            pending.pane_id,
            pending.agent,
            &pending.text,
            reply,
            outcome,
        );
    }

    fn finish_composer_submit(
        &mut self,
        pane_id: String,
        agent: Option<String>,
        submitted: &str,
        reply: ComposerReply,
        outcome: &mut ClientShellInput,
    ) {
        match reply {
            ComposerReply::Send { text, note } => {
                let method = if agent.is_some() {
                    crate::api::schema::Method::AgentPrompt(crate::api::schema::AgentPromptParams {
                        target: pane_id,
                        text,
                        wait: None,
                    })
                } else {
                    crate::api::schema::Method::PaneSendInput(
                        crate::api::schema::PaneSendInputParams {
                            pane_id,
                            text,
                            keys: vec!["Enter".into()],
                        },
                    )
                };
                let sent = self.push_endpoint_method_with_kind(
                    method,
                    PendingEndpointKind::Generic,
                    outcome,
                );
                if let Some(composer) = self.composer.as_mut() {
                    if sent {
                        // Only the submitted text is cleared; anything typed while processing stays.
                        if composer.input.trim() == submitted {
                            composer.input.clear();
                        }
                        composer.notice = note;
                    } else {
                        composer.notice = Some("发送失败：服务端未就绪或不支持，原文保留".into());
                    }
                }
            }
            ComposerReply::Block { note } => {
                if let Some(composer) = self.composer.as_mut() {
                    composer.notice = Some(note);
                }
            }
            ComposerReply::Done { note } => {
                if let Some(composer) = self.composer.as_mut() {
                    if composer.input.trim() == submitted {
                        composer.input.clear();
                    }
                    composer.notice = note;
                }
            }
        }
        outcome.repaint = true;
    }

    pub(super) fn composer_area(&self, cols: u16, rows: u16) -> Option<Rect> {
        self.composer.as_ref()?;
        let full = self.base_layout(cols, rows).pane_surface;
        let shrunk = self.layout(cols, rows).pane_surface;
        (shrunk.height < full.height).then(|| {
            Rect::new(
                full.x,
                shrunk.y.saturating_add(shrunk.height),
                full.width,
                full.height - shrunk.height,
            )
        })
    }

    pub(super) fn handle_composer_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if self.overlay.is_some() {
            return false;
        }
        let point = (mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left)
                if super::contains(self.hits.composer_toggle, point) =>
            {
                self.toggle_composer_chat(outcome);
                if let Some(composer) = self.composer.as_mut() {
                    composer.focused = true;
                }
                true
            }
            MouseEventKind::Down(MouseButton::Left)
                if super::contains(self.hits.composer_input, point)
                    || super::contains(self.hits.composer_chat, point) =>
            {
                if let Some(composer) = self.composer.as_mut() {
                    composer.focused = true;
                }
                outcome.repaint = true;
                true
            }
            MouseEventKind::ScrollUp if super::contains(self.hits.composer_chat, point) => {
                self.scroll_composer_chat(-3);
                outcome.repaint = true;
                true
            }
            MouseEventKind::ScrollDown if super::contains(self.hits.composer_chat, point) => {
                self.scroll_composer_chat(3);
                outcome.repaint = true;
                true
            }
            _ => false,
        }
    }

    fn scroll_composer_chat(&mut self, delta: isize) {
        let max_scroll = self.hits.ke_chat_max_scroll;
        if let Some(chat) = self
            .composer
            .as_mut()
            .and_then(|composer| composer.chat.as_mut())
        {
            chat.scroll = if delta.is_negative() {
                chat.scroll.saturating_sub(delta.unsigned_abs())
            } else {
                chat.scroll.saturating_add(delta.unsigned_abs())
            }
            .min(max_scroll);
        }
    }
}

#[derive(Default)]
pub(super) struct ComposerRender {
    pub cursor: Option<crate::protocol::CursorState>,
    pub input: Rect,
    pub toggle: Rect,
    pub chat: Rect,
    pub max_scroll: usize,
}

pub(super) fn render_composer(
    buffer: &mut Buffer,
    area: Rect,
    composer: &ClientComposer,
    target: &str,
    palette: &Palette,
) -> ComposerRender {
    let mut rendered = ComposerRender::default();
    if area.height == 0 || area.width < 8 {
        return rendered;
    }
    let status_style = Style::default().fg(palette.subtext0).bg(palette.panel_bg);
    let input_style = Style::default().fg(palette.text).bg(palette.surface0);
    let chat_style = Style::default().fg(palette.text).bg(palette.panel_bg);
    let header_style = Style::default()
        .fg(palette.accent)
        .bg(palette.panel_bg)
        .add_modifier(Modifier::BOLD);

    let input_y = area.y + area.height.saturating_sub(1);
    rendered.input = Rect::new(area.x, input_y, area.width, 1);
    let status_y = if area.height >= 2 {
        input_y.saturating_sub(1)
    } else {
        area.y
    };

    if composer.chat_expanded {
        if let Some(chat) = composer.chat.as_ref() {
            let header = Rect::new(area.x, area.y, area.width, 1);
            rendered.toggle = header;
            buffer.set_style(header, status_style);
            buffer.set_stringn(
                header.x,
                header.y,
                " 管家 · 点此收起 · 滚轮翻阅",
                usize::from(header.width),
                header_style,
            );
            let body_height = status_y.saturating_sub(area.y.saturating_add(1));
            if body_height > 0 {
                let body = Rect::new(area.x, area.y.saturating_add(1), area.width, body_height);
                rendered.chat = body;
                buffer.set_style(body, chat_style);
                let lines: Vec<&str> = chat.body.lines().collect();
                rendered.max_scroll = lines.len().saturating_sub(usize::from(body.height));
                let start = chat.scroll.min(rendered.max_scroll);
                for (index, line) in lines.into_iter().skip(start).enumerate() {
                    let y = body.y.saturating_add(index as u16);
                    if y >= body.bottom() {
                        break;
                    }
                    buffer.set_stringn(
                        body.x,
                        y,
                        format!(" {line}"),
                        usize::from(body.width),
                        chat_style,
                    );
                }
            }
        }
    } else if composer.chat.is_some() {
        rendered.toggle = Rect::new(area.x, status_y, area.width, 1);
    }

    let status = if !composer.focused {
        if composer.chat.is_some() && !composer.chat_expanded {
            format!(" 壳 · {target} · 点此输入 · 点标题展开管家")
        } else {
            format!(" 壳 · {target} · 点此或 Ctrl+B i 输入")
        }
    } else if let Some(note) = composer.notice.as_deref() {
        format!(" 壳 · {target} · {note}")
    } else if composer.chat.is_some() && !composer.chat_expanded {
        format!(" 壳 · {target} · Enter 发送 · 点标题展开管家")
    } else {
        format!(" 壳 · {target} · Enter 发送 · Esc 收起")
    };
    buffer.set_style(Rect::new(area.x, status_y, area.width, 1), status_style);
    buffer.set_stringn(
        area.x,
        status_y,
        &status,
        usize::from(area.width),
        status_style,
    );

    buffer.set_style(rendered.input, input_style);
    let prompt = if composer.focused { " › " } else { "   " };
    let prompt_width = u16::try_from(UnicodeWidthStr::width(prompt)).unwrap_or(u16::MAX);
    let available = usize::from(area.width.saturating_sub(prompt_width).saturating_sub(1));
    let line = format!("{prompt}{}", fitting_tail(&composer.input, available));
    buffer.set_stringn(area.x, input_y, &line, usize::from(area.width), input_style);
    if composer.focused {
        let x = area
            .x
            .saturating_add(
                u16::try_from(UnicodeWidthStr::width(line.as_str())).unwrap_or(u16::MAX),
            )
            .min(area.right().saturating_sub(1));
        rendered.cursor = Some(crate::protocol::CursorState {
            x,
            y: input_y,
            visible: true,
            shape: 0,
        });
    }
    rendered
}

fn fitting_tail(text: &str, width: usize) -> &str {
    let mut used = 0usize;
    let mut start = text.len();
    for (index, character) in text.char_indices().rev() {
        let char_width = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
        if used + char_width > width {
            break;
        }
        used += char_width;
        start = index;
    }
    &text[start..]
}
