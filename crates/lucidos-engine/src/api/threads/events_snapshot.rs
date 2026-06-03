//! Thread events snapshot endpoint, the lazy-fetch context-capture /
//! tool-result endpoints, and the payload-stripping helpers the snapshot uses
//! to keep heavy threads loadable.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::AppState;
use crate::core::ThreadEventRow;

#[derive(Deserialize)]
pub struct ThreadEventsQuery {
    pub after: Option<i64>,
    /// When true, the snapshot keeps both `ContextCaptured.sections` +
    /// `tools` AND `ToolResult.result` inline instead of stripping them. The
    /// modal lazy-load is the default caller; `exportThread.ts` opts in so
    /// bug-report dumps stay complete.
    #[serde(default)]
    pub include_context: bool,
}

/// Response shape for `GET /api/v1/threads/:thread_id/events`. Wraps the event
/// rows with a `current_aggregate` snapshot of `thread_summaries` so the
/// frontend's historical-replay path applies meta from a fetched snapshot
/// — same source-of-truth model as live SSE's per-event aggregate.
#[derive(serde::Serialize)]
pub struct ThreadEventsSnapshot {
    pub events: Vec<ThreadEventRow>,
    #[serde(rename = "currentAggregate")]
    pub current_aggregate: Option<crate::core::store::ThreadAggregate>,
}

/// GET /api/v1/threads/:thread_id/events — snapshot of persisted thread events,
/// plus the current `thread_summaries` projection snapshot.
pub(in crate::api) async fn get_thread_events_snapshot(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    Query(query): Query<ThreadEventsQuery>,
) -> Result<Json<ThreadEventsSnapshot>, (StatusCode, String)> {
    let thread_uuid = Uuid::parse_str(&thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {}", e)))?;

    // Independent fetches against the same pool — run in parallel.
    let pool = state.engine.pool();
    let (events_res, aggregate_res) = tokio::join!(
        state
            .event_store
            .get_thread_events_by_seq(thread_uuid, query.after),
        crate::core::store::fetch_thread_aggregate(pool, thread_uuid),
    );

    let mut events = events_res.map_err(|e| {
        log!("[API] Failed to get thread events: {}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    for row in &mut events {
        strip_image_content_in_tool_result(row);
        if !query.include_context {
            strip_context_capture_sections(row);
            strip_tool_result_content(row);
        }
    }

    // Aggregate fetch is best-effort — absence just means no snapshot to apply
    // (frontend logs a warning).
    let current_aggregate = aggregate_res.unwrap_or_else(|e| {
        log!(
            "[API] Failed to fetch ThreadAggregate for {}: {}",
            thread_uuid,
            e
        );
        None
    });

    Ok(Json(ThreadEventsSnapshot {
        events,
        current_aggregate,
    }))
}

/// Replace `[IMAGE_CONTENT:...]\n<base64>` payloads in `ToolResult.result` with a small stub.
/// Rescues legacy threads (pre-write-time-strip) where `read_file` of an image inlined the
/// full base64 into the event payload — those threads are otherwise unloadable on mobile.
pub(super) fn strip_image_content_in_tool_result(row: &mut ThreadEventRow) {
    if row.event_type != "ToolResult" {
        return;
    }
    let Some(result_str) = row.payload.get("result").and_then(|v| v.as_str()) else {
        return;
    };
    if let Some(stub) = crate::engine::tools::files::strip_image_content_marker(result_str) {
        row.payload["result"] = serde_json::Value::String(stub);
    }
}

/// Lazy-fetch payload returned by `GET /events/:event_id/context` — the
/// pieces of a `ContextCaptured` event that the snapshot endpoint strips.
#[derive(serde::Serialize)]
pub struct ContextCapturePayload {
    pub sections: serde_json::Value,
    pub tools: serde_json::Value,
}

/// GET /api/v1/events/:event_id/context — returns the `sections` and `tools`
/// for one `ContextCaptured` event. The snapshot endpoint strips these because
/// a single capture can be ~50 kB and a heavy thread carries hundreds; the
/// step-detail modal fetches them on demand instead.
///
/// Keyed on `event_id` only (UUIDs are unguessable; the snapshot endpoint at
/// the same auth scope already exposes the event-id list). Routing only by
/// `event_id` removes a class of frontend bugs around picking the "right"
/// thread id at fetch time (`focusedThreadId` can be null mid-navigation or
/// belong to a different thread than the snap the modal is rendering).
pub(in crate::api) async fn get_context_capture(
    State(state): State<AppState>,
    Path(event_id): Path<String>,
) -> Result<Json<ContextCapturePayload>, (StatusCode, String)> {
    let event_uuid = Uuid::parse_str(&event_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid event_id: {}", e)))?;

    let row = state
        .event_store
        .get_event_by_id(event_uuid)
        .await
        .map_err(|e| {
            log!("[API] get_context_capture: db error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Event not found".to_string()))?;

    if row.event_type != "ContextCaptured" {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Event {} is not a ContextCaptured event", event_uuid),
        ));
    }

    // Distinguish "no sections" (row never had any — legitimate empty capture)
    // from "sections missing" (corrupted row, partial write). The modal's
    // `mergeContextCaptureSections` treats both arrays as authoritative, so
    // serving an empty 200 for a corrupted row would silently mask the
    // failure. Demand at least one of the two keys be present.
    let payload_obj = row
        .payload
        .as_object()
        .ok_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, "Payload not an object".to_string()))?;
    if !payload_obj.contains_key("sections") && !payload_obj.contains_key("tools") {
        return Err((
            StatusCode::NOT_FOUND,
            format!(
                "ContextCaptured {} has neither sections nor tools (row may be corrupted)",
                event_uuid
            ),
        ));
    }

    let sections = payload_obj
        .get("sections")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    let tools = payload_obj
        .get("tools")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));

    Ok(Json(ContextCapturePayload { sections, tools }))
}

