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

/// Spec §3 — engine waits this long for pongs before deciding push.
///
/// Sized for an iOS PWA reaching the engine over cellular or a Tailscale
/// tunnel. Two RTT bands appear in real traces:
///
/// - **Steady-state cellular / Tailscale relay**: 400–800 ms round-trip
///   for the SSE-out + pong-POST cycle.
/// - **First-packet-after-radio-idle on Tailscale**: 1100–1800 ms.
///   Tailscale's userspace WireGuard has to renegotiate the path when the
///   phone radio resumes from idle, and the first packet through pays
///   that handshake cost. This band is the one that breaks "engine waits
///   then sends the OS push on top of the in-app toast" — the page renders
///   the toast synchronously on PresenceCheck receipt, but its pong
///   round-trips slower than the engine's deadline.
///
/// The previous value of 1000 ms covered the steady state but timed out
/// inside the wake-from-idle band. 2000 ms leaves ~200–400 ms headroom
/// over the observed worst-case wake RTT, so even a slow first-packet
/// pong lands before the deadline fires.
///
/// The wait only blocks fan-out when a candidate fails to pong; the
/// `notify_one` short-circuit in [`run_presence_check`] wakes immediately
/// once every expected device has answered, so increasing this only costs
/// latency when a `device_presence` row is stale (page killed without
/// firing `DeviceHidden`).
pub const DEADLINE_MS: u32 = 2000;

/// Spec §2 Step A — push_allowed iff no pong reports active.
pub fn decide_push_allowed(pongs: &[PresencePong]) -> bool {
    !pongs.iter().any(|p| p.is_active)
}

/// Spec §2 Step A / §3 — how many pongs the engine should wait for, and
/// (via `> 0`) whether to run the PresenceCheck at all.
///
/// `sse_connections` is the live count of open `GET /api/v1/events` streams —
/// the ground truth for "a page is connected and will pong". `candidate_count`
/// is the number of fresh `device_presence` heartbeat rows. We take the max:
///
/// - The SSE count is the robust signal. iOS suspends the 30s heartbeat while
///   a PWA is foregrounded, so a genuinely-active page's `device_presence` row
///   ages past the 120s window even though its EventSource is still open and
///   would pong `is_active`. Gating only on heartbeat freshness then skipped
///   the PresenceCheck and fired an OS push on top of the active page.
/// - The candidate count covers the inverse failure: a page that heartbeated
///   within the last 120s but whose SSE connection just dropped (network blip).
///   Counting it keeps the deadline short-circuit waiting for its pong too.
///
/// `0` means nobody is reachable (no open stream, no fresh heartbeat) → skip
/// the protocol and send the push directly (the "phone in your pocket" case).
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
    /// Defensive double-write — the migration owns this CREATE TABLE
    /// (see `20260517160627_consolidate_init_schema_tables.sql`). Slated
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

    /// Get push subscriptions filtered by device push_enabled setting. Each
    /// subscription is paired with the owning device's `user_agent` so the
    /// caller can gate engine-side scheduled wake-pushes (Layer 3 in
    /// `system-knowhow/notifications.md` §4.5) on `is_mac_chromium`. Legacy
    /// rows without a `device_id` and devices with no recorded UA both come
    /// back as `None` on the second tuple element.
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

    /// Get push subscriptions for a single device. Used by the wake-push path
    /// (`send_wake_push_to_device`, called from `schedule_mac_chromium_wakes`)
    /// — the engine fans out a follow-up push to one device only so other
    /// devices don't receive a duplicate notification just to unjam one SW.
    /// Returns empty when the device has push disabled OR has no subscription.
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

    log!("[Push] Generated new VAPID key pair");
    Ok(keys)
}

