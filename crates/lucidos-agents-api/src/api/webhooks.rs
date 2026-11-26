//! INVARIANT: Webhooks: the user's CRUD, and the one route a sender actually calls.
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
use crate::core::credentials::{AuthType, Credential};
use crate::core::webhook_deliveries::{DeliveryKey, DeliveryOutcome, MAX_WINDOW_SECS};
use crate::core::webhook_ingress::{AddressProbe, Family};
use crate::core::webhook_refusal::{self, RefusalCause};
use crate::core::webhooks::{
    self, DedupeConfig, DeliveryRefusal, HmacChange, HmacConfig, PresentedDelivery, RefusalRun,
    Webhook, WebhookConfig, WebhookPatch, WebhookStore,
};
use crate::core::{webhook_probe_token, CredentialStore};
use crate::engine::thread_events::MessageOrigin;
use axum::body::Bytes;

#[derive(Serialize)]
struct WebhookRow {
    id: String,
    name: String,
    event_type: String,
    enabled: bool,
    signed: bool,
    hmac: Option<HmacConfig>,
    dedupe: Option<DedupeConfig>,
    headers: Vec<String>,
    created_at: String,
    last_accepted_at: Option<String>,
    last_refused_at: Option<String>,
    last_refusal_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refusal_run: Option<RefusalRun>,
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
            last_accepted_at: hook.last_accepted_at.map(|t| t.to_rfc3339()),
            last_refused_at: hook.last_refused_at.map(|t| t.to_rfc3339()),
            last_refusal_reason: hook.last_refusal_reason,
            refusal_run: hook.refusal_run.is_running().then_some(hook.refusal_run),
        }
    }
}

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
    signing_secret: Option<SigningSecret>,
    #[serde(default)]
    dedupe: Option<DedupeConfig>,
    #[serde(default)]
    headers: Vec<String>,
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum SigningSecret {
    Generate,
    Provided { value: String },
}

#[derive(Serialize)]
struct WebhookWithToken {
    #[serde(flatten)]
    webhook: WebhookRow,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signing_secret: Option<String>,
}

async fn create_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateWebhookRequest>,
) -> Result<Json<WebhookWithToken>, ApiError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("a webhook needs a name"));
    }
    let event_type = body.event_type.trim();
    crate::core::event_subscription::validate_emittable_event_type(event_type)
        .map_err(ApiError::bad_request)?;
    let bringing_secret = body.signing_secret.is_some();
    if let Some(hmac) = body.hmac.as_ref() {
        validate_hmac_shape(hmac)?;
        if !bringing_secret {
            require_credential(&state, &hmac.credential).await?;
        }
    } else if bringing_secret {
        return Err(ApiError::bad_request(
            "signing_secret needs an hmac block, which is what names the credential",
        ));
    }
    if let Some(dedupe) = body.dedupe.as_ref() {
        validate_dedupe(dedupe)?;
    }
    let carried = validate_carried_headers(&body.headers, body.hmac.as_ref())?;

    let credential = body.hmac.as_ref().map(|hmac| hmac.credential.clone());
    let pending = match (body.signing_secret, credential.as_deref()) {
        (Some(secret), Some(name)) => {
            let existing = saved_credential(&state, name).await?;
            let pending = check_signing_secret(secret, name, existing.as_ref(), None)?;
            Some(pending.resolvable(&state.pool, existing.as_ref()).await?)
        }
        _ => None,
    };

    let actor = super::actor::user_actor(&headers, None);
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
        actor.clone(),
    )
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    let signing_secret = match (pending, credential.as_deref()) {
        (Some(pending), Some(name)) => pending.write(&state, name, None, actor).await?,
        _ => None,
    };
    Ok(Json(WebhookWithToken {
        webhook: WebhookRow::from_hook(hook, &slug),
        token,
        signing_secret,
    }))
}

struct PendingSecret {
    value: String,
    minted: Option<String>,
}

fn check_signing_secret(
    secret: SigningSecret,
    credential: &str,
    existing: Option<&Credential>,
    may_replace: Option<&str>,
) -> Result<PendingSecret, ApiError> {
    if existing.is_some() && may_replace != Some(credential) {
        return Err(ApiError::bad_request(format!(
            "a credential named '{credential}' already exists: name that one \
             instead of writing over it"
        )));
    }
    let (value, minted) = match secret {
        SigningSecret::Generate => {
            let value = webhooks::mint_token().map_err(|e| ApiError::internal(e.to_string()))?;
            (value.clone(), Some(value))
        }
        SigningSecret::Provided { value } => {
            if value.is_empty() {
                return Err(ApiError::bad_request("the signing secret is empty"));
            }
            if value.trim() != value {
                return Err(ApiError::bad_request(
                    "the signing secret starts or ends with whitespace: remove it, \
                     or no delivery will verify",
                ));
            }
            (value, None)
        }
    };
    Ok(PendingSecret { value, minted })
}

