//! Webhooks: the user's CRUD, and the one route a sender actually calls.
//!
//! Full design:
//! `docs/plans/2026-08-19-webhooks-and-engines-off-the-network.md`.
//!
//! # Two surfaces on one prefix, on purpose kept apart
//!
//! `POST /webhooks/:id/deliver` is what the *hook socket* forwards, so it is
//! reachable by anything on the public internet the user pointed at it. The
//! CRUD beside it is not. They differ by path rather than only by method, so
//! nobody has to notice a verb to see which is which.
//!
//! Delivery reads the body as raw bytes and verifies before parsing anything.
//! A public endpoint must not run a parser for a caller it has not accepted,
//! and the signature is over those exact bytes anyway.

use super::error::ApiError;
use super::*;
use crate::core::webhook_deliveries::{Claim, DeliveryLedger, MAX_WINDOW_SECS};
use crate::core::webhooks::{
    self, DedupeConfig, HmacConfig, PresentedDelivery, Webhook, WebhookConfig, WebhookPatch,
    WebhookStore,
};
use crate::core::CredentialStore;
use crate::engine::thread_events::MessageOrigin;
use axum::body::Bytes;

#[derive(Serialize)]
struct WebhookRow {
    id: String,
    name: String,
    event_type: String,
    enabled: bool,
    /// Whether a signature is configured. The config is returned too; this is
    /// the one-glance answer a list needs.
    signed: bool,
    hmac: Option<HmacConfig>,
    /// Absent means every arrival emits, so the log keeps the sender's retries.
    dedupe: Option<DedupeConfig>,
    /// Request headers this hook copies into the event payload.
    headers: Vec<String>,
    created_at: String,
    /// Path a sender posts to, under whatever host the hook socket is exposed
    /// on. The engine knows no public hostname, so it states the path alone.
    delivery_path: String,
}

impl WebhookRow {
    fn from_hook(hook: Webhook, slug: &str) -> Self {
        Self {
            delivery_path: format!("/{slug}/{}", hook.id),
            id: hook.id.to_string(),
            name: hook.name,
            event_type: hook.event_type,
            enabled: hook.enabled,
            signed: hook.hmac.is_some(),
            hmac: hook.hmac,
            dedupe: hook.dedupe,
            headers: hook.headers,
            created_at: hook.created_at.to_rfc3339(),
        }
    }
}

/// The workspace slug a delivery URL carries.
///
/// The gateway hands every engine it spawns its own `LUCIDOS_WORKSPACE_ID`.
/// With no gateway there is no hook socket either, so the directory name is a
/// reasonable label for a URL nothing will call.
fn workspace_slug(state: &AppState) -> String {
    std::env::var("LUCIDOS_WORKSPACE_ID").unwrap_or_else(|_| {
        state
            .workspace_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "workspace".to_string())
    })
}

async fn list_webhooks(State(state): State<AppState>) -> Result<Json<Vec<WebhookRow>>, ApiError> {
    let slug = workspace_slug(&state);
    let hooks = WebhookStore::list(&state.pool)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(
        hooks
            .into_iter()
            .map(|h| WebhookRow::from_hook(h, &slug))
            .collect(),
    ))
}

#[derive(Deserialize)]
struct CreateWebhookRequest {
    name: String,
    event_type: String,
    #[serde(default)]
    hmac: Option<HmacConfig>,
    #[serde(default)]
    dedupe: Option<DedupeConfig>,
    #[serde(default)]
    headers: Vec<String>,
}

#[derive(Serialize)]
struct CreatedWebhook {
    #[serde(flatten)]
    webhook: WebhookRow,
    /// The bearer token, in readable form, for the only time it ever is. Only
    /// its digest is stored, so a caller that loses this makes a new webhook.
    ///
    /// `None` for a signed hook, which authenticates by signature alone. A
    /// sender like GitHub cannot present a bearer token, so pinning one would
    /// make the hook refuse every real delivery.
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
}

