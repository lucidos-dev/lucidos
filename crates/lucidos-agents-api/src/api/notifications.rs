use super::*;

use crate::gateway_users::Reader;
use crate::scheduler::notifications::{
    default_tap, refuse_engine_only_tap, resolve_thread_tap_id, verify_event_anchors, Tap,
};
use crate::scheduler::{NotificationStore, PushSubscriptionStore};

#[derive(Debug, Deserialize)]
pub(super) struct CreateNotificationRequest {
    pub title: String,
    pub message: String,
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub event_id: Option<String>,
    #[serde(default)]
    pub tap: Option<Tap>,
}

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

    let mut tap = body
        .tap
        .unwrap_or_else(|| default_tap(link_thread_id, link_event_id));
    refuse_engine_only_tap(&tap).map_err(|e| ApiError::bad_request(format!("invalid tap: {e}")))?;

    resolve_thread_tap_id(&mut tap, None)
        .map_err(|e| ApiError::bad_request(format!("invalid tap: {e}")))?;

    verify_event_anchors(&state.pool, link_thread_id, link_event_id, &tap)
        .await
        .map_err(|e| ApiError::internal(format!("could not check event_id: {e}")))?
        .map_err(ApiError::bad_request)?;

    let actor = super::actor::user_actor(&headers, None);

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

#[derive(Deserialize)]
struct NativeReceiptRequest {
    notification_id: Uuid,
}

async fn native_push_receipt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<NativeReceiptRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Some(crate::engine::thread_events::MessageOrigin::Device { device_id }) =
        crate::api::actor::user_actor(&headers, None)
    else {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "A device must acknowledge its banner",
        ));
    };
    let audience = NotificationStore::audience(state.engine.pool(), body.notification_id)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    if !crate::engine::thread_users::Audience::from_device_ids(audience).reaches(Some(&device_id)) {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "Notification not found",
        ));
    }
    let durable: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM user_only_message_dispatches WHERE notification_id = $1)",
    )
    .bind(body.notification_id)
    .fetch_one(state.engine.pool())
    .await
    .map_err(ApiError::db)?;
    if durable {
        crate::scheduler::push::record_push_receipt(
            state.engine.pool(),
            body.notification_id,
            &format!("native:{device_id}"),
        )
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    }
    Ok(Json(serde_json::json!({"ok": true})))
}

fn reader_of(headers: &HeaderMap) -> Reader {
    Reader::of(
        crate::api::actor::user_actor(headers, None).as_ref(),
        &crate::gateway_users::current(),
    )
}

pub(super) async fn get_notifications(
    State(state): State<AppState>,
    Query(query): Query<NotificationsListQuery>,
    headers: HeaderMap,
) -> Result<Json<NotificationsResponse>, (StatusCode, String)> {
    read_notifications(&state, &headers, &query).await.map(Json)
}

pub(super) const NOTIFICATIONS_PAGE_MAX: i64 = 100;

pub(super) async fn read_notifications(
    state: &AppState,
    headers: &HeaderMap,
    query: &NotificationsListQuery,
) -> Result<NotificationsResponse, (StatusCode, String)> {
    let reader = reader_of(headers);
    let filter = query.filter.as_deref().unwrap_or("all");
    let limit = query.limit.clamp(0, NOTIFICATIONS_PAGE_MAX);

    let before_ts = query.before.map(super::parse_unix_ts);

    let unread_count = NotificationStore::count_unread(&state.pool, &reader)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to count notifications: {}", e),
            )
        })?;

    if limit == 0 {
        return Ok(NotificationsResponse {
            notifications: Vec::new(),
            unread_count,
            has_more: false,
        });
    }

    let mut items =
        NotificationStore::get_filtered(&state.pool, &reader, filter, limit + 1, before_ts)
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

    Ok(NotificationsResponse {
        notifications: items,
        unread_count,
        has_more,
    })
}

pub(super) async fn get_notifications_at_timestamp(
    State(state): State<AppState>,
    Query(query): Query<BeforeTimestampQuery>,
    headers: HeaderMap,
) -> Result<Json<NotificationsResponse>, (StatusCode, String)> {
    let timestamp = query.before;
    use chrono::TimeZone;
    let before = chrono::Utc
        .timestamp_opt(timestamp, 0)
        .single()
        .unwrap_or_else(chrono::Utc::now);

    let notifications =
        NotificationStore::get_all_before(&state.pool, &reader_of(&headers), before, 50)
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

pub(super) async fn mark_notification_read(
    State(state): State<AppState>,
    Query(query): Query<NotificationQuery>,
    headers: HeaderMap,
) -> Result<Json<MarkReadResponse>, (StatusCode, String)> {
    let id = query.id;
    let actor = crate::api::actor::user_actor(&headers, None);
    let reader = reader_of(&headers);
    let success =
        NotificationStore::mark_read(&state.pool, &state.engine.event_bus, &reader, id, actor)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to mark notification read: {}", e),
                )
            })?;

    if success {
        crate::scheduler::push::emit_native_push_dismiss_requested(
            &state.engine.event_bus,
            Some(id),
            &reader,
        )
        .await;
    }

    Ok(Json(MarkReadResponse { success }))
}

