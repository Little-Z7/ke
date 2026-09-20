//! Added by ke: model providers behind `@ke` -- OpenAI-compatible Chat Completions over a curl
//! subprocess (`provider = "openai"`), or a headless coding-agent CLI over a plain subprocess
//! (`provider = "cli"`).
//!
//! The resident never takes an HTTP crate: it shells out to `curl -N` so SSE chunks can be
//! accumulated as they arrive. The CLI path shells out to `profile.command` instead, with the
//! flattened prompt on stdin. Both paths give callers one finished string (or an error to write
//! as assistant text). Nothing here panics on a bad provider.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::KeModelProfile;

/// One Chat Completions message. Roles are the usual `system` / `user` / `assistant`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub(crate) fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }

    pub(crate) fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }
}

#[derive(Serialize)]
struct CompletionRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    stream: bool,
    /// Ollama extension. Local qwen3 spends the whole timeout on `delta.reasoning`
    /// unless this is false; cloud OpenAI-compat servers must not see the field
    /// unless the profile explicitly sets `think`.
    #[serde(skip_serializing_if = "Option::is_none")]
    think: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

/// `think` on the wire: explicit on/off, else local defaults to false and cloud omits.
pub(crate) fn think_flag(profile: &KeModelProfile) -> Option<bool> {
    match profile.think.trim() {
        "on" | "true" => Some(true),
        "off" | "false" => Some(false),
        _ => profile.is_local().then_some(false),
    }
}

fn max_tokens_flag(profile: &KeModelProfile) -> Option<u32> {
    (profile.max_tokens > 0).then_some(profile.max_tokens)
}

/// `{base_url}/chat/completions` with trailing slashes on the base collapsed.
pub(crate) fn chat_completions_url(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    format!("{base}/chat/completions")
}

/// Pulls `choices[0].delta.content` from one SSE `data:` line.
///
/// `data: [DONE]` and lines that are not data events yield `None`.
pub(crate) fn parse_sse_delta(line: &str) -> Option<String> {
    let line = line.trim();
    let payload = line.strip_prefix("data:")?.trim();
    if payload.is_empty() || payload == "[DONE]" {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    match value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("delta"))
        .and_then(|delta| delta.get("content"))
    {
        Some(serde_json::Value::String(text)) if !text.is_empty() => Some(text.clone()),
        _ => None,
    }
}

/// Pulls `choices[0].message.content` from a non-streaming Chat Completions body.
pub(crate) fn content_from_completion_json(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json.trim()).ok()?;
    match value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
    {
        Some(serde_json::Value::String(text)) if !text.is_empty() => Some(text.clone()),
        _ => None,
    }
}

/// Dispatches on `profile.provider`. `resident_dir` is only used by `provider = "cli"` (it
/// becomes the child's working directory); the OpenAI-compatible path ignores it.
pub(crate) fn complete(
    profile: &KeModelProfile,
    messages: &[ChatMessage],
    resident_dir: &Path,
) -> Result<String, String> {
    match profile.provider.trim() {
        "" | "openai" => complete_openai(profile, messages),
        "cli" => complete_cli(profile, messages, resident_dir),
        other => Err(format!(
            "v1 只支持 OpenAI 兼容的 Chat Completions（provider=\"openai\"）或 headless CLI（provider=\"cli\"），当前 provider={other}"
        )),
    }
}

