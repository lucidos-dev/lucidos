use super::super::LucidosEngine;
use crate::core::credentials::{credential_scope_covers, normalized_base_urls, Credential};
use crate::core::oauth;
use crate::core::oauth_registry;
use crate::core::AuthType;
use crate::core::CredentialStore;
use crate::engine::tools::ToolDispatchExt;

pub(crate) const CREDENTIAL_REQUEST_PREFIX: &str = "[CREDENTIAL_REQUEST]";

pub(crate) fn credential_request_envelope(payload: serde_json::Value) -> String {
    format!("{CREDENTIAL_REQUEST_PREFIX}{payload}")
}

pub(crate) fn credential_request_payload(
    service: &str,
    prompt: &str,
    base_urls: &[String],
    auth_type: &str,
) -> String {
    credential_request_with_defaults(
        service,
        prompt,
        base_urls,
        auth_type,
        serde_json::Map::new(),
        None,
    )
}

pub(crate) fn credential_request_with_defaults(
    service: &str,
    prompt: &str,
    base_urls: &[String],
    auth_type: &str,
    defaults: serde_json::Map<String, serde_json::Value>,
    env_var_name: Option<&str>,
) -> String {
    let mut payload = serde_json::json!({
        "service": service,
        "prompt": prompt,
        "auth_type": auth_type,
    });
    if !base_urls.is_empty() {
        payload["base_urls"] = serde_json::Value::from(base_urls);
    }
    if !defaults.is_empty() {
        payload["defaults"] = serde_json::Value::Object(defaults);
    }
    if let Some(name) = env_var_name.map(str::trim).filter(|s| !s.is_empty()) {
        payload["env_var_name"] = serde_json::Value::String(name.to_string());
    }
    credential_request_envelope(payload)
}

fn scope_strings(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(one) => vec![one.clone()],
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn requested_scope(args: &serde_json::Value) -> Result<Vec<String>, String> {
    let mut raw = scope_strings(&args["base_urls"]);
    raw.extend(scope_strings(&args["base_url"]));
    normalized_base_urls(raw)
}

fn missing_field_refusal(
    service_name: &str,
    prompt: &str,
    base_urls: &[String],
    auth_type: &str,
) -> Option<&'static str> {
    let needs_base_url = AuthType::parse(auth_type) != AuthType::Secret;
    if service_name.is_empty() || prompt.is_empty() || (needs_base_url && base_urls.is_empty()) {
        return Some(
            "Error: service_name and prompt are required, and so is base_urls for every \
             auth_type but 'secret'",
        );
    }
    None
}

async fn existing_credential(
    pool: &sqlx::PgPool,
    service_name: &str,
    auth_type: &str,
) -> Result<Option<Credential>, sqlx::Error> {
    if AuthType::parse(auth_type) == AuthType::OauthClient {
        CredentialStore::get_typed(pool, service_name, AuthType::OauthClient).await
    } else {
        CredentialStore::get(pool, service_name).await
    }
}

fn answers_request(stored: AuthType, requested: AuthType) -> bool {
    let token = |t| matches!(t, AuthType::ApiKey | AuthType::Bearer);
    match stored {
        AuthType::EmailPassword | AuthType::Unknown => false,
        AuthType::ApiKey | AuthType::Bearer => token(requested),
        other => other == requested,
    }
}

fn name_taken_by(existing: &Credential, requested: AuthType) -> String {
    format!(
        "'{}' already names a credential of type {}, not the {requested} that \
         was asked for. Use the stored one if it serves, or ask for this one \
         under another service name.",
        existing.service_name, existing.auth_type
    )
}

fn hosts_outside_scope(existing: &Credential, requested: &[String]) -> Vec<String> {
    if existing.auth_type == AuthType::Secret {
        return Vec::new();
    }
    requested
        .iter()
        .filter(|url| !credential_scope_covers(&existing.base_urls, url))
        .cloned()
        .collect()
}

fn already_configured(service_name: &str) -> String {
    format!(
        "Credentials for '{service_name}' are already configured. \
         You can proceed with API requests."
    )
}

fn widen_scope_request(existing: &Credential, adding: &[String]) -> String {
    let mut base_urls = existing.base_urls.clone();
    base_urls.extend(adding.iter().cloned());
    credential_request_envelope(serde_json::json!({
        "service": existing.service_name,
        "prompt": format!(
            "This lets the '{}' secret be sent to {}. It is stored already and \
             does not change, so Save is all this needs.",
            existing.service_name,
            adding.join(", "),
        ),
        "auth_type": existing.auth_type.to_string(),
        "base_urls": base_urls,
        "existing_credential_id": existing.id,
        "adding_base_urls": adding,
    }))
}

