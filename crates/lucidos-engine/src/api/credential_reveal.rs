//! Revealing a credential's plaintext to the Settings page, and to nothing
//! else.
//!
//! `GET /api/v1/credential-value` used to return the stored secret to any
//! caller. App UIs load at `/app/<id>/` on the engine's own origin, so an
//! installed app's JS could list the credentials and read every one. That
//! undercuts the premise of `api::proxy`, which exists so an iframe never sees
//! a credential.
//!
//! A reveal is now two steps inside a 30-second one-shot window, refused from
//! an app document, and audited. ADR 0117 states the full model, what it does
//! not close, and why the two steps read `Referer` by different rules. The
//! shared machinery lives in [`super::secret_reveal`], which the backup key
//! uses too.

use super::secret_reveal::{
    forbidden, reveal_request_allowed, token_required, RefererRule, RevealSubject,
    RevealTokenResponse,
};
use super::*;
use crate::core::{AuthType, CredentialStore};

/// The route that mints a credential-reveal token, named in its own refusal.
const MINT_ROUTE: &str = "/api/v1/credential-reveal-token";

/// What the 403 calls the secret this module guards.
const SUBJECT_LABEL: &str = "a credential value";

/// What an agent is told by both steps. A credential reaches an agent's work
/// through the proxy, which never shows it the value.
const AGENT_REFUSAL: &str = "An agent cannot reveal a credential value. Call the API \
     through the credential proxy, which signs the request without showing the value, \
     or ask the user to copy it in Settings.";

/// `POST /api/v1/credential-reveal-token`: mint a one-shot reveal token.
///
/// Step one of two. The Settings page mints, then immediately spends. Splitting
/// the read in half is what stops a scrape of reachable GET endpoints finding
/// the plaintext.
pub(super) async fn mint_reveal_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CredentialIdQuery>,
) -> Result<Json<RevealTokenResponse>, (StatusCode, String)> {
    if !reveal_request_allowed(&headers, RefererRule::Required) {
        log!("[Credentials] refused a reveal-token mint from an app document");
        return Err(forbidden(SUBJECT_LABEL));
    }
    crate::api::actor::refuse_agent_caller(&headers, AGENT_REFUSAL)
        .map_err(|e| (e.status, e.message))?;
    // Refuse to mint against a row that does not exist, so a caller learns that
    // here rather than after spending the token.
    match CredentialStore::get_by_id(&state.pool, query.id).await {
        Ok(Some(cred)) if cred.auth_type == AuthType::OnepasswordServiceAccount => {
            return Err(service_account_unrevealable())
        }
        Ok(Some(_)) => {}
        Ok(None) => return Err((StatusCode::NOT_FOUND, "Credential not found".to_string())),
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get credential: {}", e),
            ))
        }
    }
    let token = state
        .reveal_tokens
        .mint(RevealSubject::Credential(query.id))
        .ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            "reveal-token store is poisoned".to_string(),
        ))?;
    Ok(Json(RevealTokenResponse::new(token)))
}

/// Query for the reveal itself: the row, plus the token minted for it.
///
/// `token` is optional in the SHAPE so a caller that omits it meets the
/// handler's own refusal, which names the route that mints one. Left required,
/// axum answers a 400 that says only "failed to deserialize query string".
#[derive(Deserialize)]
pub(super) struct CredentialValueQuery {
    id: uuid::Uuid,
    #[serde(default)]
    token: Option<String>,
}

/// `GET /api/v1/credential-value`: the plaintext, for the Settings copy buttons
/// and the edit form's prefill.
///
/// By id, not name. `CredentialStore::get` is blind to `oauth_client` rows on
/// purpose. A name-keyed lookup could therefore never reach the client ID and
/// secret that row's own Copy buttons ask for.
///
/// Every success writes a `CredentialRevealed` row naming the service and the
/// device, never the value. An app that does defeat the origin check therefore
/// cannot do it quietly.
pub(super) async fn get_credential_value(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CredentialValueQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if !reveal_request_allowed(&headers, RefererRule::WhenPresent) {
        log!("[Credentials] refused a credential-value read from an app document");
        return Err(forbidden(SUBJECT_LABEL));
    }
    // Before the redeem, which is one-shot: a caller refused after it would
    // have spent its token on a request that never saw the credential.
    crate::api::actor::refuse_agent_caller(&headers, AGENT_REFUSAL)
        .map_err(|e| (e.status, e.message))?;
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None)
        .await
        .map_err(|e| (e.status, e.message))?;
    let presented = query.token.as_deref().unwrap_or_default();
    if !state
        .reveal_tokens
        .redeem(presented, RevealSubject::Credential(query.id))
    {
        log!("[Credentials] refused a credential-value read: no live reveal token");
        return Err(token_required(MINT_ROUTE));
    }
    match CredentialStore::get_by_id(&state.pool, query.id).await {
        Ok(Some(cred)) if cred.auth_type == AuthType::OnepasswordServiceAccount => {
            Err(service_account_unrevealable())
        }
        // The stored value, so a *secret reference* shows as the reference.
        // Its value already sits in the user's 1Password.
        Ok(Some(cred)) => {
            state
                .engine
                .event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::System(
                        crate::engine::event_bus::SystemEvent::CredentialRevealed {
                            service_name: cred.service_name.clone(),
                            auth_type: cred.auth_type.to_string(),
                            actor: Some(actor),
                        },
                    ),
                    "[Credentials] CredentialRevealed",
                )
                .await;
            Ok(Json(serde_json::json!({
                "auth_type": cred.auth_type.to_string(),
                "auth_value": cred.stored_value(),
            })))
        }
        Ok(None) => Err((StatusCode::NOT_FOUND, "Credential not found".to_string())),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get credential: {}", e),
        )),
    }
}

/// The refusal for a 1Password service account token, which only the `op`
/// tool ever reads.
fn service_account_unrevealable() -> (StatusCode, String) {
    (
        StatusCode::FORBIDDEN,
        "a 1Password service account token is never shown again. Replace it to change it"
            .to_string(),
    )
}

/// Routes for the two-step credential reveal.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/credential-reveal-token", post(mint_reveal_token))
        .route("/credential-value", get(get_credential_value))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusal names the route that mints, and that route is the one the
    /// router registers. A 403 pointing at a path nobody serves is a dead end
    /// for any caller outside this repository.
    #[test]
    fn the_refusal_names_a_route_this_module_serves() {
        let (_, body) = token_required(MINT_ROUTE);
        assert!(body.contains(MINT_ROUTE), "{body}");
        assert!(
            MINT_ROUTE.ends_with("/credential-reveal-token"),
            "the mint route must match the one `router` registers"
        );
    }
}
