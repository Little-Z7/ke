//! Added by ke: the resident's cross-session view.
//!
//! A resident serves one session, but a shell user routinely runs several (`ke --session work`).
//! A manager that only sees its own session cannot answer "who is waiting for me", so the roster
//! is collected from every running session instead of just the local one.
//!
//! Two invariants hold everything together:
//!
//! - `pane_id` is unique only inside a session, so anything keyed by pane identity must key on
//!   `(session, pane_id)`. `AgentKey` is that key.
//! - Only the local session's failures are fatal. A remote session that is stopping, wedged, or
//!   running an older build must cost its own rows and nothing else.
//!
//! Pane *contents* are deliberately not collected across sessions; see `snapshot`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::api::client::{ApiClient, ConnectionTarget};
use crate::api::schema::agents::AgentInfo;
use crate::api::schema::{EmptyParams, Method, Request, ResponseResult};

/// Identity of one agent pane across sessions.
pub(crate) type AgentKey = (String, String);

/// One agent pane plus the session it was found in.
#[derive(Debug, Clone)]
pub(crate) struct SessionAgent {
    pub session: String,
    /// True for the session this resident serves.
    pub local: bool,
    pub info: AgentInfo,
}

impl SessionAgent {
    pub(crate) fn key(&self) -> AgentKey {
        (self.session.clone(), self.info.pane_id.clone())
    }
}

/// A session this resident can query.
#[derive(Debug, Clone)]
pub(crate) struct SessionClient {
    pub name: String,
    pub local: bool,
    pub client: ApiClient,
}

/// Result of one polling round.
#[derive(Debug, Default)]
pub(crate) struct Roster {
    pub agents: Vec<SessionAgent>,
    /// Sessions that failed to answer, for logging and degraded rows.
    pub unreachable: Vec<String>,
    /// Whether the local session answered. Only this drives resident shutdown.
    pub local_ok: bool,
}

/// Session name plus socket, as resolved from the session directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Discovered {
    pub name: String,
    pub local: bool,
    pub socket: PathBuf,
}

/// Enumerates running sessions and marks the one this resident serves.
///
/// `ApiClient` only holds a target and connects per request, so rebuilding these every round is
/// free and picks up sessions started after the resident.
pub(crate) fn discover(local_socket: &Path) -> Vec<SessionClient> {
    let listed = crate::session::list_sessions().unwrap_or_default();
    classify(&listed, local_socket)
        .into_iter()
        .map(|found| SessionClient {
            name: found.name,
            local: found.local,
            client: ApiClient::for_target(ConnectionTarget::SocketPath(found.socket)),
        })
        .collect()
}

