use serde::{Deserialize, Serialize};
use sqlx::PgPool;

#[cfg(feature = "e2e-test-hooks")]
use super::push_test_log;

use super::notification_plain_text::plain_text_body;
use crate::core::secret_store::{PushAuth, Secret, VapidPrivateKey};
use crate::engine::types::SharedEngine;

/// INVARIANT: Spec §3: the engine waits this long for pongs before deciding push.
///
/// Sized for the slowest real band, a first packet after the phone radio
/// resumes from idle over a tunnel (1100-1800 ms observed). A deadline inside
/// that band fires an OS push on top of a toast the page already rendered.
///
/// The wait only blocks fan-out when a candidate fails to pong. The short
/// circuit in [`run_presence_check`] wakes at the first pong that suppresses
/// the push, or once every expected device has answered. So a longer deadline
/// costs latency only when a `device_presence` row is stale.
pub const DEADLINE_MS: u32 = 2000;

/// INVARIANT: Spec §2 Step A / §3: how many pongs to wait for, and (via `> 0`) whether to
/// run the PresenceCheck at all. The max of the two signals, because each
/// covers the other's failure:
///
/// - Open SSE streams are the robust signal. iOS suspends the heartbeat on a
///   foregrounded PWA, so an active page's `device_presence` row goes stale
///   while its EventSource stays open and would pong `shown`.
/// - Fresh heartbeat rows cover the inverse: a page that heartbeated recently
///   but whose SSE connection just dropped.
///
/// `0` means nobody is reachable, so skip the protocol and push directly.
fn expected_pong_count(sse_connections: usize, candidate_count: usize) -> usize {
    sse_connections.max(candidate_count)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushSubscription {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub device_id: Option<String>,
    pub scope_url: Option<String>,
}

const PUSH_ENABLED_FROM: &str = "FROM push_subscriptions ps
     LEFT JOIN devices d ON ps.device_id = d.id
     WHERE ps.device_id IS NULL OR d.push_enabled = true";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VapidKeys {
    pub private_key_pem: String,
    pub public_key: String,
}

pub struct PushSubscriptionStore;

impl PushSubscriptionStore {
    pub async fn subscribe(
        pool: &PgPool,
        sub: &PushSubscription,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
        .bind(Secret::<PushAuth>::new(sub.auth.as_str()))
        .bind(&sub.device_id)
        .bind(&sub.scope_url)
        .execute(pool)
        .await?;
        Ok(())
    }

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

    pub async fn get_push_enabled(
        pool: &PgPool,
    ) -> Result<Vec<(PushSubscription, Option<String>)>, Box<dyn std::error::Error + Send + Sync>>
    {
        let rows = sqlx::query_as::<
            _,
            (
                String,
                String,
                Secret<PushAuth>,
                Option<String>,
                Option<String>,
                Option<String>,
            ),
        >(&format!(
            "SELECT ps.endpoint, ps.p256dh, ps.auth, ps.device_id, ps.scope_url, d.user_agent \
             {PUSH_ENABLED_FROM}"
        ))
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
                            auth: auth.into_inner(),
                            device_id,
                            scope_url,
                        },
                        user_agent,
                    )
                },
            )
            .collect())
    }

    pub async fn get_push_enabled_for_device(
        pool: &PgPool,
        device_id: &str,
    ) -> Result<Vec<PushSubscription>, Box<dyn std::error::Error + Send + Sync>> {
        let rows = sqlx::query_as::<
            _,
            (
                String,
                String,
                Secret<PushAuth>,
                Option<String>,
                Option<String>,
            ),
        >(
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
                    auth: auth.into_inner(),
                    device_id,
                    scope_url,
                },
            )
            .collect())
    }
}

pub async fn get_or_create_vapid_keys(
    pool: &PgPool,
) -> Result<VapidKeys, Box<dyn std::error::Error + Send + Sync>> {
    use base64::Engine;
    use p256::ecdsa::SigningKey;
    use p256::elliptic_curve::rand_core::OsRng;
    use p256::pkcs8::EncodePrivateKey;

    if let Some(keys) = stored_vapid_keys(pool).await? {
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

    // INVARIANT: A concurrent first use may have stored its pair already. Its pair wins,
    // so every subscription keeps one signer.
    let created = sqlx::query(
        "INSERT INTO vapid_keys (public_key, private_key_pem) VALUES ($1, $2) \
         ON CONFLICT (singleton) DO NOTHING",
    )
    .bind(&public_key)
    .bind(Secret::<VapidPrivateKey>::new(private_key_pem))
    .execute(pool)
    .await?
    .rows_affected();
    if created > 0 {
        log_legacy!("[Push] Generated new VAPID key pair");
    }
    stored_vapid_keys(pool)
        .await?
        .ok_or_else(|| "the VAPID key pair was not stored".into())
}

async fn stored_vapid_keys(
    pool: &PgPool,
) -> Result<Option<VapidKeys>, Box<dyn std::error::Error + Send + Sync>> {
    let row: Option<(String, Secret<VapidPrivateKey>)> =
        sqlx::query_as("SELECT public_key, private_key_pem FROM vapid_keys")
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(public_key, private_key_pem)| VapidKeys {
        private_key_pem: private_key_pem.into_inner(),
        public_key,
    }))
}

pub fn send_push_to_all(
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
    );
}

