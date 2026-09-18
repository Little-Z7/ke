//! Added by ke: everything the shell adds on top of herdr that is not TUI chrome.
//!
//! - `paths`: where the resident's socket, panel, and chat log live for a session.
//! - `redact`: the rule-based redaction gate.
//! - `chat_log`: the append-only `@ke` conversation file shared by resident and client.
//! - `resident`: the `ke resident` process (composer processor + panel writer + model) and the
//!   supervisor the server uses to keep it running.

pub(crate) mod chat_log;
pub(crate) mod paths;
pub(crate) mod redact;
pub(crate) mod resident;
