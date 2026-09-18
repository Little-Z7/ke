//! Added by ke: the resident's system prompt (built-in constitution + user-editable resident.md).

use std::path::Path;

use super::model::ChatMessage;

/// Role constitution compiled into the binary. Users can add tone in `resident.md`; they cannot
/// replace this.
pub(crate) const CONSTITUTION: &str = "\
你是 ke 的管家，只读顾问。你看得见当前会话里每一个 agent 窗格的状态，以及焦点窗格和被阻塞窗格的最近输出。

职责：
- 根据用户问题和窗格快照，说明谁在忙、谁在等、谁空着，以及你看见的风险或卡住的点。
- 给出简短、可执行的建议，但你自己不去执行。

禁止：
- 写代码、改文件、跑命令、点权限或批准工具调用。
- 给任何 agent 发消息，或要求别人替你去跑命令。
- 编造窗格里没有出现的状态、输出或对话。
- 把快照里的密钥、token、内网地址再说一遍。

语言跟随用户。不知道就说不知道，不要猜窗格里看不见的内容。";

/// Default `~/.config/ke/resident.md` when the file does not exist yet.
pub(crate) const DEFAULT_RESIDENT_MD: &str = include_str!("../defaults/resident.md");

const DEFAULT_PROMPT_PATH: &str = "~/.config/ke/resident.md";

/// Reads `system_prompt_file`. Missing files seed the default path when it does not exist yet.
pub(crate) fn load_user_prompt(path: &str) -> String {
    let expanded = crate::worktree::expand_tilde_path(path);
    let default = crate::worktree::expand_tilde_path(DEFAULT_PROMPT_PATH);
    load_user_prompt_from(&expanded, &default)
}

pub(crate) fn load_user_prompt_from(path: &Path, default_path: &Path) -> String {
    if let Ok(text) = std::fs::read_to_string(path) {
        return text;
    }
    if !default_path.exists() {
        if let Some(parent) = default_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(default_path, DEFAULT_RESIDENT_MD);
    }
    if path != default_path {
        if let Ok(text) = std::fs::read_to_string(path) {
            return text;
        }
    }
    std::fs::read_to_string(default_path).unwrap_or_else(|_| DEFAULT_RESIDENT_MD.to_string())
}

/// Constitution (plus resident.md) in one system message; the pane snapshot in a separate one.
pub(crate) fn system_messages(
    constitution: &str,
    user_prompt: &str,
    snapshot: &str,
) -> Vec<ChatMessage> {
    let mut system = constitution.trim().to_string();
    let user_prompt = user_prompt.trim();
    if !user_prompt.is_empty() {
        system.push_str("\n\n");
        system.push_str(user_prompt);
    }
    vec![
        ChatMessage::system(system),
        ChatMessage::system(format!("当前会话窗格快照：\n{snapshot}")),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_messages_put_constitution_and_snapshot_in_separate_parts() {
        let messages = system_messages(CONSTITUTION, "口吻短一点。", "pane_id=w1:p1 agent=codex");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert!(
            messages[0].content.contains("只读顾问"),
            "{}",
            messages[0].content
        );
        assert!(
            messages[0].content.contains("口吻短一点。"),
            "user prompt is appended to the constitution message"
        );
        assert!(
            !messages[0].content.contains("pane_id=w1:p1"),
            "snapshot must not leak into the constitution message"
        );
        assert_eq!(messages[1].role, "system");
        assert!(
            messages[1].content.contains("pane_id=w1:p1 agent=codex"),
            "{}",
            messages[1].content
        );
        assert!(
            !messages[1].content.contains("只读顾问"),
            "constitution stays out of the snapshot message"
        );
    }

    #[test]
    fn load_user_prompt_reads_file_or_seeds_the_default() {
        let dir = std::env::temp_dir().join(format!(
            "ke-prompt-{}-{}",
            std::process::id(),
            crate::ke::chat_log::now_epoch().to_bits()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let custom = dir.join("resident.md");
        std::fs::write(&custom, "自定义口吻").unwrap();
        assert_eq!(
            load_user_prompt_from(&custom, &dir.join("missing.md")),
            "自定义口吻"
        );

        let missing = dir.join("nope.md");
        let seed = dir.join("seed.md");
        let loaded = load_user_prompt_from(&missing, &seed);
        assert_eq!(loaded, DEFAULT_RESIDENT_MD);
        assert_eq!(std::fs::read_to_string(&seed).unwrap(), DEFAULT_RESIDENT_MD);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