/// Drop `sections` (and `tools`) from `ContextCaptured` payloads on the snapshot
/// path and stamp a `sections_stripped: true` marker so the frontend knows to
/// lazy-fetch via `GET /events/:event_id/context` when the user opens the
/// step-detail modal. A single section can be ~50 kB; a heavy thread can carry
/// 500+ captures = many MB of JSON that the events list never renders (only the
/// modal does). Keeping `producer / model / context_window /
/// estimated_total_tokens / usage / trimmed` preserves the inline budget chip.
pub(super) fn strip_context_capture_sections(row: &mut ThreadEventRow) {
    if row.event_type != "ContextCaptured" {
        return;
    }
    let Some(obj) = row.payload.as_object_mut() else {
        return;
    };
    obj.remove("sections");
    obj.remove("tools");
    obj.insert("sections_stripped".to_string(), serde_json::Value::Bool(true));
}

/// Drop `result` from `ToolResult` payloads on the snapshot path and stamp a
/// `result_stripped: true` marker so the frontend knows to lazy-fetch via
/// `GET /events/:event_id/tool-result` when the user opens the step-detail
/// modal. `ToolResult.result` is rendered ONLY inside the modal
/// (`StepDetailModal.tsx`'s `<pre class="step-detail-result">{step.result}</pre>`)
/// — never inline in the chat exchange. A single bash-output result can be
/// 150 kB+, and a busy CC thread carries hundreds of them = ~2 MB of JSON
/// that the events list never renders. Keeping `name` + `images` preserves
/// the inline step row + the generated-image rendering paths in
/// `thread-events.ts :: ToolResult` handler.
pub(super) fn strip_tool_result_content(row: &mut ThreadEventRow) {
    if row.event_type != "ToolResult" {
        return;
    }
    let Some(obj) = row.payload.as_object_mut() else {
        return;
    };
    obj.remove("result");
    obj.insert("result_stripped".to_string(), serde_json::Value::Bool(true));
}

/// Lazy-fetch payload returned by `GET /events/:event_id/tool-result` — the
/// stripped `result` field of one `ToolResult` event.
#[derive(serde::Serialize)]
pub struct ToolResultPayload {
    /// Original `result` string. `null` for image-only tool results (no
    /// textual result was ever written); the modal renders the inline
    /// images and elides the `<pre>` block in that case.
    pub result: serde_json::Value,
}

/// GET /api/v1/events/:event_id/tool-result — returns the `result` field for
/// one `ToolResult` event. The snapshot endpoint strips this because a single
/// bash result can be 150 kB+ and a heavy CC thread carries hundreds; the
/// step-detail modal fetches it on demand instead.
///
/// Keyed on `event_id` only — same routing contract as `get_context_capture`.
pub(in crate::api) async fn get_tool_result(
    State(state): State<AppState>,
    Path(event_id): Path<String>,
) -> Result<Json<ToolResultPayload>, (StatusCode, String)> {
    let event_uuid = Uuid::parse_str(&event_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid event_id: {}", e)))?;

    let row = state
        .event_store
        .get_event_by_id(event_uuid)
        .await
        .map_err(|e| {
            log!("[API] get_tool_result: db error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Event not found".to_string()))?;

    if row.event_type != "ToolResult" {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Event {} is not a ToolResult event", event_uuid),
        ));
    }

    let payload_obj = row
        .payload
        .as_object()
        .ok_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, "Payload not an object".to_string()))?;

    // `result` may legitimately be absent (image-only tool results); return
    // null so the modal can render its image-only state without 404-ing.
    let result = payload_obj
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok(Json(ToolResultPayload { result }))
}