pub(crate) fn host_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
}

fn overrides_builtin_proxy(requested: AuthType, base_urls: &[String], builtin_base: &str) -> bool {
    if !matches!(requested, AuthType::ApiKey | AuthType::Bearer) {
        return true;
    }
    let builtin_host = host_of(builtin_base);
    builtin_host.is_none() || base_urls.iter().any(|url| host_of(url) != builtin_host)
}

fn served_by_builtin_proxy(service_name: &str, proxy: &str, base_url: &str) -> String {
    let path_hint = if crate::api::proxy_builtin::base_includes_v1(base_url) {
        " The base already ends in /v1, so send '/models', not '/v1/models'."
    } else {
        ""
    };
    format!(
        "Not requested: '{service_name}' is served by the builtin '{proxy}' proxy, which is \
         already configured. It is not in data/config/apis.json. Call proxy_request with name \
         '{proxy}' and a path relative to {base_url}, or run `lucidos proxy {proxy} <path>` \
         from a script.{path_hint} Do not ask the user for a key. Only a different service \
         needs its own credential: name a host other than the proxy's, or a non-token auth_type."
    )
}

fn builtin_proxy_refusal(
    service_name: &str,
    proxy: &str,
    state: &crate::api::proxy_builtin::BuiltinProxyState,
    requested: AuthType,
    base_urls: &[String],
) -> Option<String> {
    let crate::api::proxy_builtin::BuiltinProxyState::Configured {
        base_url,
        injects_auth: true,
    } = state
    else {
        return None;
    };
    (!overrides_builtin_proxy(requested, base_urls, base_url))
        .then(|| served_by_builtin_proxy(service_name, proxy, base_url))
}

fn requested_service_name(service_name: &str, auth_type: &str) -> String {
    if auth_type == "oauth_client" {
        oauth::client_provider_name(service_name)
    } else {
        service_name.to_string()
    }
}

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
        "redirect_uri",
    ] {
        if let Some(v) = args[key].as_str().map(str::trim).filter(|s| !s.is_empty()) {
            defaults.insert(key.to_string(), serde_json::Value::String(v.to_string()));
        }
    }
    defaults
}

fn reported(field: Option<&String>) -> Option<&str> {
    field.map(String::as_str).filter(|s| !s.trim().is_empty())
}

fn connected_sentence(provider: &str, outcome: &oauth::OAuthFlowOutcome) -> String {
    match (
        reported(outcome.email.as_ref()),
        reported(outcome.display_name.as_ref()),
    ) {
        (Some(email), _) => format!("Successfully connected {provider} account ({email})."),
        (None, Some(name)) => format!(
            "Successfully connected the {provider} account for {name}. The provider reported no \
             email address for it, so refer to it by that name and do not go looking for one."
        ),
        (None, None) => format!(
            "Successfully connected the {provider} account. The provider did not report which \
             account it is (no userinfo endpoint configured for {provider}, or it returned no \
             email), so do not guess or go looking for one."
        ),
    }
}

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