/// Send a push notification to all registered subscriptions.
/// Non-fatal: logs errors but doesn't fail.
/// If `notification_id` is provided, clicking the notification deep-links to it.
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
/// - `push_allowed = false` (an active device pong'd in) → emit
///   `NotificationToastRequested` so active pages show the in-app toast.
/// - `push_allowed = true` (no active device) → emit `NativePushRequested` so a
///   connected Tauri desktop app shows a native macOS banner, AND fan out the
///   web push to every browser / PWA subscription.
///
/// The two emits are mutually exclusive by construction (opposite branches of
/// one decision), so a device never gets both a toast and a push/native banner.
/// The decision runs whenever ANY client is reachable — a web-push
/// subscription OR an open SSE connection / fresh heartbeat — so a desktop-only
/// (Tauri) workspace with zero web-push subscriptions still gets toasts and
/// native banners. Non-fatal: every DB / SSE / web-push failure is logged and
/// execution continues so a single bad subscription doesn't sink the fan-out.
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
    // Web-push subscriptions (browser / PWA endpoints). MAY be empty — a
    // desktop-only (Tauri) workspace never creates one, because the embedded
    // WKWebView can't subscribe to Web Push. We deliberately do NOT bail on
    // empty here: a connected client still needs either the in-app toast
    // (push suppressed) or a native desktop banner (push allowed), and both
    // are decided below. The combined "nobody reachable" bail comes after we
    // know the connected-client count.
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
            // Spec §3 failure handling: if we can't see candidates, the
            // safer side is to send the push.
            log!(
                "[Push] Failed to query device_presence candidates (sending push anyway): {}",
                e
            );
            Vec::new()
        }
    };

    // §2 Step A / §3 — how many pongs to wait for, and (via `> 0`) whether to
    // run the PresenceCheck at all. The robust signal is the live
    // SSE-connection count: a page connected via SSE will pong even when its
    // device_presence heartbeat has gone stale, which iOS does whenever it
    // suspends the 30s timer on a foregrounded PWA. Gating only on heartbeat
    // freshness skipped the check and fired an OS push on top of the active
    // page. device_presence candidates cover the inverse failure mode (a
    // freshly-heartbeated page whose SSE just dropped), so we take the max.
    let sse_connections = engine.sse_connections.count();
    let expected_pongs = expected_pong_count(sse_connections, candidates.len());

    // Nothing to deliver to at all: no web-push subscription AND nobody
    // connected or recently heartbeating (no client to receive a toast or a
    // native banner over SSE either). Bail before the PresenceCheck. Note the
    // gate is now "no subs AND no reachable client", not the old "no subs" —
    // a connected client with no subscription anywhere in the workspace is a
    // valid recipient for the in-app toast / native push.
    if subs_with_ua.is_empty() && expected_pongs == 0 {
        return;
    }

    // Pick wake targets before the move below consumes the paired list.
    let wake_targets = pick_mac_chromium_wake_targets(&subs_with_ua);
    let subscriptions: Vec<PushSubscription> =
        subs_with_ua.into_iter().map(|(sub, _)| sub).collect();

    let push_allowed = if expected_pongs == 0 {
        // Nobody connected and no fresh heartbeat → nobody to pong → push
        // immediately (the common "phone in your pocket" case).
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
        // No notification_id to scope the pong by — can't run the
        // protocol, so send the push (legacy callers).
        true
    };

    if !push_allowed {
        // An active device pong'd in → the OS push is suppressed (§2 Step A).
        // Tell active pages to render the in-app toast INSTEAD (§4). This is
        // the only place the toast is triggered, and it is mutually exclusive
        // with the push fan-out below by construction: the toast event and the
        // push live on opposite branches of this `if`, so a device can never
        // receive both for the same notification. `notification_id` is always
        // Some here — `push_allowed` only goes false through the
        // `run_presence_check` arm above, which requires it.
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

    // Native desktop surface (§1, §4). Broadcast on the push-ALLOWED branch so
    // a connected Tauri desktop app renders a native macOS notification — it
    // can't receive the web push below (WKWebView has no service-worker push).
    // Browser / PWA clients ignore this frame; only a non-active Tauri client
    // acts on it. It rides the same branch as the web-push fan-out (and the
    // opposite branch from `emit_toast_requested`), so a device never gets both
    // a native banner and an in-app toast for one notification.
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

    // No web-push subscriptions (e.g. a desktop-only workspace) → the native
    // broadcast above is the whole OS surface; skip the web-push machinery.
    if subscriptions.is_empty() {
        return;
    }

    // Current workspace unread count → declarative `app_badge` (see
    // `build_push_payload`). Queried once for the whole fan-out (same value for
    // every subscription). Best-effort: a failure just omits the badge field.
    let unread_count = read_unread_count(pool).await;

    let deliveries: Vec<(PushSubscription, String)> = subscriptions
        .into_iter()
        .map(|sub| {
            let payload_bytes = build_push_payload(
                title,
                body,
                notification_id,
                app_id,
                link_thread_id,
                link_event_id,
                &tap,
                sub.scope_url.as_deref(),
                unread_count,
            )
            .to_string();
            (sub, payload_bytes)
        })
        .collect();

    fan_out_payload(pool, deliveries, "notification", notification_id).await;

    // Layer 3 of the macOS-Chromium wedge mitigation (see
    // `system-knowhow/notifications.md` §4.5). For every macOS-Chromium
    // device that just got the real push, schedule a wake-push to land
    // `MAC_CHROMIUM_WAKE_DELAY` later. The wake's only job is to be a
    // second push event arriving at the SW — that drains any queued
    // `notificationclick` (Chromium #370536109) regardless of whether
    // the user has returned to the Lucidos tab. This is the sole
    // recovery mechanism for the partial wedge today; if it misses, the
    // next genuine push to the device is what eventually drains the
    // queue.
    //
    // Gated on `notification_id` because `send_wake_push_to_device`
    // loads the persisted notification to build the wake payload — a
    // wake without an id has nothing to mirror. Empty `wake_targets`
    // is a no-op inside `schedule_mac_chromium_wakes`.
    if let Some(nid) = notification_id {
        schedule_mac_chromium_wakes(engine.clone(), wake_targets, nid, MAC_CHROMIUM_WAKE_DELAY);
    }
}