/// INVARIANT: Start [`fan_out_os_surface`] on its own task and return at once.
///
/// **Not async, on purpose.** The fan-out waits on the network, and its
/// callers must never wait with it. Among them are the EventBus subscriber
/// loop, the backup loop and a trigger's Thread Queue slot. Each of those
/// stalls everything behind it while it waits.
///
/// The task carries the caller's event-trigger chain depth, because a
/// task-local does not follow a `tokio::spawn`. Without it the events the
/// fan-out emits would read depth 0 and could restart a chain the cap stopped.
#[allow(clippy::too_many_arguments)]
pub fn send_push_to_all_with_app(
    engine: &SharedEngine,
    title: &str,
    body: &str,
    notification_id: Option<uuid::Uuid>,
    app_id: Option<&str>,
    link_thread_id: Option<uuid::Uuid>,
    link_event_id: Option<uuid::Uuid>,
    tap: crate::scheduler::notifications::Tap,
) {
    let engine = engine.clone();
    let title = title.to_string();
    let body = body.to_string();
    let app_id = app_id.map(str::to_string);
    spawn_carrying_chain_depth(async move {
        if let Err(error) = fan_out_os_surface(
            &engine,
            &title,
            &body,
            notification_id,
            app_id.as_deref(),
            link_thread_id,
            link_event_id,
            tap,
            false,
        )
        .await
        {
            log_legacy!("[Push] Fan-out failed: {}", error);
        }
    });
}

/// INVARIANT: Await the complete push fan-out for a durable users notice. The worker
/// records completion only after this boundary, never after spawning a task.
pub(crate) async fn send_user_only_push(
    engine: &SharedEngine,
    title: &str,
    body: &str,
    notification_id: uuid::Uuid,
    thread_id: uuid::Uuid,
    event_id: Option<uuid::Uuid>,
    tap: crate::scheduler::notifications::Tap,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    fan_out_os_surface(
        engine,
        title,
        body,
        Some(notification_id),
        None,
        Some(thread_id),
        event_id,
        tap,
        true,
    )
    .await
}

fn spawn_carrying_chain_depth(work: impl std::future::Future<Output = ()> + Send + 'static) {
    let depth = crate::scheduler::user_tasks::current_event_trigger_depth();
    tokio::spawn(crate::scheduler::user_tasks::EVENT_TRIGGER_DEPTH.scope(depth, work));
}

