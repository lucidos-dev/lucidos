//! Application Default Credentials (ADC) — Vertex auth without the `gcloud`
//! binary.
//!
//! A packaged build ships no `gcloud` CLI, so the legacy paths
//! (`gcloud auth application-default print-access-token` for the token and
//! `gcloud config get-value project` for the project) can't run. But a user who
//! has run `gcloud auth application-default login` already has an ADC file on
//! disk and a default project in the gcloud config. This module reads BOTH
//! directly and refreshes the OAuth token itself, so Vertex works in a packaged
//! build with no binary and no service-account key — using the user's own
//! identity (no enterprise org-policy fight over key creation).
//!
//! Only the `authorized_user` ADC type (what the login writes) is parsed here.
//! Other types (`service_account` / `external_account` / `impersonated_service_account`)
//! return `None` so the caller falls back to the `gcloud` subprocess.

use serde::Deserialize;
use std::path::PathBuf;

/// Google's OAuth2 token endpoint (refresh-grant exchange).
const GOOGLE_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";

/// A parsed `authorized_user` ADC credential. All three fields are secrets and
/// must never be logged — see the manual [`std::fmt::Debug`] below.
#[derive(Clone)]
pub struct AdcCredentials {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    /// Project carried by the ADC file (set via
    /// `gcloud auth application-default set-quota-project`), if any.
    pub quota_project_id: Option<String>,
}

impl std::fmt::Debug for AdcCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never leak the OAuth secrets through `{:?}`.
        f.debug_struct("AdcCredentials")
            .field("client_id", &"<redacted>")
            .field("client_secret", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("quota_project_id", &self.quota_project_id)
            .finish()
    }
}

/// Raw ADC JSON shape — we only handle `type: "authorized_user"`.
#[derive(Deserialize)]
struct AdcFile {
    #[serde(rename = "type")]
    cred_type: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    refresh_token: Option<String>,
    #[serde(default)]
    quota_project_id: Option<String>,
}

/// The gcloud config dir: `$CLOUDSDK_CONFIG` if set, else `~/.config/gcloud`
/// (the macOS/Linux default — the engine ships on those).
fn gcloud_config_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CLOUDSDK_CONFIG") {
        let p = PathBuf::from(p);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".config").join("gcloud"))
}

/// Locate the ADC file: `$GOOGLE_APPLICATION_CREDENTIALS`, else
/// `<gcloud-config>/application_default_credentials.json`. Returns the first
/// path that exists.
pub fn adc_file_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let p = gcloud_config_dir()?.join("application_default_credentials.json");
    p.is_file().then_some(p)
}

/// Parse ADC JSON as an `authorized_user` credential. Pure (no fs) for testing.
/// Returns `None` for any other ADC type or a malformed/missing-field file — the
/// caller then falls back to the `gcloud` subprocess.
pub fn parse_authorized_user(json: &str) -> Option<AdcCredentials> {
    let parsed: AdcFile = serde_json::from_str(json).ok()?;
    if parsed.cred_type.as_deref() != Some("authorized_user") {
        return None;
    }
    Some(AdcCredentials {
        client_id: parsed.client_id?,
        client_secret: parsed.client_secret?,
        refresh_token: parsed.refresh_token?,
        quota_project_id: parsed.quota_project_id.filter(|s| !s.is_empty()),
    })
}

/// Locate + load the `authorized_user` ADC credential, if present.
pub fn load() -> Option<AdcCredentials> {
    let path = adc_file_path()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    parse_authorized_user(&raw)
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    // `expires_in` is ignored — the shared token cache applies a fixed TTL below
    // the ~3600s lifetime, same as the gcloud path.
}

