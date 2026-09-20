//! Added by ke: TUI form for `[ke.model]` (endpoint, model id, thinking, context).
//! Modified by ke: the form now also covers `provider = "cli"` (a headless agent CLI as the
//! resident's brain) and `[ke.model].panel_model`, so both can be set without hand-editing
//! config.toml. Which rows are shown depends on the selected provider; `visible_fields()` is the
//! single source of truth both the field navigation and the renderer use, so the two never
//! disagree about how many rows exist.

use super::*;
use crate::config::KeModelProfile;
use crate::ke::slash::{self, PLAN_TEMPLATES, PlanTemplate};

pub(super) const THINK_MODES: [&str; 3] = ["", "off", "on"];
pub(super) const THINK_LABELS: [&str; 3] = ["自动", "关", "开"];
/// `KeModelProfile.provider` values the form can pick between.
pub(super) const PROVIDERS: [&str; 2] = ["openai", "cli"];
const CUSTOM_PLAN: usize = 0;

/// Stable row identities. `field` on the overlay is a *position* inside the current
/// [`ClientKeModelOverlay::visible_fields`] list (so it stays a plain, contiguous index the rest
/// of the Settings machinery can treat like any other choice list); these constants are the
/// logical row a position maps to, used to decide what a row edits/shows/toggles.
const FIELD_ENABLED: usize = 0;
const FIELD_PROVIDER: usize = 1;
const FIELD_PLAN: usize = 2;
const FIELD_NAME: usize = 3;
const FIELD_BASE_URL: usize = 4;
const FIELD_MODEL: usize = 5;
const FIELD_API_KEY_ENV: usize = 6;
const FIELD_THINK: usize = 7;
const FIELD_MAX_TOKENS: usize = 8;
const FIELD_COMMAND: usize = 9;
const FIELD_PANEL_MODEL: usize = 10;

#[derive(Debug)]
pub(super) struct ClientKeModelOverlay {
    pub(super) field: usize,
    pub(super) enabled: bool,
    /// Index into [`PROVIDERS`].
    pub(super) provider: usize,
    pub(super) plan: usize,
    pub(super) name: TextEditor,
    pub(super) base_url: TextEditor,
    pub(super) model: TextEditor,
    pub(super) api_key_env: TextEditor,
    pub(super) think: usize,
    pub(super) max_tokens: TextEditor,
    /// `provider = "cli"` argv, edited as one shell-quoted line. See [`parse_command_line`] /
    /// [`render_command_line`].
    pub(super) command: TextEditor,
    /// `[ke.model].panel_model`; empty means "same as the active profile".
    pub(super) panel_model: TextEditor,
}

impl Default for ClientKeModelOverlay {
    fn default() -> Self {
        Self::from_config(&crate::config::KeConfig::default())
    }
}

impl ClientKeModelOverlay {
    pub(super) fn from_config(ke: &crate::config::KeConfig) -> Self {
        let (name, profile) = ke
            .resolved_profile()
            .unwrap_or_else(|| ("local".into(), PLAN_TEMPLATES[0].profile()));
        let provider = usize::from(profile.provider == "cli");
        let think = THINK_MODES
            .iter()
            .position(|mode| *mode == profile.think.trim())
            .unwrap_or(0);
        let max_tokens = if profile.max_tokens == 0 {
            String::new()
        } else {
            profile.max_tokens.to_string()
        };
        let mut form = Self {
            field: 0,
            enabled: ke.model.enabled,
            provider,
            plan: match_plan(&name, &profile),
            name: TextEditor::from(name.as_str()),
            base_url: TextEditor::from(profile.base_url.as_str()),
            model: TextEditor::from(profile.model.as_str()),
            api_key_env: TextEditor::from(profile.api_key_env.as_str()),
            think,
            max_tokens: TextEditor::from(max_tokens.as_str()),
            command: TextEditor::from(render_command_line(&profile.command).as_str()),
            panel_model: TextEditor::from(ke.model.panel_model.as_str()),
        };
        form.field = form.default_field_position();
        form
    }

