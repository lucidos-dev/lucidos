//! Archive all, and the unarchive its Undo calls (ADR 0349).
//!
//! All three routes are the owner's own buttons: `require_owner_device` runs
//! first, so no agent reaches them. An agent puts threads away through
//! triage, which needs the user's reply.
//!
//! The decisions live in `engine::thread_triage::archive_all`. Here they meet
//! the cascade: every archive is `archive_family`, leaving pinned members open.

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::owner_door::{OnUnattested, OwnerAct};
use crate::api::AppState;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::thread_triage::archive_all::{
    completed_preflight, preflight, refusal_kept_slug, refuse_completed, still_safe,
    ArchiveAllPreflight, KeptThread,
};
use crate::engine::thread_triage::facts::{load, TriageScope};

use super::archive::{rejection_text, PinnedMembers};

/// The most ids one press may carry. A Current section past this is a
/// workspace to triage in passes, not a request to accept unbounded.
pub(crate) const MAX_IDS: usize = 2_000;

#[derive(Debug, Deserialize)]
pub(in crate::api) struct ThreadIdsRequest {
    thread_ids: Vec<Uuid>,
    #[serde(default)]
    parent_thread_id: Option<Uuid>,
    /// Unarchive only: bring each thread's sub-threads back too, as the thread
    /// menu's Move to Current does. Undo leaves it off and names every id.
    #[serde(default)]
    with_sub_threads: bool,
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

#[derive(Debug, Default, Deserialize)]
pub(in crate::api) struct ArchiveScope {
    parent_thread_id: Option<Uuid>,
}

/// GET /api/v1/threads/archive-all-preflight: what Archive all would put away
/// from Current, and what stays, counted by reason.
pub(in crate::api) async fn archive_all_preflight(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(scope): Query<ArchiveScope>,
) -> Result<Json<ArchiveAllPreflight>, ApiError> {
    crate::api::actor::require_owner_device(&headers, &state.pool, OnUnattested::Refuse).await?;
    let rows = load(
        &state.pool,
        scope.parent_thread_id.map_or(
            TriageScope::Inbox { except: None },
            TriageScope::Descendants,
        ),
    )
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(if scope.parent_thread_id.is_some() {
        completed_preflight(&rows)
    } else {
        preflight(&rows)
    }))
}

/// POST /api/v1/threads/archive-all `{thread_ids}`: archive the confirmed
/// threads that are still safe. Answers
/// `{archived: [...], kept: [{thread_id, reason, slug, thread_count}]}`. `archived` lists every
/// member the cascades took, which is exactly what Undo hands back.
pub(in crate::api) async fn archive_all(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ThreadIdsRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let act = format!("archive {} threads", request.thread_ids.len());
    let actor = crate::api::actor::require_owner_device(
        &headers,
        &state.pool,
        OnUnattested::Ask(OwnerAct::new(act)),
    )
    .await?;
    let parent = request.parent_thread_id;
    let confirmed = request.ids()?;
    if let Some(parent) = parent {
        return archive_completed(&state, parent, &confirmed, actor).await;
    }
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
            Err(rejection) => kept.push(KeptThread {
                thread_id: id,
                reason: rejection_text(&rejection),
                slug: refusal_kept_slug(&rejection.1),
                thread_count: rows
                    .iter()
                    .find(|r| r.facts.thread_id == id)
                    .map_or(1, |r| r.counted_threads()),
            }),
        }
    }
    Ok(Json(
        serde_json::json!({ "archived": archived, "kept": kept }),
    ))
}

/// Execute the counted ceiling. Fresh scope rejects forged or detached ids;
/// the archive path checks again while holding its family locks.
async fn archive_completed(
    state: &AppState,
    parent: Uuid,
    confirmed: &[Uuid],
    actor: MessageOrigin,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rows = load(&state.pool, TriageScope::Descendants(parent))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut archived = Vec::new();
    let mut kept = Vec::new();
    for id in confirmed {
        if archived.contains(id) {
            continue;
        }
        let row = rows.iter().find(|r| r.facts.thread_id == *id);
        let result = match refuse_completed(row) {
            Some(reason) => Err((reason, "not_completed")),
            None => super::archive::archive_completed_family(
                &state.engine,
                *id,
                actor.clone(),
                parent,
                confirmed,
            )
            .await
            .map_err(|r| (rejection_text(&r), refusal_kept_slug(&r.1))),
        };
        match result {
            Ok(outcome) => archived.extend(outcome.archived),
            Err((reason, slug)) => kept.push(KeptThread {
                thread_id: *id,
                reason,
                slug,
                thread_count: 1,
            }),
        }
    }
    archived.sort_unstable();
    archived.dedup();
    kept.retain(|k| !archived.contains(&k.thread_id));
    Ok(Json(
        serde_json::json!({"archived": archived, "kept": kept}),
    ))
}

/// POST /api/v1/threads/unarchive `{thread_ids, with_sub_threads?}`: move
/// archived threads back to the inbox. Answers `{unarchived: [...]}`.
pub(in crate::api) async fn unarchive(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ThreadIdsRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let act = format!("bring back {} archived threads", request.thread_ids.len());
    let actor = crate::api::actor::require_owner_device(
        &headers,
        &state.pool,
        OnUnattested::Ask(OwnerAct::new(act)),
    )
    .await?;
    let scope = if request.with_sub_threads {
        UnarchiveScope::WithSubThreads
    } else {
        UnarchiveScope::Exactly
    };
    let ids = request.ids()?;
    let unarchived = unarchive_threads(&state.engine.event_bus, &state.pool, &ids, scope, actor)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "unarchived": unarchived })))
}

/// Which threads an unarchive moves back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnarchiveScope {
    /// Exactly the ids given: Undo names every member of its batch.
    Exactly,
    /// The ids and every sub-thread under them, mirroring Archive's cascade.
    WithSubThreads,
}

/// Emit `ThreadUnarchived` for each thread in `scope` that is archived now,
/// parents first, and return those. A thread in the inbox, discarded or
/// unknown is left alone.
pub(crate) async fn unarchive_threads(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    ids: &[Uuid],
    scope: UnarchiveScope,
    actor: MessageOrigin,
) -> Result<Vec<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
    let archived: Vec<Uuid> = sqlx::query_scalar(
        "WITH RECURSIVE family AS ( \
             SELECT thread_id FROM thread_summaries WHERE thread_id = ANY($1) \
             UNION \
             SELECT t.thread_id FROM thread_summaries t \
             JOIN family f ON t.parent_thread_id = f.thread_id \
             WHERE $2 \
         ) \
         SELECT t.thread_id FROM thread_summaries t JOIN family f USING (thread_id) \
         WHERE t.archive_state = 'archived' AND t.state <> 'discarded' \
         ORDER BY t.depth, t.thread_id",
    )
    .bind(ids)
    .bind(scope == UnarchiveScope::WithSubThreads)
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