/// INVARIANT: Fan out the OS surface for a notification per the §2 matrix in
/// `system-knowhow/notifications.md`. The PresenceCheck pong protocol (§3) is
/// the authoritative decision input:
/// - `push_allowed = false`: a page already handled the notification in the
///   app (toast, auto-read or toasts off) and said so. Nothing more to send.
/// - `push_allowed = true`: emit `NativePushRequested` for a connected Tauri
///   desktop app, and fan the web push out to every browser subscription.
///
/// The decision runs whenever ANY client is reachable, so a desktop-only
/// workspace with no web-push subscription still gets toasts and native
/// banners.
///
/// Attempt every subscription, then report incomplete delivery for retry.
#[allow(clippy::too_many_arguments)]
async fn fan_out_os_surface(
    engine: &SharedEngine,
    title: &str,
    body: &str,
    notification_id: Option<uuid::Uuid>,
    app_id: Option<&str>,
    link_thread_id: Option<uuid::Uuid>,
    link_event_id: Option<uuid::Uuid>,
    tap: crate::scheduler::notifications::Tap,
    durable: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let body = &plain_text_body(body);
    let pool = engine.pool();
    // INVARIANT: Capture before presence waits and payload serialization. An unreadable
    // boundary cannot authorize sending work from an unknown era.
    let source_generation = match crate::core::snapshot::boundary::read(pool).await {
        Ok(boundary) => boundary.map(|b| b.restore_id),
        Err(error) => {
            log_legacy!("[Push] Cannot read source generation: {}", error);
            return Err(error.into());
        }
    };
    // INVARIANT: MAY be empty: a desktop-only workspace never creates one, because the
    // embedded WKWebView cannot subscribe to Web Push. Do NOT bail on empty
    // here. A connected client still needs the toast or the native banner, and
    // the "nobody reachable" bail comes once the client count is known.
    let mut audience = match push_audience(pool, notification_id, &tap, link_event_id).await {
        Ok(audience) => audience,
        Err(error) if durable => return Err(error.into()),
        Err(error) => {
            log_legacy!("[Push] Cannot read audience (pushing team): {}", error);
            crate::engine::thread_users::Audience::Team
        }
    };
    if durable {
        if let (Some(id), Some(devices)) = (notification_id, audience.device_ids()) {
            let snapshot = crate::gateway_users::current();
            let mut unread = std::collections::BTreeSet::new();
            for device in devices {
                let actor = crate::engine::thread_events::MessageOrigin::Device {
                    device_id: device.clone(),
                };
                let reader = crate::gateway_users::Reader::of(Some(&actor), &snapshot);
                if !crate::scheduler::NotificationStore::is_read(pool, id, &reader).await? {
                    unread.insert(device);
                }
            }
            if unread.is_empty() {
                return Ok(());
            }
            audience = crate::engine::thread_users::Audience::Devices(unread);
        }
    }
    let push_group = read_push_group(pool, notification_id).await;
    let mut subs_with_ua: Vec<_> = match PushSubscriptionStore::get_push_enabled(pool).await {
        Ok(subs) => subs
            .into_iter()
            .filter(|(sub, _)| audience.reaches(sub.device_id.as_deref()))
            .collect(),
        Err(e) => {
            log_legacy!("[Push] Failed to fetch subscriptions: {}", e);
            return Err(e);
        }
    };

    let candidates = match crate::core::DevicePresenceStore::candidates(pool).await {
        Ok(c) => c,
        Err(e) => {
            log_legacy!(
                "[Push] Failed to query device_presence candidates (sending push anyway): {}",
                e
            );
            Vec::new()
        }
    };

    let sse_connections = engine.sse_connections.count();
    let expected_pongs = expected_pong_count(sse_connections, candidates.len());

    if !durable && subs_with_ua.is_empty() && expected_pongs == 0 {
        return Ok(());
    }

    if durable {
        if let Some(id) = notification_id {
            let mut pending = Vec::new();
            for entry in subs_with_ua {
                if !push_receipt_exists(pool, id, &entry.0.endpoint).await? {
                    pending.push(entry);
                }
            }
            subs_with_ua = pending;
        }
    }

    let wake_targets = pick_mac_chromium_wake_targets(&subs_with_ua);
    let subscriptions: Vec<PushSubscription> =
        subs_with_ua.into_iter().map(|(sub, _)| sub).collect();

    let prior_decision: Option<bool> = if durable {
        sqlx::query_scalar(
            "SELECT push_allowed FROM user_only_message_dispatches WHERE notification_id = $1",
        )
        .bind(notification_id)
        .fetch_one(pool)
        .await?
    } else {
        None
    };

    let push_allowed = if let Some(allowed) = prior_decision {
        allowed
    } else if expected_pongs == 0 {
        true
    } else if let Some(nid) = notification_id {
        run_presence_check(
            &engine.event_bus,
            &engine.presence_tracker,
            ToastContent {
                notification_id: nid,
                title,
                body,
                thread_id: link_thread_id,
                event_id: link_event_id,
                app_id,
                tap: tap.clone(),
                audience: audience.device_ids(),
            },
            expected_pongs,
        )
        .await
    } else {
        true
    };

    if durable && prior_decision.is_none() {
        sqlx::query(
            "UPDATE user_only_message_dispatches SET push_allowed = $2 WHERE notification_id = $1",
        )
        .bind(notification_id)
        .bind(push_allowed)
        .execute(pool)
        .await?;
    }

    if !push_allowed {
        log_legacy!("[Push] Suppressed OS push (PresenceCheck: a page handled it in the app)");
        return Ok(());
    }

    // INVARIANT: Native desktop surface (§1, §4). A connected Tauri app cannot receive
    // the web push below, because its WKWebView has no service-worker push.
    // Browser clients ignore this frame. A Tauri page shows the banner, or
    // the in-app toast if its window came forward since it answered.
    let mut native_pending = false;
    if let Some(nid) = notification_id {
        let native_audience = if durable {
            let mut pending = Vec::new();
            for device in crate::core::DeviceStore::native_push_devices(pool).await? {
                if audience.reaches(Some(&device))
                    && !push_receipt_exists(pool, nid, &format!("native:{device}")).await?
                {
                    pending.push(device);
                }
            }
            native_pending = !pending.is_empty();
            Some(pending)
        } else {
            audience.device_ids()
        };
        if !durable || native_pending {
            if let Err(error) = emit_native_push_requested(
                &engine.event_bus,
                nid,
                title,
                body,
                link_thread_id,
                link_event_id,
                app_id,
                tap.clone(),
                native_audience,
            )
            .await
            {
                log_legacy!("[Push] Native broadcast failed: {}", error);
            }
        }
    }
    if subscriptions.is_empty() {
        return if native_pending {
            Err("Native delivery awaits client receipt".into())
        } else {
            Ok(())
        };
    }

    let unread_counts = unread_counts_by_device(pool, &subscriptions).await;
    let cross_workspace_totals = cross_workspace_unread_totals(&subscriptions).await;

    let deliveries = badge_deliveries(
        subscriptions,
        &unread_counts,
        &cross_workspace_totals,
        |sub, badge| {
            build_push_payload_fitted(
                source_generation,
                title,
                body,
                notification_id,
                app_id,
                link_thread_id,
                link_event_id,
                &tap,
                sub.scope_url.as_deref(),
                badge,
                push_group.as_ref(),
            )
        },
    );

    let mut web_failure = None;
    let mut wake_endpoints = durable.then(std::collections::BTreeSet::new);
    if durable {
        let nid = notification_id.ok_or("Durable push needs a notification id")?;
        let outcomes =
            futures::future::join_all(deliveries.into_iter().map(|(sub, payload)| async move {
                if push_receipt_exists(pool, nid, &sub.endpoint).await? {
                    return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(None);
                }
                let target = sub.endpoint.clone();
                let delivered =
                    fan_out_payload(pool, vec![(sub, payload)], "notification", Some(nid)).await;
                let remains: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM push_subscriptions WHERE endpoint = $1)",
                )
                .bind(&target)
                .fetch_one(pool)
                .await?;
                if delivered == 0 && remains {
                    return Err("Push subscription delivery failed".into());
                }
                record_push_receipt(pool, nid, &target).await?;
                Ok((delivered > 0).then_some(target))
            }))
            .await;
        for outcome in outcomes {
            match outcome {
                Ok(Some(target)) => {
                    if let Some(endpoints) = &mut wake_endpoints {
                        endpoints.insert(target);
                    }
                }
                Err(error) => web_failure = Some(error),
                _ => {}
            }
        }
    } else {
        fan_out_payload(pool, deliveries, "notification", notification_id).await;
    }

    if let Some(nid) = notification_id {
        schedule_mac_chromium_wakes(
            engine.clone(),
            source_generation,
            wake_targets,
            nid,
            wake_endpoints,
            MAC_CHROMIUM_WAKE_DELAY,
        );
    }
    if let Some(error) = web_failure {
        return Err(error);
    }
    if native_pending {
        return Err("Native delivery awaits client receipt".into());
    }
    Ok(())
}

