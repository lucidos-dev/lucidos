//! Web Push notification support for Lucidos
//!
//! Manages VAPID keys, push subscriptions, and sending notifications
//! to all registered browser endpoints.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

#[cfg(feature = "e2e-test-hooks")]
use super::push_test_log;

use crate::api::presence_pong::PresencePong;
use crate::api::SharedEngine;

/// Spec §3: the engine waits this long for pongs before deciding push.
///
/// Sized for the slowest real band, a first packet after the phone radio
/// resumes from idle over a tunnel (1100-1800 ms observed). A deadline inside
/// that band fires an OS push on top of a toast the page already rendered.
///
/// The wait only blocks fan-out when a candidate fails to pong. The short
/// circuit in [`run_presence_check`] wakes once every expected device has
/// answered, so a longer deadline costs latency only when a `device_presence`
/// row is stale.
pub const DEADLINE_MS: u32 = 2000;

/// Spec §2 Step A: push_allowed iff no pong reports active.
pub fn decide_push_allowed(pongs: &[PresencePong]) -> bool {
    !pongs.iter().any(|p| p.is_active)
}

/// Spec §2 Step A / §3: how many pongs to wait for, and (via `> 0`) whether to
/// run the PresenceCheck at all. The max of the two signals, because each
/// covers the other's failure:
///
/// - Open SSE streams are the robust signal. iOS suspends the heartbeat on a
///   foregrounded PWA, so an active page's `device_presence` row goes stale
///   while its EventSource stays open and would pong `is_active`.
/// - Fresh heartbeat rows cover the inverse: a page that heartbeated recently
///   but whose SSE connection just dropped.
///
/// `0` means nobody is reachable, so skip the protocol and push directly.
fn expected_pong_count(sse_connections: usize, candidate_count: usize) -> usize {
    sse_connections.max(candidate_count)
}

/// A browser push subscription (endpoint + encryption keys)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushSubscription {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub device_id: Option<String>,
    pub scope_url: Option<String>,
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
    /// Defensive double-write: the migration owns this CREATE TABLE. Slated
    /// for removal in `harden-init-schema-tables-vs-migrations-pattern-finish`.
    pub async fn init_schema(
        pool: &PgPool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS push_subscriptions (
                endpoint TEXT PRIMARY KEY,
                p256dh TEXT NOT NULL,
                auth TEXT NOT NULL,
                scope_url TEXT,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )",
        )
        .execute(pool)
        .await?;
        sqlx::query("ALTER TABLE push_subscriptions ADD COLUMN IF NOT EXISTS scope_url TEXT")
            .execute(pool)
            .await?;

        Ok(())
    }

    /// Store a push subscription (upsert by endpoint)
    pub async fn subscribe(
        pool: &PgPool,
        sub: &PushSubscription,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // A browser mints a new endpoint on re-subscribe, so the device's old
        // row is stale. Without a device_id there is nothing to match on, and
        // the endpoint upsert below is the whole story.
        if let Some(device_id) = &sub.device_id {
            sqlx::query("DELETE FROM push_subscriptions WHERE device_id = $1")
                .bind(device_id)
                .execute(pool)
                .await?;
        }
        sqlx::query(
            "INSERT INTO push_subscriptions (endpoint, p256dh, auth, device_id, scope_url)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (endpoint) DO UPDATE SET p256dh = $2, auth = $3, device_id = $4, scope_url = COALESCE($5, push_subscriptions.scope_url)",
        )
        .bind(&sub.endpoint)
        .bind(&sub.p256dh)
        .bind(&sub.auth)
        .bind(&sub.device_id)
        .bind(&sub.scope_url)
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

    /// Push subscriptions whose device has push enabled, each paired with that
    /// device's `user_agent`.
    ///
    /// The caller gates wake-pushes (Layer 3 in
    /// `system-knowhow/notifications.md` §4.5) on the UA. A row with no
    /// `device_id`, and a device with no recorded UA, both pair with `None`.
    pub async fn get_push_enabled(
        pool: &PgPool,
    ) -> Result<Vec<(PushSubscription, Option<String>)>, Box<dyn std::error::Error + Send + Sync>>
    {
        let rows = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                Option<String>,
                Option<String>,
                Option<String>,
            ),
        >(
            "SELECT ps.endpoint, ps.p256dh, ps.auth, ps.device_id, ps.scope_url, d.user_agent
                 FROM push_subscriptions ps
                 LEFT JOIN devices d ON ps.device_id = d.id
                 WHERE ps.device_id IS NULL OR d.push_enabled = true",
        )
        .fetch_all(pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(endpoint, p256dh, auth, device_id, scope_url, user_agent)| {
                    (
                        PushSubscription {
                            endpoint,
                            p256dh,
                            auth,
                            device_id,
                            scope_url,
                        },
                        user_agent,
                    )
                },
            )
            .collect())
    }

    /// Push subscriptions for a single device, for the wake-push path. The
    /// wake reaches one device only, so unjamming its service worker does not
    /// send every other device a duplicate notification. Empty when the device
    /// has push disabled or has no subscription.
    pub async fn get_push_enabled_for_device(
        pool: &PgPool,
        device_id: &str,
    ) -> Result<Vec<PushSubscription>, Box<dyn std::error::Error + Send + Sync>> {
        let rows = sqlx::query_as::<_, (String, String, String, Option<String>, Option<String>)>(
            "SELECT ps.endpoint, ps.p256dh, ps.auth, ps.device_id, ps.scope_url
             FROM push_subscriptions ps
             LEFT JOIN devices d ON ps.device_id = d.id
             WHERE ps.device_id = $1
               AND (d.push_enabled IS NULL OR d.push_enabled = true)",
        )
        .bind(device_id)
        .fetch_all(pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(endpoint, p256dh, auth, device_id, scope_url)| PushSubscription {
                    endpoint,
                    p256dh,
                    auth,
                    device_id,
                    scope_url,
                },
            )
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

    if let Some(keys_json) = PreferenceStore::get(pool, "vapid_keys").await? {
        let keys: VapidKeys = serde_json::from_str(&keys_json)?;
        return Ok(keys);
    }

    let signing_key = SigningKey::random(&mut OsRng);

    let private_key_pem = signing_key
        .to_pkcs8_pem(p256::pkcs8::LineEnding::LF)
        .map_err(|e| format!("Failed to encode private key: {}", e))?
        .to_string();

    let verifying_key = signing_key.verifying_key();
    let pub_bytes = verifying_key.to_encoded_point(false);
    let public_key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(pub_bytes.as_bytes());

    let keys = VapidKeys {
        private_key_pem,
        public_key,
    };

    let keys_json = serde_json::to_string(&keys)?;
    PreferenceStore::set_silent(pool, "vapid_keys", &keys_json).await?;

    log!("[Push] Generated new VAPID key pair");
    Ok(keys)
}

