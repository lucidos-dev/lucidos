use super::*;
use axum::body::Body;
use tokio::io::AsyncWriteExt;

/// SSE keep-alive cadence — sent as an SSE comment line so the connection
/// doesn't appear idle to intermediaries / EventSource. Matches the prior
/// `KeepAlive::new().interval(...)` value so observable behavior is unchanged.
const SSE_KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Inter-task pipe size between the gzip encoder writer and the response body
/// reader. 64 KiB is large enough that a burst of small events doesn't have to
/// round-trip through the writer for every single frame, but small enough to
/// bound memory if a client stalls.
const GZIP_PIPE_BUF_BYTES: usize = 64 * 1024;

pub(super) async fn global_events(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    // Register this connection in the live SSE-connection count. The guard is
    // moved into the stream's map closure below so it's dropped — and the count
    // decremented — exactly when the stream is dropped (client disconnects).
    // The push fan-out reads this count to decide whether to run the
    // PresenceCheck (system-knowhow/notifications.md §3): a connected page can
    // pong even when its device_presence heartbeat has gone stale.
    let conn_guard = state.engine.sse_connections.connect();
    let rx = state.engine.event_bus.subscribe();
    let json_stream = BroadcastStream::new(rx).map(move |r| {
        // `move` captures `conn_guard` by value; it lives as long as this
        // closure (i.e. as long as the stream), then Drop decrements the count.
        let _hold = &conn_guard;
        match r {
            Ok(emitted) => emitted.to_sse_json(),
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                log!("[SSE] Event stream lagged by {} events", n);
                lagged_event_json(n)
            }
        }
    });

    if accepts_gzip(&headers) {
        gzipped_sse_response(json_stream)
    } else {
        plain_sse_response(json_stream)
    }
}

/// Plain (uncompressed) SSE response — same shape as the pre-gzip handler.
/// Used when the client doesn't advertise `gzip` in `Accept-Encoding`.
fn plain_sse_response<S>(json_stream: S) -> Response
where
    S: Stream<Item = String> + Send + 'static,
{
    let event_stream = json_stream.map(|json| Ok::<_, Infallible>(Event::default().data(json)));
    Sse::new(event_stream)
        .keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE_INTERVAL))
        .into_response()
}