impl PendingSecret {
    async fn resolvable(
        self,
        pool: &sqlx::PgPool,
        existing: Option<&Credential>,
    ) -> Result<Self, ApiError> {
        let auth_type = existing.map_or(AuthType::Secret, |c| c.auth_type);
        crate::core::credentials::check_secret_references(pool, auth_type, &self.value)
            .await
            .map_err(ApiError::bad_request)?;
        Ok(self)
    }

    async fn write(
        self,
        state: &AppState,
        credential: &str,
        existing: Option<&Credential>,
        actor: Option<MessageOrigin>,
    ) -> Result<Option<String>, ApiError> {
        CredentialStore::upsert(
            &state.pool,
            &state.engine.event_bus,
            credential,
            existing.map_or(&[][..], |c| c.base_urls.as_slice()),
            existing.map_or(AuthType::Secret, |c| c.auth_type),
            &self.value,
            existing.map(|c| c.auth_header.as_str()),
            existing.and_then(|c| c.env_var_name.as_deref()),
            actor,
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(self.minted)
    }
}

async fn saved_credential(state: &AppState, name: &str) -> Result<Option<Credential>, ApiError> {
    CredentialStore::get(&state.pool, name)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))
}

fn validate_hmac_shape(hmac: &HmacConfig) -> Result<(), ApiError> {
    if hmac.credential.trim().is_empty() {
        return Err(ApiError::bad_request(
            "credential names the saved secret this hook verifies with",
        ));
    }
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
    Ok(())
}

async fn require_credential(state: &AppState, credential: &str) -> Result<(), ApiError> {
    let known = CredentialStore::get(&state.pool, credential)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if known.is_none() {
        return Err(ApiError::bad_request(format!(
            "no credential named '{credential}': save the signing secret first"
        )));
    }
    Ok(())
}

const AUTHORIZATION: &str = "authorization";

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
             a delivery record older than that",
            MAX_WINDOW_SECS
        )));
    }
    Ok(())
}

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
    hmac: HmacChange,
    #[serde(default)]
    signing_secret: Option<SigningSecret>,
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
) -> Result<Json<WebhookWithToken>, ApiError> {
    if let Some(event_type) = body.event_type.as_deref() {
        crate::core::event_subscription::validate_emittable_event_type(event_type.trim())
            .map_err(ApiError::bad_request)?;
    }
    if let Some(dedupe) = body.dedupe.as_ref() {
        validate_dedupe(dedupe)?;
    }
    if let HmacChange::Set(hmac) = &body.hmac {
        validate_hmac_shape(hmac)?;
    }

    let touches_verifier = body.hmac != HmacChange::Keep || body.signing_secret.is_some();
    let stored = if body.headers.is_some() || touches_verifier {
        Some(
            WebhookStore::get(&state.pool, id)
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?
                .ok_or_else(|| ApiError::bad_request("no webhook with that id"))?,
        )
    } else {
        None
    };

    let effective_hmac = match &body.hmac {
        HmacChange::Keep => stored.as_ref().and_then(|h| h.hmac.clone()),
        HmacChange::Set(hmac) => Some(hmac.clone()),
        HmacChange::Clear => None,
    };
    if body.hmac == HmacChange::Clear && stored.as_ref().is_some_and(|h| h.hmac.is_none()) {
        return Err(ApiError::bad_request(
            "this webhook does not sign, so there is no signature to remove",
        ));
    }
    let carried = match (body.headers.as_deref(), stored.as_ref()) {
        (Some(requested), _) => Some(validate_carried_headers(
            requested,
            effective_hmac.as_ref(),
        )?),
        (None, Some(hook)) => {
            validate_carried_headers(&hook.headers, effective_hmac.as_ref())?;
            None
        }
        (None, None) => None,
    };

    let credential = effective_hmac.as_ref().map(|h| h.credential.clone());
    let mut existing = None;
    let pending = match body.signing_secret {
        Some(secret) => {
            let name = credential.as_deref().ok_or_else(|| {
                ApiError::bad_request(
                    "signing_secret needs a signed webhook: send an hmac block, \
                     or drop the secret",
                )
            })?;
            existing = saved_credential(&state, name).await?;
            let may_replace = stored
                .as_ref()
                .and_then(|h| h.hmac.as_ref())
                .map(|h| &*h.credential);
            let pending = check_signing_secret(secret, name, existing.as_ref(), may_replace)?;
            Some(pending.resolvable(&state.pool, existing.as_ref()).await?)
        }
        None => {
            if let HmacChange::Set(hmac) = &body.hmac {
                require_credential(&state, &hmac.credential).await?;
            }
            None
        }
    };

    let actor = super::actor::user_actor(&headers, None);
    let slug = workspace_slug(&state);
    let (updated, token) = WebhookStore::update(
        &state.pool,
        &state.engine.event_bus,
        id,
        WebhookPatch {
            name: body.name.as_deref().map(|s| s.trim().to_string()),
            event_type: body.event_type.as_deref().map(|s| s.trim().to_string()),
            enabled: body.enabled,
            hmac: body.hmac,
            dedupe: body.dedupe,
            headers: carried,
        },
        actor.clone(),
    )
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
    .ok_or_else(|| ApiError::bad_request("no webhook with that id"))?;
    let signing_secret = match (pending, credential.as_deref()) {
        (Some(pending), Some(name)) => {
            pending
                .write(&state, name, existing.as_ref(), actor)
                .await?
        }
        _ => None,
    };
    Ok(Json(WebhookWithToken {
        webhook: WebhookRow::from_hook(updated, &slug),
        token,
        signing_secret,
    }))
}