/// Delay between the original push and the engine-side follow-up wake-push
/// for macOS-Chromium devices (Layer 3 in
/// `system-knowhow/notifications.md` §4.5). Three seconds is short enough
/// that a click queued in the wedged SW drains before the user gives up on
/// the dead tap, and long enough that Chrome doesn't coalesce the two
/// pushes as a single event (which would defeat the point — the wake needs
/// to be a separate push dispatch to resurrect the SW worker).
const MAC_CHROMIUM_WAKE_DELAY: std::time::Duration = std::time::Duration::from_secs(3);

/// Spawn one delayed wake-push per device_id in `device_ids`. Fire-and-
/// forget: each spawned task sleeps `delay`, then calls
/// `send_wake_push_to_device` and logs the outcome. Failures are logged
/// only — the wake is best-effort and the original push already landed,
/// so a dropped wake leaves the user in the pre-Layer-3 state where the
/// next genuine push to the device eventually drains the queued click.
///
/// Survives engine shutdown probabilistically, not by design. The spawned
/// task holds an `Arc<LucidosEngine>` that keeps the engine struct alive,
/// but `tokio::spawn` returns a `JoinHandle` we drop — on tokio-runtime
/// drop, the task is aborted at its next `await` regardless of strong-
/// count. In practice the wake usually completes because graceful
/// shutdown (`graceful_shutdown(10s)` in `main.rs`) outlasts the 3 s
/// sleep, but a notification fired in the final ~3 s of a restart will
/// see its wake aborted — the next genuine push is what drains then.
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