/// Gzip-compressed SSE response. Uses a streaming `GzipEncoder` with explicit
/// per-event `flush()` (Z_SYNC_FLUSH) so each event reaches the client as
/// soon as it's emitted — not buffered until the encoder fills its window.
///
/// Wire format is identical to plain SSE (`data: <json>\n\n` per event,
/// `:keepalive\n\n` for keep-alives) — only the transport bytes are
/// compressed, so EventSource parsers in browsers / curl `--compressed`
/// see the same event stream.
fn gzipped_sse_response<S>(json_stream: S) -> Response
where
    S: Stream<Item = String> + Send + 'static,
{
    use async_compression::tokio::write::GzipEncoder;
    use async_compression::Level;
    use tokio_util::io::ReaderStream;

    let (writer, reader) = tokio::io::duplex(GZIP_PIPE_BUF_BYTES);
    let body_stream = ReaderStream::new(reader);

    tokio::spawn(async move {
        // Avoid per-event allocations: write the SSE wire framing in fixed
        // byte slices around the borrowed JSON payload. The encoder buffers
        // internally until flush(), so the three writes coalesce into one
        // Z_SYNC_FLUSH block on the wire.
        const DATA_PREFIX: &[u8] = b"data: ";
        const FRAME_SUFFIX: &[u8] = b"\n\n";
        const KEEPALIVE_FRAME: &[u8] = b":keepalive\n\n";

        enum Frame {
            Data(String),
            Keepalive,
        }

        let mut encoder = GzipEncoder::with_quality(writer, Level::Fastest);
        let mut keepalive = tokio::time::interval(SSE_KEEPALIVE_INTERVAL);
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // First tick fires immediately — skip it so we don't send a keep-alive
        // before the first real event has had a chance to arrive.
        keepalive.tick().await;
        tokio::pin!(json_stream);
        loop {
            // Decide what to write inside the select (no encoder borrows here),
            // then do the IO sequentially below so each write's `&mut encoder`
            // borrow is released before the next.
            let frame = tokio::select! {
                item = json_stream.next() => match item {
                    Some(json) => Frame::Data(json),
                    None => break,
                },
                _ = keepalive.tick() => Frame::Keepalive,
            };
            // Sequential `?` short-circuits on the first IO error (client
            // gone), avoiding extra writes into a half-broken encoder.
            let write_result: std::io::Result<()> = async {
                match &frame {
                    Frame::Data(json) => {
                        encoder.write_all(DATA_PREFIX).await?;
                        encoder.write_all(json.as_bytes()).await?;
                        encoder.write_all(FRAME_SUFFIX).await?;
                    }
                    Frame::Keepalive => {
                        encoder.write_all(KEEPALIVE_FRAME).await?;
                    }
                }
                // flush() emits a Z_SYNC_FLUSH block — bytes leave the
                // encoder immediately so latency stays close to plain SSE.
                encoder.flush().await
            }
            .await;
            if write_result.is_err() {
                break;
            }
        }
        // Best-effort gzip trailer; client may have already disconnected.
        let _ = encoder.shutdown().await;
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CONTENT_ENCODING, "gzip")
        .header(header::CACHE_CONTROL, "no-cache")
        // Hint to reverse proxies (e.g. nginx) not to buffer the body.
        .header("X-Accel-Buffering", "no")
        .body(Body::from_stream(body_stream))
        .expect("static SSE response builder is infallible")
}

/// Returns true iff `Accept-Encoding` advertises gzip with a non-zero
/// quality. Honors RFC 7231 §5.3.5: a `q=0` weight on `gzip` is an
/// explicit opt-out (e.g. for clients with broken decompressors), so
/// callers must skip compression even though the token is present.
/// Strict on the token name — `x-gzip` (a distinct historical encoding)
/// is not accepted.
fn accepts_gzip(headers: &HeaderMap) -> bool {
    let Some(val) = headers.get(header::ACCEPT_ENCODING) else {
        return false;
    };
    let Ok(s) = val.to_str() else {
        return false;
    };
    s.split(',').any(|piece| {
        let mut params = piece.split(';');
        let token = params.next().unwrap_or("").trim();
        if !token.eq_ignore_ascii_case("gzip") {
            return false;
        }
        // Default weight is 1.0 when no q parameter is present.
        params.all(|param| {
            let Some((name, value)) = param.split_once('=') else {
                return true;
            };
            if !name.trim().eq_ignore_ascii_case("q") {
                return true;
            }
            // Treat unparseable / explicitly-zero weights as opt-out.
            // (Per RFC, valid q is in [0, 1] with up to 3 decimals; any
            // strictly positive value keeps gzip acceptable.)
            value
                .trim()
                .parse::<f32>()
                .map(|q| q > 0.0)
                .unwrap_or(false)
        })
    })
}

/// Wire frame the frontend sees when its broadcast subscriber falls behind the
/// 4096-event buffer. Without this, lagged events vanish silently and the UI
/// keeps a "Thinking" spinner forever waiting for a `ResponseGenerated` that
/// already happened. The frontend treats this as a signal to resync loaded
/// thread state from `/api/v1/threads/<id>/events`.
fn lagged_event_json(count: u64) -> String {
    serde_json::json!({
        "type": "Lagged",
        "data": { "count": count },
    })
    .to_string()
}

pub(super) async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let workspace_name = state
        .workspace_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let engine_version = include_str!("../../VERSION").trim();
    // Read fresh from disk on each request so version bumps from applied
    // changes are picked up without an engine restart.
    let latest_engine_version = read_engine_version();
    let latest_tauri_app_version = read_app_version();
    Json(serde_json::json!({
        "status": "ok",
        "workspace": workspace_name,
        "workspace_path": state.workspace_path.to_string_lossy(),
        "started_at": state.started_at.to_rfc3339(),
        "release": crate::LUCIDOS_RELEASE,
        "release_dirty": crate::LUCIDOS_RELEASE_DIRTY,
        "engine_version": engine_version,
        "latest_engine_version": latest_engine_version,
        "latest_tauri_app_version": latest_tauri_app_version,
    }))
}