/// POST `{base_url}/chat/completions` with `stream: true`. Errors are strings for the caller.
fn complete_openai(profile: &KeModelProfile, messages: &[ChatMessage]) -> Result<String, String> {
    if profile.base_url.trim().is_empty() {
        return Err("模型 profile 没有 base_url".into());
    }
    if profile.model.trim().is_empty() {
        return Err("模型 profile 没有 model".into());
    }

    let url = chat_completions_url(&profile.base_url);
    let payload = serde_json::to_vec(&CompletionRequest {
        model: &profile.model,
        messages,
        stream: true,
        think: think_flag(profile),
        max_tokens: max_tokens_flag(profile),
    })
    .map_err(|err| format!("序列化请求失败：{err}"))?;

    let mut command = crate::noninteractive_process::curl_command();
    command.args([
        "-sS",
        "-N",
        "--connect-timeout",
        "5",
        "--max-time",
        "120",
        "-H",
        "Content-Type: application/json",
        "-H",
        "Accept: text/event-stream",
    ]);
    if let Some(header) = authorization_header(profile) {
        command.arg("-H").arg(header);
    }
    command
        .args(["--data-binary", "@-", "-w", "\nKE_HTTP:%{http_code}", &url])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|err| format!("curl 启动失败：{err}"))?;

    match child.stdin.take() {
        Some(mut stdin) => {
            if let Err(err) = stdin.write_all(&payload) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("写入 curl 请求失败：{err}"));
            }
        }
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("curl stdin 无法写入".into());
        }
    }

    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("curl stdout 无法读取".into());
        }
    };
    let stderr_pipe = child.stderr.take();

    let mut raw = String::new();
    let mut content = String::new();
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                raw.push_str(&line);
                if let Some(delta) = parse_sse_delta(line.trim_end()) {
                    content.push_str(&delta);
                }
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("读取模型输出失败：{err}"));
            }
        }
    }

    let wait_status = child
        .wait()
        .map_err(|err| format!("等待 curl 结束失败：{err}"))?;
    let mut stderr = String::new();
    if let Some(mut pipe) = stderr_pipe {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let (body, http_status) = split_curl_body_and_status(&raw);

    if let Some(code) = http_status {
        if !(200..300).contains(&code) {
            let detail = body.trim();
            let detail = if detail.is_empty() {
                stderr.trim().to_string()
            } else {
                detail.to_string()
            };
            return Err(if detail.is_empty() {
                format!("HTTP {code}")
            } else {
                format!("HTTP {code}：{detail}")
            });
        }
    }

    if !wait_status.success() && content.is_empty() {
        let stderr = stderr.trim();
        return Err(if stderr.is_empty() {
            format!("curl 失败（{wait_status}）")
        } else {
            format!("curl 失败：{stderr}")
        });
    }

    if !content.is_empty() {
        return Ok(content);
    }
    if let Some(text) = content_from_completion_json(&body) {
        return Ok(text);
    }
    Err("模型返回空内容".into())
}

/// Maximum time to let a `provider = "cli"` subprocess run before treating it as hung.
/// Headless coding-agent CLIs are noticeably slower than one HTTP round trip: a single simple
/// answer measured around 42s against `kimi` in manual testing. 300s stays generous while still
/// guaranteeing the resident thread does not block forever on a stuck child.
const CLI_TIMEOUT: Duration = Duration::from_secs(300);
/// Poll interval while waiting for the child to exit or to hit `CLI_TIMEOUT`.
const CLI_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Stderr is only used for diagnostics in error messages; keep excerpts bounded.
const CLI_STDERR_TAIL_CHARS: usize = 2000;

/// Runs `profile.command` as a headless coding-agent CLI: the flattened prompt goes on stdin,
/// the cleaned stdout is the answer.
fn complete_cli(
    profile: &KeModelProfile,
    messages: &[ChatMessage],
    resident_dir: &Path,
) -> Result<String, String> {
    complete_cli_with_timeout(profile, messages, resident_dir, CLI_TIMEOUT)
}