/// Send a push notification to all registered subscriptions. With a
/// `notification_id`, clicking the notification deep-links to it.
pub async fn send_push_to_all(
    engine: &SharedEngine,
    title: &str,
    body: &str,
    notification_id: Option<uuid::Uuid>,
) {
    send_push_to_all_with_app(
        engine,
        title,
        body,
        notification_id,
        None,
        None,
        None,
        crate::scheduler::notifications::Tap::Modal,
    )
    .await;
}

/// Fan out the OS surface for a notification per the §2 matrix in
/// `system-knowhow/notifications.md`. The PresenceCheck pong protocol (§3) is
/// the authoritative decision input:
/// - `push_allowed = false`, so an active device pong'd in: emit
///   `NotificationToastRequested` and active pages show the in-app toast.
/// - `push_allowed = true`: emit `NativePushRequested` for a connected Tauri
///   desktop app, and fan the web push out to every browser subscription.
///
/// The two emits sit on opposite branches of one decision, so a device can
/// never get both a toast and a push. The decision runs whenever ANY client is
/// reachable, so a desktop-only workspace with no web-push subscription still
/// gets toasts and native banners.
///
/// Non-fatal throughout: one bad subscription must not sink the fan-out.
#[allow(clippy::too_many_arguments)]
pub async fn send_push_to_all_with_app(
    engine: &SharedEngine,
    title: &str,
    body: &str,
    notification_id: Option<uuid::Uuid>,
    app_id: Option<&str>,
    link_thread_id: Option<uuid::Uuid>,
    link_event_id: Option<uuid::Uuid>,
    tap: crate::scheduler::notifications::Tap,
) {
    let pool = engine.pool();
    // MAY be empty: a desktop-only workspace never creates one, because the
    // embedded WKWebView cannot subscribe to Web Push. Do NOT bail on empty
    // here. A connected client still needs the toast or the native banner, and
    // the "nobody reachable" bail comes once the client count is known.
    let subs_with_ua = match PushSubscriptionStore::get_push_enabled(pool).await {
        Ok(subs) => subs,
        Err(e) => {
            log!("[Push] Failed to fetch subscriptions: {}", e);
            return;
        }
    };

    let candidates = match crate::core::DevicePresenceStore::candidates(pool).await {
        Ok(c) => c,
        Err(e) => {
            // Spec §3 failure handling: with no view of the candidates, the
            // safer side is to send the push.
            log!(
                "[Push] Failed to query device_presence candidates (sending push anyway): {}",
                e
            );
            Vec::new()
        }
    };

    let sse_connections = engine.sse_connections.count();
    let expected_pongs = expected_pong_count(sse_connections, candidates.len());

    // Nothing to deliver to at all: no web-push subscription AND nobody
    // connected or recently heartbeating, so no client for a toast or a native
    // banner either. Both halves are required, because a connected client with
    // no subscription anywhere is still a valid recipient.
    if subs_with_ua.is_empty() && expected_pongs == 0 {
        return;
    }

    // Pick wake targets before the move below consumes the paired list.
    let wake_targets = pick_mac_chromium_wake_targets(&subs_with_ua);
    let subscriptions: Vec<PushSubscription> =
        subs_with_ua.into_iter().map(|(sub, _)| sub).collect();

    let push_allowed = if expected_pongs == 0 {
        // Nobody to pong, so push immediately.
        true
    } else if let Some(nid) = notification_id {
        run_presence_check(
            &engine.event_bus,
            &engine.presence_tracker,
            nid,
            link_event_id,
            expected_pongs,
        )
        .await
    } else {
        // No notification_id to scope the pong by, so the protocol cannot run.
        true
    };

    if !push_allowed {
        // An active device pong'd in, so the OS push is suppressed (§2 Step A)
        // and active pages render the in-app toast instead (§4). This is the
        // only place the toast is triggered. `notification_id` is always Some
        // here: `push_allowed` only goes false through the arm above, which
        // requires it.
        if let Some(nid) = notification_id {
            emit_toast_requested(
                &engine.event_bus,
                nid,
                title,
                body,
                link_thread_id,
                link_event_id,
                app_id,
                tap,
            )
            .await;
        }
        log!(
            "[Push] Suppressed OS push (PresenceCheck: an active device pinged in); \
             requested in-app toast instead"
        );
        return;
    }

    // Native desktop surface (§1, §4). A connected Tauri app cannot receive
    // the web push below, because its WKWebView has no service-worker push.
    // Browser clients ignore this frame; only a non-active Tauri client acts
    // on it. It rides the branch opposite `emit_toast_requested`, so a device
    // never gets both a native banner and an in-app toast.
    if let Some(nid) = notification_id {
        emit_native_push_requested(
            &engine.event_bus,
            nid,
            title,
            body,
            link_thread_id,
            link_event_id,
            app_id,
            tap.clone(),
        )
        .await;
    }

    // No web-push subscriptions, so the native broadcast above is the whole OS
    // surface.
    if subscriptions.is_empty() {
        return;
    }

    // Declarative `app_badge` (see `build_push_payload`). Both halves are
    // resolved once for the whole fan-out; `app_badge_for` picks which one a
    // subscription gets. Best-effort: a failure omits the badge field, or
    // falls back to the own count.
    let unread_count = read_unread_count(pool).await;
    let cross_workspace_total = cross_workspace_unread_total(&subscriptions).await;

    let deliveries: Vec<(PushSubscription, String)> = subscriptions
        .into_iter()
        .map(|sub| {
            let payload_bytes = build_push_payload_fitted(
                title,
                body,
                notification_id,
                app_id,
                link_thread_id,
                link_event_id,
                &tap,
                sub.scope_url.as_deref(),
                app_badge_for(
                    sub.scope_url.as_deref(),
                    unread_count,
                    cross_workspace_total,
                ),
            )
            .to_string();
            (sub, payload_bytes)
        })
        .collect();

    fan_out_payload(pool, deliveries, "notification", notification_id).await;

    // Layer 3 of the macOS-Chromium wedge mitigation (see
    // `system-knowhow/notifications.md` §4.5). The wake's only job is to be a
    // second push event arriving at the service worker, which drains any
    // queued `notificationclick`.
    //
    // Gated on `notification_id`: `send_wake_push_to_device` loads the
    // persisted notification to build the wake payload, so a wake without an
    // id has nothing to mirror.
    if let Some(nid) = notification_id {
        schedule_mac_chromium_wakes(engine.clone(), wake_targets, nid, MAC_CHROMIUM_WAKE_DELAY);
    }
}

