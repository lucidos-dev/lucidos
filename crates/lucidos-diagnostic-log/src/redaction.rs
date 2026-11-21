use lucidos_gateway_constants::{DEVICE_PAIRING_SECRET_PREFIX, GATEWAY_PAIRING_SECRET_PREFIX};
use std::sync::LazyLock;
const MIN_GUESSED_SECRET_LEN: usize = 8;
const TOKEN_SHAPES: &[(&str, &str)] = &[
    ("sk-ant-", "[A-Za-z0-9_-]{20,}"),
    ("sk-or-v1-", "[A-Za-z0-9_-]{20,}"),
    ("sk-proj-", "[A-Za-z0-9_-]{20,}"),
    ("sk-", "[A-Za-z0-9_-]{20,}"),
    ("github_pat_", "[A-Za-z0-9_]{22,}"),
    ("ghp_", "[A-Za-z0-9]{30,}"),
    ("gho_", "[A-Za-z0-9]{30,}"),
    ("ghu_", "[A-Za-z0-9]{30,}"),
    ("ghs_", "[A-Za-z0-9]{30,}"),
    ("ghr_", "[A-Za-z0-9]{30,}"),
    ("xoxa-", "[A-Za-z0-9-]{10,}"),
    ("xoxb-", "[A-Za-z0-9-]{10,}"),
    ("xoxp-", "[A-Za-z0-9-]{10,}"),
    ("xoxo-", "[A-Za-z0-9-]{10,}"),
    ("xoxs-", "[A-Za-z0-9-]{10,}"),
    ("xoxr-", "[A-Za-z0-9-]{10,}"),
    ("hf_", "[A-Za-z0-9]{30,}"),
    ("glpat-", "[A-Za-z0-9_-]{20,}"),
    ("AKIA", r"[0-9A-Z]{16}\b"),
    ("AIza", "[0-9A-Za-z_-]{35}"),
    // A 1Password service account token, which reads the user's vaults.
    ("ops_", "[A-Za-z0-9_+/=-]{20,}"),
    // A gateway's pairing secrets, which pair a device or another gateway.
    (DEVICE_PAIRING_SECRET_PREFIX, "[a-z2-7]{20,}"),
    (GATEWAY_PAIRING_SECRET_PREFIX, "[a-z2-7]{20,}"),
];

/// Mask every well-known token format in `text`, keeping its prefix for
/// debugging: `ghp_[REDACTED]`. A token must start at a word edge, so a
/// longer word that merely contains a prefix (`task-…`) stays whole.
pub fn redact_known_token_shapes(text: &str) -> String {
    static RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        let shapes: Vec<String> = TOKEN_SHAPES
            .iter()
            .map(|(prefix, body)| format!("{}{body}", regex::escape(prefix)))
            .collect();
        regex::Regex::new(&format!("(^|[^A-Za-z0-9_-])(?:{})", shapes.join("|")))
            .expect("token shape regex must compile")
    });
    RE.replace_all(text, |caps: &regex::Captures| {
        let edge = &caps[1];
        let token = &caps[0][edge.len()..];
        let prefix = TOKEN_SHAPES
            .iter()
            .map(|(prefix, _)| *prefix)
            .find(|prefix| token.starts_with(prefix))
            .unwrap_or_default();
        format!("{edge}{prefix}[REDACTED]")
    })
    .into_owned()
}

const SECRET_ARG_NAME_WORDS: &[&str] = &[
    "KEY",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "CREDENTIAL",
    "AUTH",
];

pub fn is_secret_arg_name(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    SECRET_ARG_NAME_WORDS.iter().any(|word| name.contains(word))
}

/// Whether `arg` is a flag naming a secret that its next argument carries.
fn is_secret_flag(arg: &str) -> bool {
    arg.starts_with('-') && !arg.contains('=') && is_secret_arg_name(arg.trim_start_matches('-'))
}

