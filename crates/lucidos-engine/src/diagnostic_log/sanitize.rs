//! The one sanitizer over a diagnostic log message. It runs when a line is
//! written and again when it is read, so a redactor improvement also reaches
//! lines already on disk. Every step is idempotent.

use std::sync::{LazyLock, RwLock};

use crate::core::subprocess_secrets::{is_secret_arg_name, redact_known_token_shapes};
use crate::core::{argv_secret_values, redact_postgres_secrets, redact_secret_values};

/// The longest message a record stores.
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024;

/// The longest raw message the writer accepts. It bounds the queue's memory
/// and the sanitizer's work.
const MAX_RAW_BYTES: usize = MAX_MESSAGE_BYTES * 4;

const TRUNCATED_MARKER: &str = " [truncated]";

const QUERY_MARKER: &str = "[query omitted]";

/// The credential values registered so far: the engine's secret-shaped env at
/// boot and its local tokens. A value outside it is masked only by shape or
/// by name.
static KNOWN_SECRETS: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Add credential values for every later sanitize call to mask.
pub fn register_secret_values(values: impl IntoIterator<Item = String>) {
    let mut known = KNOWN_SECRETS.write().unwrap_or_else(|e| e.into_inner());
    for value in values {
        if !value.is_empty() && !known.contains(&value) {
            known.push(value);
        }
    }
}

/// The steps both the write and the read run, in order: known credential
/// values, token shapes, Postgres passwords, secret-named pairs and bearer
/// values, then every URL query string.
pub fn sanitize(text: &str) -> String {
    let known = KNOWN_SECRETS.read().unwrap_or_else(|e| e.into_inner());
    let text = redact_secret_values(text, &known);
    drop(known);
    let text = redact_postgres_secrets(&redact_known_token_shapes(&text));
    let text = redact_named_secrets(&text);
    omit_url_queries(&text)
}

/// A raw message bounded to [`MAX_RAW_BYTES`], and whether it was cut. The
/// cut falls before a whitespace, so no token is left too short for its shape.
pub fn bound_raw(mut msg: String) -> (String, bool) {
    if msg.len() <= MAX_RAW_BYTES {
        return (msg, false);
    }
    let limit = msg.floor_char_boundary(MAX_RAW_BYTES);
    let cut = msg[..limit].rfind(char::is_whitespace).unwrap_or(limit);
    msg.truncate(cut);
    (msg, true)
}

/// A message as a record stores it: sanitized, then capped at
/// [`MAX_MESSAGE_BYTES`]. `cut` says [`bound_raw`] already shortened it.
pub fn for_write(msg: &str, cut: bool) -> String {
    // Sanitize before the final cut, so the cut cannot leave a token too
    // short for its shape.
    let clean = sanitize(msg);
    if clean.len() <= MAX_MESSAGE_BYTES && !cut {
        return clean;
    }
    let keep = clean.floor_char_boundary(MAX_MESSAGE_BYTES - TRUNCATED_MARKER.len());
    format!("{}{TRUNCATED_MARKER}", &clean[..keep])
}

/// A stored message as a reader gets it: sanitized with the current rules,
/// with newlines and carriage returns made visible and every other control
/// character except tab removed. One record then prints as one line, and no
/// terminal escape survives.
pub fn for_read(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    for c in sanitize(msg).chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push('\t'),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Mask values that travel under a secret name (`--token x`, `apiKey=x`,
/// `x-api-key: x`, `"token":"x"`, `api_key: Some("x")`), and every `Bearer`
/// or `Basic` credential.
fn redact_named_secrets(text: &str) -> String {
    static SCHEME: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]{8,}")
            .expect("auth scheme regex must compile")
    });
    // A name, its optional quote, a `:` or `=`, then an optionally quoted or
    // `Some(`-wrapped value. `[` never starts a value, so a mask stays put.
    static FIELD: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r#"([A-Za-z_][A-Za-z0-9_.-]*)(["']?\s*[:=]\s*(?:Some\()?["']?)([^\s"',;&(){}\[\]]{8,})"#,
        )
        .expect("named field regex must compile")
    });
    let words: Vec<String> = text.split_whitespace().map(str::to_string).collect();
    let named = redact_secret_values(text, &argv_secret_values(&words));
    let fields = FIELD.replace_all(&named, |caps: &regex::Captures| {
        if is_secret_arg_name(&caps[1]) {
            format!("{}{}[REDACTED]", &caps[1], &caps[2])
        } else {
            caps[0].to_string()
        }
    });
    SCHEME.replace_all(&fields, "$1 [REDACTED]").into_owned()
}