/// Create a webhook, and hand back its token once.
async fn create_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateWebhookRequest>,
) -> Result<Json<CreatedWebhook>, ApiError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("a webhook needs a name"));
    }
    let event_type = body.event_type.trim();
    // The same gate the app-UI emit path applies. A domain event and a system
    // frame share a wire shape, so a hook pinned to `NotificationCreated` would
    // forge one on every connected client.
    crate::core::event_subscription::validate_emittable_event_type(event_type)
        .map_err(ApiError::bad_request)?;
    if let Some(hmac) = body.hmac.as_ref() {
        validate_hmac(&state, hmac).await?;
    }
    if let Some(dedupe) = body.dedupe.as_ref() {
        validate_dedupe(dedupe)?;
    }
    let carried = validate_carried_headers(&body.headers, body.hmac.as_ref())?;

    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    let slug = workspace_slug(&state);
    let (hook, token) = WebhookStore::create(
        &state.pool,
        &state.engine.event_bus,
        name,
        event_type,
        WebhookConfig {
            hmac: body.hmac,
            dedupe: body.dedupe,
            headers: carried,
        },
        actor,
    )
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(CreatedWebhook {
        webhook: WebhookRow::from_hook(hook, &slug),
        token,
    }))
}

/// Refuse a signature config the engine could never satisfy.
///
/// The credential must exist now. A hook whose secret is missing refuses every
/// delivery, and learning that from a sender's failed retries is worse than
/// learning it here.
async fn validate_hmac(state: &AppState, hmac: &HmacConfig) -> Result<(), ApiError> {
    if hmac.signature_header.trim().is_empty() {
        return Err(ApiError::bad_request(
            "signature_header names the header carrying the signature",
        ));
    }
    if !hmac.template.contains("{body}") {
        return Err(ApiError::bad_request(
            "template must sign {body}, or the signature says nothing about the payload",
        ));
    }
    if hmac.template.contains("{timestamp}")
        && hmac.timestamp_header.is_none()
        && hmac.timestamp_key.is_none()
    {
        return Err(ApiError::bad_request(
            "a template using {timestamp} needs timestamp_header or timestamp_key",
        ));
    }
    let known = CredentialStore::get(&state.pool, &hmac.credential)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if known.is_none() {
        return Err(ApiError::bad_request(format!(
            "no credential named '{}': save the signing secret first",
            hmac.credential
        )));
    }
    Ok(())
}

/// The one header a hook may never dedupe on or carry.
///
/// As a dedupe key it is the constant bearer token, so every delivery would
/// resolve to one key and only the first would ever emit. As a carried header
/// it would write that token into an append-only log.
const AUTHORIZATION: &str = "authorization";

/// Refuse a dedupe config that cannot do what it says.
fn validate_dedupe(dedupe: &DedupeConfig) -> Result<(), ApiError> {
    if let Some(header) = dedupe.header.as_deref() {
        let header = header.trim();
        if header.is_empty() {
            return Err(ApiError::bad_request(
                "dedupe.header names the header carrying the sender's delivery id",
            ));
        }
        if header.eq_ignore_ascii_case(AUTHORIZATION) {
            return Err(ApiError::bad_request(
                "dedupe.header cannot be Authorization: the token never changes, \
                 so every delivery after the first would look like a duplicate",
            ));
        }
    }
    if dedupe.window_secs < 0 {
        return Err(ApiError::bad_request(
            "dedupe.window_secs cannot be negative; 0 switches deduping off",
        ));
    }
    if dedupe.window_secs > MAX_WINDOW_SECS {
        return Err(ApiError::bad_request(format!(
            "dedupe.window_secs is capped at {} seconds, since the sweep drops \
             a claim older than that",
            MAX_WINDOW_SECS
        )));
    }
    Ok(())
}

/// Refuse a header allow-list that would carry a secret, and normalise the rest.
///
/// The events table is append-only, so a secret copied into a payload is there
/// permanently. An allow-list beats filtering at write time: the refusal
/// reaches whoever configured the hook, instead of silently dropping a header
/// they expected to see.
fn validate_carried_headers(
    requested: &[String],
    hmac: Option<&HmacConfig>,
) -> Result<Vec<String>, ApiError> {
    let signature = hmac.map(|cfg| cfg.signature_header.trim().to_ascii_lowercase());
    let mut carried: Vec<String> = Vec::with_capacity(requested.len());
    for name in requested {
        let name = name.trim();
        if name.is_empty() {
            return Err(ApiError::bad_request("a carried header needs a name"));
        }
        let lower = name.to_ascii_lowercase();
        if lower == AUTHORIZATION {
            return Err(ApiError::bad_request(
                "Authorization cannot be carried into the payload: the events \
                 table is append-only, so the token would be there for good",
            ));
        }
        if signature.as_deref() == Some(lower.as_str()) {
            return Err(ApiError::bad_request(format!(
                "'{name}' is this hook's signature header, so carrying it would \
                 publish the signature alongside the body it signs"
            )));
        }
        if !carried.iter().any(|held| held.eq_ignore_ascii_case(name)) {
            carried.push(name.to_string());
        }
    }
    Ok(carried)
}

