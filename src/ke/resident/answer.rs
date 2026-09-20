//! Added by ke: the model half of the resident.
//!
//! Polls `chat.jsonl` on its own thread so the composer processor never waits on a model. The first
//! read only records the last timestamp (old unanswered users stay unanswered). Later user entries
//! without a following completed assistant get a `pending: true` placeholder, then one finished
//! `pending: false` answer. Entries are never rewritten.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::api::client::ApiClient;
use crate::config::{KeConfig, KeModelProfile};
use crate::ke::chat_log::{self, ChatEntry, ChatRole};
use crate::ke::paths::ResidentPaths;
use crate::ke::redact::{self, RedactRules};

use super::model::{self, ChatMessage};
use super::prompt::{self, CONSTITUTION};
use super::snapshot;
use super::Shared;

const POLL: Duration = Duration::from_millis(250);
const TAIL_LIMIT: usize = 200;
const HISTORY_LIMIT: usize = 20;

const CLOUD_UNCONFIRMED: &str =
    "云端模型尚未确认。把 [ke.model] cloud_confirmed 设为 true 后再试。";
const MISSING_PROFILE: &str = "\
没有解析到模型配置。在配置文件里加上：

[ke.model.profiles.local]
provider = \"openai\"
base_url = \"http://localhost:11434/v1\"
model = \"qwen3:4b\"

并把 [ke.model] active 设为 local。";

/// Independent thread: poll the chat log, answer new `@ke` users, exit when `stop` is set.
pub(crate) fn run(
    client: ApiClient,
    paths: ResidentPaths,
    ke: KeConfig,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
) {
    let mut primed_ts: Option<f64> = None;
    while !stop.load(Ordering::Relaxed) {
        match chat_log::read_tail(&paths.chat_log, TAIL_LIMIT) {
            Ok(entries) => match primed_ts {
                None => {
                    primed_ts = Some(entries.last().map(|entry| entry.ts).unwrap_or(0.0));
                }
                Some(prime) => {
                    if let Some(user) = next_unanswered(&entries, prime) {
                        let user = user.clone();
                        answer_one(&client, &paths, &ke, &shared, &user);
                    }
                }
            },
            Err(err) => {
                eprintln!("ke resident: chat log read failed: {err}");
            }
        }
        std::thread::sleep(POLL);
    }
}

/// Earliest user after `primed_ts` that has no completed assistant before the next user.
pub(crate) fn next_unanswered(entries: &[ChatEntry], primed_ts: f64) -> Option<&ChatEntry> {
    let mut index = 0usize;
    while index < entries.len() {
        let entry = &entries[index];
        index += 1;
        if entry.ts <= primed_ts || !matches!(entry.role, ChatRole::User) {
            continue;
        }
        let mut answered = false;
        for later in &entries[index..] {
            if matches!(later.role, ChatRole::User) {
                break;
            }
            if matches!(later.role, ChatRole::Assistant) && !later.pending {
                answered = true;
                break;
            }
        }
        if !answered {
            return Some(entry);
        }
    }
    None
}

fn answer_one(
    client: &ApiClient,
    paths: &ResidentPaths,
    ke: &KeConfig,
    shared: &Shared,
    user: &ChatEntry,
) {
    let profile = ke.resolved_profile();
    let profile_name = profile.as_ref().map(|(name, _)| name.clone());
    if let Err(err) = append_assistant(&paths.chat_log, "思考中…", true, profile_name.clone())
    {
        eprintln!("ke resident: pending chat append failed: {err}");
        return;
    }

    let text = match profile {
        None => MISSING_PROFILE.to_string(),
        Some((_, profile)) if !profile.is_local() && !ke.model.cloud_confirmed => {
            CLOUD_UNCONFIRMED.to_string()
        }
        Some((_, profile)) => complete_answer(client, &paths.chat_log, ke, shared, user, &profile),
    };

    if let Err(err) = append_assistant(&paths.chat_log, &text, false, profile_name) {
        eprintln!("ke resident: answer chat append failed: {err}");
    }
}

