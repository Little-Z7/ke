//! Added by ke: the deterministic part of the sidebar panel.
//!
//! One row per agent pane with its status and how long it has been in that status. This never
//! involves a model, so the panel stays useful when no model is configured or reachable. The model
//! half (later milestone) prepends a digest row and suggestion rows carrying `input`.

use std::collections::HashMap;
use std::time::Instant;

use crate::api::schema::agents::AgentInfo;
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
#[derive(Debug, Default)]
pub(crate) struct StatusTracker {
    since: HashMap<String, (AgentStatus, u64, Instant)>,
}

impl StatusTracker {
    pub(crate) fn observe(&mut self, agents: &[AgentInfo], now: Instant) {
        let mut seen = std::collections::HashSet::new();
        for agent in agents {
            seen.insert(agent.pane_id.clone());
            match self.since.get(&agent.pane_id) {
                Some((status, seq, _))
                    if *status == agent.agent_status && *seq == agent.state_change_seq => {}
                _ => {
                    self.since.insert(
                        agent.pane_id.clone(),
                        (agent.agent_status, agent.state_change_seq, now),
                    );
                }
            }
        }
        self.since.retain(|pane_id, _| seen.contains(pane_id));
    }

    fn age(&self, pane_id: &str, now: Instant) -> Option<std::time::Duration> {
        self.since
            .get(pane_id)
            .map(|(_, _, since)| now.saturating_duration_since(*since))
    }
}

pub(crate) fn build(agents: &[AgentInfo], tracker: &StatusTracker, now: Instant) -> Panel {
    let mut ordered: Vec<&AgentInfo> = agents.iter().collect();
    ordered.sort_by_key(|agent| (status_rank(agent.agent_status), agent.pane_id.clone()));
    let rows = ordered
        .iter()
        .take(MAX_ROWS)
        .map(|agent| {
            let name = agent
                .display_agent
                .as_deref()
                .or(agent.agent.as_deref())
                .or(agent.name.as_deref())
                .unwrap_or("agent");
            let age = tracker
                .age(&agent.pane_id, now)
                .filter(|age| age.as_secs() >= 60)
                .map(format_age)
                .map(|age| format!(" · {age}"))
                .unwrap_or_default();
            PanelRow {
                text: format!("{name} {}{age}", status_label(agent.agent_status)),
                level: status_level(agent.agent_status),
                input: None,
            }
        })
        .collect();
    Panel {
        v: 1,
        ts: crate::ke::chat_log::now_epoch(),
        title: "ke".into(),
        rows,
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
    use std::time::Duration;

    fn agent(pane_id: &str, agent: &str, status: AgentStatus, seq: u64) -> AgentInfo {
        AgentInfo {
            terminal_id: format!("term-{pane_id}"),
            name: None,
            agent: Some(agent.into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            display_agent: None,
            agent_status: status,
            screen_detection_skipped: false,
            state_labels: HashMap::new(),
            tokens: HashMap::new(),
            agent_session: None,
            workspace_id: "w1".into(),
            tab_id: "w1:t1".into(),
            pane_id: pane_id.into(),
            focused: false,
            launch_pending: false,
            interactive_ready: true,
            state_change_seq: seq,
            cwd: None,
            foreground_cwd: None,
            revision: 1,
        }
    }

    #[test]
    fn blocked_agents_come_first_with_their_age() {
        let start = Instant::now();
        let mut tracker = StatusTracker::default();
        let agents = vec![
            agent("w1:p1", "claude", AgentStatus::Working, 1),
            agent("w1:p2", "codex", AgentStatus::Blocked, 4),
            agent("w1:p3", "kimi", AgentStatus::Idle, 2),
        ];
        tracker.observe(&agents, start);
        let panel = build(&agents, &tracker, start + Duration::from_secs(200));
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
        let mut agents = vec![agent("w1:p1", "claude", AgentStatus::Working, 1)];
        tracker.observe(&agents, start);
        let panel = build(&agents, &tracker, start + Duration::from_secs(30));
        assert_eq!(
            panel.rows[0].text, "claude 工作中",
            "under a minute shows no age"
        );

        agents[0].agent_status = AgentStatus::Blocked;
        agents[0].state_change_seq = 2;
        tracker.observe(&agents, start + Duration::from_secs(600));
        let panel = build(&agents, &tracker, start + Duration::from_secs(660));
        assert_eq!(panel.rows[0].text, "claude 等你 · 1m");

        tracker.observe(&[], start + Duration::from_secs(700));
        assert!(tracker.since.is_empty(), "gone panes are forgotten");
        assert_eq!(format_age(Duration::from_secs(3 * 3600 + 5 * 60)), "3h05m");
        assert_eq!(format_age(Duration::from_secs(2 * 86_400)), "2d");
    }
}
