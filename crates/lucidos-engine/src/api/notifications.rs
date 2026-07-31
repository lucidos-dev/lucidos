use super::*;

use crate::scheduler::notifications::Tap;
use crate::scheduler::{NotificationStore, PushSubscriptionStore};

/// Body for `POST /api/v1/notifications` — used by the `lucidos notify` CLI
/// (and any other engine subprocess) to send a push notification without
/// going through an LLM thread / `send_notification` tool. Mirrors the LLM
/// tool's shape: `title` and `message` required, `app_id` / `thread_id` /
/// `tap` optional with the same deep-link semantics.
#[derive(Debug, Deserialize)]
pub(super) struct CreateNotificationRequest {
    pub title: String,
    pub message: String,
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    /// Specific event UUID inside `thread_id` to deep-link to. When set the
    /// inbox modal's "Open thread" button and a `Tap::Navigate { to.target =
    /// Thread, to.event_id = .. }` tap scroll and briefly pulse this event on
    /// land. Ignored without `thread_id`.
    #[serde(default)]
    pub event_id: Option<String>,
    /// Structured tap behavior — see [`Tap`]. Absent / null defaults to
    /// `Tap::Modal`. The serde decode rejects the old string forms
    /// ("modal" / "open_app" / "open_thread" / "none") so out-of-date scripts
    /// fail loudly with a 400 instead of silently routing to the inbox.
    #[serde(default)]
    pub tap: Option<Tap>,
}

/// `POST /api/v1/notifications` — script-facing endpoint that produces the
/// same notification a `send_notification` LLM tool call would: persisted
/// to the inbox AND pushed to subscribed devices, with the same `app_id`
/// deep-link semantics. Shares its body with `execute_send_notification`
/// via `LucidosEngine::create_notification`.
pub(super) async fn create_notification(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateNotificationRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.title.trim().is_empty() {
        return Err(ApiError::bad_request("title is required"));
    }
    if body.message.trim().is_empty() {
        return Err(ApiError::bad_request("message is required"));
    }

    let app_id = body
        .app_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let link_thread_id = super::parse_optional_uuid_trimmed(body.thread_id.as_deref())
        .map_err(|raw| ApiError::bad_request(format!("invalid thread_id: {raw}")))?;

    let link_event_id = super::parse_optional_uuid_trimmed(body.event_id.as_deref())
        .map_err(|raw| ApiError::bad_request(format!("invalid event_id: {raw}")))?;

    // Structured tap — `to.target`-specific required fields (e.g. `app_id`
    // for `target=app`) are enforced by the page-side router, not here.
    // The serde decode above already rejected malformed shapes with 400.
    let tap = body.tap.unwrap_or_default();

    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;

    let id = state
        .engine
        .create_notification(
            &body.title,
            &body.message,
            app_id,
            link_thread_id,
            link_event_id,
            tap,
            actor,
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "notification_id": id.to_string(),
    })))
}

// ===== Notification Endpoints =====

pub(super) async fn get_notifications(
    State(state): State<AppState>,
    Query(query): Query<NotificationsListQuery>,
) -> Result<Json<NotificationsResponse>, (StatusCode, String)> {
    let filter = query.filter.as_deref().unwrap_or("all");
    // Clamp upper bound so a misconfigured client can't ask for an unbounded
    // page; sibling list endpoints (changes, applied changes) cap at 100 too.
    // Lower bound is 0 — a caller wanting only `unread_count` (a count-only
    // probe) passes limit=0, so we honor it explicitly below instead of bumping
    // it to 1 (which would waste one DB row per call the caller would discard).
    let limit = query.limit.clamp(0, 100);

    let before_ts = query.before.map(super::parse_unix_ts);

    // Always fetch the real unread count (uncapped)
    let unread_count = NotificationStore::count_unread(&state.pool)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to count notifications: {}", e),
            )
        })?;

    // Short-circuit limit=0: skip the row fetch and return an empty list with
    // has_more=false. Without this, the old code would `get_filtered(.., 1, ..)`
    // then truncate(0), reporting `has_more=true` for a page the caller cannot
    // reach.
    if limit == 0 {
        return Ok(Json(NotificationsResponse {
            notifications: Vec::new(),
            unread_count,
            has_more: false,
        }));
    }

    // Fetch limit+1 to detect has_more
    let mut items = NotificationStore::get_filtered(&state.pool, filter, limit + 1, before_ts)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load notifications: {}", e),
            )
        })?;

    let has_more = items.len() as i64 > limit;
    if has_more {
        items.truncate(limit as usize);
    }

    Ok(Json(NotificationsResponse {
        notifications: items,
        unread_count,
        has_more,
    }))
}

