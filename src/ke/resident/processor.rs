//! Added by ke: the composer processor half of the resident.
//!
//! Serves the protocol the client's composer bar speaks (see `client::shell::composer`): one JSON
//! request line in, one JSON reply line out.
//!
//! - Ordinary text goes through the redaction gate and is returned as `send` with a note when
//!   something was replaced.
//! - `@ke …` is the user talking to the resident: the message is logged to `chat.jsonl` and the
//!   reply is `done` with a "思考中…" note. The model half answers asynchronously on the same log.
//! - Anything malformed is `block`: the gate fails closed.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use interprocess::local_socket::traits::{Listener as _, Stream as _};
use interprocess::local_socket::ListenerNonblockingMode;

use crate::ipc::{self, LocalListener, LocalStream};
use crate::ke::chat_log::{self, ChatEntry, ChatRole};
use crate::ke::paths::ResidentPaths;
use crate::ke::redact::{self, RedactRules};

pub(crate) const KE_MENTION: &str = "@ke";
const ACCEPT_POLL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct ComposerRequest {
    pub pane_id: String,
    #[serde(default)]
    pub agent: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    Send { text: String, note: Option<String> },
    Block { note: String },
    Done { note: Option<String> },
}

impl Decision {
    fn to_json(&self) -> String {
        let value = match self {
            Decision::Send { text, note } => serde_json::json!({
                "action": "send", "text": text, "note": note,
            }),
            Decision::Block { note } => serde_json::json!({ "action": "block", "note": note }),
            Decision::Done { note } => serde_json::json!({ "action": "done", "note": note }),
        };
        value.to_string()
    }
}

/// Pure decision for one request; the socket plumbing around it is `Processor::serve`.
pub(crate) fn decide(
    request: &ComposerRequest,
    rules: RedactRules,
    chat_log: &std::path::Path,
) -> Decision {
    let text = request.text.trim();
    if let Some(message) = text.strip_prefix(KE_MENTION) {
        let message = message.trim();
        if message.is_empty() {
            return Decision::Done {
                note: Some("@ke 后面跟你要说的话".into()),
            };
        }
        let mut entry = ChatEntry::now(ChatRole::User, message);
        entry.pane_id = Some(request.pane_id.clone());
        if let Err(err) = chat_log::append(chat_log, &entry) {
            return Decision::Block {
                note: format!("管家记不下这句话（{err}），未发送"),
            };
        }
        return Decision::Done {
            note: Some("思考中…".into()),
        };
    }
    let redaction = redact::redact(&request.text, rules);
    Decision::Send {
        text: redaction.text,
        note: (redaction.count > 0).then(|| format!("脱敏 {} 处", redaction.count)),
    }
}

pub(crate) struct Processor {
    listener: LocalListener,
    socket_path: PathBuf,
    chat_log: PathBuf,
    rules: RedactRules,
    #[allow(dead_code)] // roster lives on Shared for the answerer; processor keeps a clone
    shared: Arc<super::Shared>,
    stop: Arc<AtomicBool>,
}

impl Processor {
    pub(crate) fn bind(
        paths: &ResidentPaths,
        rules: RedactRules,
        shared: Arc<super::Shared>,
        stop: Arc<AtomicBool>,
    ) -> std::io::Result<Self> {
        ipc::prepare_socket_path(&paths.composer_socket, |path| {
            format!("ke resident composer already running at {}", path.display())
        })?;
        let listener = ipc::bind_local_listener(&paths.composer_socket)?;
        listener.set_nonblocking(ListenerNonblockingMode::Accept)?;
        Ok(Self {
            listener,
            socket_path: paths.composer_socket.clone(),
            chat_log: paths.chat_log.clone(),
            rules,
            shared,
            stop,
        })
    }

