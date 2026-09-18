//! Added by ke: rule-based redaction gate.
//!
//! This is the deterministic half of the composer processor. It never involves a model: text that
//! matches a rule is replaced before it reaches a pane or leaves the machine, and the caller learns
//! how many replacements happened so the status row can say so. Rules are toggled from
//! `[ke.redact]`; the patterns themselves are fixed here so a broken config cannot turn the gate off
//! by accident.

use std::sync::OnceLock;

use regex::Regex;

pub(crate) const REDACTED: &str = "[REDACTED]";

/// Which rule families are active. Mirrors `config::KeRedactConfig` but lives here so the gate can
/// be unit-tested without a `Config`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RedactRules {
    pub api_keys: bool,
    pub private_keys: bool,
    pub env_secrets: bool,
    pub internal_ips: bool,
    pub emails: bool,
}

impl Default for RedactRules {
    fn default() -> Self {
        Self {
            api_keys: true,
            private_keys: true,
            env_secrets: true,
            internal_ips: false,
            emails: false,
        }
    }
}

impl RedactRules {
    #[cfg(test)]
    pub(crate) const NONE: Self = Self {
        api_keys: false,
        private_keys: false,
        env_secrets: false,
        internal_ips: false,
        emails: false,
    };

    pub(crate) fn any(self) -> bool {
        self.api_keys || self.private_keys || self.env_secrets || self.internal_ips || self.emails
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Redaction {
    pub text: String,
    /// Number of replacements, summed over all rules.
    pub count: usize,
}

struct Patterns {
    api_keys: Vec<Regex>,
    private_key_block: Regex,
    env_secret: Regex,
    internal_ip: Regex,
    email: Regex,
}

fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        // Each pattern is a literal here, so a compile failure is a programming error caught by
        // `every_pattern_compiles`, never a runtime condition.
        let compile = |source: &str| {
            Regex::new(source).unwrap_or_else(|err| panic!("invalid redaction pattern {source}: {err}"))
        };
        Patterns {
            api_keys: [
                // OpenAI-style and most "sk-" families (Anthropic, Stripe live/test, DeepSeek...).
                r"\bsk-[A-Za-z0-9_-]{8,}\b",
                // AWS access key ids.
                r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b",
                // GitHub tokens.
                r"\bgh[pousr]_[A-Za-z0-9]{20,}\b",
                r"\bgithub_pat_[A-Za-z0-9_]{20,}\b",
                // Slack tokens.
                r"\bxox[abprs]-[A-Za-z0-9-]{10,}\b",
                // Google API keys.
                r"\bAIza[0-9A-Za-z_-]{30,}\b",
                // Volcengine / Ark style keys and generic long bearer values.
                r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{20,}",
                // JWTs.
                r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b",
            ]
            .into_iter()
            .map(compile)
            .collect(),
            private_key_block: compile(
                r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
            ),
            // KEY=value where the key name says it is a secret. Keeps the key name, hides the value.
            env_secret: compile(
                r#"(?im)\b([A-Z0-9_]*(?:SECRET|TOKEN|PASSWORD|PASSWD|API_KEY|APIKEY|PRIVATE_KEY|ACCESS_KEY)[A-Z0-9_]*)\s*=\s*["']?([^\s"']+)["']?"#,
            ),
            internal_ip: compile(
                r"\b(?:10\.\d{1,3}\.\d{1,3}\.\d{1,3}|192\.168\.\d{1,3}\.\d{1,3}|172\.(?:1[6-9]|2\d|3[01])\.\d{1,3}\.\d{1,3})\b",
            ),
            email: compile(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b"),
        }
    })
}