/// Replace every URL's query string with `?[query omitted]`.
fn omit_url_queries(text: &str) -> String {
    static URL_QUERY: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"([A-Za-z][A-Za-z0-9+.-]*://[^\s?#"'<>]*)\?([^\s#"'<>]*)"#)
            .expect("URL query regex must compile")
    });
    URL_QUERY
        .replace_all(text, |caps: &regex::Captures| {
            let whole = caps.get(0).expect("group 0 always matches");
            let already_omitted =
                text[whole.start() + caps[1].len() + 1..].starts_with(QUERY_MARKER);
            if already_omitted || caps[2].is_empty() {
                caps[0].to_string()
            } else {
                format!("{}?{QUERY_MARKER}", &caps[1])
            }
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GH_TOKEN: &str = "ghp_Ab3dEf6hIj9kLm2nOp5qRs8tUv1wXy4zAb7d";

    #[test]
    fn a_registered_credential_value_is_masked() {
        register_secret_values(["registered-local-token-value".to_string()]);
        assert_eq!(
            sanitize("presented registered-local-token-value twice"),
            "presented [REDACTED] twice"
        );
    }

    #[test]
    fn a_token_shape_is_masked() {
        assert_eq!(sanitize(&format!("got {GH_TOKEN}")), "got ghp_[REDACTED]");
    }

    #[test]
    fn a_pairing_secret_is_masked() {
        let device = format!(
            "{}abcdefghijklmnopqrstuvwxyz",
            lucidos_gateway_constants::DEVICE_PAIRING_SECRET_PREFIX
        );
        let gateway = format!(
            "{}abcdefghijklmnopqrstuvwxyz",
            lucidos_gateway_constants::GATEWAY_PAIRING_SECRET_PREFIX
        );
        assert_eq!(
            sanitize(&format!("pair {device} and {gateway}")),
            "pair ldp_[REDACTED] and lgp_[REDACTED]"
        );
    }

    #[test]
    fn a_postgres_password_is_masked() {
        assert_eq!(
            sanitize("connect postgres://u:hunter2pw@h:5432/d failed"),
            "connect postgres://u:***@h:5432/d failed"
        );
    }

    #[test]
    fn a_secret_named_pair_flag_and_header_are_masked() {
        assert_eq!(
            sanitize("spawn --api-key opaque-secret-value token=opaque-pair-value"),
            "spawn --api-key [REDACTED] token=[REDACTED]"
        );
        assert_eq!(
            sanitize("sent x-api-key:opaque-header-value"),
            "sent x-api-key:[REDACTED]"
        );
    }

    #[test]
    fn a_bearer_or_basic_credential_is_masked() {
        assert_eq!(
            sanitize("Authorization: Bearer opaque.bearer.value"),
            "Authorization: Bearer [REDACTED]"
        );
        assert_eq!(
            sanitize("Authorization: Basic dXNlcjpwYXNzd29yZA=="),
            "Authorization: Basic [REDACTED]"
        );
    }

    #[test]
    fn a_secret_named_field_in_json_debug_or_header_form_is_masked() {
        for (raw, masked) in [
            (
                r#"{"access_token":"opaque-access-value","scope":"read"}"#,
                r#"{"access_token":"[REDACTED]","scope":"read"}"#,
            ),
            (
                r#"Config { api_key: "opaque-debug-value" }"#,
                r#"Config { api_key: "[REDACTED]" }"#,
            ),
            (
                r#"Config { api_key: Some("opaque-option-value") }"#,
                r#"Config { api_key: Some("[REDACTED]") }"#,
            ),
            (
                r#""x-api-key": "opaque-header-value""#,
                r#""x-api-key": "[REDACTED]""#,
            ),
            (
                "refresh_token: opaque-spaced-value",
                "refresh_token: [REDACTED]",
            ),
            ("model: claude-opus-5-5", "model: claude-opus-5-5"),
        ] {
            assert_eq!(sanitize(raw), masked, "{raw}");
            assert_eq!(sanitize(masked), masked, "{masked}");
        }
    }

    #[test]
    fn every_url_query_is_omitted() {
        assert_eq!(
            sanitize("GET https://api.example.com/v1/items?q=private+words&page=2 ok"),
            "GET https://api.example.com/v1/items?[query omitted] ok"
        );
        assert_eq!(
            sanitize("no query https://example.com/a? and a plain question?"),
            "no query https://example.com/a? and a plain question?"
        );
    }

    #[test]
    fn sanitizing_twice_changes_nothing() {
        register_secret_values(["registered-idempotence-value".to_string()]);
        let raw = format!(
            "{GH_TOKEN} registered-idempotence-value postgres://u:pw123456@h/d \
             --token opaque-secret-value Bearer opaque.bearer.value \
             https://x.example/p?a=1"
        );
        let once = sanitize(&raw);
        assert_eq!(sanitize(&once), once);
        assert_eq!(for_read(&for_write(&raw, false)), for_read(&raw));
    }

    #[test]
    fn a_long_message_is_capped_after_it_is_sanitized() {
        let msg = format!("{}{GH_TOKEN} {}", "a ".repeat(4000), "b".repeat(9000));
        let stored = for_write(&msg, false);
        assert!(stored.len() <= MAX_MESSAGE_BYTES, "{}", stored.len());
        assert!(stored.ends_with(TRUNCATED_MARKER));
        assert!(
            stored.contains("ghp_[REDACTED]"),
            "the token is masked whole"
        );
        assert!(!stored.contains("ghp_Ab3d"));
    }

    #[test]
    fn the_raw_cut_never_splits_a_token() {
        // A long query collapses, so the sanitized text fits the record. A
        // token across the raw limit must still go whole or not at all.
        let query = format!("https://x.example/p?{}", "q".repeat(MAX_RAW_BYTES - 40));
        let msg = format!("{query} {GH_TOKEN} tail");
        let (bounded, cut) = bound_raw(msg);
        assert!(cut);
        assert!(bounded.len() <= MAX_RAW_BYTES);
        let stored = for_write(&bounded, cut);
        assert!(stored.ends_with(TRUNCATED_MARKER), "{stored}");
        assert!(!stored.contains("ghp_Ab3d"), "{stored}");
    }

    #[test]
    fn a_message_within_the_cap_is_stored_whole() {
        assert_eq!(bound_raw("short line".into()), ("short line".into(), false));
        assert_eq!(for_write("short line", false), "short line");
    }

    #[test]
    fn a_read_shows_line_breaks_and_drops_other_control_characters() {
        assert_eq!(
            for_read("one\ntwo\r\n\u{1b}[31mred\u{1b}[0m\tcol\u{7}"),
            "one\\ntwo\\r\\n[31mred[0m\tcol"
        );
    }
}