async fn push_receipt_exists(
    pool: &PgPool,
    id: uuid::Uuid,
    target: &str,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM user_only_message_push_receipts WHERE notification_id = $1 AND target = $2)")
        .bind(id).bind(target).fetch_one(pool).await?)
}

pub async fn record_push_receipt(
    pool: &PgPool,
    id: uuid::Uuid,
    target: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query("INSERT INTO user_only_message_push_receipts (notification_id, target) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(id).bind(target).execute(pool).await?;
    Ok(())
}

const MAC_CHROMIUM_WAKE_DELAY: std::time::Duration = std::time::Duration::from_secs(3);

fn schedule_mac_chromium_wakes(
    engine: SharedEngine,
    source_generation: Option<uuid::Uuid>,
    device_ids: Vec<String>,
    notification_id: uuid::Uuid,
    endpoints: Option<std::collections::BTreeSet<String>>,
    delay: std::time::Duration,
) {
    for device_id in device_ids {
        let engine = engine.clone();
        let endpoints = endpoints.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            match send_wake_push_to_device(
                &engine,
                source_generation,
                &device_id,
                notification_id,
                endpoints.as_ref(),
            )
            .await
            {
                Ok(0) => log_legacy!(
                    "[Push] Layer-3 wake-push for {} on {}: nothing sent \
                     (notification read during the delay window, or device \
                     unsubscribed)",
                    notification_id,
                    device_id
                ),
                Ok(n) => log_legacy!(
                    "[Push] Layer-3 wake-push for {} delivered to {} sub(s) on {}",
                    notification_id,
                    n,
                    device_id
                ),
                Err(e) => log_legacy!(
                    "[Push] Layer-3 wake-push for {} on {} failed: {}",
                    notification_id,
                    device_id,
                    e
                ),
            }
        });
    }
}

async fn read_push_group(
    pool: &PgPool,
    notification_id: Option<uuid::Uuid>,
) -> Option<crate::scheduler::notifications::PushGroup> {
    let id = notification_id?;
    crate::scheduler::NotificationStore::push_group(pool, id)
        .await
        .unwrap_or_else(|e| {
            log_legacy!("[Push] Cannot read the push group (ungrouped): {}", e);
            None
        })
}

async fn push_audience(
    pool: &PgPool,
    notification_id: Option<uuid::Uuid>,
    tap: &crate::scheduler::notifications::Tap,
    link_event_id: Option<uuid::Uuid>,
) -> Result<crate::engine::thread_users::Audience, sqlx::Error> {
    use crate::engine::thread_users::{audience_of_notification, Audience};
    match notification_id {
        Some(id) => crate::scheduler::NotificationStore::audience(pool, id)
            .await
            .map(Audience::from_device_ids),
        None => {
            audience_of_notification(pool, tap, link_event_id, &crate::gateway_users::current())
                .await
        }
    }
}

struct ToastContent<'a> {
    notification_id: uuid::Uuid,
    title: &'a str,
    body: &'a str,
    thread_id: Option<uuid::Uuid>,
    event_id: Option<uuid::Uuid>,
    app_id: Option<&'a str>,
    tap: crate::scheduler::notifications::Tap,
    audience: Option<Vec<String>>,
}

async fn run_presence_check(
    event_bus: &crate::engine::event_bus::EventBus,
    presence_tracker: &crate::engine::presence_tracker::PresenceTracker,
    toast: ToastContent<'_>,
    expected_pongs: usize,
) -> bool {
    let notification_id = toast.notification_id;
    let sent_at_ms = crate::engine::now_epoch_millis();
    let notify = presence_tracker.expect(notification_id, expected_pongs);
    if let Err(e) = event_bus
        .emit(crate::engine::event_bus::BusEvent::System(
            crate::engine::event_bus::SystemEvent::PresenceCheck {
                notification_id,
                title: toast.title.to_string(),
                body: toast.body.to_string(),
                thread_id: toast.thread_id,
                event_id: toast.event_id,
                app_id: toast.app_id.map(str::to_string),
                tap: toast.tap,
                deadline_ms: DEADLINE_MS,
                sent_at_ms,
                audience: toast.audience,
                actor: None,
            },
        ))
        .await
    {
        log_legacy!("[Push] Failed to broadcast PresenceCheck: {}", e);
        return presence_tracker.decide(notification_id);
    }

    let _ = tokio::time::timeout(
        std::time::Duration::from_millis(DEADLINE_MS as u64),
        notify.notified(),
    )
    .await;

    presence_tracker.decide(notification_id)
}

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
    audience: Option<Vec<String>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let sent_at_ms = crate::engine::now_epoch_millis();
    event_bus
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
                audience,
            },
        ))
        .await?;
    Ok(())
}