/// Delay between the original push and the follow-up wake-push (Layer 3 in
/// `system-knowhow/notifications.md` §4.5).
///
/// Short enough that a click queued in the wedged service worker drains before
/// the user gives up on the dead tap. Long enough that Chrome does not
/// coalesce the two into one event, which would defeat the point: the wake
/// must be a separate push dispatch to resurrect the worker.
const MAC_CHROMIUM_WAKE_DELAY: std::time::Duration = std::time::Duration::from_secs(3);

/// Spawn one delayed wake-push per device id. Fire and forget: the wake is
/// best-effort, and a dropped one leaves the next genuine push to the device
/// to drain the queued click.
///
/// **Surviving engine shutdown is incidental, never guaranteed.** The spawned
/// task holds an `Arc<LucidosEngine>`, but its `JoinHandle` is dropped, so a
/// runtime drop aborts it at the next `await` whatever the strong count.
/// Graceful shutdown usually outlasts the sleep, so the wake usually completes
/// anyway.
fn schedule_mac_chromium_wakes(
    engine: SharedEngine,
    device_ids: Vec<String>,
    notification_id: uuid::Uuid,
    delay: std::time::Duration,
) {
    for device_id in device_ids {
        let engine = engine.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            match send_wake_push_to_device(&engine, &device_id, notification_id).await {
                Ok(0) => log!(
                    "[Push] Layer-3 wake-push for {} on {}: nothing sent \
                     (notification read during the delay window, or device \
                     unsubscribed)",
                    notification_id,
                    device_id
                ),
                Ok(n) => log!(
                    "[Push] Layer-3 wake-push for {} delivered to {} sub(s) on {}",
                    notification_id,
                    n,
                    device_id
                ),
                Err(e) => log!(
                    "[Push] Layer-3 wake-push for {} on {} failed: {}",
                    notification_id,
                    device_id,
                    e
                ),
            }
        });
    }
}

/// Broadcast a PresenceCheck and wait up to `DEADLINE_MS` for every candidate
/// device to pong. Returns the `push_allowed` decision per §2 Step A: true iff
/// no pong reports `is_active`. Drains the tracker slot either way.
///
/// The PresenceCheck carries no toast content. [`emit_toast_requested`] fires
/// AFTER this decision resolves, so the toast and the OS push can never both
/// happen (notifications.md §3-§4). `event_id` rides along only so the pong
/// can report `event_in_viewport`.
async fn run_presence_check(
    event_bus: &crate::engine::event_bus::EventBus,
    presence_tracker: &crate::api::presence_pong::PresenceTracker,
    notification_id: uuid::Uuid,
    event_id: Option<uuid::Uuid>,
    expected_pongs: usize,
) -> bool {
    let sent_at_ms = crate::engine::now_epoch_millis();
    let notify = presence_tracker.expect(notification_id, expected_pongs);
    if let Err(e) = event_bus
        .emit(crate::engine::event_bus::BusEvent::System(
            crate::engine::event_bus::SystemEvent::PresenceCheck {
                notification_id,
                event_id,
                deadline_ms: DEADLINE_MS,
                sent_at_ms,
                actor: None,
            },
        ))
        .await
    {
        // No page knows to pong, so no informed decision is possible. Drain
        // the slot and send the push, the safer side.
        log!("[Push] Failed to broadcast PresenceCheck: {}", e);
        let _ = presence_tracker.collect(notification_id);
        return true;
    }

    let _ = tokio::time::timeout(
        std::time::Duration::from_millis(DEADLINE_MS as u64),
        notify.notified(),
    )
    .await;

    let pongs = presence_tracker.collect(notification_id);
    decide_push_allowed(&pongs)
}

/// Broadcast a [`SystemEvent::NotificationToastRequested`] so active pages
/// render the in-app toast for a notification whose OS push was suppressed.
///
/// Broadcast SSE: hidden pages ignore it via the §4 row matrix. Non-fatal, and
/// the worst case is the bell badge bumping without the toast. See
/// `system-knowhow/notifications.md` §4.
#[allow(clippy::too_many_arguments)]
async fn emit_toast_requested(
    event_bus: &crate::engine::event_bus::EventBus,
    notification_id: uuid::Uuid,
    title: &str,
    body: &str,
    thread_id: Option<uuid::Uuid>,
    event_id: Option<uuid::Uuid>,
    app_id: Option<&str>,
    tap: crate::scheduler::notifications::Tap,
) {
    let sent_at_ms = crate::engine::now_epoch_millis();
    if let Err(e) = event_bus
        .emit(crate::engine::event_bus::BusEvent::System(
            crate::engine::event_bus::SystemEvent::NotificationToastRequested {
                notification_id,
                title: title.to_string(),
                body: body.to_string(),
                thread_id,
                event_id,
                app_id: app_id.map(|s| s.to_string()),
                tap,
                sent_at_ms,
            },
        ))
        .await
    {
        log!(
            "[Push] Failed to broadcast NotificationToastRequested for {}: {}",
            notification_id,
            e
        );
    }
}

/// Broadcast a [`SystemEvent::NativePushRequested`] so a connected Tauri
/// desktop app renders a native macOS banner for an allowed push.
///
/// The desktop counterpart of the web-push fan-out. A Tauri app's WKWebView
/// cannot subscribe to Web Push, so the engine reaches it over the already-open
/// SSE stream. Browser pages ignore the frame, and a Tauri page shows the
/// banner only when it is not active (§4 row matrix).
///
/// Non-fatal, and the worst case is the desktop user seeing only the bell
/// badge. See `system-knowhow/notifications.md` §1, §4.
#[allow(clippy::too_many_arguments)]
async fn emit_native_push_requested(
    event_bus: &crate::engine::event_bus::EventBus,
    notification_id: uuid::Uuid,
    title: &str,
    body: &str,
    thread_id: Option<uuid::Uuid>,
    event_id: Option<uuid::Uuid>,
    app_id: Option<&str>,
    tap: crate::scheduler::notifications::Tap,
) {
    let sent_at_ms = crate::engine::now_epoch_millis();
    if let Err(e) = event_bus
        .emit(crate::engine::event_bus::BusEvent::System(
            crate::engine::event_bus::SystemEvent::NativePushRequested {
                notification_id,
                title: title.to_string(),
                body: body.to_string(),
                thread_id,
                event_id,
                app_id: app_id.map(|s| s.to_string()),
                tap,
                sent_at_ms,
            },
        ))
        .await
    {
        log!(
            "[Push] Failed to broadcast NativePushRequested for {}: {}",
            notification_id,
            e
        );
    }
}