/// Same as [`complete_cli`] with an injectable timeout so tests do not have to wait out the
/// real `CLI_TIMEOUT`.
fn complete_cli_with_timeout(
    profile: &KeModelProfile,
    messages: &[ChatMessage],
    resident_dir: &Path,
    timeout: Duration,
) -> Result<String, String> {
    let mut argv = profile.command.iter();
    let program = match argv.next() {
        Some(program) => program,
        None => return Err("cli provider 没有配置 command".into()),
    };
    let args: Vec<&str> = argv.map(String::as_str).collect();
    let prompt = flatten_prompt(messages);

    // `resident_dir` (not the caller's cwd) so an agent CLI that reads project files like
    // AGENTS.md/CLAUDE.md out of its working directory picks up the resident's own directory
    // instead of whatever project the user happens to be sitting in.
    let mut command = crate::noninteractive_process::command(program);
    command
        .args(&args)
        .current_dir(resident_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Lets a hung or misbehaving CLI's whole process tree (not just the direct child) be killed
    // as a unit below via `kill_child_tree`. A CLI is often a shell wrapper or spawns its own
    // tool subprocesses; signaling only the direct child would leave those descendants running
    // with our stdout/stderr pipes still open, which then hangs the reader threads'
    // `read_to_string` well past the timeout instead of returning EOF.
    crate::platform::configure_killable_process_tree(&mut command);

    let mut child = command
        .spawn()
        .map_err(|err| format!("cli 启动失败：{err}"))?;

    // Must happen before stdin/stdout/stderr are used below: on Windows the child starts
    // suspended (see `configure_killable_process_tree`) and only actually begins running once
    // `attach` has bound it to a kill-on-close job, so no descendant can be forked before it is
    // covered by that job.
    let mut tree_guard = match crate::platform::ProcessTreeGuard::attach(&mut child) {
        Ok(guard) => guard,
        Err(err) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("cli 启动失败：{err}"));
        }
    };

    let mut stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            kill_child_tree(&mut child, &mut tree_guard);
            let _ = child.wait();
            return Err("cli stdin 无法写入".into());
        }
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            kill_child_tree(&mut child, &mut tree_guard);
            let _ = child.wait();
            return Err("cli stdout 无法读取".into());
        }
    };
    let stderr_pipe = match child.stderr.take() {
        Some(pipe) => pipe,
        None => {
            kill_child_tree(&mut child, &mut tree_guard);
            let _ = child.wait();
            return Err("cli stderr 无法读取".into());
        }
    };

    // Why this cannot deadlock: three helper threads own all blocking I/O, and the main
    // thread below only ever calls the non-blocking `try_wait`.
    //   - `writer_handle` writes the prompt then drops `stdin`, closing the pipe so the CLI
    //     sees EOF on stdin and can stop waiting for more input (some CLIs otherwise hang
    //     forever reading stdin).
    //   - `stdout_handle` / `stderr_handle` drain their pipes concurrently with the write and
    //     with each other, so a chatty child cannot fill an OS pipe buffer and block on a
    //     write while nobody is reading it yet.
    // Because the main thread never does a blocking read or a blocking `wait`, the timeout
    // loop below always gets to check the deadline. On timeout we call `kill_child_tree`, which
    // kills the whole process tree the child leads, not just the direct child; every process
    // holding our stdout/stderr pipes open dies, so the reader threads see EOF and return
    // promptly, and joining them afterward cannot hang either.
    let prompt_bytes = prompt.into_bytes();
    let writer_handle = std::thread::spawn(move || {
        let _ = stdin.write_all(&prompt_bytes);
        // `stdin` (the write half of the pipe) drops here, closing it.
    });
    let stdout_handle = std::thread::spawn(move || {
        let mut buffer = String::new();
        let mut stdout = stdout;
        let _ = stdout.read_to_string(&mut buffer);
        buffer
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut buffer = String::new();
        let mut pipe = stderr_pipe;
        let _ = pipe.read_to_string(&mut buffer);
        buffer
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    break None;
                }
                std::thread::sleep(CLI_POLL_INTERVAL);
            }
            Err(err) => {
                kill_child_tree(&mut child, &mut tree_guard);
                let _ = child.wait();
                let _ = writer_handle.join();
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                return Err(format!("等待 cli 结束失败：{err}"));
            }
        }
    };

    let status = match status {
        Some(status) => status,
        None => {
            // Timed out: kill the process tree and reap the direct child so no zombie is
            // left behind, then drain the reader threads -- the kill just closed every pipe
            // end in the tree, so they see EOF promptly.
            kill_child_tree(&mut child, &mut tree_guard);
            let _ = child.wait();
            let _ = writer_handle.join();
            let stderr_content = stderr_handle.join().unwrap_or_default();
            let _ = stdout_handle.join();
            let detail = tail_chars(stderr_content.trim(), CLI_STDERR_TAIL_CHARS);
            return Err(if detail.is_empty() {
                format!("cli 超时（超过 {}s 未退出，已终止）", timeout.as_secs())
            } else {
                format!(
                    "cli 超时（超过 {}s 未退出，已终止）：{detail}",
                    timeout.as_secs()
                )
            });
        }
    };

    let _ = writer_handle.join();
    let stdout_content = stdout_handle.join().unwrap_or_default();
    let stderr_content = stderr_handle.join().unwrap_or_default();

    let cleaned = strip_ansi(&stdout_content);
    let cleaned = cleaned.trim();

    if !status.success() && cleaned.is_empty() {
        let detail = tail_chars(stderr_content.trim(), CLI_STDERR_TAIL_CHARS);
        return Err(if detail.is_empty() {
            format!("cli 失败（{status}）")
        } else {
            format!("cli 失败（{status}）：{detail}")
        });
    }

    if cleaned.is_empty() {
        let detail = tail_chars(stderr_content.trim(), CLI_STDERR_TAIL_CHARS);
        return Err(if detail.is_empty() {
            "cli 返回空内容".into()
        } else {
            format!("cli 返回空内容：{detail}")
        });
    }

    Ok(cleaned.to_string())
}