fn connect_result_message(
    provider: &str,
    outcome: &oauth::OAuthFlowOutcome,
    row: Option<&oauth_registry::OAuthProviderRow>,
) -> String {
    let missing = oauth::missing_requested_scopes(
        &outcome.requested_scopes,
        &outcome.granted_scopes,
        oauth::GrantEvidence {
            refresh_token: outcome.has_refresh_token,
        },
    );
    let mut message = connected_sentence(provider, outcome);
    if missing.is_empty() {
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

#[easy_ext::ext(CredentialsExt)]
pub(crate) impl LucidosEngine {
    async fn execute_credential_tool(
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
                let auth_type = args["auth_type"].as_str().unwrap_or("api_key");
                let requested = AuthType::parse(auth_type);
                if !requested.agent_requestable() {
                    return Ok(format!(
                        "Error: '{auth_type}' is not an auth_type this can collect. Use one of: {}",
                        AuthType::agent_requestable_values().join(", ")
                    ));
                }
                let base_urls = if requested == AuthType::Secret {
                    Vec::new()
                } else {
                    match requested_scope(args) {
                        Ok(urls) => urls,
                        Err(reason) => return Ok(format!("Error: {reason}")),
                    }
                };

                if let Some(refusal) =
                    missing_field_refusal(service_name, prompt, &base_urls, auth_type)
                {
                    return Ok(refusal.to_string());
                }

                let service_name = requested_service_name(service_name, auth_type);
                let service_name = service_name.as_str();

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

                if let Some(proxy) =
                    crate::api::proxy_builtin::builtin_proxy_for_service(service_name)
                {
                    let state =
                        crate::api::proxy_builtin::builtin_proxy_state(&self.clone_arc(), proxy)
                            .await;
                    if let Some(refusal) = builtin_proxy_refusal(
                        service_name,
                        proxy.name,
                        &state,
                        requested,
                        &base_urls,
                    ) {
                        return Ok(refusal);
                    }
                }

                match existing_credential(&self.pool, service_name, auth_type).await {
                    Ok(Some(existing)) => {
                        if !answers_request(existing.auth_type, requested) {
                            return Ok(name_taken_by(&existing, requested));
                        }
                        let adding = hosts_outside_scope(&existing, &base_urls);
                        if adding.is_empty() {
                            return Ok(already_configured(service_name));
                        }
                        return Ok(widen_scope_request(&existing, &adding));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        return Ok(format!(
                            "Error: could not check whether '{}' is already configured: {}. Not asking the user for it again until this read works.",
                            service_name, e
                        ))
                    }
                }

                let defaults = if auth_type == "oauth_client" {
                    oauth_defaults_from_args(args)
                } else {
                    serde_json::Map::new()
                };

                Ok(credential_request_with_defaults(
                    service_name,
                    prompt,
                    &base_urls,
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

                let cred_service = oauth::client_provider_name(&provider);
                if CredentialStore::get_oauth_client(&self.pool, &cred_service)
                    .await?
                    .is_none()
                {
                    let str_arg = |key: &str| {
                        args[key]
                            .as_str()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                    };
                    let row =
                        oauth_registry::find_provider(self.system_knowhow_dir(), &cred_service);
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
                        redirect_uri: str_arg("redirect_uri").or(from_row.redirect_uri),
                    };
                    return Ok(credential_request_envelope(oauth::oauth_client_request(
                        &provider, &overrides,
                    )));
                }

                // INVARIANT: The engine does NOT open the browser. It asks the user's own
                // device to, so the authorization page lands wherever they
                // configured links to open. Shelling out to macOS `open` here
                // ignored that preference and did nothing at all on Linux.
                // `purpose: "oauth"` is what lets the client close the in-app
                // browser panel again once the flow lands, instead of leaving
                // the user on a dead callback page inside the app. See
                // `oauthAuthFlow` in store/actions/oauth.ts.
                // The page goes to the last used device, the screen the user is
                // on now. That device comes back to the front when the flow lands.
                //
                // It is a persisted *form request*, not a transient navigation:
                // a client that missed the frame still finds it on its next
                // stream open, for as long as the flow is listening.
                let page_device = self.last_used_device(thread_id, device_id).await;
                let route = oauth::CallbackRoute::for_browser(
                    page_device
                        .as_deref()
                        .and_then(crate::core::oauth_callback::browser_for_device),
                );
                let actor = page_device.map(|id| crate::engine::thread_events::device_actor(&id));
                let open_auth_url = async |auth_url: &str, request_id: uuid::Uuid| {
                    let payload = serde_json::json!({
                        "target": "url",
                        "url": auth_url,
                        "purpose": "oauth",
                    });
                    self.event_bus
                        .emit(crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::OAuthAuthorizationRequested {
                                request_id,
                                payload: payload.to_string(),
                            },
                            meta: crate::engine::thread_events::EventMeta::with_actor(actor.clone()),
                        })
                        .await
                        .map(|_| ())
                        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                            format!("could not open the authorization page: {e}").into()
                        })
                };
                let outcome = oauth::run_oauth_flow(
                    &self.pool,
                    &self.event_bus,
                    &provider,
                    scopes,
                    actor.clone(),
                    route,
                    open_auth_url,
                )
                .await?;

                let row = oauth_registry::find_provider(self.system_knowhow_dir(), &cred_service);
                Ok(connect_result_message(&provider, &outcome, row.as_ref()))
            }
            _ => Err(format!("Unknown credential tool: {}", name).into()),
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

    fn scope(urls: &[&str]) -> Vec<String> {
        urls.iter().map(|u| u.to_string()).collect()
    }

    #[test]
    fn payload_is_valid_json_with_multiline_prompt() {
        let prompt = "1. Open dashboard\n2. Create API key\n3. Paste it below";
        let result = credential_request_payload(
            "binance",
            prompt,
            &scope(&["https://api.binance.com"]),
            "api_key",
        );
        let parsed = parse_payload(&result);
        assert_eq!(parsed["service"], "binance");
        assert_eq!(parsed["prompt"], prompt);
        assert_eq!(
            parsed["base_urls"],
            serde_json::json!(["https://api.binance.com"])
        );
        assert_eq!(parsed["auth_type"], "api_key");
    }

    #[test]
    fn a_request_may_name_every_host_the_provider_uses() {
        let result = credential_request_payload(
            "binance",
            "prompt",
            &scope(&["https://api.binance.com", "https://fapi.binance.com"]),
            "api_key",
        );
        assert_eq!(
            parse_payload(&result)["base_urls"],
            serde_json::json!(["https://api.binance.com", "https://fapi.binance.com"]),
            "every member reaches the modal, in order, so it seeds one row each"
        );
    }

    #[test]
    fn a_secret_request_carries_no_base_urls() {
        let result = credential_request_payload(
            "deploys-github",
            "Paste the secret GitHub signs its webhooks with.",
            &[],
            "secret",
        );
        let parsed = parse_payload(&result);
        assert_eq!(parsed["auth_type"], "secret");
        assert!(
            parsed.get("base_urls").is_none(),
            "an empty scope must be omitted, not sent as an empty array: {parsed}"
        );
    }

    #[test]
    fn base_urls_are_required_for_every_type_but_secret() {
        assert!(missing_field_refusal("svc", "prompt", &[], "secret").is_none());
        for auth_type in ["api_key", "bearer", "basic", "password", "oauth_client"] {
            assert!(
                missing_field_refusal("svc", "prompt", &[], auth_type).is_some(),
                "{auth_type} is presented to a host, so it must name one"
            );
        }
        assert!(missing_field_refusal("", "prompt", &[], "secret").is_some());
        assert!(missing_field_refusal("svc", "", &[], "secret").is_some());
        assert!(missing_field_refusal(
            "svc",
            "prompt",
            &scope(&["https://api.example.com"]),
            "api_key"
        )
        .is_none());
    }

    #[test]
    fn a_request_reads_both_spellings_of_the_scope() {
        let read = |args: serde_json::Value| requested_scope(&args).expect("a valid scope");
        assert_eq!(
            read(serde_json::json!({ "base_urls": ["https://a.test", "https://b.test"] })),
            scope(&["https://a.test", "https://b.test"])
        );
        assert_eq!(
            read(serde_json::json!({ "base_url": "https://a.test" })),
            scope(&["https://a.test"]),
            "the singular still lands, as a one-member set"
        );
        assert_eq!(
            read(serde_json::json!({ "base_urls": "https://a.test" })),
            scope(&["https://a.test"]),
            "a bare string in the array-typed argument is read, not dropped"
        );
        assert_eq!(
            read(serde_json::json!({
                "base_urls": ["https://a.test"],
                "base_url": "https://a.test",
            })),
            scope(&["https://a.test"]),
            "both spellings union, and the duplicate collapses"
        );
        assert!(
            read(serde_json::json!({ "base_urls": ["  "] })).is_empty(),
            "a blank member is dropped, so it reads as no host at all"
        );
    }

    #[test]
    fn a_member_naming_no_host_is_refused_with_a_reason() {
        let refusal = requested_scope(&serde_json::json!({ "base_urls": ["api.example.com"] }))
            .expect_err("a scheme-less member names no host");
        assert!(refusal.contains("api.example.com"), "{refusal}");
        assert!(
            refusal.contains("https://"),
            "it shows the shape: {refusal}"
        );
    }

    #[test]
    fn payload_escapes_quotes_and_backslashes_in_prompt() {
        let prompt = r#"Use the "API Key" field, escape backslashes like \n correctly"#;
        let result = credential_request_payload(
            "svc",
            prompt,
            &scope(&["https://api.example.com"]),
            "api_key",
        );
        let parsed = parse_payload(&result);
        assert_eq!(parsed["prompt"], prompt);
    }

    #[test]
    fn payload_handles_special_chars_in_other_fields() {
        let result = credential_request_payload(
            r#"weird"service"#,
            "prompt",
            &scope(&[r#"https://example.com/path with "quotes""#]),
            "api_key",
        );
        let parsed = parse_payload(&result);
        assert_eq!(parsed["service"], r#"weird"service"#);
        assert_eq!(
            parsed["base_urls"],
            serde_json::json!([r#"https://example.com/path with "quotes""#])
        );
    }

    #[test]
    fn basic_payload_has_no_defaults_block() {
        let result = credential_request_payload(
            "svc",
            "prompt",
            &scope(&["https://api.example.com"]),
            "api_key",
        );
        let parsed = parse_payload(&result);
        assert!(
            parsed.get("defaults").is_none(),
            "non-oauth payloads must not carry a defaults block: {parsed}"
        );
    }

    fn stored(base_urls: &[&str]) -> Credential {
        let mut credential = Credential::fixture("github", AuthType::Bearer, "ghp_secret");
        credential.id = uuid::Uuid::nil();
        credential.base_urls = scope(base_urls);
        credential
    }

    #[test]
    fn a_host_the_scope_already_covers_is_not_missing() {
        let existing = stored(&["https://api.github.com", "https://github.com"]);
        assert!(hosts_outside_scope(&existing, &scope(&["https://github.com"])).is_empty());
        assert!(
            hosts_outside_scope(&existing, &scope(&["https://github.com/example-org"])).is_empty(),
            "a path under a scoped host is covered, exactly as the gate reads it"
        );
        assert!(
            hosts_outside_scope(&existing, &[]).is_empty(),
            "a request naming no host can never want one"
        );
    }

    #[test]
    fn a_host_outside_the_scope_is_reported_once_and_exactly() {
        assert_eq!(
            hosts_outside_scope(
                &stored(&["https://api.github.com"]),
                &scope(&["https://api.github.com", "https://github.com"]),
            ),
            scope(&["https://github.com"]),
            "only the uncovered host is new; the covered one is not re-proposed"
        );
    }

    #[test]
    fn a_stored_secret_is_never_proposed_for_widening() {
        let mut existing = stored(&[]);
        existing.auth_type = AuthType::Secret;
        assert!(hosts_outside_scope(&existing, &scope(&["https://api.github.com"])).is_empty());
    }

    #[test]
    fn only_a_row_of_the_same_shape_answers_a_request() {
        use AuthType::*;

        for stored in [ApiKey, Bearer] {
            for requested in [ApiKey, Bearer] {
                assert!(answers_request(stored, requested));
            }
        }

        for stored in [Basic, Password, Secret, OauthClient] {
            assert!(answers_request(stored, stored));
            for requested in [ApiKey, Bearer, Basic, Password, Secret] {
                assert_eq!(
                    answers_request(stored, requested),
                    stored == requested,
                    "a stored {stored} against a request for {requested}"
                );
            }
        }

        for stored in [EmailPassword, Unknown] {
            for requested in [ApiKey, Bearer, Basic, Password, Secret] {
                assert!(
                    !answers_request(stored, requested),
                    "a stored {stored} must not answer a request for {requested}"
                );
            }
        }

        for stored in AuthType::ALL {
            for requested in AuthType::ALL.iter().filter(|t| t.agent_requestable()) {
                let expected = match stored {
                    EmailPassword | Unknown => false,
                    ApiKey | Bearer => matches!(requested, ApiKey | Bearer),
                    other => other == requested,
                };
                assert_eq!(
                    answers_request(*stored, *requested),
                    expected,
                    "a stored {stored} against a request for {requested}"
                );
            }
        }
    }

    #[test]
    fn a_name_held_by_another_shape_says_so_and_says_what_to_do() {
        let mut existing = stored(&[]);
        existing.service_name = "stripe".to_string();
        existing.auth_type = AuthType::Secret;
        let message = name_taken_by(&existing, AuthType::ApiKey);
        assert!(message.contains("'stripe'"), "{message}");
        assert!(
            message.contains("type secret"),
            "it names what is stored: {message}"
        );
        assert!(
            message.contains("api_key"),
            "and what was asked for: {message}"
        );
        assert!(
            message.contains("another service name"),
            "and the way forward: {message}"
        );
        for stored in AuthType::ALL {
            for requested in AuthType::ALL {
                existing.auth_type = *stored;
                let message = name_taken_by(&existing, *requested);
                for spelling in [stored, requested] {
                    assert!(
                        !message.contains(&format!("a {spelling} ")),
                        "an article would have to agree with {spelling}: {message}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_widening_seeds_the_union_and_targets_the_stored_row() {
        let existing = stored(&["https://api.github.com"]);
        let parsed = parse_payload(&widen_scope_request(
            &existing,
            &scope(&["https://github.com"]),
        ));
        assert_eq!(
            parsed["base_urls"],
            serde_json::json!(["https://api.github.com", "https://github.com"]),
            "the stored host survives: the form is authoritative on save"
        );
        assert_eq!(
            parsed["adding_base_urls"],
            serde_json::json!(["https://github.com"]),
            "named apart, so the modal can say what is NEW"
        );
        assert_eq!(
            parsed["existing_credential_id"],
            serde_json::json!(uuid::Uuid::nil()),
            "the save updates this row rather than creating a second one"
        );
        assert_eq!(parsed["service"], "github");
        assert_eq!(
            parsed["auth_type"], "bearer",
            "the stored type, not a guess"
        );
    }

    #[test]
    fn the_widening_prompt_names_the_hosts_before_it_reassures() {
        let parsed = parse_payload(&widen_scope_request(
            &stored(&["https://api.github.com"]),
            &scope(&["https://github.com", "https://codeload.github.com"]),
        ));
        let prompt = parsed["prompt"].as_str().expect("a prompt");
        let grant = prompt.find("https://github.com").expect("names the host");
        assert!(
            prompt.contains("https://codeload.github.com"),
            "every added host, not just the first: {prompt}"
        );
        assert!(
            grant < prompt.find("does not change").expect("still reassures"),
            "the grant comes first: {prompt}"
        );
    }

    #[test]
    fn a_widening_seeds_nothing_the_stored_row_already_answers() {
        let mut existing = stored(&["https://api.github.com"]);
        existing.env_var_name = Some("GITHUB_TOKEN".to_string());
        existing.auth_header = "X-Api-Key".to_string();
        let parsed = parse_payload(&widen_scope_request(
            &existing,
            &scope(&["https://github.com"]),
        ));
        for field in ["env_var_name", "auth_header", "defaults"] {
            assert!(
                parsed.get(field).is_none(),
                "{field} comes off the resolved row, never the request: {parsed}"
            );
        }
    }

    #[test]
    fn a_widening_payload_never_carries_the_stored_secret() {
        let existing = stored(&["https://api.github.com"]);
        let payload = widen_scope_request(&existing, &scope(&["https://github.com"]));
        assert!(!payload.contains(existing.stored_value()), "{payload}");
        assert!(parse_payload(&payload).get("auth_value").is_none());
    }

    const BUILTIN_BASE: &str = "https://api.provider.test/v1";

    #[test]
    fn a_token_for_the_builtin_host_does_not_override_the_proxy() {
        for requested in [AuthType::ApiKey, AuthType::Bearer] {
            assert!(!overrides_builtin_proxy(
                requested,
                &scope(&["https://api.provider.test"]),
                BUILTIN_BASE
            ));
            assert!(
                !overrides_builtin_proxy(
                    requested,
                    &scope(&["https://API.provider.test/v1/models"]),
                    BUILTIN_BASE
                ),
                "host comparison ignores case and path"
            );
        }
    }

    #[test]
    fn another_auth_type_or_another_host_overrides_the_proxy() {
        for requested in [
            AuthType::OauthClient,
            AuthType::Basic,
            AuthType::Password,
            AuthType::Secret,
        ] {
            assert!(
                overrides_builtin_proxy(
                    requested,
                    &scope(&["https://api.provider.test"]),
                    BUILTIN_BASE
                ),
                "{requested} is not the token the builtin injects"
            );
        }
        assert!(overrides_builtin_proxy(
            AuthType::ApiKey,
            &scope(&["https://relay.example.com"]),
            BUILTIN_BASE
        ));
        assert!(
            overrides_builtin_proxy(
                AuthType::ApiKey,
                &scope(&["https://api.provider.test", "https://relay.example.com"]),
                BUILTIN_BASE
            ),
            "one host outside the builtin is enough"
        );
    }

    #[test]
    fn only_a_configured_builtin_that_injects_auth_refuses() {
        use crate::api::proxy_builtin::BuiltinProxyState;
        let token_request = |state: &BuiltinProxyState| {
            builtin_proxy_refusal(
                "openai",
                "openai",
                state,
                AuthType::ApiKey,
                &scope(&["https://api.provider.test"]),
            )
        };
        let configured = |injects_auth| BuiltinProxyState::Configured {
            base_url: BUILTIN_BASE.to_string(),
            injects_auth,
        };
        assert!(token_request(&configured(true)).is_some());
        assert!(
            token_request(&configured(false)).is_none(),
            "a keyless builtin holds no key, so a key may be asked for"
        );
        assert!(token_request(&BuiltinProxyState::NotConfigured).is_none());
        assert!(
            token_request(&BuiltinProxyState::Unknown).is_none(),
            "a failed check never refuses"
        );
    }

    #[test]
    fn the_builtin_refusal_names_the_proxy_and_the_call() {
        let message = served_by_builtin_proxy("OpenAI", "openai", "https://api.openai.com/v1");
        for needle in [
            "'OpenAI'",
            "builtin 'openai' proxy",
            "not in data/config/apis.json",
            "proxy_request with name 'openai'",
            "lucidos proxy openai",
            "not '/v1/models'",
            "Do not ask the user for a key",
        ] {
            assert!(message.contains(needle), "missing {needle:?}: {message}");
        }
        assert!(
            !served_by_builtin_proxy("svc", "svc", "https://api.example.com").contains("/v1"),
            "the /v1 hint only appears for a base that has it"
        );
    }

    #[test]
    fn the_already_configured_answer_is_unchanged() {
        assert_eq!(
            already_configured("github"),
            "Credentials for 'github' are already configured. You can proceed with API requests."
        );
    }

    #[test]
    fn this_module_writes_no_credential() {
        let production: String = include_str!("credentials.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("the module has a body before its tests")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect();
        for mutator in [
            "upsert",
            "update",
            "delete",
            "set_base_urls",
            "infer_undeclared_scopes",
        ] {
            assert!(
                !production.contains(&format!("CredentialStore::{mutator}")),
                "this module must not write a credential, but it calls {mutator}"
            );
        }
        assert!(
            !production.contains("sqlx::query"),
            "nor reach past the store to write one by hand"
        );
    }

    #[test]
    fn oauth_payload_attaches_supplied_endpoint_defaults() {
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
            &scope(&["https://healthcare.googleapis.com"]),
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
            &scope(&["https://api.example.com"]),
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
            &scope(&["https://api.apple.com"]),
            "password",
            serde_json::Map::new(),
            Some("APPLE_PASSWORD"),
        );
        let parsed = parse_payload(&result);
        assert_eq!(parsed["env_var_name"], "APPLE_PASSWORD");
    }

    #[test]
    fn oauth_client_requests_normalize_to_the_bare_provider() {
        assert_eq!(requested_service_name("dropbox", "oauth_client"), "dropbox");
        assert_eq!(
            requested_service_name("oauth:dropbox", "oauth_client"),
            "dropbox"
        );
    }

    #[test]
    fn non_oauth_credentials_keep_their_name_verbatim() {
        for auth_type in ["api_key", "bearer", "basic", "password", "email_password"] {
            for name in ["dropbox", "Dropbox", "oauth:dropbox"] {
                assert_eq!(
                    requested_service_name(name, auth_type),
                    name,
                    "{auth_type} must not be renamed"
                );
            }
        }
        assert_eq!(
            requested_service_name("email:work", "email_password"),
            "email:work"
        );
    }

    #[test]
    fn the_modal_payload_carries_the_normalized_oauth_name() {
        let result = credential_request_with_defaults(
            &requested_service_name("Dropbox", "oauth_client"),
            "Paste your Dropbox App key into Client ID.",
            &scope(&["https://api.dropboxapi.com"]),
            "oauth_client",
            serde_json::Map::new(),
            None,
        );
        assert_eq!(parse_payload(&result)["service"], "dropbox");
    }

    #[test]
    fn blank_env_var_name_omits_the_field() {
        for name in [None, Some(""), Some("   ")] {
            let result = credential_request_with_defaults(
                "svc",
                "prompt",
                &scope(&["https://api.example.com"]),
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
            has_refresh_token: false,
        }
    }

    fn outcome_with_refresh_token(granted: &str, requested: &str) -> oauth::OAuthFlowOutcome {
        oauth::OAuthFlowOutcome {
            has_refresh_token: true,
            ..outcome(Some("user@example.com"), None, granted, requested)
        }
    }

    const OUTLOOK_GRANTED: &str = "https://outlook.office.com/SMTP.Send \
                                   https://outlook.office.com/IMAP.AccessAsUser.All \
                                   https://outlook.office.com/Mail.Read";

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
            sign_in_scopes: None,
        }
    }

    #[test]
    fn a_full_grant_reports_exactly_what_it_always_did() {
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
    fn a_refresh_token_settles_offline_access_whatever_the_echo_listed() {
        let requested = format!("{OUTLOOK_GRANTED} offline_access");
        assert_eq!(
            connect_result_message(
                "microsoft",
                &outcome_with_refresh_token(OUTLOOK_GRANTED, &requested),
                Some(&row_with_console()),
            ),
            "Successfully connected microsoft account (user@example.com)."
        );
    }

    #[test]
    fn offline_access_with_no_refresh_token_is_still_reported() {
        let requested = format!("{OUTLOOK_GRANTED} offline_access");
        let message = connect_result_message(
            "microsoft",
            &outcome(Some("user@example.com"), None, OUTLOOK_GRANTED, &requested),
            Some(&row_with_console()),
        );
        assert!(
            message.contains("offline_access"),
            "a grant with no refresh token is short of it: {message}"
        );
        assert!(
            message.contains("RECONNECT"),
            "and a reconnect is the fix: {message}"
        );
    }

    #[test]
    fn the_sign_in_scopes_are_never_reported_from_the_echo() {
        assert_eq!(
            connect_result_message(
                "github",
                &outcome(
                    Some("user@example.com"),
                    None,
                    "repo",
                    "openid email profile repo",
                ),
                None,
            ),
            "Successfully connected github account (user@example.com)."
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
        let message =
            connect_result_message("acme", &outcome(None, None, "read", "read write"), None);
        assert!(
            !message.contains("Just say the acme account is connected."),
            "a shortfall's closing instruction is the reconnect one: {message}"
        );
        assert!(message.contains("RECONNECT"));
    }

    #[tokio::test]
    async fn an_oauth_client_is_found_by_type_and_every_other_credential_by_name() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        CredentialStore::upsert(
            &pool,
            &bus,
            "acme",
            &scope(&["https://api.acme.test"]),
            AuthType::ApiKey,
            "key-1",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        CredentialStore::upsert(
            &pool,
            &bus,
            "svc",
            &scope(&["https://api.svc.test"]),
            AuthType::Bearer,
            "tok-1",
            None,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(
            existing_credential(&pool, "acme", "oauth_client")
                .await
                .unwrap()
                .is_none(),
            "an API key answered a request for the app registration"
        );
        let found = existing_credential(&pool, "svc", "api_key")
            .await
            .unwrap()
            .expect("a credential under the requested name was missed over its type");
        assert_eq!(found.service_name, "svc");

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn the_startup_scope_inference_never_scopes_a_secret() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        CredentialStore::upsert(
            &pool,
            &bus,
            "signer",
            &[],
            AuthType::Secret,
            "value",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE credentials SET scope_declared = false WHERE service_name = 'signer'")
            .execute(&pool)
            .await
            .unwrap();
        let configured = std::collections::BTreeMap::from([(
            "signer".to_string(),
            std::collections::BTreeSet::from(["https://api.signer.test".to_string()]),
        )]);

        let scoped = CredentialStore::infer_undeclared_scopes(&pool, &bus, &configured)
            .await
            .unwrap();

        assert!(scoped.is_empty(), "a secret was scoped: {scoped:?}");
        let row = CredentialStore::get(&pool, "signer")
            .await
            .unwrap()
            .expect("the secret is stored");
        assert!(row.base_urls.is_empty());

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_failed_lookup_is_reported_and_never_reopens_the_modal() {
        use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};
        let (pool, db) = setup_test_db().await;
        let workspace = tempfile::tempdir().unwrap();
        let engine = crate::engine::LucidosEngine::new(
            workspace.path().to_path_buf(),
            &test_db_url(&db),
            crate::llm::ProviderBuildContext {
                default_model: "mock".into(),
                model_is_mock: true,
                vertex_project_id: String::new(),
                vertex_location: crate::llm::vertex::location_handle(String::new()),
                vertex_token_cache: None,
                model_registry: Default::default(),
                not_served: Default::default(),
                claude_gateway_access: None,
                chatgpt_plan_access: None,
            },
            std::sync::Arc::new(crate::engine::agent_recovery::EngineAgentHost),
            Box::new(crate::engine::provider_adapters::ProviderAdapters::default()),
        )
        .await
        .unwrap();
        engine.pool.close().await;

        let args = serde_json::json!({
            "service_name": "acme",
            "prompt": "Enter the key",
            "base_urls": ["https://api.acme.test"],
            "auth_type": "api_key",
        });
        let out = engine
            .execute_credential_tool("request_credential", &args, uuid::Uuid::new_v4(), None)
            .await
            .unwrap();

        assert!(
            out.starts_with("Error: could not check whether"),
            "an unreadable lookup was read as no credential: {out}"
        );
        assert!(!out.starts_with(CREDENTIAL_REQUEST_PREFIX), "{out}");

        pool.close().await;
        teardown_test_db(&db).await;
    }

    mod guard_tests {
        use super::*;

        #[test]
        fn the_result_message_names_no_provider() {
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
}
