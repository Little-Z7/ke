//! Added by ke: `/ke` commands and `//` passthrough, parsed on the client before the processor.

use crate::config::{upsert_section_bool, upsert_section_value, KeConfig, KeModelProfile};

pub(crate) const HELP: &str = "\
/ke help
/ke model            打开设置里的模型页（含 cli 预设）
/ke model [预设名|模板|模型id]
/ke redact           打开设置里的脱敏页
/ke provider <名> openai <base_url> <KEY_ENV>
/ke prompt
/ke memory
/ke log
/ke status
/ke update           终端 ke update 的安装命令
//               窗格斜杠指令（Tab 补全）
@ke 话           问管家";

/// Built-in OpenAI-compatible access templates. These are not subscription products.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlanTemplate {
    pub name: &'static str,
    pub label: &'static str,
    pub base_url: &'static str,
    pub model: &'static str,
    pub api_key_env: &'static str,
}

pub(crate) const PLAN_TEMPLATES: &[PlanTemplate] = &[
    PlanTemplate {
        name: "local",
        label: "Ollama 本地",
        base_url: "http://localhost:11434/v1",
        model: "qwen3:4b",
        api_key_env: "",
    },
    PlanTemplate {
        name: "ark",
        label: "火山方舟 / Coding Plan",
        base_url: "https://ark.cn-beijing.volces.com/api/v3",
        model: "doubao-seed-1-6",
        api_key_env: "ARK_API_KEY",
    },
    PlanTemplate {
        name: "openai",
        label: "OpenAI",
        base_url: "https://api.openai.com/v1",
        model: "gpt-4o",
        api_key_env: "OPENAI_API_KEY",
    },
];

pub(crate) fn plan_template(name: &str) -> Option<&'static PlanTemplate> {
    PLAN_TEMPLATES.iter().find(|plan| plan.name == name)
}