fn complete_answer(
    client: &ApiClient,
    chat_log: &Path,
    ke: &KeConfig,
    shared: &Shared,
    user: &ChatEntry,
    profile: &KeModelProfile,
) -> String {
    let agents = shared
        .agents
        .lock()
        .map(|slot| slot.clone())
        .unwrap_or_default();
    let mut snapshot_text = snapshot::capture(&agents, client);
    let user_prompt = prompt::load_user_prompt(&ke.model.system_prompt_file);
    let rules = redact_rules_for(ke, profile);
    if let Some(rules) = rules {
        snapshot_text = mask_with_shared(&snapshot_text, rules, shared);
    }

    let mut messages = prompt::system_messages(CONSTITUTION, &user_prompt, &snapshot_text);
    if let Ok(entries) = chat_log::read_tail(chat_log, TAIL_LIMIT) {
        messages.extend(history_messages(&entries, user, rules, shared));
    }
    let user_text = match rules {
        Some(rules) => mask_with_shared(&user.text, rules, shared),
        None => user.text.clone(),
    };
    messages.push(ChatMessage::user(user_text));

    // The resident directory (where chat.jsonl lives) doubles as the cwd for `provider =
    // "cli"` subprocesses, so a headless agent CLI reads the resident's own files instead of
    // whatever project the user's active pane happens to be sitting in.
    // chat.jsonl always sits in the resident directory, so the fallback is unreachable in
    // practice. It must still not be the process cwd: the resident inherits that from the server,
    // which is usually the user's project, and a CLI provider started there would read that
    // project's own agent instructions into the manager's context.
    let cwd_fallback = std::env::temp_dir();
    let resident_dir = chat_log.parent().unwrap_or(&cwd_fallback);
    match model::complete(profile, &messages, resident_dir) {
        Ok(text) => text,
        Err(err) => format!("模型调用失败：{err}"),
    }
}

/// Runs [`redact::mask`] against the session's single shared [`redact::Mapping`], holding its
/// lock only for the duration of the `mask` call itself -- never across the network request that
/// fetched `text` (the pane snapshot) or the model call that follows it. Sharing the same mapping
/// with the composer processor is what keeps a placeholder in a pane and the same placeholder in
/// the model's context pointing at the same real value.
fn mask_with_shared(text: &str, rules: RedactRules, shared: &Shared) -> String {
    match shared.redaction.lock() {
        Ok(mut map) => redact::mask(text, rules, &mut map).text,
        // Poisoned lock: fall back to the non-reversible gate rather than ever sending
        // unredacted text to the model. Same fail-closed idea as `Mapping` hitting its capacity.
        Err(_) => redact::redact(text, rules).text,
    }
}

fn history_messages(
    entries: &[ChatEntry],
    current: &ChatEntry,
    rules: Option<RedactRules>,
    shared: &Shared,
) -> Vec<ChatMessage> {
    let mut selected = Vec::new();
    for entry in entries {
        if entry.pending {
            continue;
        }
        if entry.ts == current.ts
            && matches!(entry.role, ChatRole::User)
            && entry.text == current.text
        {
            continue;
        }
        let role = match entry.role {
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
            ChatRole::System => "system",
        };
        let text = match rules {
            Some(rules) => mask_with_shared(&entry.text, rules, shared),
            None => entry.text.clone(),
        };
        selected.push(ChatMessage {
            role: role.into(),
            content: text,
        });
    }
    if selected.len() > HISTORY_LIMIT {
        selected.drain(0..selected.len() - HISTORY_LIMIT);
    }
    selected
}

fn redact_rules_for(ke: &KeConfig, profile: &KeModelProfile) -> Option<RedactRules> {
    if !profile.is_local() || ke.redact.redact_for_local_models {
        Some(ke.redact_rules())
    } else {
        None
    }
}

fn append_assistant(
    path: &Path,
    text: &str,
    pending: bool,
    profile: Option<String>,
) -> std::io::Result<()> {
    let mut entry = ChatEntry::now(ChatRole::Assistant, text);
    entry.pending = pending;
    entry.profile = profile;
    chat_log::append(path, &entry)
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
    fn next_unanswered_skips_history_and_completed_turns() {
        let entries = vec![
            entry(1.0, ChatRole::User, "旧问题", false),
            entry(2.0, ChatRole::User, "谁空着", false),
            entry(2.5, ChatRole::Assistant, "思考中…", true),
            entry(3.0, ChatRole::Assistant, "codex 空着", false),
            entry(4.0, ChatRole::User, "那 claude 呢", false),
        ];
        assert!(
            next_unanswered(&entries, 4.0).is_none(),
            "first read primes on the last ts and must not answer history"
        );
        let next = next_unanswered(&entries, 1.0).expect("new unanswered user");
        assert_eq!(next.text, "那 claude 呢");
        assert!(next_unanswered(&entries, 3.0).is_some());
        assert!(next_unanswered(&entries[..4], 1.0).is_none());
    }

    #[test]
    fn next_unanswered_retries_when_only_a_pending_placeholder_follows() {
        let entries = vec![
            entry(10.0, ChatRole::User, "卡住了吗", false),
            entry(10.1, ChatRole::Assistant, "思考中…", true),
        ];
        let next = next_unanswered(&entries, 0.0).expect("pending is not an answer");
        assert_eq!(next.text, "卡住了吗");
    }

    #[test]
    fn next_unanswered_returns_the_earliest_of_two_open_users() {
        let entries = vec![
            entry(1.0, ChatRole::User, "第一句", false),
            entry(2.0, ChatRole::User, "第二句", false),
        ];
        let next = next_unanswered(&entries, 0.0).expect("first user");
        assert_eq!(next.text, "第一句");
    }
}