/// Read a VERSION file from disk, returning "unknown" if missing.
fn read_version_file(path: &std::path::Path) -> String {
    std::fs::read_to_string(path)
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Read the engine VERSION from disk (picks up bumps without restart).
fn read_engine_version() -> String {
    let path = crate::paths::repo_root()
        .ok()
        .map(|r| r.join("crates/lucidos-engine/VERSION"))
        .unwrap_or_default();
    read_version_file(&path)
}

/// Read the Tauri app VERSION from disk.
fn read_app_version() -> String {
    let path = crate::paths::repo_root()
        .ok()
        .map(|r| r.join("crates/lucidos-app/VERSION"))
        .unwrap_or_default();
    read_version_file(&path)
}

/// Spawn `web-dev.sh --engine-only` to rebuild and restart the engine binary,
/// leaving Vite and parent scripts untouched. Errors return `{"error": msg}`
/// so the UI can show the actual reason instead of a silent failure.
///
/// Before spawning the restart script: enumerates all in-flight chat + CC
/// threads and pre-emits boundary events with `actor: <user>` so the post-
/// restart timeline reads "You restarted" instead of "⚙ System restarted".
/// Recovery on the new engine sees these threads are already terminated and
/// skips them.
pub(super) async fn restart_engine(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    state.engine.abort_in_flight_for_restart(actor).await;

    let script = crate::paths::script("web-dev.sh").map_err(|e| {
        log!("[Restart] {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
    })?;
    let ws = state.workspace_path.to_string_lossy().to_string();
    let log_path = state.workspace_path.join(".lucidos/engine.log");
    log!(
        "[Restart] Running {} --engine-only -w {}",
        script.display(),
        ws
    );
    let mut cmd = tokio::process::Command::new(&script);
    cmd.args(["-w", &ws, "--engine-only"]);
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(log_file) => {
            let stderr_file = log_file.try_clone().or_else(|_| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_path)
            });
            cmd.stdout(log_file);
            match stderr_file {
                Ok(f) => {
                    cmd.stderr(f);
                }
                Err(_) => {
                    cmd.stderr(std::process::Stdio::null());
                }
            }
        }
        Err(e) => {
            log!(
                "[Restart] Failed to open log file for restart: {} — spawning with no output",
                e
            );
            cmd.stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
        }
    }
    match cmd.spawn() {
        Ok(_) => Ok(StatusCode::OK),
        Err(e) => {
            let msg = format!("Failed to spawn {}: {}", script.display(), e);
            log!("[Restart] {}", msg);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": msg })),
            ))
        }
    }
}

/// List other running Lucidos workspaces by calling status.sh --json.
/// Excludes the current workspace from results. Times out after 10s to
/// avoid blocking if Docker or target engines are unresponsive.
pub(super) async fn list_workspaces(State(state): State<AppState>) -> Json<serde_json::Value> {
    let empty = || Json(serde_json::json!({ "workspaces": [] }));
    let script = match crate::paths::script("status.sh") {
        Ok(p) => p,
        Err(e) => {
            log!("[Workspaces] {}", e);
            return empty();
        }
    };
    let ws = state.workspace_path.to_string_lossy().to_string();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::process::Command::new(&script)
            .args(["--json", "-w", &ws])
            .output(),
    )
    .await;
    match result {
        Ok(Ok(output)) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            match serde_json::from_str::<serde_json::Value>(&stdout) {
                Ok(val) => Json(val),
                Err(e) => {
                    log!("[Workspaces] Failed to parse status.sh JSON: {}", e);
                    empty()
                }
            }
        }
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log!("[Workspaces] status.sh failed: {}", stderr);
            empty()
        }
        Ok(Err(e)) => {
            log!("[Workspaces] Failed to run status.sh: {}", e);
            empty()
        }
        Err(_) => {
            log!("[Workspaces] status.sh timed out");
            empty()
        }
    }
}

/// Get conversation history up to a specific event
pub(super) async fn get_history(
    State(state): State<AppState>,
    Query(query): Query<EventQuery>,
) -> Result<Json<ConversationSnapshot>, (StatusCode, String)> {
    let event_id = query.event;
    match state
        .event_store
        .get_conversation_at_event(event_id, &state.workspace_path)
        .await
    {
        Ok(snapshot) => Ok(Json(snapshot)),
        Err(e) => Err((StatusCode::NOT_FOUND, format!("Error: {}", e))),
    }
}

