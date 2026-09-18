//! Added by ke: OpenAI-compatible Chat Completions over a curl subprocess.
//!
//! The resident never takes an HTTP crate: it shells out to `curl -N` so SSE chunks can be
//! accumulated as they arrive. Callers always get one finished string (or an error to write as
//! assistant text). Nothing here panics on a bad provider.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::Stdio;

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

/// POST `{base_url}/chat/completions` with `stream: true`. Errors are strings for the caller.
pub(crate) fn complete(
    profile: &KeModelProfile,
    messages: &[ChatMessage],
) -> Result<String, String> {
    let provider = profile.provider.trim();
    if !provider.is_empty() && provider != "openai" {
        return Err(format!(
            "v1 只支持 OpenAI 兼容的 Chat Completions，当前 provider={provider}"
        ));
    }
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
}
