use super::super::LucidosEngine;
use crate::core::oauth;
use crate::core::oauth_registry;
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

/// A userinfo field the provider actually answered.
///
/// A present-but-blank field is the same as an absent one, and it does reach
/// here: userinfo parsing takes whatever string the JSON carries, so a provider
/// answering `"name": ""` would otherwise produce "the account for ." and one
/// answering `"email": ""` would produce "account ()".
fn reported(field: Option<&String>) -> Option<&str> {
    field.map(String::as_str).filter(|s| !s.trim().is_empty())
}

/// Connected, and whose account it is. Says nothing about scopes.
fn connected_sentence(provider: &str, outcome: &oauth::OAuthFlowOutcome) -> String {
    match (
        reported(outcome.email.as_ref()),
        reported(outcome.display_name.as_ref()),
    ) {
        (Some(email), _) => format!("Successfully connected {provider} account ({email})."),
        // No email, but the provider did say who this is. Naming the account
        // beats reporting it as unidentified, which is what this branch did for
        // as long as the display name was dropped on the floor here.
        (None, Some(name)) => format!(
            "Successfully connected the {provider} account for {name}. The provider reported no \
             email address for it, so refer to it by that name and do not go looking for one."
        ),
        // The provider gave no userinfo endpoint, or it returned neither field.
        // Say that, rather than reporting the account as literally named
        // "unknown" and sending the agent off to curl the provider's API to
        // find out who it is.
        (None, None) => format!(
            "Successfully connected the {provider} account. The provider did not report which \
             account it is (no userinfo endpoint configured for {provider}, or it returned no \
             email), so do not guess or go looking for one."
        ),
    }
}

/// What the authorization asked for and did not get, and what to do about it.
///
/// The per-provider half comes from the *OAuth provider registry* row, never
/// from a branch on the provider name: which console to open and what has to be
/// enabled there is data, and the same data already drives the credential
/// form's help block. A provider with no row (or an install with no staged
/// system-knowhow) still gets the generic instruction, which is the part that
/// actually unblocks the user.
fn scope_shortfall_sentences(
    missing: &[String],
    row: Option<&oauth_registry::OAuthProviderRow>,
) -> String {
    let noun = if missing.len() == 1 {
        "scope"
    } else {
        "scopes"
    };
    let pronoun = if missing.len() == 1 { "it" } else { "them" };
    let mut text = format!(
        "The provider did not grant everything that was requested. Missing {noun}: {}. The \
         account is connected and works for what it did get, but any call needing {pronoun} will \
         fail. Enable {pronoun} for this app in the provider's own console, then RECONNECT the \
         account (the Reconnect button on Settings > Accounts, or another connect_oauth_account \
         call): neither a token refresh nor the existing grant picks up a newly enabled scope.",
        missing.join(", ")
    );
    let Some(row) = row else { return text };
    if let Some(hint) = row.permissions_hint.as_deref() {
        text.push(' ');
        text.push_str(hint);
    }
    if let Some(url) = row.console_url.as_deref() {
        let label = row.console_label.as_deref().unwrap_or("Console");
        text.push_str(&format!(" {label}: {url}"));
    }
    text
}

