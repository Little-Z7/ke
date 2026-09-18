//! Added by ke: a text snapshot of agent panes for the resident's model context.
//!
//! The panel loop already refreshes `agent.list` into `Shared`. This module formats that list and
//! reads the recent buffer of focused and blocked panes. A failed pane read becomes one line of
//! "读失败" so the rest of the snapshot still goes out.

use std::collections::HashSet;
use std::time::Duration;

use crate::api::client::{parse_response_value, ApiClient};
use crate::api::schema::agents::AgentInfo;
use crate::api::schema::{
    AgentStatus, Method, PaneReadParams, ReadFormat, ReadIntent, ReadSource, Request,
    ResponseResult,
};

const PANE_READ_LINES: u32 = 40;
const PANE_READ_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PANE_CHARS: usize = 4000;
const MAX_SNAPSHOT_CHARS: usize = 20_000;

/// Builds the context snapshot: agent roster plus recent text of focused/blocked panes.
pub(crate) fn capture(agents: &[AgentInfo], client: &ApiClient) -> String {
    let mut out = format_agents(agents);
    if out.is_empty() {
        out.push_str("（没有 agent 窗格）");
    }
    for agent in agents_to_read(agents) {
        out.push('\n');
        out.push_str(&format_pane_body(agent, client));
    }
    truncate_chars(out, MAX_SNAPSHOT_CHARS)
}

/// One labeled line per agent. Pure; used by tests and as the snapshot header.
pub(crate) fn format_agents(agents: &[AgentInfo]) -> String {
    let mut lines = Vec::with_capacity(agents.len());
    for agent in agents {
        let name = agent
            .display_agent
            .as_deref()
            .or(agent.agent.as_deref())
            .or(agent.name.as_deref())
            .unwrap_or("agent");
        let cwd = agent.cwd.as_deref().unwrap_or("-");
        let blocked = agent.agent_status == AgentStatus::Blocked;
        lines.push(format!(
            "pane_id={} agent={} status={} cwd={} focused={} blocked={}",
            agent.pane_id,
            name,
            status_name(agent.agent_status),
            cwd,
            agent.focused,
            blocked
        ));
    }
    lines.join("\n")
}

fn agents_to_read(agents: &[AgentInfo]) -> Vec<&AgentInfo> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for agent in agents {
        if !(agent.focused || agent.agent_status == AgentStatus::Blocked) {
            continue;
        }
        if !seen.insert(agent.pane_id.as_str()) {
            continue;
        }
        out.push(agent);
    }
    out
}

fn format_pane_body(agent: &AgentInfo, client: &ApiClient) -> String {
    let kind = if agent.focused && agent.agent_status == AgentStatus::Blocked {
        "focused,blocked"
    } else if agent.focused {
        "focused"
    } else {
        "blocked"
    };
    let header = format!("### pane {} ({kind})", agent.pane_id);
    match read_pane(client, &agent.pane_id) {
        Ok(text) => format!("{header}\n{}", truncate_chars(text, MAX_PANE_CHARS)),
        Err(_) => format!("{header}\n读失败"),
    }
}

fn read_pane(client: &ApiClient, pane_id: &str) -> Result<String, String> {
    let value = client
        .request_value_with_timeout(
            &Request {
                id: format!("ke-resident:pane.read:{pane_id}"),
                method: Method::PaneRead(PaneReadParams {
                    pane_id: pane_id.to_owned(),
                    source: ReadSource::Recent,
                    lines: Some(PANE_READ_LINES),
                    format: ReadFormat::Text,
                    strip_ansi: true,
                    intent: ReadIntent::Passive,
                }),
            },
            PANE_READ_TIMEOUT,
        )
        .map_err(|err| err.to_string())?;
    match parse_response_value(value).map_err(|err| err.to_string())? {
        crate::api::schema::SuccessResponse {
            result: ResponseResult::PaneRead { read },
            ..
        } => Ok(read.text),
        other => Err(format!("unexpected pane.read response: {other:?}")),
    }
}

fn status_name(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Idle => "idle",
        AgentStatus::Working => "working",
        AgentStatus::Blocked => "blocked",
        AgentStatus::Done => "done",
        AgentStatus::Unknown => "unknown",
    }
}

fn truncate_chars(mut text: String, max: usize) -> String {
    if let Some((index, _)) = text.char_indices().nth(max) {
        text.truncate(index);
        text.push('…');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn agent(
        pane_id: &str,
        agent: &str,
        status: AgentStatus,
        focused: bool,
        cwd: Option<&str>,
        display_agent: Option<&str>,
    ) -> AgentInfo {
        AgentInfo {
            terminal_id: format!("term-{pane_id}"),
            name: None,
            agent: Some(agent.into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            display_agent: display_agent.map(str::to_owned),
            agent_status: status,
            screen_detection_skipped: false,
            state_labels: HashMap::new(),
            tokens: HashMap::new(),
            agent_session: None,
            workspace_id: "w1".into(),
            tab_id: "w1:t1".into(),
            pane_id: pane_id.into(),
            focused,
            launch_pending: false,
            interactive_ready: true,
            state_change_seq: 1,
            cwd: cwd.map(str::to_owned),
            foreground_cwd: None,
            revision: 1,
        }
    }

    #[test]
    fn format_agents_lists_identity_status_and_flags() {
        let agents = vec![
            agent(
                "w1:p1",
                "claude",
                AgentStatus::Working,
                true,
                Some("/proj"),
                Some("Claude"),
            ),
            agent("w1:p2", "codex", AgentStatus::Blocked, false, None, None),
        ];
        let text = format_agents(&agents);
        assert_eq!(
            text,
            "pane_id=w1:p1 agent=Claude status=working cwd=/proj focused=true blocked=false\n\
             pane_id=w1:p2 agent=codex status=blocked cwd=- focused=false blocked=true"
        );
        assert!(format_agents(&[]).is_empty());
    }

    #[test]
    fn truncate_chars_caps_length() {
        assert_eq!(truncate_chars("abcd".into(), 3), "abc…");
        assert_eq!(truncate_chars("ab".into(), 3), "ab");
    }
}
