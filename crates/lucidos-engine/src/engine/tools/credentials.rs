use super::super::LucidosEngine;
use crate::core::oauth;
use crate::core::CredentialStore;

/// Sentinel prefix on a tool result that the agentic loop strips off and
/// re-emits as a `CredentialPromptRequested` SSE event for the frontend modal.
pub(crate) const CREDENTIAL_REQUEST_PREFIX: &str = "[CREDENTIAL_REQUEST]";

/// Wrap a pre-built credential-request JSON value in the sentinel prefix.
/// The JSON must be built via `serde_json` (not `format!`-interpolated) so
/// newlines, quotes, and backslashes in the inputs are escaped correctly —
/// the agentic loop strips the prefix and parses the rest as JSON.
pub(crate) fn credential_request_envelope(payload: serde_json::Value) -> String {
    format!("{CREDENTIAL_REQUEST_PREFIX}{payload}")
}

/// Convenience wrapper for the common 4-field credential-request shape.
pub(crate) fn credential_request_payload(
    service: &str,
    prompt: &str,
    base_url: &str,
    auth_type: &str,
) -> String {
    credential_request_with_defaults(
        service,
        prompt,
        base_url,
        auth_type,
        serde_json::Map::new(),
        None,
    )
}

/// Build the enveloped credential-request payload, optionally attaching an
/// oauth `defaults` block (endpoint URLs + scopes the modal pre-fills) and an
/// `env_var_name` the modal pre-fills into its custom-env-var-name field. An empty
/// `defaults` map omits the block entirely, so the modal treats it as a custom
/// provider and expands the endpoint section for manual entry. A `None`/blank
/// `env_var_name` omits the field, so the modal starts empty (default
/// `CRED_<NAME>` injection).
pub(crate) fn credential_request_with_defaults(
    service: &str,
    prompt: &str,
    base_url: &str,
    auth_type: &str,
    defaults: serde_json::Map<String, serde_json::Value>,
    env_var_name: Option<&str>,
) -> String {
    let mut payload = serde_json::json!({
        "service": service,
        "prompt": prompt,
        "base_url": base_url,
        "auth_type": auth_type,
    });
    if !defaults.is_empty() {
        payload["defaults"] = serde_json::Value::Object(defaults);
    }
    if let Some(name) = env_var_name.map(str::trim).filter(|s| !s.is_empty()) {
        payload["env_var_name"] = serde_json::Value::String(name.to_string());
    }
    credential_request_envelope(payload)
}

/// The service name a `request_credential` call actually writes under.
///
/// For `oauth_client` this is NOT the agent's `service_name` verbatim: it is
/// lowercased and any leading `oauth:` is stripped, so an agent that still says
/// `oauth:google` (the spelling the chat system prompt used for as long as the
/// tool existed) lands on the same row as one that says `google`. See
/// `oauth::client_provider_name`. Every other auth type keeps its name exactly,
/// because that name is what `CRED_<NAME>` env injection and `apis.json` service
/// lookups key off.
fn requested_service_name(service_name: &str, auth_type: &str) -> String {
    if auth_type == "oauth_client" {
        oauth::client_provider_name(service_name)
    } else {
        service_name.to_string()
    }
}

/// Collect the optional oauth endpoint + scopes args an agent passes (looked up
/// from `system-knowhow/oauth-providers.md`) into a `defaults` map. Blank/absent
/// args are dropped so they never pre-fill an empty field.
fn oauth_defaults_from_args(
    args: &serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    let mut defaults = serde_json::Map::new();
    for key in [
        "auth_url",
        "token_url",
        "userinfo_url",
        "userinfo_method",
        "authorize_params",
        "scopes",
        "redirect_uri",
    ] {
        if let Some(v) = args[key].as_str().map(str::trim).filter(|s| !s.is_empty()) {
            defaults.insert(key.to_string(), serde_json::Value::String(v.to_string()));
        }
    }
    defaults
}