/// Broadcast a [`SystemEvent::NativePushDismissRequested`] so a connected Tauri
/// desktop app REMOVES banners for a notification just read on any device.
/// `Some(id)` removes one banner; `None` removes all.
///
/// The macOS-desktop-only half of cross-device dismiss. Browser pages ignore
/// it: the open web cannot silently remove a Web Push banner, and Safari
/// revokes a subscription after three silent pushes.
///
/// Non-fatal, and the worst case is a stale OS banner the user swipes away.
/// See `system-knowhow/notifications.md` §4 and
/// `docs/plans/2026-05-18-cross-device-notification-dismiss-design.md`.
pub(crate) async fn emit_native_push_dismiss_requested(
    event_bus: &crate::engine::event_bus::EventBus,
    notification_id: Option<uuid::Uuid>,
) {
    let sent_at_ms = crate::engine::now_epoch_millis();
    if let Err(e) = event_bus
        .emit(crate::engine::event_bus::BusEvent::System(
            crate::engine::event_bus::SystemEvent::NativePushDismissRequested {
                notification_id,
                sent_at_ms,
            },
        ))
        .await
    {
        log!(
            "[Push] Failed to broadcast NativePushDismissRequested for {:?}: {}",
            notification_id,
            e
        );
    }
}

/// Whether a device is affected by Chromium #370536109, the macOS-Chromium
/// dispatcher bug that silently queues `notificationclick` until a new push
/// event drains the service worker.
///
/// This gates the Layer 3 wake-push (`system-knowhow/notifications.md` §4.5),
/// so every other browser avoids an extra push per notification. The UA is
/// stored on the `devices` row at register time, so the decision needs no
/// per-push round trip to the page.
fn is_mac_chromium(user_agent: &str) -> bool {
    user_agent.contains("Macintosh")
        && user_agent.contains("Chrome/")
        && !user_agent.contains("iPhone")
        && !user_agent.contains("iPad")
        && !user_agent.contains("iPod")
}

/// The deduplicated device ids that should receive a follow-up wake-push.
///
/// **Dedup matters.** Two tabs on one device produce two subscriptions sharing
/// a `device_id`, and `send_wake_push_to_device` already fans out to every
/// subscription on the device. Once is enough to drain the queued
/// `notificationclick`.
///
/// A subscription with no `device_id`, or a device with no recorded
/// `user_agent`, is skipped: neither can be targeted confidently, and the next
/// genuine push drains the queue instead.
fn pick_mac_chromium_wake_targets(
    subs_with_ua: &[(PushSubscription, Option<String>)],
) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (sub, ua) in subs_with_ua {
        let Some(ua) = ua.as_deref() else { continue };
        if !is_mac_chromium(ua) {
            continue;
        }
        let Some(device_id) = sub.device_id.as_deref() else {
            continue;
        };
        if seen.insert(device_id.to_string()) {
            out.push(device_id.to_string());
        }
    }
    out
}

/// Whether a scheduled wake push should still fire once its delay elapses.
///
/// A wake push exists to resurrect a wedged service worker (Chromium
/// #370536109) so a queued `notificationclick` drains. A notification already
/// marked `read` proves the tap landed, so the worker was not wedged. Waking
/// then has nothing to drain, and macOS stacks a fresh banner rather than
/// replacing the one the tap closed.
///
/// The caller re-fetches the live read state at fire time, because the tap
/// lands DURING the wake delay. See `system-knowhow/notifications.md` §4.5.
fn wake_still_needed(notification: &crate::scheduler::notifications::Notification) -> bool {
    !notification.read
}

/// Send a wake-push to a single device, the workaround for Chromium #370536109.
/// Returns how many subscriptions it delivered to, `0` when skipped.
///
/// The payload carries the SAME content as the original, so Chrome counts the
/// push as visible (§4.5). Sending a push is the canonical way to wake an
/// inactive service worker: any push event resurrects the worker thread and
/// drains queued `notificationclick` events as a side effect.
pub(crate) async fn send_wake_push_to_device(
    engine: &crate::engine::LucidosEngine,
    device_id: &str,
    notification_id: uuid::Uuid,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let pool = engine.pool();
    let notification =
        match crate::scheduler::NotificationStore::get_by_id(pool, notification_id).await? {
            Some(n) => n,
            None => return Ok(0),
        };

    // Re-checked at fire time: a tap during the wake delay marks the
    // notification read, and the wake would only re-pop it as a fresh banner.
    if !wake_still_needed(&notification) {
        return Ok(0);
    }

    let subscriptions = PushSubscriptionStore::get_push_enabled_for_device(pool, device_id).await?;
    if subscriptions.is_empty() {
        return Ok(0);
    }

    let unread_count = read_unread_count(pool).await;
    let cross_workspace_total = cross_workspace_unread_total(&subscriptions).await;
    let deliveries: Vec<(PushSubscription, String)> = subscriptions
        .into_iter()
        .map(|sub| {
            let payload_bytes = build_wake_payload(
                &notification,
                sub.scope_url.as_deref(),
                app_badge_for(
                    sub.scope_url.as_deref(),
                    unread_count,
                    cross_workspace_total,
                ),
            )
            .to_string();
            (sub, payload_bytes)
        })
        .collect();
    Ok(fan_out_payload(pool, deliveries, "wake", Some(notification_id)).await)
}

/// Default notification tag — used when `notification_id` is absent so the
/// browser still deduplicates repeat pushes for the same logical channel.
/// Mirrors `DEFAULT_NOTIFICATION_TAG` in `sw.js`.
const DEFAULT_NOTIFICATION_TAG: &str = "lucidos-notification";

/// Top-level magic that opts the payload into Declarative Web Push parsing
/// (RFC 8030 homage). Required by Safari 18.5+ to read `notification.navigate`
/// for the tap path even though the iOS SW `push` handler may still run. See
/// `system-knowhow/notifications.md` §4.5.
const DECLARATIVE_WEB_PUSH_MAGIC: i64 = 8030;

/// Hard ceiling the web-push transport enforces, in BYTES of the plaintext
/// handed to `set_payload`.
///
/// Measured against the crate, not assumed. The check runs BEFORE encryption
/// on the serialized JSON envelope, so the encrypted size never enters into
/// it. Do not trust the crate's own error text, which reports a different
/// number in a different unit.
/// `s4_5_crate_ceiling_constant_matches_what_build_actually_enforces` pins
/// this from both sides, so a crate bump fails a test instead of silently
/// dropping pushes.
const MAX_PUSH_PAYLOAD_BYTES: usize = 3052;