async fn delete_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let actor = super::actor::user_actor(&headers, None);
    let removed = WebhookStore::delete(&state.pool, &state.engine.event_bus, id, actor)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if !removed {
        return Err(ApiError::bad_request("no webhook with that id"));
    }
    Ok(StatusCode::NO_CONTENT)
}

fn refused() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "unauthorized" })),
    )
        .into_response()
}

fn is_probe_delivery(headers: &HeaderMap) -> bool {
    webhooks::presented_bearer(header_str(headers, "authorization"))
        .is_some_and(webhook_probe_token::is_probe_token)
}

async fn stamp_refused(
    state: &AppState,
    hook: &Webhook,
    headers: &HeaderMap,
    refusal: DeliveryRefusal,
) {
    if is_probe_delivery(headers) {
        return;
    }
    if let Err(e) = WebhookStore::record_refused(&state.pool, hook.id, refusal).await {
        crate::log_legacy!("[Webhook] '{}' could not stamp a refusal: {e}", hook.name);
    }
}

async fn stamp_accepted(state: &AppState, hook: &Webhook) {
    if let Err(e) = WebhookStore::record_accepted(&state.pool, hook.id).await {
        crate::log_legacy!("[Webhook] '{}' could not stamp an accept: {e}", hook.name);
    }
}

async fn signing_secret(
    pool: &sqlx::PgPool,
    hmac: Option<&HmacConfig>,
) -> Result<Option<String>, (StatusCode, String)> {
    let Some(cfg) = hmac else {
        return Ok(None);
    };
    let credential = CredentialStore::get(pool, &cfg.credential)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    match credential {
        None => Ok(None),
        Some(credential) => credential
            .secret(pool)
            .await
            .map(Some)
            .map_err(|reason| (StatusCode::SERVICE_UNAVAILABLE, reason)),
    }
}