#[derive(Deserialize)]
pub(super) struct MessagesQuery {
    #[serde(default = "default_messages_limit")]
    limit: i64,
    #[serde(default)]
    before: Option<String>,
}

fn default_messages_limit() -> i64 {
    20
}

/// Parse an optional RFC3339 timestamp string into `DateTime<Utc>`.
/// Used by query endpoints that accept `since`/`until`/`before` cursors.
fn parse_optional_rfc3339(s: Option<&str>) -> Option<chrono::DateTime<chrono::Utc>> {
    s.and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

/// Get recent messages across all history (flat timeline)
pub(super) async fn get_recent_messages(
    State(state): State<AppState>,
    Query(query): Query<MessagesQuery>,
) -> Result<Json<Vec<SessionMessage>>, (StatusCode, String)> {
    let before = parse_optional_rfc3339(query.before.as_deref());
    let limit = query.limit.clamp(1, 500);
    let messages = state
        .event_store
        .get_recent_messages(limit, before)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load messages: {}", e),
            )
        })?;
    Ok(Json(messages))
}

/// Get all messages for a specific session (for history time travel)
pub(super) async fn get_session_messages(
    State(state): State<AppState>,
    Query(query): Query<SessionMessagesQuery>,
) -> Result<Json<Vec<SessionMessage>>, (StatusCode, String)> {
    let request_id = query.id;
    let messages = state
        .event_store
        .get_request_messages_by_id(&request_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load messages: {}", e),
            )
        })?;
    Ok(Json(messages))
}

#[derive(Deserialize)]
pub(super) struct EventsQueryParams {
    #[serde(default, alias = "type")]
    event_type: Option<String>,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    until: Option<String>,
    #[serde(default)]
    before_event_id: Option<uuid::Uuid>,
    #[serde(default)]
    after_event_id: Option<uuid::Uuid>,
    #[serde(default = "default_events_limit")]
    limit: i64,
}

fn default_events_limit() -> i64 {
    100
}

/// Reject paging requests that pin both ends of the cursor at once: there's
/// no coherent semantics for "strictly older than X AND strictly newer than
/// Y" in a paging API. Returned as a 400 by the HTTP handler.
fn validate_cursor_pair(
    before_event_id: Option<uuid::Uuid>,
    after_event_id: Option<uuid::Uuid>,
) -> Result<(), String> {
    if before_event_id.is_some() && after_event_id.is_some() {
        return Err("before_event_id and after_event_id are mutually exclusive".into());
    }
    Ok(())
}

#[derive(Deserialize)]
pub(super) struct EventsCountParams {
    #[serde(default, alias = "type")]
    event_type: Option<String>,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    until: Option<String>,
}

/// REST endpoint that mirrors the `count_events` LLM tool: per-type counts +
/// byte totals over the given time window. With `event_type`, returns
/// `{count, byte_total}` for that type; without, returns
/// `{by_type:[...], total_count, total_byte_total}` sorted by count desc.
///
/// Use this to size a sweep before drilling with `/events/query` — same
/// design rationale as the LLM tool (see `crate::engine::tools::execute_count_events`).
pub(super) async fn count_events(
    State(state): State<AppState>,
    Query(q): Query<EventsCountParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let since = parse_optional_rfc3339(q.since.as_deref());
    let until = parse_optional_rfc3339(q.until.as_deref());
    if let Some(et) = q.event_type.as_deref() {
        let (count, byte_total) = state
            .event_store
            .count_events(Some(et), since, until)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to count events: {}", e),
                )
            })?;
        Ok(Json(serde_json::json!({
            "count": count,
            "byte_total": byte_total,
        })))
    } else {
        let rows = state
            .event_store
            .count_events_by_type(since, until)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to count events: {}", e),
                )
            })?;
        let total_count: i64 = rows.iter().map(|(_, c, _)| *c).sum();
        let total_byte_total: i64 = rows.iter().map(|(_, _, b)| *b).sum();
        let by_type: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|(et, count, byte_total)| {
                serde_json::json!({
                    "event_type": et,
                    "count": count,
                    "byte_total": byte_total,
                })
            })
            .collect();
        Ok(Json(serde_json::json!({
            "by_type": by_type,
            "total_count": total_count,
            "total_byte_total": total_byte_total,
        })))
    }
}

