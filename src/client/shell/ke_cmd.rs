//! Added by ke: run `/ke` commands from the composer dock (client-local, no processor).

use super::*;
use crate::ke::slash::{self, SlashCommand};

impl ClientShellState {
    pub(super) fn run_ke_slash(&mut self, command: SlashCommand, outcome: &mut ClientShellInput) {
        match command {
            SlashCommand::Help | SlashCommand::Unknown(_) => {
                let note = match &command {
                    SlashCommand::Unknown(message) => Some(message.clone()),
                    _ => None,
                };
                self.finish_ke_slash(note.as_deref(), Some(("壳命令", slash::HELP)), outcome);
            }
            SlashCommand::Status => {
                let body = self.ke_status_text();
                self.finish_ke_slash(None, Some(("壳状态", &body)), outcome);
            }
            SlashCommand::Log => match self.ke_log_overlay() {
                Some(overlay) => {
                    let _ = self.set_composer_chat(overlay);
                    self.finish_ke_slash(Some("最近的管家对话"), None, outcome);
                }
                None => self.finish_ke_slash(Some("还没有 @ke 对话"), None, outcome),
            },
            SlashCommand::Prompt => {
                let path =
                    crate::worktree::expand_tilde_path(&self.config.ke.model.system_prompt_file);
                match seed_markdown(&path, crate::ke::resident::prompt::DEFAULT_RESIDENT_MD) {
                    Ok(()) => self.finish_ke_slash(
                        Some(&format!("提示词文件：{}", path.display())),
                        None,
                        outcome,
                    ),
                    Err(err) => self.finish_ke_slash(Some(&err), None, outcome),
                }
            }
            SlashCommand::Memory => {
                let path = crate::config::config_dir().join("memory.md");
                match seed_markdown(
                    &path,
                    "# 关于你\n\n（空着。用 @ke 记住 … 或直接改这个文件。）\n",
                ) {
                    Ok(()) => self.finish_ke_slash(
                        Some(&format!("记忆文件：{}", path.display())),
                        None,
                        outcome,
                    ),
                    Err(err) => self.finish_ke_slash(Some(&err), None, outcome),
                }
            }
            SlashCommand::Model { arg: None } => {
                let body = slash::describe_model(&self.config.ke);
                self.finish_ke_slash(None, Some(("壳模型", &body)), outcome);
            }
            SlashCommand::Model { arg: Some(arg) } => self.ke_slash_set_model(&arg, outcome),
            SlashCommand::Provider {
                name,
                kind,
                base_url,
                key_env,
            } => self.ke_slash_set_provider(&name, &kind, &base_url, &key_env, outcome),
        }
    }

    fn ke_slash_set_model(&mut self, arg: &str, outcome: &mut ClientShellInput) {
        if self.config.ke.model.profiles.contains_key(arg) || arg == "local" {
            if !slash::profile_name_ok(arg) {
                self.finish_ke_slash(Some("预设名只能用字母、数字、短横和下划线"), None, outcome);
                return;
            }
            let wrote = crate::config::update_file_at(
                &crate::config::config_path(),
                "ke model",
                |content| slash::apply_active_profile(content, arg),
            );
            match wrote {
                Ok(()) => {
                    self.request_config_reload(outcome);
                    self.finish_ke_slash(Some(&format!("已切换到预设 {arg}")), None, outcome);
                }
                Err(err) => self.finish_ke_slash(Some(&err), None, outcome),
            }
            return;
        }
        let (name, profile) = match self.config.ke.resolved_profile() {
            Some(pair) => pair,
            None => {
                self.finish_ke_slash(
                    Some("没有当前预设。先 /ke provider 建一个，或 /ke model local"),
                    None,
                    outcome,
                );
                return;
            }
        };
        if !slash::profile_name_ok(&name) {
            self.finish_ke_slash(Some("当前预设名不合法，改不了模型"), None, outcome);
            return;
        }
        let wrote =
            crate::config::update_file_at(&crate::config::config_path(), "ke model", |content| {
                slash::apply_profile_model(content, &name, &profile, arg)
            });
        match wrote {
            Ok(()) => {
                self.request_config_reload(outcome);
                self.finish_ke_slash(
                    Some(&format!("预设 {name} 的模型改为 {arg}")),
                    None,
                    outcome,
                );
            }
            Err(err) => self.finish_ke_slash(Some(&err), None, outcome),
        }
    }