impl PlanTemplate {
    pub(crate) fn profile(self) -> KeModelProfile {
        KeModelProfile {
            provider: "openai".into(),
            base_url: self.base_url.into(),
            api_key_env: self.api_key_env.into(),
            model: self.model.into(),
            ..KeModelProfile::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SlashHint {
    pub fill: &'static str,
    pub label: &'static str,
}

pub(crate) const KE_SLASH_HINTS: &[SlashHint] = &[
    SlashHint {
        fill: "/ke redact",
        label: "脱敏规则",
    },
    SlashHint {
        fill: "/ke model",
        label: "模型配置",
    },
    SlashHint {
        fill: "/ke status",
        label: "管家状态",
    },
    SlashHint {
        fill: "/ke update",
        label: "更新命令",
    },
    SlashHint {
        fill: "/ke log",
        label: "最近对话",
    },
    SlashHint {
        fill: "/ke prompt",
        label: "提示词文件",
    },
    SlashHint {
        fill: "/ke memory",
        label: "记忆文件",
    },
    SlashHint {
        fill: "/ke help",
        label: "命令列表",
    },
    SlashHint {
        fill: "/ke provider",
        label: "新建接口预设",
    },
];

pub(crate) const PANE_SLASH_HINTS: &[SlashHint] = &[
    SlashHint {
        fill: "//clear",
        label: "清对话",
    },
    SlashHint {
        fill: "//compact",
        label: "压缩上下文",
    },
    SlashHint {
        fill: "//help",
        label: "帮助",
    },
    SlashHint {
        fill: "//status",
        label: "状态",
    },
    SlashHint {
        fill: "//model",
        label: "切模型",
    },
    SlashHint {
        fill: "//cost",
        label: "用量",
    },
    SlashHint {
        fill: "//memory",
        label: "记忆",
    },
    SlashHint {
        fill: "//init",
        label: "初始化",
    },
    SlashHint {
        fill: "//review",
        label: "回顾",
    },
    SlashHint {
        fill: "//exit",
        label: "退出",
    },
];

/// Hints for a composer line that is `/ke…` or `//…`.
pub(crate) fn slash_palette(text: &str) -> Option<Vec<SlashHint>> {
    let text = text.trim();
    let (hints, query) = if text == "/ke" || text.starts_with("/ke ") {
        (
            KE_SLASH_HINTS,
            text.strip_prefix("/ke").unwrap_or("").trim(),
        )
    } else if text.starts_with("//") {
        (PANE_SLASH_HINTS, text.strip_prefix("//").unwrap_or(""))
    } else {
        return None;
    };
    let query = query.to_ascii_lowercase();
    let hits: Vec<SlashHint> = hints
        .iter()
        .copied()
        .filter(|hint| {
            query.is_empty()
                || hint.fill.to_ascii_lowercase().contains(&query)
                || hint.label.contains(&query)
        })
        .collect();
    (!hits.is_empty()).then_some(hits)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SlashCommand {
    Help,
    Redact,
    Status,
    Log,
    Prompt,
    Memory,
    Update,
    Model {
        arg: Option<String>,
    },
    Provider {
        name: String,
        kind: String,
        base_url: String,
        key_env: String,
    },
    Unknown(String),
}

/// `Some` when the line is a ke slash command (`/ke` or `/ke …`).
pub(crate) fn parse_ke_command(text: &str) -> Option<SlashCommand> {
    let text = text.trim();
    let rest = if text == "/ke" {
        ""
    } else {
        text.strip_prefix("/ke ")?
    };
    let rest = rest.trim();
    if rest.is_empty() || rest == "help" {
        return Some(SlashCommand::Help);
    }
    let mut parts = rest.split_whitespace();
    let Some(verb) = parts.next() else {
        return Some(SlashCommand::Help);
    };
    Some(match verb {
        "help" => SlashCommand::Help,
        "redact" => SlashCommand::Redact,
        "status" => SlashCommand::Status,
        "log" => SlashCommand::Log,
        "prompt" => SlashCommand::Prompt,
        "memory" => SlashCommand::Memory,
        "update" => SlashCommand::Update,
        "model" => SlashCommand::Model {
            arg: parts.next().map(str::to_string),
        },
        "provider" => {
            let name = parts.next().unwrap_or_default();
            let kind = parts.next().unwrap_or_default();
            let base_url = parts.next().unwrap_or_default();
            let key_env = parts.next().unwrap_or_default();
            if name.is_empty() || kind.is_empty() || base_url.is_empty() || key_env.is_empty() {
                SlashCommand::Unknown("用法：/ke provider <名> openai <base_url> <KEY_ENV>".into())
            } else {
                SlashCommand::Provider {
                    name: name.to_string(),
                    kind: kind.to_string(),
                    base_url: base_url.to_string(),
                    key_env: key_env.to_string(),
                }
            }
        }
        other => SlashCommand::Unknown(format!("未知命令 /ke {other}。输入 /ke help 看列表。")),
    })
}

/// `//foo` → `/foo` (strip exactly one leading `/`). Anything else is `None`.
pub(crate) fn passthrough_after_slash_escape(text: &str) -> Option<&str> {
    let text = text.trim();
    text.starts_with("//").then(|| &text[1..])
}

pub(crate) fn toml_quote(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(crate) fn profile_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

pub(crate) fn apply_active_profile(content: &str, name: &str) -> String {
    upsert_section_value(content, "ke.model", "active", &toml_quote(name))
}

pub(crate) fn apply_profile_model(
    content: &str,
    name: &str,
    profile: &KeModelProfile,
    model: &str,
) -> String {
    let section = format!("ke.model.profiles.{name}");
    let mut next = content.to_string();
    next = upsert_section_value(&next, &section, "provider", &toml_quote(&profile.provider));
    if !profile.base_url.is_empty() {
        next = upsert_section_value(&next, &section, "base_url", &toml_quote(&profile.base_url));
    }
    if !profile.api_key_env.is_empty() {
        next = upsert_section_value(
            &next,
            &section,
            "api_key_env",
            &toml_quote(&profile.api_key_env),
        );
    }
    upsert_section_value(&next, &section, "model", &toml_quote(model))
}

pub(crate) fn apply_provider(content: &str, name: &str, base_url: &str, key_env: &str) -> String {
    let section = format!("ke.model.profiles.{name}");
    let mut next = upsert_section_value(content, &section, "provider", &toml_quote("openai"));
    next = upsert_section_value(&next, &section, "base_url", &toml_quote(base_url));
    upsert_section_value(&next, &section, "api_key_env", &toml_quote(key_env))
}

/// Builds a TOML array literal from string elements, e.g. `["opencode", "run"]`.
fn toml_string_array(values: &[String]) -> String {
    let items: Vec<String> = values.iter().map(|value| toml_quote(value)).collect();
    format!("[{}]", items.join(", "))
}

pub(crate) fn apply_model_form(
    content: &str,
    enabled: bool,
    name: &str,
    profile: &KeModelProfile,
    cloud_confirmed: bool,
) -> String {
    let mut next = upsert_section_bool(content, "ke.model", "enabled", enabled);
    next = upsert_section_value(&next, "ke.model", "active", &toml_quote(name));
    if cloud_confirmed {
        next = upsert_section_bool(&next, "ke.model", "cloud_confirmed", true);
    }
    let section = format!("ke.model.profiles.{name}");
    // Modified by ke: write the provider the form actually selected instead of always hardcoding
    // "openai" (that used to silently rewrite a hand-written `provider = "cli"` preset back to
    // "openai" the moment someone pressed apply on any other field in Settings). Only write the
    // fields that make sense for that provider so saving a cli preset does not also leave stale
    // openai-only fields (or vice versa).
    next = upsert_section_value(&next, &section, "provider", &toml_quote(&profile.provider));
    if profile.provider == "cli" {
        next = upsert_section_value(
            &next,
            &section,
            "command",
            &toml_string_array(&profile.command),
        );
    } else {
        next = upsert_section_value(&next, &section, "base_url", &toml_quote(&profile.base_url));
        next = upsert_section_value(
            &next,
            &section,
            "api_key_env",
            &toml_quote(&profile.api_key_env),
        );
        next = upsert_section_value(&next, &section, "model", &toml_quote(&profile.model));
        next = upsert_section_value(&next, &section, "think", &toml_quote(&profile.think));
        next = upsert_section_value(
            &next,
            &section,
            "max_tokens",
            &profile.max_tokens.to_string(),
        );
    }
    next
}

pub(crate) fn apply_plan_template(content: &str, plan: &PlanTemplate) -> String {
    let profile = plan.profile();
    apply_model_form(content, true, plan.name, &profile, !profile.is_local())
}

pub(crate) fn describe_model(ke: &KeConfig) -> String {
    let mut lines = Vec::new();
    match ke.resolved_profile() {
        Some((name, profile)) => {
            lines.push(format!(
                "当前预设 {name} · {} · {}",
                if profile.model.is_empty() {
                    "(未设模型)"
                } else {
                    profile.model.as_str()
                },
                if profile.base_url.is_empty() {
                    profile.provider.as_str()
                } else {
                    profile.base_url.as_str()
                }
            ));
        }
        None => lines.push(format!(
            "当前预设 {} 没有配置。用 /ke provider 建一个。",
            ke.model.active
        )),
    }
    lines.push(format!("提示词 {}", ke.model.system_prompt_file));
    if ke.model.profiles.is_empty() {
        lines.push("可切换预设：local（内置）".into());
    } else {
        let names: Vec<_> = ke.model.profiles.keys().map(String::as_str).collect();
        lines.push(format!("可切换预设：{}", names.join("、")));
    }
    lines.push(format!(
        "接入模板：{}",
        PLAN_TEMPLATES
            .iter()
            .map(|plan| format!("{}（{}）", plan.name, plan.label))
            .collect::<Vec<_>>()
            .join("、")
    ));
    lines.join("\n")
}

/// Text for `/ke update`: the same installer `ke update` runs in the terminal.
pub(crate) fn update_text() -> String {
    format!(
        "当前版本 ke {}\n终端执行 ke update 会跑官方安装脚本（不走 herdr 更新通道）：\n{}",
        crate::build_info::KE_VERSION,
        crate::build_info::KE_INSTALL_COMMAND
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_recognizes_ke_commands() {
        assert_eq!(parse_ke_command("/ke"), Some(SlashCommand::Help));
        assert_eq!(parse_ke_command("  /ke help  "), Some(SlashCommand::Help));
        assert_eq!(parse_ke_command("/ke log"), Some(SlashCommand::Log));
        assert_eq!(parse_ke_command("/ke update"), Some(SlashCommand::Update));
        assert_eq!(
            parse_ke_command("/ke model"),
            Some(SlashCommand::Model { arg: None })
        );
        assert_eq!(
            parse_ke_command("/ke model ark"),
            Some(SlashCommand::Model {
                arg: Some("ark".into())
            })
        );
        assert_eq!(
            parse_ke_command("/ke provider ark openai https://example/v3 ARK_API_KEY"),
            Some(SlashCommand::Provider {
                name: "ark".into(),
                kind: "openai".into(),
                base_url: "https://example/v3".into(),
                key_env: "ARK_API_KEY".into(),
            })
        );
        assert!(
            matches!(parse_ke_command("/ke nope"), Some(SlashCommand::Unknown(_))),
            "unknown verb"
        );
        assert!(parse_ke_command("/clear").is_none());
        assert!(parse_ke_command("//ke log").is_none());
        assert!(parse_ke_command("@ke hi").is_none());
        assert!(parse_ke_command("/kehelp").is_none());
    }

    #[test]
    fn slash_escape_strips_exactly_one_leading_slash() {
        assert_eq!(passthrough_after_slash_escape("//clear"), Some("/clear"));
        assert_eq!(
            passthrough_after_slash_escape("  //ke log"),
            Some("/ke log")
        );
        assert_eq!(passthrough_after_slash_escape("/ke log"), None);
        assert_eq!(passthrough_after_slash_escape("hello"), None);
    }

    #[test]
    fn apply_writes_profile_sections() {
        let next = apply_active_profile("", "ark");
        assert!(next.contains("[ke.model]"));
        assert!(next.contains("active = \"ark\""));
        let next = apply_provider("", "ark", "https://example/v3", "ARK_API_KEY");
        assert!(next.contains("[ke.model.profiles.ark]"));
        assert!(next.contains("provider = \"openai\""));
        assert!(next.contains("base_url = \"https://example/v3\""));
        assert!(next.contains("api_key_env = \"ARK_API_KEY\""));
        let next = apply_profile_model(
            "",
            "local",
            &KeModelProfile {
                provider: "openai".into(),
                base_url: "http://localhost:11434/v1".into(),
                model: "qwen3:4b".into(),
                ..KeModelProfile::default()
            },
            "qwen3:8b",
        );
        assert!(next.contains("model = \"qwen3:8b\""));
        assert!(next.contains("base_url = \"http://localhost:11434/v1\""));
        let form = apply_model_form(
            "",
            true,
            "ark",
            &KeModelProfile {
                provider: "openai".into(),
                base_url: "https://example/v3".into(),
                api_key_env: "ARK_API_KEY".into(),
                model: "ep-demo".into(),
                think: "on".into(),
                max_tokens: 4096,
                ..KeModelProfile::default()
            },
            true,
        );
        assert!(form.contains("enabled = true"));
        assert!(form.contains("cloud_confirmed = true"));
        assert!(form.contains("model = \"ep-demo\""));
        assert!(form.contains("think = \"on\""));
        assert!(form.contains("max_tokens = 4096"));
        let plan = apply_plan_template("", plan_template("ark").expect("ark"));
        assert!(plan.contains("active = \"ark\""));
        assert!(plan.contains("ark.cn-beijing.volces.com"));
        assert!(plan.contains("ARK_API_KEY"));
        assert!(plan.contains("cloud_confirmed = true"));
    }

    /// A hand-written `provider = "cli"` preset must round-trip through the form: apply must
    /// write the provider the caller actually selected, not silently rewrite it back to
    /// "openai" (a real bug: the old code hardcoded `toml_quote("openai")` here), and cli-only
    /// fields (`command`) must be written while openai-only fields stay out.
    #[test]
    fn apply_model_form_writes_the_selected_provider_not_always_openai() {
        let form = apply_model_form(
            "",
            true,
            "kimi-cli",
            &KeModelProfile {
                provider: "cli".into(),
                command: vec!["sh".into(), "-c".into(), "kimi -p \"$(cat)\"".into()],
                ..KeModelProfile::default()
            },
            false,
        );
        assert!(form.contains("[ke.model.profiles.kimi-cli]"));
        assert!(form.contains("provider = \"cli\""));
        assert!(!form.contains("provider = \"openai\""));
        assert!(form.contains(r#"command = ["sh", "-c", "kimi -p \"$(cat)\""]"#));
        assert!(!form.contains("base_url"));
        assert!(!form.contains("api_key_env"));
        assert!(!form.contains("max_tokens"));

        // Re-applying an openai profile still writes all its fields (no regression from the
        // provider-conditional branch).
        let openai_form = apply_model_form(
            "",
            true,
            "ark",
            &KeModelProfile {
                provider: "openai".into(),
                base_url: "https://example/v3".into(),
                ..KeModelProfile::default()
            },
            false,
        );
        assert!(openai_form.contains("provider = \"openai\""));
        assert!(openai_form.contains("base_url = \"https://example/v3\""));
        assert!(!openai_form.contains("command ="));
    }

    #[test]
    fn profile_names_reject_path_pieces() {
        assert!(profile_name_ok("local"));
        assert!(profile_name_ok("claude-sub"));
        assert!(!profile_name_ok("a.b"));
        assert!(!profile_name_ok("a b"));
        assert!(!profile_name_ok(""));
    }

    #[test]
    fn slash_palette_filters_ke_and_pane_hints() {
        let ke = slash_palette("/ke").expect("ke palette");
        assert!(ke.iter().any(|hint| hint.fill == "/ke model"));
        assert!(ke.iter().any(|hint| hint.fill == "/ke update"));
        let model = slash_palette("/ke mo").expect("filter");
        let update = slash_palette("/ke up").expect("update");
        assert_eq!(update[0].fill, "/ke update");
        assert_eq!(model[0].fill, "/ke model");
        let pane = slash_palette("//cl").expect("pane palette");
        assert_eq!(pane[0].fill, "//clear");
        assert!(slash_palette("/compact").is_none());
        assert!(slash_palette("@ke").is_none());
    }
}
