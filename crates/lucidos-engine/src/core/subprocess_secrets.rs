//! What a subprocess may inherit of the engine's secrets, and the masks over
//! what it prints.
//!
//! Three layers, one per way a secret reaches a child's output:
//! - the secrets the spawn injects (credentials, OAuth tokens, the database
//!   handle, the origin token);
//! - secret-shaped variables the child inherits from the engine's own env, or
//!   gets from an `environment_variables` row;
//! - well-known token formats, whatever their source.
//!
//! A chat or script subprocess is stripped of the inherited ones as well. A
//! coding agent keeps them, since it may authenticate through one, and only
//! its output is masked.

use std::sync::{LazyLock, OnceLock};

use super::{redact_postgres_secrets, redact_secret_values, LUCIDOS_DATABASE_URL};

/// Name endings that mark an env variable as a secret. `_KEY` also covers
/// `_API_KEY`, `_ACCESS_KEY` and `_PRIVATE_KEY`.
const SECRET_NAME_SUFFIXES: &[&str] = &[
    "_TOKEN",
    "_SECRET",
    "_PASSWORD",
    "_PASSWD",
    "_KEY",
    "_PAT",
    "_CREDENTIALS",
];

/// Well-known secrets whose names no suffix catches.
const SECRET_NAMES: &[&str] = &["PGPASSWORD"];

/// Well-known names that a suffix catches but that hold a path a child needs.
const PATH_NAMES: &[&str] = &["GOOGLE_APPLICATION_CREDENTIALS"];

/// Whether `name` is shaped like a secret's env variable.
///
/// In the engine-owned `LUCIDOS_` namespace only an `_API_KEY` matches, such
/// as `LUCIDOS_XAI_API_KEY`. Its other `_KEY` names are paths a child needs,
/// such as `LUCIDOS_TLS_KEY`, and its injected tokens are named one by one in
/// [`subprocess_secret_values`].
pub fn is_secret_env_name(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    if name.starts_with("LUCIDOS_") {
        return name.ends_with("_API_KEY");
    }
    if PATH_NAMES.contains(&name.as_str()) {
        return false;
    }
    SECRET_NAMES.contains(&name.as_str())
        || SECRET_NAME_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

/// Shortest value a name heuristic masks. A short value under a guessed name,
/// such as `true`, stays unmasked rather than hiding that word everywhere.
const MIN_GUESSED_SECRET_LEN: usize = 8;

/// The secret-shaped variables of the engine's own env, as it started.
///
/// Captured before the stored `environment_variables` rows are copied in, so
/// it holds only what the engine inherited from whoever launched it. A
/// workspace row is a spawn pair instead, and is masked from there.
#[derive(Debug, Default)]
pub struct InheritedSecretEnv(Vec<(String, String)>);

impl InheritedSecretEnv {
    /// The secret-shaped, non-empty entries of `vars`.
    pub fn capture(vars: impl IntoIterator<Item = (String, String)>) -> Self {
        Self(
            vars.into_iter()
                .filter(|(name, value)| is_secret_env_name(name) && !value.is_empty())
                .collect(),
        )
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(name, _)| name.as_str())
    }

    fn values(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(_, value)| value.as_str())
    }
}

static INHERITED_SECRET_ENV: OnceLock<InheritedSecretEnv> = OnceLock::new();

/// The engine's [`InheritedSecretEnv`], captured on first use.
///
/// `apply_to_process_env` calls this before it copies the stored rows into the
/// process env, which makes the first use the startup one.
pub fn inherited_secret_env() -> &'static InheritedSecretEnv {
    INHERITED_SECRET_ENV.get_or_init(|| {
        InheritedSecretEnv::capture(std::env::vars_os().filter_map(|(name, value)| {
            Some((name.into_string().ok()?, value.into_string().ok()?))
        }))
    })
}

/// Whether a spawn keeps the secret-shaped variables it inherits from the
/// engine. Every spawn through `apply_to_subprocess_env` states it.
#[derive(Debug, Clone, Copy)]
pub enum InheritedSecrets<'a> {
    /// A coding agent, which may authenticate through an inherited provider
    /// key. Its output is masked instead.
    Keep,
    /// A chat or script subprocess. Each name in the snapshot is removed
    /// unless the spawn's own pairs set it.
    Strip(&'a InheritedSecretEnv),
}