/// Exchange the refresh token for a fresh access token. Errors carry the HTTP
/// status + Google's `error_description` body (which never contains the secrets
/// we send), never the credential fields themselves.
pub async fn refresh_access_token(
    creds: &AdcCredentials,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let resp = reqwest::Client::new()
        .post(GOOGLE_TOKEN_URI)
        .form(&[
            ("client_id", creds.client_id.as_str()),
            ("client_secret", creds.client_secret.as_str()),
            ("refresh_token", creds.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(format!("ADC token refresh failed (HTTP {}): {}", status, body).into());
    }
    let parsed: TokenResponse =
        serde_json::from_str(&body).map_err(|e| format!("ADC token response parse error: {e}"))?;
    Ok(parsed.access_token)
}

/// Resolve the Vertex project WITHOUT the `gcloud` binary: the ADC file's
/// `quota_project_id`, else the active gcloud configuration's `[core] project`.
/// `None` if neither is available (caller falls back to `gcloud config`).
pub fn project_from_files() -> Option<String> {
    if let Some(p) = load().and_then(|c| c.quota_project_id) {
        return Some(p);
    }
    project_from_gcloud_config()
}

/// Read `[core] project` from the active gcloud configuration file
/// (`<config>/configurations/config_<active>`, where `<active>` comes from the
/// `active_config` file, default `default`). No `gcloud` binary needed.
fn project_from_gcloud_config() -> Option<String> {
    let dir = gcloud_config_dir()?;
    let active = std::fs::read_to_string(dir.join("active_config"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".to_string());
    let raw = std::fs::read_to_string(dir.join("configurations").join(format!("config_{active}")))
        .ok()?;
    parse_ini_core_project(&raw)
}

/// Pull `project` out of the `[core]` section of a gcloud INI config. Pure for
/// testing.
fn parse_ini_core_project(ini: &str) -> Option<String> {
    let mut in_core = false;
    for line in ini.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_core = line[1..line.len() - 1].trim().eq_ignore_ascii_case("core");
            continue;
        }
        if !in_core {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim().eq_ignore_ascii_case("project") {
                let v = v.trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_authorized_user_with_quota_project() {
        let json = r#"{
            "type": "authorized_user",
            "client_id": "abc.apps.googleusercontent.com",
            "client_secret": "secret",
            "refresh_token": "1//refresh",
            "quota_project_id": "my-project"
        }"#;
        let c = parse_authorized_user(json).expect("authorized_user parses");
        assert_eq!(c.quota_project_id.as_deref(), Some("my-project"));
    }

    #[test]
    fn parse_redacts_secrets_in_debug() {
        let json = r#"{"type":"authorized_user","client_id":"id","client_secret":"sshh","refresh_token":"1//rt"}"#;
        let c = parse_authorized_user(json).unwrap();
        let dbg = format!("{c:?}");
        assert!(
            !dbg.contains("sshh"),
            "client_secret must be redacted: {dbg}"
        );
        assert!(
            !dbg.contains("1//rt"),
            "refresh_token must be redacted: {dbg}"
        );
        assert!(!dbg.contains("\"id\""), "client_id must be redacted: {dbg}");
    }

    #[test]
    fn rejects_non_authorized_user_types() {
        // A service_account ADC (or any other type) is not handled here — the
        // caller falls back to gcloud.
        let sa = r#"{"type":"service_account","client_email":"x@y.iam","private_key":"-----BEGIN-----"}"#;
        assert!(parse_authorized_user(sa).is_none());
        let ext = r#"{"type":"external_account"}"#;
        assert!(parse_authorized_user(ext).is_none());
    }

    #[test]
    fn rejects_authorized_user_missing_fields() {
        // type matches but a required field is absent → None (fall back).
        let missing = r#"{"type":"authorized_user","client_id":"id"}"#;
        assert!(parse_authorized_user(missing).is_none());
    }

    #[test]
    fn parses_project_from_core_section() {
        let ini =
            "[core]\naccount = me@example.com\nproject = my-gcp-project\n[compute]\nzone = us\n";
        assert_eq!(
            parse_ini_core_project(ini).as_deref(),
            Some("my-gcp-project")
        );
    }

    #[test]
    fn ignores_project_outside_core_section() {
        let ini = "[other]\nproject = wrong\n[core]\naccount = me\n";
        assert_eq!(parse_ini_core_project(ini), None);
    }
}
