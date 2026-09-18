//! Added by ke: client-local `@ke` answer overlay, fed by `resident/chat.jsonl`.
//!
//! The resident appends JSON Lines; the TUI polls mtime on the same 2s timer as the sidebar panel.
//! The first successful read only remembers the last timestamp so startup does not dump history.
//! Later `role=assistant` entries with `pending == false` open (or update) a scrollable overlay.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use crate::ke::chat_log::{ChatEntry, ChatRole};

pub(super) const KE_CHAT_POLL: Duration = Duration::from_secs(2);
const KE_CHAT_TAIL: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClientKeChatOverlay {
    pub title: String,
    pub body: String,
    pub scroll: usize,
}

#[derive(Debug, Clone, Default)]
pub(super) struct KeChatSource {
    path: Option<PathBuf>,
    next_check: Option<Instant>,
    mtime: Option<SystemTime>,
    primed: bool,
    last_seen_ts: Option<f64>,
    pending: Option<ClientKeChatOverlay>,
}

impl KeChatSource {
    /// `path` comes from `KeConfig::chat_log_path()`; tests never watch the real file.
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path: if cfg!(test) { None } else { path },
            ..Self::default()
        }
    }

    /// Re-points the source (live config reload). Pending overlay is dropped so the next tick can
    /// prime the new file without dumping its history.
    pub fn set_path(&mut self, path: Option<PathBuf>) {
        if cfg!(test) || self.path == path {
            return;
        }
        self.path = path;
        self.next_check = None;
        self.mtime = None;
        self.primed = false;
        self.last_seen_ts = None;
        self.pending = None;
    }

    /// Returns a new overlay when a visible assistant reply arrived and nothing else is blocking.
    pub fn tick(&mut self, now: Instant, overlay_blocking: bool) -> Option<ClientKeChatOverlay> {
        if let Some(path) = self.path.clone() {
            if self.next_check.is_none_or(|deadline| now >= deadline) {
                self.next_check = Some(now + KE_CHAT_POLL);
                match std::fs::metadata(&path) {
                    Ok(meta) => {
                        let mtime = meta.modified().ok();
                        if mtime.is_none() || mtime != self.mtime {
                            self.mtime = mtime;
                            if let Ok(entries) = crate::ke::chat_log::read_tail(&path, KE_CHAT_TAIL)
                            {
                                if let Some(overlay) = self.apply_entries(&entries) {
                                    self.pending = Some(overlay);
                                }
                            }
                        }
                    }
                    Err(_) => {
                        self.mtime = None;
                    }
                }
            }
        }
        if overlay_blocking {
            return None;
        }
        self.pending.take()
    }

    #[cfg(test)]
    pub fn ingest_for_test(&mut self, entries: &[ChatEntry]) -> Option<ClientKeChatOverlay> {
        let overlay = self.apply_entries(entries);
        if let Some(overlay) = overlay {
            self.pending = Some(overlay.clone());
            Some(overlay)
        } else {
            None
        }
    }

    fn apply_entries(&mut self, entries: &[ChatEntry]) -> Option<ClientKeChatOverlay> {
        let last_ts = entries.last().map(|entry| entry.ts);
        if !self.primed {
            self.primed = true;
            self.last_seen_ts = last_ts;
            return None;
        }
        let cutoff = self.last_seen_ts.unwrap_or(f64::NEG_INFINITY);
        if let Some(ts) = last_ts {
            self.last_seen_ts = Some(ts);
        }
        let has_new = entries
            .iter()
            .any(|entry| entry.role == ChatRole::Assistant && !entry.pending && entry.ts > cutoff);
        if !has_new {
            return None;
        }
        overlay_from_entries(entries)
    }
}

pub(super) fn format_turn(user: &str, assistant: &str) -> String {
    if user.is_empty() {
        assistant.to_string()
    } else {
        format!("{user}\n\n{assistant}")
    }
}

pub(super) fn latest_visible_assistant(entries: &[ChatEntry]) -> Option<(String, String, f64)> {
    let assistant_idx = entries
        .iter()
        .rposition(|entry| entry.role == ChatRole::Assistant && !entry.pending)?;
    let assistant = &entries[assistant_idx];
    let user = entries[..assistant_idx]
        .iter()
        .rev()
        .find(|entry| entry.role == ChatRole::User && !entry.pending)
        .map(|entry| entry.text.clone())
        .unwrap_or_default();
    Some((user, assistant.text.clone(), assistant.ts))
}

