// Modified by ke: sidebar panel fed by the ke coordinator.
//
// The coordinator (`workcat ke coord`) writes `panel.json` next to its state
// file. The client re-reads it from the main-loop timer only when the file's
// mtime changes, so rendering stays a pure function of in-memory state.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

use super::render::put_text;
use crate::app::state::Palette;

pub(super) const KE_PANEL_POLL: Duration = Duration::from_secs(2);
/// Older than this the coordinator is considered gone; the panel dims.
pub(super) const KE_PANEL_STALE_SECS: f64 = 30.0;
/// Older than this the panel is hidden entirely.
pub(super) const KE_PANEL_HIDE_SECS: f64 = 600.0;
pub(super) const KE_PANEL_MAX_ROWS: usize = 12;
const KE_PANEL_MAX_FILE_BYTES: u64 = 64 * 1024;
/// The agents section keeps at least this many rows.
const KE_PANEL_MIN_AGENT_ROWS: u16 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KePanelLevel {
    Ok,
    Warn,
    Err,
    Info,
    Dim,
}

impl KePanelLevel {
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("ok") => Self::Ok,
            Some("warn") => Self::Warn,
            Some("err") | Some("error") => Self::Err,
            Some("dim") => Self::Dim,
            _ => Self::Info,
        }
    }

    fn style(self, palette: &Palette) -> Style {
        match self {
            Self::Ok => Style::default().fg(palette.green),
            Self::Warn => Style::default().fg(palette.yellow),
            Self::Err => Style::default().fg(palette.red),
            Self::Info => Style::default().fg(palette.text),
            Self::Dim => Style::default().fg(palette.overlay0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct KePanelRow {
    pub(super) text: String,
    pub(super) level: KePanelLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct KePanel {
    pub(super) title: String,
    pub(super) rows: Vec<KePanelRow>,
    pub(super) stale: bool,
}

/// Parses `panel.json`; returns the panel and its write timestamp (unix secs).
///
/// ```json
/// {"v":1,"ts":1726200000.0,"title":"ke","rows":[{"text":"claude ok 5h 34%","level":"ok"}]}
/// ```
pub(super) fn parse_panel(text: &str) -> Option<(KePanel, f64)> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let ts = value.get("ts")?.as_f64()?;
    let title = value
        .get("title")
        .and_then(|title| title.as_str())
        .unwrap_or("ke")
        .chars()
        .filter(|ch| !ch.is_control())
        .take(40)
        .collect();
    let rows = value
        .get("rows")
        .and_then(|rows| rows.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let text: String = row
                        .get("text")?
                        .as_str()?
                        .chars()
                        .map(|ch| if ch.is_control() { ' ' } else { ch })
                        .take(120)
                        .collect();
                    Some(KePanelRow {
                        text,
                        level: KePanelLevel::parse(row.get("level").and_then(|l| l.as_str())),
                    })
                })
                .take(KE_PANEL_MAX_ROWS)
                .collect()
        })
        .unwrap_or_default();
    Some((
        KePanel {
            title,
            rows,
            stale: false,
        },
        ts,
    ))
}

/// Applies the age policy: hidden when too old, dimmed when stale.
pub(super) fn visible_panel(parsed: &KePanel, ts: f64, now_epoch: f64) -> Option<KePanel> {
    let age = now_epoch - ts;
    if age > KE_PANEL_HIDE_SECS {
        return None;
    }
    let mut panel = parsed.clone();
    panel.stale = age > KE_PANEL_STALE_SECS;
    Some(panel)
}

pub(super) fn ke_panel_path() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    if let Some(path) = std::env::var_os("KE_PANEL_FILE").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(path));
    }
    if let Some(home) = std::env::var_os("WORKCAT_KE_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(home).join("panel.json"));
    }
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".workcat")
            .join("ke")
            .join("panel.json"),
    )
}

#[derive(Debug, Clone, Default)]
pub(super) struct KePanelSource {
    path: Option<PathBuf>,
    next_check: Option<Instant>,
    mtime: Option<SystemTime>,
    parsed: Option<(KePanel, f64)>,
    panel: Option<KePanel>,
}

