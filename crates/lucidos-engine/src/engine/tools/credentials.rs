use super::super::LucidosEngine;
use crate::core::credentials::{credential_scope_covers, normalized_base_urls, Credential};
use crate::core::oauth;
use crate::core::oauth_registry;
use crate::core::AuthType;
use crate::core::CredentialStore;

/// Sentinel prefix on a tool result that the agentic loop strips off and emits
/// as the *form request* `CredentialRequested`, which drives the credential form.
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

/// Build the enveloped credential-request payload, optionally attaching an
/// oauth `defaults` block (endpoint URLs + scopes the modal pre-fills) and an
/// `env_var_name` the modal pre-fills into its custom-env-var-name field. An empty
/// `defaults` map omits the block entirely, so the modal treats it as a custom
/// provider and expands the endpoint section for manual entry. A `None`/blank
/// `env_var_name` omits the field, so the modal starts empty (default
/// `CRED_<NAME>` injection).
///
/// `base_urls` is the *credential scope* the modal seeds, one row per host
/// (ADR 0161). An empty set drops the key: a `secret` declares no scope, and the
/// modal hides the field for it, so a seeded row would be an edit nobody sees.
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

/// The hosts one argument names, whichever shape it arrives in.
///
/// One reader serves both spellings, so it takes both. `base_urls` is an array
/// and the back-compat `base_url` is a string, and reading a string is what the
/// singular one needs.
///
/// Reading a bare string under the PLURAL key falls out of that, and is welcome:
/// a JSON Schema `type` is advisory to a model, and one handed an array argument
/// sometimes writes a scalar.
///
/// Not a *temporary measure*, despite reading like model tolerance. The `String`
/// arm is load-bearing for the singular `base_url`, which ADR 0161 decision 7
/// keeps permanently. So there is no separate site to remove, and a registry row
/// would name a deletion nobody could perform.
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

/// The *credential scope* a `request_credential` call declares, normalized, or
/// the refusal the caller should return verbatim.
///
/// `base_urls` is the argument. The singular `base_url` is still read and
/// unioned in, on ADR 0161 decision 7's terms: that spelling stayed legal on the
/// request bodies, so a model still reaching for it is answered rather than
/// refused.
///
/// Both go through [`normalized_base_urls`], the one speller. A member naming no
/// host is refused here, rather than stored as a scope the gate can only reject.
fn requested_scope(args: &serde_json::Value) -> Result<Vec<String>, String> {
    let mut raw = scope_strings(&args["base_urls"]);
    raw.extend(scope_strings(&args["base_url"]));
    normalized_base_urls(raw)
}

/// The refusal a `request_credential` call earns, or `None` when it carries
/// everything the modal needs.
///
/// A `secret` is the one type needing no base URL. It is signed with rather
/// than sent, so it declares no scope and there is no host to name. Every other
/// type is presented to one, and the proxy refuses a credential with no scope.
fn missing_field_refusal(
    service_name: &str,
    prompt: &str,
    base_urls: &[String],
    auth_type: &str,
) -> Option<&'static str> {
    // The set is already normalized, so a whitespace-only host has been dropped
    // and reads as no host at all. Accepting one would open a modal with no
    // scope and tell the agent nothing.
    let needs_base_url = AuthType::parse(auth_type) != AuthType::Secret;
    if service_name.is_empty() || prompt.is_empty() || (needs_base_url && base_urls.is_empty()) {
        return Some(
            "Error: service_name and prompt are required, and so is base_urls for every \
             auth_type but 'secret'",
        );
    }
    None
}

/// The credential a `request_credential` call is about, if one is stored.
///
/// **An `oauth_client` is looked up by name AND type**, because it is the one
/// type allowed to share a name with another credential. A bare-name check
/// would call a Google API key "already configured" to an agent asking for the
/// Google app registration, and never open the modal.
///
/// **Every other type is looked up by NAME ALONE.** The partial unique index
/// keeps a name globally unique across all of them. So the name is the whole
/// handle, and the requested type is a guess the caller should not have to get
/// right. Missing the row over that guess is expensive: the handler opens a
/// CREATE modal, the user types the same secret again, and the save replaces
/// the working row, type and scope included.
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

