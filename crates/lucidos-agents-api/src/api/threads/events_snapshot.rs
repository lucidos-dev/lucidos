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
    #[serde(default)]
    pub include_context: bool,
    pub limit: Option<i64>,
    pub before_created: Option<chrono::DateTime<chrono::Utc>>,
    pub before_seq: Option<i64>,
}

const MAX_EVENTS_PAGE: i64 = 2_000;

#[derive(serde::Serialize)]
pub struct ThreadEventsSnapshot {
    pub events: Vec<ThreadEventRow>,
    #[serde(rename = "currentAggregate")]
    pub current_aggregate: Option<crate::core::store::ThreadAggregate>,
    #[serde(
        rename = "hasMore",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub has_more: bool,
    /// INVARIANT: The highest `sequence` the whole thread holds, sent only for a PAGE.
    ///
    /// A page cannot supply it. A sequence is allocated globally and can run
    /// against the clock, so the newest row by clock often does not hold the
    /// highest one. Half the threads on this workspace are like that.
    /// Deriving the forward watermark from the page would make the next delta
    /// refetch history and replay it through the live path.
    #[serde(rename = "maxSequence", skip_serializing_if = "Option::is_none")]
    pub max_sequence: Option<i64>,
    /// INVARIANT: The thread's pair stamps, sent only for a PAGE. The event that stamped
    /// the model, effort or provider may sit behind the page. The chat rung
    /// pill reads the pair from these, not the account default. An unpaged
    /// read carries every event, so it sends none.
    #[serde(rename = "pairStamps", skip_serializing_if = "Option::is_none")]
    pub pair_stamps: Option<Vec<crate::core::PairStamp>>,
    /// INVARIANT: The thread's card sizes, sent only for a PAGE, for the same reason: the
    /// pick that sized a widget instance's cards may sit behind the page.
    #[serde(rename = "cardSizes", skip_serializing_if = "Option::is_none")]
    pub card_sizes: Option<Vec<crate::engine::widgets::CardSize>>,
}

type BeforeCursor = (chrono::DateTime<chrono::Utc>, i64);

pub(super) fn before_cursor(
    created: Option<chrono::DateTime<chrono::Utc>>,
    seq: Option<i64>,
) -> Result<Option<BeforeCursor>, (StatusCode, String)> {
    match (created, seq) {
        (Some(created), Some(seq)) => Ok(Some((created, seq))),
        (None, None) => Ok(None),
        _ => Err((
            StatusCode::BAD_REQUEST,
            "before_created and before_seq must be given together".to_string(),
        )),
    }
}

