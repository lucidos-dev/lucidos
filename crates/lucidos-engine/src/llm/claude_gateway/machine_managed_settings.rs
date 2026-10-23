//! The gateway URL Claude Code's managed settings force on this machine.
//!
//! Settings offers it as the gateway URL. Reading it never signs in: the user
//! still confirms the URL.

use std::path::Path;

use super::GatewayOrigin;

/// The managed setting that names the gateway Claude Code signs in to.
const GATEWAY_URL_KEY: &str = "forceLoginGatewayUrl";

/// The gateway this machine's Claude Code managed settings name, if any.
pub fn suggested_url() -> Option<String> {
    suggested_url_in(&lucidos_claude_code_managed::managed_settings_path(
        Path::new(lucidos_claude_code_managed::MANAGED_DIR),
    ))
}

/// The gateway the managed settings file at `path` names. A missing file, or
/// a URL that cannot be a gateway origin, suggests nothing.
fn suggested_url_in(path: &Path) -> Option<String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            crate::log!("[ClaudeGateway] Could not read {}: {}", path.display(), e);
            return None;
        }
    };
    let settings: serde_json::Value = match serde_json::from_str(&text) {
        Ok(settings) => settings,
        Err(e) => {
            crate::log!("[ClaudeGateway] Could not parse {}: {}", path.display(), e);
            return None;
        }
    };
    let url = settings.get(GATEWAY_URL_KEY)?.as_str()?;
    GatewayOrigin::parse(url)
        .map(|origin| origin.to_string())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_file(contents: &str) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().expect("temp file");
        std::fs::write(file.path(), contents).expect("write settings");
        file
    }

    #[test]
    fn suggests_the_forced_gateway_as_an_origin() {
        let file = settings_file(
            r#"{"forceLoginMethod": "gateway", "forceLoginGatewayUrl": "https://Gateway.example/"}"#,
        );
        assert_eq!(
            suggested_url_in(file.path()).as_deref(),
            Some("https://gateway.example")
        );
    }

    #[test]
    fn a_missing_file_suggests_nothing() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert_eq!(
            suggested_url_in(&dir.path().join("managed-settings.json")),
            None
        );
    }

    #[test]
    fn settings_without_a_gateway_suggest_nothing() {
        let file = settings_file(r#"{"permissions": {}}"#);
        assert_eq!(suggested_url_in(file.path()), None);
    }

    #[test]
    fn a_url_that_cannot_be_a_gateway_suggests_nothing() {
        let file = settings_file(r#"{"forceLoginGatewayUrl": "http://gateway.example"}"#);
        assert_eq!(suggested_url_in(file.path()), None);
    }

    #[test]
    fn unparsable_settings_suggest_nothing() {
        let file = settings_file("{not json");
        assert_eq!(suggested_url_in(file.path()), None);
    }
}
