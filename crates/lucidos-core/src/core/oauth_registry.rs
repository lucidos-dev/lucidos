use std::path::Path;

use serde::{Deserialize, Serialize};

const REGISTRY_FILE: &str = "oauth-providers.json";

/// INVARIANT: One known provider.
///
/// Field names match the credential JSON keys the OAuth flow reads
/// (`auth_url`, `token_url`, …) so a row maps onto
/// [`crate::core::oauth::OAuthClientOverrides`] without translation. The three
/// optional endpoint fields mean "the engine default" when absent, exactly as
/// they do on a stored credential: `userinfo_method` defaults to GET,
/// `authorize_params` to the standard offline-access pair, and `redirect_uri` to
/// the loopback-IP form.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuthProviderRow {
    pub id: String,
    pub label: String,
    pub base_url: String,
    pub auth_url: String,
    pub token_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub userinfo_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub userinfo_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorize_params: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in_scopes: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    providers: Vec<OAuthProviderRow>,
}

pub fn load_providers(system_knowhow_dir: Option<&Path>) -> Vec<OAuthProviderRow> {
    let Some(dir) = system_knowhow_dir else {
        log_legacy!("[OAuthRegistry] system-knowhow dir unavailable, no known providers");
        return Vec::new();
    };
    let path = dir.join(REGISTRY_FILE);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            log_legacy!(
                "[OAuthRegistry] could not read {}: {}. No known providers.",
                path.display(),
                e
            );
            return Vec::new();
        }
    };
    match serde_json::from_str::<RegistryFile>(&raw) {
        Ok(file) => file.providers,
        Err(e) => {
            log_legacy!(
                "[OAuthRegistry] {} is not valid provider JSON: {}. No known providers.",
                path.display(),
                e
            );
            Vec::new()
        }
    }
}

/// INVARIANT: The row for `provider`, matched case-insensitively on the id.
///
/// A *derived provider* name (a second, narrowly-scoped connection under its own
/// name, running on a known provider's endpoints) is deliberately NOT resolved
/// here: it is not the base provider, and guessing which one it meant from its
/// spelling is what the Connect form now asks the user instead. The caller
/// resolves a derived name by looking up the base provider the user picked.
pub fn find_provider(
    system_knowhow_dir: Option<&Path>,
    provider: &str,
) -> Option<OAuthProviderRow> {
    let wanted = provider.trim().to_lowercase();
    if wanted.is_empty() {
        return None;
    }
    load_providers(system_knowhow_dir)
        .into_iter()
        .find(|p| p.id.to_lowercase() == wanted)
}

/// INVARIANT: The row whose authorization endpoint sits on `auth_url`'s host.
///
/// How a *derived provider* finds its base provider: its credential carries the
/// base provider's endpoints, whatever name it was saved under. Compared on the
/// parsed host, never the URL text, so a path or query cannot impersonate a row.
pub fn row_for_auth_url<'a>(
    rows: &'a [OAuthProviderRow],
    auth_url: &str,
) -> Option<&'a OAuthProviderRow> {
    let host = reqwest::Url::parse(auth_url.trim())
        .ok()?
        .host_str()?
        .to_lowercase();
    rows.iter().find(|row| {
        reqwest::Url::parse(&row.auth_url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_lowercase))
            .is_some_and(|row_host| row_host == host)
    })
}

#[cfg(test)]
#[path = "oauth_registry_tests.rs"]
mod tests;