/// Reserve a payload we had to truncate leaves below [`MAX_PUSH_PAYLOAD_BYTES`].
///
/// Correctness does not rest on it: [`fit_payload_body`] searches on the REAL
/// serialized length, so the fit is exact rather than estimated. This is
/// deliberate slack, so a body we already had to cut is not also sitting one
/// byte under the transport's limit. A payload that fits without truncation is
/// measured against the full ceiling and passed through untouched.
const PUSH_PAYLOAD_SAFETY_MARGIN_BYTES: usize = 64;

/// What a TRUNCATED payload aims for: the hard ceiling less the reserve above.
/// [`fit_payload_body`] falls back to the hard ceiling when the envelope alone
/// already eats into the reserve, so the slack can never cost deliverable text.
const TRUNCATED_PAYLOAD_TARGET_BYTES: usize =
    MAX_PUSH_PAYLOAD_BYTES - PUSH_PAYLOAD_SAFETY_MARGIN_BYTES;

/// Appended to a body the size guard had to cut, so the banner reads as
/// truncated instead of as a sentence that stops mid-word. The full text is
/// always intact in the notification row: the push is a banner, not the content
/// of record.
const TRUNCATION_MARKER: &str = "…";

/// How far back from the cut point [`truncate_body_to_bytes`] will look for a
/// whitespace boundary. Snapping to one keeps the banner from ending mid-word;
/// beyond this distance the lost text costs more than the ragged edge.
const TRUNCATION_WHITESPACE_LOOKBACK: usize = 96;

/// Cut `body` down to at most `budget` BYTES, [`TRUNCATION_MARKER`] included.
///
/// UTF-8 safe by construction: the cut lands on a char boundary, so the
/// multi-byte characters real bodies carry are never split. A whitespace
/// boundary within [`TRUNCATION_WHITESPACE_LOOKBACK`] bytes of the cut wins,
/// so the banner ends on a word.
fn truncate_body_to_bytes(body: &str, budget: usize) -> String {
    if body.len() <= budget {
        return body.to_string();
    }
    if budget < TRUNCATION_MARKER.len() {
        // No room even for the marker. Drop the body; the rest of the envelope
        // (title, navigate, tag, badge) still carries a usable banner.
        return String::new();
    }
    let mut cut = body.floor_char_boundary(budget - TRUNCATION_MARKER.len());
    let lookback_floor = cut.saturating_sub(TRUNCATION_WHITESPACE_LOOKBACK);
    if let Some(ws) = body[..cut].rfind(char::is_whitespace) {
        if ws >= lookback_floor && ws > 0 {
            cut = ws;
        }
    }
    let mut out = body[..cut].trim_end().to_string();
    out.push_str(TRUNCATION_MARKER);
    out
}

/// Build a push payload whose serialized form is guaranteed to fit
/// [`MAX_PUSH_PAYLOAD_BYTES`], shrinking the BODY (and only the body) by as
/// much as it takes.
///
/// `build` renders the whole envelope around a candidate body, so the budget
/// is measured against the exact bytes the transport will see. The envelope's
/// overhead is both substantial and VARIABLE: the iOS `navigate` URL comes
/// from the subscription's own `scope_url`, `data` carries a second navigate
/// URL, and the wake variant adds a `wake: true` sibling. All of those survive
/// the cut intact.
///
/// The budget is found by BINARY SEARCH on the rendered length, never by
/// arithmetic. JSON escaping is not a fixed cost, so "ceiling minus envelope"
/// is an upper bound only. A single correction saturates to zero on a
/// control-char-dense body, and the banner then ships EMPTY where hundreds of
/// characters would have fitted. Truncation and rendered length are both
/// monotone in the budget, so the search lands on the largest body that fits.
fn fit_payload_body(
    body: &str,
    kind: &str,
    build: impl Fn(&str) -> serde_json::Value,
) -> serde_json::Value {
    let rendered_len = |candidate_body: &str| build(candidate_body).to_string().len();

    // The common case renders the envelope ONCE and hands that same value
    // back, so there is no second build on the hot path.
    let full = build(body);
    if full.to_string().len() <= MAX_PUSH_PAYLOAD_BYTES {
        return full;
    }

    // Everything the body shares the ceiling with, measured for THIS
    // subscription. Used to pick the target and to report the overhead. The
    // search below does not trust it as an arithmetic budget.
    let envelope_len = rendered_len("");

    // Aim for the reserve when there is room for one, and for the hard ceiling
    // when the envelope already eats into it. The reserve is deliberate slack,
    // and slack must never cost the user deliverable text.
    let target = if envelope_len <= TRUNCATED_PAYLOAD_TARGET_BYTES {
        TRUNCATED_PAYLOAD_TARGET_BYTES
    } else {
        MAX_PUSH_PAYLOAD_BYTES
    };

    // Largest byte budget whose rendered payload still fits the target. `lo`
    // holds the best budget known to fit, `hi` the smallest known not to. When
    // even an empty body overshoots, `lo` stays 0 and the branch below reports
    // it.
    let (mut lo, mut hi) = (0usize, body.len());
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if rendered_len(&truncate_body_to_bytes(body, mid)) <= target {
            lo = mid;
        } else {
            hi = mid;
        }
    }

    let truncated = truncate_body_to_bytes(body, lo);
    let payload = build(&truncated);
    let payload_len = payload.to_string().len();
    if payload_len <= MAX_PUSH_PAYLOAD_BYTES {
        log!(
            "[Push] Truncated {} body to fit the {} B payload ceiling: {} B of body kept {} B \
             ({} B payload, {} B envelope overhead). The notification row keeps the full text.",
            kind,
            MAX_PUSH_PAYLOAD_BYTES,
            body.len(),
            truncated.len(),
            payload_len,
            envelope_len
        );
    } else {
        // Nothing left to give: the envelope alone overshoots. Returned anyway,
        // so the send loop's last-resort arm reports the real per-subscription
        // failure.
        log!(
            "[Push] Cannot fit the {} payload for this subscription: {} B with the body dropped \
             entirely, over the {} B ceiling. The envelope alone (title plus deep-link URLs) is \
             too large, so NO push will be delivered here.",
            kind,
            payload_len,
            MAX_PUSH_PAYLOAD_BYTES
        );
    }
    payload
}