/// Broadcast a PresenceCheck and wait up to `DEADLINE_MS` for every
/// candidate device to pong. Returns the `push_allowed` decision per §2
/// Step A — true iff no pong reports `is_active`. Drains the tracker
/// slot whether or not all pongs arrived.
///
/// The PresenceCheck carries no toast content — the in-app toast is now
/// triggered by [`emit_toast_requested`] AFTER this decision resolves, so
/// the toast and the OS push can never both fire (see notifications.md
/// §3-§4). `event_id` rides along only so the pong can report
/// `event_in_viewport`.
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
        // SSE broadcast failed — without pages knowing to pong we can't
        // make an informed decision. Drain the slot and send the push
        // (safer side, same as the candidates-query error branch).
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
/// render the in-app toast for a notification whose OS push was suppressed
/// (§2 Step A found an active device). Broadcast SSE — hidden pages ignore
/// it via the §4 row matrix, active pages render the toast (or auto-read on
/// Row 1). Non-fatal: a failed emit is logged; the worst case is the user
/// sees the bell badge bump (driven by `NotificationCreated`) without the
/// transient toast. See `system-knowhow/notifications.md` §4.
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
/// desktop app renders a NATIVE macOS notification for a notification whose OS
/// push was allowed (§2 Step A found no active device). Broadcast SSE — browser
/// / PWA pages ignore it (they receive the real web push), and a Tauri page
/// shows the banner only when it is not currently active (§4 row matrix). This
/// is the desktop counterpart of the web-push fan-out: the WKWebView the Tauri
/// app embeds can't subscribe to Web Push, so the engine reaches it over the
/// already-open SSE stream instead. Non-fatal: a failed emit is logged; the
/// worst case is the desktop user sees only the bell badge (driven by
/// `NotificationCreated`). See `system-knowhow/notifications.md` §1, §4.
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
/// desktop app REMOVES the already-delivered native macOS banner(s) for a
/// notification that was just read (on this or another device). `notification_id
/// = Some(id)` removes one banner; `None` removes all (the mark-all-read path).
/// Broadcast SSE — browser / PWA pages ignore it (the open web can't silently
/// remove a Web Push banner; Safari revokes a subscription after 3 silent
/// pushes), so this is the macOS-desktop-only half of cross-device dismiss. The
/// desktop app stays SSE-connected, so it acts on this deterministically.
/// Non-fatal: a failed emit is logged; the worst case is a stale OS banner the
/// user swipes away manually (the prior, pre-dismiss behaviour). See
/// `system-knowhow/notifications.md` §4 and
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

/// Decides whether a device with the given `user_agent` is affected by
/// Chromium #370536109 — the macOS-Chromium dispatcher bug where
/// `notificationclick` is silently queued until a new push event drains
/// the SW. Used to gate the engine-side scheduled follow-up wake-push
/// (Layer 3 in `system-knowhow/notifications.md` §4.5) so Safari / iOS /
/// non-macOS Chromium / Firefox don't pay an extra push per notification.
///
/// `user_agent` is captured at `POST /api/v1/devices/register` time (see
/// `api/settings.rs::register_device` → `DeviceStore::register`) and stored
/// on the `devices` row, so the engine can decide purely from database
/// state — no per-push round trip to the page.
fn is_mac_chromium(user_agent: &str) -> bool {
    user_agent.contains("Macintosh")
        && user_agent.contains("Chrome/")
        && !user_agent.contains("iPhone")
        && !user_agent.contains("iPad")
        && !user_agent.contains("iPod")
}

/// Pure: from the just-pushed subscriptions paired with each device's
/// `user_agent`, return the deduplicated device_ids that should receive a
/// follow-up wake-push. Used by the engine-side scheduling layer to decide
/// which devices need the Layer 3 wake (see `is_mac_chromium` above for the
/// gate and `system-knowhow/notifications.md` §4.5 for the full mitigation
/// stack).
///
/// Dedup matters: two browser tabs on the same device produce two
/// subscriptions sharing the same `device_id`. `send_wake_push_to_device`
/// already fans out to every subscription on that device, so spawning two
/// delayed wake tasks would deliver the wake-push twice per SW. Once is
/// enough to drain the queued `notificationclick`.
///
/// Subscriptions with `device_id = None` (legacy rows) and devices with no
/// recorded `user_agent` are conservatively skipped — both signal "we can't
/// confidently target this device". The wake is the only recovery mechanism
/// today, so a skipped device falls back to "next genuine push drains the
/// queue" — same as a wake-task aborted at engine shutdown.
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
/// A wake push exists for one reason: resurrect a wedged service worker
/// (Chromium #370536109) so a queued `notificationclick` drains. A notification
/// that is already `read` proves the user's tap on the original banner already
/// landed — the SW was NOT wedged — so the wake has nothing to drain and would
/// only re-pop an already-handled notification as a fresh unread banner (macOS
/// won't replace a banner the tap already closed; it stacks a new one). The
/// read flag is thus a precise proxy for "was the SW wedged": read ⇒ tap
/// succeeded ⇒ skip; unread ⇒ either the user hasn't tapped yet or the tap was
/// swallowed by a wedged SW ⇒ the wake is still the right thing.
///
/// Pure so the decision is unit-testable without a DB; the caller re-fetches
/// the live read state at fire time (the tap lands DURING the wake delay, so
/// this cannot be decided when the wake is scheduled). See
/// `system-knowhow/notifications.md` §4.5.
fn wake_still_needed(notification: &crate::scheduler::notifications::Notification) -> bool {
    !notification.read
}