/// Pure half of `discover`: keep running sessions, mark the local one, and guarantee the local
/// session is present even when it is not listed (an explicit `--api-socket` outside the session
/// directory, or a directory that has not been created yet).
pub(crate) fn classify(
    listed: &[crate::session::SessionInfo],
    local_socket: &Path,
) -> Vec<Discovered> {
    let mut out: Vec<Discovered> = listed
        .iter()
        .filter(|info| info.running)
        .map(|info| Discovered {
            name: info.name.clone(),
            local: same_socket(Path::new(&info.socket_path), local_socket),
            socket: PathBuf::from(&info.socket_path),
        })
        .collect();
    if !out.iter().any(|found| found.local) {
        out.insert(
            0,
            Discovered {
                name: local_name(listed, local_socket),
                local: true,
                socket: local_socket.to_path_buf(),
            },
        );
    }
    // Local session first, then by name, so the panel reads as "mine, then everyone else".
    out.sort_by(|a, b| b.local.cmp(&a.local).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Name for a local session missing from the listing: the listed entry if its socket matches, the
/// default session when the socket sits directly in the config directory, otherwise the socket's
/// own directory name, which is how named sessions are laid out.
fn local_name(listed: &[crate::session::SessionInfo], local_socket: &Path) -> String {
    for info in listed {
        if same_socket(Path::new(&info.socket_path), local_socket) {
            return info.name.clone();
        }
    }
    let parent = local_socket.parent();
    if parent.is_some_and(|parent| same_socket(parent, &crate::config::config_dir())) {
        return crate::session::DEFAULT_SESSION_NAME.to_string();
    }
    parent
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| crate::session::DEFAULT_SESSION_NAME.to_string())
}

/// Sockets come from the same path builders, so a string compare is enough; `canonicalize` is a
/// best-effort refinement for symlinked state directories and must not fail the comparison.
fn same_socket(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// Polls every session for its agents. Remote failures are recorded, not propagated.
pub(crate) fn fetch_all(sessions: &[SessionClient], timeout: Duration) -> Roster {
    let mut roster = Roster::default();
    for session in sessions {
        match fetch_agents(&session.client, timeout) {
            Ok(agents) => {
                if session.local {
                    roster.local_ok = true;
                }
                roster
                    .agents
                    .extend(agents.into_iter().map(|info| SessionAgent {
                        session: session.name.clone(),
                        local: session.local,
                        info,
                    }));
            }
            Err(err) => {
                if session.local {
                    eprintln!("ke resident: local agent.list failed: {err}");
                } else {
                    roster.unreachable.push(session.name.clone());
                }
            }
        }
    }
    roster
}

fn fetch_agents(client: &ApiClient, timeout: Duration) -> Result<Vec<AgentInfo>, String> {
    let value = client
        .request_value_with_timeout(
            &Request {
                id: "ke-resident:agent.list".into(),
                method: Method::AgentList(EmptyParams::default()),
            },
            timeout,
        )
        .map_err(|err| err.to_string())?;
    match crate::api::client::parse_response_value(value).map_err(|err| err.to_string())? {
        crate::api::schema::SuccessResponse {
            result: ResponseResult::AgentList { agents },
            ..
        } => Ok(agents),
        other => Err(format!("unexpected agent.list response: {other:?}")),
    }
}

/// Builders shared by the resident's tests, so panel, snapshot, and session tests all describe a
/// roster the same way.
#[cfg(test)]
pub(crate) mod test_support {
    use super::SessionAgent;
    use crate::api::schema::agents::AgentInfo;
    use crate::api::schema::AgentStatus;
    use std::collections::HashMap;

    pub(crate) fn agent_info(pane_id: &str, agent: &str, status: AgentStatus) -> AgentInfo {
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
            state_change_seq: 1,
            cwd: None,
            foreground_cwd: None,
            revision: 1,
        }
    }

    pub(crate) fn session_agent(
        session: &str,
        local: bool,
        pane_id: &str,
        agent: &str,
        status: AgentStatus,
    ) -> SessionAgent {
        SessionAgent {
            session: session.into(),
            local,
            info: agent_info(pane_id, agent, status),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::AgentStatus;

    fn info(name: &str, running: bool, socket: &str) -> crate::session::SessionInfo {
        crate::session::SessionInfo {
            name: name.into(),
            default: name == crate::session::DEFAULT_SESSION_NAME,
            running,
            socket_path: socket.into(),
            session_dir: socket.trim_end_matches("/herdr.sock").into(),
        }
    }

    #[test]
    fn classify_keeps_running_sessions_and_marks_the_local_one() {
        let listed = vec![
            info("default", true, "/state/ke/herdr.sock"),
            info("work", true, "/state/ke/sessions/work/herdr.sock"),
            info("stopped", false, "/state/ke/sessions/stopped/herdr.sock"),
        ];
        let found = classify(&listed, Path::new("/state/ke/sessions/work/herdr.sock"));
        assert_eq!(found.len(), 2, "stopped sessions are dropped");
        assert_eq!(found[0].name, "work", "local session sorts first");
        assert!(found[0].local);
        assert_eq!(found[1].name, "default");
        assert!(!found[1].local);
    }

    #[test]
    fn classify_inserts_a_local_session_that_is_not_listed() {
        let listed = vec![info("default", true, "/state/ke/herdr.sock")];
        let found = classify(&listed, Path::new("/tmp/custom/herdr.sock"));
        assert_eq!(found.len(), 2);
        assert!(found[0].local, "the served session is always present");
        assert_eq!(
            found[0].name, "custom",
            "an unlisted socket is named after its directory"
        );
        assert_eq!(found[0].socket, PathBuf::from("/tmp/custom/herdr.sock"));
    }

    #[test]
    fn classify_without_any_running_session_still_yields_the_local_one() {
        let socket = crate::config::config_dir().join("herdr.sock");
        let found = classify(&[], &socket);
        assert_eq!(found.len(), 1);
        assert!(found[0].local);
        assert_eq!(
            found[0].name,
            crate::session::DEFAULT_SESSION_NAME,
            "a socket directly in the config dir is the default session, not the app directory"
        );
    }

    #[test]
    fn agent_key_separates_identical_pane_ids_in_different_sessions() {
        let left =
            test_support::session_agent("default", true, "w1:p1", "claude", AgentStatus::Idle);
        let right =
            test_support::session_agent("work", false, "w1:p1", "claude", AgentStatus::Idle);
        assert_ne!(
            left.key(),
            right.key(),
            "pane ids repeat across sessions; the key must not collapse them"
        );
    }
}
