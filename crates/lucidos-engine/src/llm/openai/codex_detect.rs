//! Auto-detect an OpenAI API key from the Codex CLI's auth file.
//!
//! The OpenAI Codex CLI is the OpenAI-side coding agent (as Claude Code is the
//! Anthropic side). When a user signs in with an API key (`codex login
//! --with-api-key`), Codex stores that key in plaintext at
//! `${CODEX_HOME:-~/.codex}/auth.json` as `{"auth_mode":"apikey","OPENAI_API_KEY":"…"}`.
//! This module reads that key directly, so a machine that already has a working
//! Codex API-key login gets OpenAI models in Lucidos without any extra config —
//! the exact parallel of [`crate::llm::vertex::adc`] reading the user's gcloud
//! ADC file for Vertex.
//!
//! Scope (deliberately narrow, mirroring ADC parsing only `authorized_user`):
//! - Only `auth_mode == "apikey"` is used. A `chatgpt` login stores an OAuth
//!   `tokens` object instead, which is NOT a standalone key usable against
//!   `api.openai.com/v1` — so we ignore it and detect `None`.
//! - Only the file-based store is read. Codex can instead keep credentials in the
//!   OS keyring (`cli_auth_credentials_store = "keyring"`); then there is nothing
//!   on disk and we detect `None`.
//!
//! This is the lowest-precedence OpenAI key source (see
//! [`super::resolve_openai_api_key`]): a stored `openai` credential and the
//! `OPENAI_API_KEY` env var both win over it. The detected key is used fresh at
//! provider-build time and never copied into the credentials store.
//!
//! The key is a secret: it is returned to the caller but never logged here.

use serde::Deserialize;
use std::ffi::OsString;
use std::path::PathBuf;

/// Raw `auth.json` shape — we only need the auth mode and the API key. Other
/// fields (`tokens`, `last_refresh`, …) are ignored.
#[derive(Deserialize)]
struct CodexAuthFile {
    #[serde(default)]
    auth_mode: Option<String>,
    #[serde(rename = "OPENAI_API_KEY", default)]
    openai_api_key: Option<String>,
}

/// Resolve the Codex config directory from the environment: `CODEX_HOME` (when
/// set and non-empty), else `$HOME/.codex`. Pure over its inputs so the env
/// resolution is testable without mutating process env. `None` when neither is
/// available.
fn resolve_codex_home(
    codex_home_env: Option<OsString>,
    home_env: Option<OsString>,
) -> Option<PathBuf> {
    if let Some(dir) = codex_home_env.filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = home_env.filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".codex"))
}

/// Locate the Codex auth file: `${CODEX_HOME:-~/.codex}/auth.json`. Returns the
/// path only when it exists as a file (so a machine without Codex, or with a
/// keyring-backed store, resolves to `None`).
pub fn codex_auth_path() -> Option<PathBuf> {
    let dir = resolve_codex_home(std::env::var_os("CODEX_HOME"), std::env::var_os("HOME"))?;
    let path = dir.join("auth.json");
    path.is_file().then_some(path)
}

/// Parse a Codex `auth.json` into a usable OpenAI API key. Pure (no fs) for
/// testing. Returns `Some(key)` ONLY for an `apikey` login with a non-empty
/// (after trim) `OPENAI_API_KEY`; `chatgpt` mode, a missing/blank key, or
/// malformed JSON all return `None` so the caller cleanly falls through to
/// "OpenAI not configured" rather than feeding a bad value to the provider.
pub fn parse_codex_api_key(json: &str) -> Option<String> {
    let parsed: CodexAuthFile = serde_json::from_str(json).ok()?;
    if parsed.auth_mode.as_deref() != Some("apikey") {
        return None;
    }
    let key = parsed.openai_api_key?;
    let trimmed = key.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Locate + load the Codex API key, if a file-based `apikey` login is present.
/// `None` when Codex isn't installed, the store is keyring-backed, the login is
/// ChatGPT/OAuth, or the file is unreadable/malformed. The returned key is a
/// secret — never log it.
pub fn load() -> Option<String> {
    let path = codex_auth_path()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    parse_codex_api_key(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_apikey_login() {
        let json = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-codex-123"}"#;
        assert_eq!(parse_codex_api_key(json).as_deref(), Some("sk-codex-123"));
    }

    #[test]
    fn trims_whitespace_around_key() {
        let json = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"  sk-codex-123  "}"#;
        assert_eq!(parse_codex_api_key(json).as_deref(), Some("sk-codex-123"));
    }

    #[test]
    fn ignores_chatgpt_oauth_login() {
        // A ChatGPT login carries OAuth tokens, not a standalone API key usable
        // against api.openai.com/v1 — must detect None.
        let json = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"at","refresh_token":"rt"}}"#;
        assert_eq!(parse_codex_api_key(json), None);
    }

    #[test]
    fn none_when_key_field_missing() {
        let json = r#"{"auth_mode":"apikey"}"#;
        assert_eq!(parse_codex_api_key(json), None);
    }

    #[test]
    fn none_when_key_blank() {
        let json = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"   "}"#;
        assert_eq!(parse_codex_api_key(json), None);
    }

    #[test]
    fn none_when_auth_mode_missing() {
        // No auth_mode at all — can't assume apikey; detect None.
        let json = r#"{"OPENAI_API_KEY":"sk-codex-123"}"#;
        assert_eq!(parse_codex_api_key(json), None);
    }

    #[test]
    fn none_on_malformed_json() {
        assert_eq!(parse_codex_api_key("not json"), None);
        assert_eq!(parse_codex_api_key(""), None);
    }

    #[test]
    fn codex_home_env_overrides_home() {
        let dir = resolve_codex_home(
            Some(OsString::from("/custom/codex")),
            Some(OsString::from("/home/u")),
        )
        .expect("resolves from CODEX_HOME");
        assert_eq!(dir, PathBuf::from("/custom/codex"));
    }

    #[test]
    fn falls_back_to_home_dot_codex() {
        let dir =
            resolve_codex_home(None, Some(OsString::from("/home/u"))).expect("resolves from HOME");
        assert_eq!(dir, PathBuf::from("/home/u/.codex"));
    }

    #[test]
    fn empty_codex_home_falls_back_to_home() {
        let dir = resolve_codex_home(Some(OsString::new()), Some(OsString::from("/home/u")))
            .expect("empty CODEX_HOME ignored, uses HOME");
        assert_eq!(dir, PathBuf::from("/home/u/.codex"));
    }

    #[test]
    fn none_when_no_home_and_no_codex_home() {
        assert_eq!(resolve_codex_home(None, None), None);
        assert_eq!(
            resolve_codex_home(Some(OsString::new()), Some(OsString::new())),
            None
        );
    }
}
