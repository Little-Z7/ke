//! Added by ke: keeps one `ke resident` child alive per server while `[ke.model].enabled` is set.
//!
//! The server calls `apply_config` at startup and after every config reload, and `shutdown` when
//! it exits. Between those calls a background thread restarts the child with exponential backoff
//! when it dies. The supervisor is a process-wide singleton because there is exactly one server
//! per process and the hooks in upstream code should stay one line each.

use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::config::{Config, KeModelConfig};

const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// A child that lived at least this long counts as healthy: the next backoff starts over.
const HEALTHY_RUN: Duration = Duration::from_secs(60);
const STOP_POLL: Duration = Duration::from_millis(100);
/// Longer than the resident's poll interval so a graceful exit usually wins over `kill`.
const STOP_GRACE: Duration = Duration::from_secs(4);

struct Running {
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    thread: Option<std::thread::JoinHandle<()>>,
    config: KeModelConfig,
}

fn slot() -> &'static Mutex<Option<Running>> {
    static SLOT: OnceLock<Mutex<Option<Running>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Starts, restarts, or stops the resident to match `config`. Cheap when nothing changed.
pub(crate) fn apply_config(config: &Config) {
    let wanted = &config.ke.model;
    let Ok(mut guard) = slot().lock() else {
        return;
    };
    let unchanged = guard
        .as_ref()
        .is_some_and(|running| &running.config == wanted);
    if unchanged {
        return;
    }
    if let Some(running) = guard.take() {
        stop_running(running);
    }
    if !wanted.enabled {
        return;
    }
    *guard = Some(start(wanted.clone()));
}

/// Stops the resident if it is running. Safe to call more than once.
pub(crate) fn shutdown() {
    let running = slot().lock().ok().and_then(|mut guard| guard.take());
    if let Some(running) = running {
        stop_running(running);
    }
}

/// Whether a resident child is currently alive.
#[allow(dead_code)] // reported by `/ke status` (next milestone)
pub(crate) fn is_running() -> bool {
    slot().lock().ok().is_some_and(|guard| {
        guard.as_ref().is_some_and(|running| {
            running.child.lock().ok().is_some_and(|mut child| {
                matches!(child.as_mut().map(Child::try_wait), Some(Ok(None)))
            })
        })
    })
}

fn start(config: KeModelConfig) -> Running {
    let stop = Arc::new(AtomicBool::new(false));
    let child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
    let thread = {
        let stop = Arc::clone(&stop);
        let child = Arc::clone(&child);
        std::thread::Builder::new()
            .name("ke-resident-supervisor".into())
            .spawn(move || supervise(stop, child))
            .ok()
    };
    Running {
        stop,
        child,
        thread,
        config,
    }
}

fn supervise(stop: Arc<AtomicBool>, slot: Arc<Mutex<Option<Child>>>) {
    let mut backoff = BACKOFF_START;
    while !stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        let spawned = spawn_child();
        let mut child = match spawned {
            Ok(child) => child,
            Err(err) => {
                tracing::warn!(%err, "failed to start ke resident");
                sleep_unless_stopped(&stop, backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX);
                continue;
            }
        };
        tracing::info!(pid = child.id(), "ke resident started");
        if let Ok(mut guard) = slot.lock() {
            *guard = Some(child);
        } else {
            let _ = child.kill();
            return;
        }
        // Wait for exit without holding the lock so `stop_running` can kill the child.
        let status = loop {
            if stop.load(Ordering::Relaxed) {
                break None;
            }
            let polled = slot
                .lock()
                .ok()
                .and_then(|mut guard| guard.as_mut().map(|child| child.try_wait()));
            match polled {
                Some(Ok(Some(status))) => break Some(status),
                Some(Ok(None)) => std::thread::sleep(STOP_POLL),
                Some(Err(err)) => {
                    tracing::warn!(%err, "ke resident wait failed");
                    break None;
                }
                None => break None,
            }
        };
        if let Ok(mut guard) = slot.lock() {
            guard.take();
        }
        if stop.load(Ordering::Relaxed) {
            return;
        }
        tracing::warn!(?status, "ke resident exited; restarting");
        if started.elapsed() >= HEALTHY_RUN {
            backoff = BACKOFF_START;
        }
        sleep_unless_stopped(&stop, backoff);
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

fn spawn_child() -> std::io::Result<Child> {
    let exe = crate::platform::launch_executable()?;
    let api_socket = crate::api::socket_path();
    let paths = crate::ke::paths::ResidentPaths::beside_api_socket(&api_socket);
    paths.ensure_dir()?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log)?;
    let mut command = crate::noninteractive_process::command(exe);
    command
        .arg("resident")
        .arg("--api-socket")
        .arg(&api_socket)
        .env(crate::api::SOCKET_PATH_ENV_VAR, &api_socket)
        .env(crate::HERDR_ENV_VAR, crate::HERDR_ENV_VALUE)
        // Our own child: keep the HERDR_* it inherits instead of dropping them (see ke_env).
        .env(crate::ke_env::KE_ENV_VAR, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    if let Some(name) = crate::session::active_name() {
        command.env(crate::session::SESSION_ENV_VAR, name);
    }
    command.spawn()
}

fn stop_running(mut running: Running) {
    running.stop.store(true, Ordering::Relaxed);
    let paths = crate::ke::paths::ResidentPaths::beside_api_socket(&crate::api::socket_path());
    if let Ok(mut guard) = running.child.lock() {
        if let Some(child) = guard.as_mut() {
            // Removing the socket file is the stop signal the resident watches for; it then exits
            // on its own and removes its panel. Kill only if it does not.
            let _ = std::fs::remove_file(&paths.composer_socket);
            let deadline = Instant::now() + STOP_GRACE;
            while Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(STOP_POLL);
            }
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        guard.take();
    }
    if let Some(thread) = running.thread.take() {
        let _ = thread.join();
    }
    // A killed child had no chance to clean up; the panel must not linger as "offline".
    let _ = std::fs::remove_file(&paths.composer_socket);
    let _ = std::fs::remove_file(&paths.panel_file);
}

fn sleep_unless_stopped(stop: &AtomicBool, duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(STOP_POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}