/// The agent-facing result of a completed authorization.
///
/// A full grant reads exactly as it always did. A partial one still reports the
/// connection (it happened, and refusing to say so would send the agent back
/// through a flow that worked) but names the shortfall, because the alternative
/// is what shipped until now: an unqualified success for an account holding one
/// of the four scopes it asked for, with the Accounts panel as the only surface
/// that knew.
fn connect_result_message(
    provider: &str,
    outcome: &oauth::OAuthFlowOutcome,
    row: Option<&oauth_registry::OAuthProviderRow>,
) -> String {
    let missing =
        oauth::missing_requested_scopes(&outcome.requested_scopes, &outcome.granted_scopes);
    let mut message = connected_sentence(provider, outcome);
    if missing.is_empty() {
        // Nothing follows, so an unidentified account gets its closing
        // instruction here. With a shortfall the closing instruction is the
        // reconnect one instead, and "just say it is connected" would
        // contradict it.
        if reported(outcome.email.as_ref()).is_none()
            && reported(outcome.display_name.as_ref()).is_none()
        {
            message.push_str(&format!(" Just say the {provider} account is connected."));
        }
        return message;
    }
    message.push(' ');
    message.push_str(&scope_shortfall_sentences(&missing, row));
    message
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
                    //
                    // Anything the agent did NOT pass falls back to the *OAuth
                    // provider registry* row, which is the same data it would
                    // have read out of the knowhow. Passing wins, so a derived
                    // name carrying the base provider's URLs behaves exactly as
                    // before; the fallback only rescues the case where the agent
                    // skipped the lookup, which used to drop the user into a
                    // blank endpoint form.
                    let row = oauth_registry::find_provider(
                        self.system_knowhow_dir(),
                        &oauth::client_provider_name(&provider),
                    );
                    let from_row = row
                        .as_ref()
                        .map(oauth::OAuthClientOverrides::from_registry)
                        .unwrap_or_default();
                    let overrides = oauth::OAuthClientOverrides {
                        base_url: str_arg("base_url").or(from_row.base_url),
                        auth_url: str_arg("auth_url").or(from_row.auth_url),
                        token_url: str_arg("token_url").or(from_row.token_url),
                        userinfo_url: str_arg("userinfo_url").or(from_row.userinfo_url),
                        userinfo_method: str_arg("userinfo_method").or(from_row.userinfo_method),
                        authorize_params: str_arg("authorize_params").or(from_row.authorize_params),
                        scopes: Some(scopes.to_string()),
                        redirect_uri: str_arg("redirect_uri").or(from_row.redirect_uri),
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
                // `oauthAuthFlow` in store/actions/oauth.ts.
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
                let outcome = oauth::run_oauth_flow(
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

                // The registry row supplies the per-provider half of a shortfall
                // message (which console to open, what has to be enabled there).
                // Looked up the same way the no-credentials branch above does,
                // and absent registry rows are a supported state.
                let row = oauth_registry::find_provider(
                    self.system_knowhow_dir(),
                    &oauth::client_provider_name(&provider),
                );
                Ok(connect_result_message(&provider, &outcome, row.as_ref()))
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

    // ─── What the agent is told a connection actually got ──────────────────
    //
    // Until 2026-08-07 this said "Successfully connected {provider} account"
    // whatever came back, so a Dropbox app whose App Console had not been
    // submitted connected an account holding one of its four requested scopes
    // and reported it as done. The Accounts panel drew the shortfall the whole
    // time; the agent had no way to see it.

    fn outcome(
        email: Option<&str>,
        display_name: Option<&str>,
        granted: &str,
        requested: &str,
    ) -> oauth::OAuthFlowOutcome {
        oauth::OAuthFlowOutcome {
            email: email.map(str::to_string),
            display_name: display_name.map(str::to_string),
            granted_scopes: granted.to_string(),
            requested_scopes: requested.to_string(),
        }
    }

    /// A registry row with only the fields a shortfall message reads. Named for
    /// nothing shipped, so the source scan below stays meaningful.
    fn row_with_console() -> oauth_registry::OAuthProviderRow {
        oauth_registry::OAuthProviderRow {
            id: "acme".to_string(),
            label: "Acme".to_string(),
            base_url: "https://api.acme.test".to_string(),
            auth_url: "https://acme.test/authorize".to_string(),
            token_url: "https://api.acme.test/token".to_string(),
            userinfo_url: None,
            userinfo_method: None,
            authorize_params: None,
            redirect_uri: None,
            client_type: None,
            console_label: Some("Acme Developer Console".to_string()),
            console_url: Some("https://acme.test/apps".to_string()),
            setup_hint: None,
            permissions_hint: Some("Tick the permission and press Submit.".to_string()),
        }
    }

    #[test]
    fn a_full_grant_reports_exactly_what_it_always_did() {
        // Pinned character for character: this string is what every working
        // connection has read since the tool existed, and a shortfall report is
        // not a licence to reword the success case.
        assert_eq!(
            connect_result_message(
                "acme",
                &outcome(Some("user@example.com"), None, "read write", "read write"),
                Some(&row_with_console()),
            ),
            "Successfully connected acme account (user@example.com)."
        );
    }

    #[test]
    fn a_partial_grant_names_every_missing_scope_and_says_reconnect() {
        let message = connect_result_message(
            "acme",
            &outcome(
                Some("user@example.com"),
                None,
                "account_info.read",
                "files.content.write files.metadata.read account_info.read",
            ),
            None,
        );
        assert!(
            message.starts_with("Successfully connected acme account (user@example.com)."),
            "the account did connect and the message must still say so: {message}"
        );
        for scope in ["files.content.write", "files.metadata.read"] {
            assert!(message.contains(scope), "{scope} must be named: {message}");
        }
        assert!(
            !message.contains("Missing scope: account_info.read"),
            "a granted scope must not be reported as missing: {message}"
        );
        assert!(
            message.contains("RECONNECT"),
            "the fix is a reconnect, and a refresh will not do it: {message}"
        );
        assert!(
            message.contains("refresh"),
            "the message must say why a refresh does not help: {message}"
        );
    }

    #[test]
    fn a_shortfall_carries_the_registry_row_and_not_a_hardcoded_provider_rule() {
        let message = connect_result_message(
            "acme",
            &outcome(Some("user@example.com"), None, "", "files.content.write"),
            Some(&row_with_console()),
        );
        assert!(
            message.contains("Tick the permission and press Submit."),
            "the per-provider sentence comes from the registry: {message}"
        );
        assert!(
            message.contains("Acme Developer Console: https://acme.test/apps"),
            "the console link is what makes the instruction actionable: {message}"
        );
    }

    #[test]
    fn a_shortfall_with_no_registry_row_still_says_what_to_do() {
        // A derived provider, or an install with no staged system-knowhow. The
        // generic instruction is the half that unblocks the user, so it cannot
        // depend on the row being there.
        let message = connect_result_message(
            "ghealth",
            &outcome(
                None,
                None,
                "",
                "https://www.googleapis.com/auth/cloud-healthcare",
            ),
            None,
        );
        assert!(message.contains("https://www.googleapis.com/auth/cloud-healthcare"));
        assert!(message.contains("RECONNECT"));
    }

    #[test]
    fn an_account_with_a_name_but_no_email_is_named_rather_than_unknown() {
        // `display_name` used to be bound and dropped, so a provider that
        // reports a name and no email (Dropbox nests one as
        // `name.display_name`) was reported as unidentifiable.
        let message = connect_result_message(
            "acme",
            &outcome(None, Some("Ada Lovelace"), "read", "read"),
            None,
        );
        assert!(
            message.contains("Ada Lovelace"),
            "the provider said whose account this is: {message}"
        );
        assert!(
            !message.contains("did not report which account"),
            "it did report which account: {message}"
        );
    }

    #[test]
    fn a_blank_userinfo_field_counts_as_not_reported() {
        // Userinfo parsing takes whatever string the JSON carries, so a provider
        // answering `"email": ""` or `"name": ""` reaches here as Some(""). Read
        // literally that renders "account ()" and "the account for .".
        let message =
            connect_result_message("acme", &outcome(Some("  "), Some(""), "read", "read"), None);
        assert!(
            message.contains("did not report which account it is"),
            "a blank field is not an answer: {message}"
        );
        assert!(!message.contains("account ()"));
        assert!(!message.contains("account for ."));
    }

    #[test]
    fn an_unidentified_account_keeps_its_do_not_go_looking_instruction() {
        let message = connect_result_message("acme", &outcome(None, None, "read", "read"), None);
        assert!(message.contains("do not guess or go looking for one"));
        assert!(message.contains("Just say the acme account is connected."));
    }

    #[test]
    fn an_unidentified_account_short_of_a_scope_is_not_told_to_say_it_is_fine() {
        // The two closing instructions contradict each other, so only one runs.
        let message =
            connect_result_message("acme", &outcome(None, None, "read", "read write"), None);
        assert!(
            !message.contains("Just say the acme account is connected."),
            "a shortfall's closing instruction is the reconnect one: {message}"
        );
        assert!(message.contains("RECONNECT"));
    }

    #[test]
    fn the_result_message_names_no_provider() {
        // CLAUDE.md bans provider-specific instructions in engine code, and the
        // per-provider half of a shortfall message is exactly the kind of thing
        // that invites one. It comes from the registry row instead, so a
        // literal here would be a second copy free to drift from the JSON.
        //
        // Scoped to the three message builders rather than the whole file: the
        // rest of the module legitimately quotes a provider name in comments
        // about historical credential spellings, and the tests below name
        // providers on purpose.
        let source = include_str!("credentials.rs");
        let builders = source
            .split("fn connected_sentence")
            .nth(1)
            .and_then(|rest| rest.split("impl LucidosEngine").next())
            .expect("the message builders sit between their first fn and the impl block");
        assert!(
            builders.contains("fn connect_result_message"),
            "the scanned slice must cover every message builder"
        );
        let dir = crate::paths::repo_root()
            .expect("repo root resolves under cargo test")
            .join("system-knowhow");
        let rows = oauth_registry::load_providers(Some(dir.as_path()));
        assert!(!rows.is_empty(), "the shipped registry must list providers");
        for row in rows {
            assert!(
                !builders.to_lowercase().contains(&row.id.to_lowercase()),
                "the connect result message names the provider '{}'. Per-provider wording \
                 belongs in system-knowhow/oauth-providers.json.",
                row.id
            );
        }
    }
}
