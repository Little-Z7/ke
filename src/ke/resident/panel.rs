//! Added by ke: the deterministic part of the sidebar panel.
//!
//! One row per agent pane with its status and how long it has been in that status, across every
//! running session (see `sessions`). This never involves a model, so the panel stays useful when
//! no model is configured or reachable. The model half (later milestone) prepends a digest row and
//! suggestion rows carrying `input`.
//!
//! Rows from other sessions carry a `[name]` prefix, and ages are tracked per `(session, pane_id)`
//! because pane ids repeat across sessions.

use std::collections::HashMap;
use std::time::Instant;

use super::sessions::{AgentKey, SessionAgent};
use crate::api::schema::AgentStatus;

/// Matches `client::shell::ke_panel::KE_PANEL_MAX_ROWS`.
const MAX_ROWS: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct PanelRow {
    pub text: String,
    pub level: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct Panel {
    pub v: u8,
    pub ts: f64,
    pub title: String,
    pub rows: Vec<PanelRow>,
}

impl Panel {
    pub(crate) fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }
}

/// Remembers when each agent last changed status so rows can say "blocked · 3m".
///
/// Keyed by `(session, pane_id)`: two sessions routinely hold the same `w1:p1`, and collapsing
/// them would make one pane's age follow the other's status changes.
#[derive(Debug, Default)]
pub(crate) struct StatusTracker {
    since: HashMap<AgentKey, (AgentStatus, u64, Instant)>,
}

impl StatusTracker {
    pub(crate) fn observe(&mut self, agents: &[SessionAgent], now: Instant) {
        let mut seen = std::collections::HashSet::new();
        for agent in agents {
            let key = agent.key();
            let info = &agent.info;
            seen.insert(key.clone());
            match self.since.get(&key) {
                Some((status, seq, _))
                    if *status == info.agent_status && *seq == info.state_change_seq => {}
                _ => {
                    self.since
                        .insert(key, (info.agent_status, info.state_change_seq, now));
                }
            }
        }
        self.since.retain(|key, _| seen.contains(key));
    }

    fn age(&self, key: &AgentKey, now: Instant) -> Option<std::time::Duration> {
        self.since
            .get(key)
            .map(|(_, _, since)| now.saturating_duration_since(*since))
    }
}

/// Builds the panel from the whole cross-session roster.
///
/// `unreachable` names sessions that failed to answer this round; they get one dim row each so a
/// stalled session reads as "unknown" instead of silently having no agents.
pub(crate) fn build(
    agents: &[SessionAgent],
    unreachable: &[String],
    tracker: &StatusTracker,
    now: Instant,
) -> Panel {
    let mut ordered: Vec<&SessionAgent> = agents.iter().collect();
    // Whoever is waiting comes first regardless of session; the local session breaks ties so your
    // own panes stay where you expect them.
    ordered.sort_by(|a, b| {
        status_rank(a.info.agent_status)
            .cmp(&status_rank(b.info.agent_status))
            .then_with(|| b.local.cmp(&a.local))
            .then_with(|| a.session.cmp(&b.session))
            .then_with(|| a.info.pane_id.cmp(&b.info.pane_id))
    });
    let mut rows: Vec<PanelRow> = ordered
        .iter()
        .take(MAX_ROWS)
        .map(|agent| {
            let info = &agent.info;
            let name = info
                .display_agent
                .as_deref()
                .or(info.agent.as_deref())
                .or(info.name.as_deref())
                .unwrap_or("agent");
            let age = tracker
                .age(&agent.key(), now)
                .filter(|age| age.as_secs() >= 60)
                .map(format_age)
                .map(|age| format!(" · {age}"))
                .unwrap_or_default();
            PanelRow {
                text: format!(
                    "{}{name} {}{age}",
                    session_prefix(agent),
                    status_label(info.agent_status)
                ),
                level: status_level(info.agent_status),
                input: None,
            }
        })
        .collect();
    for session in unreachable {
        if rows.len() >= MAX_ROWS {
            break;
        }
        rows.push(PanelRow {
            text: format!("[{session}] 连不上"),
            level: "dim",
            input: None,
        });
    }
    Panel {
        v: 1,
        ts: crate::ke::chat_log::now_epoch(),
        title: "ke".into(),
        rows,
    }
}

/// Rows from the session you are looking at stay unprefixed; everything else is labeled.
fn session_prefix(agent: &SessionAgent) -> String {
    if agent.local {
        String::new()
    } else {
        format!("[{}] ", agent.session)
    }
}

fn status_rank(status: AgentStatus) -> u8 {
    match status {
        AgentStatus::Blocked => 0,
        AgentStatus::Done => 1,
        AgentStatus::Working => 2,
        AgentStatus::Idle => 3,
        AgentStatus::Unknown => 4,
    }
}

fn status_label(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Blocked => "等你",
        AgentStatus::Done => "完成",
        AgentStatus::Working => "工作中",
        AgentStatus::Idle => "空闲",
        AgentStatus::Unknown => "未知",
    }
}

fn status_level(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Blocked => "warn",
        AgentStatus::Done => "ok",
        AgentStatus::Working => "info",
        AgentStatus::Idle => "dim",
        AgentStatus::Unknown => "dim",
    }
}

