//! Added by ke: the `@ke` conversation as an append-only JSON Lines file.
//!
//! The resident appends what the user said and what the model answered; the client re-reads the
//! tail when the file's mtime changes and shows new assistant entries in a popup. One JSON object
//! per line:
//!
//! ```json
//! {"ts":1726200000.0,"role":"user","text":"哪个 agent 空着","pane_id":"w1:p2"}
//! {"ts":1726200003.2,"role":"assistant","text":"codex 空着…","pending":false}
//! ```
//!
//! Entries are never rewritten in place. While the model is still answering the resident may
//! append a `pending: true` placeholder; the final answer is a separate entry, and the client only
//! shows entries with `pending == false`.

use std::io::{BufRead, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

const MAX_TEXT_CHARS: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChatRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ChatEntry {
    /// Unix seconds.
    pub ts: f64,
    pub role: ChatRole,
    pub text: String,
    /// Pane that was focused when the user spoke.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    /// True while the model is still answering; such entries are not shown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
    /// Profile name that produced an assistant entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

impl ChatEntry {
    pub(crate) fn now(role: ChatRole, text: impl Into<String>) -> Self {
        Self {
            ts: now_epoch(),
            role,
            text: truncate_chars(text.into(), MAX_TEXT_CHARS),
            pane_id: None,
            pending: false,
            profile: None,
        }
    }
}

pub(crate) fn now_epoch() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

fn truncate_chars(mut text: String, max: usize) -> String {
    if let Some((index, _)) = text.char_indices().nth(max) {
        text.truncate(index);
        text.push('…');
    }
    text
}

/// Appends one entry; newlines inside `text` are JSON-escaped so the file stays one entry per line.
pub(crate) fn append(path: &Path, entry: &ChatEntry) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())
}

/// Reads the last `limit` well-formed entries. A missing file is an empty log.
pub(crate) fn read_tail(path: &Path, limit: usize) -> std::io::Result<Vec<ChatEntry>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut entries: std::collections::VecDeque<ChatEntry> =
        std::collections::VecDeque::with_capacity(limit.min(256));
    for line in std::io::BufReader::new(file).lines() {
        let line = line?;
        let Ok(entry) = serde_json::from_str::<ChatEntry>(&line) else {
            continue;
        };
        if entries.len() == limit {
            entries.pop_front();
        }
        entries.push_back(entry);
    }
    Ok(entries.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ke-chat-log-{}-{}",
            std::process::id(),
            now_epoch().to_bits()
        ));
        dir.join("chat.jsonl")
    }

    #[test]
    fn append_then_tail_round_trips_and_skips_garbage() {
        let path = temp_log();
        let mut user = ChatEntry::now(ChatRole::User, "第一行\n第二行");
        user.pane_id = Some("w1:p1".into());
        append(&path, &user).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"not json\n")
            .unwrap();
        let mut answer = ChatEntry::now(ChatRole::Assistant, "好的");
        answer.profile = Some("local".into());
        append(&path, &answer).unwrap();

        let all = read_tail(&path, 10).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].text, "第一行\n第二行");
        assert_eq!(all[0].pane_id.as_deref(), Some("w1:p1"));
        assert_eq!(all[1].role, ChatRole::Assistant);
        assert_eq!(all[1].profile.as_deref(), Some("local"));
        assert!(!all[1].pending);

        let last = read_tail(&path, 1).unwrap();
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].text, "好的");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_log_is_empty_and_long_text_is_capped() {
        assert!(read_tail(Path::new("/nonexistent/ke/chat.jsonl"), 5)
            .unwrap()
            .is_empty());
        let entry = ChatEntry::now(ChatRole::System, "x".repeat(MAX_TEXT_CHARS + 10));
        assert_eq!(entry.text.chars().count(), MAX_TEXT_CHARS + 1);
        assert!(entry.text.ends_with('…'));
    }
}