async fn deliver(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let hook = match WebhookStore::get(&state.pool, id).await {
        Ok(Some(hook)) => hook,
        Ok(None) => return refused(),
        Err(e) => {
            crate::log_legacy!("[Webhook] could not load {id}: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "internal error" })),
            )
                .into_response();
        }
    };
    if !hook.enabled {
        stamp_refused(&state, &hook, &headers, DeliveryRefusal::Disabled).await;
        return refused();
    }

    let Ok(body_str) = std::str::from_utf8(&body) else {
        crate::log_legacy!("[Webhook] '{}' refused: body is not UTF-8", hook.name);
        stamp_refused(&state, &hook, &headers, DeliveryRefusal::BodyNotUtf8).await;
        return refused();
    };

    let secret = match signing_secret(&state.pool, hook.hmac.as_ref()).await {
        Ok(secret) => secret,
        Err((status, reason)) => {
            crate::log_legacy!(
                "[Webhook] '{}' credential lookup failed: {reason}",
                hook.name
            );
            let error = if status == StatusCode::SERVICE_UNAVAILABLE {
                "the signing secret cannot be read right now"
            } else {
                "internal error"
            };
            return (status, Json(serde_json::json!({ "error": error }))).into_response();
        }
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
        crate::log_legacy!("[Webhook] '{}' refused: {}", hook.name, refusal.reason());
        stamp_refused(&state, &hook, &headers, refusal).await;
        return refused();
    }

    if let Some(challenge) = webhooks::slack_url_verification(&hook, body_str) {
        stamp_accepted(&state, &hook).await;
        crate::log_legacy!("[Webhook] '{}' answered Slack's URL check", hook.name);
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "challenge": challenge })),
        )
            .into_response();
    }

    let delivery = hook
        .dedupe
        .as_ref()
        .filter(|cfg| cfg.window_secs > 0)
        .map(|cfg| DeliveryKey {
            webhook_id: hook.id,
            key: webhooks::dedupe_key(
                cfg.header
                    .as_deref()
                    .and_then(|name| header_str(&headers, name)),
                body_str,
            ),
            window_secs: cfg.window_secs,
        });
    let payload = delivery_payload(
        body_str,
        &hook.name,
        carried_headers(&headers, &hook.headers),
    );
    let actor = MessageOrigin::Webhook {
        webhook_id: hook.id.to_string(),
        name: hook.name.clone(),
    };
    let emitted = match &delivery {
        Some(delivery) => {
            state
                .engine
                .emit_webhook_delivery(&hook.event_type, payload, actor, delivery)
                .await
        }
        None => state
            .engine
            // INVARIANT: No trigger: a webhook delivery arrives from outside the workspace,
            // so no fire of ours emitted it and it must wake every subscriber.
            .emit_domain_event(&hook.event_type, payload, actor, None)
            .await
            .map(DeliveryOutcome::Emitted),
    };
    match emitted {
        Ok(DeliveryOutcome::Emitted(event_id)) => {
            stamp_accepted(&state, &hook).await;
            crate::log_legacy!(
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
        Ok(DeliveryOutcome::Duplicate(event_id)) => {
            crate::log_legacy!("[Webhook] '{}' ignored a resend", hook.name);
            stamp_accepted(&state, &hook).await;
            duplicate(event_id)
        }
        Err(e) => {
            crate::log_legacy!("[Webhook] '{}' could not emit: {e}", hook.name);
            emit_failed()
        }
    }
}

fn duplicate(event_id: Uuid) -> Response {
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "duplicate": true,
            "event_id": event_id.to_string(),
        })),
    )
        .into_response()
}

fn emit_failed() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": "could not emit the event" })),
    )
        .into_response()
}

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

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

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

#[derive(Serialize)]
pub(super) struct IngressStatus {
    degraded: Option<IngressOutage>,
}

#[derive(Serialize)]
struct IngressOutage {
    webhook_name: String,
    host: String,
    port: u16,
    families: Vec<Family>,
    addresses: Vec<AddressProbe>,
    down_since: String,
    down_secs: i64,
}

async fn ingress_status(State(state): State<AppState>) -> Result<Json<IngressStatus>, ApiError> {
    read_ingress_status(&state).await.map(Json)
}

pub(super) async fn read_ingress_status(state: &AppState) -> Result<IngressStatus, ApiError> {
    let declared = crate::scheduler::webhook_ingress::declared_outage(&state.pool)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let Some(outage) = declared else {
        return Ok(IngressStatus { degraded: None });
    };

    let hooks = WebhookStore::list(&state.pool)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if !hooks.iter().any(|hook| hook.enabled) {
        return Ok(IngressStatus { degraded: None });
    }

    Ok(IngressStatus {
        degraded: Some(IngressOutage {
            webhook_name: outage.webhook_name,
            host: outage.host,
            port: outage.port,
            families: outage.families,
            addresses: outage.addresses,
            down_since: outage.down_since.to_rfc3339(),
            down_secs: outage.down_secs,
        }),
    })
}

#[derive(Serialize)]
pub(super) struct RefusalStatus {
    refusing: Vec<WebhookRefusal>,
}

#[derive(Serialize)]
struct WebhookRefusal {
    webhook_id: String,
    webhook_name: String,
    enabled: bool,
    cause: RefusalCause,
    refusals: i64,
    reasons: std::collections::BTreeMap<String, i64>,
    refusing_since: String,
    refusing_secs: i64,
}

async fn refusal_status(State(state): State<AppState>) -> Result<Json<RefusalStatus>, ApiError> {
    read_refusal_status(&state).await.map(Json)
}

