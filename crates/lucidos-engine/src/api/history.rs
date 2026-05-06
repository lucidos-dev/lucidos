use super::*;

pub(super) async fn global_events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.engine.event_bus.subscribe();
    let stream = BroadcastStream::new(rx)
        .filter_map(|r| match r {
            Ok(emitted) => Some(emitted.to_sse_json()),
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                log!("[SSE] Event stream lagged by {} events", n);
                None
            }
        })
        .map(|json| Ok(Event::default().data(json)));

    Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(30)))
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
pub(super) async fn restart_engine(
    State(state): State<AppState>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
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
                "Failed to open log file for restart: {} — spawning with no output",
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
    #[serde(default = "default_events_limit")]
    limit: i64,
}

fn default_events_limit() -> i64 {
    100
}

/// REST endpoint to query stored events by type/time (not SSE)
pub(super) async fn query_events(
    State(state): State<AppState>,
    Query(q): Query<EventsQueryParams>,
) -> Result<Json<Vec<crate::core::EventRow>>, (StatusCode, String)> {
    let since = parse_optional_rfc3339(q.since.as_deref());
    let until = parse_optional_rfc3339(q.until.as_deref());
    let limit = q.limit.clamp(1, 1000);
    let events = state
        .event_store
        .query_events(q.event_type.as_deref(), since, until, limit)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to query events: {}", e),
            )
        })?;
    Ok(Json(events))
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

/// Validate an event type submitted to `POST /api/events/emit`.
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
    Json(body): Json<EmitEventRequest>,
) -> Response {
    if let Err(msg) = validate_emittable_event_type(&body.event_type) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response();
    }

    if body.transient {
        match state
            .engine
            .broadcast_transient_domain_event(&body.event_type, body.payload)
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
            .emit_domain_event(&body.event_type, body.payload)
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
