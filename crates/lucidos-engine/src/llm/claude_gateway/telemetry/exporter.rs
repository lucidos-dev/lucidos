//! Reads the persisted gateway calls after a cursor, and posts them as OTLP
//! log records. A call is a `ContextCaptured` row with usage, sent as
//! `api_request`, or a `ModelCallFailed` row, sent as `api_error`.
//!
//! The cursor is a sequence in the event store. Each pass reads up to the
//! *committed horizon*, so a call whose sequence commits late is still
//! read (ADR 0364). The cursor advances only after a `2xx`, or past calls
//! no telemetry applies to: made before the current sign-in, or before its
//! policy asked for telemetry. So a restart neither drops nor resends
//! a record, and one sign-in's records never reach another gateway.
//!
//! An engine whose environment opts out flags the cursor row, at startup and
//! on each pass. The next pass without the opt-out skips everything up to the
//! horizon before it clears the flag. So a call made while opted out is
//! never sent, even one made after the last opted-out pass.

use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::header::CONTENT_TYPE;
use reqwest::StatusCode;
use sqlx::PgPool;
use uuid::Uuid;

use super::otlp;
use super::record::{log_record, unix_nanos, CallRow, Identity, Outcome};
use super::switch::{self, Destination, EngineEnv, Off, Target};
use crate::engine::agent_session::CHAT_ORIGINATING_EVENT_TYPES;
use crate::engine::event_bus::{committed_event_horizon, BusEvent, EmittedEvent, SystemEvent};
use crate::engine::thread_events::ThreadEvent;
use crate::llm::claude_gateway::account::ClaudeGatewayAccountStore;
use crate::llm::claude_gateway::token_endpoint::{excerpt, http_client};
use crate::llm::claude_gateway::{GatewayAccessFactory, GatewayOrigin};
use crate::llm::ProviderKind;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Records sent in one request at most.
const BATCH_LIMIT: i64 = 500;

/// How often the exporter sends what has gathered.
const EXPORT_INTERVAL: Duration = Duration::from_secs(5);

/// Attempts per batch, for a status the protocol says to retry.
const MAX_ATTEMPTS: u32 = 3;

/// The wait before the second attempt, doubled before the third.
const RETRY_STEP: Duration = Duration::from_secs(1);

/// The longest wait between failed passes.
const MAX_BACKOFF: Duration = Duration::from_secs(5 * 60);

/// What one pass left behind.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Pass {
    /// Everything up to the committed horizon is sent or skipped.
    Done,
    /// A full batch went out, and more may wait.
    More,
}

/// The stored cursor row.
struct Cursor {
    sent_through: i64,
    /// An opted-out engine ran since the cursor last moved.
    opted_out: bool,
}

/// One gateway call row as read.
struct StoredCall {
    row: CallRow,
    /// Made while telemetry applied: under the current sign-in, and since its
    /// policy asked for telemetry.
    sendable: bool,
}

/// One batch of gateway call rows as read.
struct Batch {
    /// The rows that read back, oldest first.
    calls: Vec<StoredCall>,
    /// The highest sequence fetched, whether or not its row read back.
    last_fetched: Option<i64>,
    /// The fetch hit the batch limit, so more rows may wait.
    full: bool,
}

/// How a collector answered a batch it did not take.
enum Sent {
    Delivered,
    /// A status that sending again cannot fix, such as `400` or `413`.
    Refused {
        status: StatusCode,
        why: String,
    },
}

pub(crate) struct Exporter {
    pool: PgPool,
    access: GatewayAccessFactory,
    engine_env: EngineEnv,
    retry_step: Duration,
}

impl Exporter {
    pub(crate) fn new(pool: PgPool, access: GatewayAccessFactory) -> Self {
        Self {
            pool,
            access,
            engine_env: switch::process_env,
            retry_step: RETRY_STEP,
        }
    }