/// The secret VALUES a subprocess can print, ready for
/// [`redact_subprocess_output`]. `env_vars` is the env the spawn injects.
///
/// Every caller handing a child's output on masks with this, except
/// `api::proxy_script_runner`, which builds its own list. It covers:
/// - each injected credential and OAuth token, and a credential's custom
///   alias for free, since the alias carries the same string;
/// - the origin token and the whole `LUCIDOS_DATABASE_URL`;
/// - each secret-shaped variable in `env_vars`, such as a user's
///   `GITHUB_TOKEN` row, and in `inherited`.
///
/// Masking by value has three holes, none closable here. A token can split
/// across two drain windows, or be cut by `Stream::push` trimming its front.
/// And `runtime::python` writes a raw copy under `.lucidos/exhaust/` first.
///
/// A `CRED_*_USERNAME` and an `OAUTH_*_EMAIL` are deliberately not secrets.
/// Masking by value is safe only while the value is unique. So the database
/// handle is masked whole, never its password alone: every dev workspace's
/// password is the literal `lucidos`, also the user, the database and every
/// path. [`redact_postgres_secrets`] covers a password elsewhere.
pub fn subprocess_secret_values(
    env_vars: &[(String, String)],
    inherited: &InheritedSecretEnv,
) -> Vec<String> {
    let guessed = |value: &str| value.len() >= MIN_GUESSED_SECRET_LEN;
    env_vars
        .iter()
        .filter(|(name, value)| {
            is_injected_secret(name) || (is_secret_env_name(name) && guessed(value))
        })
        .map(|(_, value)| value.as_str())
        .chain(inherited.values().filter(|value| guessed(value)))
        .map(str::to_string)
        .collect()
}

/// The names the engine itself injects with a secret value.
fn is_injected_secret(name: &str) -> bool {
    (name.starts_with("CRED_") && !name.ends_with("_USERNAME"))
        || (name.starts_with("OAUTH_") && name.ends_with("_ACCESS_TOKEN"))
        // Engine-minted, thread-bound, and accepted on every mutating route,
        // so it is a credential in the sense that matters here.
        || name == crate::api::actor::ENV_AGENT_ORIGIN_TOKEN
        || name == LUCIDOS_DATABASE_URL
}

/// Well-known token formats: a literal prefix, and the body that makes it a
/// token. The minimum body lengths keep ordinary words like `sk-learn`, and
/// short test placeholders, unmasked. A longer prefix comes before any prefix
/// it starts with.
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

/// The one mask over a subprocess's output: the known secret values, then the
/// token shapes, then any Postgres URL password.
pub fn redact_subprocess_output(text: &str, secrets: &[String]) -> String {
    redact_postgres_secrets(&redact_known_token_shapes(&redact_secret_values(
        text, secrets,
    )))
}