fn format_age(age: std::time::Duration) -> String {
    let secs = age.as_secs();
    if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ke::resident::sessions::test_support::session_agent;
    use std::time::Duration;

    fn local(pane_id: &str, agent: &str, status: AgentStatus, seq: u64) -> SessionAgent {
        let mut out = session_agent("ke", true, pane_id, agent, status);
        out.info.state_change_seq = seq;
        out
    }

    fn remote(
        session: &str,
        pane_id: &str,
        agent: &str,
        status: AgentStatus,
        seq: u64,
    ) -> SessionAgent {
        let mut out = session_agent(session, false, pane_id, agent, status);
        out.info.state_change_seq = seq;
        out
    }

    #[test]
    fn blocked_agents_come_first_with_their_age() {
        let start = Instant::now();
        let mut tracker = StatusTracker::default();
        let agents = vec![
            local("w1:p1", "claude", AgentStatus::Working, 1),
            local("w1:p2", "codex", AgentStatus::Blocked, 4),
            local("w1:p3", "kimi", AgentStatus::Idle, 2),
        ];
        tracker.observe(&agents, start);
        let panel = build(&agents, &[], &tracker, start + Duration::from_secs(200));
        assert_eq!(panel.title, "ke");
        assert_eq!(panel.rows.len(), 3);
        assert_eq!(panel.rows[0].text, "codex 等你 · 3m");
        assert_eq!(panel.rows[0].level, "warn");
        assert_eq!(panel.rows[1].text, "claude 工作中 · 3m");
        assert_eq!(panel.rows[2].level, "dim");
        assert!(panel.rows.iter().all(|row| row.input.is_none()));
        let json: serde_json::Value = serde_json::from_str(&panel.to_json()).unwrap();
        assert_eq!(json["v"], 1);
        assert!(json["ts"].as_f64().unwrap() > 0.0);
        assert!(json["rows"][0].get("input").is_none(), "absent, not null");
    }

    #[test]
    fn age_resets_when_status_or_sequence_changes_and_young_ages_are_hidden() {
        let start = Instant::now();
        let mut tracker = StatusTracker::default();
        let mut agents = vec![local("w1:p1", "claude", AgentStatus::Working, 1)];
        tracker.observe(&agents, start);
        let panel = build(&agents, &[], &tracker, start + Duration::from_secs(30));
        assert_eq!(
            panel.rows[0].text, "claude 工作中",
            "under a minute shows no age"
        );

        agents[0].info.agent_status = AgentStatus::Blocked;
        agents[0].info.state_change_seq = 2;
        tracker.observe(&agents, start + Duration::from_secs(600));
        let panel = build(&agents, &[], &tracker, start + Duration::from_secs(660));
        assert_eq!(panel.rows[0].text, "claude 等你 · 1m");

        tracker.observe(&[], start + Duration::from_secs(700));
        assert!(tracker.since.is_empty(), "gone panes are forgotten");
        assert_eq!(format_age(Duration::from_secs(3 * 3600 + 5 * 60)), "3h05m");
        assert_eq!(format_age(Duration::from_secs(2 * 86_400)), "2d");
    }

    #[test]
    fn remote_rows_are_labeled_and_waiting_agents_outrank_the_local_session() {
        let start = Instant::now();
        let mut tracker = StatusTracker::default();
        let agents = vec![
            local("w1:p1", "claude", AgentStatus::Working, 1),
            remote("work", "w1:p1", "codex", AgentStatus::Blocked, 1),
        ];
        tracker.observe(&agents, start);
        let panel = build(&agents, &[], &tracker, start + Duration::from_secs(120));
        assert_eq!(
            panel.rows[0].text, "[work] codex 等你 · 2m",
            "another session's blocked agent is the whole point of the panel"
        );
        assert_eq!(
            panel.rows[1].text, "claude 工作中 · 2m",
            "the local session stays unprefixed"
        );
    }

    #[test]
    fn identical_pane_ids_in_two_sessions_keep_separate_ages() {
        let start = Instant::now();
        let mut tracker = StatusTracker::default();
        let mut agents = vec![
            local("w1:p1", "claude", AgentStatus::Blocked, 1),
            remote("work", "w1:p1", "codex", AgentStatus::Working, 1),
        ];
        tracker.observe(&agents, start);
        // Only the remote pane changes status; the local pane's age must keep running.
        agents[1].info.agent_status = AgentStatus::Blocked;
        agents[1].info.state_change_seq = 2;
        tracker.observe(&agents, start + Duration::from_secs(600));

        let panel = build(&agents, &[], &tracker, start + Duration::from_secs(660));
        let texts: Vec<&str> = panel.rows.iter().map(|row| row.text.as_str()).collect();
        assert!(
            texts.contains(&"claude 等你 · 11m"),
            "local age survived the remote change: {texts:?}"
        );
        assert!(
            texts.contains(&"[work] codex 等你 · 1m"),
            "remote age restarted: {texts:?}"
        );
    }

    #[test]
    fn unreachable_sessions_get_a_dim_row() {
        let start = Instant::now();
        let mut tracker = StatusTracker::default();
        let agents = vec![local("w1:p1", "claude", AgentStatus::Idle, 1)];
        tracker.observe(&agents, start);
        let panel = build(&agents, &["work".to_string()], &tracker, start);
        assert_eq!(panel.rows.len(), 2);
        assert_eq!(panel.rows[1].text, "[work] 连不上");
        assert_eq!(panel.rows[1].level, "dim");
    }

    #[test]
    fn rows_never_exceed_the_client_limit() {
        let start = Instant::now();
        let mut tracker = StatusTracker::default();
        let agents: Vec<SessionAgent> = (0..MAX_ROWS + 4)
            .map(|index| local(&format!("w1:p{index}"), "claude", AgentStatus::Idle, 1))
            .collect();
        tracker.observe(&agents, start);
        let unreachable: Vec<String> = vec!["a".into(), "b".into()];
        let panel = build(&agents, &unreachable, &tracker, start);
        assert_eq!(panel.rows.len(), MAX_ROWS);
    }
}
