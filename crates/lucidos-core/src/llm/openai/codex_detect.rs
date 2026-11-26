use serde::Deserialize;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Deserialize)]
struct CodexAuthFile {
    #[serde(default)]
    auth_mode: Option<String>,
    #[serde(rename = "OPENAI_API_KEY", default)]
    openai_api_key: Option<String>,
}

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

pub fn codex_auth_path() -> Option<PathBuf> {
    let dir = resolve_codex_home(std::env::var_os("CODEX_HOME"), std::env::var_os("HOME"))?;
    let path = dir.join("auth.json");
    path.is_file().then_some(path)
}

pub fn parse_codex_api_key(json: &str) -> Option<String> {
    let parsed: CodexAuthFile = serde_json::from_str(json).ok()?;
    if parsed.auth_mode.as_deref() != Some("apikey") {
        return None;
    }
    let key = parsed.openai_api_key?;
    let trimmed = key.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// INVARIANT: Locate + load the Codex API key, if a file-based `apikey` login is present.
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