/// Get notifications at a specific point in time (for time travel)
pub(super) async fn get_notifications_at_timestamp(
    State(state): State<AppState>,
    Query(query): Query<BeforeTimestampQuery>,
) -> Result<Json<NotificationsResponse>, (StatusCode, String)> {
    let timestamp = query.before;
    use chrono::TimeZone;
    let before = chrono::Utc
        .timestamp_opt(timestamp, 0)
        .single()
        .unwrap_or_else(chrono::Utc::now);

    let notifications = NotificationStore::get_all_before(&state.pool, before, 50)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load notifications: {}", e),
            )
        })?;
    let unread_count = notifications.iter().filter(|n| !n.read).count() as i64;

    Ok(Json(NotificationsResponse {
        notifications,
        unread_count,
        has_more: false,
    }))
}

/// Mark a notification as read
pub(super) async fn mark_notification_read(
    State(state): State<AppState>,
    Query(query): Query<NotificationQuery>,
    headers: HeaderMap,
) -> Result<Json<MarkReadResponse>, (StatusCode, String)> {
    let id = query.id;
    let success = NotificationStore::mark_read(&state.pool, id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to mark notification read: {}", e),
            )
        })?;

    if success {
        state
            .engine
            .event_bus
            .emit_user_system(
                &headers,
                &state.pool,
                "[Notifications] NotificationRead",
                |actor| crate::engine::event_bus::SystemEvent::NotificationRead {
                    id: id.to_string(),
                    actor,
                },
            )
            .await;
        // Cross-device dismiss (macOS desktop only): tell any connected Tauri
        // app to REMOVE the already-delivered native banner for this id. The
        // open web can't silently remove a Web Push banner (Safari revokes a
        // subscription after 3 silent pushes), so browser / PWA banners still
        // persist until manually swiped; in-app unread state syncs via the
        // `NotificationRead` SSE above. See notifications.md §4.
        crate::scheduler::push::emit_native_push_dismiss_requested(
            &state.engine.event_bus,
            Some(id),
        )
        .await;
    }

    Ok(Json(MarkReadResponse { success }))
}

/// Mark all notifications as read
pub(super) async fn mark_all_notifications_read(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<MarkReadResponse>, (StatusCode, String)> {
    let count = NotificationStore::mark_all_read(&state.pool)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to mark all notifications read: {}", e),
            )
        })?;

    if count > 0 {
        state
            .engine
            .event_bus
            .emit_user_system(
                &headers,
                &state.pool,
                "[Notifications] NotificationsAllRead",
                |actor| crate::engine::event_bus::SystemEvent::NotificationsAllRead { actor },
            )
            .await;
        // Cross-device dismiss (macOS desktop only): `None` = remove ALL
        // delivered native banners on connected Tauri apps. Web / PWA banners
        // persist (no silent Web Push removal). See notifications.md §4.
        crate::scheduler::push::emit_native_push_dismiss_requested(&state.engine.event_bus, None)
            .await;
    }

    Ok(Json(MarkReadResponse { success: count > 0 }))
}

/// Get a single notification by ID
pub(super) async fn get_notification(
    State(state): State<AppState>,
    Query(query): Query<NotificationQuery>,
) -> Result<Json<Option<Notification>>, (StatusCode, String)> {
    let id = query.id;
    let notification = NotificationStore::get_by_id(&state.pool, id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get notification: {}", e),
            )
        })?;

    Ok(Json(notification))
}

// ===== Push Notification Endpoints =====

/// GET /api/v1/push/vapid-key — return the VAPID public key for browser subscription
pub(super) async fn get_vapid_key(
    State(state): State<AppState>,
) -> Result<Json<VapidKeyResponse>, (StatusCode, String)> {
    let keys = crate::scheduler::push::get_or_create_vapid_keys(&state.pool)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get VAPID keys: {}", e),
            )
        })?;

    Ok(Json(VapidKeyResponse {
        public_key: keys.public_key,
    }))
}

/// POST /api/v1/push/subscribe — store a browser push subscription
pub(super) async fn push_subscribe(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PushSubscribeRequest>,
) -> Json<ApiResult> {
    let scope_url = sanitize_push_scope_url(request.scope_url, &headers);
    let sub = PushSubscription {
        endpoint: request.endpoint,
        p256dh: request.p256dh,
        auth: request.auth,
        device_id: request.device_id,
        scope_url,
    };
    match PushSubscriptionStore::subscribe(&state.pool, &sub).await {
        Ok(()) => ApiResult::ok(),
        Err(e) => ApiResult::err(format!("Failed to store subscription: {}", e)),
    }
}

fn sanitize_push_scope_url(raw: Option<String>, headers: &HeaderMap) -> Option<String> {
    let mut url = reqwest::Url::parse(raw?.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    url.set_query(None);
    url.set_fragment(None);
    if !url.path().ends_with('/') {
        let mut path = url.path().to_string();
        path.push('/');
        url.set_path(&path);
    }

    let expected_path = crate::api::base_path::forwarded_prefix(headers);
    if url.path() != expected_path {
        return None;
    }

    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        let origin_url = reqwest::Url::parse(origin).ok()?;
        if url.scheme() != origin_url.scheme()
            || url.host_str() != origin_url.host_str()
            || url.port_or_known_default() != origin_url.port_or_known_default()
        {
            return None;
        }
    }

    Some(url.to_string())
}

