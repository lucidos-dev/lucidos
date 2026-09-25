//! *Form requests*: a request from the agent that the user must act on.
//!
//! Five `ThreadEvent` variants open one (`FORM_REQUEST_EVENT_TYPES`), and one
//! `FormRequestResolved` closes it, keyed on `request_id`. Both are persisted,
//! so an unanswered request survives a lost stream frame, a reload and an
//! engine restart. The client reads the open ones from
//! `GET /api/v1/form-requests/pending` on every stream open.
//!
//! This module is the one place that decides whether a request is still open.
//! See `docs/plans/2026-09-24-form-requests-survive-a-reconnect.md`.

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, FormRequestOutcome, MessageOrigin, ThreadEvent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The request variants. Each carries `request_id` and a JSON `payload`.
pub const FORM_REQUEST_EVENT_TYPES: &[&str] = &[
    "CredentialRequested",
    "PluginInstallRequested",
    "PluginUninstallRequested",
    "EmailConfirmRequested",
    "OAuthAuthorizationRequested",
];

/// The kinds whose open state lives in engine memory, and so dies with it:
/// plugin staging and the OAuth callback listener. Expired at boot.
const MEMORY_BACKED_EVENT_TYPES: &[&str] = &[
    "PluginInstallRequested",
    "PluginUninstallRequested",
    "OAuthAuthorizationRequested",
];

/// Serializes every resolve, so a check and its emit cannot interleave with
/// another resolve of the same request. One engine owns a workspace database.
static RESOLVE_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// An open form request, as `GET /api/v1/form-requests/pending` serves it.
#[derive(Debug, Clone, Serialize)]
pub struct PendingFormRequest {
    pub thread_id: Uuid,
    pub request_id: Uuid,
    /// The request event as the stream carries it: `type` plus its payload,
    /// meta fields (`actor`, `request_event_id`) included.
    pub event: serde_json::Value,
}

/// SQL that is true when the request row `e` has a resolution.
const RESOLVED_SQL: &str = "EXISTS ( \
     SELECT 1 FROM events r \
     WHERE r.event_type = 'FormRequestResolved' \
       AND r.payload->>'request_id' = e.payload->>'request_id')";

/// Every open form request in a thread the user can still see: not archived,
/// not discarded. Oldest first.
pub async fn pending(pool: &PgPool) -> Result<Vec<PendingFormRequest>, sqlx::Error> {
    let rows: Vec<(Uuid, Option<String>, serde_json::Value)> = sqlx::query_as(&format!(
        "SELECT e.thread_id, e.payload->>'request_id', \
                e.payload || jsonb_build_object('type', e.event_type) \
         FROM events e \
         JOIN thread_summaries ts ON ts.thread_id = e.thread_id \
         WHERE e.event_type = ANY($1) \
           AND ts.archive_state <> 'archived' \
           AND NOT {RESOLVED_SQL} \
         ORDER BY e.created, e.sequence"
    ))
    .bind(FORM_REQUEST_EVENT_TYPES)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(thread_id, request_id, event)| {
            let request_id = request_id.as_deref().and_then(|s| s.parse().ok())?;
            Some(PendingFormRequest {
                thread_id,
                request_id,
                event,
            })
        })
        .collect())
}

/// One open request row, for the in-engine sweeps.
struct OpenRequest {
    request_id: Uuid,
    event_type: String,
    /// The request's own `payload` string, parsed.
    body: serde_json::Value,
}

/// Open requests of the given types, in one thread or in all of them.
async fn open_requests(
    pool: &PgPool,
    thread_id: Option<Uuid>,
    types: &[&str],
) -> Result<Vec<OpenRequest>, sqlx::Error> {
    let rows: Vec<(Option<String>, String, Option<String>)> = sqlx::query_as(&format!(
        "SELECT e.payload->>'request_id', e.event_type, e.payload->>'payload' \
         FROM events e \
         WHERE e.event_type = ANY($1) \
           AND ($2::uuid IS NULL OR e.thread_id = $2) \
           AND NOT {RESOLVED_SQL}"
    ))
    .bind(types)
    .bind(thread_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(request_id, event_type, payload)| {
            Some(OpenRequest {
                request_id: request_id.as_deref()?.parse().ok()?,
                event_type,
                body: payload
                    .as_deref()
                    .and_then(|p| serde_json::from_str(p).ok())
                    .unwrap_or(serde_json::Value::Null),
            })
        })
        .collect())
}