/// Send a wake-push to a single device — the workaround for Chromium
/// #370536109 (`notificationclick` silently queued on macOS-Chrome). Looks up
/// the notification by id, skips if it was read in the meantime (see
/// [`wake_still_needed`]), builds a `wake: true` payload carrying the SAME
/// content (so Chrome counts the push as visible, see §4.5), filters
/// subscriptions to the requesting device, and fans out. Returns the number
/// of subscriptions actually delivered to (0 when skipped).
///
/// Per web.dev `push-notifications-common-issues`, sending a push is the
/// canonical mechanism to wake an inactive SW — any push event resurrects
/// the worker thread and drains queued `notificationclick` events as a side
/// effect. The trigger is `schedule_mac_chromium_wakes` further down in this
/// file: every real push to a macOS-Chrome subscription spawns a delayed
/// `send_wake_push_to_device` MAC_CHROMIUM_WAKE_DELAY later so the queued
/// click drains without the user having to come back to the tab.
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

    // Re-checked at fire time: if the user tapped the original banner during the
    // wake delay, the notification is now read and the wake would only resurrect
    // it as a fresh banner. See `wake_still_needed`.
    if !wake_still_needed(&notification) {
        return Ok(0);
    }

    let subscriptions = PushSubscriptionStore::get_push_enabled_for_device(pool, device_id).await?;
    if subscriptions.is_empty() {
        return Ok(0);
    }

    let unread_count = read_unread_count(pool).await;
    let deliveries: Vec<(PushSubscription, String)> = subscriptions
        .into_iter()
        .map(|sub| {
            let payload_bytes =
                build_wake_payload(&notification, sub.scope_url.as_deref(), unread_count)
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

/// Build the `key=value&…` deep-link param string shared by both navigate URL
/// forms (see [`navigate_url_ios`] / [`navigate_url_sw`]). Empty when there's
/// nothing to deep-link, so the callers can fall back to a bare `/`.
///
/// The iOS declarative `navigate` field is built from the subscription's stored
/// scope URL when available, so the engine does not have to guess the gateway
/// workspace prefix from an APNs/FCM endpoint.
///
/// The `tap` JSON is only emitted for non-modal kinds — modal-kind URLs stay
/// short, and the page safely demotes a missing `tap` param to the modal
/// default.
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
/// NOT applied to that open window — WebKit just focuses it, the URL never
/// changes, and the page-side hash router finds nothing to route (the "tap
/// nav to thread only focuses the app" bug). A query string changes the
/// document, so iOS performs a real navigation the page picks up on load /
/// resume via `parseDeepLinkFromUrl` (which reads query params). See
/// `system-knowhow/notifications.md` §4.5.
///
/// TEMPORARY MEASURE (`docs/temporary-measures.md` § "Cross-document
/// notification-tap reload on iOS"). Choosing a cross-document URL is what makes
/// every iOS tap reload the PWA. It is the only channel WebKit actually applies
/// today, not a preference: `launchQueue` / `launch_handler: focus-existing` are
/// unimplemented and a same-document navigate is ignored. When WebKit ships a
/// reload-free channel this function changes shape and the page-side `?notification=`
/// DETECTION in `crates/lucidos-app/index.html` comes out with it. The quiet boot
/// cover that detection turns on does NOT: it also serves a user-requested
/// refresh, which no upstream fix affects.
///
/// `scope_url` is the concrete service-worker scope captured by the page at
/// subscription time, e.g. `https://host/dev/`. When it is present we emit an
/// absolute URL (`https://host/dev/?notification=...`) instead of relying on
/// WebKit/APNs to resolve a query-only relative value. Existing legacy rows that
/// lack `scope_url` keep the relative fallback until the next page load refreshes
/// their subscription.
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

/// Chrome service-worker navigate URL — a **hash** URL, read off
/// `notification.data.navigate` by the `notificationclick` handler. It feeds
/// only the COLD `clients.openWindow()` path (no Lucidos tab open): the
/// freshly-opened page's cold-start `handleHashLocation` reads the deep-link
/// params off the hash. A warm, already-open tab is routed by `postMessage`
/// instead of by this URL — Chrome doesn't fire `hashchange` for a fragment-
/// only `client.navigate()`, so the SW posts the structured deep link straight
/// to the page (see `routeToDeepLink` in `sw.js` and
/// `system-knowhow/notifications.md` §4.5). The query-vs-hash split with
/// [`navigate_url_ios`] is kept: iOS needs a cross-document (query) URL.
///
/// This remains **scope-relative** (no leading slash) so it resolves inside the
/// gateway `/<slug>/` scope. The Chrome SW always passes it through
/// `resolveNavigate` in `sw.js` (which resolves against `origin + SCOPE_PATH`).
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

/// Build a `wake: true` push payload from a stored notification. Mirrors
/// `build_push_payload` field-for-field so the SW path is symmetric — the
/// only wire difference is the added top-level `wake: true` flag (sibling to
/// `web_push` / `notification`, NOT inside the notification object) so Safari
/// ignores it while the SW reads it to gate `renotify` / `silent`. Safari
/// never sees wake pushes (filtered by `is_mac_chromium`); the flag is
/// purely for Chrome's SW.
fn build_wake_payload(
    notification: &crate::scheduler::notifications::Notification,
    ios_scope_url: Option<&str>,
    unread_count: Option<i64>,
) -> serde_json::Value {
    let mut payload = build_push_payload(
        &notification.title,
        &notification.message,
        Some(notification.id),
        notification.app_id.as_deref(),
        notification.thread_id,
        notification.event_id,
        &notification.tap,
        ios_scope_url,
        unread_count,
    );
    payload["wake"] = serde_json::Value::Bool(true);
    payload
}

/// Build the JSON payload delivered to the push transport.
///
/// Wire shape is the Declarative Web Push envelope (W3C Push API
/// "Declarative Web Push", merged Aug 2025; WebKit blog
/// `meet-declarative-web-push`):
///
/// ```json
/// {
///   "web_push": 8030,
///   "notification": {
///     "title": "...",
///     "body": "...",
///     "navigate": "https://host/<slug>/?notification=...&thread=...&event=...&tap=...",
///     "tag": "<notification_id or lucidos-notification>",
///     "data": {
///       "notification_id": "...",
///       "thread_id": "...",
///       "event_id": "...",
///       "app_id": "...",
///       "tap": { "kind": "modal" | "navigate", ... },
///       "navigate": "#notification=...  (HASH form, for the Chrome SW notificationclick path)"
///     }
///   }
/// }
/// ```
///
/// **Two navigate URL forms, same params.** `notification.navigate` (consumed
/// by iOS Safari) is an **absolute query** URL when the subscription has a
/// stored scope; `notification.data.navigate` (consumed by the Chrome SW) is a
/// **hash** URL. They carry identical deep-link params — see
/// [`navigate_url_ios`] / [`navigate_url_sw`] for why.
///
/// **iOS Safari 18.5+** sees the `web_push: 8030` magic. NOTE: the SW `push`
/// handler still FIRES on iOS (confirmed via the `[Client/sw] push` breadcrumb)
/// — the earlier claim that iOS "bypasses the SW entirely" was wrong, and the
/// SW's `showNotification` is what renders the visible banner on iOS. (We tested
/// skipping `showNotification` on iOS so the OS would render the declarative
/// notification natively + dodge the `notificationclick` deep-link bug — it
/// showed NO banner at all, because iOS only uses the declarative fallback when
/// the SW handler errors/times out, not when it cleanly resolves without showing.
/// Reverted; the SW always displays. See notifications.md §4.5 seventeenth
/// iteration — do not retry.) On tap the OS navigates the existing top-level
/// traversable to `notification.navigate`. We build that URL from the stored
/// service-worker scope (`https://host/<slug>/`) so the gateway prefix is
/// preserved and WebKit does not have to accept a query-only relative URL. It
/// MUST be a cross-document (query) URL: a same-document (hash-only) navigation
/// is not applied to an already-open PWA window — WebKit just focuses it — so a
/// hash URL silently no-ops the deep link. The page's `handleHashLocation` →
/// `dispatchDeepLink` chain reads the query params on load/resume and marks-read
/// + routes.
///
/// **Chrome / Firefox** don't recognize the magic, so the SW `push` handler
/// fires as usual, reads `data.notification.*` to populate `showNotification`,
/// and the `notificationclick` path runs on tap: an already-open tab is routed
/// by `postMessage` (the structured deep link straight to the page), and the
/// hash `data.navigate` URL feeds only the cold `clients.openWindow()` fallback
/// (no tab open). See `system-knowhow/notifications.md` §4.5.
///
/// `tap` is ALWAYS encoded as part of the `data` block — the page's tap
/// dispatcher routes on `tap.kind` (`modal` opens the inbox, `navigate`
/// deep-links). Modal-kind taps are omitted from the `navigate` URL params to
/// keep them short (the page demotes missing `tap` to modal).
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
    unread_count: Option<i64>,
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
    // HASH form inside `data` so the Chrome SW `notificationclick` handler can
    // read the engine-built URL straight off `event.notification.data` for the
    // cold `clients.openWindow()` path (no tab open) instead of rebuilding it.
    // Warm taps route via postMessage, not this URL — see `navigate_url_sw`.
    data.insert("navigate".into(), serde_json::Value::String(navigate_sw));

    let mut payload = serde_json::json!({
        "web_push": DECLARATIVE_WEB_PUSH_MAGIC,
        "notification": {
            "title": title,
            "body": body,
            // QUERY form — iOS Safari's declarative-push parent-process
            // navigation needs a cross-document URL to navigate an open window.
            "navigate": navigate_ios,
            "tag": tag,
            "data": serde_json::Value::Object(data),
        },
    });
    // Declarative Web Push top-level `app_badge` (sibling of `web_push` /
    // `notification`, NOT inside the notification object): iOS Safari reads it
    // in its parent process and sets the installed PWA's home-screen badge
    // WITHOUT running the service worker — the ONLY badge path for a CLOSED iOS
    // PWA, since iOS may bypass the SW for declarative pushes (see the §1002
    // note above). `0` clears the badge. The workspace engine's own unread
    // count, so a per-workspace PWA badges its own workspace. Omitted only when
    // the count couldn't be read (the badge then simply isn't touched).
    if let Some(count) = unread_count {
        payload["app_badge"] = serde_json::json!(count.max(0));
    }
    payload
}

/// Dispatch the per-subscription send loop. `kind` appears in log lines for
/// grepping. Non-fatal — per-subscription failures are logged and the loop
/// continues. Under `e2e-test-hooks` the network send is replaced with a
/// write to `push_log` so browser e2e tests can assert delivery without
/// waiting on APNs/FCM.
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
                // VAPID `sub` claim is required by FCM (Chrome) and Apple push
                // services. Must be a valid mailto: or https: URL — Apple
                // rejects localhost.
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
                log!("[Push] Failed to build {} message: {}", kind, e);
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
                // 410 Gone means the subscription is no longer valid
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
        // No notification_id to attribute the log row to — no caller does
        // this today, and tests can't assert on an untagged row anyway.
        return 0;
    };
    let mut delivered = 0usize;
    for (sub, payload) in &deliveries {
        let endpoint_label = &sub.endpoint[..sub.endpoint.floor_char_boundary(60)];
        // Legacy rows without a device_id can't be attributed; skip so the
        // test log only contains rows tests will actually assert against.
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
