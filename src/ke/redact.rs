//! Added by ke: rule-based redaction gate.
//!
//! This is the deterministic half of the composer processor. It never involves a model: text that
//! matches a rule is replaced before it reaches a pane or leaves the machine, and the caller learns
//! how many replacements happened so the status row can say so. Rules are toggled from
//! `[ke.redact]`; the patterns themselves are fixed here so a broken config cannot turn the gate off
//! by accident.

use std::collections::HashMap;
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

/// Which family a masked value belongs to. Determines the placeholder prefix `mask` assigns it
/// and keeps the per-family counters in [`Mapping`] independent, so hiding an email never skips
/// or reuses a number that belongs to a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PlaceholderKind {
    /// `api_keys`, `private_keys`, and `env_secrets` all share this family: they are all
    /// credentials, and a caller reversing a mask does not need to know which rule caught one.
    Secret,
    Ip,
    Email,
}

impl PlaceholderKind {
    fn prefix(self) -> &'static str {
        match self {
            PlaceholderKind::Secret => "KE_SECRET",
            PlaceholderKind::Ip => "KE_IP",
            PlaceholderKind::Email => "KE_EMAIL",
        }
    }

    /// The inverse of [`prefix`](Self::prefix): the family whose placeholder prefix is exactly
    /// `prefix`, or `None` for anything else. Used to interpret a prefix recovered by scanning
    /// old text (see `max_placeholder_numbers`), never to mint a placeholder itself.
    fn from_prefix(prefix: &str) -> Option<Self> {
        match prefix {
            "KE_SECRET" => Some(PlaceholderKind::Secret),
            "KE_IP" => Some(PlaceholderKind::Ip),
            "KE_EMAIL" => Some(PlaceholderKind::Email),
            _ => None,
        }
    }
}

fn placeholder_scan_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"\bKE_(SECRET|IP|EMAIL)_(\d+)")
            .unwrap_or_else(|err| panic!("invalid placeholder-scan pattern: {err}"))
    })
}

/// The highest placeholder number already present in `text`, per family, keyed by that family's
/// prefix (`"KE_SECRET"`, `"KE_IP"`, `"KE_EMAIL"`; see [`PlaceholderKind::prefix`]). A family with
/// no match in `text` is absent from the result.
///
/// This reads only placeholder *shapes* -- a fixed, non-secret prefix followed by digits -- never
/// `map` and never a real value. It exists so [`Mapping::reserve_from_text`] can find out, from
/// old chat history, which numbers a previous `Mapping` (cleared by a resident restart) already
/// handed out, so a fresh `Mapping` never reissues one of them for a different real value.
fn max_placeholder_numbers(text: &str) -> HashMap<&'static str, usize> {
    let mut out: HashMap<&'static str, usize> = HashMap::new();
    for caps in placeholder_scan_pattern().captures_iter(text) {
        let family = match &caps[1] {
            "SECRET" => "KE_SECRET",
            "IP" => "KE_IP",
            "EMAIL" => "KE_EMAIL",
            _ => continue,
        };
        let Ok(number) = caps[2].parse::<usize>() else {
            continue;
        };
        let slot = out.entry(family).or_insert(0);
        if number > *slot {
            *slot = number;
        }
    }
    out
}

/// Hard cap on the number of distinct real values a single [`Mapping`] will remember. Ordinary
/// sessions never get near this; it exists so pathological input (a runaway script that emits
/// endless unique secret-shaped text, say) cannot grow the mapping without bound for the lifetime
/// of a resident process. Once the cap is reached, values that have not been seen before are
/// still hidden -- fail closed, never plaintext -- but as the irreversible [`REDACTED`] constant
/// instead of a new placeholder, since there is no budget left to remember how to restore them.
/// See `mapping_fails_closed_once_capacity_is_reached` below.
pub(crate) const MAX_MAPPING_ENTRIES: usize = 10_000;