/// [`redact_subprocess_output`] over every string in a JSON value, such as a
/// tool call's input. A command can carry a token its author read from env.
pub fn redact_subprocess_output_in_json(value: &mut serde_json::Value, secrets: &[String]) {
    match value {
        serde_json::Value::String(s) => {
            let masked = redact_subprocess_output(s, secrets);
            if masked != *s {
                *s = masked;
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_subprocess_output_in_json(item, secrets);
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values_mut() {
                redact_subprocess_output_in_json(item, secrets);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
        entries
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn secret_shaped_names_match_each_suffix() {
        for name in [
            "SLACK_USER_TOKEN",
            "AWS_SECRET",
            "DB_PASSWORD",
            "FTP_PASSWD",
            "OPENAI_API_KEY",
            "AWS_ACCESS_KEY",
            "SSH_PRIVATE_KEY",
            "STRIPE_KEY",
            "GITLAB_PAT",
            "GOOGLE_CREDENTIALS",
            "github_token",
            "PGPASSWORD",
            "LUCIDOS_XAI_API_KEY",
            "LUCIDOS_OPENROUTER_API_KEY",
        ] {
            assert!(is_secret_env_name(name), "{name} is secret-shaped");
        }
    }

    #[test]
    fn ordinary_and_engine_owned_names_are_not_secret_shaped() {
        for name in [
            "LUCIDOS_TLS_KEY",
            "LUCIDOS_AGENT_ORIGIN_TOKEN",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "PATH",
            "HOME",
            "TOKEN",
            "KEYBOARD_LAYOUT",
            "CRED_MAIL_USERNAME",
            "OAUTH_GOOGLE_EMAIL",
        ] {
            assert!(!is_secret_env_name(name), "{name} is not secret-shaped");
        }
    }

    #[test]
    fn the_snapshot_keeps_only_non_empty_secret_shaped_entries() {
        let snapshot = InheritedSecretEnv::capture(pairs(&[
            ("SLACK_USER_TOKEN", "xoxp-inherited-value"),
            ("EMPTY_TOKEN", ""),
            ("HOME", "/Users/me"),
            ("LUCIDOS_TLS_KEY", "/Users/me/key.pem"),
        ]));
        assert_eq!(snapshot.names().collect::<Vec<_>>(), ["SLACK_USER_TOKEN"]);
    }

    #[test]
    fn secret_values_pick_the_secret_bearing_names_only() {
        let env = pairs(&[
            ("CRED_GITHUB", "ghp_tokenvalue"),
            ("CRED_DB_USERNAME", "reporter"),
            ("CRED_DB_PASSWORD", "hunter2pw"),
            ("OAUTH_GOOGLE_ACCESS_TOKEN", "ya29.opaque"),
            ("OAUTH_GOOGLE_EMAIL", "user@example.com"),
            ("LUCIDOS_AGENT_ORIGIN_TOKEN", "aa11bb22cc33dd44"),
            ("LUCIDOS_WORKSPACE", "/Users/me/ws"),
        ]);
        let mut picked = subprocess_secret_values(&env, &InheritedSecretEnv::default());
        picked.sort();
        assert_eq!(
            picked,
            [
                "aa11bb22cc33dd44",
                "ghp_tokenvalue",
                "hunter2pw",
                "ya29.opaque"
            ],
            "a username, an OAuth email and the workspace path are not secrets"
        );
    }

    #[test]
    fn a_secret_shaped_row_is_masked_by_value() {
        let env = pairs(&[
            ("GITHUB_TOKEN", "row-token-value"),
            ("FEATURE_KEY", "true"),
            ("EDITOR", "vim-but-long-enough"),
        ]);
        assert_eq!(
            subprocess_secret_values(&env, &InheritedSecretEnv::default()),
            ["row-token-value"],
            "a short value under a guessed name, and a plain name, stay unmasked"
        );
    }

    #[test]
    fn an_inherited_secret_is_masked_by_value() {
        let inherited = InheritedSecretEnv::capture(pairs(&[
            ("SLACK_USER_TOKEN", "inherited-slack-value"),
            ("SHORT_TOKEN", "abc"),
        ]));
        assert_eq!(
            subprocess_secret_values(&[], &inherited),
            ["inherited-slack-value"]
        );
    }

    /// Masking by value is safe only while the value is unique to the secret.
    /// Every dev and e2e workspace uses the literal Postgres password
    /// `lucidos`, as do the user, the database and every path. Masking it
    /// turned `ls ~/workspaces/dev` into a listing of `[REDACTED]`.
    #[test]
    fn the_database_handle_is_masked_whole_and_its_parts_survive() {
        let handle = format!(
            "postgresql://lucidos:lucidos@localhost:5432/lucidos_dev?options={}",
            lucidos_pg_url::READ_ONLY_OPTION
        );
        let env = pairs(&[
            (LUCIDOS_DATABASE_URL, &handle),
            ("CRED_REAL", "ghp_uniquevalue"),
        ]);
        let secrets = subprocess_secret_values(&env, &InheritedSecretEnv::default());
        let result = format!(
            "LUCIDOS_DATABASE_URL={handle}\n\
             ls /Users/me/workspaces/dev/.lucidos: lucidos-engine lucidos_dev"
        );
        assert_eq!(
            redact_subprocess_output(&result, &secrets),
            "LUCIDOS_DATABASE_URL=[REDACTED]\n\
             ls /Users/me/workspaces/dev/.lucidos: lucidos-engine lucidos_dev",
            "an `env` dump hides the handle, and ordinary output survives"
        );
    }

    /// A tool result that echoed an injected secret must not reach the event
    /// store carrying it. `curl -v`, `env` and `set -x` all do this.
    #[test]
    fn an_echoed_credential_does_not_survive_into_the_tool_result() {
        let env = pairs(&[
            ("CRED_GITHUB", "opaque-gh-value"),
            ("OAUTH_GOOGLE_ACCESS_TOKEN", "ya29.opaque"),
            ("OAUTH_GOOGLE_EMAIL", "user@example.com"),
        ]);
        let result = "> Authorization: Bearer opaque-gh-value\n[stderr]\n\
                      GITHUB_TOKEN=opaque-gh-value\nya29.opaque\nuser@example.com";
        let redacted = redact_subprocess_output(
            result,
            &subprocess_secret_values(&env, &InheritedSecretEnv::default()),
        );
        assert!(!redacted.contains("opaque-gh-value"), "{redacted}");
        assert!(!redacted.contains("ya29.opaque"), "{redacted}");
        assert_eq!(redacted.matches("[REDACTED]").count(), 3);
        assert!(redacted.contains("GITHUB_TOKEN=") && redacted.contains("user@example.com"));
    }

    #[test]
    fn each_token_shape_is_masked_with_its_prefix_kept() {
        let body = "Ab3dEf6hIj9kLm2nOp5qRs8tUv1wXy4zAb7dEf0h";
        for (prefix, token) in [
            ("sk-ant-", format!("sk-ant-api03-{body}")),
            ("sk-or-v1-", format!("sk-or-v1-{body}")),
            ("sk-proj-", format!("sk-proj-{body}")),
            ("sk-", format!("sk-{body}")),
            ("github_pat_", format!("github_pat_11ABC_{body}")),
            ("ghp_", format!("ghp_{body}")),
            ("gho_", format!("gho_{body}")),
            ("ghu_", format!("ghu_{body}")),
            ("ghs_", format!("ghs_{body}")),
            ("ghr_", format!("ghr_{body}")),
            ("xoxb-", "xoxb-1234567890-0987654321-abcdef".to_string()),
            ("xoxp-", "xoxp-1234567890-0987654321-abcdef".to_string()),
            ("hf_", format!("hf_{body}")),
            ("glpat-", format!("glpat-{body}")),
            ("AKIA", "AKIAIOSFODNN7EXAMPLE".to_string()),
            (
                "AIza",
                "AIzaSyA1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q".to_string(),
            ),
        ] {
            let text = format!("export TOKEN={token}\n\"{token}\" done");
            let masked = redact_known_token_shapes(&text);
            assert_eq!(
                masked,
                format!("export TOKEN={prefix}[REDACTED]\n\"{prefix}[REDACTED]\" done"),
                "{token}"
            );
        }
    }

    #[test]
    fn ordinary_text_is_not_mistaken_for_a_token() {
        for text in [
            "pip install scikit-learn sk-learn",
            "task-runner-for-the-nightly-production-rollout",
            "desk-and-chair-inventory-for-the-whole-office",
            "ghp_test sk-test-123 xoxb-1 hf_short glpat-short",
            "commit 9fceb02d0ae598e95dc970b74767f19372d61af8",
            "AKIAshort and AIza alone",
            "my_ghp_Ab3dEf6hIj9kLm2nOp5qRs8tUv1wXy4zAb7dEf0h",
        ] {
            assert_eq!(redact_known_token_shapes(text), text);
        }
    }

    #[test]
    fn the_one_entry_point_applies_all_three_masks() {
        let secrets = vec!["opaque-row-value".to_string()];
        let text = "a=opaque-row-value b=ghp_Ab3dEf6hIj9kLm2nOp5qRs8tUv1wXy4zAb7d \
                    c=postgres://u:hunter2@h:5432/d";
        assert_eq!(
            redact_subprocess_output(text, &secrets),
            "a=[REDACTED] b=ghp_[REDACTED] c=postgres://u:***@h:5432/d"
        );
    }
}
