//! Archive all, and the unarchive its Undo calls (ADR 0349).
//!
//! All three routes are the owner's own buttons: `require_owner_device` runs
//! first, so no agent reaches them. An agent puts threads away through
//! triage, which needs the user's reply.
//!
//! The decisions live in `engine::thread_triage::archive_all`. Here they meet
//! the cascade: every archive is `archive_family`, leaving pinned members open.

use axum::{extract::State, http::HeaderMap, Json};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::AppState;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::thread_triage::archive_all::{preflight, still_safe, ArchiveAllPreflight};
use crate::engine::thread_triage::facts::{load, TriageScope};

use super::archive::{rejection_text, PinnedMembers};

/// The most ids one press may carry. A Current section past this is a
/// workspace to triage in passes, not a request to accept unbounded.
const MAX_IDS: usize = 2_000;

#[derive(Debug, Deserialize)]
pub(in crate::api) struct ThreadIdsRequest {
    thread_ids: Vec<Uuid>,
}

impl ThreadIdsRequest {
    fn ids(self) -> Result<Vec<Uuid>, ApiError> {
        if self.thread_ids.len() > MAX_IDS {
            return Err(ApiError::bad_request(format!(
                "At most {MAX_IDS} threads per request."
            )));
        }
        let mut ids = self.thread_ids;
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }
}

/// GET /api/v1/threads/archive-all-preflight: what Archive all would put away
/// from Current, and what stays, counted by reason.
pub(in crate::api) async fn archive_all_preflight(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ArchiveAllPreflight>, ApiError> {
    crate::api::actor::require_owner_device(&headers, &state.pool).await?;
    let rows = load(&state.pool, TriageScope::Inbox { except: None })
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(preflight(&rows)))
}

/// POST /api/v1/threads/archive-all `{thread_ids}`: archive the confirmed
/// threads that are still safe. Answers
/// `{archived: [...], kept: [{thread_id, reason}]}`. `archived` lists every
/// member the cascades took, which is exactly what Undo hands back.
pub(in crate::api) async fn archive_all(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ThreadIdsRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = crate::api::actor::require_owner_device(&headers, &state.pool).await?;
    let confirmed = request.ids()?;
    let rows = load(&state.pool, TriageScope::Ids(&confirmed))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let (safe, mut kept) = still_safe(&rows, &confirmed);

    let mut archived = Vec::new();
    for id in safe {
        match super::archive::archive_family(
            &state.engine,
            id,
            Some(actor.clone()),
            PinnedMembers::LeaveOpen,
        )
        .await
        {
            Ok(outcome) => archived.extend(outcome.archived),
            Err(rejection) => kept.push(crate::engine::thread_triage::archive_all::KeptThread {
                thread_id: id,
                reason: rejection_text(&rejection),
            }),
        }
    }
    Ok(Json(
        serde_json::json!({ "archived": archived, "kept": kept }),
    ))
}

/// POST /api/v1/threads/unarchive `{thread_ids}`: move archived threads back
/// to the inbox. Answers `{unarchived: [...]}`.
pub(in crate::api) async fn unarchive(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ThreadIdsRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = crate::api::actor::require_owner_device(&headers, &state.pool).await?;
    let ids = request.ids()?;
    let unarchived = unarchive_threads(&state.engine.event_bus, &state.pool, &ids, actor)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "unarchived": unarchived })))
}

/// Emit `ThreadUnarchived` for each id that is archived now, and return those.
/// An id in the inbox, discarded or unknown is left alone.
pub(crate) async fn unarchive_threads(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    ids: &[Uuid],
    actor: MessageOrigin,
) -> Result<Vec<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
    let archived: Vec<Uuid> = sqlx::query_scalar(
        "SELECT thread_id FROM thread_summaries \
         WHERE thread_id = ANY($1) AND archive_state = 'archived' AND state <> 'discarded'",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    for id in &archived {
        bus.emit(BusEvent::Thread {
            thread_id: *id,
            event: ThreadEvent::ThreadUnarchived,
            meta: EventMeta::with_actor(Some(actor.clone())),
        })
        .await?;
    }
    Ok(archived)
}

#[cfg(test)]
#[path = "archive_all_tests.rs"]
mod tests;