    /// Flag the cursor row when the engine's environment opts out. Runs before
    /// any model call, so a run that stops before its first pass is covered.
    pub(crate) async fn record_opt_out(&self) -> Result<(), sqlx::Error> {
        if !switch::opted_out(self.engine_env) {
            return Ok(());
        }
        let horizon = committed_event_horizon(&self.pool).await?;
        sqlx::query(
            "INSERT INTO claude_gateway_telemetry_cursor (sent_through, opted_out)
             VALUES ($1, TRUE)
             ON CONFLICT (singleton) DO UPDATE SET opted_out = TRUE, updated_at = NOW()",
        )
        .bind(horizon)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Send, or skip, the calls after the cursor, one batch at most.
    pub(crate) async fn export_once(&self) -> Result<Pass, BoxError> {
        let horizon = committed_event_horizon(&self.pool).await?;
        let opted_out = switch::opted_out(self.engine_env);
        let Some(Cursor {
            sent_through: cursor,
            opted_out: ran_opted_out,
        }) = self.cursor_or_seed(horizon, opted_out).await?
        else {
            return Ok(Pass::Done);
        };
        if opted_out {
            return self
                .skip_to(cursor, horizon, &Off::OptedOut.to_string(), true)
                .await;
        }
        if ran_opted_out {
            return self
                .skip_to(
                    cursor,
                    horizon,
                    "they were made while the engine's environment opted out",
                    false,
                )
                .await;
        }
        if cursor >= horizon {
            return Ok(Pass::Done);
        }
        let Some(account) = ClaudeGatewayAccountStore::get(&self.pool).await? else {
            return self
                .skip_to(cursor, horizon, "Lucidos is not signed in", false)
                .await;
        };
        let Ok(origin) = GatewayOrigin::parse(&account.issuer) else {
            return self
                .skip_to(
                    cursor,
                    horizon,
                    "the sign-in's issuer is not a gateway origin",
                    false,
                )
                .await;
        };
        if account.managed_settings_fetched_at.is_none() {
            // The policy that decides is still on its way. Its arrival
            // announces itself and wakes the exporter again.
            return Ok(Pass::Done);
        }
        let policy = account.managed_settings();
        let target = match switch::target(&origin, policy.as_ref(), self.engine_env) {
            Ok(target) => target,
            Err(off) => return self.skip_to(cursor, horizon, &off.to_string(), false).await,
        };

        let Batch {
            calls,
            last_fetched,
            full,
        } = self
            .read(cursor, horizon, account.managed_telemetry_since)
            .await?;
        let through = match last_fetched {
            Some(last) if full => last,
            _ => horizon,
        };
        let unsendable = calls.iter().filter(|c| !c.sendable).count();
        if unsendable > 0 {
            crate::log!(
                "[ClaudeGateway] Not sending {unsendable} telemetry records made before this \
                 sign-in or while telemetry was off"
            );
        }
        let to_send: Vec<&CallRow> = calls
            .iter()
            .filter(|c| c.sendable)
            .map(|c| &c.row)
            .collect();
        if !to_send.is_empty() {
            let access = (self.access)(origin);
            let encode = |bearer: &str| {
                let identity = Identity::of_bearer(bearer);
                let records: Vec<_> = to_send
                    .iter()
                    .map(|row| log_record(row, policy.as_ref(), &identity))
                    .collect();
                otlp::encode(target.protocol, &records, unix_nanos(Utc::now()))
            };
            let mut bearer = access.bearer().await?;
            let mut sent = send(&target, &bearer, encode(&bearer), self.retry_step).await?;
            if matches!(&sent, Sent::Refused { status, .. } if *status == StatusCode::UNAUTHORIZED)
                && target.destination == Destination::Gateway
            {
                bearer = access.renew_rejected(&bearer).await?;
                sent = send(&target, &bearer, encode(&bearer), self.retry_step).await?;
            }
            match sent {
                Sent::Delivered => crate::log!(
                    "[ClaudeGateway] Sent {} telemetry records to {}",
                    to_send.len(),
                    target.url
                ),
                Sent::Refused { status, why } => crate::log!(
                    "[ClaudeGateway] Dropping {} telemetry records that {} refused with {status}: {why}",
                    to_send.len(),
                    target.url
                ),
            }
        }
        self.store_cursor(through, false).await?;
        Ok(if full { Pass::More } else { Pass::Done })
    }

    /// The stored cursor. On the first run it is seeded at `horizon`, flagged
    /// when `opted_out`, and `None` returned: calls made before this
    /// exporter existed are not sent.
    async fn cursor_or_seed(
        &self,
        horizon: i64,
        opted_out: bool,
    ) -> Result<Option<Cursor>, sqlx::Error> {
        let stored: Option<(i64, bool)> =
            sqlx::query_as("SELECT sent_through, opted_out FROM claude_gateway_telemetry_cursor")
                .fetch_optional(&self.pool)
                .await?;
        if let Some((sent_through, opted_out)) = stored {
            return Ok(Some(Cursor {
                sent_through,
                opted_out,
            }));
        }
        sqlx::query(
            "INSERT INTO claude_gateway_telemetry_cursor (sent_through, opted_out)
             VALUES ($1, $2)
             ON CONFLICT (singleton) DO NOTHING",
        )
        .bind(horizon)
        .bind(opted_out)
        .execute(&self.pool)
        .await?;
        Ok(None)
    }

    /// Move the cursor to `through` and set the opt-out flag to `opted_out`,
    /// in one statement: a cleared flag always comes with the skip it covers.
    async fn store_cursor(&self, through: i64, opted_out: bool) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE claude_gateway_telemetry_cursor
             SET sent_through = GREATEST(sent_through, $1), opted_out = $2,
                 updated_at = NOW()",
        )
        .bind(through)
        .bind(opted_out)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Move the cursor past every call up to `horizon` unsent, logging how
    /// many and `why`. The opt-out flag is left as `opted_out`.
    async fn skip_to(
        &self,
        cursor: i64,
        horizon: i64,
        why: &str,
        opted_out: bool,
    ) -> Result<Pass, BoxError> {
        let skipped: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM events WHERE {GATEWAY_CALLS}"
        ))
        .bind(cursor)
        .bind(horizon)
        .bind(ProviderKind::ClaudeGateway.as_str())
        .fetch_one(&self.pool)
        .await?;
        if skipped > 0 {
            crate::log!("[ClaudeGateway] Not sending {skipped} telemetry records: {why}");
        }
        self.store_cursor(horizon, opted_out).await?;
        Ok(Pass::Done)
    }

    /// The gateway calls in `(cursor, horizon]`, oldest first, one batch
    /// at most. Only those made at or after `telemetry_since` are sendable.
    ///
    /// Each row's turn comes with it: the `request_event_id` it was written
    /// under, else the thread's newest origin event before it. A turn's row
    /// keeps its own origin even when a newer message is queued behind it
    /// (`engine::in_flight_request_event_id`).
    async fn read(
        &self,
        cursor: i64,
        horizon: i64,
        telemetry_since: Option<DateTime<Utc>>,
    ) -> Result<Batch, sqlx::Error> {
        let rows: Vec<StoredRow> = sqlx::query_as(&format!(
            "SELECT sequence, thread_id, created, event_type, payload,
                    COALESCE(
                        NULLIF(payload->>'request_event_id', ''),
                        (SELECT origin.id::text FROM events origin
                         WHERE origin.thread_id = events.thread_id
                           AND origin.sequence < events.sequence
                           AND origin.event_type = ANY($5)
                         ORDER BY origin.sequence DESC LIMIT 1)
                    ) AS prompt_id
             FROM events
             WHERE {GATEWAY_CALLS}
             ORDER BY sequence
             LIMIT $4"
        ))
        .bind(cursor)
        .bind(horizon)
        .bind(ProviderKind::ClaudeGateway.as_str())
        .bind(BATCH_LIMIT)
        .bind(CHAT_ORIGINATING_EVENT_TYPES)
        .fetch_all(&self.pool)
        .await?;
        Ok(Batch {
            last_fetched: rows.last().map(|row| row.sequence),
            full: rows.len() as i64 == BATCH_LIMIT,
            calls: rows
                .into_iter()
                .filter_map(|stored| {
                    let created = stored.created;
                    let row = call_row(stored)?;
                    Some(StoredCall {
                        sendable: telemetry_since.is_some_and(|since| created >= since),
                        row,
                    })
                })
                .collect(),
        })
    }
}

