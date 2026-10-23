//! The *OAuth provider registry*: the rows behind the Connect form's autofill.
//!
//! Reads `system-knowhow/oauth-providers.json`, the engine-shipped data file
//! that lists every provider whose OAuth endpoints Lucidos knows. Adding a
//! provider is a data edit, never an engine change, which is what keeps
//! CLAUDE.md's "no provider-specific instructions in code" true: **no provider
//! name appears in this module**, and `oauth_registry_names_no_provider` pins
//! that.
//!
//! Until 2026-08-07 the rows lived only in a markdown table in
//! `oauth-providers.md`, readable by the *Lucidos Agent* and by nobody else. So
//! the Connect button on Settings > Accounts asked for a provider's
//! authorization and token URLs by hand while the engine was shipping them on
//! disk, and a user who left the endpoint fields blank (nothing enforced them)
//! saved a client that could only ever fail with "Missing auth_url in OAuth
//! credentials". The prose that used to sit around that table stays in the
//! markdown, which no longer restates a single row.
//!
//! **The registry never drives a flow.** It prefills a credential at write time
//! and nothing more: `prepare_oauth_flow` still reads endpoints out of the
//! stored credential, so a credential keeps fully describing its own
//! authorization and a later registry edit cannot silently change one that
//! already works.
//!
//! **Absence is a supported state.** `system_knowhow_dir` is `Option` (an
//! unstaged packaged bundle resolves to `None`, see
//! [`crate::core::resolve_system_knowhow_dir`]), and the file may be missing or
//! malformed. All three degrade to an empty registry with one warning: the
//! quick-provider buttons render nothing, the typed-name path still works, and
//! the form asks for the endpoints exactly as it did before.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The file, relative to the resolved system-knowhow directory.
const REGISTRY_FILE: &str = "oauth-providers.json";

/// One known provider.
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
    /// The provider id, lowercase. This is the credential's service name and the
    /// connected account's provider.
    pub id: String,
    /// What the UI calls it.
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
    /// `"public"` (leave the client secret blank, PKCE) or `"confidential"` (a
    /// secret is required). Advisory copy for the form: the engine still derives
    /// the actual client type from whether the saved credential carries a
    /// secret, per the *OAuth client type* glossary entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console_url: Option<String>,
    /// One or two sentences on what to register with the provider before the
    /// Client ID exists. Rendered verbatim by the form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_hint: Option<String>,
    /// What the provider needs enabled on its side, where that is a separate
    /// step from requesting the scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions_hint: Option<String>,
}

/// The parsed file. Its `note` key is prose for a human opening the JSON and is
/// deliberately not deserialized: nothing serves it.
#[derive(Debug, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    providers: Vec<OAuthProviderRow>,
}

/// Every known provider, in file order.
///
/// Returns an empty vec when the directory is unavailable, the file is absent,
/// or the JSON does not parse. Each of those is logged once per call and is a
/// supported state, not an error: autofill is an accelerator, and the manual
/// path it accelerates still exists.
pub fn load_providers(system_knowhow_dir: Option<&Path>) -> Vec<OAuthProviderRow> {
    let Some(dir) = system_knowhow_dir else {
        log!("[OAuthRegistry] system-knowhow dir unavailable, no known providers");
        return Vec::new();
    };
    let path = dir.join(REGISTRY_FILE);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            log!(
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
            log!(
                "[OAuthRegistry] {} is not valid provider JSON: {}. No known providers.",
                path.display(),
                e
            );
            Vec::new()
        }
    }
}

/// The row for `provider`, matched case-insensitively on the id.
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

#[cfg(test)]
#[path = "oauth_registry_tests.rs"]
mod tests;