    pub(super) fn is_openai(&self) -> bool {
        self.provider == 0
    }

    /// Rows shown for the current provider, in display order. A position in `field` indexes into
    /// this list; the value at that position is the logical row id.
    pub(super) fn visible_fields(&self) -> Vec<usize> {
        let mut fields = vec![FIELD_ENABLED, FIELD_PROVIDER];
        if self.is_openai() {
            fields.extend_from_slice(&[
                FIELD_PLAN,
                FIELD_NAME,
                FIELD_BASE_URL,
                FIELD_MODEL,
                FIELD_API_KEY_ENV,
                FIELD_THINK,
                FIELD_MAX_TOKENS,
            ]);
        } else {
            fields.extend_from_slice(&[FIELD_NAME, FIELD_COMMAND]);
        }
        fields.push(FIELD_PANEL_MODEL);
        fields
    }

    /// Logical row id the current `field` position points at.
    pub(super) fn current_field_id(&self) -> usize {
        let fields = self.visible_fields();
        fields
            .get(self.field)
            .copied()
            .unwrap_or_else(|| fields.last().copied().unwrap_or(FIELD_ENABLED))
    }

    /// Where the cursor should land right after opening the form: the access template row for
    /// openai (a quick way to fill a known preset), the profile name for cli (no templates there).
    fn default_field_position(&self) -> usize {
        let target = if self.is_openai() {
            FIELD_PLAN
        } else {
            FIELD_NAME
        };
        self.visible_fields()
            .iter()
            .position(|&id| id == target)
            .unwrap_or(0)
    }

