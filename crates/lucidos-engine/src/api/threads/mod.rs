//! Thread HTTP API handlers.
//!
//! Originally one large `api::threads` module; split by responsibility seam
//! into child modules. This barrel re-exports the handler surface the router
//! (`api/mod.rs`) and `api/search.rs` consume so every `api::threads::*` path
//! resolves unchanged. The request/response shapes stay reachable at
//! `api::threads::<child>::<Type>`.

use axum::http::StatusCode;
use uuid::Uuid;

mod actions;
mod archive;
mod events_snapshot;
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
    count_thread_summaries, get_filter_facets, get_older_threads, get_thread_summary,
    list_thread_summaries, list_threads,
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

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