pub(super) async fn mark_all_notifications_read(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<MarkReadResponse>, (StatusCode, String)> {
    let actor = crate::api::actor::user_actor(&headers, None);
    let reader = reader_of(&headers);
    let count =
        NotificationStore::mark_all_read(&state.pool, &state.engine.event_bus, &reader, actor)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to mark all notifications read: {}", e),
                )
            })?;

    if count > 0 {
        crate::scheduler::push::emit_native_push_dismiss_requested(
            &state.engine.event_bus,
            None,
            &reader,
        )
        .await;
    }

    Ok(Json(MarkReadResponse { success: count > 0 }))
}

pub(super) async fn get_notification(
    State(state): State<AppState>,
    Query(query): Query<NotificationQuery>,
    headers: HeaderMap,
) -> Result<Json<Option<Notification>>, (StatusCode, String)> {
    let id = query.id;
    let notification = NotificationStore::get_by_id(&state.pool, &reader_of(&headers), id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get notification: {}", e),
            )
        })?;

    Ok(Json(notification))
}

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

pub(super) async fn push_subscribe(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PushSubscribeRequest>,
) -> Json<ApiResult> {
    if let Some(reason) = subscribe_refusal(&headers, &request) {
        return ApiResult::err(reason);
    }
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

fn subscribe_refusal(headers: &HeaderMap, request: &PushSubscribeRequest) -> Option<String> {
    if !push_endpoint_is_deliverable(&request.endpoint) {
        return Some(
            "A push endpoint must be an https URL on a public host. \
             This one is not, so nothing was stored."
                .to_string(),
        );
    }
    match request.device_id.as_deref() {
        Some(device_id) => super::settings::claims_another_device(headers, device_id)
            .map(|reason| format!("{reason}, so no push subscription was stored.")),
        None => super::settings::claims_another_device(headers, "").map(|_| {
            "A paired device must name itself in device_id, \
             so no push subscription was stored."
                .to_string()
        }),
    }
}

fn push_endpoint_is_deliverable(raw: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(raw.trim()) else {
        return false;
    };
    if url.scheme() != "https" {
        return false;
    }
    let Some(domain) = url.domain() else {
        return false;
    };
    let host = domain.trim_end_matches('.').to_ascii_lowercase();
    host.contains('.')
        && host != "localhost"
        && !host.ends_with(".localhost")
        && !host.ends_with(".local")
        && !host.ends_with(".internal")
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

    let expected_path = crate::core::base_path::forwarded_prefix(headers);
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

pub(super) async fn push_unsubscribe(
    State(state): State<AppState>,
    Json(request): Json<PushUnsubscribeRequest>,
) -> Json<ApiResult> {
    match PushSubscriptionStore::unsubscribe(&state.pool, &request.endpoint).await {
        Ok(_) => ApiResult::ok(),
        Err(e) => ApiResult::err(format!("Failed to remove subscription: {}", e)),
    }
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
}

#[cfg(feature = "e2e-test-hooks")]
pub(super) async fn get_push_log(
    State(state): State<AppState>,
    Query(q): Query<PushLogQuery>,
) -> Result<Json<Vec<PushLogEntry>>, (StatusCode, String)> {
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

pub(super) fn router() -> Router<AppState> {
    let router = Router::new()
        .route("/notifications", get(get_notifications))
        .route("/notifications", post(create_notification))
        .route("/notifications/before", get(get_notifications_at_timestamp))
        .route("/notifications/read-all", post(mark_all_notifications_read))
        .route("/notification", get(get_notification))
        .route("/notification/read", post(mark_notification_read))
        .route("/notification/native-receipt", post(native_push_receipt))
        .route("/push/vapid-key", get(get_vapid_key))
        .route("/push/subscribe", post(push_subscribe))
        .route("/push/unsubscribe", post(push_unsubscribe));

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

    fn subscribe_request(device_id: Option<&str>) -> PushSubscribeRequest {
        PushSubscribeRequest {
            endpoint: "https://fcm.googleapis.com/fcm/send/abc123".to_string(),
            p256dh: "key".to_string(),
            auth: "auth".to_string(),
            device_id: device_id.map(str::to_string),
            scope_url: None,
        }
    }

    fn device_header(device_id: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            crate::core::subprocess_origin::HEADER_DEVICE_ID,
            HeaderValue::from_str(device_id).unwrap(),
        );
        h
    }

    #[test]
    fn a_paired_device_cannot_subscribe_for_another_device() {
        let reason = subscribe_refusal(&device_header("dev-a"), &subscribe_request(Some("dev-b")))
            .expect("dev-a must not replace dev-b's push subscription");
        assert!(reason.contains("dev-a"), "{reason}");
        assert!(reason.contains("dev-b"), "{reason}");
    }

    #[test]
    fn a_paired_device_cannot_subscribe_for_no_device() {
        assert!(
            subscribe_refusal(&device_header("dev-a"), &subscribe_request(None)).is_some(),
            "a device-less row would reach every notification"
        );
        assert_eq!(
            subscribe_refusal(&HeaderMap::new(), &subscribe_request(None)),
            None
        );
    }

    #[test]
    fn a_device_may_subscribe_for_itself() {
        assert_eq!(
            subscribe_refusal(&device_header("dev-a"), &subscribe_request(Some("dev-a"))),
            None
        );
    }

    #[test]
    fn a_loopback_caller_with_no_device_header_may_subscribe() {
        assert_eq!(
            subscribe_refusal(&HeaderMap::new(), &subscribe_request(Some("dev-a"))),
            None
        );
    }

    #[test]
    fn push_endpoint_is_deliverable_accepts_the_real_push_services() {
        for endpoint in [
            "https://fcm.googleapis.com/fcm/send/abc123",
            "https://web.push.apple.com/QAbc-123",
            "https://updates.push.services.mozilla.com/wpush/v2/gAA",
        ] {
            assert!(push_endpoint_is_deliverable(endpoint), "{endpoint}");
        }
    }

    #[test]
    fn push_endpoint_is_deliverable_refuses_a_redirected_or_internal_target() {
        for endpoint in [
            "http://fcm.googleapis.com/fcm/send/abc",
            "https://127.0.0.1:8080/p",
            "https://169.254.169.254/latest/meta-data/",
            "https://[::1]/p",
            "https://localhost/p",
            "https://engine.local/p",
            "https://metadata.internal/p",
            "https://localhost./p",
            "https://engine.local./p",
            "https://metadata.internal./p",
            "https://intranet/p",
            "file:///etc/passwd",
            "not a url",
            "",
        ] {
            assert!(!push_endpoint_is_deliverable(endpoint), "{endpoint}");
        }
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
    fn sanitize_push_scope_url_keeps_a_relayed_outside_prefix() {
        let scope = sanitize_push_scope_url(
            Some("https://gateway-a.test/dev@cloud/".into()),
            &headers("/dev@cloud/", None),
        );
        assert_eq!(scope.as_deref(), Some("https://gateway-a.test/dev@cloud/"));
    }

    #[test]
    fn sanitize_push_scope_url_rejects_wrong_workspace_prefix() {
        let scope = sanitize_push_scope_url(
            Some("https://lucidos.test/myws/".into()),
            &headers("/dev/", Some("https://lucidos.test")),
        );
        assert!(scope.is_none());
    }

    #[tokio::test]
    async fn a_script_tap_naming_the_current_thread_is_a_400_from_the_handler() {
        use crate::api::permission_requests::tests::permission_test_state;
        use crate::scheduler::notifications::{NavigateTarget, NavigateUi};
        use crate::test_support::{
            boot_mock_engine, in_engine_child, setup_test_db, teardown_test_db,
        };
        if !in_engine_child(
            "api::notifications::tests::a_script_tap_naming_the_current_thread_is_a_400_from_the_handler",
        )
        .await
        {
            return;
        }
        let (_pool, db) = setup_test_db().await;
        let (engine, workspace) = boot_mock_engine(&db).await;
        let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
        let request = CreateNotificationRequest {
            title: "Done".to_string(),
            message: "The run finished".to_string(),
            app_id: None,
            thread_id: None,
            event_id: None,
            tap: Some(Tap::Navigate {
                to: Box::new(NavigateUi {
                    target: NavigateTarget::Thread,
                    id: Some("current".to_string()),
                    ..Default::default()
                }),
            }),
        };
        let refusal = create_notification(State(state), HeaderMap::new(), Json(request))
            .await
            .expect_err("a script has no ambient thread, so the alias is refused");
        assert_eq!(refusal.status, StatusCode::BAD_REQUEST);
        teardown_test_db(&db).await;
    }

    mod guard_tests {
        fn production_src() -> String {
            crate::test_support::source_scan::read_production_source(
                &crate::test_support::source_scan::engine_source("api/notifications.rs"),
            )
        }

        #[test]
        fn push_subscribe_refuses_before_it_reaches_the_store() {
            let src = production_src();
            let at = src
                .find("pub(super) async fn push_subscribe")
                .expect("push_subscribe is still here");
            let body = &src[at..];
            let guard = body
                .find("subscribe_refusal(&headers, &request)")
                .expect("push_subscribe must check the request it is handed");
            let store = body
                .find("PushSubscriptionStore::subscribe")
                .expect("push_subscribe still writes through the store");
            assert!(
                guard < store,
                "the refusal must precede the delete-then-insert"
            );
        }
    }
}
