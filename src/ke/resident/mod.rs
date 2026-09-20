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

/// Shared, read-only view the processor and answerer threads need from the poller.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    pub agents: std::sync::Mutex<Vec<SessionAgent>>,
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
