//! Thread events snapshot endpoint, the lazy-fetch context-capture,
//! tool-result and tool-args endpoints, and the payload-stripping helpers the
//! snapshot uses to keep heavy threads loadable.
//!
//! Every strip has a paired lazy fetch, and both sides ask the same predicate
//! which event types they cover (`is_tool_result_event`, `is_tool_call_event`).
//! A strip whose fetch does not recognise the type it stripped serves a 404
//! where the modal expects content.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::AppState;
use crate::core::events::HasEventPayload;
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
        strip_inline_image_payloads(row);
        if !query.include_context {
            strip_context_capture_sections(row);
            strip_tool_result_content(row);
            strip_tool_call_args(row);
        }
        // Sections that survive to the client are served verbatim, so a
        // pre-rename row has to be respelled the way the viewer reads it. Two
        // event types reach here with sections: `include_context` keeps a
        // `ContextCaptured`'s, and `ContextAssembled` is the retired
        // predecessor, which nothing strips. A stripped row has none left, so
        // this is a no-op there. Gated on the type for the same reason
        // `strip_context_capture_sections` is: another event's `sections` key
        // would mean something else.
        if matches!(
            row.event_type.as_str(),
            "ContextCaptured" | "ContextAssembled"
        ) {
            rename_legacy_section_size_in_payload(&mut row.payload);
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

/// The two event types carrying a tool's OUTPUT, one per channel. Every read
/// path that rewrites a `result` asks here rather than naming a type inline.
///
/// One predicate, because the chat channel's name was the only one the strips
/// knew for months. So the shape they never fired on was the coding-agent
/// thread, the heaviest a workspace holds. A third channel joins in one place.
pub(super) fn is_tool_result_event(event_type: &str) -> bool {
    matches!(event_type, "ToolResult" | "CodingAgentToolResult")
}

/// The event type carrying a coding agent's tool INPUT. Its chat-channel
/// sibling `ToolCalled` is deliberately absent: those args are small, and
/// `thread-sync.ts` reads the write target straight off them.
pub(super) fn is_tool_call_event(event_type: &str) -> bool {
    event_type == "CodingAgentToolCalled"
}

/// Rewrite a tool result's `result` in place when `strip` recognises its
/// sentinel. A no-op for any other event type, a missing `result`, or
/// unrecognised text.
///
/// Generic over `HasEventPayload` because the two read paths hold different row
/// types: the snapshot serves `ThreadEventRow`, the lazy fetch serves `EventRow`.
fn rewrite_tool_result<E: HasEventPayload>(row: &mut E, strip: impl Fn(&str) -> Option<String>) {
    if !is_tool_result_event(row.event_type()) {
        return;
    }
    let Some(result_str) = row.payload().get("result").and_then(|v| v.as_str()) else {
        return;
    };
    if let Some(stub) = strip(result_str) {
        row.payload_mut()["result"] = serde_json::Value::String(stub);
    }
}

/// Replace `[IMAGE_CONTENT:...]\n<base64>` payloads in `ToolResult.result` with a small stub.
/// Rescues legacy threads (pre-write-time-strip) where `read_file` of an image inlined the
/// full base64 into the event payload. Those threads are otherwise unloadable on mobile.
pub(super) fn strip_image_content_in_tool_result<E: HasEventPayload>(row: &mut E) {
    rewrite_tool_result(row, crate::engine::tools::files::strip_image_content_marker);
}

/// The same rescue for `[APP_CAPTURE:<base64>]\n<dom>` payloads, which the write
/// path only started stubbing on 2026-07-30. Every capture taken before that is
/// still sitting in the events table at full size (1.53 MB rows measured), and
/// no migration rewrites them, so this is what keeps them off the wire.
pub(super) fn strip_app_capture_in_tool_result<E: HasEventPayload>(row: &mut E) {
    rewrite_tool_result(row, crate::engine::strip_app_capture_marker);
}

/// Both inline-image rescues in one call. Read paths should use this rather
/// than picking a stripper: the app-capture half was missed for months
/// precisely because it was a separate decision at each call site.
pub(super) fn strip_inline_image_payloads<E: HasEventPayload>(row: &mut E) {
    strip_image_content_in_tool_result(row);
    strip_app_capture_in_tool_result(row);
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
    let payload_obj = row.payload.as_object().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Payload not an object".to_string(),
        )
    })?;
    if !payload_obj.contains_key("sections") && !payload_obj.contains_key("tools") {
        return Err((
            StatusCode::NOT_FOUND,
            format!(
                "ContextCaptured {} has neither sections nor tools (row may be corrupted)",
                event_uuid
            ),
        ));
    }

    let mut sections = payload_obj
        .get("sections")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    let tools = payload_obj
        .get("tools")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    rename_legacy_section_size(&mut sections);

    Ok(Json(ContextCapturePayload { sections, tools }))
}

