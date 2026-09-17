//! Modified by ke: native composer bar (壳输入栏).
//!
//! While the composer is open, typed and pasted text goes into a bar under the pane surface instead of the
//! focused pane. On Enter the text is handed to a local processor (redaction or a resident model) over a Unix
//! socket, then submitted to the focused pane with `agent.prompt` (agent panes) or `pane.send_input` (others).
//! Without a configured processor the text is sent unchanged; if a processor is configured but fails, the text
//! stays in the bar and nothing is sent. With an empty bar, Enter / Esc / Backspace still reach the pane so the
//! agent's own prompts and interrupts keep working.

use super::*;

pub(super) const COMPOSER_ROWS: u16 = 2;
const HOOK_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);
/// Replies faster than this finish inside the key handler; slower ones complete from the timer.
const SYNC_WAIT: std::time::Duration = std::time::Duration::from_millis(30);
/// A processor that has not answered by now is treated as a block.
pub(super) const PENDING_LIMIT: std::time::Duration = std::time::Duration::from_millis(5000);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ClientComposer {
    pub(super) input: String,
    pub(super) notice: Option<String>,
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

#[cfg(unix)]
fn exchange(
    path: &std::path::Path,
    request: &ComposerRequest<'_>,
) -> std::io::Result<ComposerReply> {
    use std::io::{BufRead, Write};
    let mut stream = std::os::unix::net::UnixStream::connect(path)?;
    stream.set_read_timeout(Some(HOOK_TIMEOUT))?;
    stream.set_write_timeout(Some(HOOK_TIMEOUT))?;
    let mut line = serde_json::json!({
        "pane_id": request.pane_id,
        "agent": request.agent,
        "text": request.text,
    })
    .to_string();
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    let mut reply = String::new();
    std::io::BufReader::new(stream).read_line(&mut reply)?;
    Ok(parse_reply(&reply, request.text))
}

#[cfg(not(unix))]
fn exchange(
    _path: &std::path::Path,
    _request: &ComposerRequest<'_>,
) -> std::io::Result<ComposerReply> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "the composer processor needs a unix socket",
    ))
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
        self.composer.is_some()
            && self.overlay.is_none()
            && self.mode == ClientShellMode::Terminal
            && self.popup_terminal_id.is_none()
            && !self.popup_pending
            && self.focused_pane_id().is_some()
    }

    pub(super) fn toggle_composer(&mut self, outcome: &mut ClientShellInput) {
        if self.composer.take().is_none() {
            self.composer = Some(ClientComposer::default());
        }
        outcome.repaint = true;
        outcome.resize = true;
    }

    /// Opens the bar if needed and replaces its text (coordinator panel click). The text is not
    /// sent: the user still reviews it and presses Enter, so it goes through the processor as usual.
    pub(super) fn composer_open_with(&mut self, text: &str, outcome: &mut ClientShellInput) {
        if self.composer.is_none() {
            self.toggle_composer(outcome);
        }
        if let Some(composer) = self.composer.as_mut() {
            composer.input.clear();
            composer.notice = None;
        }
        self.composer_insert(text);
        outcome.repaint = true;
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
        let Some((pane_id, agent)) = self.composer_target() else {
            return;
        };
        let Some(text) = self
            .composer
            .as_ref()
            .map(|composer| composer.input.trim().to_owned())
            .filter(|text| !text.is_empty())
        else {
            return;
        };
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
            Ok(reply) => self.finish_composer_submit(pane_id, agent, &text, reply, outcome),
            Err(_) => {
                self.composer_pending = Some(ComposerPending {
                    rx,
                    pane_id,
                    agent,
                    text,
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
}

pub(super) fn render_composer(
    buffer: &mut Buffer,
    area: Rect,
    composer: &ClientComposer,
    target: &str,
    palette: &Palette,
) -> Option<crate::protocol::CursorState> {
    if area.height == 0 || area.width < 8 {
        return None;
    }
    let status_style = Style::default().fg(palette.subtext0).bg(palette.panel_bg);
    buffer.set_style(Rect::new(area.x, area.y, area.width, 1), status_style);
    let status = match composer.notice.as_deref() {
        Some(note) => format!(" 壳 · {target} · {note}"),
        None => format!(" 壳 · {target} · Enter 发送 · Esc 清空"),
    };
    buffer.set_stringn(
        area.x,
        area.y,
        &status,
        usize::from(area.width),
        status_style,
    );
    if area.height < 2 {
        return None;
    }
    let input_y = area.y + area.height - 1;
    let input_style = Style::default().fg(palette.text).bg(palette.surface0);
    buffer.set_style(Rect::new(area.x, input_y, area.width, 1), input_style);
    let prompt = " › ";
    let prompt_width = u16::try_from(UnicodeWidthStr::width(prompt)).unwrap_or(u16::MAX);
    let available = usize::from(area.width.saturating_sub(prompt_width).saturating_sub(1));
    let line = format!("{prompt}{}", fitting_tail(&composer.input, available));
    buffer.set_stringn(area.x, input_y, &line, usize::from(area.width), input_style);
    let x = area
        .x
        .saturating_add(u16::try_from(UnicodeWidthStr::width(line.as_str())).unwrap_or(u16::MAX))
        .min(area.right().saturating_sub(1));
    Some(crate::protocol::CursorState {
        x,
        y: input_y,
        visible: true,
        shape: 0,
    })
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