/// A bidirectional mapping between real sensitive values and the stable placeholder tokens
/// [`mask`] substitutes for them, plus the per-family counters used to mint new placeholders.
///
/// This lives only in process memory for the lifetime the caller keeps it around. It is never
/// serialized, written to disk, or given a `serde` impl: doing either would put the exact secrets
/// this type exists to keep off the filesystem back onto it.
#[derive(Debug, Default)]
pub(crate) struct Mapping {
    real_to_placeholder: HashMap<String, String>,
    placeholder_to_real: HashMap<String, String>,
    next_index: HashMap<PlaceholderKind, usize>,
}

impl Mapping {
    /// Raises the starting counter for the placeholder family whose prefix is `prefix` (matching
    /// [`PlaceholderKind::prefix`], e.g. `"KE_SECRET"`) so the next placeholder minted for that
    /// family is strictly greater than `at_least`. Only ever moves the counter forward: a smaller
    /// `at_least` than what is already reserved (or already used) is a no-op, and so is an
    /// unrecognized `prefix`. Never records any real value -- this only changes where numbering
    /// resumes.
    pub(crate) fn reserve_through(&mut self, prefix: &str, at_least: usize) {
        let Some(kind) = PlaceholderKind::from_prefix(prefix) else {
            return;
        };
        let counter = self.next_index.entry(kind).or_insert(0);
        if *counter < at_least {
            *counter = at_least;
        }
    }

    /// Scans `text` for placeholder tokens [`mask`] could have minted (see
    /// `max_placeholder_numbers`) and reserves through the highest number found in each family.
    ///
    /// This is what lets a resident starting up feed its fresh `Mapping` the session's
    /// `chat.jsonl` so a restart never causes two different real values to share one placeholder:
    /// `Mapping` only lives in process memory (see its own doc comment) and a restart clears it,
    /// but the chat log is not cleared, so it can still contain a placeholder from before the
    /// restart -- most often echoed back verbatim in a past model reply, since a model only ever
    /// sees masked text. Left alone, a fresh `Mapping` would start counting from 1 again and could
    /// hand that same placeholder string to a completely different real value, and the model would
    /// have no way to tell the two apart. After this call, that old placeholder is instead an
    /// orphan -- this `Mapping` has no record of what it stood for, so [`restore`] leaves it alone
    /// -- rather than a number that gets reused.
    pub(crate) fn reserve_from_text(&mut self, text: &str) {
        for (prefix, max_number) in max_placeholder_numbers(text) {
            self.reserve_through(prefix, max_number);
        }
    }

    /// Returns the placeholder for `real`, reusing the one already assigned if this exact value
    /// has been seen before (in this call or an earlier one), or minting the next number in
    /// `kind`'s sequence otherwise. Once [`MAX_MAPPING_ENTRIES`] distinct values have been
    /// recorded, a value that has never been seen before gets [`REDACTED`] instead of a fresh
    /// placeholder -- still hidden, just not reversible; the mapping itself is never grown past
    /// the cap.
    fn placeholder_for(&mut self, kind: PlaceholderKind, real: &str) -> String {
        if let Some(existing) = self.real_to_placeholder.get(real) {
            return existing.clone();
        }
        if self.real_to_placeholder.len() >= MAX_MAPPING_ENTRIES {
            return REDACTED.to_owned();
        }
        let counter = self.next_index.entry(kind).or_insert(0);
        *counter += 1;
        let index = *counter;
        let prefix = kind.prefix();
        let placeholder = format!("{prefix}_{index}");
        self.real_to_placeholder
            .insert(real.to_owned(), placeholder.clone());
        self.placeholder_to_real
            .insert(placeholder.clone(), real.to_owned());
        placeholder
    }
}

