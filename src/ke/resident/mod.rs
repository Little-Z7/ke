//! Added by ke: the resident model process ("管家"), started as `ke resident` by the server.
//!
//! One resident per session. It owns three files in the session's `resident/` directory (see
//! `ke::paths`): the composer socket it serves, the panel it writes, and the chat log it appends
//! to. It reads session state through the normal JSON API over the socket the server hands it in
//! `HERDR_SOCKET_PATH`, so it never touches server internals.
//!
//! Its roster spans every running session, not just the one it serves (see `sessions`): a manager
//! that cannot see the pane waiting for you in another session is not much of a manager. Only the
//! local session's failures end the process.
//!
//! The process is deliberately dumb about lifecycle: when the API stops answering it exits, and
//! the supervisor in the server decides whether to start it again.

pub(crate) mod answer;
pub(crate) mod model;
pub(crate) mod panel;
pub(crate) mod processor;
pub(crate) mod prompt;
pub(crate) mod sessions;
pub(crate) mod snapshot;
pub(crate) mod supervisor;

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::api::client::{ApiClient, ConnectionTarget};

use super::paths::ResidentPaths;
use super::redact;
use sessions::SessionAgent;

/// How often the panel is rebuilt from `agent.list`.
const POLL: Duration = Duration::from_secs(3);
/// Consecutive failed API calls before the resident assumes the server is gone. Only the local
/// session counts: another session going away is normal and must not take this resident with it.
const MAX_API_FAILURES: u32 = 5;
const API_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn command_spec() -> clap::Command {
    clap::Command::new("resident")
        .about("Run the ke resident model process (started by the server; rarely run by hand)")
        .arg(
            clap::Arg::new("api-socket")
                .long("api-socket")
                .value_name("PATH")
                .help("Socket of the server this resident serves (default: HERDR_SOCKET_PATH)"),
        )
}

/// Entry point for `ke resident [--api-socket PATH]`.
pub(crate) fn run_resident_command(args: &[String]) -> io::Result<i32> {
    let mut api_socket: Option<PathBuf> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--api-socket" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("usage: herdr resident [--api-socket PATH]");
                    return Ok(2);
                };
                api_socket = Some(PathBuf::from(value));
                index += 2;
            }
            "help" | "--help" | "-h" => {
                println!("usage: herdr resident [--api-socket PATH]");
                return Ok(0);
            }
            other => {
                eprintln!("unknown argument: {other}");
                return Ok(2);
            }
        }
    }
    let api_socket = api_socket.unwrap_or_else(crate::api::socket_path);
    let paths = ResidentPaths::beside_api_socket(&api_socket);
    let config = crate::config::Config::load().config;
    let outcome = run(api_socket, paths, &config);
    match outcome {
        Ok(()) => Ok(0),
        Err(err) => {
            eprintln!("resident: {err}");
            Ok(1)
        }
    }
}

/// State shared between the processor and answerer threads.
///
/// `agents` is the read-only roster snapshot the poller refreshes each round. `redaction` is the
/// one [`redact::Mapping`] for the whole session: both threads mask through it, so a placeholder
/// a pane shows the user (via the composer processor) and the same placeholder in the model's
/// context (via the answerer) always resolve to the same real value. Hold its lock only for the
/// duration of a single `redact::mask` call -- never across a network request, file IO, or a
/// model call.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    pub agents: std::sync::Mutex<Vec<SessionAgent>>,
    pub redaction: std::sync::Mutex<redact::Mapping>,
}

fn run(
    api_socket: PathBuf,
    paths: ResidentPaths,
    config: &crate::config::Config,
) -> io::Result<()> {
    paths.ensure_dir()?;
    let client = ApiClient::for_target(ConnectionTarget::SocketPath(api_socket.clone()));
    let stop = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Shared::default());
    // Before either worker thread can mint or send a placeholder, make sure this fresh
    // `Mapping` cannot reissue a number an earlier process cycle already used (see
    // `seed_redaction_watermarks`).
    seed_redaction_watermarks(&shared, &paths.chat_log);

    let processor = processor::Processor::bind(
        &paths,
        config.ke.redact_rules(),
        Arc::clone(&shared),
        Arc::clone(&stop),
    )?;
    let processor_thread = std::thread::Builder::new()
        .name("ke-resident-processor".into())
        .spawn(move || processor.serve())?;
    let answerer_thread = std::thread::Builder::new()
        .name("ke-resident-answerer".into())
        .spawn({
            let client = client.clone();
            let paths = paths.clone();
            let ke = config.ke.clone();
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            move || answer::run(client, paths, ke, shared, stop)
        })?;
    {
        // Ctrl-C / SIGTERM when run by hand; the supervisor stops us by removing the socket file.
        let stop = Arc::clone(&stop);
        let _ = ctrlc::set_handler(move || stop.store(true, Ordering::Relaxed));
    }

    eprintln!(
        "ke resident: serving {} for {}",
        paths.composer_socket.display(),
        api_socket.display()
    );

    let mut tracker = panel::StatusTracker::default();
    let mut failures = 0u32;
    while !stop.load(Ordering::Relaxed) {
        if !paths.composer_socket.exists() {
            eprintln!("ke resident: socket removed; exiting");
            break;
        }
        let started = Instant::now();
        // Rediscovered every round so sessions started after this resident show up, and stopped
        // ones drop out. `ApiClient` only holds a path, so this costs a directory read.
        let discovered = sessions::discover(&api_socket);
        let roster = sessions::fetch_all(&discovered, API_TIMEOUT);
        if roster.local_ok {
            failures = 0;
        } else {
            failures += 1;
            eprintln!("ke resident: local agent.list failed ({failures}/{MAX_API_FAILURES})");
            if failures >= MAX_API_FAILURES {
                break;
            }
        }
        if !roster.unreachable.is_empty() {
            eprintln!(
                "ke resident: sessions unreachable this round: {}",
                roster.unreachable.join(", ")
            );
        }
        // A degraded roster still refreshes the panel: the local rows are what the user watches,
        // and an unreachable session gets its own row rather than vanishing.
        tracker.observe(&roster.agents, Instant::now());
        if let Ok(mut slot) = shared.agents.lock() {
            *slot = roster.agents.clone();
        }
        let panel = panel::build(
            &roster.agents,
            &roster.unreachable,
            &tracker,
            Instant::now(),
        );
        // Always rewrite so `ts` moves and the panel never dims while we are alive.
        if let Err(err) = write_atomic(&paths.panel_file, &panel.to_json()) {
            eprintln!("ke resident: panel write failed: {err}");
        }
        let elapsed = started.elapsed();
        if elapsed < POLL {
            std::thread::sleep(POLL - elapsed);
        }
    }
    stop.store(true, Ordering::Relaxed);
    let _ = std::fs::remove_file(&paths.composer_socket);
    let _ = std::fs::remove_file(&paths.panel_file);
    // The processor thread wakes from `accept` when the socket file is gone or on its own timeout.
    let _ = processor_thread.join();
    let _ = answerer_thread.join();
    Ok(())
}