/// Applies the enabled rules and reports how many replacements were made.
pub(crate) fn redact(text: &str, rules: RedactRules) -> Redaction {
    if !rules.any() || text.is_empty() {
        return Redaction {
            text: text.to_owned(),
            count: 0,
        };
    }
    let patterns = patterns();
    let mut out = text.to_owned();
    let mut count = 0usize;
    let mut apply = |regex: &Regex, replacement: &dyn Fn(&regex::Captures<'_>) -> String| {
        let mut hits = 0usize;
        let replaced = match regex.replace_all(&out, |caps: &regex::Captures<'_>| {
            hits += 1;
            replacement(caps)
        }) {
            std::borrow::Cow::Owned(replaced) => Some(replaced),
            std::borrow::Cow::Borrowed(_) => None,
        };
        if let Some(replaced) = replaced {
            out = replaced;
            count += hits;
        }
    };
    // Private key blocks first: they contain base64 that other rules would shred into pieces.
    if rules.private_keys {
        apply(&patterns.private_key_block, &|_| REDACTED.to_owned());
    }
    if rules.env_secrets {
        apply(&patterns.env_secret, &|caps| {
            format!("{}={REDACTED}", &caps[1])
        });
    }
    if rules.api_keys {
        for regex in &patterns.api_keys {
            apply(regex, &|caps| {
                let whole = &caps[0];
                // Keep the "bearer " prefix so the shape of the text stays readable.
                match whole
                    .get(..7)
                    .filter(|prefix| prefix.eq_ignore_ascii_case("bearer "))
                {
                    Some(prefix) => format!("{prefix}{REDACTED}"),
                    None => REDACTED.to_owned(),
                }
            });
        }
    }
    if rules.internal_ips {
        apply(&patterns.internal_ip, &|_| REDACTED.to_owned());
    }
    if rules.emails {
        apply(&patterns.email, &|_| REDACTED.to_owned());
    }
    Redaction { text: out, count }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_compiles() {
        let _ = patterns();
    }

    #[test]
    fn default_rules_hide_keys_blocks_and_env_values() {
        let text = "deploy with sk-live-abcdef123456 and AKIAABCDEFGHIJKLMNOP\n\
                    export OPENAI_API_KEY=\"sk-proj-zzzzzzzzzzzz\"\n\
                    DB_PASSWORD=hunter2\n\
                    -----BEGIN RSA PRIVATE KEY-----\nMIIEow==\n-----END RSA PRIVATE KEY-----\n\
                    contact ops@example.com at 10.0.0.7";
        let out = redact(text, RedactRules::default());
        assert!(!out.text.contains("abcdef123456"));
        assert!(!out.text.contains("AKIAABCDEFGHIJKLMNOP"));
        assert!(!out.text.contains("hunter2"));
        assert!(!out.text.contains("MIIEow=="));
        assert!(
            out.text.contains("OPENAI_API_KEY=[REDACTED]"),
            "{}",
            out.text
        );
        assert!(out.text.contains("DB_PASSWORD=[REDACTED]"), "{}", out.text);
        assert!(
            out.text.contains("ops@example.com") && out.text.contains("10.0.0.7"),
            "emails and internal ips stay by default: {}",
            out.text
        );
        assert_eq!(out.count, 5, "{}", out.text);
    }

    #[test]
    fn optional_rules_are_opt_in() {
        let rules = RedactRules {
            internal_ips: true,
            emails: true,
            ..RedactRules::default()
        };
        let out = redact("mail ops@example.com from 192.168.1.20", rules);
        assert_eq!(out.text, "mail [REDACTED] from [REDACTED]");
        assert_eq!(out.count, 2);
    }

    #[test]
    fn plain_text_passes_untouched() {
        let text = "echo hello && ls -la ~/projects/skate-park";
        let out = redact(text, RedactRules::default());
        assert_eq!(out.text, text);
        assert_eq!(out.count, 0);
        assert_eq!(
            redact("sk-live-abc", RedactRules::NONE).count,
            0,
            "all rules off"
        );
    }

    #[test]
    fn bearer_prefix_is_kept_and_jwts_are_hidden() {
        let out = redact(
            "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
            RedactRules::default(),
        );
        assert!(
            out.text.starts_with("Authorization: Bearer [REDACTED]"),
            "{}",
            out.text
        );
        assert!(!out.text.contains("eyJ"));
    }
}