/// The byte length of the complete placeholder at the *start* of `value`, or `None` if `value`
/// does not begin with one (one of the three placeholder prefixes followed by one or more ASCII
/// digits). Digits are consumed greedily, so `KE_SECRET_12` reads as placeholder number 12, not
/// number 1 followed by a literal `2`.
///
/// `mask` uses this to recognize its own prior output by shape rather than by checking whether
/// `map` already contains it. A `map`-based check would only catch a placeholder minted by this
/// exact `Mapping` instance; text that was masked in an earlier process or session (chat history
/// replayed through the redaction gate again, for example) would look like ordinary sensitive
/// input to a fresh `Mapping` and get wrapped in a second layer of placeholders. The placeholder
/// format is distinctive enough that real sensitive data colliding with it is negligible.
///
/// A *prefix* match, not a whole-value match, is what call sites act on: a greedily captured
/// value can carry trailing text that belongs to the surrounding sentence rather than the
/// placeholder, e.g. `env_secret`'s value group turns `TOKEN=KE_SECRET_1; echo ok` into the value
/// `KE_SECRET_1;` (semicolon included, since it isn't whitespace). Treating that as "not a
/// placeholder" because of the trailing `;` would mask it again and swallow the semicolon.
/// Whether a value that starts with a placeholder but is followed by something other than digits
/// (`KE_SECRET_1abc`) is itself skipped is a deliberate tradeoff: a real secret that happens to
/// begin with our own placeholder format is negligible, while corrupting already-masked text on
/// every replay is not.
fn placeholder_prefix_len(value: &str) -> Option<usize> {
    const PREFIXES: [&str; 3] = ["KE_SECRET_", "KE_IP_", "KE_EMAIL_"];
    PREFIXES.iter().find_map(|prefix| {
        let rest = value.strip_prefix(prefix)?;
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        (digits > 0).then_some(prefix.len() + digits)
    })
}

/// Like [`redact`], but reversibly: every match is replaced with a stable, typed placeholder
/// token (`KE_SECRET_1`, `KE_EMAIL_1`, `KE_IP_1`, ...) instead of the constant [`REDACTED`]
/// string, and the real value is recorded in `map` so [`restore`] can put it back later.
///
/// The same real value always receives the same placeholder, both within one call and across
/// repeated calls that share the same `map`; two different values never share a placeholder. A
/// value that starts with a placeholder (see [`placeholder_prefix_len`]) is left untouched instead
/// of being masked again, so `mask` is idempotent: running it a second time over text it already
/// produced — including text where a placeholder is glued to trailing punctuation or prose picked
/// up by a greedy capture group — is a no-op and does not inflate `map`'s counters.
/// `Redaction.count` has the same meaning as in `redact`: the number of replacements made, not
/// counting matches that were skipped because they were already placeholders.
pub(crate) fn mask(text: &str, rules: RedactRules, map: &mut Mapping) -> Redaction {
    if !rules.any() || text.is_empty() {
        return Redaction {
            text: text.to_owned(),
            count: 0,
        };
    }
    let patterns = patterns();
    let mut out = text.to_owned();
    let mut count = 0usize;
    let mut apply = |regex: &Regex, replacement: &mut dyn FnMut(&regex::Captures<'_>) -> String| {
        let mut hits = 0usize;
        let replaced = match regex.replace_all(&out, |caps: &regex::Captures<'_>| {
            // Compare against the whole match rather than incrementing unconditionally: a
            // replacement closure that recognized an already-masked placeholder and returned the
            // match unchanged did not actually redact anything, and must not count as a hit.
            let original = &caps[0];
            let result = replacement(caps);
            if result != original {
                hits += 1;
            }
            result
        }) {
            std::borrow::Cow::Owned(replaced) => Some(replaced),
            std::borrow::Cow::Borrowed(_) => None,
        };
        if let Some(replaced) = replaced {
            out = replaced;
            count += hits;
        }
    };
    // Same rule order as `redact`: private key blocks first so other rules cannot shred their
    // base64 body, then env secrets before bare api keys for the same reason.
    if rules.private_keys {
        apply(&patterns.private_key_block, &mut |caps| {
            let whole = &caps[0];
            if placeholder_prefix_len(whole).is_some() {
                return whole.to_owned();
            }
            map.placeholder_for(PlaceholderKind::Secret, whole)
        });
    }
    if rules.env_secrets {
        apply(&patterns.env_secret, &mut |caps| {
            let value = &caps[2];
            if placeholder_prefix_len(value).is_some() {
                return caps[0].to_owned();
            }
            let placeholder = map.placeholder_for(PlaceholderKind::Secret, value);
            format!("{}={placeholder}", &caps[1])
        });
    }
    if rules.api_keys {
        for regex in &patterns.api_keys {
            apply(regex, &mut |caps| {
                let whole = &caps[0];
                // Keep the "bearer " prefix so the shape of the text stays readable, same as
                // `redact`; only the token after it is a value worth mapping.
                match whole
                    .get(..7)
                    .filter(|prefix| prefix.eq_ignore_ascii_case("bearer "))
                {
                    Some(prefix) => {
                        let value = &whole[7..];
                        if placeholder_prefix_len(value).is_some() {
                            return whole.to_owned();
                        }
                        let placeholder = map.placeholder_for(PlaceholderKind::Secret, value);
                        format!("{prefix}{placeholder}")
                    }
                    None => {
                        if placeholder_prefix_len(whole).is_some() {
                            return whole.to_owned();
                        }
                        map.placeholder_for(PlaceholderKind::Secret, whole)
                    }
                }
            });
        }
    }
    if rules.internal_ips {
        apply(&patterns.internal_ip, &mut |caps| {
            let whole = &caps[0];
            if placeholder_prefix_len(whole).is_some() {
                return whole.to_owned();
            }
            map.placeholder_for(PlaceholderKind::Ip, whole)
        });
    }
    if rules.emails {
        apply(&patterns.email, &mut |caps| {
            let whole = &caps[0];
            if placeholder_prefix_len(whole).is_some() {
                return whole.to_owned();
            }
            map.placeholder_for(PlaceholderKind::Email, whole)
        });
    }
    Redaction { text: out, count }
}