impl KePanelSource {
    pub(super) fn from_env() -> Self {
        Self {
            path: ke_panel_path(),
            ..Self::default()
        }
    }

    pub(super) fn panel(&self) -> Option<&KePanel> {
        self.panel.as_ref()
    }

    /// Returns true when the visible panel changed and the shell should repaint.
    pub(super) fn tick(&mut self, now: Instant) -> bool {
        let Some(path) = self.path.as_ref() else {
            return false;
        };
        if self.next_check.is_some_and(|deadline| now < deadline) {
            return false;
        }
        self.next_check = Some(now + KE_PANEL_POLL);
        match std::fs::metadata(path) {
            Ok(meta) if meta.len() <= KE_PANEL_MAX_FILE_BYTES => {
                let mtime = meta.modified().ok();
                if mtime.is_none() || mtime != self.mtime {
                    self.mtime = mtime;
                    self.parsed = std::fs::read_to_string(path)
                        .ok()
                        .and_then(|text| parse_panel(&text));
                }
            }
            _ => {
                self.mtime = None;
                self.parsed = None;
            }
        }
        let now_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |elapsed| elapsed.as_secs_f64());
        let next = self
            .parsed
            .as_ref()
            .and_then(|(panel, ts)| visible_panel(panel, *ts, now_epoch));
        let changed = next != self.panel;
        self.panel = next;
        changed
    }
}

/// Splits the agents section: the panel takes the bottom rows, leaving the
/// section's last row (sidebar toggle / menu line) untouched.
pub(super) fn split_detail(area: Rect, panel: Option<&KePanel>) -> (Rect, Rect) {
    let Some(panel) = panel else {
        return (area, Rect::default());
    };
    let want = 1 + panel.rows.len().min(KE_PANEL_MAX_ROWS) as u16;
    let room = area
        .height
        .saturating_sub(1)
        .saturating_sub(KE_PANEL_MIN_AGENT_ROWS);
    let height = want.min(room).min(area.height / 2);
    if height < 2 || area.width < 4 {
        return (area, Rect::default());
    }
    let panel_y = area.bottom().saturating_sub(1 + height);
    (
        Rect::new(area.x, area.y, area.width, panel_y - area.y),
        Rect::new(area.x, panel_y, area.width, height),
    )
}

