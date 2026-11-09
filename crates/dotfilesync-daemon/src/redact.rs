//! Best-effort secret redaction for log lines.
//!
//! This is defense in depth: the daemon's primary rule is to log only event metadata, never
//! request params, reports, config bodies or subprocess output.

pub const REDACTED: &str = "[REDACTED]";

const TOKEN_PREFIXES: &[&str] = &["github_pat_", "ghp_", "gho_", "ghu_", "ghs_", "ghr_"];
const SENSITIVE_KEYS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "api-key",
    "access_key",
    "private_key",
    "credential",
    "authorization",
];

fn is_sensitive_key(key: &str) -> bool {
    let k = key
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
        .to_ascii_lowercase();
    !k.is_empty() && SENSITIVE_KEYS.iter().any(|s| k.contains(s))
}

fn redact_token_prefixes(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut rest = word;
    'outer: while !rest.is_empty() {
        for prefix in TOKEN_PREFIXES {
            if let Some(pos) = rest.find(prefix) {
                let after = &rest[pos + prefix.len()..];
                let tail_len = after
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(after.len());
                if tail_len >= 4 {
                    out.push_str(&rest[..pos]);
                    out.push_str(REDACTED);
                    rest = &after[tail_len..];
                    continue 'outer;
                }
            }
        }
        out.push_str(rest);
        break;
    }
    out
}

fn redact_url_userinfo(word: &str) -> String {
    if let Some(scheme_end) = word.find("://") {
        let auth_start = scheme_end + 3;
        let rest = &word[auth_start..];
        let host_end = rest.find('/').unwrap_or(rest.len());
        if let Some(at) = rest[..host_end].rfind('@') {
            return format!("{}{}{}", &word[..auth_start], REDACTED, &rest[at..]);
        }
    }
    word.to_string()
}

fn redact_key_value(word: &str) -> Option<String> {
    let sep = word.find(['=', ':'])?;
    let (key, value) = (&word[..sep], &word[sep + 1..]);
    if value.is_empty() || value.starts_with("//") || !is_sensitive_key(key) {
        return None;
    }
    Some(format!("{}{}{}", key, &word[sep..sep + 1], REDACTED))
}

/// Redact secrets from a single log line.
pub fn redact(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut redact_next = false;
    for (i, word) in line.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if word.is_empty() {
            continue;
        }
        let lower = word.to_ascii_lowercase();
        let bare = lower.trim_end_matches(':');
        if bare == "bearer" || bare == "basic" {
            redact_next = true;
            out.push_str(word);
            continue;
        }
        if redact_next {
            out.push_str(REDACTED);
            redact_next = false;
            continue;
        }
        if word.ends_with(':') && is_sensitive_key(word) {
            redact_next = true;
            out.push_str(word);
            continue;
        }
        let mut w = redact_url_userinfo(word);
        if let Some(kv) = redact_key_value(&w) {
            w = kv;
        }
        out.push_str(&redact_token_prefixes(&w));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_github_tokens() {
        let s = redact("pushing with ghp_abcdefghijklmnopqrstuvwxyz0123456789 now");
        assert_eq!(s, "pushing with [REDACTED] now");
        let s = redact("x=github_pat_11ABCDEFG_hijklmnop");
        assert!(!s.contains("11ABCDEFG"), "{s}");
    }

    #[test]
    fn redacts_auth_headers_and_key_values() {
        assert_eq!(
            redact("Authorization: Bearer abc.def.ghi"),
            "Authorization: Bearer [REDACTED]"
        );
        assert_eq!(redact("Basic dXNlcjpwYXNz"), "Basic [REDACTED]");
        assert_eq!(redact("password=hunter2"), "password=[REDACTED]");
        assert_eq!(redact("API_KEY: sk-12345"), "API_KEY: [REDACTED]");
        assert_eq!(redact("db_secret=\"xyz\""), "db_secret=[REDACTED]");
        assert_eq!(redact("approval token issued"), "approval token issued");
    }

    #[test]
    fn redacts_url_userinfo() {
        assert_eq!(
            redact("cloning https://user:pa55@github.com/me/repo.git"),
            "cloning https://[REDACTED]@github.com/me/repo.git"
        );
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        let s = "job j-0123 kind=scan status=succeeded path=/Users/me/.config";
        assert_eq!(redact(s), s);
        assert_eq!(
            redact("url=https://example.com/x"),
            "url=https://example.com/x"
        );
    }
}
