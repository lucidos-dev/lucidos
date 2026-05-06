use super::*;

use crate::scheduler::{NotificationStore, PushSubscriptionStore};

// ===== Notification Endpoints =====

pub(super) async fn get_notifications(
    State(state): State<AppState>,
    Query(query): Query<NotificationsListQuery>,
) -> Result<Json<NotificationsResponse>, (StatusCode, String)> {
    let filter = query.filter.as_deref().unwrap_or("all");
    // Clamp upper bound so a misconfigured client can't ask for an unbounded
    // page; sibling list endpoints (changes, applied changes) cap at 100 too.
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
        let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
        state
            .engine
            .event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::System(
                    crate::engine::event_bus::SystemEvent::NotificationRead {
                        id: id.to_string(),
                        actor,
                    },
                ),
                "[Notifications] NotificationRead",
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
        let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
        state
            .engine
            .event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::System(
                    crate::engine::event_bus::SystemEvent::NotificationsAllRead { actor },
                ),
                "[Notifications] NotificationsAllRead",
            )
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

/// GET /api/push/vapid-key — return the VAPID public key for browser subscription
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

/// POST /api/push/subscribe — store a browser push subscription
pub(super) async fn push_subscribe(
    State(state): State<AppState>,
    Json(request): Json<PushSubscribeRequest>,
) -> Json<ApiResult> {
    let sub = PushSubscription {
        endpoint: request.endpoint,
        p256dh: request.p256dh,
        auth: request.auth,
        device_id: request.device_id,
    };
    match PushSubscriptionStore::subscribe(&state.pool, &sub).await {
        Ok(()) => ApiResult::ok(),
        Err(e) => ApiResult::err(format!("Failed to store subscription: {}", e)),
    }
}

/// POST /api/push/unsubscribe — remove a browser push subscription
pub(super) async fn push_unsubscribe(
    State(state): State<AppState>,
    Json(request): Json<PushUnsubscribeRequest>,
) -> Json<ApiResult> {
    match PushSubscriptionStore::unsubscribe(&state.pool, &request.endpoint).await {
        Ok(_) => ApiResult::ok(),
        Err(e) => ApiResult::err(format!("Failed to remove subscription: {}", e)),
    }
}

/// POST /api/notification-clicked — SW notificationclick stores the tapped notification ID.
/// Bypasses client-side IDB/Cache/postMessage issues on iOS Safari by using
/// a simple server round-trip that both SW and page can access via fetch().
pub(super) async fn notification_clicked(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> StatusCode {
    if let Some(id) = body.get("notification_id").and_then(|v| v.as_str()) {
        *state.pending_notification_click.lock().unwrap() = Some(id.to_string());
    }
    StatusCode::NO_CONTENT
}

/// GET /api/notification-clicked — returns and clears the pending notification ID.
pub(super) async fn get_notification_clicked(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let id = state.pending_notification_click.lock().unwrap().take();
    Json(serde_json::json!({ "notification_id": id }))
}

/// POST /api/notification-pushed — SW push event stores the notification ID.
/// This is a fallback for iOS where notificationclick fires too late (after
/// the page's visibilitychange check) or doesn't fire at all on warm resume.
pub(super) async fn notification_pushed(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> StatusCode {
    if let Some(id) = body.get("notification_id").and_then(|v| v.as_str()) {
        *state.pending_notification_push.lock().unwrap() =
            Some((id.to_string(), std::time::Instant::now()));
    }
    StatusCode::NO_CONTENT
}

/// GET /api/notification-pushed — returns and clears the pending push notification ID,
/// but only if it was stored within the last 60 seconds (avoids showing stale pushes
/// when the user opens the app independently).
pub(super) async fn get_notification_pushed(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let maybe = state.pending_notification_push.lock().unwrap().take();
    let id = maybe.and_then(|(id, at)| {
        if at.elapsed() < std::time::Duration::from_secs(60) {
            Some(id)
        } else {
            None
        }
    });
    Json(serde_json::json!({ "notification_id": id }))
}

/// Clear the pending push if it matches the dismissed notification id.
fn clear_pending_push_if_matches(
    pending: &std::sync::Arc<std::sync::Mutex<Option<(String, std::time::Instant)>>>,
    dismissed_id: &str,
) {
    let mut guard = pending.lock().unwrap();
    if guard.as_ref().is_some_and(|(stored, _)| stored == dismissed_id) {
        *guard = None;
    }
}

/// POST /api/notification-dismissed — SW notificationclose clears the pending push
/// when the user dismisses the OS notification (close button or notification-center
/// swipe). Without this, the push fallback (/api/notification-pushed, 60s window)
/// fires the next time the app gains focus, auto-opening the modal for a
/// notification the user explicitly dismissed.
pub(super) async fn notification_dismissed(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> StatusCode {
    if let Some(id) = body.get("notification_id").and_then(|v| v.as_str()) {
        clear_pending_push_if_matches(&state.pending_notification_push, id);
    }
    StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    fn pending_with(id: &str) -> Arc<Mutex<Option<(String, Instant)>>> {
        Arc::new(Mutex::new(Some((id.to_string(), Instant::now()))))
    }

    #[test]
    fn dismiss_clears_pending_when_id_matches() {
        let pending = pending_with("notif-abc");
        clear_pending_push_if_matches(&pending, "notif-abc");
        assert!(pending.lock().unwrap().is_none());
    }

    #[test]
    fn dismiss_keeps_pending_when_id_differs() {
        // Push for notif-A is stored, but the user dismisses notif-B (e.g. an
        // earlier push that was overwritten before the user touched it). The
        // newer pending entry must survive.
        let pending = pending_with("notif-A");
        clear_pending_push_if_matches(&pending, "notif-B");
        let guard = pending.lock().unwrap();
        let (kept, _) = guard.as_ref().expect("pending should still be set");
        assert_eq!(kept, "notif-A");
    }

    #[test]
    fn dismiss_is_noop_when_no_pending() {
        let pending: Arc<Mutex<Option<(String, Instant)>>> = Arc::new(Mutex::new(None));
        clear_pending_push_if_matches(&pending, "notif-abc");
        assert!(pending.lock().unwrap().is_none());
    }
}
