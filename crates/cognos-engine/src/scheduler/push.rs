//! Web Push notification support for CognOS
//!
//! Manages VAPID keys, push subscriptions, and sending notifications
//! to all registered browser endpoints.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

/// A browser push subscription (endpoint + encryption keys)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushSubscription {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub device_id: Option<String>,
}

/// VAPID key pair for Web Push authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VapidKeys {
    /// PEM-encoded EC private key
    pub private_key_pem: String,
    /// Base64url-encoded public key (for the browser)
    pub public_key: String,
}

/// Manages push subscriptions in PostgreSQL
pub struct PushSubscriptionStore;

impl PushSubscriptionStore {
    /// Create the push_subscriptions table if it doesn't exist
    pub async fn init_schema(
        pool: &PgPool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS push_subscriptions (
                endpoint TEXT PRIMARY KEY,
                p256dh TEXT NOT NULL,
                auth TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )",
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Store a push subscription (upsert by endpoint)
    pub async fn subscribe(
        pool: &PgPool,
        sub: &PushSubscription,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // If device_id is provided, replace any existing subscription for that device
        // (browser generates a new endpoint on re-subscribe, so the old one is stale).
        // Fall back to endpoint-based upsert for subscriptions without a device_id.
        if let Some(device_id) = &sub.device_id {
            sqlx::query("DELETE FROM push_subscriptions WHERE device_id = $1")
                .bind(device_id)
                .execute(pool)
                .await?;
        }
        sqlx::query(
            "INSERT INTO push_subscriptions (endpoint, p256dh, auth, device_id)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (endpoint) DO UPDATE SET p256dh = $2, auth = $3, device_id = $4",
        )
        .bind(&sub.endpoint)
        .bind(&sub.p256dh)
        .bind(&sub.auth)
        .bind(&sub.device_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Remove a push subscription by endpoint
    pub async fn unsubscribe(
        pool: &PgPool,
        endpoint: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let result = sqlx::query("DELETE FROM push_subscriptions WHERE endpoint = $1")
            .bind(endpoint)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Get all active push subscriptions
    pub async fn get_all(
        pool: &PgPool,
    ) -> Result<Vec<PushSubscription>, Box<dyn std::error::Error + Send + Sync>> {
        let rows = sqlx::query_as::<_, (String, String, String, Option<String>)>(
            "SELECT endpoint, p256dh, auth, device_id FROM push_subscriptions",
        )
        .fetch_all(pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(endpoint, p256dh, auth, device_id)| PushSubscription {
                endpoint,
                p256dh,
                auth,
                device_id,
            })
            .collect())
    }

    /// Get push subscriptions filtered by device push_enabled setting
    pub async fn get_push_enabled(
        pool: &PgPool,
    ) -> Result<Vec<PushSubscription>, Box<dyn std::error::Error + Send + Sync>> {
        let rows = sqlx::query_as::<_, (String, String, String, Option<String>)>(
            "SELECT ps.endpoint, ps.p256dh, ps.auth, ps.device_id
             FROM push_subscriptions ps
             LEFT JOIN devices d ON ps.device_id = d.id
             WHERE ps.device_id IS NULL OR d.push_enabled = true",
        )
        .fetch_all(pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(endpoint, p256dh, auth, device_id)| PushSubscription {
                endpoint,
                p256dh,
                auth,
                device_id,
            })
            .collect())
    }
}

/// Get or create VAPID keys, stored in the preferences table
pub async fn get_or_create_vapid_keys(
    pool: &PgPool,
) -> Result<VapidKeys, Box<dyn std::error::Error + Send + Sync>> {
    use crate::core::PreferenceStore;
    use base64::Engine;
    use p256::ecdsa::SigningKey;
    use p256::elliptic_curve::rand_core::OsRng;
    use p256::pkcs8::EncodePrivateKey;

    // Check if keys already exist
    if let Some(keys_json) = PreferenceStore::get(pool, "vapid_keys").await? {
        let keys: VapidKeys = serde_json::from_str(&keys_json)?;
        return Ok(keys);
    }

    // Generate new EC P-256 key pair
    let signing_key = SigningKey::random(&mut OsRng);

    // PEM-encode the private key (PKCS#8)
    let private_key_pem = signing_key
        .to_pkcs8_pem(p256::pkcs8::LineEnding::LF)
        .map_err(|e| format!("Failed to encode private key: {}", e))?
        .to_string();

    // Extract the uncompressed public key bytes and base64url-encode
    let verifying_key = signing_key.verifying_key();
    let pub_bytes = verifying_key.to_encoded_point(false);
    let public_key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(pub_bytes.as_bytes());

    let keys = VapidKeys {
        private_key_pem,
        public_key,
    };

    // Store in preferences
    let keys_json = serde_json::to_string(&keys)?;
    PreferenceStore::set(pool, "vapid_keys", &keys_json).await?;

    log!("Generated new VAPID key pair");
    Ok(keys)
}

/// Send a push notification to all registered subscriptions.
/// Non-fatal: logs errors but doesn't fail.
/// If `notification_id` is provided, clicking the notification deep-links to it.
/// If `app_id` is provided, clicking the notification can open the app directly.
pub async fn send_push_to_all(
    pool: &PgPool,
    title: &str,
    body: &str,
    notification_id: Option<uuid::Uuid>,
) {
    send_push_to_all_with_app(pool, title, body, notification_id, None, None).await;
}

/// Like `send_push_to_all` but includes an `app_id` in the push payload for deep linking.
///
/// `link_thread_id` serves two roles when set:
/// - **Suppress**: devices currently focused on that thread (according to the
///   `thread_presence` projection) are excluded — the user is already looking
///   at the relevant conversation.
/// - **Deep link**: the value is included in the push payload as `thread_id`
///   so the service worker can navigate the recipient straight to the thread
///   when they tap the notification.
pub async fn send_push_to_all_with_app(
    pool: &PgPool,
    title: &str,
    body: &str,
    notification_id: Option<uuid::Uuid>,
    app_id: Option<&str>,
    link_thread_id: Option<uuid::Uuid>,
) {
    // Run the subscription fetch and (if applicable) presence query in
    // parallel — they hit independent tables and we always need both before
    // we can send.
    let (subs_result, focused_result) = match link_thread_id {
        Some(thread_id) => {
            let (subs, focused) = tokio::join!(
                PushSubscriptionStore::get_push_enabled(pool),
                crate::core::ThreadPresenceStore::devices_focused_on(pool, thread_id),
            );
            (subs, Some(focused))
        }
        None => (PushSubscriptionStore::get_push_enabled(pool).await, None),
    };

    let subscriptions = match subs_result {
        Ok(subs) => subs,
        Err(e) => {
            log!("Failed to fetch subscriptions: {}", e);
            return;
        }
    };

    if subscriptions.is_empty() {
        return;
    }

    // Filter out subscriptions whose device is currently viewing the thread.
    let subscriptions = match (link_thread_id, focused_result) {
        (Some(thread_id), Some(Ok(focused))) => {
            let before = subscriptions.len();
            let filtered = filter_subscriptions_by_presence(subscriptions, &focused);
            if filtered.len() != before {
                log!(
                    "Suppressed push to {} device(s) viewing thread {}",
                    before - filtered.len(),
                    thread_id
                );
            }
            filtered
        }
        (_, Some(Err(e))) => {
            log!("Failed to query thread_presence (sending to all): {}", e);
            subscriptions
        }
        _ => subscriptions,
    };

    if subscriptions.is_empty() {
        return;
    }

    let keys = match get_or_create_vapid_keys(pool).await {
        Ok(k) => k,
        Err(e) => {
            log!("Failed to get VAPID keys: {}", e);
            return;
        }
    };

    let payload_bytes =
        build_push_payload(title, body, notification_id, app_id, link_thread_id).to_string();

    let client = match web_push::IsahcWebPushClient::new() {
        Ok(c) => c,
        Err(e) => {
            log!("Failed to create push client: {}", e);
            return;
        }
    };

    let mut stale_endpoints = Vec::new();

    for sub in &subscriptions {
        let sub_info = web_push::SubscriptionInfo::new(&sub.endpoint, &sub.p256dh, &sub.auth);

        // Build VAPID signature from PEM
        let sig = match web_push::VapidSignatureBuilder::from_pem(
            keys.private_key_pem.as_bytes(),
            &sub_info,
        ) {
            Ok(mut builder) => {
                // VAPID `sub` claim is required by FCM (Chrome) and Apple push services.
                // Must be a valid mailto: or https: URL — Apple rejects localhost.
                builder.add_claim("sub", "mailto:push@cognos.app");
                match builder.build() {
                    Ok(sig) => sig,
                    Err(e) => {
                        log!("Failed to build VAPID signature: {}", e);
                        continue;
                    }
                }
            }
            Err(e) => {
                log!("Failed to create VAPID builder: {}", e);
                continue;
            }
        };

        let mut msg_builder = web_push::WebPushMessageBuilder::new(&sub_info);
        msg_builder.set_payload(
            web_push::ContentEncoding::Aes128Gcm,
            payload_bytes.as_bytes(),
        );
        msg_builder.set_vapid_signature(sig);

        let message = match msg_builder.build() {
            Ok(m) => m,
            Err(e) => {
                log!("Failed to build push message: {}", e);
                continue;
            }
        };

        use web_push::WebPushClient;
        match client.send(message).await {
            Ok(_) => {
                log!(
                    "Sent notification to {}",
                    &sub.endpoint[..sub.endpoint.floor_char_boundary(60)]
                );
            }
            Err(e) => {
                let err_str = e.to_string();
                // 410 Gone means the subscription is no longer valid
                if err_str.contains("410") || err_str.contains("Gone") {
                    log!(
                        "Subscription expired (410), will remove: {}",
                        &sub.endpoint[..sub.endpoint.floor_char_boundary(60)]
                    );
                    stale_endpoints.push(sub.endpoint.clone());
                } else {
                    log!(
                        "Failed to send to {}: {}",
                        &sub.endpoint[..sub.endpoint.floor_char_boundary(60)],
                        e
                    );
                }
            }
        }
    }

    // Clean up stale subscriptions
    for endpoint in stale_endpoints {
        if let Err(e) = PushSubscriptionStore::unsubscribe(pool, &endpoint).await {
            log!("Failed to remove stale subscription: {}", e);
        }
    }
}

/// Build the JSON payload delivered to the service worker.
///
/// `title` and `body` are always present. The optional fields are included only
/// when set so the service worker can rely on `if (data.thread_id)` to detect a
/// deep link without seeing `null`/`undefined` strings.
fn build_push_payload(
    title: &str,
    body: &str,
    notification_id: Option<uuid::Uuid>,
    app_id: Option<&str>,
    link_thread_id: Option<uuid::Uuid>,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "title": title,
        "body": body,
    });
    if let Some(id) = notification_id {
        payload["notification_id"] = serde_json::json!(id.to_string());
    }
    if let Some(aid) = app_id {
        payload["app_id"] = serde_json::json!(aid);
    }
    if let Some(tid) = link_thread_id {
        payload["thread_id"] = serde_json::json!(tid.to_string());
    }
    payload
}