    pub(super) fn plan_label(&self) -> &'static str {
        if self.plan == CUSTOM_PLAN {
            return "自定义";
        }
        PLAN_TEMPLATES
            .get(self.plan.saturating_sub(1))
            .map(|plan| plan.label)
            .unwrap_or("自定义")
    }

    pub(super) fn active_editor(&mut self) -> Option<&mut TextEditor> {
        match self.current_field_id() {
            FIELD_NAME => Some(&mut self.name),
            FIELD_BASE_URL => Some(&mut self.base_url),
            FIELD_MODEL => Some(&mut self.model),
            FIELD_API_KEY_ENV => Some(&mut self.api_key_env),
            FIELD_MAX_TOKENS => Some(&mut self.max_tokens),
            FIELD_COMMAND => Some(&mut self.command),
            FIELD_PANEL_MODEL => Some(&mut self.panel_model),
            _ => None,
        }
    }

    /// Read-only counterpart of [`Self::active_editor`], used by the renderer to draw any text
    /// row (not only the selected one).
    pub(super) fn editor_for(&self, field_id: usize) -> Option<&TextEditor> {
        match field_id {
            FIELD_NAME => Some(&self.name),
            FIELD_BASE_URL => Some(&self.base_url),
            FIELD_MODEL => Some(&self.model),
            FIELD_API_KEY_ENV => Some(&self.api_key_env),
            FIELD_MAX_TOKENS => Some(&self.max_tokens),
            FIELD_COMMAND => Some(&self.command),
            FIELD_PANEL_MODEL => Some(&self.panel_model),
            _ => None,
        }
    }

    /// Display text for a row, used for both the toggle rows and the non-focused rendering of
    /// text rows (the focused text row is drawn by the live editor instead so it can show a
    /// cursor).
    pub(super) fn field_value(&self, field_id: usize) -> String {
        match field_id {
            FIELD_ENABLED => if self.enabled { "on" } else { "off" }.to_string(),
            FIELD_PROVIDER => PROVIDERS[self.provider].to_string(),
            FIELD_PLAN => self.plan_label().to_string(),
            FIELD_THINK => THINK_LABELS
                .get(self.think)
                .copied()
                .unwrap_or("自动")
                .to_string(),
            other => self
                .editor_for(other)
                .map(|editor| editor.as_str().to_string())
                .unwrap_or_default(),
        }
    }

    fn cycle_think(&mut self, delta: isize) {
        let len = THINK_MODES.len() as isize;
        self.think = (self.think as isize + delta).rem_euclid(len) as usize;
    }

    fn cycle_provider(&mut self, delta: isize) {
        let len = PROVIDERS.len() as isize;
        self.provider = (self.provider as isize + delta).rem_euclid(len) as usize;
        // The visible row list changes size with the provider; keep the selection on a row that
        // still exists instead of pointing past the end of the new list.
        self.field = self.field.min(self.visible_fields().len().saturating_sub(1));
    }

    fn cycle_plan(&mut self, delta: isize) {
        let count = (PLAN_TEMPLATES.len() + 1) as isize;
        self.plan = (self.plan as isize + delta).rem_euclid(count) as usize;
        if let Some(plan) = PLAN_TEMPLATES.get(self.plan.saturating_sub(1)) {
            self.apply_plan(*plan);
        }
    }

    fn apply_plan(&mut self, plan: PlanTemplate) {
        // Access templates are openai-compatible HTTP presets; picking one only makes sense with
        // provider = "openai" (the row is hidden for "cli" anyway, but stay consistent).
        self.provider = 0;
        self.name = TextEditor::from(plan.name);
        self.base_url = TextEditor::from(plan.base_url);
        self.model = TextEditor::from(plan.model);
        self.api_key_env = TextEditor::from(plan.api_key_env);
        self.think = 0;
        self.max_tokens = TextEditor::from("");
    }

    fn profile(&self) -> Result<(String, KeModelProfile), String> {
        let name = self.name.as_str().trim();
        if !slash::profile_name_ok(name) {
            return Err("预设名只能用字母、数字、短横和下划线".into());
        }
        let provider = PROVIDERS[self.provider];
        if provider == "cli" {
            let command = parse_command_line(self.command.as_str())
                .map_err(|err| format!("command 解析失败：{err}"))?;
            if command.is_empty() {
                return Err("command 不能为空".into());
            }
            return Ok((
                name.to_string(),
                KeModelProfile {
                    provider: provider.to_string(),
                    command,
                    ..KeModelProfile::default()
                },
            ));
        }
        let max_tokens = self.max_tokens.as_str().trim();
        let max_tokens = if max_tokens.is_empty() {
            0
        } else {
            max_tokens
                .parse::<u32>()
                .map_err(|_| "上下文 max_tokens 必须是数字".to_string())?
        };
        Ok((
            name.to_string(),
            KeModelProfile {
                provider: provider.to_string(),
                base_url: self.base_url.as_str().trim().to_string(),
                api_key_env: self.api_key_env.as_str().trim().to_string(),
                model: self.model.as_str().trim().to_string(),
                think: THINK_MODES[self.think].to_string(),
                max_tokens,
                ..KeModelProfile::default()
            },
        ))
    }
}

fn match_plan(name: &str, profile: &KeModelProfile) -> usize {
    PLAN_TEMPLATES
        .iter()
        .position(|plan| {
            plan.name == name
                || (plan.base_url == profile.base_url.trim() && plan.model == profile.model.trim())
        })
        .map(|index| index + 1)
        .unwrap_or(CUSTOM_PLAN)
}