/// Reads the session's `chat.jsonl` in full and raises `shared`'s [`redact::Mapping`] counters so
/// it will never reissue a placeholder number a previous process cycle's `Mapping` already used
/// for a different real value (see `redact::Mapping::reserve_from_text`). Called once at startup,
/// before either worker thread that could mint or send a placeholder starts.
///
/// The whole file is scanned, not just a recent tail: a number used early in a long session's log
/// is just as reusable -- and just as dangerous to reuse -- as one used near the end. A missing
/// file, a read failure, or a log with no placeholders at all all mean "nothing to reserve"; none
/// of them may stop the resident from starting.
fn seed_redaction_watermarks(shared: &Shared, chat_log_path: &std::path::Path) {
    // Streamed line by line rather than through `chat_log::read_tail`: only the highest number per
    // family is wanted, so holding the whole log in memory to read a few integers would make
    // startup cost grow with a session's entire history. Placeholders are plain ASCII and JSON
    // never escapes them, so the raw line is as good as the decoded entry here.
    let file = match std::fs::File::open(chat_log_path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return,
        Err(err) => {
            eprintln!("ke resident: chat log scan for redaction watermarks failed: {err}");
            return;
        }
    };
    let Ok(mut map) = shared.redaction.lock() else {
        return;
    };
    // Nothing else holds this lock yet: the processor and answerer threads start after seeding.
    for line in std::io::BufRead::lines(std::io::BufReader::new(file)) {
        match line {
            Ok(line) => map.reserve_from_text(&line),
            Err(err) => {
                eprintln!("ke resident: chat log scan stopped early: {err}");
                break;
            }
        }
    }
}

/// Writes via a sibling temp file and rename so readers never see a partial panel.
pub(crate) fn write_atomic(path: &std::path::Path, text: &str) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        #[cfg(windows)]
        Err(_) if path.exists() => {
            std::fs::remove_file(path)?;
            std::fs::rename(&tmp, path)
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ke::chat_log::{self, ChatEntry, ChatRole};

    fn temp_chat_log(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!(
                "ke-resident-mod-{}-{}-{}",
                std::process::id(),
                tag,
                chat_log::now_epoch().to_bits()
            ))
            .join("chat.jsonl")
    }

    /// Masks a throwaway secret through `shared`'s mapping and returns the result, so a test can
    /// read back which placeholder number the seeded `Mapping` hands out next.
    fn next_secret_placeholder(shared: &Shared) -> String {
        let mut map = shared.redaction.lock().expect("mapping lock");
        redact::mask(
            "token sk-live-abcdefgh12345678",
            redact::RedactRules::default(),
            &mut map,
        )
        .text
    }

    #[test]
    fn missing_chat_log_leaves_numbering_at_the_start() {
        let path = temp_chat_log("missing");
        let shared = Shared::default();
        seed_redaction_watermarks(&shared, &path);
        assert_eq!(next_secret_placeholder(&shared), "token KE_SECRET_1");
    }

    #[test]
    fn chat_log_with_no_placeholders_leaves_numbering_at_the_start() {
        let path = temp_chat_log("no-placeholders");
        chat_log::append(
            &path,
            &ChatEntry::now(
                ChatRole::User,
                "just an ordinary question, no secrets in sight",
            ),
        )
        .unwrap();
        let shared = Shared::default();
        seed_redaction_watermarks(&shared, &path);
        assert_eq!(next_secret_placeholder(&shared), "token KE_SECRET_1");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn seeding_scans_the_whole_file_not_only_a_recent_tail() {
        let path = temp_chat_log("whole-file");
        // The reused number sits in the very first entry; every entry after it is filler with no
        // placeholder at all. A scan bounded to a small recent tail (like `answer::TAIL_LIMIT`
        // elsewhere in the resident) would miss it and leave the watermark at zero -- exactly the
        // bug this seeding exists to close.
        chat_log::append(
            &path,
            &ChatEntry::now(ChatRole::Assistant, "previously used KE_SECRET_7 here"),
        )
        .unwrap();
        for i in 0..250 {
            chat_log::append(
                &path,
                &ChatEntry::now(ChatRole::User, format!("filler {i}")),
            )
            .unwrap();
        }
        let shared = Shared::default();
        seed_redaction_watermarks(&shared, &path);
        assert_eq!(
            next_secret_placeholder(&shared),
            "token KE_SECRET_8",
            "must resume numbering after the highest number found anywhere in the file"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