pub(in crate::api) async fn get_thread_events_snapshot(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    Query(query): Query<ThreadEventsQuery>,
) -> Result<Json<ThreadEventsSnapshot>, (StatusCode, String)> {
    let thread_uuid = Uuid::parse_str(&thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {}", e)))?;

    if query.limit.is_some() && query.after.is_some() {
        return Err((
            StatusCode::BAD_REQUEST,
            "after and limit are mutually exclusive".to_string(),
        ));
    }
    let before = before_cursor(query.before_created, query.before_seq)?;
    if before.is_some() && query.limit.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "before_created requires limit".to_string(),
        ));
    }

    let pool = state.engine.pool();
    let (events_res, aggregate_res, pair_stamps_res, card_sizes_res) = tokio::join!(
        read_events(&state, thread_uuid, &query, before),
        crate::core::store::fetch_thread_aggregate(pool, thread_uuid),
        async {
            match query.limit {
                Some(_) => crate::core::PreferenceStore::thread_pair_stamps(pool, thread_uuid)
                    .await
                    .map(Some),
                None => Ok(None),
            }
        },
        async {
            match query.limit {
                Some(_) => crate::engine::widgets::thread_card_sizes(pool, thread_uuid)
                    .await
                    .map(Some),
                None => Ok(None),
            }
        },
    );

    let EventsRead {
        mut events,
        has_more,
        max_sequence,
    } = events_res.map_err(|e| {
        log_legacy!("[API] Failed to get thread events: {}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;
    let pair_stamps = pair_stamps_res.map_err(|e| {
        log_legacy!(
            "[API] Failed to read pair stamps for {}: {}",
            thread_uuid,
            e
        );
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;
    let card_sizes = card_sizes_res.map_err(|e| {
        log_legacy!("[API] Failed to read card sizes for {}: {}", thread_uuid, e);
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    for row in &mut events {
        strip_inline_image_payloads(row);
        if !query.include_context {
            strip_context_capture_sections(row);
            strip_tool_result_content(row);
            strip_tool_call_args(row);
        }
        rename_legacy_section_size_in_row(row);
    }

    let current_aggregate = aggregate_res.unwrap_or_else(|e| {
        log_legacy!(
            "[API] Failed to fetch ThreadAggregate for {}: {}",
            thread_uuid,
            e
        );
        None
    });

    Ok(Json(ThreadEventsSnapshot {
        events,
        current_aggregate,
        has_more,
        max_sequence,
        pair_stamps,
        card_sizes,
    }))
}

pub(super) fn rename_legacy_section_size_in_row(row: &mut ThreadEventRow) {
    if matches!(
        row.event_type.as_str(),
        "ContextCaptured" | "ContextAssembled"
    ) {
        rename_legacy_section_size_in_payload(&mut row.payload);
    }
}

struct EventsRead {
    events: Vec<ThreadEventRow>,
    has_more: bool,
    max_sequence: Option<i64>,
}

async fn read_events(
    state: &AppState,
    thread_uuid: Uuid,
    query: &ThreadEventsQuery,
    before: Option<(chrono::DateTime<chrono::Utc>, i64)>,
) -> Result<EventsRead, sqlx::Error> {
    let Some(limit) = query.limit else {
        let events = state
            .event_store
            .get_thread_events_by_seq(thread_uuid, query.after)
            .await?;
        return Ok(EventsRead {
            events,
            has_more: false,
            max_sequence: None,
        });
    };
    let limit = limit.clamp(1, MAX_EVENTS_PAGE);
    let page = state
        .event_store
        .get_thread_events_page(thread_uuid, before, limit)
        .await?;
    Ok(EventsRead {
        events: page.events,
        has_more: page.has_more,
        max_sequence: page.max_sequence,
    })
}

pub(super) fn is_tool_result_event(event_type: &str) -> bool {
    matches!(event_type, "ToolResult" | "CodingAgentToolResult")
}

pub(super) fn is_tool_call_event(event_type: &str) -> bool {
    matches!(event_type, "CodingAgentToolCalled" | "ToolCalled")
}

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

pub(super) fn strip_image_content_in_tool_result<E: HasEventPayload>(row: &mut E) {
    rewrite_tool_result(row, crate::engine::tools::files::strip_image_content_marker);
}

pub(super) fn strip_app_capture_in_tool_result<E: HasEventPayload>(row: &mut E) {
    rewrite_tool_result(row, crate::engine::agentic_loop::strip_app_capture_marker);
}

pub(super) fn strip_inline_image_payloads<E: HasEventPayload>(row: &mut E) {
    strip_image_content_in_tool_result(row);
    strip_app_capture_in_tool_result(row);
}

const LEGACY_CODING_AGENT_PREVIEW_CHARS: usize = 200;

pub(super) const LEGACY_PREVIEW_NOTE: &str = "\n\n[Older records kept only the first 200 \
     characters of a coding agent's output. This one may be cut short.]";

/// INVARIANT: Label an old coding-agent result on the way out, never in the store.
pub(super) fn label_legacy_coding_agent_preview<E: HasEventPayload>(row: &mut E) {
    if row.event_type() != "CodingAgentToolResult" {
        return;
    }
    rewrite_tool_result(row, |result| {
        (result.chars().count() == LEGACY_CODING_AGENT_PREVIEW_CHARS)
            .then(|| format!("{result}{LEGACY_PREVIEW_NOTE}"))
    });
}

#[derive(serde::Serialize)]
pub struct ContextCapturePayload {
    pub sections: serde_json::Value,
    pub tools: serde_json::Value,
}

/// INVARIANT: GET /api/v1/events/:event_id/context — returns the `sections` and `tools`
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
            log_legacy!("[API] get_context_capture: db error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Event not found".to_string()))?;

    if row.event_type != "ContextCaptured" {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Event {} is not a ContextCaptured event", event_uuid),
        ));
    }

    context_capture_from_payload(&row.payload, event_uuid).map(Json)
}

pub(super) fn context_capture_from_payload(
    payload: &serde_json::Value,
    event_uuid: Uuid,
) -> Result<ContextCapturePayload, (StatusCode, String)> {
    let payload_obj = payload.as_object().ok_or_else(|| {
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

    Ok(ContextCapturePayload { sections, tools })
}

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

pub(super) fn rename_legacy_section_size_in_payload(payload: &mut serde_json::Value) {
    let Some(sections) = payload.get_mut("sections") else {
        return;
    };
    rename_legacy_section_size(sections);
}

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

pub(super) fn strip_tool_result_content(row: &mut ThreadEventRow) {
    if !is_tool_result_event(&row.event_type) {
        return;
    }
    let Some(obj) = row.payload.as_object_mut() else {
        return;
    };
    crate::engine::thread_events::ThreadEvent::strip_result_text(obj);
}

pub(super) fn strip_tool_call_args(row: &mut ThreadEventRow) {
    if !is_tool_call_event(&row.event_type) {
        return;
    }
    if let Some(obj) = row.payload.as_object_mut() {
        crate::engine::thread_events::ThreadEvent::strip_tool_call_args(&row.event_type, obj);
    }
}

#[derive(serde::Serialize)]
pub struct ToolResultPayload {
    pub result: serde_json::Value,
}

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
            log_legacy!("[API] get_tool_result: db error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Event not found".to_string()))?;

    if !is_tool_result_event(&row.event_type) {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Event {} is not a tool result event", event_uuid),
        ));
    }

    strip_inline_image_payloads(&mut row);
    label_legacy_coding_agent_preview(&mut row);

    let payload_obj = row.payload.as_object().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Payload not an object".to_string(),
        )
    })?;

    let result = payload_obj
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok(Json(ToolResultPayload { result }))
}

#[derive(serde::Serialize)]
pub struct ToolArgsPayload {
    pub args: serde_json::Value,
}

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
            log_legacy!("[API] get_tool_args: db error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Event not found".to_string()))?;

    if !is_tool_call_event(&row.event_type) {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Event {} is not a tool call", event_uuid),
        ));
    }

    let payload_obj = row.payload.as_object().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Payload not an object".to_string(),
        )
    })?;

    let args = payload_obj
        .get("args")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok(Json(ToolArgsPayload { args }))
}

#[derive(serde::Serialize)]
pub struct EventLocation {
    pub thread_id: Option<Uuid>,
}

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
            log_legacy!("[API] get_event_location: db error: {}", e);
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