/// Splits a one-line command string into argv, following simplified POSIX shell quoting: bare
/// whitespace separates tokens, `'...'` takes everything literally (including backslashes),
/// `"..."` allows `\` to escape `"`, `\`, `$` and `` ` `` (any other backslash stays literal), and
/// outside quotes `\` escapes the very next character (including a space). This is enough to type
/// `opencode run` or `sh -c "kimi -p \"$(cat)\""` directly into a single text box.
///
/// Pure and total: never panics, and either returns the full argv or an error naming the exact
/// problem (never a partially-parsed result).
pub(super) fn parse_command_line(input: &str) -> Result<Vec<String>, String> {
    #[derive(PartialEq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut has_current = false;
    let mut quote = Quote::None;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        match quote {
            Quote::None => match ch {
                ' ' | '\t' => {
                    if has_current {
                        tokens.push(std::mem::take(&mut current));
                        has_current = false;
                    }
                }
                '\'' => {
                    quote = Quote::Single;
                    has_current = true;
                }
                '"' => {
                    quote = Quote::Double;
                    has_current = true;
                }
                '\\' => match chars.next() {
                    Some(next) => {
                        current.push(next);
                        has_current = true;
                    }
                    None => return Err("反斜杠后面缺字符".into()),
                },
                other => {
                    current.push(other);
                    has_current = true;
                }
            },
            Quote::Single => match ch {
                '\'' => quote = Quote::None,
                other => current.push(other),
            },
            Quote::Double => match ch {
                '"' => quote = Quote::None,
                '\\' => match chars.peek() {
                    Some('"') | Some('\\') | Some('$') | Some('`') => {
                        if let Some(next) = chars.next() {
                            current.push(next);
                        }
                    }
                    _ => current.push('\\'),
                },
                other => current.push(other),
            },
        }
    }
    match quote {
        Quote::Single => return Err("单引号没有闭合".into()),
        Quote::Double => return Err("双引号没有闭合".into()),
        Quote::None => {}
    }
    if has_current {
        tokens.push(current);
    }
    Ok(tokens)
}