pub(super) async fn read_refusal_status(state: &AppState) -> Result<RefusalStatus, ApiError> {
    let declared = crate::scheduler::webhook_refusal::declared_refusals(&state.pool)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if declared.is_empty() {
        return Ok(RefusalStatus {
            refusing: Vec::new(),
        });
    }

    let hooks = WebhookStore::list(&state.pool)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let refusing = hooks
        .into_iter()
        .filter(|hook| declared.contains_key(&hook.id.to_string()))
        .filter_map(|hook| match webhook_refusal::judge(&hook) {
            webhook_refusal::RefusalVerdict::Clear => None,
            webhook_refusal::RefusalVerdict::Refusing(cause) => Some(WebhookRefusal {
                webhook_id: hook.id.to_string(),
                webhook_name: hook.name.clone(),
                enabled: hook.enabled,
                cause,
                refusals: hook.refusal_run.refusals,
                refusing_since: hook
                    .refusal_run
                    .since
                    .map(|at| at.to_rfc3339())
                    .unwrap_or_default(),
                refusing_secs: webhook_refusal::refusing_secs(&hook.refusal_run),
                reasons: hook.refusal_run.reasons,
            }),
        })
        .collect();
    Ok(RefusalStatus { refusing })
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/webhooks", get(list_webhooks).post(create_webhook))
        .route("/webhooks/ingress", get(ingress_status))
        .route("/webhooks/refusals", get(refusal_status))
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

    #[tokio::test]
    async fn a_failed_credential_read_is_an_error_and_not_a_missing_secret() {
        let unreachable = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(500))
            .connect_lazy("postgres://test@127.0.0.1:1/none")
            .unwrap();

        assert!(
            signing_secret(&unreachable, Some(&hmac_signed_with("X-Signature")))
                .await
                .is_err(),
            "a lookup that could not run must not answer None"
        );
        assert_eq!(
            signing_secret(&unreachable, None).await.unwrap(),
            None,
            "a hook that signs nothing reads no credential"
        );
    }

    #[tokio::test]
    async fn an_unreadable_signing_reference_answers_503() {
        let (pool, db) = crate::test_support::setup_test_db().await;
        crate::test_support::seed_credential(
            &pool,
            "example-repo-webhook",
            "",
            AuthType::Secret,
            &format!("op://Engineering/hook-{}/secret", Uuid::new_v4().simple()),
        )
        .await;

        let (status, reason) = signing_secret(&pool, Some(&hmac_signed_with("X-Signature")))
            .await
            .unwrap_err();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{reason}");
        assert!(reason.contains("example-repo-webhook"), "{reason}");

        pool.close().await;
        crate::test_support::teardown_test_db(&db).await;
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

    #[test]
    fn the_three_hmac_wire_forms_are_distinct() {
        fn parse(body: &str) -> HmacChange {
            serde_json::from_str::<UpdateWebhookRequest>(body)
                .expect(body)
                .hmac
        }
        assert_eq!(parse(r#"{"name":"x"}"#), HmacChange::Keep);
        assert_eq!(parse(r#"{"hmac":null}"#), HmacChange::Clear);
        let set =
            parse(r#"{"hmac":{"credential":"c","signature_header":"X-Sig","template":"{body}"}}"#);
        match set {
            HmacChange::Set(cfg) => assert_eq!(cfg.credential, "c"),
            other => panic!("an object must set, got {other:?}"),
        }
    }

    fn saved_as(name: &str, auth_type: AuthType) -> Credential {
        let mut credential = Credential::fixture(name, auth_type, "the-users-live-api-key");
        credential.base_urls = vec!["https://api.example.com".into()];
        credential
    }

    #[test]
    fn a_signing_secret_cannot_write_over_an_unrelated_credential() {
        let generate = || SigningSecret::Generate;

        assert!(check_signing_secret(generate(), "deploys-github", None, None).is_ok());
        assert!(
            check_signing_secret(generate(), "deploys-github", None, Some("old-name")).is_ok(),
            "a config repointed at a new credential creates it"
        );

        let own = saved_as("deploys-github", AuthType::Secret);
        assert!(
            check_signing_secret(
                generate(),
                "deploys-github",
                Some(&own),
                Some("deploys-github")
            )
            .is_ok(),
            "rotating the hook's own secret is the point of the field"
        );

        let theirs = saved_as("openai", AuthType::ApiKey);
        assert!(
            check_signing_secret(generate(), "openai", Some(&theirs), None).is_err(),
            "create may overwrite nothing"
        );
        assert!(
            check_signing_secret(generate(), "openai", Some(&theirs), Some("deploys-github"))
                .is_err(),
            "an update may overwrite only the credential the hook already names"
        );
    }

    #[test]
    fn a_pasted_secret_is_neither_trimmed_nor_emptied() {
        let paste = |value: &str| SigningSecret::Provided {
            value: value.to_string(),
        };
        let checked = check_signing_secret(paste("whsec_a.b-c"), "s", None, None).unwrap();
        assert_eq!(checked.value, "whsec_a.b-c");
        assert!(
            checked.minted.is_none(),
            "a pasted secret is never echoed back: the caller already has it"
        );

        assert!(check_signing_secret(paste(" whsec_pad "), "s", None, None).is_err());
        assert!(check_signing_secret(paste("whsec_pad\n"), "s", None, None).is_err());
        assert!(check_signing_secret(paste(""), "s", None, None).is_err());

        let minted = check_signing_secret(SigningSecret::Generate, "s", None, None).unwrap();
        assert_eq!(minted.value.len(), 64, "32 bytes as lowercase hex");
        assert_eq!(minted.minted.as_deref(), Some(minted.value.as_str()));
    }

    #[test]
    fn a_signing_secret_states_which_side_invented_it() {
        let generate: SigningSecret = serde_json::from_str(r#"{"mode":"generate"}"#).unwrap();
        assert!(matches!(generate, SigningSecret::Generate));
        let provided: SigningSecret =
            serde_json::from_str(r#"{"mode":"provided","value":"whsec_x"}"#).unwrap();
        match provided {
            SigningSecret::Provided { value } => assert_eq!(value, "whsec_x"),
            SigningSecret::Generate => panic!("a provided secret parsed as a generated one"),
        }
        assert!(serde_json::from_str::<SigningSecret>(r#"{"mode":"provided"}"#).is_err());
        assert!(serde_json::from_str::<SigningSecret>(r#"{"mode":"invent"}"#).is_err());
    }

    #[test]
    fn a_signature_config_that_cannot_verify_is_refused() {
        let ok = HmacConfig {
            template: "{body}".into(),
            ..hmac_signed_with("X-Hub-Signature-256")
        };
        assert!(validate_hmac_shape(&ok).is_ok());

        assert!(
            validate_hmac_shape(&HmacConfig {
                credential: "  ".into(),
                ..ok.clone()
            })
            .is_err(),
            "a config naming no credential verifies against nothing"
        );
        assert!(
            validate_hmac_shape(&HmacConfig {
                signature_header: " ".into(),
                ..ok.clone()
            })
            .is_err(),
            "no header means no signature to read"
        );
        assert!(
            validate_hmac_shape(&HmacConfig {
                template: "{timestamp}".into(),
                ..ok.clone()
            })
            .is_err(),
            "a signature over no body says nothing about the payload"
        );
        assert!(
            validate_hmac_shape(&HmacConfig {
                template: "v0:{timestamp}:{body}".into(),
                ..ok.clone()
            })
            .is_err(),
            "a timestamp template needs somewhere to read the timestamp"
        );
        assert!(validate_hmac_shape(&HmacConfig {
            template: "v0:{timestamp}:{body}".into(),
            timestamp_header: Some("X-Slack-Request-Timestamp".into()),
            ..ok
        })
        .is_ok());
    }

    #[test]
    fn only_the_engines_own_probe_is_exempt_from_the_run() {
        static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());

        let token = webhook_probe_token::mint().expect("mint");
        let bearer = |value: &str| header_map(&[("authorization", value)]);

        assert!(is_probe_delivery(&bearer(&format!("Bearer {token}"))));
        assert!(
            is_probe_delivery(&bearer(&format!("bearer {token}"))),
            "the scheme is matched case-insensitively, as HTTP says"
        );

        assert!(!is_probe_delivery(&HeaderMap::new()));
        assert!(!is_probe_delivery(&bearer("Bearer some-other-token")));
        assert!(!is_probe_delivery(&bearer("Bearer ")));
        assert!(!is_probe_delivery(&bearer(&token)), "no scheme, no match");

        webhook_probe_token::mint().expect("mint again");
        assert!(!is_probe_delivery(&bearer(&format!("Bearer {token}"))));

        webhook_probe_token::clear();
    }

    #[test]
    fn a_delivery_cannot_choose_the_event_type() {
        let payload = delivery_payload(
            r#"{"event_type":"NotificationCreated"}"#,
            "hook",
            no_headers(),
        );
        assert_eq!(payload["payload"]["event_type"], "NotificationCreated");
    }

    async fn booted_state(db: &str) -> (AppState, tempfile::TempDir) {
        let (engine, workspace) = crate::test_support::boot_mock_engine(db).await;
        let state = crate::api::permission_requests::tests::permission_test_state(
            engine,
            workspace.path().into(),
        )
        .await;
        (state, workspace)
    }

    async fn deliver_once(
        state: &AppState,
        id: Uuid,
        bearer: Option<&str>,
        body: &'static [u8],
    ) -> (StatusCode, serde_json::Value) {
        let mut headers = HeaderMap::new();
        if let Some(bearer) = bearer {
            headers.insert("authorization", format!("Bearer {bearer}").parse().unwrap());
        }
        let response = deliver(
            State(state.clone()),
            Path(id),
            headers,
            Bytes::from_static(body),
        )
        .await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn every_refusal_answers_alike_and_never_says_why() {
        if !crate::test_support::in_engine_child(
            "api::webhooks::tests::every_refusal_answers_alike_and_never_says_why",
        )
        .await
        {
            return;
        }
        let (_pool, db) = crate::test_support::setup_test_db().await;
        let (state, _workspace) = booted_state(&db).await;
        let (hook, token) = WebhookStore::create(
            &state.pool,
            &state.engine.event_bus,
            "refusal-bodies",
            "WebhookRefusalProbe",
            WebhookConfig::default(),
            None,
        )
        .await
        .unwrap();
        let token = token.expect("an unsigned hook prints its token");
        let expected = serde_json::json!({ "error": "unauthorized" });

        let mut answers = vec![
            deliver_once(&state, hook.id, Some("wrong-token"), b"{}").await,
            deliver_once(&state, hook.id, None, b"{}").await,
            deliver_once(&state, hook.id, Some(&token), &[0xff, 0xfe]).await,
        ];
        WebhookStore::update(
            &state.pool,
            &state.engine.event_bus,
            hook.id,
            WebhookPatch {
                enabled: Some(false),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap()
        .expect("the hook exists");
        answers.push(deliver_once(&state, hook.id, Some(&token), b"{}").await);

        let reason = WebhookStore::get(&state.pool, hook.id)
            .await
            .unwrap()
            .unwrap()
            .last_refusal_reason
            .expect("the refusal was recorded for the owner");
        for (status, body) in answers {
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(body, expected);
            assert!(!body.to_string().contains(&reason));
        }

        let pool = state.pool.clone();
        drop(state);
        pool.close().await;
        crate::test_support::teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_hook_cannot_be_pinned_to_a_reserved_event_type() {
        if !crate::test_support::in_engine_child(
            "api::webhooks::tests::a_hook_cannot_be_pinned_to_a_reserved_event_type",
        )
        .await
        {
            return;
        }
        let (_pool, db) = crate::test_support::setup_test_db().await;
        let (state, _workspace) = booted_state(&db).await;

        let created = create_webhook(
            State(state.clone()),
            HeaderMap::new(),
            Json(CreateWebhookRequest {
                name: "reserved-create".into(),
                event_type: "NotificationCreated".into(),
                hmac: None,
                signing_secret: None,
                dedupe: None,
                headers: Vec::new(),
            }),
        )
        .await;
        let refusal = created.err().expect("a reserved name is refused at create");
        assert_eq!(refusal.into_response().status(), StatusCode::BAD_REQUEST);

        let (hook, _) = WebhookStore::create(
            &state.pool,
            &state.engine.event_bus,
            "reserved-update",
            "WebhookReservedProbe",
            WebhookConfig::default(),
            None,
        )
        .await
        .unwrap();
        let updated = update_webhook(
            State(state.clone()),
            HeaderMap::new(),
            Path(hook.id),
            Json(UpdateWebhookRequest {
                name: None,
                event_type: Some("NotificationCreated".into()),
                enabled: None,
                hmac: HmacChange::Keep,
                signing_secret: None,
                dedupe: None,
                headers: None,
            }),
        )
        .await;
        let refusal = updated.err().expect("a reserved name is refused at update");
        assert_eq!(refusal.into_response().status(), StatusCode::BAD_REQUEST);

        let pool = state.pool.clone();
        drop(state);
        pool.close().await;
        crate::test_support::teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_failed_hook_write_leaves_the_signing_credential_untouched() {
        if !crate::test_support::in_engine_child(
            "api::webhooks::tests::a_failed_hook_write_leaves_the_signing_credential_untouched",
        )
        .await
        {
            return;
        }
        let (_pool, db) = crate::test_support::setup_test_db().await;
        let (state, _workspace) = booted_state(&db).await;
        let plain = |name: &str| {
            let state = state.clone();
            let name = name.to_string();
            async move {
                WebhookStore::create(
                    &state.pool,
                    &state.engine.event_bus,
                    &name,
                    "WebhookOrderProbe",
                    WebhookConfig::default(),
                    None,
                )
                .await
                .unwrap()
                .0
            }
        };
        let taken = plain("order-taken").await;

        let created = create_webhook(
            State(state.clone()),
            HeaderMap::new(),
            Json(CreateWebhookRequest {
                name: taken.name.clone(),
                event_type: "WebhookOrderProbe".into(),
                hmac: Some(HmacConfig {
                    credential: "order-created-secret".into(),
                    ..hmac_signed_with("X-Signature")
                }),
                signing_secret: Some(SigningSecret::Generate),
                dedupe: None,
                headers: Vec::new(),
            }),
        )
        .await;
        assert!(created.is_err(), "a duplicate name fails the hook write");
        assert!(
            CredentialStore::get(&state.pool, "order-created-secret")
                .await
                .unwrap()
                .is_none(),
            "no credential may be written for a hook that was not"
        );

        crate::test_support::seed_credential(
            &state.pool,
            "order-rotated-secret",
            "",
            AuthType::Secret,
            "old-secret",
        )
        .await;
        let (signed, _) = WebhookStore::create(
            &state.pool,
            &state.engine.event_bus,
            "order-signed",
            "WebhookOrderProbe",
            WebhookConfig {
                hmac: Some(HmacConfig {
                    credential: "order-rotated-secret".into(),
                    ..hmac_signed_with("X-Signature")
                }),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
        let updated = update_webhook(
            State(state.clone()),
            HeaderMap::new(),
            Path(signed.id),
            Json(UpdateWebhookRequest {
                name: Some(taken.name.clone()),
                event_type: None,
                enabled: None,
                hmac: HmacChange::Keep,
                signing_secret: Some(SigningSecret::Provided {
                    value: "new-secret".into(),
                }),
                dedupe: None,
                headers: None,
            }),
        )
        .await;
        assert!(updated.is_err(), "a duplicate name fails the hook write");
        let kept = CredentialStore::get(&state.pool, "order-rotated-secret")
            .await
            .unwrap()
            .expect("the credential still exists");
        assert_eq!(kept.secret(&state.pool).await.unwrap(), "old-secret");

        let pool = state.pool.clone();
        drop(state);
        pool.close().await;
        crate::test_support::teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn an_ingress_outage_goes_stale_when_no_enabled_webhook_is_left() {
        if !crate::test_support::in_engine_child(
            "api::webhooks::tests::an_ingress_outage_goes_stale_when_no_enabled_webhook_is_left",
        )
        .await
        {
            return;
        }
        let (_pool, db) = crate::test_support::setup_test_db().await;
        let (state, _workspace) = booted_state(&db).await;
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate) \
             VALUES ($1, 'WebhookIngressDegraded', $2, now(), 'hook', 'webhook')",
        )
        .bind(Uuid::new_v4())
        .bind(serde_json::json!({
            "type": "WebhookIngressDegraded",
            "data": {
                "webhook_id": "hook",
                "webhook_name": "probe",
                "host": "hooks.example.ts.net",
                "port": 8443,
                "degraded_families": ["ipv4"],
            },
        }))
        .execute(&state.pool)
        .await
        .unwrap();

        let none_left = read_ingress_status(&state).await.ok().unwrap();
        assert!(none_left.degraded.is_none(), "no webhook, so no retraction");

        let (hook, _) = WebhookStore::create(
            &state.pool,
            &state.engine.event_bus,
            "ingress-stale",
            "WebhookIngressProbe",
            WebhookConfig::default(),
            None,
        )
        .await
        .unwrap();
        let standing = read_ingress_status(&state).await.ok().unwrap();
        assert!(standing.degraded.is_some(), "an enabled hook keeps it");

        WebhookStore::update(
            &state.pool,
            &state.engine.event_bus,
            hook.id,
            WebhookPatch {
                enabled: Some(false),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap()
        .expect("the hook exists");
        let disabled = read_ingress_status(&state).await.ok().unwrap();
        assert!(disabled.degraded.is_none(), "only disabled hooks are left");

        let pool = state.pool.clone();
        drop(state);
        pool.close().await;
        crate::test_support::teardown_test_db(&db).await;
    }

    #[test]
    fn a_standing_refusal_run_is_listed_whole_and_an_ended_one_is_not() {
        let at = chrono::Utc::now();
        let hook = |refusals: i64| Webhook {
            id: Uuid::new_v4(),
            name: "run".into(),
            event_type: "WebhookRunProbe".into(),
            token_hash: None,
            hmac: None,
            dedupe: None,
            headers: Vec::new(),
            enabled: true,
            created_at: at,
            updated_at: at,
            last_accepted_at: None,
            last_refused_at: Some(at),
            last_refusal_reason: Some("probe".into()),
            refusal_run: RefusalRun {
                refusals,
                reasons: [
                    ("bad_signature".to_string(), refusals - 1),
                    ("probe".to_string(), 1),
                ]
                .into_iter()
                .collect(),
                ..Default::default()
            },
        };
        let standing = serde_json::to_value(WebhookRow::from_hook(hook(5), "ws")).unwrap();
        assert_eq!(standing["refusal_run"]["refusals"], 5);
        assert_eq!(standing["refusal_run"]["reasons"]["bad_signature"], 4);
        assert_eq!(standing["last_refusal_reason"], "probe");
        let ended = serde_json::to_value(WebhookRow::from_hook(hook(0), "ws")).unwrap();
        assert!(ended.get("refusal_run").is_none());
    }
}