/// The gateway calls in `($1, $2]`, served by provider `$3`: a capture with a
/// usage block (one without reported nothing to count) or a failed call.
const GATEWAY_CALLS: &str = "sequence > $1 AND sequence <= $2
     AND payload->>'provider' = $3
     AND ((event_type = 'ContextCaptured' AND payload->'usage' IS NOT NULL)
          OR event_type = 'ModelCallFailed')";

/// One event row as `read` selects it.
#[derive(sqlx::FromRow)]
struct StoredRow {
    sequence: i64,
    thread_id: Option<Uuid>,
    created: DateTime<Utc>,
    event_type: String,
    payload: serde_json::Value,
    prompt_id: Option<String>,
}

fn call_row(stored: StoredRow) -> Option<CallRow> {
    let StoredRow {
        sequence,
        thread_id,
        created,
        event_type,
        payload,
        prompt_id,
    } = stored;
    let prompt_id = prompt_id.and_then(|id| Uuid::parse_str(&id).ok());
    match ThreadEvent::from_stored(&event_type, payload) {
        Ok(ThreadEvent::ContextCaptured {
            model,
            usage: Some(usage),
            duration_ms,
            purpose,
            reasoning_effort,
            ttft_ms,
            request_id,
            ..
        }) => Some(CallRow {
            sequence,
            thread_id,
            created,
            model,
            duration_ms,
            purpose,
            reasoning_effort,
            prompt_id,
            request_id,
            outcome: Outcome::Completed { usage, ttft_ms },
        }),
        Ok(ThreadEvent::ModelCallFailed {
            model,
            purpose,
            reasoning_effort,
            duration_ms,
            attempts,
            failure,
            status_code,
            request_id,
            ..
        }) => Some(CallRow {
            sequence,
            thread_id,
            created,
            model,
            duration_ms: Some(duration_ms),
            purpose,
            reasoning_effort,
            prompt_id,
            request_id,
            outcome: Outcome::Failed {
                failure,
                status_code,
                attempts,
            },
        }),
        Ok(_) => None,
        Err(e) => {
            crate::log!("[ClaudeGateway] Call {sequence} does not read back, skipping it: {e}");
            None
        }
    }
}

