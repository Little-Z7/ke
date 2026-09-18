//! Added by ke: file layout of the resident model for one session.
//!
//! Everything lives next to the session's `herdr.sock` so named sessions never share a resident:
//!
//! ```text
//! <session data dir>/resident/
//!   composer.sock   composer processor (Unix socket)
//!   panel.json      sidebar panel
//!   chat.jsonl      @ke conversation, append-only
//!   taskboard.json  per-session task board
//!   resident.log    stdout/stderr of the resident process
//! ```

use std::path::{Path, PathBuf};

pub(crate) const RESIDENT_DIR: &str = "resident";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResidentPaths {
    pub dir: PathBuf,
    pub composer_socket: PathBuf,
    pub panel_file: PathBuf,
    pub chat_log: PathBuf,
    pub taskboard: PathBuf,
    pub log: PathBuf,
}

impl ResidentPaths {
    pub(crate) fn in_dir(dir: PathBuf) -> Self {
        Self {
            composer_socket: dir.join("composer.sock"),
            panel_file: dir.join("panel.json"),
            chat_log: dir.join("chat.jsonl"),
            taskboard: dir.join("taskboard.json"),
            log: dir.join("resident.log"),
            dir,
        }
    }

    /// Paths for the session this process addresses (`HERDR_SESSION` / `--session`, else default).
    pub(crate) fn for_active_session() -> Self {
        Self::in_dir(crate::session::data_dir().join(RESIDENT_DIR))
    }

    /// Paths for the session that owns `api_socket` (the resident receives that path from the
    /// server and derives everything else from it).
    pub(crate) fn beside_api_socket(api_socket: &Path) -> Self {
        let dir = api_socket
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Self::in_dir(dir.join(RESIDENT_DIR))
    }

    pub(crate) fn ensure_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_sit_in_the_resident_dir_next_to_the_api_socket() {
        let paths = ResidentPaths::beside_api_socket(Path::new("/cfg/ke/sessions/x/herdr.sock"));
        assert_eq!(paths.dir, PathBuf::from("/cfg/ke/sessions/x/resident"));
        assert_eq!(
            paths.composer_socket,
            PathBuf::from("/cfg/ke/sessions/x/resident/composer.sock")
        );
        assert_eq!(paths.panel_file.file_name().unwrap(), "panel.json");
        assert_eq!(paths.chat_log.file_name().unwrap(), "chat.jsonl");
        assert_eq!(paths.taskboard.file_name().unwrap(), "taskboard.json");
        assert_eq!(paths.log.file_name().unwrap(), "resident.log");
    }
}