/// The variant name of the form request `request_id` names, if any does.
pub async fn event_type_of(pool: &PgPool, request_id: Uuid) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT event_type FROM events \
         WHERE event_type = ANY($1) AND payload->>'request_id' = $2 \
         LIMIT 1",
    )
    .bind(FORM_REQUEST_EVENT_TYPES)
    .bind(request_id.to_string())
    .fetch_optional(pool)
    .await
}

/// Close `request_id` with `outcome`, once.
///
/// Returns whether this call emitted the resolution. `false` means there is
/// nothing to close: no form request has that id (a plugin staged over HTTP, a
/// Settings-initiated OAuth flow), or it is already resolved.
pub async fn resolve(
    pool: &PgPool,
    bus: &EventBus,
    request_id: Uuid,
    outcome: FormRequestOutcome,
    actor: Option<MessageOrigin>,
) -> Result<bool, BoxError> {
    let _serialized = RESOLVE_LOCK.lock().await;
    let row: Option<(Uuid, bool)> = sqlx::query_as(&format!(
        "SELECT e.thread_id, {RESOLVED_SQL} \
         FROM events e \
         WHERE e.event_type = ANY($1) \
           AND e.payload->>'request_id' = $2 \
         LIMIT 1"
    ))
    .bind(FORM_REQUEST_EVENT_TYPES)
    .bind(request_id.to_string())
    .fetch_optional(pool)
    .await?;
    let Some((thread_id, false)) = row else {
        return Ok(false);
    };
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::FormRequestResolved {
            request_id,
            outcome,
        },
        meta: EventMeta::with_actor(actor),
    })
    .await?;
    Ok(true)
}

/// [`resolve`] for a caller that has no one to report a failure to: a sweep, a
/// listener task, an emit site whose own work already succeeded.
pub async fn resolve_or_log(
    pool: &PgPool,
    bus: &EventBus,
    request_id: Uuid,
    outcome: FormRequestOutcome,
    actor: Option<MessageOrigin>,
) {
    if let Err(e) = resolve(pool, bus, request_id, outcome, actor).await {
        crate::log!("[FormRequests] resolving {request_id} as {outcome:?} failed: {e}");
    }
}

/// Complete `request_id` when it names a request of `kind`. For a save or a
/// send that says which form it answers. The work itself already succeeded, so
/// a failure here is logged rather than reported.
pub async fn complete_named_request(
    pool: &PgPool,
    bus: &EventBus,
    request_id: Uuid,
    kind: &str,
    actor: Option<MessageOrigin>,
) {
    match event_type_of(pool, request_id).await {
        Ok(Some(found)) if found == kind => {
            resolve_or_log(pool, bus, request_id, FormRequestOutcome::Completed, actor).await
        }
        Ok(found) => {
            crate::log!("[FormRequests] {request_id} is {found:?}, not a {kind}; left open")
        }
        Err(e) => crate::log!("[FormRequests] looking up {request_id} failed: {e}"),
    }
}

/// Resolve every open request `pick` selects. For the sweeps.
async fn resolve_open_where(
    pool: &PgPool,
    bus: &EventBus,
    thread_id: Option<Uuid>,
    types: &[&str],
    outcome: FormRequestOutcome,
    actor: Option<MessageOrigin>,
    pick: impl Fn(&OpenRequest) -> bool,
) {
    let open = match open_requests(pool, thread_id, types).await {
        Ok(open) => open,
        Err(e) => {
            crate::log!("[FormRequests] open-request query failed: {e}");
            return;
        }
    };
    for request in open.iter().filter(|r| pick(r)) {
        resolve_or_log(pool, bus, request.request_id, outcome, actor.clone()).await;
    }
}