/// Spell a stored section's size the way the client reads it today.
///
/// A section's budget delta was called `char_count` for months. `ContextSection`
/// carries `serde(alias = "char_count")` for that, but this endpoint and the
/// `include_context` snapshot serve `payload->'sections'` VERBATIM, so serde
/// never runs on them. Without this a months-old capture reaches the Context
/// Viewer with no size field at all, and every row renders `NaN`.
///
/// Only the key moves. `content_chars` stays absent, which is what absent
/// means: nobody measured it when the row was written.
pub(super) fn rename_legacy_section_size(sections: &mut serde_json::Value) {
    let Some(array) = sections.as_array_mut() else {
        return;
    };
    for section in array {
        let Some(obj) = section.as_object_mut() else {
            continue;
        };
        if obj.contains_key("budget_delta_chars") {
            continue;
        }
        if let Some(value) = obj.remove("char_count") {
            obj.insert("budget_delta_chars".to_string(), value);
        }
    }
}

/// [`rename_legacy_section_size`] applied to an event payload's `sections`.
pub(super) fn rename_legacy_section_size_in_payload(payload: &mut serde_json::Value) {
    let Some(sections) = payload.get_mut("sections") else {
        return;
    };
    rename_legacy_section_size(sections);
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
    obj.insert(
        "sections_stripped".to_string(),
        serde_json::Value::Bool(true),
    );
}

/// Drop `result` from a tool result's payload on the snapshot path and stamp a
/// `result_stripped: true` marker so the frontend knows to lazy-fetch via
/// `GET /events/:event_id/tool-result` when the user opens the step-detail
/// modal. A `result` is rendered ONLY inside the modal
/// (`StepDetailModal.tsx`'s `<pre class="step-detail-result">{step.result}</pre>`)
/// — never inline in the chat exchange. A single bash-output result can be
/// 150 kB+, and a busy coding-agent thread carries hundreds of them = ~2 MB of
/// JSON that the events list never renders. Keeping `name`, `images` and
/// `tool_use_id` preserves the inline step row, the generated-image rendering
/// paths, and the pairing the coding-agent arm settles its step by.
pub(super) fn strip_tool_result_content(row: &mut ThreadEventRow) {
    if !is_tool_result_event(&row.event_type) {
        return;
    }
    let Some(obj) = row.payload.as_object_mut() else {
        return;
    };
    obj.remove("result");
    obj.insert("result_stripped".to_string(), serde_json::Value::Bool(true));
}

/// Drop `args` from a coding agent's tool call on the snapshot path. Stamp an
/// `args_stripped: true` marker, so the frontend lazy-fetches via
/// `GET /events/:event_id/tool-args` when the user opens the step-detail modal.
///
/// `args` is the single heaviest thing a coding-agent snapshot carries: an
/// `Edit`'s two versions of a hunk, a `Write`'s whole file. Nothing inline
/// renders it. The modal's un-elided command line is its only reader
/// (`fullCommandForCCTool` in `exchange.ts`).
///
/// **`description` is filled first, and that ordering is the whole trick.**
/// The inline label reads `description || describeCCTool(name, args)`, so
/// dropping `args` from a row with no description would leave a bare tool
/// name. The write path has stamped one since May 2026 (`run_session/run.rs`),
/// so only older rows take this branch, and they take the very same function.
pub(super) fn strip_tool_call_args(row: &mut ThreadEventRow) {
    if !is_tool_call_event(&row.event_type) {
        return;
    }
    let name = row
        .payload
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let described = row
        .payload
        .get("description")
        .and_then(|v| v.as_str())
        .is_some_and(|d| !d.is_empty());
    let fallback = (!described).then(|| {
        let args = row
            .payload
            .get("args")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        crate::core::describe_cc_tool(&name, &args)
    });
    let Some(obj) = row.payload.as_object_mut() else {
        return;
    };
    if let Some(description) = fallback {
        obj.insert("description".to_string(), description.into());
    }
    obj.remove("args");
    obj.insert("args_stripped".to_string(), serde_json::Value::Bool(true));
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

    let mut row = state
        .event_store
        .get_event_by_id(event_uuid)
        .await
        .map_err(|e| {
            log!("[API] get_tool_result: db error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Event not found".to_string()))?;

    if !is_tool_result_event(&row.event_type) {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Event {} is not a tool result event", event_uuid),
        ));
    }

    // The modal renders this in a `<pre>`, so an inlined image is unreadable
    // noise at best. Legacy rows predate the write-time stubs and can be
    // multi-megabyte; this endpoint applied no stripping at all until now, so
    // it was the one read path still shipping them in full.
    strip_inline_image_payloads(&mut row);

    let payload_obj = row.payload.as_object().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Payload not an object".to_string(),
        )
    })?;

    // `result` may legitimately be absent (image-only tool results); return
    // null so the modal can render its image-only state without 404-ing.
    let result = payload_obj
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok(Json(ToolResultPayload { result }))
}