/// Whether a stored credential is an answer to a request for `requested`.
///
/// The name-keyed lookup finds whatever row holds the name. This is what stops
/// the handler treating an unrelated one as the credential being asked for.
/// Wrong, the tool reports a mailbox password as a configured API key, and a
/// widening then puts that password in reach of an HTTP host.
///
/// **Only `api_key` and `bearer` are interchangeable.** Both hold a bare token
/// reaching a script as `CRED_<NAME>`, and which one the user picked is a
/// header detail the row itself carries. Every other type is a shape the agent
/// cannot use as asked, so it answers only itself: a `password` splits into two
/// env vars, a `basic` holds `username:password`, and a `secret` is sent
/// nowhere. `only_a_row_of_the_same_shape_answers_a_request` walks the grid.
///
/// `email_password` and `unknown` answer nothing. Neither is
/// [`AuthType::agent_requestable`]: `configure_email` owns the first, and the
/// second is a row a newer engine wrote.
fn answers_request(stored: AuthType, requested: AuthType) -> bool {
    let token = |t| matches!(t, AuthType::ApiKey | AuthType::Bearer);
    match stored {
        AuthType::EmailPassword | AuthType::Unknown => false,
        AuthType::ApiKey | AuthType::Bearer => token(requested),
        // `oauth_client` included: the lookup finds one only for a request that
        // named that type, so it can only ever have matched itself.
        other => other == requested,
    }
}

/// The answer when the name is held by a credential of another shape.
///
/// It states the fact and leaves the move to the agent, which is the only
/// honest thing it can do: the stored row may well serve, under its own type
/// and its own env vars. The two answers it replaces were both worse. Told
/// "already configured", the agent reads a signing secret as an API key. Shown
/// an unexplained create modal, the user overwrites the row the name belongs to.
///
/// Written "a credential of type X" rather than "a X credential", so no
/// indefinite article has to agree with a wire spelling. Four of the eight start
/// with a vowel.
fn name_taken_by(existing: &Credential, requested: AuthType) -> String {
    format!(
        "'{}' already names a credential of type {}, not the {requested} that \
         was asked for. Use the stored one if it serves, or ask for this one \
         under another service name.",
        existing.service_name, existing.auth_type
    )
}

/// The requested hosts a stored credential does not already reach.
///
/// Empty means it reaches everything that was asked for, so there is nothing to
/// propose.
///
/// Judged by [`credential_scope_covers`], the same predicate the proxy and every
/// git credential callback ask. So a host this calls missing is exactly a host
/// the gate would refuse.
///
/// **A `secret` is never widened.** Its EMPTY scope is the answer rather than a
/// gap, because the value is signed with and never sent.
/// `CredentialStore::infer_scope_if_empty` skips one for the same reason:
/// giving it a host grants exactly the reach the type exists to refuse.
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

/// The answer for a credential that already reaches everything asked for.
///
/// Pinned by a test and deliberately unchanged. The agent has read this exact
/// sentence since the tool existed, and rewording a success is churn it has to
/// relearn for nothing.
fn already_configured(service_name: &str) -> String {
    format!(
        "Credentials for '{service_name}' are already configured. \
         You can proceed with API requests."
    )
}