/// Build the `key=value&…` deep-link param string shared by both navigate URL
/// forms. Empty when there is nothing to deep-link, so callers fall back to a
/// bare `/`.
///
/// The `tap` JSON is emitted only for non-modal kinds. Modal-kind URLs stay
/// short, and the page demotes a missing `tap` param to the modal default.
fn build_navigate_params(
    notification_id: Option<uuid::Uuid>,
    link_thread_id: Option<uuid::Uuid>,
    link_event_id: Option<uuid::Uuid>,
    tap: &crate::scheduler::notifications::Tap,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(id) = notification_id {
        parts.push(format!(
            "notification={}",
            urlencoding::encode(&id.to_string())
        ));
    }
    if let Some(tid) = link_thread_id {
        parts.push(format!("thread={}", urlencoding::encode(&tid.to_string())));
    }
    if let Some(eid) = link_event_id {
        parts.push(format!("event={}", urlencoding::encode(&eid.to_string())));
    }
    if !matches!(tap, crate::scheduler::notifications::Tap::Modal) {
        let tap_json = serde_json::to_string(tap).expect("Tap serializes infallibly");
        parts.push(format!("tap={}", urlencoding::encode(&tap_json)));
    }
    parts.join("&")
}

/// iOS Safari declarative-push navigate URL — a **cross-document** (query
/// string) URL.
///
/// Safari handles a declarative push in the parent process and reuses the
/// already-open PWA window on tap. A same-document (hash-only) navigation is
/// NOT applied to that window: WebKit focuses it, the URL never changes, and
/// the page-side router finds nothing to route. A query string changes the
/// document, so iOS performs a real navigation the page picks up on load or
/// resume. See `system-knowhow/notifications.md` §4.5.
///
/// TEMPORARY MEASURE (`docs/temporary-measures.md` § "Cross-document
/// notification-tap reload on iOS"). The cross-document URL is what makes every
/// iOS tap reload the PWA, and it is the only channel WebKit applies today.
/// When WebKit ships a reload-free channel, this function changes shape and the
/// page-side `?notification=` detection comes out with it.
///
/// `scope_url` is the concrete service-worker scope the page captured at
/// subscription time. With it, the URL is absolute; a legacy row without one
/// keeps the relative fallback until its subscription refreshes.
fn navigate_url_ios(params: &str, scope_url: Option<&str>) -> String {
    let relative = if params.is_empty() {
        ".".to_string()
    } else {
        format!("?{params}")
    };
    let Some(scope) = scope_url.and_then(normalize_scope_url) else {
        return relative;
    };
    if params.is_empty() {
        scope
    } else {
        format!("{scope}?{params}")
    }
}

fn normalize_scope_url(scope_url: &str) -> Option<String> {
    let trimmed = scope_url.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return None;
    }
    let no_fragment = trimmed.split('#').next().unwrap_or(trimmed);
    let no_query = no_fragment.split('?').next().unwrap_or(no_fragment);
    if no_query.is_empty() {
        return None;
    }
    if no_query.ends_with('/') {
        Some(no_query.to_string())
    } else {
        Some(format!("{no_query}/"))
    }
}

/// Chrome service-worker navigate URL: a **hash** URL, read off
/// `notification.data.navigate` by the `notificationclick` handler.
///
/// It feeds only the COLD `clients.openWindow()` path. A warm tab is routed by
/// `postMessage` instead, because Chrome fires no `hashchange` for a
/// fragment-only `client.navigate()`. The query-vs-hash split with
/// [`navigate_url_ios`] is deliberate: iOS needs a cross-document URL.
///
/// This stays **scope-relative** (no leading slash), so it resolves inside the
/// gateway `/<slug>/` scope through `resolveNavigate` in `sw.js`.
fn navigate_url_sw(params: &str) -> String {
    if params.is_empty() {
        ".".to_string()
    } else {
        format!("#{params}")
    }
}

/// Read the workspace's current unread-notification count for the declarative
/// `app_badge` field. Best-effort: a query failure logs and returns `None`, so
/// the push still goes out (just without touching the badge that one time).
async fn read_unread_count(pool: &sqlx::PgPool) -> Option<i64> {
    match crate::scheduler::NotificationStore::count_unread(pool).await {
        Ok(count) => Some(count),
        Err(e) => {
            log!("[Push] Failed to count unread (omitting app_badge): {}", e);
            None
        }
    }
}

/// How long the engine waits for the gateway's cross-workspace unread total
/// before badging its own count. A push must never sit behind a badge
/// refinement, so this is shorter than `workspace_label`'s deadline: the hop
/// is one loopback request.
const GATEWAY_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

/// Whether a subscription's recorded service-worker scope belongs to a
/// GATEWAY-served install, i.e. one whose app-icon badge must be the
/// cross-workspace total rather than this workspace's own count.
///
/// `scope_url` is the SW scope the page captured, validated at subscribe time
/// against the forwarded prefix. It is `https://host/<slug>/` behind the
/// gateway and `https://host:<engine-port>/` on a direct engine, so a path
/// deeper than `/` means the gateway served the page.
///
/// The gateway re-stamps every manifest it serves with `scope: "/"`, so ONE
/// installed icon covers the picker and every workspace on that origin. A
/// legacy row with no `scope_url` keeps the own-count behaviour.
fn is_gateway_scoped(scope_url: Option<&str>) -> bool {
    let Some(raw) = scope_url else {
        return false;
    };
    let Ok(url) = reqwest::Url::parse(raw.trim()) else {
        return false;
    };
    url.path() != "/"
}

/// The `app_badge` value one subscription should carry: the cross-workspace
/// total for a gateway-served install, this workspace's own unread count
/// otherwise. `None` leaves the badge untouched.
///
/// The aggregate falls back to the own count rather than to nothing. An
/// unreachable gateway then leaves the icon slightly low, instead of frozen at
/// whatever the previous push wrote.
fn app_badge_for(
    scope_url: Option<&str>,
    own_unread: Option<i64>,
    cross_workspace_total: Option<i64>,
) -> Option<i64> {
    if is_gateway_scoped(scope_url) {
        cross_workspace_total.or(own_unread)
    } else {
        own_unread
    }
}