/// Lazy-fetch payload returned by `GET /events/:event_id/tool-args`, carrying
/// the stripped `args` field of one coding-agent tool call.
#[derive(serde::Serialize)]
pub struct ToolArgsPayload {
    /// Original `args` value. `null` for a call that recorded none, where the
    /// modal renders its label alone and elides the command block.
    pub args: serde_json::Value,
}

/// GET /api/v1/events/:event_id/tool-args: the `args` for one coding-agent
/// tool call. A single `Write` carries a whole file and a heavy thread carries
/// hundreds, so the snapshot strips it. The step-detail modal fetches it on
/// demand instead.
///
/// Keyed on `event_id` only, the same routing contract as `get_tool_result`.
pub(in crate::api) async fn get_tool_args(
    State(state): State<AppState>,
    Path(event_id): Path<String>,
) -> Result<Json<ToolArgsPayload>, (StatusCode, String)> {
    let event_uuid = Uuid::parse_str(&event_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid event_id: {}", e)))?;

    let row = state
        .event_store
        .get_event_by_id(event_uuid)
        .await
        .map_err(|e| {
            log!("[API] get_tool_args: db error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Event not found".to_string()))?;

    if !is_tool_call_event(&row.event_type) {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Event {} is not a coding-agent tool call", event_uuid),
        ));
    }

    let payload_obj = row.payload.as_object().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Payload not an object".to_string(),
        )
    })?;

    // Absent is a real answer, not a missing one: a tool can be called with no
    // arguments. Serving null lets the modal draw its label-only state rather
    // than a failure.
    let args = payload_obj
        .get("args")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok(Json(ToolArgsPayload { args }))
}

/// Where one event lives, returned by `GET /events/:event_id/location`.
///
/// `thread_id` is `None` for an event that belongs to no conversation, i.e.
/// anything whose `aggregate` is not `thread` (a workspace *domain event*, an
/// app event, a trigger event). That is a real answer, not a missing one: the
/// events insert derives the column as
/// `CASE WHEN aggregate = 'thread' THEN aggregate_id ELSE NULL END`
/// (`engine/event_bus`), so a null here means "nowhere in any transcript"
/// rather than "not recorded". An event id with no row at all is a 404, so the
/// caller can tell the two apart.
#[derive(serde::Serialize)]
pub struct EventLocation {
    pub thread_id: Option<Uuid>,
}

/// GET /api/v1/events/:event_id/location: the thread one event belongs to.
///
/// Exists for the *event wait* step's "show it". An `EventWaitDelivered`
/// carries the matched event's id, type and payload, but not its thread, and
/// the whole point of a wait is that the match usually happened somewhere else.
/// Without this the card could only search the open thread's DOM, which is the
/// one place the event reliably is not.
///
/// Keyed on `event_id` only, the same routing contract as
/// `get_context_capture` and `get_tool_result`.
pub(in crate::api) async fn get_event_location(
    State(state): State<AppState>,
    Path(event_id): Path<String>,
) -> Result<Json<EventLocation>, (StatusCode, String)> {
    let event_uuid = Uuid::parse_str(&event_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid event_id: {}", e)))?;

    let row = state
        .event_store
        .get_event_by_id(event_uuid)
        .await
        .map_err(|e| {
            log!("[API] get_event_location: db error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("Event {} not found", event_uuid),
            )
        })?;

    Ok(Json(EventLocation {
        thread_id: row.thread_id,
    }))
}
