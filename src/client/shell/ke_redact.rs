//! Added by ke: TUI form for `[ke.redact]`. There is no free-text here (every field is a
//! bool toggle), so unlike `ke_model.rs` this form does not need a text editor or a dynamic
//! field list: `field` is always a plain index into [`REDACT_FIELDS`].

use super::*;
use crate::config::KeRedactConfig;

pub(super) const REDACT_FIELD_COUNT: usize = 6;

/// `(label, description)` for each `[ke.redact]` toggle, in display order. The description says
/// what turning the toggle *off* (or, for the two default-off families, *on*) changes, so the
/// meaning of flipping it is never a guess.
pub(super) const REDACT_FIELDS: [(&str, &str); REDACT_FIELD_COUNT] = [
    (
        "API 密钥格式",
        "sk-…、AKIA…、ghp_…、xox… 等常见密钥/令牌格式。关闭后这些内容会原样发给 agent 和模型。",
    ),
    (
        "PEM 私钥块",
        "-----BEGIN … PRIVATE KEY----- 这类私钥块。关闭后私钥内容会原样发出。",
    ),
    (
        "KEY=secret 环境变量行",
        "形如 KEY=value 且 KEY 像密钥/令牌/密码的整行。关闭后这些行会原样发出。",
    ),
    (
        "内网 IP",
        "RFC 1918 内网地址（10.x / 172.16.x / 192.168.x 等）。默认不打码，开启后才会脱敏。",
    ),
    (
        "邮箱地址",
        "邮箱地址。默认不打码，开启后才会脱敏。",
    ),
    (
        "本机模型也脱敏",
        "默认只脱敏发往非本机 profile 的内容；开启后连本机 profile（如本地 Ollama、cli 预设）也会脱敏。",
    ),
];

#[derive(Debug)]
pub(super) struct ClientKeRedactOverlay {
    pub(super) field: usize,
    pub(super) api_keys: bool,
    pub(super) private_keys: bool,
    pub(super) env_secrets: bool,
    pub(super) internal_ips: bool,
    pub(super) emails: bool,
    pub(super) redact_for_local_models: bool,
}

impl Default for ClientKeRedactOverlay {
    fn default() -> Self {
        Self::from_config(&crate::config::KeConfig::default())
    }
}

impl ClientKeRedactOverlay {
    pub(super) fn from_config(ke: &crate::config::KeConfig) -> Self {
        Self {
            field: 0,
            api_keys: ke.redact.api_keys,
            private_keys: ke.redact.private_keys,
            env_secrets: ke.redact.env_secrets,
            internal_ips: ke.redact.internal_ips,
            emails: ke.redact.emails,
            redact_for_local_models: ke.redact.redact_for_local_models,
        }
    }

    pub(super) fn value(&self, field: usize) -> bool {
        match field {
            0 => self.api_keys,
            1 => self.private_keys,
            2 => self.env_secrets,
            3 => self.internal_ips,
            4 => self.emails,
            5 => self.redact_for_local_models,
            _ => false,
        }
    }

    fn toggle_field(&mut self, field: usize) {
        match field {
            0 => self.api_keys = !self.api_keys,
            1 => self.private_keys = !self.private_keys,
            2 => self.env_secrets = !self.env_secrets,
            3 => self.internal_ips = !self.internal_ips,
            4 => self.emails = !self.emails,
            5 => self.redact_for_local_models = !self.redact_for_local_models,
            _ => {}
        }
    }

    pub(super) fn config(&self) -> KeRedactConfig {
        KeRedactConfig {
            api_keys: self.api_keys,
            private_keys: self.private_keys,
            env_secrets: self.env_secrets,
            internal_ips: self.internal_ips,
            emails: self.emails,
            redact_for_local_models: self.redact_for_local_models,
        }
    }
}