pub(super) fn render_ke_panel(buffer: &mut Buffer, area: Rect, panel: &KePanel, palette: &Palette) {
    if area.is_empty() {
        return;
    }
    let header = if panel.stale {
        format!(" {} · offline", panel.title)
    } else {
        format!(" {}", panel.title)
    };
    put_text(
        buffer,
        area.x,
        area.y,
        area.width,
        &header,
        Style::default()
            .fg(palette.overlay0)
            .add_modifier(Modifier::BOLD),
    );
    for (index, row) in panel
        .rows
        .iter()
        .take(area.height.saturating_sub(1) as usize)
        .enumerate()
    {
        let style = if panel.stale {
            KePanelLevel::Dim.style(palette)
        } else {
            row.level.style(palette)
        };
        put_text(
            buffer,
            area.x.saturating_add(1),
            area.y + 1 + index as u16,
            area.width.saturating_sub(1),
            &row.text,
            style,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "ESC" is swapped for a JSON-escaped control character at test time.
    const SAMPLE: &str = r#"{"v":1,"ts":1000.0,"title":"ke","rows":[
        {"text":"claude ok 5h 34%","level":"ok"},
        {"text":"codex limited","level":"warn"},
        {"text":"badESC[31m","level":"nope"},
        {"level":"ok"}
    ]}"#;

    fn sample() -> String {
        SAMPLE.replace("ESC", "\\u001b")
    }

    #[test]
    fn parse_panel_reads_rows_and_sanitizes_control_chars() {
        let (panel, ts) = parse_panel(&sample()).expect("parses");
        assert_eq!(ts, 1000.0);
        assert_eq!(panel.title, "ke");
        assert_eq!(panel.rows.len(), 3, "row without text is dropped");
        assert_eq!(panel.rows[0].level, KePanelLevel::Ok);
        assert_eq!(panel.rows[1].level, KePanelLevel::Warn);
        assert_eq!(panel.rows[2].level, KePanelLevel::Info);
        assert!(!panel.rows[2].text.chars().any(char::is_control));
        assert!(parse_panel("{}").is_none(), "ts is required");
        assert!(parse_panel("not json").is_none());
    }

    #[test]
    fn visible_panel_dims_then_hides_by_age() {
        let (panel, ts) = parse_panel(&sample()).unwrap();
        assert!(!visible_panel(&panel, ts, ts + 5.0).unwrap().stale);
        assert!(
            visible_panel(&panel, ts, ts + KE_PANEL_STALE_SECS + 1.0)
                .unwrap()
                .stale
        );
        assert!(visible_panel(&panel, ts, ts + KE_PANEL_HIDE_SECS + 1.0).is_none());
    }

    #[test]
    fn split_detail_keeps_agent_rows_and_last_line() {
        let (panel, _) = parse_panel(&sample()).unwrap();
        let area = Rect::new(0, 10, 30, 20);
        let (agents, ke) = split_detail(area, Some(&panel));
        assert_eq!(ke.height, 4, "header + 3 rows");
        assert_eq!(
            ke.bottom(),
            area.bottom() - 1,
            "section's last row stays free"
        );
        assert_eq!(agents.y, area.y);
        assert_eq!(agents.bottom(), ke.y);
        assert_eq!(split_detail(area, None), (area, Rect::default()));
        let tiny = Rect::new(0, 0, 30, 6);
        assert_eq!(
            split_detail(tiny, Some(&panel)).1,
            Rect::default(),
            "too small: no panel"
        );
    }

    #[test]
    fn render_ke_panel_draws_header_and_rows() {
        let (mut panel, _) = parse_panel(&sample()).unwrap();
        let palette = Palette::catppuccin();
        let area = Rect::new(0, 0, 24, 4);
        let line = |buffer: &Buffer, y: u16| -> String {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        };
        let mut buffer = Buffer::empty(area);
        render_ke_panel(&mut buffer, area, &panel, &palette);
        assert!(line(&buffer, 0).starts_with(" ke"));
        assert!(line(&buffer, 1).starts_with(" claude ok 5h 34%"));
        assert_eq!(buffer[(1, 1)].fg, palette.green);
        assert_eq!(buffer[(1, 2)].fg, palette.yellow);

        panel.stale = true;
        let mut buffer = Buffer::empty(area);
        render_ke_panel(&mut buffer, area, &panel, &palette);
        assert!(line(&buffer, 0).starts_with(" ke · offline"));
        assert_eq!(buffer[(1, 1)].fg, palette.overlay0);
    }

    #[test]
    fn source_without_path_never_repaints() {
        let mut source = KePanelSource::default();
        assert!(!source.tick(Instant::now()));
        assert!(source.panel().is_none());
    }

    #[test]
    fn source_reads_file_once_per_mtime() {
        let dir = std::env::temp_dir().join(format!("ke-panel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("panel.json");
        let now_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        std::fs::write(
            &path,
            format!(r#"{{"ts":{now_epoch},"rows":[{{"text":"a"}}]}}"#),
        )
        .unwrap();
        let mut source = KePanelSource {
            path: Some(path.clone()),
            ..KePanelSource::default()
        };
        let start = Instant::now();
        assert!(source.tick(start), "first read shows the panel");
        assert_eq!(source.panel().unwrap().rows[0].text, "a");
        assert!(!source.tick(start), "throttled until the next poll");
        assert!(
            !source.tick(start + KE_PANEL_POLL),
            "unchanged file: no repaint"
        );
        std::fs::remove_file(&path).unwrap();
        assert!(
            source.tick(start + KE_PANEL_POLL * 2),
            "file gone: panel hides"
        );
        assert!(source.panel().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