/// POST /api/v1/push/unsubscribe — remove a browser push subscription
pub(super) async fn push_unsubscribe(
    State(state): State<AppState>,
    Json(request): Json<PushUnsubscribeRequest>,
) -> Json<ApiResult> {
    match PushSubscriptionStore::unsubscribe(&state.pool, &request.endpoint).await {
        Ok(_) => ApiResult::ok(),
        Err(e) => ApiResult::err(format!("Failed to remove subscription: {}", e)),
    }
}

// ===== Test-only push log =====
//
// Backs the `expectPushSent` / `expectNoPushSent` helpers used by Playwright
// e2e tests. The handler, the query/row types, and the route registration
// in api/mod.rs are all gated on the `e2e-test-hooks` cargo feature; the
// production binary never compiles them in. See
// system-knowhow/notifications.md §5.4.

#[cfg(feature = "e2e-test-hooks")]
#[derive(serde::Deserialize)]
pub(super) struct PushLogQuery {
    #[serde(default)]
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub notification_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub device_id: Option<String>,
}

#[cfg(feature = "e2e-test-hooks")]
#[derive(serde::Serialize, sqlx::FromRow)]
pub(super) struct PushLogEntry {
    pub device_id: String,
    pub notification_id: uuid::Uuid,
    pub sent_at: chrono::DateTime<chrono::Utc>,
    /// The push payload bytes the engine would have encrypted + sent. Let
    /// e2e tests assert on the Declarative Web Push envelope shape rather
    /// than just delivery. Nullable for rows that pre-date the migration —
    /// always populated by the current write path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
}

/// GET /api/v1/_test/push-log — return rows the push-transport stub appended
/// (see crates/lucidos-engine/src/scheduler/push_test_log.rs). Optional
/// `since`, `notification_id`, `device_id` filter the result; the response
/// is sorted ascending by `sent_at` for deterministic test assertions.
#[cfg(feature = "e2e-test-hooks")]
pub(super) async fn get_push_log(
    State(state): State<AppState>,
    Query(q): Query<PushLogQuery>,
) -> Result<Json<Vec<PushLogEntry>>, (StatusCode, String)> {
    // Build the query with positional binds. sqlx::query_as won't accept
    // optional WHERE clauses dynamically, so use a fixed SELECT with
    // COALESCE-style "match all when filter absent" expressions.
    let rows: Vec<PushLogEntry> = sqlx::query_as::<_, PushLogEntry>(
        "SELECT device_id, notification_id, sent_at, payload FROM push_log \
         WHERE ($1::timestamptz IS NULL OR sent_at >= $1) \
           AND ($2::uuid IS NULL OR notification_id = $2) \
           AND ($3::text IS NULL OR device_id = $3) \
         ORDER BY sent_at ASC",
    )
    .bind(q.since)
    .bind(q.notification_id)
    .bind(q.device_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to query push_log: {}", e),
        )
    })?;

    Ok(Json(rows))
}

/// Routes for the `/notifications*`, `/notification*`, and `/push/*`
/// surfaces (push subscriptions are part of the notification fan-out and
/// their handlers live here).
pub(super) fn router() -> Router<AppState> {
    let router = Router::new()
        .route("/notifications", get(get_notifications))
        .route("/notifications", post(create_notification))
        .route("/notifications/before", get(get_notifications_at_timestamp))
        .route("/notifications/read-all", post(mark_all_notifications_read))
        .route("/notification", get(get_notification))
        .route("/notification/read", post(mark_notification_read))
        // Push notification endpoints
        .route("/push/vapid-key", get(get_vapid_key))
        .route("/push/subscribe", post(push_subscribe))
        .route("/push/unsubscribe", post(push_unsubscribe));

    // Test-only push assertion endpoint. Compiled in only with the
    // `e2e-test-hooks` cargo feature so production binaries don't expose
    // the push_log to the network at all.
    #[cfg(feature = "e2e-test-hooks")]
    let router = router.route("/_test/push-log", get(get_push_log));

    router
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(prefix: &str, origin: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-prefix", HeaderValue::from_str(prefix).unwrap());
        if let Some(origin) = origin {
            h.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
        }
        h
    }

    #[test]
    fn sanitize_push_scope_url_accepts_matching_origin_and_prefix() {
        let scope = sanitize_push_scope_url(
            Some("https://lucidos.test/dev/?stale=1#frag".into()),
            &headers("/dev/", Some("https://lucidos.test")),
        );
        assert_eq!(scope.as_deref(), Some("https://lucidos.test/dev/"));
    }

    #[test]
    fn sanitize_push_scope_url_rejects_cross_origin_value() {
        let scope = sanitize_push_scope_url(
            Some("https://evil.test/dev/".into()),
            &headers("/dev/", Some("https://lucidos.test")),
        );
        assert!(scope.is_none());
    }

    #[test]
    fn sanitize_push_scope_url_rejects_wrong_workspace_prefix() {
        let scope = sanitize_push_scope_url(
            Some("https://lucidos.test/myws/".into()),
            &headers("/dev/", Some("https://lucidos.test")),
        );
        assert!(scope.is_none());
    }
}
