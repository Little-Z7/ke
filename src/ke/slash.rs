//! Added by ke: `/ke` commands and `//` passthrough, parsed on the client before the processor.

use crate::config::{upsert_section_value, KeConfig, KeModelProfile};

pub(crate) const HELP: &str = "\
/ke help
/ke model [预设名|模型id]
/ke provider <名> openai <base_url> <KEY_ENV>
/ke prompt
/ke memory
/ke log
/ke status
//命令     去掉一个 / 后发给当前窗格
@ke 话     问管家";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SlashCommand {
    Help,
    Status,
    Log,
    Prompt,
    Memory,
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
        "status" => SlashCommand::Status,
        "log" => SlashCommand::Log,
        "prompt" => SlashCommand::Prompt,
        "memory" => SlashCommand::Memory,
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
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_recognizes_ke_commands() {
        assert_eq!(parse_ke_command("/ke"), Some(SlashCommand::Help));
        assert_eq!(parse_ke_command("  /ke help  "), Some(SlashCommand::Help));
        assert_eq!(parse_ke_command("/ke log"), Some(SlashCommand::Log));
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
                api_key_env: String::new(),
                model: "qwen3:4b".into(),
                command: Vec::new(),
            },
            "qwen3:8b",
        );
        assert!(next.contains("model = \"qwen3:8b\""));
        assert!(next.contains("base_url = \"http://localhost:11434/v1\""));
    }

    #[test]
    fn profile_names_reject_path_pieces() {
        assert!(profile_name_ok("local"));
        assert!(profile_name_ok("claude-sub"));
        assert!(!profile_name_ok("a.b"));
        assert!(!profile_name_ok("a b"));
        assert!(!profile_name_ok(""));
    }
}