/// Reverses [`mask`]: every placeholder token present in `text` is replaced with the real value
/// recorded in `map`. Text that contains no placeholders (including when `map` is empty) is
/// returned unchanged.
// Still has no caller: nothing reads a model's answer back through the resident and substitutes
// real values into it yet. Drop this allow once the sub entry point that round-trips a model
// reply (which may echo a placeholder verbatim) back onto a pane lands and calls this.
#[allow(dead_code)]
pub(crate) fn restore(text: &str, map: &Mapping) -> String {
    if map.placeholder_to_real.is_empty() || text.is_empty() {
        return text.to_owned();
    }
    // Longest placeholder first: `KE_SECRET_10` must be tried before `KE_SECRET_1` wherever both
    // could match the same starting position, otherwise the shorter one would be substituted and
    // leave a stray `0` behind in the output.
    let mut placeholders: Vec<&str> = map.placeholder_to_real.keys().map(String::as_str).collect();
    placeholders.sort_unstable_by_key(|placeholder| std::cmp::Reverse(placeholder.len()));

    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let matched = placeholders
            .iter()
            .find(|placeholder| rest.starts_with(**placeholder));
        match matched {
            Some(placeholder) => {
                if let Some(real) = map.placeholder_to_real.get(*placeholder) {
                    out.push_str(real);
                }
                rest = &rest[placeholder.len()..];
            }
            None => {
                let mut chars = rest.chars();
                if let Some(c) = chars.next() {
                    out.push(c);
                }
                rest = chars.as_str();
            }
        }
    }
    out
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

    #[test]
    fn mask_reuses_the_same_placeholder_for_a_repeated_value() {
        let rules = RedactRules {
            emails: true,
            ..RedactRules::default()
        };
        let mut map = Mapping::default();
        let out = mask(
            "primary ops@example.com, backup ops@example.com",
            rules,
            &mut map,
        );
        assert_eq!(out.text, "primary KE_EMAIL_1, backup KE_EMAIL_1");
        assert_eq!(out.count, 2, "{}", out.text);
    }

    #[test]
    fn mask_keeps_the_same_placeholder_across_separate_calls() {
        let rules = RedactRules {
            emails: true,
            ..RedactRules::default()
        };
        let mut map = Mapping::default();
        let first = mask("contact ops@example.com", rules, &mut map);
        assert_eq!(first.text, "contact KE_EMAIL_1");

        let second = mask("again ops@example.com and new@example.com", rules, &mut map);
        assert_eq!(
            second.text, "again KE_EMAIL_1 and KE_EMAIL_2",
            "the first mask call must not renumber the value it already assigned"
        );
    }

    #[test]
    fn mask_assigns_increasing_numbers_to_distinct_values() {
        let rules = RedactRules {
            emails: true,
            ..RedactRules::default()
        };
        let mut map = Mapping::default();
        let out = mask("a@example.com and b@example.com", rules, &mut map);
        assert_eq!(out.text, "KE_EMAIL_1 and KE_EMAIL_2");
        assert_eq!(out.count, 2);
    }

    #[test]
    fn mask_uses_a_distinct_prefix_per_rule_family() {
        let rules = RedactRules {
            internal_ips: true,
            emails: true,
            ..RedactRules::default()
        };
        let mut map = Mapping::default();
        let out = mask("mail ops@example.com from 192.168.1.20", rules, &mut map);
        assert_eq!(out.text, "mail KE_EMAIL_1 from KE_IP_1");
        assert_eq!(out.count, 2);
    }

    #[test]
    fn mask_hides_api_keys_and_env_secrets_behind_a_secret_placeholder() {
        let mut map = Mapping::default();
        let out = mask(
            "token sk-live-abcdef123456\nDB_PASSWORD=hunter2",
            RedactRules::default(),
            &mut map,
        );
        // env_secrets runs before api_keys (same order as `redact`), so the env value is
        // numbered first.
        assert!(out.text.contains("DB_PASSWORD=KE_SECRET_1"), "{}", out.text);
        assert!(out.text.contains("token KE_SECRET_2"), "{}", out.text);
        assert_eq!(out.count, 2, "{}", out.text);
    }

    #[test]
    fn mask_leaves_plain_text_untouched_with_zero_count() {
        let mut map = Mapping::default();
        let text = "echo hello && ls -la ~/projects/skate-park";
        let out = mask(text, RedactRules::default(), &mut map);
        assert_eq!(out.text, text);
        assert_eq!(out.count, 0);
        assert_eq!(
            restore(text, &map),
            text,
            "nothing was masked, so restore is a no-op"
        );
    }

    #[test]
    fn restore_reverses_mask_back_to_the_original_text() {
        let rules = RedactRules {
            internal_ips: true,
            emails: true,
            ..RedactRules::default()
        };
        let mut map = Mapping::default();
        let text = "reach ops@example.com from 10.0.0.7, then ops@example.com again";
        let out = mask(text, rules, &mut map);
        assert_ne!(
            out.text, text,
            "sanity check: masking actually changed the text"
        );
        assert_eq!(restore(&out.text, &map), text);
    }

    #[test]
    fn restore_handles_placeholder_numbers_that_share_a_prefix() {
        let mut map = Mapping::default();
        // Ten distinct secret-shaped values force `KE_SECRET_10` to exist alongside
        // `KE_SECRET_1`; restore must not let the shorter placeholder eat the "0".
        let text: String = (0..10)
            .map(|i| format!("sk-live-secretvalue{i:03}abcdefgh"))
            .collect::<Vec<_>>()
            .join(" ");
        let out = mask(&text, RedactRules::default(), &mut map);
        assert!(out.text.contains("KE_SECRET_10"), "{}", out.text);
        assert_eq!(out.count, 10, "{}", out.text);
        assert_eq!(restore(&out.text, &map), text);
    }

    #[test]
    fn masking_an_already_masked_text_is_a_no_op() {
        let mut map = Mapping::default();
        let text = "TOKEN=sk-live-abcdefgh12345678";
        let first = mask(text, RedactRules::default(), &mut map);
        let second = mask(&first.text, RedactRules::default(), &mut map);
        assert_eq!(
            second.text, first.text,
            "a placeholder must not be masked again"
        );
        assert_eq!(second.count, 0, "second pass should find nothing new");
        assert_eq!(restore(&first.text, &map), text, "single restore recovers");
    }

    #[test]
    fn masking_already_masked_text_is_a_no_op_with_mixed_placeholder_types() {
        let rules = RedactRules {
            internal_ips: true,
            emails: true,
            ..RedactRules::default()
        };
        let mut map = Mapping::default();
        let text = "reach ops@example.com from 10.0.0.7";
        let first = mask(text, rules, &mut map);
        assert_eq!(first.text, "reach KE_EMAIL_1 from KE_IP_1");

        let second = mask(&first.text, rules, &mut map);
        assert_eq!(
            second.text, first.text,
            "a mix of already-masked placeholder types must not be masked again"
        );
        assert_eq!(second.count, 0, "second pass should find nothing new");
        assert_eq!(restore(&first.text, &map), text, "single restore recovers");
    }

    #[test]
    fn masking_stays_idempotent_when_a_secret_ends_in_punctuation() {
        let mut map = Mapping::default();
        let text = "export TOKEN=sk-live-abcdefgh12345678; echo done";
        let first = mask(text, RedactRules::default(), &mut map);
        let second = mask(&first.text, RedactRules::default(), &mut map);
        assert_eq!(second.text, first.text, "masked once: {}", first.text);
        assert_eq!(second.count, 0);
    }

    #[test]
    fn masking_leaves_a_handwritten_placeholder_and_its_trailing_text_alone() {
        let mut map = Mapping::default();
        // Seed the map so KE_SECRET_1 is a real placeholder.
        let seeded = mask(
            "TOKEN=sk-live-abcdefgh12345678",
            RedactRules::default(),
            &mut map,
        );
        assert!(seeded.text.contains("KE_SECRET_1"));
        // Now text where a model wrote the placeholder itself, followed by punctuation.
        let written = "export TOKEN=KE_SECRET_1; echo ok";
        let again = mask(written, RedactRules::default(), &mut map);
        assert_eq!(again.text, written, "handwritten placeholder was re-masked");
        assert_eq!(again.count, 0);
    }

    #[test]
    fn masking_leaves_a_placeholder_mid_sentence_followed_by_non_whitespace_alone() {
        let mut map = Mapping::default();
        let seeded = mask(
            "TOKEN=sk-live-abcdefgh12345678",
            RedactRules::default(),
            &mut map,
        );
        assert!(seeded.text.contains("KE_SECRET_1"), "{}", seeded.text);

        // A placeholder glued directly (no separating whitespace) to trailing Chinese punctuation
        // and prose -- the CJK analogue of the ASCII-punctuation case above. `env_secret`'s
        // greedy value group swallows everything up to the next whitespace, so this must be
        // recognized by prefix, not by whole-value equality.
        let written = "用 TOKEN=KE_SECRET_1,联系他";
        let again = mask(written, RedactRules::default(), &mut map);
        assert_eq!(
            again.text, written,
            "placeholder glued to trailing CJK text was re-masked"
        );
        assert_eq!(again.count, 0);

        // A placeholder sitting mid-sentence where no rule's pattern matches at all (no `@`, no
        // `=`) is trivially a no-op too, and must not disturb the mapping built above.
        let rules = RedactRules {
            emails: true,
            ..RedactRules::default()
        };
        let sentence = "用 KE_EMAIL_1, 联系他";
        let sentence_out = mask(sentence, rules, &mut map);
        assert_eq!(sentence_out.text, sentence);
        assert_eq!(sentence_out.count, 0);
    }

    #[test]
    fn mapping_fails_closed_once_capacity_is_reached() {
        let rules = RedactRules {
            emails: true,
            ..RedactRules::NONE
        };
        let mut map = Mapping::default();
        for i in 0..MAX_MAPPING_ENTRIES {
            let out = mask(&format!("user{i}@example.com"), rules, &mut map);
            assert_eq!(out.count, 1, "value {i} should still be masked");
        }

        // A brand new value past the cap must never reach the model as plaintext: it still gets
        // hidden, just with the irreversible constant instead of a fresh, restorable placeholder.
        let overflow = mask("overflow@example.com", rules, &mut map);
        assert_eq!(
            overflow.text, REDACTED,
            "capacity exceeded must fail closed to REDACTED, never plaintext"
        );
        assert_eq!(overflow.count, 1);
        assert!(
            !map.placeholder_to_real.contains_key(REDACTED),
            "the fail-closed constant must not be recorded as a restorable placeholder"
        );

        // Values recorded before the cap was hit keep working normally (still a hit: the text
        // was substituted, even though the placeholder itself is not new).
        let known = mask("user0@example.com", rules, &mut map);
        assert_eq!(known.text, "KE_EMAIL_1");
        assert_eq!(known.count, 1);
    }

    #[test]
    fn max_placeholder_numbers_finds_the_highest_per_family_and_ignores_absent_ones() {
        let text = "a KE_SECRET_3 b KE_SECRET_10 c KE_EMAIL_1 d KE_SECRET_2 e";
        let found = max_placeholder_numbers(text);
        assert_eq!(
            found.get("KE_SECRET"),
            Some(&10),
            "non-contiguous numbers, including a two-digit one, must still find the true max"
        );
        assert_eq!(found.get("KE_EMAIL"), Some(&1));
        assert_eq!(
            found.get("KE_IP"),
            None,
            "a family with no match in the text must be absent from the result"
        );
    }

    #[test]
    fn max_placeholder_numbers_is_empty_for_text_with_no_placeholders() {
        assert!(max_placeholder_numbers("").is_empty());
        assert!(max_placeholder_numbers("no placeholders in this sentence at all").is_empty());
    }

    #[test]
    fn reserve_through_raises_the_floor_per_family_independently() {
        let mut map = Mapping::default();
        map.reserve_through("KE_SECRET", 5);
        map.reserve_through("KE_EMAIL", 2);
        // Unrecognized prefix: must not panic, and must not invent a new family.
        map.reserve_through("KE_BOGUS", 99);

        let rules = RedactRules {
            emails: true,
            ..RedactRules::default()
        };
        let out = mask(
            "token sk-live-abcdefgh12345678 and ops@example.com",
            rules,
            &mut map,
        );
        assert!(
            out.text.contains("KE_SECRET_6"),
            "secret numbering resumes just past its reserved floor: {}",
            out.text
        );
        assert!(
            out.text.contains("KE_EMAIL_3"),
            "email numbering resumes just past its own, independent floor: {}",
            out.text
        );

        // Reserving a lower floor afterward must not un-reserve numbers already promised.
        map.reserve_through("KE_SECRET", 1);
        let more = mask("token2 sk-live-zzzzzzzzzzzzzzzzzz", rules, &mut map);
        assert!(more.text.contains("KE_SECRET_7"), "{}", more.text);
    }

    #[test]
    fn reserve_from_text_with_no_matches_does_not_disturb_normal_numbering() {
        let rules = RedactRules {
            emails: true,
            ..RedactRules::default()
        };
        let mut map = Mapping::default();
        map.reserve_from_text(""); // empty chat log
        map.reserve_from_text("no placeholders here, just an ordinary question");
        let out = mask("mail ops@example.com", rules, &mut map);
        assert_eq!(
            out.text, "mail KE_EMAIL_1",
            "numbering starts at 1 as normal"
        );
    }

    #[test]
    fn reserve_from_text_prevents_a_reset_mapping_from_reusing_an_old_placeholder() {
        let mut map = Mapping::default();
        let out = mask(
            "token sk-live-abcdefgh12345678",
            RedactRules::default(),
            &mut map,
        );
        assert_eq!(out.text, "token KE_SECRET_1");

        // Simulate a resident restart: a brand new `Mapping`, seeded only from what a previous
        // process cycle left behind in `chat.jsonl` (here: the masked text above, standing in for
        // an old model reply that echoed the placeholder back).
        let mut fresh = Mapping::default();
        fresh.reserve_from_text(&out.text);

        let next = mask(
            "token2 sk-live-zzzzzzzzzzzzzzzzzz",
            RedactRules::default(),
            &mut fresh,
        );
        assert_ne!(
            next.text, out.text,
            "a fresh Mapping seeded from an old chat log must not reissue KE_SECRET_1 for a \
             different real value"
        );
        assert!(next.text.contains("KE_SECRET_2"), "{}", next.text);
    }

    #[test]
    fn mask_restore_mask_round_trip_keeps_the_same_placeholders() {
        let mut map = Mapping::default();
        let text = "TOKEN=sk-live-abcdefgh12345678 and ops@example.com";
        let first = mask(text, RedactRules::default(), &mut map);
        let back = restore(&first.text, &map);
        assert_eq!(back, text, "restore must recover the original");
        let again = mask(&back, RedactRules::default(), &mut map);
        assert_eq!(
            again.text, first.text,
            "re-masking restored text must reuse the same placeholders"
        );
    }
}