/// Kills `child`'s whole process tree via `guard` (see `configure_killable_process_tree` at
/// spawn time and `ProcessTreeGuard` for how each platform tracks what "the tree" means). A
/// bare `child.kill()` only signals the direct child; a CLI that is itself a shell wrapper, or
/// that forks its own helper/tool subprocesses, can leave those descendants alive and holding
/// our stdout/stderr pipes open, which would keep the reader threads blocked in
/// `read_to_string` past whatever timeout the caller intended. Best-effort and infallible:
/// every caller already treats the outcome as "the process is gone" and follows up with
/// `child.wait()` to reap it.
fn kill_child_tree(child: &mut std::process::Child, guard: &mut crate::platform::ProcessTreeGuard) {
    guard.kill(child);
}

/// Last `limit` chars of `text`, used to keep stderr excerpts in error messages bounded.
/// Counts chars (not bytes) so it never splits a multi-byte character.
fn tail_chars(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        text.to_string()
    } else {
        text.chars().skip(count - limit).collect()
    }
}

/// Flattens a Chat-Completions-style message list into one prompt for a CLI that only accepts
/// free text on stdin.
///
/// All `system` messages (constitution, user prompt, pane snapshot) are kept verbatim and in
/// full -- a CLI provider must see exactly the same system context the OpenAI-compatible path
/// would have sent. Everything before the last `user` message is folded into a labeled
/// "conversation so far" transcript; the last `user` message is broken out into its own
/// clearly labeled section so the model cannot mistake it for history.
fn flatten_prompt(messages: &[ChatMessage]) -> String {
    let current_index = messages.iter().rposition(|message| message.role == "user");

    let mut system_parts = Vec::new();
    let mut history_parts = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        if Some(index) == current_index {
            continue;
        }
        match message.role.as_str() {
            "system" => system_parts.push(message.content.as_str()),
            "assistant" => history_parts.push(format!("Assistant: {}", message.content)),
            "user" => history_parts.push(format!("User: {}", message.content)),
            other => history_parts.push(format!("{other}: {}", message.content)),
        }
    }

    let mut sections = Vec::new();
    if !system_parts.is_empty() {
        sections.push(system_parts.join("\n\n"));
    }
    if !history_parts.is_empty() {
        sections.push(format!(
            "Conversation so far:\n{}",
            history_parts.join("\n\n")
        ));
    }
    let current = current_index
        .map(|index| messages[index].content.as_str())
        .unwrap_or("");
    sections.push(format!("Current question (answer only this):\n{current}"));

    sections.join("\n\n---\n\n")
}