/// Inverse of [`parse_command_line`]: renders argv back into one line, quoting whichever elements
/// need it so re-parsing reproduces the same vector (`parse_command_line(render_command_line(v))
/// == Ok(v)` for any `v`, covered in the tests below).
pub(super) fn render_command_line(command: &[String]) -> String {
    command
        .iter()
        .map(|part| render_command_token(part))
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_command_token(token: &str) -> String {
    // A bare `'` outside quotes starts single-quote mode in `parse_command_line`, so it needs
    // quoting just like whitespace/`"`/`\` do, or a token like `and'quote` would come back
    // unterminated.
    let needs_quoting = token.is_empty()
        || token
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '"' | '\'' | '\\'));
    if !needs_quoting {
        return token.to_string();
    }
    let mut out = String::from("\"");
    for ch in token.chars() {
        if matches!(ch, '"' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

impl ClientShellState {
    pub(super) fn open_ke_model_form(&mut self, outcome: &mut ClientShellInput) {
        self.open_settings_overlay();
        self.select_settings_section(ClientSettingsSection::KeModel, outcome);
        outcome.repaint = true;
    }

    pub(super) fn ke_model_form(&self) -> Option<&ClientKeModelOverlay> {
        match self.overlay.as_ref() {
            Some(ClientShellOverlay::KeModel(form)) => Some(form),
            Some(ClientShellOverlay::Settings(settings))
                if settings.section == ClientSettingsSection::KeModel =>
            {
                Some(&settings.ke_model)
            }
            _ => None,
        }
    }

    pub(super) fn ke_model_form_mut(&mut self) -> Option<&mut ClientKeModelOverlay> {
        match self.overlay.as_mut() {
            Some(ClientShellOverlay::KeModel(form)) => Some(form),
            Some(ClientShellOverlay::Settings(settings))
                if settings.section == ClientSettingsSection::KeModel =>
            {
                Some(&mut settings.ke_model)
            }
            _ => None,
        }
    }

    pub(super) fn handle_ke_model_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if self.ke_model_form().is_none() {
            return false;
        }
        if key.kind == crossterm::event::KeyEventKind::Release {
            return true;
        }
        let plain = key
            .modifiers
            .difference(crossterm::event::KeyModifiers::SHIFT)
            .is_empty();
        let shift = key
            .modifiers
            .contains(crossterm::event::KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc => {
                self.overlay = None;
                outcome.repaint = true;
                return true;
            }
            KeyCode::Tab if !shift => {
                self.ke_model_nudge_field(1);
                outcome.repaint = true;
                return true;
            }
            KeyCode::BackTab => {
                self.ke_model_nudge_field(-1);
                outcome.repaint = true;
                return true;
            }
            KeyCode::Up => {
                self.ke_model_nudge_field(-1);
                outcome.repaint = true;
                return true;
            }
            KeyCode::Down => {
                self.ke_model_nudge_field(1);
                outcome.repaint = true;
                return true;
            }
            KeyCode::Enter if plain => {
                // The standalone overlay's own save/cancel row lives one position past the last
                // visible field, so it moves automatically as the field list changes size.
                let on_save_row = self
                    .ke_model_form()
                    .map(|form| form.field == form.visible_fields().len())
                    .unwrap_or(false);
                if on_save_row {
                    self.save_ke_model_form(outcome);
                } else {
                    self.ke_model_nudge_field(1);
                    outcome.repaint = true;
                }
                return true;
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if self.ke_model_is_toggle() => {
                let delta = if matches!(key.code, KeyCode::Left) {
                    -1
                } else {
                    1
                };
                self.ke_model_toggle(delta);
                outcome.repaint = true;
                return true;
            }
            _ => {}
        }
        if let Some(form) = self.ke_model_form_mut() {
            if let Some(editor) = form.active_editor() {
                if editor.handle_key(key).is_some() {
                    outcome.repaint = true;
                }
            }
        }
        true
    }

    pub(super) fn ke_model_is_toggle(&self) -> bool {
        self.ke_model_form()
            .map(|form| {
                matches!(
                    form.current_field_id(),
                    FIELD_ENABLED | FIELD_PROVIDER | FIELD_PLAN | FIELD_THINK
                )
            })
            .unwrap_or(false)
    }

    pub(super) fn ke_model_nudge_field(&mut self, delta: isize) {
        // The standalone overlay has one extra row past the fields (its own save button); the
        // Settings tab applies via the shared "↵ apply" button instead, so it does not.
        let standalone = matches!(self.overlay, Some(ClientShellOverlay::KeModel(_)));
        if let Some(form) = self.ke_model_form_mut() {
            let count = form.visible_fields().len() + usize::from(standalone);
            if count > 0 {
                form.field = (form.field as isize + delta).rem_euclid(count as isize) as usize;
            }
        }
        if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
            settings.selected = settings.ke_model.field;
        }
    }

    pub(super) fn ke_model_toggle(&mut self, delta: isize) {
        if let Some(form) = self.ke_model_form_mut() {
            match form.current_field_id() {
                FIELD_ENABLED => form.enabled = !form.enabled,
                FIELD_PROVIDER => form.cycle_provider(delta),
                FIELD_PLAN => form.cycle_plan(delta),
                FIELD_THINK => form.cycle_think(delta),
                _ => {}
            }
        }
    }

    pub(super) fn save_ke_model_form(&mut self, outcome: &mut ClientShellInput) {
        let Some(form) = self.ke_model_form() else {
            return;
        };
        let enabled = form.enabled;
        let panel_model = form.panel_model.as_str().trim().to_string();
        let (name, profile) = match form.profile() {
            Ok(pair) => pair,
            Err(err) => {
                if let Some(composer) = self.composer.as_mut() {
                    composer.notice = Some(err);
                    composer.focused = true;
                }
                outcome.repaint = true;
                return;
            }
        };
        let cloud_confirmed = !profile.is_local();
        let wrote =
            crate::config::update_file_at(&crate::config::config_path(), "ke model", |content| {
                let next =
                    slash::apply_model_form(content, enabled, &name, &profile, cloud_confirmed);
                crate::config::upsert_section_value(
                    &next,
                    "ke.model",
                    "panel_model",
                    &slash::toml_quote(&panel_model),
                )
            });
        match wrote {
            Ok(()) => {
                self.overlay = None;
                self.request_config_reload(outcome);
                if let Some(composer) = self.composer.as_mut() {
                    composer.notice = Some(format!("已保存预设 {name}"));
                    composer.focused = true;
                }
            }
            Err(err) => {
                if let Some(composer) = self.composer.as_mut() {
                    composer.notice = Some(err);
                    composer.focused = true;
                }
            }
        }
        outcome.repaint = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_round_trips_the_readme_examples() {
        for command in [
            vec!["opencode".to_string(), "run".to_string()],
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "kimi -p \"$(cat)\"".to_string(),
            ],
        ] {
            let rendered = render_command_line(&command);
            assert_eq!(
                parse_command_line(&rendered),
                Ok(command.clone()),
                "round trip of {command:?} via {rendered:?}"
            );
        }
        assert_eq!(
            parse_command_line("opencode run"),
            Ok(vec!["opencode".into(), "run".into()])
        );
        assert_eq!(
            parse_command_line(r#"sh -c "kimi -p \"$(cat)\"""#),
            Ok(vec![
                "sh".into(),
                "-c".into(),
                "kimi -p \"$(cat)\"".into(),
            ])
        );
    }

    #[test]
    fn command_line_parse_handles_quoting_and_escapes() {
        assert_eq!(
            parse_command_line("  claude   --print  "),
            Ok(vec!["claude".into(), "--print".into()])
        );
        assert_eq!(
            parse_command_line("'one two' three"),
            Ok(vec!["one two".into(), "three".into()])
        );
        assert_eq!(
            parse_command_line(r"a\ b c"),
            Ok(vec!["a b".into(), "c".into()])
        );
        assert_eq!(
            parse_command_line(r"'a\ b'"),
            Ok(vec![r"a\ b".into()])
        );
        assert_eq!(parse_command_line(""), Ok(Vec::new()));
        assert_eq!(parse_command_line("   "), Ok(Vec::new()));
    }

    #[test]
    fn command_line_parse_reports_unterminated_quotes_instead_of_panicking() {
        assert_eq!(
            parse_command_line("foo 'bar"),
            Err("单引号没有闭合".into())
        );
        assert_eq!(
            parse_command_line(r#"foo "bar"#),
            Err("双引号没有闭合".into())
        );
        assert_eq!(parse_command_line(r"foo\"), Err("反斜杠后面缺字符".into()));
    }

    #[test]
    fn command_line_round_trips_are_general() {
        let cases: [&[&str]; 6] = [
            &[],
            &["single"],
            &["a", "b", "c"],
            &["has space"],
            &["has\"quote"],
            &["has\\backslash", "and'quote", ""],
        ];
        for case in cases {
            let command: Vec<String> = case.iter().map(|s| s.to_string()).collect();
            let rendered = render_command_line(&command);
            assert_eq!(
                parse_command_line(&rendered),
                Ok(command.clone()),
                "round trip of {command:?} via {rendered:?}"
            );
        }
    }

    #[test]
    fn provider_toggle_switches_visible_fields_and_clamps_selection() {
        let mut form = ClientKeModelOverlay::default();
        assert!(form.is_openai());
        let openai_fields = form.visible_fields();
        assert_eq!(openai_fields.len(), 10);
        form.field = openai_fields.len() - 1; // last openai row (panel model)
        form.cycle_provider(1);
        assert!(!form.is_openai());
        let cli_fields = form.visible_fields();
        assert_eq!(cli_fields.len(), 5);
        assert!(form.field < cli_fields.len(), "selection must stay in range");
        form.cycle_provider(1);
        assert!(form.is_openai());
    }

    #[test]
    fn profile_rejects_empty_cli_command() {
        let form = ClientKeModelOverlay {
            provider: 1,
            name: TextEditor::from("agent"),
            command: TextEditor::from("   "),
            ..ClientKeModelOverlay::default()
        };
        assert!(form.profile().is_err());
    }

    #[test]
    fn profile_builds_cli_provider_with_parsed_command() {
        let form = ClientKeModelOverlay {
            provider: 1,
            name: TextEditor::from("agent"),
            command: TextEditor::from("opencode run"),
            ..ClientKeModelOverlay::default()
        };
        let (name, profile) = form.profile().expect("cli profile");
        assert_eq!(name, "agent");
        assert_eq!(profile.provider, "cli");
        assert_eq!(profile.command, vec!["opencode", "run"]);
    }
}
