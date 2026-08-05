//! Thread HTTP API handlers.
//!
//! Originally one large `api::threads` module; split by responsibility seam
//! into child modules. This barrel re-exports the handler surface its own
//! `router()`, `api/history.rs` (the two `/events/:event_id/*` routes) and
//! `api/search.rs` consume, so every `api::threads::*` path resolves
//! unchanged. The request/response shapes stay reachable at
//! `api::threads::<child>::<Type>`.

use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::Router;
use uuid::Uuid;

mod actions;
mod archive;
mod events_snapshot;
mod follow_up;
mod list;
mod search;

// Handler functions consumed by the router and the SearchEverywhere endpoint.
pub(super) use actions::{
    answer_thread_question, continue_thread, get_thread_messages, rename_thread, save_thread,
    suggest_title, unsave_thread,
};
pub(super) use archive::archive_thread;
pub(super) use events_snapshot::{
    get_context_capture, get_thread_events_snapshot, get_tool_result,
};
pub(super) use list::{
    count_thread_summaries, get_archived_count, get_filter_facets, get_older_threads,
    get_thread_summary, list_thread_summaries, list_threads,
};
pub(super) use search::{combined_thread_search, search_threads};

/// Extract a `thread_id` UUID from a JSON body that uses the
/// `{"thread_id": "<uuid>"}` shape. Used by save/unsave/rename/archive handlers.
fn extract_thread_uuid(request: &serde_json::Value) -> Result<Uuid, (StatusCode, String)> {
    let thread_id = request
        .get("thread_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing thread_id".to_string()))?;
    Uuid::parse_str(thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {}", e)))
}

#[derive(sqlx::FromRow)]
struct ThreadActionFacts {
    is_coding_agent: bool,
    status: String,
    archive_state: String,
    coding_agent_proposed: bool,
    blocking_descendant_count: i32,
    is_saved: bool,
    compose_text: Option<String>,
    compose_images: Option<serde_json::Value>,
}

/// Server-side mirror of the frontend's per-thread action availability: load
/// the DB-derivable facts from `thread_summaries` and run the same
/// `available_thread_actions` the codegen'd frontend uses. Mutating handlers
/// guard on this so a stale frontend or a raw API caller can't invoke an action
/// the user couldn't currently take (defense in depth — "frontend sends intent,
/// backend owns logic"). Returns `[]` for an unknown thread.
pub(in crate::api) async fn available_thread_actions_for(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<Vec<crate::engine::thread_lifecycle::Action>, sqlx::Error> {
    use crate::engine::thread_lifecycle::{
        available_thread_actions, ArchiveState, ThreadStatus, ThreadType,
    };
    let facts: Option<ThreadActionFacts> = sqlx::query_as(
        "SELECT is_coding_agent, status, archive_state, coding_agent_proposed,
                blocking_descendant_count, is_saved, compose_text, compose_images
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    let Some(f) = facts else {
        return Ok(vec![]);
    };
    let thread_type = if f.is_coding_agent {
        ThreadType::CodingAgent
    } else {
        ThreadType::Chat
    };
    let has_unsent_draft = f.compose_text.as_deref().is_some_and(|t| !t.is_empty())
        || f.compose_images
            .as_ref()
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty());
    Ok(available_thread_actions(
        thread_type,
        ThreadStatus::parse(&f.status),
        ArchiveState::parse(&f.archive_state),
        f.coding_agent_proposed,
        f.blocking_descendant_count > 0,
        has_unsent_draft,
        f.is_saved,
    ))
}

/// Routes for the `/threads*` URL surface. Grouped by path, not handler
/// location: compose / blob-upload / cc-diff / image handlers live in
/// sibling modules, but their thread-shaped routes register here so a
/// reader looking for `/threads/...` finds every route in one place.
pub(super) fn router() -> Router<super::AppState> {
    Router::new()
        .route("/threads", get(list_threads))
        .route("/threads", post(super::threads_compose::post_thread))
        .route("/threads/list", get(list_thread_summaries))
        .route("/threads/count", get(count_thread_summaries))
        .route("/threads/archived-count", get(get_archived_count))
        .route("/threads/search", get(search_threads))
        .route("/threads/save", post(save_thread))
        .route("/threads/unsave", post(unsave_thread))
        .route("/threads/rename", post(rename_thread))
        .route("/threads/archive", post(archive_thread))
        .route("/threads/suggest-title", post(suggest_title))
        .route("/threads/older", get(get_older_threads))
        .route("/threads/filter-facets", get(get_filter_facets))
        .route(
            "/threads/:thread_id/answer-question",
            post(answer_thread_question),
        )
        .route("/threads/:thread_id/messages", get(get_thread_messages))
        .route(
            "/threads/:thread_id/events",
            get(get_thread_events_snapshot),
        )
        .route("/threads/:thread_id/continue", post(continue_thread))
        // The parent-to-child edge. `:thread_id` is the CHILD; the parent is
        // whoever the thread-bound origin token says the caller is, never a
        // field on the request. Not a bare `/threads/<param>` leaf, so it is
        // free to use the descriptive param name the rest of the file uses
        // (only the bare leaf is constrained; see the note below).
        .route(
            "/threads/:thread_id/follow-up",
            post(follow_up::follow_up_child),
        )
        .route(
            "/threads/:thread_id/cc-diff",
            get(super::repositories::get_thread_cc_diff),
        )
        .route(
            "/threads/:thread_id/images",
            get(super::images::list_thread_images),
        )
        .route(
            "/threads/:thread_id/images/:index",
            get(super::images::get_thread_image),
        )
        // By-id summary GET shares the `/threads/:id` leaf with delete_thread.
        // It MUST use the `:id` param name: only ONE bare `/threads/<param>`
        // leaf may exist — a second one with a different param name makes
        // matchit panic at router build. Same path + non-overlapping methods
        // → axum merges GET+DELETE into one method router.
        .route(
            "/threads/:id",
            get(get_thread_summary).delete(super::threads_compose::delete_thread),
        )
        .route(
            "/threads/:id/compose",
            put(super::threads_compose::put_compose),
        )
        .route("/threads/:id/blobs", post(super::blobs::post_blob))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