fn overlay_from_entries(entries: &[ChatEntry]) -> Option<ClientKeChatOverlay> {
    let (user, assistant, _) = latest_visible_assistant(entries)?;
    Some(ClientKeChatOverlay {
        title: "管家".into(),
        body: format_turn(&user, &assistant),
        scroll: 0,
    })
}

/// Last visible turns for `/ke log`. Pending placeholders are skipped.
pub(super) fn overlay_from_log(entries: &[ChatEntry]) -> Option<ClientKeChatOverlay> {
    let mut parts = Vec::new();
    for entry in entries.iter().filter(|entry| !entry.pending) {
        let who = match entry.role {
            ChatRole::User => "你",
            ChatRole::Assistant => "管家",
            ChatRole::System => "系统",
        };
        parts.push(format!("{who}：{}", entry.text));
    }
    if parts.is_empty() {
        return None;
    }
    Some(ClientKeChatOverlay {
        title: "管家记录".into(),
        body: parts.join("\n\n"),
        scroll: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn format_turn_joins_user_and_assistant() {
        assert_eq!(format_turn("谁空着", "codex"), "谁空着\n\ncodex");
        assert_eq!(format_turn("", "只有回答"), "只有回答");
    }

    #[test]
    fn latest_visible_assistant_skips_pending() {
        let entries = [
            entry(1.0, ChatRole::User, "问", false),
            entry(2.0, ChatRole::Assistant, "旧回答", false),
            entry(3.0, ChatRole::User, "再问", false),
            entry(4.0, ChatRole::Assistant, "写着", true),
        ];
        let (user, assistant, ts) = latest_visible_assistant(&entries).expect("visible turn");
        assert_eq!(user, "问");
        assert_eq!(assistant, "旧回答");
        assert_eq!(ts, 2.0);
        assert!(latest_visible_assistant(&[]).is_none());
        assert!(latest_visible_assistant(&[entry(1.0, ChatRole::User, "只问", false)]).is_none());
    }

    #[test]
    fn source_first_ingest_is_silent_then_new_assistant_opens() {
        let mut source = KeChatSource::new(None);
        let history = [
            entry(1.0, ChatRole::User, "哪个 agent 空着", false),
            entry(2.0, ChatRole::Assistant, "codex 空着", false),
        ];
        assert!(source.ingest_for_test(&history).is_none());
        let mut next = history.to_vec();
        next.push(entry(3.0, ChatRole::User, "再问", false));
        next.push(entry(4.0, ChatRole::Assistant, "claude 也空", false));
        let overlay = source.ingest_for_test(&next).expect("new reply");
        assert_eq!(overlay.title, "管家");
        assert!(overlay.body.contains("claude 也空"));
        assert_eq!(overlay.scroll, 0);
    }

    #[test]
    fn source_pending_assistant_does_not_open() {
        let mut source = KeChatSource::new(None);
        let history = [entry(1.0, ChatRole::Assistant, "旧", false)];
        assert!(source.ingest_for_test(&history).is_none());
        let next = [
            entry(1.0, ChatRole::Assistant, "旧", false),
            entry(2.0, ChatRole::Assistant, "写着", true),
        ];
        assert!(source.ingest_for_test(&next).is_none());
    }

    #[test]
    fn source_queues_while_blocked_then_flushes() {
        let mut source = KeChatSource::new(None);
        let history = [entry(1.0, ChatRole::Assistant, "旧", false)];
        assert!(source.ingest_for_test(&history).is_none());
        let next = [
            entry(1.0, ChatRole::Assistant, "旧", false),
            entry(2.0, ChatRole::Assistant, "新回答", false),
        ];
        assert!(source.ingest_for_test(&next).is_some());
        let now = Instant::now();
        assert!(
            source.tick(now, true).is_none(),
            "blocked overlay stays queued"
        );
        let flushed = source.tick(now, false).expect("flush after overlay clears");
        assert!(flushed.body.contains("新回答"));
        assert!(source.tick(now, false).is_none());
    }

    #[test]
    fn overlay_from_log_joins_visible_turns() {
        let overlay = overlay_from_log(&[
            entry(1.0, ChatRole::User, "谁空着", false),
            entry(2.0, ChatRole::Assistant, "写着", true),
            entry(3.0, ChatRole::Assistant, "codex", false),
        ])
        .expect("log");
        assert_eq!(overlay.title, "管家记录");
        assert!(overlay.body.contains("你：谁空着"));
        assert!(overlay.body.contains("管家：codex"));
        assert!(!overlay.body.contains("写着"));
        assert!(overlay_from_log(&[]).is_none());
    }
}