#[derive(Deserialize)]
struct UpdateWebhookRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    event_type: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    dedupe: Option<DedupeConfig>,
    #[serde(default)]
    headers: Option<Vec<String>>,
}

async fn update_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateWebhookRequest>,
) -> Result<Json<WebhookRow>, ApiError> {
    if let Some(event_type) = body.event_type.as_deref() {
        crate::core::event_subscription::validate_emittable_event_type(event_type.trim())
            .map_err(ApiError::bad_request)?;
    }
    if let Some(dedupe) = body.dedupe.as_ref() {
        validate_dedupe(dedupe)?;
    }
    // The signature header lives on the stored hook. So a carried-header list
    // is checked against what this hook verifies with, not against a config the
    // request never sent. Only read it when there is a list to check, since the
    // common update is a name or an enabled flag.
    let carried = match body.headers.as_deref() {
        Some(requested) => {
            let stored = WebhookStore::get(&state.pool, id)
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?
                .ok_or_else(|| ApiError::bad_request("no webhook with that id"))?;
            Some(validate_carried_headers(requested, stored.hmac.as_ref())?)
        }
        None => None,
    };

    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    let slug = workspace_slug(&state);
    let updated = WebhookStore::update(
        &state.pool,
        &state.engine.event_bus,
        id,
        WebhookPatch {
            name: body.name.as_deref().map(|s| s.trim().to_string()),
            event_type: body.event_type.as_deref().map(|s| s.trim().to_string()),
            enabled: body.enabled,
            dedupe: body.dedupe,
            headers: carried,
        },
        actor,
    )
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
    .ok_or_else(|| ApiError::bad_request("no webhook with that id"))?;
    Ok(Json(WebhookRow::from_hook(updated, &slug)))
}

async fn delete_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    let removed = WebhookStore::delete(&state.pool, &state.engine.event_bus, id, actor)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if !removed {
        return Err(ApiError::bad_request("no webhook with that id"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// What a refused delivery says back.
///
/// One message for every refusal, and it never names the reason. A public
/// endpoint that tells "wrong token" apart from "bad signature" is telling
/// whoever is guessing which half they got right. The log carries the detail.
fn refused() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "unauthorized" })),
    )
        .into_response()
}