/// Writes all six `[ke.redact]` keys via the shared `upsert_section_bool` helper (same approach
/// as every other Settings write; no ad hoc TOML serialization).
fn apply_redact_form(content: &str, config: &KeRedactConfig) -> String {
    let mut next =
        crate::config::upsert_section_bool(content, "ke.redact", "api_keys", config.api_keys);
    next =
        crate::config::upsert_section_bool(&next, "ke.redact", "private_keys", config.private_keys);
    next =
        crate::config::upsert_section_bool(&next, "ke.redact", "env_secrets", config.env_secrets);
    next =
        crate::config::upsert_section_bool(&next, "ke.redact", "internal_ips", config.internal_ips);
    next = crate::config::upsert_section_bool(&next, "ke.redact", "emails", config.emails);
    crate::config::upsert_section_bool(
        &next,
        "ke.redact",
        "redact_for_local_models",
        config.redact_for_local_models,
    )
}

impl ClientShellState {
    /// Opens Settings straight on the redact page, the way `/ke redact` reaches it.
    pub(super) fn open_ke_redact_form(&mut self, outcome: &mut ClientShellInput) {
        self.open_settings_overlay();
        self.select_settings_section(ClientSettingsSection::KeRedact, outcome);
        outcome.repaint = true;
    }

    pub(super) fn ke_redact_form(&self) -> Option<&ClientKeRedactOverlay> {
        match self.overlay.as_ref() {
            Some(ClientShellOverlay::Settings(settings))
                if settings.section == ClientSettingsSection::KeRedact =>
            {
                Some(&settings.ke_redact)
            }
            _ => None,
        }
    }

    pub(super) fn ke_redact_form_mut(&mut self) -> Option<&mut ClientKeRedactOverlay> {
        match self.overlay.as_mut() {
            Some(ClientShellOverlay::Settings(settings))
                if settings.section == ClientSettingsSection::KeRedact =>
            {
                Some(&mut settings.ke_redact)
            }
            _ => None,
        }
    }

    pub(super) fn ke_redact_nudge_field(&mut self, delta: isize) {
        if let Some(form) = self.ke_redact_form_mut() {
            let count = REDACT_FIELD_COUNT as isize;
            form.field = (form.field as isize + delta).rem_euclid(count) as usize;
        }
        if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
            settings.selected = settings.ke_redact.field;
        }
    }

    /// Flips whichever field is currently selected (every redact row is a bool, so there is no
    /// per-field dispatch like `ke_model_toggle` needs).
    pub(super) fn ke_redact_toggle_current(&mut self) {
        if let Some(form) = self.ke_redact_form_mut() {
            let field = form.field;
            form.toggle_field(field);
        }
    }

    pub(super) fn save_ke_redact_form(&mut self, outcome: &mut ClientShellInput) {
        let Some(form) = self.ke_redact_form() else {
            return;
        };
        let config = form.config();
        let wrote =
            crate::config::update_file_at(&crate::config::config_path(), "ke redact", |content| {
                apply_redact_form(content, &config)
            });
        match wrote {
            Ok(()) => {
                self.overlay = None;
                self.request_config_reload(outcome);
                if let Some(composer) = self.composer.as_mut() {
                    composer.notice = Some("已保存脱敏设置".into());
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
    fn toggle_field_flips_only_the_targeted_bool() {
        let mut form = ClientKeRedactOverlay::default();
        assert!(form.api_keys);
        assert!(!form.internal_ips);
        form.toggle_field(0);
        assert!(!form.api_keys);
        form.toggle_field(3);
        assert!(form.internal_ips);
        // Untouched fields keep the config default.
        assert!(form.private_keys);
        assert!(form.env_secrets);
    }

    #[test]
    fn apply_redact_form_writes_every_key() {
        let config = KeRedactConfig {
            api_keys: false,
            private_keys: true,
            env_secrets: false,
            internal_ips: true,
            emails: true,
            redact_for_local_models: true,
        };
        let written = apply_redact_form("", &config);
        assert!(written.contains("[ke.redact]"));
        assert!(written.contains("api_keys = false"));
        assert!(written.contains("private_keys = true"));
        assert!(written.contains("env_secrets = false"));
        assert!(written.contains("internal_ips = true"));
        assert!(written.contains("emails = true"));
        assert!(written.contains("redact_for_local_models = true"));
    }
}