/// Reopen the credential the agent asked about, seeded to reach `adding` too.
///
/// The engine proposes and writes nothing. `existing_credential_id` routes the
/// modal's save to an update of this one row, so the user presses Save and the
/// secret is never retyped. ADR 0161 decision 6 keeps the write the user's, and
/// the alternative in practice is a second row holding the same token.
///
/// The seeded scope is the UNION, stored first. A replacement would let one
/// widening quietly drop a host the user had, and the form is authoritative on
/// save. The user still sees every row and can remove one before saving.
///
/// **The prompt leads with the grant, not with the reassurance.** A widening
/// costs the user no secret, so reading the form IS the whole of the consent. A
/// prompt opening on "this only widens where it may be sent" invites a Save
/// nobody read, and the hosts are what they must read.
///
/// Nothing else is seeded. The modal resolves this row before it renders, so
/// its own stored auth type, header and env var name already win over anything
/// carried here.
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
        // Named apart from the union so the modal can say what is NEW. Read off
        // the seeded rows it could not, since those hold the stored hosts too.
        "adding_base_urls": adding,
    }))
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
    let missing = oauth::missing_requested_scopes(
        &outcome.requested_scopes,
        &outcome.granted_scopes,
        oauth::GrantEvidence {
            refresh_token: outcome.has_refresh_token,
        },
    );
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
    /// authorization page is opened by the user's own client, through an
    /// `OAuthAuthorizationRequested` form request. So the flow needs to know
    /// which thread to emit on and which device is in front of the user.
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
                let auth_type = args["auth_type"].as_str().unwrap_or("api_key");
                // Refused here, so every type reaching the rest of this arm is
                // one the modal can actually collect. A spelling the enum does
                // not know used to reach the form itself, where the Auth Type
                // dropdown has no such option to select.
                let requested = AuthType::parse(auth_type);
                if !requested.agent_requestable() {
                    return Ok(format!(
                        "Error: '{auth_type}' is not an auth_type this can collect. Use one of: {}",
                        AuthType::agent_requestable_values().join(", ")
                    ));
                }
                // A `secret` declares no scope whatever the model passed, since
                // it is signed with rather than sent. Clearing it here rather
                // than refusing it keeps every downstream claim true, including
                // the payload builder's "an empty set drops the key". A
                // malformed member is not reported either: the field is not
                // this type's to fill.
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

                // Check if credential already exists.
                //
                // A lookup that FAILED is not "no credential is stored". Read as
                // one, a DB blip re-opens the modal for a credential the user
                // already entered, and they type it in again.
                match existing_credential(&self.pool, service_name, auth_type).await {
                    Ok(Some(existing)) => {
                        // The name may be held by a credential of another kind,
                        // which answers nothing this asked for.
                        if !answers_request(existing.auth_type, requested) {
                            return Ok(name_taken_by(&existing, requested));
                        }
                        // A row exists, but "already configured" is only true for
                        // the hosts its scope covers. Said of a host outside it,
                        // the answer is false and leaves the agent nowhere: the
                        // way around it is a second service name holding the same
                        // secret, which is exactly what ADR 0161 rejected.
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
                    // Anything the agent did NOT pass falls back to the *OAuth
                    // provider registry* row, which is the same data it would
                    // have read out of the knowhow. Passing wins, so a derived
                    // name carrying the base provider's URLs behaves exactly as
                    // before; the fallback only rescues the case where the agent
                    // skipped the lookup, which used to drop the user into a
                    // blank endpoint form.
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
                // The page goes to the last used device, the screen the user is
                // on now. That device comes back to the front when the flow lands.
                //
                // It is a persisted *form request*, not a transient navigation:
                // a client that missed the frame still finds it on its next
                // stream open, for as long as the flow is listening.
                let actor = match self.last_used_device(thread_id, device_id).await {
                    Some(id) => Some(super::navigate::device_actor(&self.pool, &id).await),
                    None => None,
                };
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
                    open_auth_url,
                )
                .await?;

                // The registry row supplies the per-provider half of a shortfall
                // message (which console to open, what has to be enabled there).
                // Looked up the same way the no-credentials branch above does,
                // and absent registry rows are a supported state.
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

    /// The shape the singular `base_url` could not express: one key, several
    /// hostnames of one provider, on ONE credential (ADR 0161).
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

    /// A `secret` reaches scripts as `CRED_<NAME>` and no host, so the modal
    /// hides its scope field. A seeded row there would be an edit nobody sees.
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

    /// The one type that needs no base URL, and the five that do.
    #[test]
    fn base_urls_are_required_for_every_type_but_secret() {
        assert!(missing_field_refusal("svc", "prompt", &[], "secret").is_none());
        for auth_type in ["api_key", "bearer", "basic", "password", "oauth_client"] {
            assert!(
                missing_field_refusal("svc", "prompt", &[], auth_type).is_some(),
                "{auth_type} is presented to a host, so it must name one"
            );
        }
        // The other two fields are required whatever the type is.
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

    /// Either spelling and either shape reaches the same normalized set. The
    /// singular stayed legal on the request bodies (ADR 0161 decision 7), so a
    /// model still reaching for it is answered rather than refused.
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

    /// Refused at the tool, rather than stored as a scope the gate can only
    /// reject. Stored, it would surface as a 502 far from the call behind it.
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

    // ─── Asking again for a host the credential does not reach ─────────────
    //
    // The reported case: a token saved for a provider's REST host, then wanted
    // for its git host. "Already configured" is false there, and the way around
    // it was a second service name holding the same secret. ADR 0161 named that
    // outcome as the thing a set exists to stop.

    fn stored(base_urls: &[&str]) -> Credential {
        Credential {
            id: uuid::Uuid::nil(),
            service_name: "github".to_string(),
            base_urls: scope(base_urls),
            auth_type: AuthType::Bearer,
            auth_value: "ghp_secret".to_string(),
            auth_header: "Authorization".to_string(),
            env_var_name: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
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

    /// A stored `secret` is never widened. Its empty scope is the answer, so
    /// every host reads as missing and a naive comparison proposes all of them.
    /// Saving that turns a value the engine only signs with into one it sends.
    #[test]
    fn a_stored_secret_is_never_proposed_for_widening() {
        let mut existing = stored(&[]);
        existing.auth_type = AuthType::Secret;
        assert!(hosts_outside_scope(&existing, &scope(&["https://api.github.com"])).is_empty());
    }

    /// The name-keyed lookup finds whatever row holds the name, so the handler
    /// has to ask whether that row is an answer at all.
    #[test]
    fn only_a_row_of_the_same_shape_answers_a_request() {
        use AuthType::*;

        // A bare token under `CRED_<NAME>`, either way. Which one the user
        // picked is a header detail, and the reported bug is a model guessing
        // the other one.
        for stored in [ApiKey, Bearer] {
            for requested in [ApiKey, Bearer] {
                assert!(answers_request(stored, requested));
            }
        }

        // Every other type answers only itself. A `password` splits into two
        // env vars and a `basic` holds `username:password`, so neither is a
        // token an agent told "already configured" could use.
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

        // Never this tool's to touch, whatever was asked for.
        for stored in [EmailPassword, Unknown] {
            for requested in [ApiKey, Bearer, Basic, Password, Secret] {
                assert!(
                    !answers_request(stored, requested),
                    "a stored {stored} must not answer a request for {requested}"
                );
            }
        }

        // The whole grid, so neither axis can grow a variant nobody judged.
        // `requested` can only be a requestable type: the handler refuses the
        // other two before the lookup runs.
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
        // Never "a email_password". Four of the eight wire spellings start with
        // a vowel, so the sentence is shaped to need no indefinite article at
        // all. Both slots are swept: either could grow one.
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

    /// The prompt leads with the grant. A widening costs the user no secret, so
    /// reading the form is the whole of the consent. An opening line about how
    /// little is changing invites a Save nobody read.
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

    /// The modal resolves the row before it renders, so its stored auth type,
    /// header and env var name already win. A copy here is a field nobody reads,
    /// and a later reordering of that precedence would make it load-bearing
    /// without anyone deciding so.
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

    /// The request payload is persisted now, in the events table and every
    /// trigger payload. A widening reopens a row that holds a secret, and none
    /// of it may ride along.
    #[test]
    fn a_widening_payload_never_carries_the_stored_secret() {
        let existing = stored(&["https://api.github.com"]);
        let payload = widen_scope_request(&existing, &scope(&["https://github.com"]));
        assert!(!payload.contains(&existing.auth_value), "{payload}");
        assert!(parse_payload(&payload).get("auth_value").is_none());
    }

    #[test]
    fn the_already_configured_answer_is_unchanged() {
        assert_eq!(
            already_configured("github"),
            "Credentials for 'github' are already configured. You can proceed with API requests."
        );
    }

    /// The load-bearing one: the engine PROPOSES a widening and writes nothing.
    /// ADR 0161 decision 6 keeps a scope change the user's act, and the modal's
    /// Save is what performs it.
    ///
    /// Scoped to the whole module rather than to the `request_credential` arm,
    /// because a helper it calls is where a write would actually land. The
    /// widening builder and the scope comparison both live outside the arm, and
    /// an arm-only slice reads them as somebody else's code.
    ///
    /// Comments are stripped first. A doc comment naming a mutator, to say the
    /// module does NOT call it, is prose. Read as a call, the explanation
    /// becomes the failure.
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
            "infer_scope_if_empty",
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
            &scope(&["https://api.dropboxapi.com"]),
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
            has_refresh_token: false,
        }
    }

    /// The same, for a connection that came back with a refresh token.
    fn outcome_with_refresh_token(granted: &str, requested: &str) -> oauth::OAuthFlowOutcome {
        oauth::OAuthFlowOutcome {
            has_refresh_token: true,
            ..outcome(Some("user@example.com"), None, granted, requested)
        }
    }

    /// What Microsoft echoes for a token issued to the `outlook.office.com`
    /// RESOURCE: that resource's own scopes, and nothing else.
    const OUTLOOK_GRANTED: &str = "https://outlook.office.com/SMTP.Send \
                                   https://outlook.office.com/IMAP.AccessAsUser.All \
                                   https://outlook.office.com/Mail.Read";

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
    fn a_refresh_token_settles_offline_access_whatever_the_echo_listed() {
        // The reported bug. Microsoft granted the refresh token and left
        // `offline_access` out of the echo. The diff read that as a refusal
        // and sent the user to the Entra portal, to enable something already
        // enabled.
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
        // The case that actually breaks renewal, and the only one worth a
        // console trip. The echo says the same thing in both tests; the
        // evidence is what differs.
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
        // Every Connect asks for `openid email profile` (the frontend's
        // SIGN_IN_SCOPES). GitHub has no such scopes and echoes neither, which
        // reported every GitHub account as short of all three.
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