/// Strips ANSI escape sequences from CLI output: CSI sequences (`ESC [ ... final-byte`, e.g.
/// SGR colors `\x1b[31m` and cursor movement `\x1b[2;5H`), OSC sequences (`ESC ] ... BEL` or
/// `ESC ] ... ESC \`, e.g. terminal hyperlinks/titles), and other two-byte `ESC x` forms (e.g.
/// charset selection). Plain text is left untouched.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                loop {
                    match chars.next() {
                        Some('\u{7}') | None => break,
                        Some('\u{1b}') => {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                        Some(_) => {}
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

fn authorization_header(profile: &KeModelProfile) -> Option<String> {
    let name = profile.api_key_env.trim();
    if name.is_empty() {
        return None;
    }
    let key = std::env::var(name).ok()?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    Some(format!("Authorization: Bearer {key}"))
}

fn split_curl_body_and_status(stdout: &str) -> (String, Option<u16>) {
    match stdout.rfind("KE_HTTP:") {
        Some(index) => {
            let body = stdout[..index].trim_end_matches(['\r', '\n']).to_string();
            let status = stdout[index + "KE_HTTP:".len()..].trim().parse().ok();
            (body, status)
        }
        None => (stdout.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_completions_url_normalizes_slashes() {
        assert_eq!(
            chat_completions_url("http://localhost:11434/v1"),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("http://localhost:11434/v1/"),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("  https://example.com/v1///  "),
            "https://example.com/v1/chat/completions"
        );
    }

    #[test]
    fn parse_sse_delta_reads_content_and_ignores_done() {
        let chunk = r#"data: {"choices":[{"delta":{"content":"你好"}}]}"#;
        assert_eq!(parse_sse_delta(chunk).as_deref(), Some("你好"));
        assert_eq!(
            parse_sse_delta(r#"data:{"choices":[{"delta":{"content":"世界"}}]}"#).as_deref(),
            Some("世界")
        );
        assert_eq!(parse_sse_delta("data: [DONE]"), None);
        assert_eq!(parse_sse_delta("data:[DONE]"), None);
        assert_eq!(
            parse_sse_delta(r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#),
            None,
            "role-only delta is not content"
        );
        assert_eq!(parse_sse_delta(": keep-alive"), None);
        assert_eq!(parse_sse_delta("event: message"), None);
        assert_eq!(parse_sse_delta(""), None);
    }

    #[test]
    fn content_from_completion_json_reads_message() {
        let json = r#"{
            "choices": [
                {"message": {"role": "assistant", "content": "窗格 2 空着"}}
            ]
        }"#;
        assert_eq!(
            content_from_completion_json(json).as_deref(),
            Some("窗格 2 空着")
        );
        assert_eq!(content_from_completion_json("not json"), None);
        assert_eq!(
            content_from_completion_json(r#"{"choices":[{"message":{"content":""}}]}"#),
            None
        );
        assert_eq!(content_from_completion_json("{}"), None);
    }

    #[test]
    fn local_requests_disable_ollama_thinking() {
        let local = KeModelProfile {
            provider: "openai".into(),
            base_url: "http://localhost:11434/v1".into(),
            model: "qwen3:4b".into(),
            ..KeModelProfile::default()
        };
        let local_json = serde_json::to_value(&CompletionRequest {
            model: "qwen3:4b",
            messages: &[],
            stream: true,
            think: think_flag(&local),
            max_tokens: None,
        })
        .unwrap();
        assert_eq!(local_json["think"], false);

        let cloud = KeModelProfile {
            provider: "openai".into(),
            base_url: "https://ark.example/api/v3".into(),
            model: "doubao".into(),
            think: "on".into(),
            max_tokens: 2048,
            ..KeModelProfile::default()
        };
        let cloud_json = serde_json::to_value(&CompletionRequest {
            model: "doubao",
            messages: &[],
            stream: true,
            think: think_flag(&cloud),
            max_tokens: (cloud.max_tokens > 0).then_some(cloud.max_tokens),
        })
        .unwrap();
        assert_eq!(cloud_json["think"], true);
        assert_eq!(cloud_json["max_tokens"], 2048);
    }

    #[test]
    fn split_curl_body_and_status_takes_the_trailer() {
        let (body, status) = split_curl_body_and_status("data: [DONE]\nKE_HTTP:200");
        assert_eq!(body, "data: [DONE]");
        assert_eq!(status, Some(200));
        let (body, status) = split_curl_body_and_status("oops");
        assert_eq!(body, "oops");
        assert_eq!(status, None);
    }

    #[test]
    fn flatten_prompt_preserves_system_and_marks_current_question() {
        let messages = vec![
            ChatMessage::system("你是 ke 的管家。"),
            ChatMessage::system("当前会话窗格快照：\npane_id=w1:p1 agent=codex"),
            ChatMessage {
                role: "user".into(),
                content: "旧问题".into(),
            },
            ChatMessage {
                role: "assistant".into(),
                content: "旧回答".into(),
            },
            ChatMessage::user("谁在忙？"),
        ];

        let flat = flatten_prompt(&messages);

        assert!(flat.contains("你是 ke 的管家。"), "{flat}");
        assert!(flat.contains("pane_id=w1:p1 agent=codex"), "{flat}");
        assert!(flat.contains("User: 旧问题"), "{flat}");
        assert!(flat.contains("Assistant: 旧回答"), "{flat}");
        assert!(
            flat.contains("Current question (answer only this):\n谁在忙？"),
            "{flat}"
        );
        // The current question is broken out once, not duplicated into the history transcript.
        assert_eq!(flat.matches("谁在忙？").count(), 1);
        // System context must precede the current-question section.
        let system_pos = flat.find("你是 ke 的管家。").unwrap();
        let current_pos = flat.find("Current question").unwrap();
        assert!(system_pos < current_pos);
    }

    #[test]
    fn flatten_prompt_handles_system_only_input() {
        let messages = vec![ChatMessage::system("only system context")];
        let flat = flatten_prompt(&messages);
        assert!(flat.contains("only system context"));
        assert!(flat.contains("Current question (answer only this):\n"));
        assert!(!flat.contains("Conversation so far"));
    }

    #[test]
    fn strip_ansi_removes_csi_sequences_and_keeps_text() {
        assert_eq!(strip_ansi("\x1b[31mhello\x1b[0m"), "hello");
        assert_eq!(strip_ansi("\x1b[2;5Hworld"), "world");
        assert_eq!(
            strip_ansi("plain text, no escapes"),
            "plain text, no escapes"
        );
        assert_eq!(
            strip_ansi("normal \x1b[1mbold\x1b[0m normal"),
            "normal bold normal"
        );
    }

    #[test]
    fn strip_ansi_removes_osc_sequences() {
        // OSC 8 hyperlink, BEL terminated.
        assert_eq!(
            strip_ansi("\x1b]8;;http://example.com\x07link\x1b]8;;\x07"),
            "link"
        );
        // OSC terminated with ESC \ (ST) instead of BEL.
        assert_eq!(strip_ansi("\x1b]0;title\x1b\\rest"), "rest");
    }

    #[test]
    fn tail_chars_keeps_only_the_last_chars() {
        assert_eq!(tail_chars("hello", 10), "hello");
        assert_eq!(tail_chars("hello world", 5), "world");
        assert_eq!(tail_chars("你好世界", 2), "世界");
    }

    #[test]
    fn complete_rejects_unknown_provider() {
        let profile = KeModelProfile {
            provider: "unknown-thing".into(),
            ..KeModelProfile::default()
        };
        let err = complete(&profile, &[], Path::new(".")).unwrap_err();
        assert!(err.contains("cli"), "{err}");
        assert!(err.contains("unknown-thing"), "{err}");
    }

    #[cfg(unix)]
    fn cli_profile(command: Vec<&str>) -> KeModelProfile {
        KeModelProfile {
            provider: "cli".into(),
            command: command.into_iter().map(String::from).collect(),
            ..KeModelProfile::default()
        }
    }

    #[cfg(unix)]
    #[test]
    fn cat_echoes_the_flattened_prompt_back() {
        let messages = vec![
            ChatMessage::system("system ctx"),
            ChatMessage::user("hello?"),
        ];
        let expected = flatten_prompt(&messages);
        let result = complete_cli(&cli_profile(vec!["cat"]), &messages, &std::env::temp_dir())
            .expect("cat should echo stdin");
        assert_eq!(result, expected.trim());
    }

    #[cfg(unix)]
    #[test]
    fn cli_output_is_ansi_stripped_and_trimmed() {
        let messages = vec![ChatMessage::user("hi")];
        let script = "printf '\\n\\033[31mhello\\033[0m\\n\\n'";
        let result = complete_cli(
            &cli_profile(vec!["sh", "-c", script]),
            &messages,
            &std::env::temp_dir(),
        )
        .expect("colored output should be cleaned");
        assert_eq!(result, "hello");
    }

    #[cfg(unix)]
    #[test]
    fn cli_empty_output_is_an_error() {
        let messages = vec![ChatMessage::user("hi")];
        let err = complete_cli(
            &cli_profile(vec!["sh", "-c", "true"]),
            &messages,
            &std::env::temp_dir(),
        )
        .unwrap_err();
        assert!(err.contains("空"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn cli_nonzero_exit_with_empty_stdout_is_an_error() {
        let messages = vec![ChatMessage::user("hi")];
        let err = complete_cli(
            &cli_profile(vec!["sh", "-c", "echo boom >&2; exit 3"]),
            &messages,
            &std::env::temp_dir(),
        )
        .unwrap_err();
        assert!(err.contains("boom"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn cli_nonzero_exit_with_stdout_content_still_returns_that_content() {
        let messages = vec![ChatMessage::user("hi")];
        let result = complete_cli(
            &cli_profile(vec!["sh", "-c", "echo ok; exit 2"]),
            &messages,
            &std::env::temp_dir(),
        )
        .expect("stdout content wins over a nonzero exit code");
        assert_eq!(result, "ok");
    }

    #[cfg(unix)]
    #[test]
    fn cli_timeout_kills_the_child_and_reports_an_error() {
        let messages = vec![ChatMessage::user("hi")];
        let started = Instant::now();
        let err = complete_cli_with_timeout(
            &cli_profile(vec!["sh", "-c", "sleep 30"]),
            &messages,
            &std::env::temp_dir(),
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert!(err.contains("超时"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timeout should cut the wait short, took {:?}",
            started.elapsed()
        );
    }

    #[cfg(unix)]
    #[test]
    fn cli_missing_command_is_a_clear_error_not_a_panic() {
        let messages = vec![ChatMessage::user("hi")];
        let err = complete_cli(&cli_profile(vec![]), &messages, &std::env::temp_dir()).unwrap_err();
        assert!(err.contains("command"), "{err}");
    }
}
