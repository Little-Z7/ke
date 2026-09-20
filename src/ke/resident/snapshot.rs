//! Added by ke: a text snapshot of agent panes for the resident's model context.
//!
//! The panel loop already refreshes the cross-session roster into `Shared`. This module formats
//! that roster and reads the recent buffer of focused and blocked panes. A failed pane read
//! becomes one line of "读失败" so the rest of the snapshot still goes out.
//!
//! The roster spans every running session, but pane *contents* are read only from the local
//! session. A project can be marked as never leaving the machine, and that marking belongs to the
//! session that holds it; pulling another session's buffers through this session's model would
//! route around it. Cross-session rows therefore carry status only, and the snapshot says so, so
//! the model does not claim to have read what it cannot see.

use std::collections::HashSet;
use std::time::Duration;

use super::sessions::SessionAgent;
use crate::api::client::{parse_response_value, ApiClient};
use crate::api::schema::{
    AgentStatus, Method, PaneReadParams, ReadFormat, ReadIntent, ReadSource, Request,
    ResponseResult,
};

const PANE_READ_LINES: u32 = 40;
const PANE_READ_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PANE_CHARS: usize = 4000;
const MAX_SNAPSHOT_CHARS: usize = 20_000;

/// Builds the context snapshot: cross-session roster plus recent text of local focused/blocked
/// panes.
pub(crate) fn capture(agents: &[SessionAgent], local: &ApiClient) -> String {
    let mut out = format_agents(agents);
    if out.is_empty() {
        out.push_str("（没有 agent 窗格）");
    }
    if agents.iter().any(|agent| !agent.local) {
        out.push_str("\n（其它会话只有上面的状态，没有窗格内容；要看内容请让用户切过去）");
    }
    for agent in agents_to_read(agents) {
        out.push('\n');
        out.push_str(&format_pane_body(agent, local));
    }
    truncate_chars(out, MAX_SNAPSHOT_CHARS)
}

/// One labeled line per agent. Pure; used by tests and as the snapshot header.
pub(crate) fn format_agents(agents: &[SessionAgent]) -> String {
    let mut lines = Vec::with_capacity(agents.len());
    for agent in agents {
        let info = &agent.info;
        let name = info
            .display_agent
            .as_deref()
            .or(info.agent.as_deref())
            .or(info.name.as_deref())
            .unwrap_or("agent");
        let cwd = info.cwd.as_deref().unwrap_or("-");
        let blocked = info.agent_status == AgentStatus::Blocked;
        lines.push(format!(
            "session={} local={} pane_id={} agent={} status={} cwd={} focused={} blocked={}",
            agent.session,
            agent.local,
            info.pane_id,
            name,
            status_name(info.agent_status),
            cwd,
            info.focused,
            blocked
        ));
    }
    lines.join("\n")
}

/// Only local panes are readable; `pane_id` is unique within that one session.
fn agents_to_read(agents: &[SessionAgent]) -> Vec<&SessionAgent> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for agent in agents {
        if !agent.local {
            continue;
        }
        let info = &agent.info;
        if !(info.focused || info.agent_status == AgentStatus::Blocked) {
            continue;
        }
        if !seen.insert(info.pane_id.as_str()) {
            continue;
        }
        out.push(agent);
    }
    out
}

fn format_pane_body(agent: &SessionAgent, client: &ApiClient) -> String {
    let info = &agent.info;
    let kind = if info.focused && info.agent_status == AgentStatus::Blocked {
        "focused,blocked"
    } else if info.focused {
        "focused"
    } else {
        "blocked"
    };
    let header = format!("### pane {} ({kind})", info.pane_id);
    match read_pane(client, &info.pane_id) {
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
    use crate::ke::resident::sessions::test_support::session_agent;

    fn agent(
        session: &str,
        local: bool,
        pane_id: &str,
        name: &str,
        status: AgentStatus,
        focused: bool,
        cwd: Option<&str>,
        display_agent: Option<&str>,
    ) -> SessionAgent {
        let mut out = session_agent(session, local, pane_id, name, status);
        out.info.focused = focused;
        out.info.cwd = cwd.map(str::to_owned);
        out.info.display_agent = display_agent.map(str::to_owned);
        out
    }

    #[test]
    fn format_agents_lists_session_identity_status_and_flags() {
        let agents = vec![
            agent(
                "ke",
                true,
                "w1:p1",
                "claude",
                AgentStatus::Working,
                true,
                Some("/proj"),
                Some("Claude"),
            ),
            agent(
                "work",
                false,
                "w1:p2",
                "codex",
                AgentStatus::Blocked,
                false,
                None,
                None,
            ),
        ];
        let text = format_agents(&agents);
        assert_eq!(
            text,
            "session=ke local=true pane_id=w1:p1 agent=Claude status=working cwd=/proj focused=true blocked=false\n\
             session=work local=false pane_id=w1:p2 agent=codex status=blocked cwd=- focused=false blocked=true"
        );
        assert!(format_agents(&[]).is_empty());
    }

    #[test]
    fn only_local_panes_are_read() {
        let agents = vec![
            agent(
                "ke",
                true,
                "w1:p1",
                "claude",
                AgentStatus::Blocked,
                false,
                None,
                None,
            ),
            agent(
                "work",
                false,
                "w1:p9",
                "codex",
                AgentStatus::Blocked,
                true,
                None,
                None,
            ),
        ];
        let readable: Vec<&str> = agents_to_read(&agents)
            .iter()
            .map(|agent| agent.info.pane_id.as_str())
            .collect();
        assert_eq!(
            readable,
            vec!["w1:p1"],
            "a blocked, focused pane in another session must not be read through this session"
        );
    }

    #[test]
    fn truncate_chars_caps_length() {
        assert_eq!(truncate_chars("abcd".into(), 3), "abc…");
        assert_eq!(truncate_chars("ab".into(), 3), "ab");
    }
}