pub async fn emit_native_push_dismiss_requested(
    event_bus: &crate::engine::event_bus::EventBus,
    notification_id: Option<uuid::Uuid>,
    reader: &crate::gateway_users::Reader,
) {
    let sent_at_ms = crate::engine::now_epoch_millis();
    if let Err(e) = event_bus
        .emit(crate::engine::event_bus::BusEvent::System(
            crate::engine::event_bus::SystemEvent::NativePushDismissRequested {
                notification_id,
                sent_at_ms,
                audience: reader.devices().map(<[String]>::to_vec),
            },
        ))
        .await
    {
        log_legacy!(
            "[Push] Failed to broadcast NativePushDismissRequested for {:?}: {}",
            notification_id,
            e
        );
    }
}

fn is_mac_chromium(user_agent: &str) -> bool {
    user_agent.contains("Macintosh")
        && user_agent.contains("Chrome/")
        && !user_agent.contains("iPhone")
        && !user_agent.contains("iPad")
        && !user_agent.contains("iPod")
}

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

fn wake_still_needed(notification: &crate::scheduler::notifications::Notification) -> bool {
    !notification.read
}

/// INVARIANT: Send a wake-push to a single device, the workaround for Chromium #370536109.
/// Returns how many subscriptions it delivered to, `0` when skipped.
///
/// The payload carries the SAME content as the original, so Chrome counts the
/// push as visible (§4.5). Sending a push is the canonical way to wake an
/// inactive service worker: any push event resurrects the worker thread and
/// drains queued `notificationclick` events as a side effect.
async fn send_wake_push_to_device(
    engine: &crate::engine::LucidosEngine,
    source_generation: Option<uuid::Uuid>,
    device_id: &str,
    notification_id: uuid::Uuid,
    endpoints: Option<&std::collections::BTreeSet<String>>,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let pool = engine.pool();
    // INVARIANT: This provenance was captured before scheduling the timer, never adopted
    // from the current workspace after its delay.
    let reader = crate::gateway_users::Reader::of(
        Some(&crate::engine::thread_events::MessageOrigin::Device {
            device_id: device_id.to_string(),
        }),
        &crate::gateway_users::current(),
    );
    let notification =
        match crate::scheduler::NotificationStore::get_by_id(pool, &reader, notification_id).await?
        {
            Some(n) => n,
            None => return Ok(0),
        };

    if !wake_still_needed(&notification) {
        return Ok(0);
    }

    let subscriptions: Vec<_> = PushSubscriptionStore::get_push_enabled_for_device(pool, device_id)
        .await?
        .into_iter()
        .filter(|sub| endpoints.is_none_or(|allowed| allowed.contains(&sub.endpoint)))
        .collect();
    if subscriptions.is_empty() {
        return Ok(0);
    }

    let unread_counts = unread_counts_by_device(pool, &subscriptions).await;
    let cross_workspace_totals = cross_workspace_unread_totals(&subscriptions).await;
    let push_group = read_push_group(pool, Some(notification_id)).await;
    let deliveries = badge_deliveries(
        subscriptions,
        &unread_counts,
        &cross_workspace_totals,
        |sub, badge| {
            build_wake_payload(
                source_generation,
                &notification,
                sub.scope_url.as_deref(),
                badge,
                push_group.as_ref(),
            )
        },
    );
    Ok(fan_out_payload(pool, deliveries, "wake", Some(notification_id)).await)
}

const DEFAULT_NOTIFICATION_TAG: &str = "lucidos-notification";

const DECLARATIVE_WEB_PUSH_MAGIC: i64 = 8030;

const MAX_PUSH_PAYLOAD_BYTES: usize = 3052;

const PUSH_PAYLOAD_SAFETY_MARGIN_BYTES: usize = 64;

const TRUNCATED_PAYLOAD_TARGET_BYTES: usize =
    MAX_PUSH_PAYLOAD_BYTES - PUSH_PAYLOAD_SAFETY_MARGIN_BYTES;

const TRUNCATION_MARKER: &str = "…";

const TRUNCATION_WHITESPACE_LOOKBACK: usize = 96;