/// Drop subscriptions whose device_id is in `focused_device_ids`. Subscriptions
/// without a device_id (legacy rows) are always kept — we have no way to know
/// whether they belong to a focused device.
fn filter_subscriptions_by_presence(
    subscriptions: Vec<PushSubscription>,
    focused_device_ids: &[String],
) -> Vec<PushSubscription> {
    if focused_device_ids.is_empty() {
        return subscriptions;
    }
    let focused: std::collections::HashSet<&str> =
        focused_device_ids.iter().map(String::as_str).collect();
    subscriptions
        .into_iter()
        .filter(|s| match &s.device_id {
            Some(d) => !focused.contains(d.as_str()),
            None => true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(endpoint: &str, device_id: Option<&str>) -> PushSubscription {
        PushSubscription {
            endpoint: endpoint.into(),
            p256dh: "p256dh".into(),
            auth: "auth".into(),
            device_id: device_id.map(String::from),
        }
    }

    #[test]
    fn no_focused_devices_keeps_everything() {
        let subs = vec![
            sub("https://endpoint/a", Some("dev-1")),
            sub("https://endpoint/b", Some("dev-2")),
        ];
        let filtered = filter_subscriptions_by_presence(subs, &[]);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn drops_only_focused_devices() {
        let subs = vec![
            sub("https://endpoint/a", Some("dev-1")),
            sub("https://endpoint/b", Some("dev-2")),
            sub("https://endpoint/c", Some("dev-3")),
        ];
        let filtered = filter_subscriptions_by_presence(subs, &["dev-2".into()]);
        assert_eq!(filtered.len(), 2);
        let ids: Vec<&str> = filtered
            .iter()
            .filter_map(|s| s.device_id.as_deref())
            .collect();
        assert_eq!(ids, vec!["dev-1", "dev-3"]);
    }

    #[test]
    fn payload_minimal_has_title_and_body_only() {
        let payload = build_push_payload("Hi", "There", None, None, None);
        assert_eq!(payload["title"], "Hi");
        assert_eq!(payload["body"], "There");
        assert!(payload.get("notification_id").is_none());
        assert!(payload.get("app_id").is_none());
        assert!(payload.get("thread_id").is_none());
    }

    #[test]
    fn payload_includes_thread_id_for_deep_link() {
        let tid = uuid::Uuid::parse_str("12345678-1234-5678-1234-567812345678").unwrap();
        let payload = build_push_payload("Claude is asking", "Pick one", None, None, Some(tid));
        assert_eq!(payload["thread_id"], tid.to_string());
    }

    #[test]
    fn payload_omits_thread_id_when_link_absent() {
        // Regression guard: the SW relies on `if (data.thread_id)` — never emit
        // the key as a literal string "null" or empty value.
        let payload = build_push_payload("Hi", "There", None, Some("app-x"), None);
        assert!(payload.get("thread_id").is_none());
        assert_eq!(payload["app_id"], "app-x");
    }

    #[test]
    fn payload_carries_all_fields_when_provided() {
        let nid = uuid::Uuid::new_v4();
        let tid = uuid::Uuid::new_v4();
        let payload = build_push_payload("T", "B", Some(nid), Some("the-app"), Some(tid));
        assert_eq!(payload["notification_id"], nid.to_string());
        assert_eq!(payload["app_id"], "the-app");
        assert_eq!(payload["thread_id"], tid.to_string());
    }

    #[test]
    fn keeps_subscriptions_with_no_device_id() {
        // Legacy push subscriptions without a device_id — we can't tell which
        // device they belong to, so we conservatively keep them.
        let subs = vec![
            sub("https://endpoint/legacy", None),
            sub("https://endpoint/a", Some("dev-1")),
        ];
        let filtered = filter_subscriptions_by_presence(subs, &["dev-1".into()]);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].endpoint, "https://endpoint/legacy");
    }
}