/// REST endpoint to query stored events by type/time (not SSE)
pub(super) async fn query_events(
    State(state): State<AppState>,
    Query(q): Query<EventsQueryParams>,
) -> Result<Json<Vec<crate::core::EventRow>>, (StatusCode, String)> {
    let since = parse_optional_rfc3339(q.since.as_deref());
    let until = parse_optional_rfc3339(q.until.as_deref());
    let limit = q.limit.clamp(1, 1000);
    if let Err(msg) = validate_cursor_pair(q.before_event_id, q.after_event_id) {
        return Err((StatusCode::BAD_REQUEST, msg));
    }
    let result = state
        .event_store
        .query_events_paged(
            q.event_type.as_deref(),
            since,
            until,
            q.before_event_id,
            q.after_event_id,
            limit,
        )
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to query events: {}", e),
            )
        })?;
    match result {
        crate::core::store::QueryEventsResult::Events(events) => Ok(Json(events)),
        crate::core::store::QueryEventsResult::CursorNotFound => Err((
            StatusCode::NOT_FOUND,
            "cursor event_id not found".to_string(),
        )),
    }
}

/// Well-known persisted event types — always available even in empty workspaces.
const KNOWN_EVENT_TYPES: &[&str] = &[
    "ChangeApplied",
    "ChangeApplyFailed",
    "ChangeDiscarded",
    "ChangeProposed",
    "ChangeReverted",
    "CodingAgentIdled",
    "CodingAgentUserMessageSent",
    "MergeConflictDetected",
    "MessageReceived",
    "NotificationCreated",
    "ResponseAborted",
    "ResponseCanceled",
    "ResponseFailed",
    "ResponseGenerated",
    "TriggerCompleted",
    "TriggerStarted",
    "SessionEnded",
    "SessionStarted",
    "ThreadTitleGenerated",
];

/// Return known event types merged with any additional types from the database.
pub(super) async fn event_types(
    State(state): State<AppState>,
) -> Result<Json<Vec<String>>, (StatusCode, String)> {
    let db_types = state
        .event_store
        .distinct_event_types()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to query event types: {}", e),
            )
        })?;
    let mut all: Vec<String> = KNOWN_EVENT_TYPES.iter().map(|s| s.to_string()).collect();
    for t in db_types {
        if !all.contains(&t) {
            all.push(t);
        }
    }
    all.sort();
    Ok(Json(all))
}

/// Validate an event type submitted to `POST /api/v1/events/emit`.
///
/// Domain events sent over HTTP come from app UIs (untrusted). After
/// `to_sse_json()` unwraps `DomainEvent` to `{"type": <event_type>, ...}`,
/// the wire shape of a domain event is identical to a system frame. Without
/// this guard, an app could call `emit_event("NotificationCreated", {...})`
/// and forge a notification on every connected SSE client.
fn validate_emittable_event_type(event_type: &str) -> Result<(), String> {
    if event_type.is_empty() {
        return Err("event_type is required".into());
    }
    if crate::engine::event_bus::SystemEvent::is_reserved_type_name(event_type) {
        return Err(format!(
            "event_type '{}' is reserved for system events and cannot be emitted via this API",
            event_type
        ));
    }
    Ok(())
}

pub(super) async fn emit_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<EmitEventRequest>,
) -> Response {
    if let Err(msg) = validate_emittable_event_type(&body.event_type) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response();
    }

    // App UI emits are user-driven on a real device — stamp the resolved actor
    // so persisted rows and SSE frames carry the same `actor` key as every
    // other actor-bearing event. The merge is done inside
    // `SystemEvent::DomainEvent::to_payload` so non-object payloads are
    // preserved unchanged (no panic, just a no-op merge).
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;

    if body.transient {
        match state
            .engine
            .broadcast_transient_domain_event(&body.event_type, body.payload, actor)
            .await
        {
            Ok(()) => Json(serde_json::json!({ "success": true })).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to emit event: {}", e) })),
            )
                .into_response(),
        }
    } else {
        match state
            .engine
            .emit_domain_event(&body.event_type, body.payload, actor)
            .await
        {
            Ok(id) => Json(serde_json::json!({
                "success": true,
                "event_id": id.to_string(),
            }))
            .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to emit event: {}", e) })),
            )
                .into_response(),
        }
    }
}