/// `POST /api/v1/webhooks/:id/deliver`: verify a delivery, then emit its event.
///
/// The body arrives as [`Bytes`] rather than `Json`, which is the whole point.
/// Verification runs on those exact bytes, before any parse, and the signature
/// covers them verbatim.
async fn deliver(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let hook = match WebhookStore::get(&state.pool, id).await {
        Ok(Some(hook)) => hook,
        // An unknown id and a disabled hook answer alike. Either way this URL
        // accepts no deliveries, and saying which is a probe oracle.
        Ok(None) => return refused(),
        Err(e) => {
            crate::log!("[Webhook] could not load {id}: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "internal error" })),
            )
                .into_response();
        }
    };
    if !hook.enabled {
        return refused();
    }

    // A sender may sign any bytes. Non-UTF-8 cannot have come from a scheme we
    // express, so it is refused before the lossy conversion that would
    // otherwise change what gets verified.
    let Ok(body_str) = std::str::from_utf8(&body) else {
        crate::log!("[Webhook] '{}' refused: body is not UTF-8", hook.name);
        return refused();
    };

    let secret = match hook.hmac.as_ref() {
        Some(cfg) => match CredentialStore::get(&state.pool, &cfg.credential).await {
            Ok(found) => found.map(|c| c.auth_value),
            Err(e) => {
                crate::log!("[Webhook] '{}' credential lookup failed: {e}", hook.name);
                None
            }
        },
        None => None,
    };

    let presented = PresentedDelivery {
        authorization: header_str(&headers, "authorization"),
        signature_header: hook
            .hmac
            .as_ref()
            .and_then(|cfg| header_str(&headers, &cfg.signature_header)),
        timestamp_header: hook
            .hmac
            .as_ref()
            .and_then(|cfg| cfg.timestamp_header.as_deref())
            .and_then(|name| header_str(&headers, name)),
        body: body_str,
        now_unix: chrono::Utc::now().timestamp(),
    };

    if let Err(refusal) = webhooks::verify(&hook, &presented, secret.as_deref()) {
        crate::log!("[Webhook] '{}' refused: {}", hook.name, refusal.reason());
        return refused();
    }

    // Everything below runs only for a delivery that authenticated. A public
    // caller must not be able to write the ledger, or learn from a `duplicate`
    // answer which delivery ids this hook has seen.
    let window = hook
        .dedupe
        .as_ref()
        .map(|cfg| cfg.window_secs)
        .filter(|secs| *secs > 0);
    let key = window.map(|_| {
        let sent_id = hook
            .dedupe
            .as_ref()
            .and_then(|cfg| cfg.header.as_deref())
            .and_then(|name| header_str(&headers, name));
        webhooks::dedupe_key(sent_id, body_str)
    });
    // Proof this delivery owns its claim, carried to the write that finishes
    // it. An emit slower than the window can be superseded mid-flight, and the
    // token is what stops the loser touching the winner's row.
    let mut claim_id = None;
    if let (Some(key), Some(window)) = (key.as_deref(), window) {
        match DeliveryLedger::claim(&state.pool, hook.id, key, window).await {
            Ok(Claim::Won { claim_id: won }) => claim_id = Some(won),
            Ok(Claim::Duplicate { event_id }) => {
                crate::log!("[Webhook] '{}' ignored a resend", hook.name);
                return duplicate(event_id);
            }
            Err(e) => {
                // Fail closed. A claim we could not take is not a claim we won.
                // The sender owns retrying, so refusing costs one retry while
                // guessing costs a duplicate event.
                crate::log!("[Webhook] '{}' could not claim a delivery: {e}", hook.name);
                return emit_failed();
            }
        }
    }

    let payload = delivery_payload(
        body_str,
        &hook.name,
        carried_headers(&headers, &hook.headers),
    );
    let actor = MessageOrigin::Webhook {
        webhook_id: hook.id.to_string(),
        name: hook.name.clone(),
    };
    match state
        .engine
        // No trigger: a webhook delivery arrives from outside the workspace, so
        // no fire of ours emitted it and it must wake every subscriber.
        .emit_domain_event(&hook.event_type, payload, Some(actor), None)
        .await
    {
        Ok(event_id) => {
            if let (Some(key), Some(claim)) = (key.as_deref(), claim_id) {
                if let Err(e) =
                    DeliveryLedger::record_event(&state.pool, hook.id, key, claim, event_id).await
                {
                    // The event is out and the claim still stands, so this
                    // cannot double-emit. A later resend answers `duplicate`
                    // with no id rather than with this one.
                    crate::log!("[Webhook] '{}' could not record the event: {e}", hook.name);
                }
            }
            crate::log!(
                "[Webhook] '{}' fired {} ({})",
                hook.name,
                hook.event_type,
                event_id
            );
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "event_id": event_id.to_string() })),
            )
                .into_response()
        }
        Err(e) => {
            crate::log!("[Webhook] '{}' could not emit: {e}", hook.name);
            if let (Some(key), Some(claim)) = (key.as_deref(), claim_id) {
                // Hand the claim back. Holding it would answer the sender's
                // retry with `duplicate` for an event that never happened,
                // which is the one way deduping could lose a delivery.
                if let Err(e) = DeliveryLedger::release(&state.pool, hook.id, key, claim).await {
                    crate::log!("[Webhook] '{}' could not release a claim: {e}", hook.name);
                }
            }
            emit_failed()
        }
    }
}

/// What a resend gets, which depends on whether the delivery it duplicates has
/// produced anything yet.
///
/// **An emitted event answers 200**, carrying its id. The work is done, so the
/// sender must stop retrying.
///
/// **A claim with no event yet answers 503**, because the holder is still in
/// flight and may yet fail. Telling this sender "already handled" would be a
/// promise nobody has kept: the holder's emit can fail, release the claim, and
/// leave no request anywhere still trying to deliver. A retryable answer costs
/// one more request and cannot lose the delivery. The retry then finds either a
/// recorded event, answered 200 above, or a free key it claims and emits.
fn duplicate(event_id: Option<Uuid>) -> Response {
    let Some(event_id) = event_id else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "a delivery with this id is still being handled, retry shortly",
                "duplicate": true,
            })),
        )
            .into_response();
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "duplicate": true,
            "event_id": event_id.to_string(),
        })),
    )
        .into_response()
}