fn truncate_body_to_bytes(body: &str, budget: usize) -> String {
    if body.len() <= budget {
        return body.to_string();
    }
    if budget < TRUNCATION_MARKER.len() {
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

/// INVARIANT: Build a push payload whose serialized form is guaranteed to fit
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

    let full = build(body);
    if full.to_string().len() <= MAX_PUSH_PAYLOAD_BYTES {
        return full;
    }

    let envelope_len = rendered_len("");

    let target = if envelope_len <= TRUNCATED_PAYLOAD_TARGET_BYTES {
        TRUNCATED_PAYLOAD_TARGET_BYTES
    } else {
        MAX_PUSH_PAYLOAD_BYTES
    };

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
        log_legacy!(
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
        log_legacy!(
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

fn build_navigate_params(
    source_generation: Option<uuid::Uuid>,
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
    if !parts.is_empty() {
        let generation = source_generation.map_or_else(|| "none".to_string(), |id| id.to_string());
        parts.push(format!("restore_generation={generation}"));
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
/// resume. See `system-knowhow/notifications/4-5-service-worker-wedge-mitigation-stac.md` §4.5.
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

/// INVARIANT: Chrome service-worker navigate URL: a **hash** URL, read off
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

async fn read_unread_count(
    pool: &sqlx::PgPool,
    reader: &crate::gateway_users::Reader,
) -> Option<i64> {
    match crate::scheduler::NotificationStore::count_unread(pool, reader).await {
        Ok(count) => Some(count),
        Err(e) => {
            log_legacy!("[Push] Failed to count unread (omitting app_badge): {}", e);
            None
        }
    }
}

async fn unread_counts_by_device(
    pool: &sqlx::PgPool,
    subscriptions: &[PushSubscription],
) -> std::collections::HashMap<(Option<String>, BadgeScope), Option<i64>> {
    let Some(snapshot) = crate::gateway_users::known() else {
        return Default::default();
    };
    let mut counts = std::collections::HashMap::new();
    for sub in subscriptions {
        let scope = badge_scope(sub.scope_url.as_deref());
        let key = (sub.device_id.clone(), scope);
        if counts.contains_key(&key) {
            continue;
        }
        let Some(reader) = badge_reader(sub.device_id.as_deref(), scope, &snapshot) else {
            counts.insert(key, None);
            continue;
        };
        counts.insert(key, read_unread_count(pool, &reader).await);
    }
    counts
}

/// INVARIANT: How long the engine waits for the gateway's cross-workspace unread total
/// before badging its own count. A push must never sit behind a badge
/// refinement, so this is shorter than `workspace_label`'s deadline: the hop
/// is one loopback request.
const GATEWAY_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum BadgeScope {
    Own,
    Gateway,
    Relayed,
}

/// INVARIANT: Classify a subscription's recorded service-worker scope.
///
/// `scope_url` is the SW scope the page captured, validated at subscribe time
/// against the forwarded prefix. It is `https://host/<slug>/` behind the
/// gateway and `https://host:<engine-port>/` on a direct engine, so a path
/// deeper than `/` means a gateway served the page. A first segment holding
/// the remote route separator means a remote gateway relayed it.
///
/// The gateway re-stamps every manifest it serves with `scope: "/"`, so ONE
/// installed icon covers the picker and every workspace on that origin.
fn badge_scope(scope_url: Option<&str>) -> BadgeScope {
    let Some(url) = scope_url.and_then(|raw| reqwest::Url::parse(raw.trim()).ok()) else {
        return BadgeScope::Own;
    };
    let first_segment = url.path().trim_start_matches('/').split('/').next();
    match first_segment {
        None | Some("") => BadgeScope::Own,
        Some(segment) if segment.contains(lucidos_gateway_constants::REMOTE_ROUTE_SEPARATOR) => {
            BadgeScope::Relayed
        }
        Some(_) => BadgeScope::Gateway,
    }
}

/// INVARIANT: The `app_badge` value one subscription should carry: the cross-workspace
/// total for a gateway-served install, this workspace's own unread count for
/// a direct one. `None` leaves the badge untouched, as for a relayed install.
///
/// The aggregate falls back to the own count rather than to nothing. An
/// unreachable gateway then leaves the icon slightly low, instead of frozen at
/// whatever the previous push wrote.
fn app_badge_for(
    scope_url: Option<&str>,
    own_unread: Option<i64>,
    cross_workspace_total: Option<i64>,
) -> Option<i64> {
    match badge_scope(scope_url) {
        BadgeScope::Own => own_unread,
        BadgeScope::Gateway => cross_workspace_total.or(own_unread),
        BadgeScope::Relayed => None,
    }
}

fn badge_deliveries(
    subscriptions: Vec<PushSubscription>,
    own: &std::collections::HashMap<(Option<String>, BadgeScope), Option<i64>>,
    gateway: &std::collections::HashMap<String, Option<i64>>,
    payload: impl Fn(&PushSubscription, Option<i64>) -> serde_json::Value,
) -> Vec<(PushSubscription, String)> {
    subscriptions
        .into_iter()
        .map(|sub| {
            let badge = app_badge_for(
                sub.scope_url.as_deref(),
                own.get(&(sub.device_id.clone(), badge_scope(sub.scope_url.as_deref())))
                    .copied()
                    .flatten(),
                sub.device_id
                    .as_ref()
                    .and_then(|id| gateway.get(id).copied().flatten()),
            );
            let body = payload(&sub, badge).to_string();
            (sub, body)
        })
        .collect()
}

fn badge_reader(
    device_id: Option<&str>,
    scope: BadgeScope,
    snapshot: &crate::gateway_users::UserSnapshot,
) -> Option<crate::gateway_users::Reader> {
    if scope == BadgeScope::Relayed
        || (scope == BadgeScope::Gateway
            && (snapshot.user_count() == 0 || device_id.is_none_or(|id| id.is_empty())))
    {
        return None;
    }
    let Some(device_id) = device_id.filter(|id| !id.is_empty()) else {
        return (snapshot.user_count() < 2).then_some(crate::gateway_users::Reader::Team);
    };
    if (scope == BadgeScope::Gateway || snapshot.user_count() >= 2)
        && snapshot.user_of_device(device_id).is_none()
    {
        return None;
    }
    let actor = crate::engine::thread_events::MessageOrigin::Device {
        device_id: device_id.into(),
    };
    Some(crate::gateway_users::Reader::of(Some(&actor), snapshot))
}

async fn cross_workspace_unread_totals(
    subscriptions: &[PushSubscription],
) -> std::collections::HashMap<String, Option<i64>> {
    gateway_unread_totals_at(
        subscriptions,
        crate::core::base_path::gateway_port().as_deref(),
    )
    .await
}

async fn gateway_unread_totals_at(
    subscriptions: &[PushSubscription],
    gateway_port: Option<&str>,
) -> std::collections::HashMap<String, Option<i64>> {
    let Some(port) = gateway_port else {
        return Default::default();
    };
    let recipients: std::collections::BTreeSet<_> = subscriptions
        .iter()
        .filter(|sub| badge_scope(sub.scope_url.as_deref()) == BadgeScope::Gateway)
        .filter_map(|sub| sub.device_id.as_deref().filter(|id| !id.is_empty()))
        .collect();
    futures::future::join_all(recipients.into_iter().map(|id| async move {
        let count =
            match tokio::time::timeout(GATEWAY_TOTAL_TIMEOUT, ask_gateway_unread_total(port, id))
                .await
            {
                Ok(count) => count,
                Err(_) => {
                    log_legacy!(
                        "[Push] gateway unread total timed out; badging the recipient's own count"
                    );
                    None
                }
            };
        (id.to_owned(), count)
    }))
    .await
    .into_iter()
    .collect()
}

async fn ask_gateway_unread_total(gateway_port: &str, recipient_device_id: &str) -> Option<i64> {
    let client = match crate::gateway_auth::client_builder().build() {
        Ok(client) => client,
        Err(e) => {
            log_legacy!(
                "[Push] could not build the gateway client ({}); badging this workspace's own count",
                e
            );
            return None;
        }
    };
    let url = format!(
        "{}/~/api/v1/control/unread-total",
        lucidos_local_token::client::origin(gateway_port)
    );
    let resp = match client
        .get(&url)
        .query(&[("recipient-device-id", recipient_device_id)])
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            log_legacy!(
                "[Push] gateway on :{} unreachable ({}); badging this workspace's own count",
                gateway_port,
                e
            );
            return None;
        }
    };
    if !resp.status().is_success() {
        log_legacy!(
            "[Push] gateway unread total returned {}; badging this workspace's own count",
            resp.status()
        );
        return None;
    }
    let Ok(body) = resp.text().await else {
        log_legacy!(
            "[Push] gateway unread total body was unreadable; badging this workspace's own count"
        );
        return None;
    };
    parse_unread_total(&body)
}

fn parse_unread_total(body: &str) -> Option<i64> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("total")?
        .as_i64()
}

/// INVARIANT: Build a `wake: true` push payload from a stored notification.
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
    source_generation: Option<uuid::Uuid>,
    notification: &crate::scheduler::notifications::Notification,
    ios_scope_url: Option<&str>,
    app_badge: Option<i64>,
    push_group: Option<&crate::scheduler::notifications::PushGroup>,
) -> serde_json::Value {
    fit_payload_body(
        &plain_text_body(&notification.message),
        "wake",
        |candidate_body| {
            let mut payload = build_push_payload(
                source_generation,
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
            if let Some(group) = push_group {
                group.apply(&mut payload);
            }
            payload
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn build_push_payload_fitted(
    source_generation: Option<uuid::Uuid>,
    title: &str,
    body: &str,
    notification_id: Option<uuid::Uuid>,
    app_id: Option<&str>,
    link_thread_id: Option<uuid::Uuid>,
    link_event_id: Option<uuid::Uuid>,
    tap: &crate::scheduler::notifications::Tap,
    ios_scope_url: Option<&str>,
    app_badge: Option<i64>,
    push_group: Option<&crate::scheduler::notifications::PushGroup>,
) -> serde_json::Value {
    fit_payload_body(body, "notification", |candidate_body| {
        let mut payload = build_push_payload(
            source_generation,
            title,
            candidate_body,
            notification_id,
            app_id,
            link_thread_id,
            link_event_id,
            tap,
            ios_scope_url,
            app_badge,
        );
        if let Some(group) = push_group {
            group.apply(&mut payload);
        }
        payload
    })
}

#[allow(clippy::too_many_arguments)]
fn build_push_payload(
    source_generation: Option<uuid::Uuid>,
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
    let params = build_navigate_params(
        source_generation,
        notification_id,
        link_thread_id,
        link_event_id,
        tap,
    );
    let navigate_ios = navigate_url_ios(&params, ios_scope_url);
    let navigate_sw = navigate_url_sw(&params);
    let tag = notification_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| DEFAULT_NOTIFICATION_TAG.to_string());

    let mut data = serde_json::Map::new();
    data.insert(
        "restore_generation".into(),
        serde_json::json!(source_generation),
    );
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
    // INVARIANT: HASH form, so the Chrome SW reads the engine-built URL straight off
    // `event.notification.data` for the cold `clients.openWindow()` path
    // instead of rebuilding it. Warm taps route via postMessage.
    data.insert("navigate".into(), serde_json::Value::String(navigate_sw));

    let mut payload = serde_json::json!({
        "web_push": DECLARATIVE_WEB_PUSH_MAGIC,
        "notification": {
            "title": title,
            "body": body,
            // INVARIANT: QUERY form: iOS Safari's declarative-push navigation needs a
            // cross-document URL to navigate an open window.
            "navigate": navigate_ios,
            "tag": tag,
            "data": serde_json::Value::Object(data),
        },
    });
    // INVARIANT: Top-level `app_badge`, a sibling of `web_push` rather than a field
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

#[cfg_attr(feature = "e2e-test-hooks", allow(dead_code))]
async fn fan_out_to_web_push(
    pool: &PgPool,
    deliveries: Vec<(PushSubscription, String)>,
    kind: &str,
) -> usize {
    let keys = match get_or_create_vapid_keys(pool).await {
        Ok(k) => k,
        Err(e) => {
            log_legacy!("[Push] Failed to get VAPID keys for {}: {}", kind, e);
            return 0;
        }
    };

    let client = match web_push::IsahcWebPushClient::new() {
        Ok(c) => c,
        Err(e) => {
            log_legacy!("[Push] Failed to create push client for {}: {}", kind, e);
            return 0;
        }
    };

    let (client, keys) = (&client, &keys);
    let outcome = send_each(&deliveries, kind, move |sub, payload_bytes| {
        let endpoint_label = endpoint_label(sub);
        let sub_info = web_push::SubscriptionInfo::new(&sub.endpoint, &sub.p256dh, &sub.auth);

        let sig = match web_push::VapidSignatureBuilder::from_pem(
            keys.private_key_pem.as_bytes(),
            &sub_info,
        ) {
            Ok(mut builder) => {
                // INVARIANT: FCM and Apple both require the VAPID `sub` claim, as a valid
                // mailto: or https: URL. Apple rejects localhost.
                builder.add_claim("sub", "mailto:push@lucidos.app");
                match builder.build() {
                    Ok(sig) => sig,
                    Err(e) => {
                        log_legacy!("[Push] Failed to build VAPID signature for {}: {}", kind, e);
                        return None;
                    }
                }
            }
            Err(e) => {
                log_legacy!("[Push] Failed to create VAPID builder for {}: {}", kind, e);
                return None;
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
                log_legacy!(
                    "[Push] NO {} push delivered to {} ({} B payload): the encrypted message \
                     could not be built: {}. The notification row and bell badge are \
                     unaffected, so nothing else surfaces this failure.",
                    kind,
                    endpoint_label,
                    payload_bytes.len(),
                    e
                );
                return None;
            }
        };

        use web_push::WebPushClient;
        Some(client.send(message))
    })
    .await;

    for endpoint in outcome.stale_endpoints {
        if let Err(e) = PushSubscriptionStore::unsubscribe(pool, &endpoint).await {
            log_legacy!("[Push] Failed to remove stale subscription: {}", e);
        }
    }

    outcome.delivered
}

/// INVARIANT: How long one subscription's send may take before the fan-out gives up on it.
///
/// The transport sets no deadline of its own: web-push documents its client's
/// `send` as "Never times out", and isahc bounds only the connect. A socket that
/// finished its TLS handshake and then went quiet would park the fan-out for
/// good. Generous, because a phone radio waking from idle is slow but not dead.
const PUSH_SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Default, PartialEq)]
struct SendOutcome {
    delivered: usize,
    stale_endpoints: Vec<String>,
}

/// INVARIANT: Send to every subscription at once, each bounded by [`PUSH_SEND_TIMEOUT`],
/// so the whole pass takes at most one deadline however many devices are quiet.
///
/// `send` builds one subscription's request and returns `None` when it could
/// not (it logs why). Generic over the transport so a test can hand it one
/// that never answers. A send that runs out of time is a failure for that
/// subscription only, and the subscription is kept: a silent socket says
/// nothing about whether the endpoint is still valid.
async fn send_each<'d, Fut, E>(
    deliveries: &'d [(PushSubscription, String)],
    kind: &str,
    mut send: impl FnMut(&'d PushSubscription, &'d str) -> Option<Fut>,
) -> SendOutcome
where
    Fut: std::future::Future<Output = Result<(), E>>,
    E: std::fmt::Display,
{
    let requests = deliveries
        .iter()
        .filter_map(|(sub, payload_bytes)| send(sub, payload_bytes).map(|request| (sub, request)))
        .map(|(sub, request)| async move {
            (sub, tokio::time::timeout(PUSH_SEND_TIMEOUT, request).await)
        });
    let results = futures::future::join_all(requests).await;

    let mut outcome = SendOutcome::default();
    for (sub, result) in results {
        let endpoint_label = endpoint_label(sub);
        match result {
            Ok(Ok(())) => {
                outcome.delivered += 1;
                log_legacy!("[Push] Sent {} to {}", kind, endpoint_label);
            }
            Ok(Err(e)) => {
                let err_str = e.to_string();
                if err_str.contains("410") || err_str.contains("Gone") {
                    log_legacy!(
                        "[Push] Subscription expired (410), will remove: {}",
                        endpoint_label
                    );
                    outcome.stale_endpoints.push(sub.endpoint.clone());
                } else {
                    log_legacy!(
                        "[Push] Failed to send {} to {}: {}",
                        kind,
                        endpoint_label,
                        e
                    );
                }
            }
            Err(_) => log_legacy!(
                "[Push] Gave up sending {} to {} after {:?} with no answer; \
                 keeping the subscription",
                kind,
                endpoint_label,
                PUSH_SEND_TIMEOUT
            ),
        }
    }
    outcome
}

fn endpoint_label(sub: &PushSubscription) -> &str {
    &sub.endpoint[..sub.endpoint.floor_char_boundary(60)]
}

#[cfg(feature = "e2e-test-hooks")]
async fn fan_out_to_push_log(
    pool: &PgPool,
    deliveries: Vec<(PushSubscription, String)>,
    kind: &str,
    notification_id: Option<uuid::Uuid>,
) -> usize {
    let Some(nid) = notification_id else {
        return 0;
    };
    let mut delivered = 0usize;
    for (sub, payload) in &deliveries {
        let endpoint_label = endpoint_label(sub);
        let Some(device_id) = sub.device_id.as_deref() else {
            continue;
        };
        match push_test_log::record(pool, device_id, nid, payload).await {
            Ok(()) => {
                delivered += 1;
                log_legacy!("[Push:test] Logged {} to {}", kind, endpoint_label);
            }
            Err(e) => {
                log_legacy!(
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