/// Routes for the engine-level surfaces this module's handlers own:
/// `/health`, `/restart`, `/workspaces`, `/history`, `/messages`,
/// `/session/messages`, and the `/events*` surface (global SSE stream +
/// event-store queries). The two `/events/:event_id/*` routes are part of
/// the events URL surface, so they register here even though their handlers
/// live in `api::threads`.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/restart", post(restart_engine))
        .route("/workspaces", get(list_workspaces))
        .route("/events", get(global_events))
        .route("/history", get(get_history))
        .route("/messages", get(get_recent_messages))
        .route("/session/messages", get(get_session_messages))
        .route("/events/query", get(query_events))
        .route("/events/count", get(count_events))
        .route("/events/types", get(event_types))
        .route("/events/emit", post(emit_event))
        .route(
            "/events/:event_id/context",
            get(super::threads::get_context_capture),
        )
        .route(
            "/events/:event_id/tool-result",
            get(super::threads::get_tool_result),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: read_app_version must read fresh from disk on every call,
    /// not return a cached value. The original bug cached the version in
    /// AppState at startup, so version bumps from applied changes were
    /// invisible to the health endpoint until an engine restart.
    ///
    /// Single test to avoid races — both scenarios mutate the same file.
    #[test]
    fn read_app_version_reads_fresh_from_disk() {
        let engine_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let version_file = engine_dir
            .parent()
            .unwrap()
            .join("lucidos-app")
            .join("VERSION");
        let original = std::fs::read_to_string(&version_file).ok();

        // Drop guard ensures cleanup even if an assertion panics.
        struct Restore(std::path::PathBuf, Option<String>);
        impl Drop for Restore {
            fn drop(&mut self) {
                match &self.1 {
                    Some(content) => {
                        let _ = std::fs::write(&self.0, content);
                    }
                    None => {
                        let _ = std::fs::remove_file(&self.0);
                    }
                }
            }
        }
        let _guard = Restore(version_file.clone(), original);

        // Picks up initial write.
        std::fs::write(&version_file, "1.0.0-test\n").unwrap();
        assert_eq!(read_app_version(), "1.0.0-test");

        // Picks up version bump without restart.
        std::fs::write(&version_file, "2.0.0-test\n").unwrap();
        assert_eq!(read_app_version(), "2.0.0-test");

        // Returns "unknown" when the file is missing.
        std::fs::remove_file(&version_file).unwrap();
        assert_eq!(read_app_version(), "unknown");
    }

    #[test]
    fn validate_emittable_event_type_rejects_empty() {
        assert!(validate_emittable_event_type("").is_err());
    }

    /// `Accept-Encoding` parser must recognise gzip in any of its standard
    /// shapes — bare token, with a q-value, anywhere in a comma-separated list,
    /// case-insensitive — and reject look-alikes like `x-gzip` (which is a
    /// distinct historical encoding).
    #[test]
    fn accepts_gzip_recognises_standard_offers() {
        fn h(value: &str) -> axum::http::HeaderMap {
            let mut m = axum::http::HeaderMap::new();
            m.insert(
                axum::http::header::ACCEPT_ENCODING,
                axum::http::HeaderValue::from_str(value).unwrap(),
            );
            m
        }
        assert!(accepts_gzip(&h("gzip")));
        assert!(accepts_gzip(&h("GZIP")));
        assert!(accepts_gzip(&h("gzip;q=1.0")));
        assert!(accepts_gzip(&h("deflate, gzip")));
        assert!(accepts_gzip(&h("br, gzip;q=0.9, deflate;q=0.5")));
        assert!(!accepts_gzip(&h("deflate")));
        assert!(!accepts_gzip(&h("br")));
        assert!(!accepts_gzip(&h("x-gzip")));
        assert!(!accepts_gzip(&h("")));
        assert!(!accepts_gzip(&axum::http::HeaderMap::new()));

        // RFC 7231 §5.3.5: a quality of 0 means "not acceptable" — clients
        // use this to explicitly opt OUT of an encoding (e.g. broken
        // intermediaries, decompression bugs). Compressing the response
        // anyway forces gzip on a client that asked us not to.
        assert!(!accepts_gzip(&h("gzip;q=0")));
        assert!(!accepts_gzip(&h("gzip; q=0")));
        assert!(!accepts_gzip(&h("gzip;q=0.0")));
        assert!(!accepts_gzip(&h("gzip;q=0.000")));
        assert!(!accepts_gzip(&h("deflate, gzip;q=0")));
        // Other encodings at q=0 don't change gzip's acceptability.
        assert!(accepts_gzip(&h("gzip, deflate;q=0")));
    }

    /// Spoofing prevention: untrusted apps must not be able to emit a
    /// domain event whose name collides with a `SystemEvent` variant —
    /// after the SSE unwrap, the wire frame would be indistinguishable
    /// from a real system frame (e.g. a forged `NotificationCreated`
    /// would render a fake notification on every connected client).
    #[test]
    fn validate_emittable_event_type_rejects_reserved_system_names() {
        for name in [
            "NotificationCreated",
            "NotificationRead",
            "PreferencesChanged",
            "AppDeleted",
            "Toast",
            "DomainEvent",
            "ChangesUpdated",
            "TriggerCreated",
            "ThreadEvent",
        ] {
            assert!(
                validate_emittable_event_type(name).is_err(),
                "{name} should be rejected as reserved",
            );
        }
    }

    #[test]
    fn validate_emittable_event_type_accepts_domain_names() {
        for name in [
            "SlidePresenterState",
            "SlideRemoteCommand",
            "HabitCompleted",
            "MyCustomEvent",
        ] {
            assert!(
                validate_emittable_event_type(name).is_ok(),
                "{name} should be allowed as a domain event",
            );
        }
    }

    #[test]
    fn validate_cursor_pair_allows_zero_or_one_cursor() {
        assert!(validate_cursor_pair(None, None).is_ok());
        assert!(validate_cursor_pair(Some(uuid::Uuid::new_v4()), None).is_ok());
        assert!(validate_cursor_pair(None, Some(uuid::Uuid::new_v4())).is_ok());
    }

    #[test]
    fn validate_cursor_pair_rejects_both_cursors_set() {
        let err = validate_cursor_pair(Some(uuid::Uuid::new_v4()), Some(uuid::Uuid::new_v4()))
            .unwrap_err();
        assert!(
            err.contains("mutually exclusive"),
            "error should mention mutual exclusion: {err}"
        );
    }

    /// The frontend's SSE handler keys off `type` and reads `data.count`.
    /// If the wire shape drifts, lagged tabs lose their resync signal and
    /// stuck "Thinking" spinners come back.
    #[test]
    fn lagged_event_json_has_stable_wire_shape() {
        let parsed: serde_json::Value = serde_json::from_str(&lagged_event_json(42)).unwrap();
        assert_eq!(parsed["type"], "Lagged");
        assert_eq!(parsed["data"]["count"], 42);
    }

    /// read_engine_version reads the engine VERSION from disk on each call,
    /// allowing the health endpoint to detect newer engine versions on disk
    /// without an engine restart.
    #[test]
    fn read_engine_version_reads_fresh_from_disk() {
        let engine_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let version_file = engine_dir.join("VERSION");
        let original = std::fs::read_to_string(&version_file).ok();

        struct Restore(std::path::PathBuf, Option<String>);
        impl Drop for Restore {
            fn drop(&mut self) {
                match &self.1 {
                    Some(content) => {
                        let _ = std::fs::write(&self.0, content);
                    }
                    None => {
                        let _ = std::fs::remove_file(&self.0);
                    }
                }
            }
        }
        let _guard = Restore(version_file.clone(), original);

        std::fs::write(&version_file, "2026.04.13.1\n").unwrap();
        assert_eq!(read_engine_version(), "2026.04.13.1");

        std::fs::write(&version_file, "2026.04.13.2\n").unwrap();
        assert_eq!(read_engine_version(), "2026.04.13.2");

        std::fs::remove_file(&version_file).unwrap();
        assert_eq!(read_engine_version(), "unknown");
    }
}