/// A delivery that authenticated but could not be turned into an event. The
/// sender should retry, so this is deliberately a 5xx.
fn emit_failed() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": "could not emit the event" })),
    )
        .into_response()
}

/// The allow-listed headers this delivery actually carried.
///
/// Keyed by the name as configured, so a condition matches the spelling the
/// user wrote rather than a normalised one. An absent header is simply missing
/// from the map.
fn carried_headers(
    headers: &HeaderMap,
    allowed: &[String],
) -> serde_json::Map<String, serde_json::Value> {
    allowed
        .iter()
        .filter_map(|name| {
            header_str(headers, name)
                .map(|value| (name.clone(), serde_json::Value::String(value.to_string())))
        })
        .collect()
}

/// A trimmed, non-empty header value.
fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The event payload a delivery becomes: `{summary, headers, payload}`.
///
/// Three keys, always, whatever the sender posted. A condition reads
/// `payload.action` for the sender's own field and `headers.X-GitHub-Event` for
/// a carried header, both through the field paths of ADR 0119.
///
/// Wrapping is what makes a collision impossible rather than a rule about who
/// wins one. A sender with its own `headers` field is simply `payload.headers`.
///
/// `summary` is copied up when the sender provides one, because the timeline
/// reads it there and the sender's prose beats ours. The copy leaves
/// `payload.summary` intact.
fn delivery_payload(
    body: &str,
    hook_name: &str,
    headers: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    let sent = serde_json::from_str::<serde_json::Value>(body)
        .unwrap_or_else(|_| serde_json::Value::String(body.to_string()));
    let summary = sent
        .get("summary")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{hook_name} webhook fired"));
    serde_json::json!({
        "summary": summary,
        "headers": headers,
        "payload": sent,
    })
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/webhooks", get(list_webhooks).post(create_webhook))
        .route("/webhooks/:id", put(update_webhook).delete(delete_webhook))
        .route("/webhooks/:id/deliver", post(deliver))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::event_subscription::condition;

    fn no_headers() -> serde_json::Map<String, serde_json::Value> {
        serde_json::Map::new()
    }

    fn hmac_signed_with(header: &str) -> HmacConfig {
        HmacConfig {
            credential: "example-repo-webhook".into(),
            signature_header: header.into(),
            algorithm: Default::default(),
            encoding: Default::default(),
            prefix: None,
            signature_key: None,
            timestamp_header: None,
            timestamp_key: None,
            template: "{body}".into(),
            tolerance_secs: None,
        }
    }

    fn header_map(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        map
    }

    #[test]
    fn a_senders_own_fields_land_under_payload() {
        let payload = delivery_payload(r#"{"action":"opened","number":7}"#, "github", no_headers());
        assert_eq!(payload["payload"]["action"], "opened");
        assert_eq!(payload["payload"]["number"], 7);
        assert_eq!(payload["summary"], "github webhook fired");
    }

    #[test]
    fn a_summary_the_sender_wrote_is_copied_up_and_left_in_place() {
        let payload = delivery_payload(r#"{"summary":"build 42 failed"}"#, "ci", no_headers());
        assert_eq!(payload["summary"], "build 42 failed");
        assert_eq!(
            payload["payload"]["summary"], "build 42 failed",
            "copied, not moved: the sender's object is intact"
        );
    }

    /// The `body` special case is gone. Whatever the sender posted, a reader
    /// finds it at exactly one place.
    #[test]
    fn every_body_produces_the_same_three_keys() {
        for body in [r#"{"a":1}"#, "[1,2,3]", "not json at all", "17"] {
            let payload = delivery_payload(body, "ci", no_headers());
            let keys: Vec<&str> = payload
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(
                keys,
                vec!["headers", "payload", "summary"],
                "for body {body}"
            );
        }
        assert_eq!(
            delivery_payload("[1,2,3]", "ci", no_headers())["payload"],
            serde_json::json!([1, 2, 3])
        );
        assert_eq!(
            delivery_payload("not json at all", "ci", no_headers())["payload"],
            "not json at all"
        );
    }

    /// Wrapping exists so this cannot be a rule about who wins. A sender's own
    /// `headers` field is just another field it sent.
    #[test]
    fn a_sender_field_named_headers_does_not_collide() {
        let carried = carried_headers(
            &header_map(&[("x-github-event", "push")]),
            &["X-GitHub-Event".to_string()],
        );
        let payload = delivery_payload(r#"{"headers":{"mine":true}}"#, "github", carried);
        assert_eq!(payload["headers"]["X-GitHub-Event"], "push");
        assert_eq!(payload["payload"]["headers"]["mine"], true);
    }

    /// The invariant ADR 0119's field paths buy: both halves are addressable by
    /// a real condition, not just readable by hand.
    #[test]
    fn a_condition_resolves_both_a_sender_field_and_a_carried_header() {
        let carried = carried_headers(
            &header_map(&[("x-github-event", "pull_request")]),
            &["X-GitHub-Event".to_string()],
        );
        let payload = delivery_payload(r#"{"action":"opened"}"#, "github", carried);
        let matching = serde_json::json!({
            "payload.action": "opened",
            "headers.X-GitHub-Event": "pull_request",
        });
        assert!(condition::evaluate(Some(&matching), &payload));

        let wrong = serde_json::json!({ "payload.action": "closed" });
        assert!(!condition::evaluate(Some(&wrong), &payload));
    }

    #[test]
    fn only_allow_listed_headers_are_carried_and_an_absent_one_is_skipped() {
        let carried = carried_headers(
            &header_map(&[
                ("x-github-event", "push"),
                ("authorization", "Bearer supersecret"),
            ]),
            &["X-GitHub-Event".to_string(), "X-Not-Sent".to_string()],
        );
        assert_eq!(carried.len(), 1);
        assert_eq!(carried["X-GitHub-Event"], "push");
    }

    #[test]
    fn authorization_can_neither_be_carried_nor_deduped_on() {
        assert!(validate_carried_headers(&["Authorization".to_string()], None).is_err());
        assert!(validate_carried_headers(&["authorization".to_string()], None).is_err());
        assert!(validate_dedupe(&DedupeConfig {
            header: Some("Authorization".into()),
            window_secs: 3600,
        })
        .is_err());
    }

    #[test]
    fn a_hooks_own_signature_header_cannot_be_carried() {
        let hmac = hmac_signed_with("X-Hub-Signature-256");
        assert!(
            validate_carried_headers(&["x-hub-signature-256".to_string()], Some(&hmac)).is_err(),
            "matched case-insensitively, as HTTP header names are"
        );
        assert!(validate_carried_headers(&["X-GitHub-Event".to_string()], Some(&hmac)).is_ok());
    }

    #[test]
    fn a_window_outside_the_ledgers_bounds_is_refused() {
        let at_cap = DedupeConfig {
            header: None,
            window_secs: MAX_WINDOW_SECS,
        };
        assert!(validate_dedupe(&at_cap).is_ok());
        assert!(validate_dedupe(&DedupeConfig {
            window_secs: MAX_WINDOW_SECS + 1,
            ..at_cap.clone()
        })
        .is_err());
        assert!(validate_dedupe(&DedupeConfig {
            window_secs: -1,
            ..at_cap.clone()
        })
        .is_err());
        assert!(
            validate_dedupe(&DedupeConfig {
                window_secs: 0,
                ..at_cap
            })
            .is_ok(),
            "zero is the off switch, not an invalid window"
        );
    }

    /// A resend is only told "already handled" once something actually handled
    /// it. While the holder is still in flight, the honest answer is retryable:
    /// that holder can still fail and release, and a 200 here would have been
    /// the last word on a delivery nobody emitted.
    #[test]
    fn a_resend_is_acknowledged_only_once_an_event_exists() {
        assert_eq!(duplicate(Some(Uuid::new_v4())).status(), StatusCode::OK);
        assert_eq!(
            duplicate(None).status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "a claim with no event yet must leave the sender retrying"
        );
    }

    #[test]
    fn a_delivery_cannot_choose_the_event_type() {
        // `event_type` in the body is another field the sender sent, and stays
        // inert. `deliver` passes `hook.event_type` to `emit_domain_event`, and
        // nothing anywhere reads this field.
        let payload = delivery_payload(
            r#"{"event_type":"NotificationCreated"}"#,
            "hook",
            no_headers(),
        );
        assert_eq!(payload["payload"]["event_type"], "NotificationCreated");
    }
}