impl LucidosEngine {
    /// `thread_id` + `device_id` are here for `connect_oauth_account`: the
    /// authorization page is opened by the user's own client (see
    /// [`Self::request_navigation`]), so the flow needs to know which thread to
    /// emit on and which device is actually in front of the user.
    pub(crate) async fn execute_credential_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
        device_id: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match name {
            "request_credential" => {
                let service_name = args["service_name"].as_str().unwrap_or("");
                let prompt = args["prompt"].as_str().unwrap_or("");
                let base_url = args["base_url"].as_str().unwrap_or("");
                let auth_type = args["auth_type"].as_str().unwrap_or("api_key");

                if service_name.is_empty() || prompt.is_empty() || base_url.is_empty() {
                    return Ok("Error: service_name, prompt, and base_url are required".to_string());
                }

                let service_name = requested_service_name(service_name, auth_type);
                let service_name = service_name.as_str();

                // Optional custom env var name to pre-fill the modal with. Validate
                // it the same way the Settings UI does (the API boundary re-validates
                // on submit, but rejecting here gives the agent a precise error
                // instead of silently pre-filling a name the user can't save).
                let env_var_name = args["env_var_name"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                if let Some(name) = env_var_name {
                    if let Err(rejection) = crate::core::environment_variables::validate_name(name)
                    {
                        return Ok(format!("Error: {}", rejection.message(name)));
                    }
                }

                // Check if credential already exists. Typed, because an
                // `oauth_client` row is the one type allowed to share a name
                // with another credential: a bare-name check would report a
                // Google API key as "already configured" when the agent is
                // asking for the Google app registration, and never open the
                // modal.
                let existing = CredentialStore::get_typed(
                    &self.pool,
                    service_name,
                    crate::core::AuthType::parse(auth_type),
                )
                .await;
                if let Ok(Some(_)) = existing {
                    return Ok(format!(
                        "Credentials for '{}' are already configured. You can proceed with API requests.",
                        service_name
                    ));
                }

                // For oauth_client, the agent may pass endpoint URLs (looked up in
                // the oauth-providers knowhow) so the modal pre-fills them instead
                // of demanding the user type Google's own endpoints by hand.
                let defaults = if auth_type == "oauth_client" {
                    oauth_defaults_from_args(args)
                } else {
                    serde_json::Map::new()
                };

                Ok(credential_request_with_defaults(
                    service_name,
                    prompt,
                    base_url,
                    auth_type,
                    defaults,
                    env_var_name,
                ))
            }
            "connect_oauth_account" => {
                let provider = args["provider"].as_str().unwrap_or("").to_lowercase();
                let scopes = args["scopes"].as_str().unwrap_or("");

                if provider.is_empty() || scopes.is_empty() {
                    return Ok("Error: provider and scopes are required".to_string());
                }

                // Check if client credentials exist for this provider
                let cred_service = oauth::client_provider_name(&provider);
                if CredentialStore::get_oauth_client(&self.pool, &cred_service)
                    .await?
                    .is_none()
                {
                    // No client credentials yet — open the modal. Forward any
                    // endpoints the agent looked up in the oauth-providers knowhow
                    // (so a derived name like "ghealth" pre-fills Google's URLs),
                    // and seed the default scopes from the requested scopes.
                    let str_arg = |key: &str| {
                        args[key]
                            .as_str()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                    };
                    let overrides = oauth::OAuthClientOverrides {
                        base_url: str_arg("base_url"),
                        auth_url: str_arg("auth_url"),
                        token_url: str_arg("token_url"),
                        userinfo_url: str_arg("userinfo_url"),
                        userinfo_method: str_arg("userinfo_method"),
                        authorize_params: str_arg("authorize_params"),
                        scopes: Some(scopes.to_string()),
                        redirect_uri: str_arg("redirect_uri"),
                    };
                    return Ok(credential_request_envelope(oauth::oauth_client_request(
                        &provider, &overrides,
                    )));
                }

                // The engine does NOT open the browser. It asks the user's own
                // device to, so the authorization page lands wherever they
                // configured links to open. Shelling out to macOS `open` here
                // ignored that preference and did nothing at all on Linux.
                // `purpose: "oauth"` is what lets the client close the in-app
                // browser panel again once the flow lands, instead of leaving
                // the user on a dead callback page inside the app. See
                // `oauthAuthPanelUrl` in store/actions/oauth.ts.
                let open_auth_url = async |auth_url: &str| {
                    self.request_navigation(
                        &serde_json::json!({
                            "target": "url",
                            "url": auth_url,
                            "purpose": "oauth",
                        }),
                        thread_id,
                        device_id,
                    )
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                        format!("could not open the authorization page: {e}").into()
                    })
                };
                let (email, _display_name, _merged_scopes) = oauth::run_oauth_flow(
                    &self.pool,
                    &self.event_bus,
                    &provider,
                    scopes,
                    // Same device the authorization page was handed to: it is the
                    // one to bring back to the front when the flow lands.
                    self.turn_device_actor(device_id).await,
                    open_auth_url,
                )
                .await?;

                match email.as_deref() {
                    Some(email) => Ok(format!(
                        "Successfully connected {provider} account ({email})."
                    )),
                    // The provider gave no userinfo endpoint, or it did not
                    // return an email. Say that, rather than reporting the
                    // account as literally named "unknown" and sending the agent
                    // off to curl the provider's API to find out who it is.
                    None => Ok(format!(
                        "Successfully connected the {provider} account. The provider did not \
                         report which account it is (no userinfo endpoint configured for \
                         {provider}, or it returned no email), so do not guess or go looking \
                         for one: just say the {provider} account is connected."
                    )),
                }
            }
            _ => Ok(format!("Unknown credential tool: {}", name)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_payload(s: &str) -> serde_json::Value {
        let json_part = s
            .strip_prefix(CREDENTIAL_REQUEST_PREFIX)
            .expect("missing CREDENTIAL_REQUEST_PREFIX");
        serde_json::from_str(json_part).expect("payload must be valid JSON")
    }

    #[test]
    fn payload_is_valid_json_with_multiline_prompt() {
        let prompt = "1. Open dashboard\n2. Create API key\n3. Paste it below";
        let result =
            credential_request_payload("binance", prompt, "https://api.binance.com", "api_key");
        let parsed = parse_payload(&result);
        assert_eq!(parsed["service"], "binance");
        assert_eq!(parsed["prompt"], prompt);
        assert_eq!(parsed["base_url"], "https://api.binance.com");
        assert_eq!(parsed["auth_type"], "api_key");
    }

    #[test]
    fn payload_escapes_quotes_and_backslashes_in_prompt() {
        let prompt = r#"Use the "API Key" field, escape backslashes like \n correctly"#;
        let result =
            credential_request_payload("svc", prompt, "https://api.example.com", "api_key");
        let parsed = parse_payload(&result);
        assert_eq!(parsed["prompt"], prompt);
    }

    #[test]
    fn payload_handles_special_chars_in_other_fields() {
        let result = credential_request_payload(
            r#"weird"service"#,
            "prompt",
            r#"https://example.com/path with "quotes""#,
            "api_key",
        );
        let parsed = parse_payload(&result);
        assert_eq!(parsed["service"], r#"weird"service"#);
        assert_eq!(
            parsed["base_url"],
            r#"https://example.com/path with "quotes""#
        );
    }

    #[test]
    fn basic_payload_has_no_defaults_block() {
        let result =
            credential_request_payload("svc", "prompt", "https://api.example.com", "api_key");
        let parsed = parse_payload(&result);
        assert!(
            parsed.get("defaults").is_none(),
            "non-oauth payloads must not carry a defaults block: {parsed}"
        );
    }

    #[test]
    fn oauth_payload_attaches_supplied_endpoint_defaults() {
        // request_credential with oauth_client + endpoints the agent looked up in
        // the oauth-providers knowhow → the modal pre-fills (and stops requiring)
        // the endpoint fields for a derived provider name like "oauth:ghealth".
        let mut defaults = serde_json::Map::new();
        defaults.insert(
            "auth_url".to_string(),
            serde_json::json!("https://accounts.google.com/o/oauth2/v2/auth"),
        );
        defaults.insert(
            "token_url".to_string(),
            serde_json::json!("https://oauth2.googleapis.com/token"),
        );
        let result = credential_request_with_defaults(
            "oauth:ghealth",
            "Enter your OAuth client credentials.",
            "https://healthcare.googleapis.com",
            "oauth_client",
            defaults,
            None,
        );
        let parsed = parse_payload(&result);
        assert_eq!(parsed["service"], "oauth:ghealth");
        assert_eq!(parsed["auth_type"], "oauth_client");
        assert_eq!(
            parsed["defaults"]["auth_url"],
            "https://accounts.google.com/o/oauth2/v2/auth"
        );
        assert_eq!(
            parsed["defaults"]["token_url"],
            "https://oauth2.googleapis.com/token"
        );
    }

    #[test]
    fn empty_defaults_map_omits_the_block() {
        let result = credential_request_with_defaults(
            "svc",
            "prompt",
            "https://api.example.com",
            "oauth_client",
            serde_json::Map::new(),
            None,
        );
        let parsed = parse_payload(&result);
        assert!(
            parsed.get("defaults").is_none(),
            "an empty defaults map must omit the block entirely: {parsed}"
        );
    }

    #[test]
    fn env_var_name_is_attached_when_supplied() {
        let result = credential_request_with_defaults(
            "apple",
            "Enter your app-specific password.",
            "https://api.apple.com",
            "password",
            serde_json::Map::new(),
            Some("APPLE_PASSWORD"),
        );
        let parsed = parse_payload(&result);
        assert_eq!(parsed["env_var_name"], "APPLE_PASSWORD");
    }

    /// The exact 2026-08-05 call that produced two Dropbox credentials:
    /// `request_credential(service_name: "dropbox", auth_type: "oauth_client")`.
    /// Both spellings an agent might use have to reach ONE row, or the user is
    /// back to holding two credentials for one provider.
    #[test]
    fn oauth_client_requests_normalize_to_the_bare_provider() {
        assert_eq!(requested_service_name("dropbox", "oauth_client"), "dropbox");
        // The spelling the system prompt taught for as long as the tool existed.
        assert_eq!(
            requested_service_name("oauth:dropbox", "oauth_client"),
            "dropbox"
        );
    }

    /// Normalization is scoped to `oauth_client`. Every other type keeps its
    /// name verbatim: it is what `CRED_<NAME>` injection and `apis.json` service
    /// lookups resolve, so rewriting it would break live scripts.
    #[test]
    fn non_oauth_credentials_keep_their_name_verbatim() {
        for auth_type in ["api_key", "bearer", "basic", "password", "email_password"] {
            assert_eq!(
                requested_service_name("dropbox", auth_type),
                "dropbox",
                "{auth_type} must not be renamed"
            );
        }
        assert_eq!(
            requested_service_name("email:work", "email_password"),
            "email:work"
        );
    }

    /// The envelope the modal receives carries the canonical name, so the row
    /// the user saves is the row the OAuth flow later reads.
    #[test]
    fn the_modal_payload_carries_the_normalized_oauth_name() {
        let result = credential_request_with_defaults(
            &requested_service_name("Dropbox", "oauth_client"),
            "Paste your Dropbox App key into Client ID.",
            "https://api.dropboxapi.com",
            "oauth_client",
            serde_json::Map::new(),
            None,
        );
        assert_eq!(parse_payload(&result)["service"], "dropbox");
    }

    #[test]
    fn blank_env_var_name_omits_the_field() {
        // Whitespace-only / empty names must not pre-fill the modal — the
        // credential then injects under the default CRED_<NAME> form only.
        for name in [None, Some(""), Some("   ")] {
            let result = credential_request_with_defaults(
                "svc",
                "prompt",
                "https://api.example.com",
                "api_key",
                serde_json::Map::new(),
                name,
            );
            let parsed = parse_payload(&result);
            assert!(
                parsed.get("env_var_name").is_none(),
                "a blank env_var_name ({name:?}) must omit the field: {parsed}"
            );
        }
    }
}