/// The secrets inside one argument: any URL credential, a secret-named
/// header's value, and the value of a secret-named `--flag=`, env assignment
/// or URL query key. A header may also sit after a `--flag=`.
fn named_secret_values(arg: &str) -> impl Iterator<Item = String> + '_ {
    static USERINFO: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"[A-Za-z][A-Za-z0-9+.-]*://([^/?#@\s]+)@")
            .expect("URL credential regex must compile")
    });
    static HEADER: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^([A-Za-z][A-Za-z0-9-]*)\s*:\s*(\S.*)$")
            .expect("header regex must compile")
    });
    static PAIR: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?:^-{0,2}|[?&;])([A-Za-z_][A-Za-z0-9_.-]*)=([^&;#\s]*)")
            .expect("argv pair regex must compile")
    });
    let userinfo = USERINFO.captures_iter(arg).map(|caps| caps[1].to_string());
    let flag_value = arg
        .starts_with('-')
        .then(|| arg.split_once('=').map(|(_, value)| value))
        .flatten();
    let headers = std::iter::once(arg)
        .chain(flag_value)
        .filter_map(|text| HEADER.captures(text))
        .filter(|caps| is_secret_arg_name(&caps[1]))
        .map(|caps| caps[2].to_string());
    let pairs = PAIR
        .captures_iter(arg)
        .filter(|caps| is_secret_arg_name(&caps[1]))
        .map(|caps| caps[2].to_string());
    userinfo.chain(headers).chain(pairs)
}

/// Secret values following flags or embedded in named arguments.
///
/// A flag never counts as a value, since a secret-named flag may take none.
/// Like any guessed secret, a value shorter than [`MIN_GUESSED_SECRET_LEN`]
/// stays unmasked.
pub fn argv_secret_values(args: &[String]) -> Vec<String> {
    let mut values = Vec::new();
    let mut follows_secret_flag = false;
    for arg in args {
        if follows_secret_flag && !arg.starts_with('-') {
            values.push(arg.clone());
            follows_secret_flag = false;
        } else {
            follows_secret_flag = is_secret_flag(arg);
            values.extend(named_secret_values(arg));
        }
    }
    values.retain(|value| value.len() >= MIN_GUESSED_SECRET_LEN);
    values
}

pub fn redact_postgres_secrets(s: &str) -> String {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(postgres(?:ql)?://[^:@/?\s]+):[^@/?\s]+@")
            .expect("postgres redaction regex must compile")
    });
    RE.replace_all(s, "$1:***@").into_owned()
}

pub const MIN_REDACTABLE_SECRET_LEN: usize = 4;

/// What a redacted secret value reads as.
pub const REDACTED_MARKER: &str = "[REDACTED]";

/// Replace every occurrence of each known secret value in `text` with
/// [`REDACTED_MARKER`]. Keeps credential material out of logs and error
/// messages that untrusted code could otherwise echo.
pub fn redact_secret_values(text: &str, secrets: &[String]) -> String {
    match redact_secret_bytes(text.as_bytes(), secrets) {
        // A UTF-8 secret matches only on character boundaries, and the marker
        // is ASCII, so the result stays UTF-8.
        Some(redacted) => String::from_utf8(redacted)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
        None => text.to_string(),
    }
}

/// [`redact_secret_values`] over raw bytes, for a body that need not be text.
/// Returns `None` when no secret occurs, so the caller keeps its own bytes.
/// Longest secrets are scrubbed first, so a secret that is a substring of
/// another leaves no partial match behind.
pub fn redact_secret_bytes(bytes: &[u8], secrets: &[String]) -> Option<Vec<u8>> {
    let mut ordered: Vec<&[u8]> = secrets
        .iter()
        .map(|s| s.as_bytes())
        .filter(|s| s.len() >= MIN_REDACTABLE_SECRET_LEN)
        .collect();
    ordered.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut out: Option<Vec<u8>> = None;
    for secret in ordered {
        let current = out.as_deref().unwrap_or(bytes);
        if let Some(replaced) = replace_all_bytes(current, secret, REDACTED_MARKER.as_bytes()) {
            out = Some(replaced);
        }
    }
    out
}

/// Every non-overlapping `needle` in `haystack` replaced by `with`, or `None`
/// when `needle` does not occur. `needle` must not be empty.
fn replace_all_bytes(haystack: &[u8], needle: &[u8], with: &[u8]) -> Option<Vec<u8>> {
    let find = |from: usize| {
        haystack[from..]
            .windows(needle.len())
            .position(|window| window == needle)
            .map(|at| from + at)
    };
    let mut at = find(0)?;
    let mut out = Vec::with_capacity(haystack.len());
    let mut copied = 0;
    loop {
        out.extend_from_slice(&haystack[copied..at]);
        out.extend_from_slice(with);
        copied = at + needle.len();
        match find(copied) {
            Some(next) => at = next,
            None => break,
        }
    }
    out.extend_from_slice(&haystack[copied..]);
    Some(out)
}