/// The gateway's fresh cross-workspace unread total, for this fan-out.
///
/// Resolved ONCE per fan-out, and only when a subscription needs it. No
/// gateway-scoped subscription, or no gateway at all, means no request.
/// Best-effort throughout: `None` on every failure path, and the caller then
/// badges the workspace's own count.
async fn cross_workspace_unread_total(subscriptions: &[PushSubscription]) -> Option<i64> {
    if !subscriptions
        .iter()
        .any(|sub| is_gateway_scoped(sub.scope_url.as_deref()))
    {
        return None;
    }
    let gateway_port = crate::api::base_path::gateway_port()?;
    match tokio::time::timeout(
        GATEWAY_TOTAL_TIMEOUT,
        ask_gateway_unread_total(&gateway_port),
    )
    .await
    {
        Ok(total) => total,
        Err(_) => {
            log!(
                "[Push] gateway on :{} did not answer the unread total within {:?}; \
                 badging this workspace's own count",
                gateway_port,
                GATEWAY_TOTAL_TIMEOUT
            );
            None
        }
    }
}

/// The hop itself, unbounded: [`cross_workspace_unread_total`] owns the
/// deadline. Resolved scheme first, the other protocol second, so a
/// dev/packaged TLS mismatch still connects (`.claude/rules/rust.md`
/// § Intra-host scheme).
async fn ask_gateway_unread_total(gateway_port: &str) -> Option<i64> {
    let client = match reqwest::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            log!(
                "[Push] could not build the gateway client ({}); badging this workspace's own count",
                e
            );
            return None;
        }
    };
    for scheme in crate::net_config::peer_scheme_order() {
        let url = format!("{scheme}://127.0.0.1:{gateway_port}/~/api/v1/control/unread-total");
        let Ok(resp) = client.get(&url).send().await else {
            continue; // unreachable on this scheme, so try the other protocol
        };
        if !resp.status().is_success() {
            // The gateway answered and refused; the other scheme won't differ.
            log!(
                "[Push] gateway unread total returned {}; badging this workspace's own count",
                resp.status()
            );
            return None;
        }
        let Ok(body) = resp.text().await else {
            log!("[Push] gateway unread total body was unreadable; badging this workspace's own count");
            return None;
        };
        return parse_unread_total(&body);
    }
    log!(
        "[Push] gateway on :{} unreachable; badging this workspace's own count",
        gateway_port
    );
    None
}

/// Pull the aggregate out of the gateway's `{"total": N}` answer. Pure, so the
/// wire contract is pinned without a live gateway.
fn parse_unread_total(body: &str) -> Option<i64> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("total")?
        .as_i64()
}

/// Build a `wake: true` push payload from a stored notification.
///
/// Mirrors `build_push_payload` field for field. The one wire difference is
/// the top-level `wake: true` flag, a sibling of `web_push` rather than a
/// field inside the notification object. The Chrome SW reads it to gate
/// `renotify` and `silent`.
///
/// The fitting measures THIS envelope, `wake` flag included. That flag makes
/// the wake larger than the push it mirrors. A body that just squeezed into
/// the original send would otherwise blow the wake up.
fn build_wake_payload(
    notification: &crate::scheduler::notifications::Notification,
    ios_scope_url: Option<&str>,
    app_badge: Option<i64>,
) -> serde_json::Value {
    fit_payload_body(&notification.message, "wake", |candidate_body| {
        let mut payload = build_push_payload(
            &notification.title,
            candidate_body,
            Some(notification.id),
            notification.app_id.as_deref(),
            notification.thread_id,
            notification.event_id,
            &notification.tap,
            ios_scope_url,
            app_badge,
        );
        payload["wake"] = serde_json::Value::Bool(true);
        payload
    })
}

/// [`build_push_payload`] with the body fitted to the transport ceiling.
///
/// Every send path builds through this. `build_push_payload` stays the pure
/// envelope builder underneath, so the wire shape is testable without the size
/// guard in the way. It is also what [`fit_payload_body`] re-renders.
#[allow(clippy::too_many_arguments)]
fn build_push_payload_fitted(
    title: &str,
    body: &str,
    notification_id: Option<uuid::Uuid>,
    app_id: Option<&str>,
    link_thread_id: Option<uuid::Uuid>,
    link_event_id: Option<uuid::Uuid>,
    tap: &crate::scheduler::notifications::Tap,
    ios_scope_url: Option<&str>,
    app_badge: Option<i64>,
) -> serde_json::Value {
    fit_payload_body(body, "notification", |candidate_body| {
        build_push_payload(
            title,
            candidate_body,
            notification_id,
            app_id,
            link_thread_id,
            link_event_id,
            tap,
            ios_scope_url,
            app_badge,
        )
    })
}

/// Build the JSON payload delivered to the push transport.
///
/// The wire shape is the Declarative Web Push envelope.
/// `system-knowhow/notifications.md` § Layer 1 writes it out field by field,
/// with each client's handling and one tested dead end nobody should retry.
///
/// **Two navigate URL forms, same params.** `notification.navigate` is what
/// iOS Safari reads, an absolute query URL when the subscription has a stored
/// scope. `notification.data.navigate` is what the Chrome SW reads, a hash
/// URL. See [`navigate_url_ios`] / [`navigate_url_sw`] for why they differ.
///
/// `tap` is ALWAYS encoded in the `data` block, because the page's dispatcher
/// routes on `tap.kind`. Modal-kind taps are left out of the `navigate` URL to
/// keep it short, and the page demotes a missing `tap` to modal.
#[allow(clippy::too_many_arguments)]
fn build_push_payload(
    title: &str,
    body: &str,
    notification_id: Option<uuid::Uuid>,
    app_id: Option<&str>,
    link_thread_id: Option<uuid::Uuid>,
    link_event_id: Option<uuid::Uuid>,
    tap: &crate::scheduler::notifications::Tap,
    ios_scope_url: Option<&str>,
    app_badge: Option<i64>,
) -> serde_json::Value {
    let params = build_navigate_params(notification_id, link_thread_id, link_event_id, tap);
    let navigate_ios = navigate_url_ios(&params, ios_scope_url);
    let navigate_sw = navigate_url_sw(&params);
    let tag = notification_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| DEFAULT_NOTIFICATION_TAG.to_string());

    let mut data = serde_json::Map::new();
    if let Some(id) = notification_id {
        data.insert(
            "notification_id".into(),
            serde_json::Value::String(id.to_string()),
        );
    }
    if let Some(aid) = app_id {
        data.insert("app_id".into(), serde_json::Value::String(aid.to_string()));
    }
    if let Some(tid) = link_thread_id {
        data.insert(
            "thread_id".into(),
            serde_json::Value::String(tid.to_string()),
        );
    }
    if let Some(eid) = link_event_id {
        data.insert(
            "event_id".into(),
            serde_json::Value::String(eid.to_string()),
        );
    }
    data.insert(
        "tap".into(),
        serde_json::to_value(tap).expect("Tap serializes infallibly"),
    );
    // HASH form, so the Chrome SW reads the engine-built URL straight off
    // `event.notification.data` for the cold `clients.openWindow()` path
    // instead of rebuilding it. Warm taps route via postMessage.
    data.insert("navigate".into(), serde_json::Value::String(navigate_sw));

    let mut payload = serde_json::json!({
        "web_push": DECLARATIVE_WEB_PUSH_MAGIC,
        "notification": {
            "title": title,
            "body": body,
            // QUERY form: iOS Safari's declarative-push navigation needs a
            // cross-document URL to navigate an open window.
            "navigate": navigate_ios,
            "tag": tag,
            "data": serde_json::Value::Object(data),
        },
    });
    // Top-level `app_badge`, a sibling of `web_push` rather than a field
    // inside the notification object. iOS Safari reads it in its parent
    // process and badges the installed PWA without running the service
    // worker. That is the only badge path for a closed iOS PWA. The Chrome SW
    // mirrors the same field. `0` clears the badge.
    //
    // WHICH count this is belongs to the caller, not the envelope: see
    // `app_badge_for`. Omitted when neither count could be read, and the badge
    // is then left untouched.
    if let Some(count) = app_badge {
        payload["app_badge"] = serde_json::json!(count.max(0));
    }
    payload
}