/// Post `body` to `target`. The bearer goes only to the gateway; a collector
/// on another host gets its own headers. `429` and gateway errors retry. A
/// client error that sending again cannot fix is [`Sent::Refused`], as the
/// OTLP/HTTP specification says to drop such a batch.
async fn send(
    target: &Target,
    bearer: &str,
    body: Vec<u8>,
    retry_step: Duration,
) -> Result<Sent, BoxError> {
    let mut last_failure = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        let mut request = http_client()
            .post(&target.url)
            .header(CONTENT_TYPE, target.protocol.content_type())
            .body(body.clone());
        request = match &target.destination {
            Destination::Gateway => request.bearer_auth(bearer),
            Destination::Collector { headers } => {
                headers.iter().fold(request, |request, (name, value)| {
                    request.header(name, value)
                })
            }
        };
        match request.send().await {
            Ok(resp) if resp.status().is_success() => return Ok(Sent::Delivered),
            Ok(resp) => {
                let status = resp.status();
                let reply = resp.text().await.unwrap_or_default();
                if status.is_client_error() && !matches!(status.as_u16(), 408 | 429) {
                    return Ok(Sent::Refused {
                        status,
                        why: excerpt(&reply).to_string(),
                    });
                }
                last_failure = format!("{} answered {status}: {}", target.url, excerpt(&reply));
                if !matches!(status.as_u16(), 408 | 429 | 502 | 503 | 504) {
                    break;
                }
            }
            Err(e) => last_failure = format!("could not reach {}: {e}", target.url),
        }
        if attempt < MAX_ATTEMPTS {
            tokio::time::sleep(retry_step * 2u32.pow(attempt - 1)).await;
        }
    }
    Err(format!("Usage telemetry was not sent: {last_failure}").into())
}

/// Export gateway telemetry in the background: about every 5 s while calls
/// wait, backing off while sending fails. An opt-out is on the cursor row
/// before this returns, so callers must await it before any model call.
pub(crate) async fn spawn_exporter(
    rx: tokio::sync::broadcast::Receiver<EmittedEvent>,
    pool: PgPool,
    access: GatewayAccessFactory,
) {
    let exporter = Exporter::new(pool, access);
    if let Err(e) = exporter.record_opt_out().await {
        crate::log!("[ClaudeGateway] Could not record the telemetry opt-out: {e}");
    }
    spawn(exporter, rx);
}

fn spawn(exporter: Exporter, mut rx: tokio::sync::broadcast::Receiver<EmittedEvent>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(EXPORT_INTERVAL);
        let mut pending = true;
        let mut failures = 0u32;
        let mut retry_at = tokio::time::Instant::now();
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                received = rx.recv() => {
                    match received {
                        Ok(emitted) => pending |= wakes_the_exporter(&emitted.typed),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => pending = true,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                    continue;
                }
            }
            if !pending || tokio::time::Instant::now() < retry_at {
                continue;
            }
            match exporter.export_once().await {
                Ok(pass) => {
                    pending = pass == Pass::More;
                    failures = 0;
                }
                Err(e) => {
                    failures += 1;
                    let backoff =
                        (EXPORT_INTERVAL * 2u32.saturating_pow(failures)).min(MAX_BACKOFF);
                    retry_at = tokio::time::Instant::now() + backoff;
                    crate::log!(
                        "[ClaudeGateway] {e}. Trying again in {}s",
                        backoff.as_secs()
                    );
                }
            }
        }
    });
}

/// A gateway call to send, or a sign-in or policy change that decides
/// what happens to unsent ones.
fn wakes_the_exporter(event: &BusEvent) -> bool {
    match event {
        BusEvent::Thread {
            event:
                ThreadEvent::ContextCaptured { provider, .. }
                | ThreadEvent::ModelCallFailed { provider, .. },
            ..
        } => provider.as_deref() == Some(ProviderKind::ClaudeGateway.as_str()),
        BusEvent::System(
            SystemEvent::ClaudeGatewaySignedIn { .. }
            | SystemEvent::ClaudeGatewaySignedOut { .. }
            | SystemEvent::ClaudeGatewayManagedSettingsChanged { .. },
        ) => true,
        _ => false,
    }
}

#[cfg(test)]
#[path = "exporter_tests.rs"]
mod tests;
