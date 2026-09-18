//! Added by ke: TUI form for `[ke.model]` (endpoint, model id, thinking, context).

use super::*;
use crate::config::KeModelProfile;
use crate::ke::slash::{self, PLAN_TEMPLATES, PlanTemplate};

pub(super) const THINK_MODES: [&str; 3] = ["", "off", "on"];
pub(super) const THINK_LABELS: [&str; 3] = ["自动", "关", "开"];
const CUSTOM_PLAN: usize = 0;
const FIELD_COUNT: usize = 9;

#[derive(Debug)]
pub(super) struct ClientKeModelOverlay {
    pub(super) field: usize,
    pub(super) enabled: bool,
    pub(super) plan: usize,
    pub(super) name: TextEditor,
    pub(super) base_url: TextEditor,
    pub(super) model: TextEditor,
    pub(super) api_key_env: TextEditor,
    pub(super) think: usize,
    pub(super) max_tokens: TextEditor,
}

impl ClientKeModelOverlay {
    pub(super) fn from_config(ke: &crate::config::KeConfig) -> Self {
        let (name, profile) = ke
            .resolved_profile()
            .unwrap_or_else(|| ("local".into(), PLAN_TEMPLATES[0].profile()));
        let think = THINK_MODES
            .iter()
            .position(|mode| *mode == profile.think.trim())
            .unwrap_or(0);
        let max_tokens = if profile.max_tokens == 0 {
            String::new()
        } else {
            profile.max_tokens.to_string()
        };
        Self {
            field: 1,
            enabled: ke.model.enabled,
            plan: match_plan(&name, &profile),
            name: TextEditor::from(name.as_str()),
            base_url: TextEditor::from(profile.base_url.as_str()),
            model: TextEditor::from(profile.model.as_str()),
            api_key_env: TextEditor::from(profile.api_key_env.as_str()),
            think,
            max_tokens: TextEditor::from(max_tokens.as_str()),
        }
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
        match self.field {
            2 => Some(&mut self.name),
            3 => Some(&mut self.base_url),
            4 => Some(&mut self.model),
            5 => Some(&mut self.api_key_env),
            7 => Some(&mut self.max_tokens),
            _ => None,
        }
    }

    fn cycle_think(&mut self, delta: isize) {
        let len = THINK_MODES.len() as isize;
        self.think = (self.think as isize + delta).rem_euclid(len) as usize;
    }

    fn cycle_plan(&mut self, delta: isize) {
        let count = (PLAN_TEMPLATES.len() + 1) as isize;
        self.plan = (self.plan as isize + delta).rem_euclid(count) as usize;
        if let Some(plan) = PLAN_TEMPLATES.get(self.plan.saturating_sub(1)) {
            self.apply_plan(*plan);
        }
    }

    fn apply_plan(&mut self, plan: PlanTemplate) {
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
                provider: "openai".into(),
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

impl ClientShellState {
    pub(super) fn open_ke_model_form(&mut self, outcome: &mut ClientShellInput) {
        self.overlay = Some(ClientShellOverlay::KeModel(
            ClientKeModelOverlay::from_config(&self.config.ke),
        ));
        outcome.repaint = true;
    }

    pub(super) fn handle_ke_model_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(ClientShellOverlay::KeModel(_)) = self.overlay.as_ref() else {
            return false;
        };
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
                if self.ke_model_field() == 8 {
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
        if let Some(ClientShellOverlay::KeModel(form)) = self.overlay.as_mut() {
            if let Some(editor) = form.active_editor() {
                if editor.handle_key(key).is_some() {
                    outcome.repaint = true;
                }
            }
        }
        true
    }

    fn ke_model_field(&self) -> usize {
        match &self.overlay {
            Some(ClientShellOverlay::KeModel(form)) => form.field,
            _ => 0,
        }
    }

    fn ke_model_is_toggle(&self) -> bool {
        matches!(self.ke_model_field(), 0 | 1 | 6)
    }

    fn ke_model_nudge_field(&mut self, delta: isize) {
        if let Some(ClientShellOverlay::KeModel(form)) = self.overlay.as_mut() {
            let count = FIELD_COUNT as isize;
            form.field = (form.field as isize + delta).rem_euclid(count) as usize;
        }
    }

    fn ke_model_toggle(&mut self, delta: isize) {
        if let Some(ClientShellOverlay::KeModel(form)) = self.overlay.as_mut() {
            match form.field {
                0 => form.enabled = !form.enabled,
                1 => form.cycle_plan(delta),
                6 => form.cycle_think(delta),
                _ => {}
            }
        }
    }

    fn save_ke_model_form(&mut self, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::KeModel(form)) = self.overlay.as_ref() else {
            return;
        };
        let enabled = form.enabled;
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
                slash::apply_model_form(content, enabled, &name, &profile, cloud_confirmed)
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