/// What a request is about, when a newer one for the same thing replaces it.
/// `None` for a kind where two open at once are both valid (two emails).
fn subject(event_type: &str, body: &serde_json::Value) -> Option<String> {
    let key = match event_type {
        "CredentialRequested" => "service",
        "PluginInstallRequested" | "PluginUninstallRequested" => "plugin_id",
        _ => return None,
    };
    body[key].as_str().map(str::to_string)
}

/// Supersede the thread's open requests for the same subject as `event`.
/// Run just before `event` is emitted, so only the newest stays open.
pub async fn supersede_same_subject(
    pool: &PgPool,
    bus: &EventBus,
    thread_id: Uuid,
    event: &ThreadEvent,
) {
    let (event_type, payload) = match event {
        ThreadEvent::CredentialRequested { payload, .. }
        | ThreadEvent::PluginInstallRequested { payload, .. }
        | ThreadEvent::PluginUninstallRequested { payload, .. } => (event.event_type(), payload),
        _ => return,
    };
    let body = serde_json::from_str(payload).unwrap_or(serde_json::Value::Null);
    let Some(wanted) = subject(event_type, &body) else {
        return;
    };
    resolve_open_where(
        pool,
        bus,
        Some(thread_id),
        &[event_type],
        FormRequestOutcome::Superseded,
        None,
        |open| subject(&open.event_type, &open.body).as_deref() == Some(wanted.as_str()),
    )
    .await;
}

/// Emit a new form request, superseding the older ones it replaces first.
/// The one emitter for a sentinel-born request, shared by both agentic loops.
pub async fn emit_request(
    pool: &PgPool,
    bus: &EventBus,
    thread_id: Uuid,
    event: ThreadEvent,
    meta: EventMeta,
    label: &str,
) {
    supersede_same_subject(pool, bus, thread_id, &event).await;
    bus.emit_or_log(
        BusEvent::Thread {
            thread_id,
            event,
            meta,
        },
        label,
    )
    .await;
}

/// A new user message answers the thread's open forms, as it does a permission
/// card. The OAuth page is left to its own flow, which is still listening.
pub async fn supersede_on_user_message(
    pool: &PgPool,
    bus: &EventBus,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) {
    resolve_open_where(
        pool,
        bus,
        Some(thread_id),
        &[
            "CredentialRequested",
            "PluginInstallRequested",
            "PluginUninstallRequested",
            "EmailConfirmRequested",
        ],
        FormRequestOutcome::Superseded,
        actor,
        |_| true,
    )
    .await;
}

/// A saved credential answers every open request for its service, whichever
/// form or device saved it.
pub async fn complete_credential_requests_for(
    pool: &PgPool,
    bus: &EventBus,
    service_name: &str,
    actor: Option<MessageOrigin>,
) {
    resolve_open_where(
        pool,
        bus,
        None,
        &["CredentialRequested"],
        FormRequestOutcome::Completed,
        actor,
        |open| open.body["service"].as_str() == Some(service_name),
    )
    .await;
}

/// Whether `request` can still be answered, as far as engine memory can say.
///
/// A plugin request whose staging is gone cannot: its Confirm would 404. It is
/// filtered out rather than resolved, because a Confirm in flight pops the
/// staging before it records its own outcome.
pub fn is_answerable(request: &PendingFormRequest, is_staged: impl Fn(&str, Uuid) -> bool) -> bool {
    match request.event["type"].as_str() {
        Some(kind @ ("PluginInstallRequested" | "PluginUninstallRequested")) => {
            is_staged(kind, request.request_id)
        }
        _ => true,
    }
}

/// Boot: expire every open request whose state lived in engine memory. The
/// staging and the listeners died with the previous engine.
pub async fn expire_memory_backed_requests(pool: &PgPool, bus: &EventBus) {
    resolve_open_where(
        pool,
        bus,
        None,
        MEMORY_BACKED_EVENT_TYPES,
        FormRequestOutcome::Expired,
        None,
        |_| true,
    )
    .await;
}

#[cfg(test)]
#[path = "form_requests_tests.rs"]
mod tests;