    /// Accepts connections until `stop` is set or the socket file disappears.
    pub(crate) fn serve(self) {
        loop {
            if self.stop.load(Ordering::Relaxed) || !self.socket_path.exists() {
                return;
            }
            match self.listener.accept() {
                Ok(stream) => {
                    if let Err(err) = self.handle(stream) {
                        eprintln!("ke resident: composer request failed: {err}");
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(ACCEPT_POLL);
                }
                Err(err) => {
                    eprintln!("ke resident: accept failed: {err}");
                    std::thread::sleep(ACCEPT_POLL);
                }
            }
        }
    }

    fn handle(&self, mut stream: LocalStream) -> std::io::Result<()> {
        let _ = stream.set_nonblocking(false);
        let mut line = String::new();
        BufReader::new(&mut stream).read_line(&mut line)?;
        let decision = match serde_json::from_str::<ComposerRequest>(line.trim()) {
            Ok(request) => decide(&request, self.rules, &self.chat_log),
            Err(err) => Decision::Block {
                note: format!("管家收到了看不懂的请求（{err}），未发送"),
            },
        };
        let mut reply = decision.to_json();
        reply.push('\n');
        stream.write_all(reply.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::path::PathBuf;

    fn request(text: &str) -> ComposerRequest {
        ComposerRequest {
            pane_id: "w1:p1".into(),
            agent: Some("claude".into()),
            text: text.into(),
        }
    }

    fn temp_chat_log() -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "ke-processor-{}-{}",
                std::process::id(),
                chat_log::now_epoch().to_bits()
            ))
            .join("chat.jsonl")
    }

    #[test]
    fn plain_text_is_redacted_and_sent() {
        let log = temp_chat_log();
        let decision = decide(
            &request("use sk-live-abcdef123456 please"),
            RedactRules::default(),
            &log,
        );
        assert_eq!(
            decision,
            Decision::Send {
                text: "use [REDACTED] please".into(),
                note: Some("脱敏 1 处".into()),
            }
        );
        assert_eq!(
            decide(&request("ls -la"), RedactRules::default(), &log),
            Decision::Send {
                text: "ls -la".into(),
                note: None,
            }
        );
        assert!(
            chat_log::read_tail(&log, 5).unwrap().is_empty(),
            "plain text is not logged"
        );
    }

    #[test]
    fn ke_mention_is_logged_and_done() {
        let log = temp_chat_log();
        let decision = decide(
            &request("  @ke 哪个 agent 空着 "),
            RedactRules::default(),
            &log,
        );
        assert_eq!(
            decision,
            Decision::Done {
                note: Some("思考中…".into()),
            },
            "{decision:?}"
        );
        let entries = chat_log::read_tail(&log, 5).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, ChatRole::User);
        assert_eq!(entries[0].text, "哪个 agent 空着");
        assert_eq!(entries[0].pane_id.as_deref(), Some("w1:p1"));
        assert!(matches!(
            decide(&request("@ke"), RedactRules::default(), &log),
            Decision::Done { note: Some(_) }
        ));
        assert_eq!(
            chat_log::read_tail(&log, 5).unwrap().len(),
            1,
            "empty mention is not logged"
        );
        let _ = std::fs::remove_dir_all(log.parent().unwrap());
    }

    #[test]
    fn replies_serialize_to_the_composer_protocol() {
        let send: serde_json::Value = serde_json::from_str(
            &Decision::Send {
                text: "x".into(),
                note: None,
            }
            .to_json(),
        )
        .unwrap();
        assert_eq!(send["action"], "send");
        assert_eq!(send["text"], "x");
        assert!(send["note"].is_null());
        let block: serde_json::Value =
            serde_json::from_str(&Decision::Block { note: "no".into() }.to_json()).unwrap();
        assert_eq!(block["action"], "block");
        assert_eq!(block["note"], "no");
        let done: serde_json::Value =
            serde_json::from_str(&Decision::Done { note: None }.to_json()).unwrap();
        assert_eq!(done["action"], "done");
    }

    #[test]
    fn socket_round_trip_matches_the_client_protocol() {
        let dir = std::env::temp_dir().join(format!("ke-proc-sock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = ResidentPaths::in_dir(dir.clone());
        paths.ensure_dir().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let processor = Processor::bind(
            &paths,
            RedactRules::default(),
            Arc::new(super::super::Shared::default()),
            Arc::clone(&stop),
        )
        .unwrap();
        let socket = paths.composer_socket.clone();
        let server = std::thread::spawn(move || processor.serve());

        let mut stream = crate::ipc::connect_local_stream(&socket).unwrap();
        stream
            .write_all(b"{\"pane_id\":\"w1:p1\",\"agent\":null,\"text\":\"token ghp_abcdefghijklmnopqrstuvwxyz\"}\n")
            .unwrap();
        let mut reply = String::new();
        BufReader::new(&mut stream).read_line(&mut reply).unwrap();
        let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["action"], "send");
        assert_eq!(reply["text"], "token [REDACTED]");

        let mut stream = crate::ipc::connect_local_stream(&socket).unwrap();
        stream.write_all(b"this is not json\n").unwrap();
        let mut reply = String::new();
        BufReader::new(&mut stream).read_line(&mut reply).unwrap();
        let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["action"], "block", "malformed requests fail closed");

        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