    fn ke_slash_set_provider(
        &mut self,
        name: &str,
        kind: &str,
        base_url: &str,
        key_env: &str,
        outcome: &mut ClientShellInput,
    ) {
        if !slash::profile_name_ok(name) {
            self.finish_ke_slash(Some("预设名只能用字母、数字、短横和下划线"), None, outcome);
            return;
        }
        if kind != "openai" {
            self.finish_ke_slash(
                Some("v1 只支持 openai。用法：/ke provider <名> openai <base_url> <KEY_ENV>"),
                None,
                outcome,
            );
            return;
        }
        let wrote = crate::config::update_file_at(
            &crate::config::config_path(),
            "ke provider",
            |content| slash::apply_provider(content, name, base_url, key_env),
        );
        match wrote {
            Ok(()) => {
                self.request_config_reload(outcome);
                self.finish_ke_slash(
                    Some(&format!("已写入预设 {name}。/ke model {name} 可切换到它")),
                    None,
                    outcome,
                );
            }
            Err(err) => self.finish_ke_slash(Some(&err), None, outcome),
        }
    }

    fn finish_ke_slash(
        &mut self,
        notice: Option<&str>,
        chat: Option<(&str, &str)>,
        outcome: &mut ClientShellInput,
    ) {
        if let Some((title, body)) = chat {
            let _ = self.set_composer_chat(super::ke_chat::ClientKeChatOverlay {
                title: title.into(),
                body: body.into(),
                scroll: 0,
            });
            outcome.resize = true;
        }
        if let Some(composer) = self.composer.as_mut() {
            composer.input.clear();
            composer.notice = notice.map(str::to_string);
            composer.focused = true;
        }
        outcome.repaint = true;
    }

    fn ke_status_text(&self) -> String {
        let socket = self.config.ke.composer_socket_path();
        let socket_live = socket.as_ref().is_some_and(|path| path.exists());
        let mut lines = Vec::new();
        lines.push(if socket_live {
            format!(
                "管家：在线 · {}",
                socket
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default()
            )
        } else if self.config.ke.model.enabled {
            "管家：已启用，但还没看到 composer.sock（进程可能在重启）".into()
        } else {
            "管家：未启用。在 config.toml 把 [ke.model] enabled 设为 true".into()
        });
        lines.push(slash::describe_model(&self.config.ke));
        if let Some(path) = self.config.ke.panel_file_path() {
            match std::fs::metadata(&path).and_then(|meta| meta.modified()) {
                Ok(mtime) => {
                    let age = std::time::SystemTime::now()
                        .duration_since(mtime)
                        .map(|elapsed| elapsed.as_secs())
                        .unwrap_or(0);
                    lines.push(format!("面板：{age}s 前更新 · {}", path.display()));
                }
                Err(_) => lines.push(format!("面板：还没有 · {}", path.display())),
            }
        }
        if let Some(path) = self.config.ke.chat_log_path() {
            match crate::ke::chat_log::read_tail(&path, 32) {
                Ok(entries) => {
                    let last = entries
                        .iter()
                        .rev()
                        .find(|entry| {
                            entry.role == crate::ke::chat_log::ChatRole::Assistant && !entry.pending
                        })
                        .map(|entry| entry.text.as_str())
                        .unwrap_or("还没有回答");
                    let preview: String = last.chars().take(80).collect();
                    lines.push(format!("最近回答：{preview}"));
                }
                Err(_) => lines.push("最近回答：读不了 chat.jsonl".into()),
            }
        }
        lines.join("\n")
    }

    fn ke_log_overlay(&self) -> Option<super::ke_chat::ClientKeChatOverlay> {
        let path = self.config.ke.chat_log_path()?;
        let entries = crate::ke::chat_log::read_tail(&path, 32).ok()?;
        super::ke_chat::overlay_from_log(&entries)
    }

    fn request_config_reload(&mut self, outcome: &mut ClientShellInput) {
        let _ = self.push_endpoint_method_with_kind(
            crate::api::schema::Method::ServerReloadConfig(
                crate::api::schema::EmptyParams::default(),
            ),
            PendingEndpointKind::ReloadConfig,
            outcome,
        );
    }

    pub(super) fn open_ke_chat_prompt(&mut self, outcome: &mut ClientShellInput) {
        self.composer_open_with("@ke ", outcome);
        outcome.resize = true;
    }
}

fn seed_markdown(path: &std::path::Path, default: &str) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("建目录失败：{err}"))?;
    }
    std::fs::write(path, default).map_err(|err| format!("写文件失败：{err}"))
}