/// Dispatch the per-subscription send loop. `kind` appears in log lines.
/// Non-fatal: a per-subscription failure is logged and the loop continues.
/// Under `e2e-test-hooks` the network send becomes a `push_log` write, so
/// browser e2e tests assert delivery without waiting on APNs or FCM.
async fn fan_out_payload(
    pool: &PgPool,
    deliveries: Vec<(PushSubscription, String)>,
    kind: &str,
    notification_id: Option<uuid::Uuid>,
) -> usize {
    #[cfg(feature = "e2e-test-hooks")]
    {
        fan_out_to_push_log(pool, deliveries, kind, notification_id).await
    }
    #[cfg(not(feature = "e2e-test-hooks"))]
    {
        let _ = notification_id;
        fan_out_to_web_push(pool, deliveries, kind).await
    }
}

#[cfg(not(feature = "e2e-test-hooks"))]
async fn fan_out_to_web_push(
    pool: &PgPool,
    deliveries: Vec<(PushSubscription, String)>,
    kind: &str,
) -> usize {
    let keys = match get_or_create_vapid_keys(pool).await {
        Ok(k) => k,
        Err(e) => {
            log!("[Push] Failed to get VAPID keys for {}: {}", kind, e);
            return 0;
        }
    };

    let client = match web_push::IsahcWebPushClient::new() {
        Ok(c) => c,
        Err(e) => {
            log!("[Push] Failed to create push client for {}: {}", kind, e);
            return 0;
        }
    };

    let mut stale_endpoints: Vec<String> = Vec::new();
    let mut delivered = 0usize;

    for (sub, payload_bytes) in &deliveries {
        let endpoint_label = &sub.endpoint[..sub.endpoint.floor_char_boundary(60)];
        let sub_info = web_push::SubscriptionInfo::new(&sub.endpoint, &sub.p256dh, &sub.auth);

        let sig = match web_push::VapidSignatureBuilder::from_pem(
            keys.private_key_pem.as_bytes(),
            &sub_info,
        ) {
            Ok(mut builder) => {
                // FCM and Apple both require the VAPID `sub` claim, as a valid
                // mailto: or https: URL. Apple rejects localhost.
                builder.add_claim("sub", "mailto:push@lucidos.app");
                match builder.build() {
                    Ok(sig) => sig,
                    Err(e) => {
                        log!("[Push] Failed to build VAPID signature for {}: {}", kind, e);
                        continue;
                    }
                }
            }
            Err(e) => {
                log!("[Push] Failed to create VAPID builder for {}: {}", kind, e);
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
                // Genuine last resort. The builders already truncate the body
                // to fit `MAX_PUSH_PAYLOAD_BYTES`, so reaching here means
                // something the guard cannot shrink: an oversized title, bad
                // subscription keys, an unparseable endpoint. Say plainly that
                // this device gets NOTHING.
                log!(
                    "[Push] NO {} push delivered to {} ({} B payload): the encrypted message \
                     could not be built: {}. The notification row and bell badge are \
                     unaffected, so nothing else surfaces this failure.",
                    kind,
                    endpoint_label,
                    payload_bytes.len(),
                    e
                );
                continue;
            }
        };

        use web_push::WebPushClient;
        match client.send(message).await {
            Ok(_) => {
                delivered += 1;
                log!("[Push] Sent {} to {}", kind, endpoint_label);
            }
            Err(e) => {
                let err_str = e.to_string();
                // 410 Gone means the subscription is no longer valid.
                if err_str.contains("410") || err_str.contains("Gone") {
                    log!(
                        "[Push] Subscription expired (410), will remove: {}",
                        endpoint_label
                    );
                    stale_endpoints.push(sub.endpoint.clone());
                } else {
                    log!(
                        "[Push] Failed to send {} to {}: {}",
                        kind,
                        endpoint_label,
                        e
                    );
                }
            }
        }
    }

    for endpoint in stale_endpoints {
        if let Err(e) = PushSubscriptionStore::unsubscribe(pool, &endpoint).await {
            log!("[Push] Failed to remove stale subscription: {}", e);
        }
    }

    delivered
}

#[cfg(feature = "e2e-test-hooks")]
async fn fan_out_to_push_log(
    pool: &PgPool,
    deliveries: Vec<(PushSubscription, String)>,
    kind: &str,
    notification_id: Option<uuid::Uuid>,
) -> usize {
    let Some(nid) = notification_id else {
        // No notification_id to attribute the log row to, and a test cannot
        // assert on an untagged row.
        return 0;
    };
    let mut delivered = 0usize;
    for (sub, payload) in &deliveries {
        let endpoint_label = &sub.endpoint[..sub.endpoint.floor_char_boundary(60)];
        // A row without a device_id cannot be attributed, so the test log
        // holds only rows a test can assert against.
        let Some(device_id) = sub.device_id.as_deref() else {
            continue;
        };
        match push_test_log::record(pool, device_id, nid, payload).await {
            Ok(()) => {
                delivered += 1;
                log!("[Push:test] Logged {} to {}", kind, endpoint_label);
            }
            Err(e) => {
                log!(
                    "[Push:test] Failed to write push_log for {}: {}",
                    endpoint_label,
                    e
                );
            }
        }
    }
    delivered
}

#[cfg(test)]
#[path = "push_tests.rs"]
mod tests;
